use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use notify::event::{CreateKind, EventKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{error, info, warn};

use crate::config::{Config, MutableConfig};
use osubot_plugin::PluginManager;

pub struct ReloadHandle {
    pub config: Arc<RwLock<Config>>,
    pub pm: Arc<Mutex<Option<PluginManager>>>,
    pub drain: Arc<AtomicBool>,
    pub in_flight: Arc<AtomicUsize>,
}

impl ReloadHandle {
    pub fn new(config: Arc<RwLock<Config>>, pm: Arc<Mutex<Option<PluginManager>>>) -> Self {
        Self {
            config,
            pm,
            drain: Arc::new(AtomicBool::new(false)),
            in_flight: Arc::new(AtomicUsize::new(0)),
        }
    }
}

pub struct ReloadCoordinator {
    handle: ReloadHandle,
    config_path: PathBuf,
    plugin_dir: Arc<std::sync::RwLock<PathBuf>>,
}

impl ReloadCoordinator {
    pub fn new(handle: ReloadHandle, config_path: PathBuf, plugin_dir: PathBuf) -> Self {
        Self {
            handle,
            config_path,
            plugin_dir: Arc::new(std::sync::RwLock::new(plugin_dir)),
        }
    }

    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                error!("ReloadCoordinator 致命错误: {e}");
            }
        })
    }

    async fn run(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (tx, mut rx) = mpsc::channel::<()>(16);

        let config_path = self.config_path.clone();
        let plugin_dir: Arc<std::sync::RwLock<PathBuf>> = self.plugin_dir.clone();
        let tx_clone = tx.clone();

        let mut watcher = RecommendedWatcher::new(
            move |event: Result<Event, notify::Error>| {
                if let Ok(event) = event {
                    let current_dir = plugin_dir.read().unwrap_or_else(|e| e.into_inner());
                    let is_config = event.paths.iter().any(|p| p == &config_path);
                    let is_wasm = event.paths.iter().any(|p| {
                        p.extension().map(|e| e == "wasm").unwrap_or(false)
                            && p.starts_with(&*current_dir)
                    });
                    let is_relevant = matches!(
                        event.kind,
                        EventKind::Modify(_) | EventKind::Create(CreateKind::File)
                    );
                    if (is_config || is_wasm) && is_relevant {
                        let _ = tx_clone.try_send(());
                    }
                }
            },
            notify::Config::default(),
        )?;

        let initial_dir = self
            .plugin_dir
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        watcher.watch(&self.config_path, RecursiveMode::NonRecursive)?;
        watcher.watch(&initial_dir, RecursiveMode::NonRecursive)?;

        info!(
            "文件监控已启动: config={}, plugins={}",
            self.config_path.display(),
            initial_dir.display()
        );

        while let Some(()) = rx.recv().await {
            let mut deadline = tokio::time::Instant::now() + Duration::from_millis(500);
            loop {
                match tokio::time::timeout_at(deadline, rx.recv()).await {
                    Ok(Some(())) => {
                        deadline = tokio::time::Instant::now() + Duration::from_millis(500);
                        continue;
                    }
                    Ok(None) => return Ok(()),
                    Err(_) => {
                        self.reload(&mut watcher, &self.plugin_dir).await;
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn reload(
        &self,
        watcher: &mut RecommendedWatcher,
        plugin_dir: &Arc<std::sync::RwLock<PathBuf>>,
    ) {
        info!("收到文件变更，开始热重载...");

        // 设置全局 drain，config 和 plugin 在同一个 drain 窗口内原子完成
        self.handle.drain.store(true, Ordering::SeqCst);
        self.wait_drain().await;

        // 配置重载
        if let Err(e) = self.reload_config(watcher, plugin_dir).await {
            error!("配置重载失败，保留旧配置: {e}");
        }

        // 插件重载
        if let Err(e) = self.reload_plugins().await {
            error!("插件重载失败: {e}");
        }

        self.handle.drain.store(false, Ordering::SeqCst);
    }

    async fn reload_config(
        &self,
        watcher: &mut RecommendedWatcher,
        plugin_dir: &Arc<std::sync::RwLock<PathBuf>>,
    ) -> Result<(), String> {
        let content = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| format!("读取配置文件失败: {e}"))?;

        let mutable: MutableConfig =
            toml::from_str(&content).map_err(|e| format!("TOML 解析失败: {e}"))?;

        let new_config = {
            let old = self.handle.config.read().await;
            Config {
                osu: old.osu.clone(),
                bot: old.bot.clone(),
                database: old.database.clone(),
                irc: old.irc.clone(),
                scheduler: mutable.scheduler,
                group_filter: mutable.group_filter,
                groups: mutable.groups,
                upstream: mutable.upstream,
                plugin: mutable.plugin,
            }
        };

        // 基本校验
        if new_config.bot.onebot_url.is_empty() {
            return Err("onebot_url 为空".to_string());
        }

        // 更新插件目录监控（如果目录已变更）
        let new_plugin_dir = std::path::PathBuf::from(&new_config.plugin.dir);
        {
            let mut cur_dir = plugin_dir.write().unwrap_or_else(|e| e.into_inner());
            if *cur_dir != new_plugin_dir {
                watcher
                    .unwatch(cur_dir.as_path())
                    .map_err(|e| format!("unwatch old plugin dir failed: {e}"))?;
                std::fs::create_dir_all(&new_plugin_dir).ok();
                watcher
                    .watch(&new_plugin_dir, RecursiveMode::NonRecursive)
                    .map_err(|e| format!("watch new plugin dir failed: {e}"))?;
                info!(
                    "插件目录监控已更新: {} → {}",
                    cur_dir.display(),
                    new_plugin_dir.display()
                );
                *cur_dir = new_plugin_dir;
            }
        }

        {
            let mut cfg = self.handle.config.write().await;
            *cfg = new_config;
        }

        info!("配置热重载成功");
        Ok(())
    }

    async fn reload_plugins(&self) -> Result<(), String> {
        let plugin_config = {
            let cfg = self.handle.config.read().await;
            cfg.plugin.clone()
        };

        {
            let mut guard = self.handle.pm.lock().await;
            if let Some(ref mut pm) = *guard {
                pm.reload_all(&plugin_config).await?;
            } else if plugin_config.instances.iter().any(|p| p.enabled) {
                warn!(
                    "PluginManager 未初始化（可能在启动时未启用插件），已启用插件将在下次 OneBot 重连后加载"
                );
            }
        }

        info!("插件热重载成功");
        Ok(())
    }

    async fn wait_drain(&self) {
        let start = tokio::time::Instant::now();
        let timeout = Duration::from_secs(30);
        loop {
            let count = self.handle.in_flight.load(Ordering::SeqCst);
            if count == 0 {
                return;
            }
            if start.elapsed() >= timeout {
                warn!(in_flight = count, "等待进行中任务超时 (30s)，强制切换");
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
