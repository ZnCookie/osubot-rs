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
use osubot_core::irc::{IrcClient, IrcConfig as CoreIrcConfig};
use osubot_core::{OauthTokenCache, RateLimiter, UpstreamBindingProvider, UpstreamChain};
use osubot_plugin::PluginManager;

/// 热重载 drain 超时（秒），超时后强制切换不再等待进行中任务
const DRAIN_TIMEOUT_SECS: u64 = 30;

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
    pub irc_handle: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub irc_tx: Option<mpsc::Sender<osubot_core::irc::IrcPrivateMessage>>,
}

impl ReloadHandle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: Arc<RwLock<Config>>,
        pm: Arc<Mutex<Option<PluginManager>>>,
        onebot_api_timeout: Arc<AtomicU64>,
        upstream_chain: Arc<RwLock<UpstreamChain>>,
        oauth: Arc<OauthTokenCache>,
        rate_limiter: Arc<RateLimiter>,
        scheduler: Scheduler,
        irc_handle: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
        irc_tx: Option<mpsc::Sender<osubot_core::irc::IrcPrivateMessage>>,
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
            irc_handle,
            irc_tx,
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
        let (old_onebot_url, old_osu, old_irc) = {
            let old = self.handle.config.read().await;
            (old.bot.onebot_url.clone(), old.osu.clone(), old.irc.clone())
        };

        let content = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| format!("读取配置文件失败: {e}"))?;

        let mutable: MutableConfig =
            toml::from_str(&content).map_err(|e| format!("TOML 解析失败: {e}"))?;

        let new_osu = mutable.osu.unwrap_or(old_osu.clone());
        let new_irc = mutable.irc.unwrap_or(old_irc.clone());

        let new_config = {
            let old = self.handle.config.read().await;
            Config {
                osu: new_osu.clone(),
                bot: mutable.bot,
                database: old.database.clone(),
                irc: new_irc.clone(),
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
        if new_config.bot.command_timeout_secs > 3600 {
            return Err(format!(
                "command_timeout_secs 过大（{} > 3600 秒）",
                new_config.bot.command_timeout_secs
            ));
        }
        if new_config.bot.render_timeout_secs > 600 {
            return Err(format!(
                "render_timeout_secs 过大（{} > 600 秒）",
                new_config.bot.render_timeout_secs
            ));
        }
        if new_config.bot.onebot_api_timeout_secs > 120 {
            return Err(format!(
                "onebot_api_timeout_secs 过大（{} > 120 秒）",
                new_config.bot.onebot_api_timeout_secs
            ));
        }
        if new_config.bot.ur_timeout_secs > 300 {
            return Err(format!(
                "ur_timeout_secs 过大（{} > 300 秒）",
                new_config.bot.ur_timeout_secs
            ));
        }
        if new_config.scheduler.interval_minutes > 1440 {
            return Err(format!(
                "scheduler.interval_minutes 过大（{} > 1440 分钟 = 1 天）",
                new_config.scheduler.interval_minutes
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

        // osu! API 凭据变更：更新 OAuth 缓存，旧 token 自动失效
        if new_osu.client_id != old_osu.client_id || new_osu.client_secret != old_osu.client_secret
        {
            info!("osu! API 凭据已变更，正在更新 OAuth 缓存");
            self.handle
                .oauth
                .update_credentials(new_osu.client_id, new_osu.client_secret)
                .await;
        }

        // IRC 配置变更：重启 IRC 连接
        if new_irc.enabled != old_irc.enabled
            || new_irc.server != old_irc.server
            || new_irc.port != old_irc.port
            || new_irc.nickname != old_irc.nickname
            || new_irc.password != old_irc.password
        {
            info!("IRC 配置已变更，正在重启连接");
            self.restart_irc(&new_irc).await;
        }

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
        let timeout = Duration::from_secs(DRAIN_TIMEOUT_SECS);
        loop {
            let count = self.handle.in_flight.load(Ordering::SeqCst);
            if count == 0 {
                return;
            }
            if start.elapsed() >= timeout {
                warn!(
                    in_flight = count,
                    "等待进行中任务超时 ({DRAIN_TIMEOUT_SECS}s)，强制切换"
                );
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn restart_irc(&self, irc_cfg: &crate::config::IrcConfig) {
        let mut guard = self.handle.irc_handle.lock().unwrap();
        if let Some(old) = guard.take() {
            old.abort();
        }

        if !irc_cfg.enabled {
            info!("IRC 已禁用，连接已关闭");
            return;
        }

        if irc_cfg.nickname.is_empty() || irc_cfg.password.is_empty() {
            warn!("IRC 已启用但 nickname/password 为空，跳过重连");
            return;
        }

        if let Some(ref tx) = self.handle.irc_tx {
            let client_config = CoreIrcConfig::new(
                irc_cfg.enabled,
                &irc_cfg.server,
                irc_cfg.port,
                &irc_cfg.nickname,
                &irc_cfg.password,
            );
            let client = IrcClient::new(client_config, tx.clone());
            *guard = Some(tokio::spawn(async move {
                if let Err(e) = client.run().await {
                    error!(error = %e, "IRC client error");
                }
            }));
            info!("IRC 连接已重启");
        }
    }
}

pub(crate) fn build_upstream_chain(
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
