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
use osubot_core::log_fmt;
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
                error!("{}", log_fmt!("reload.coordinator_fatal", error = &e));
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
            "{}",
            log_fmt!(
                "reload.file_watch_started",
                config = self.config_path.display(),
                plugins = initial_dir.display()
            )
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
        info!("{}", log_fmt!("reload.file_change_detected"));

        self.handle.drain.store(true, Ordering::SeqCst);
        let drained = self.wait_drain().await;

        let old_config = self.handle.config.read().await.clone();

        let new_config = match self.reload_config(watcher, plugin_dir, &old_config).await {
            Ok(cfg) => cfg,
            Err(e) => {
                error!("{}", log_fmt!("reload.config_reload_failed", error = &e));
                self.handle.drain.store(false, Ordering::SeqCst);
                return;
            }
        };

        if !drained {
            error!("{}", log_fmt!("reload.drain_timeout"));
            self.apply_side_effects(&old_config, &new_config).await;
            *self.handle.config.write().await = new_config.clone();
            self.handle.drain.store(false, Ordering::SeqCst);
            return;
        }

        if let Err(e) = self.reload_plugins().await {
            error!(
                "{}",
                log_fmt!("reload.plugin_reload_failed_rollback", error = &e)
            );
            // 即使插件失败，也要应用 side effects（如 IRC 重连）
            // apply_side_effects 比较 old_config 和 new_config，用 new_config 的值触发副作用
            self.apply_side_effects(&old_config, &new_config).await;
            *self.handle.config.write().await = old_config;
            self.handle.drain.store(false, Ordering::SeqCst);
            return;
        }

        // 插件重载成功，写入新配置
        *self.handle.config.write().await = new_config.clone();
        self.apply_side_effects(&old_config, &new_config).await;

        self.handle.drain.store(false, Ordering::SeqCst);
    }

    async fn reload_config(
        &self,
        watcher: &mut RecommendedWatcher,
        plugin_dir: &Arc<std::sync::RwLock<PathBuf>>,
        old_config: &Config,
    ) -> Result<Config, String> {
        let (old_osu, old_irc) = (old_config.osu.clone(), old_config.irc.clone());

        let content = tokio::fs::read_to_string(&self.config_path)
            .await
            .map_err(|e| log_fmt!("reload.err_read_failed", error = e.to_string()).to_string())?;

        let mutable: MutableConfig = toml::from_str(&content)
            .map_err(|e| log_fmt!("reload.err_toml_parse", error = e.to_string()).to_string())?;

        let new_osu = mutable.osu.unwrap_or(old_osu.clone());
        let new_irc = mutable.irc.unwrap_or(old_irc.clone());
        let new_scheduler = mutable.scheduler.unwrap_or(old_config.scheduler.clone());

        let new_config = Config {
            osu: new_osu.clone(),
            bot: mutable.bot.unwrap_or(old_config.bot.clone()),
            database: old_config.database.clone(),
            irc: new_irc.clone(),
            scheduler: new_scheduler,
            group_filter: mutable.group_filter,
            groups: mutable.groups,
            upstream: mutable.upstream,
            plugin: mutable.plugin,
        };

        // validate
        if new_config.bot.onebot_url.is_empty() {
            return Err(log_fmt!("reload.err_onebot_url_empty").to_string());
        }
        if new_config.bot.command_timeout_secs < 5 {
            return Err(log_fmt!(
                "reload.err_cmd_timeout_too_small",
                value = new_config.bot.command_timeout_secs
            )
            .to_string());
        }
        if new_config.bot.render_timeout_secs < 5 {
            return Err(log_fmt!(
                "reload.err_render_timeout_too_small",
                value = new_config.bot.render_timeout_secs
            )
            .to_string());
        }
        if new_config.bot.onebot_api_timeout_secs < 2 {
            return Err(log_fmt!(
                "reload.err_api_timeout_too_small",
                value = new_config.bot.onebot_api_timeout_secs
            )
            .to_string());
        }
        if new_config.bot.ur_timeout_secs < 3 {
            return Err(log_fmt!(
                "reload.err_ur_timeout_too_small",
                value = new_config.bot.ur_timeout_secs
            )
            .to_string());
        }
        if new_config.bot.command_timeout_secs > 3600 {
            return Err(log_fmt!(
                "reload.err_cmd_timeout_too_large",
                value = new_config.bot.command_timeout_secs
            )
            .to_string());
        }
        if new_config.bot.render_timeout_secs > 600 {
            return Err(log_fmt!(
                "reload.err_render_timeout_too_large",
                value = new_config.bot.render_timeout_secs
            )
            .to_string());
        }
        if new_config.bot.onebot_api_timeout_secs > 120 {
            return Err(log_fmt!(
                "reload.err_api_timeout_too_large",
                value = new_config.bot.onebot_api_timeout_secs
            )
            .to_string());
        }
        if new_config.bot.ur_timeout_secs > 300 {
            return Err(log_fmt!(
                "reload.err_ur_timeout_too_large",
                value = new_config.bot.ur_timeout_secs
            )
            .to_string());
        }
        if new_config.scheduler.interval_minutes > 1440 {
            return Err(log_fmt!(
                "reload.err_sched_interval_too_large",
                value = new_config.scheduler.interval_minutes
            )
            .to_string());
        }

        let new_plugin_dir = std::path::PathBuf::from(&new_config.plugin.dir);
        let dir_changed = {
            let cur_dir = plugin_dir.write().unwrap_or_else(|e| e.into_inner());
            let changed = *cur_dir != new_plugin_dir;
            if changed {
                watcher.unwatch(cur_dir.as_path()).map_err(|e| {
                    log_fmt!("reload.unwatch_old_dir_failed", error = &e).to_string()
                })?;
            }
            changed
        };
        if dir_changed {
            tokio::fs::create_dir_all(&new_plugin_dir).await.ok();
            let mut cur_dir = plugin_dir.write().unwrap_or_else(|e| e.into_inner());
            watcher
                .watch(&new_plugin_dir, RecursiveMode::NonRecursive)
                .map_err(|e| log_fmt!("reload.watch_new_dir_failed", error = &e).to_string())?;
            info!(
                "{}",
                log_fmt!(
                    "reload.plugin_dir_watch_updated",
                    old = cur_dir.display(),
                    new = new_plugin_dir.display()
                )
            );
            *cur_dir = new_plugin_dir;
        }

        info!("{}", log_fmt!("reload.config_validated"));
        Ok(new_config)
    }

    async fn apply_side_effects(&self, old_config: &Config, new_config: &Config) {
        let old_osu = &old_config.osu;
        let old_irc = &old_config.irc;
        let old_onebot_url = &old_config.bot.onebot_url;
        let new_osu = &new_config.osu;
        let new_irc = &new_config.irc;
        let new_onebot_url = &new_config.bot.onebot_url;
        let onebot_api_timeout_secs = new_config.bot.onebot_api_timeout_secs;
        let upstream_config = &new_config.upstream;

        self.handle
            .onebot_api_timeout
            .store(onebot_api_timeout_secs, Ordering::Relaxed);

        // osu! API 凭据变更：更新 OAuth 缓存，旧 token 自动失效
        if new_osu.client_id != old_osu.client_id || new_osu.client_secret != old_osu.client_secret
        {
            info!("{}", log_fmt!("reload.oauth_credentials_changed"));
            self.handle
                .oauth
                .update_credentials(new_osu.client_id.clone(), new_osu.client_secret.clone())
                .await;
        }

        // IRC 配置变更：重启 IRC 连接
        if new_irc.enabled != old_irc.enabled
            || new_irc.server != old_irc.server
            || new_irc.port != old_irc.port
            || new_irc.nickname != old_irc.nickname
            || new_irc.password != old_irc.password
        {
            info!("{}", log_fmt!("reload.irc_config_changed"));
            self.restart_irc(new_irc).await;
        }

        if old_onebot_url != new_onebot_url {
            info!(
                old = %old_onebot_url,
                new = %new_onebot_url,
                "{}",
                log_fmt!("reload.onebot_url_changed")
            );
            self.handle.force_reconnect.store(true, Ordering::SeqCst);
        }

        self.handle.scheduler.reschedule_all().await;

        let new_chain = build_upstream_chain(
            upstream_config,
            &self.handle.oauth,
            &self.handle.rate_limiter,
        );
        {
            let mut chain = self.handle.upstream_chain.write().await;
            *chain = new_chain;
        }

        info!("{}", log_fmt!("reload.config_reload_success"));
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
                warn!("{}", log_fmt!("reload.plugin_manager_not_init"));
            }
        }

        info!("{}", log_fmt!("reload.plugin_reload_success"));
        Ok(())
    }

    async fn wait_drain(&self) -> bool {
        let start = tokio::time::Instant::now();
        let timeout = Duration::from_secs(DRAIN_TIMEOUT_SECS);
        loop {
            let count = self.handle.in_flight.load(Ordering::SeqCst);
            if count == 0 {
                return true;
            }
            if start.elapsed() >= timeout {
                warn!(
                    in_flight = count,
                    "{}",
                    log_fmt!("reload.drain_wait_timeout", timeout = DRAIN_TIMEOUT_SECS)
                );
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn restart_irc(&self, irc_cfg: &crate::config::IrcConfig) {
        let mut guard = self
            .handle
            .irc_handle
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(old) = guard.take() {
            old.abort();
        }

        if !irc_cfg.enabled {
            info!("{}", log_fmt!("reload.irc_disabled"));
            return;
        }

        if irc_cfg.nickname.is_empty() || irc_cfg.password.is_empty() {
            warn!("{}", log_fmt!("reload.irc_skipped"));
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
                    error!(error = %e, "{}", log_fmt!("reload.irc_client_error"));
                }
            }));
            info!("{}", log_fmt!("reload.irc_restarted"));
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
                    warn!("{}", log_fmt!("reload.unknown_upstream", provider = other));
                }
            }
        }
        UpstreamChain::new(providers)
    } else {
        UpstreamChain::new(Vec::new())
    }
}
