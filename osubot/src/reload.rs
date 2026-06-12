use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use notify::event::{CreateKind, EventKind};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{error, info, warn};

use crate::config::{Config, MutableConfig, UpstreamConfig};
use crate::scheduler::Scheduler;
use crate::xfs_upstream::XfsUpstream;
use crate::yumu_upstream::YumuUpstream;
use osubot_core::{OauthTokenCache, RateLimiter, UpstreamBindingProvider, UpstreamChain};
use osubot_plugin::PluginManager;

pub struct ReloadHandle {
    pub config: Arc<RwLock<Config>>,
    pub pm: Arc<Mutex<Option<PluginManager>>>,
    pub drain: Arc<AtomicBool>,
    pub in_flight: Arc<AtomicUsize>,
    pub onebot_api_timeout: Arc<AtomicU64>,
    pub upstream_chain: Arc<RwLock<UpstreamChain>>,
    pub oauth: Arc<OauthTokenCache>,
    pub rate_limiter: Arc<RateLimiter>,
    pub force_reconnect: Arc<AtomicBool>,
    pub scheduler: Scheduler,
}

impl ReloadHandle {
    pub fn new(
        config: Arc<RwLock<Config>>,
        pm: Arc<Mutex<Option<PluginManager>>>,
        onebot_api_timeout: Arc<AtomicU64>,
        upstream_chain: Arc<RwLock<UpstreamChain>>,
        oauth: Arc<OauthTokenCache>,
        rate_limiter: Arc<RateLimiter>,
        scheduler: Scheduler,
    ) -> Self {
        Self {
            config,
            pm,
            drain: Arc::new(AtomicBool::new(false)),
            in_flight: Arc::new(AtomicUsize::new(0)),
            onebot_api_timeout,
            upstream_chain,
            oauth,
            rate_limiter,
            force_reconnect: Arc::new(AtomicBool::new(false)),
            scheduler,
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

        self.handle.drain.store(true, Ordering::SeqCst);
        self.wait_drain().await;

        match self.reload_config(watcher, plugin_dir).await {
            Ok(()) => {
                if let Err(e) = self.reload_plugins().await {
                    error!("插件重载失败: {e}");
                }
            }
            Err(e) => {
                error!("配置重载失败，保留旧配置并跳过插件重载: {e}");
            }
        }

        self.handle.drain.store(false, Ordering::SeqCst);
    }

    async fn reload_config(
        &self,
        watcher: &mut RecommendedWatcher,
        plugin_dir: &Arc<std::sync::RwLock<PathBuf>>,
    ) -> Result<(), String> {
        let old_onebot_url = {
            let old = self.handle.config.read().await;
            old.bot.onebot_url.clone()
        };

        let content = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| format!("读取配置文件失败: {e}"))?;

        let mutable: MutableConfig =
            toml::from_str(&content).map_err(|e| format!("TOML 解析失败: {e}"))?;

        let new_config = {
            let old = self.handle.config.read().await;
            Config {
                osu: old.osu.clone(),
                bot: mutable.bot,
                database: old.database.clone(),
                irc: old.irc.clone(),
                scheduler: mutable.scheduler,
                group_filter: mutable.group_filter,
                groups: mutable.groups,
                upstream: mutable.upstream,
                plugin: mutable.plugin,
            }
        };

        // validate
        if new_config.bot.onebot_url.is_empty() {
            return Err("onebot_url 为空".to_string());
        }
        if new_config.bot.command_timeout_secs < 5 {
            return Err(format!(
                "command_timeout_secs 过小（{} < 5 秒）",
                new_config.bot.command_timeout_secs
            ));
        }
        if new_config.bot.render_timeout_secs < 5 {
            return Err(format!(
                "render_timeout_secs 过小（{} < 5 秒）",
                new_config.bot.render_timeout_secs
            ));
        }
        if new_config.bot.onebot_api_timeout_secs < 2 {
            return Err(format!(
                "onebot_api_timeout_secs 过小（{} < 2 秒）",
                new_config.bot.onebot_api_timeout_secs
            ));
        }
        if new_config.bot.ur_timeout_secs < 3 {
            return Err(format!(
                "ur_timeout_secs 过小（{} < 3 秒）",
                new_config.bot.ur_timeout_secs
            ));
        }

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

        let onebot_api_timeout_secs = new_config.bot.onebot_api_timeout_secs;
        let new_onebot_url = new_config.bot.onebot_url.clone();
        let upstream_config = new_config.upstream.clone();

        {
            let mut cfg = self.handle.config.write().await;
            *cfg = new_config;
        }

        self.handle
            .onebot_api_timeout
            .store(onebot_api_timeout_secs, Ordering::Relaxed);

        if old_onebot_url != new_onebot_url {
            info!(
                old = %old_onebot_url,
                new = %new_onebot_url,
                "onebot_url 已变更，触发重连"
            );
            self.handle.force_reconnect.store(true, Ordering::SeqCst);
        }

        self.handle.scheduler.reschedule_all().await;

        let new_chain = build_upstream_chain(
            &upstream_config,
            &self.handle.oauth,
            &self.handle.rate_limiter,
        );
        {
            let mut chain = self.handle.upstream_chain.write().await;
            *chain = new_chain;
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

fn build_upstream_chain(
    upstream: &UpstreamConfig,
    oauth: &Arc<OauthTokenCache>,
    rate_limiter: &Arc<RateLimiter>,
) -> UpstreamChain {
    if upstream.enabled {
        let mut providers: Vec<Box<dyn UpstreamBindingProvider>> = Vec::new();
        for p_cfg in &upstream.providers {
            match p_cfg.provider_type.as_str() {
                "xfs" => {
                    providers.push(Box::new(XfsUpstream::from_config(
                        p_cfg,
                        oauth.clone(),
                        rate_limiter.clone(),
                    )));
                }
                "yumu" => {
                    providers.push(Box::new(YumuUpstream::from_config(p_cfg)));
                }
                other => {
                    warn!("unknown upstream provider type: {other}");
                }
            }
        }
        UpstreamChain::new(providers)
    } else {
        UpstreamChain::new(Vec::new())
    }
}
