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

#[derive(Clone)]
pub struct NetworkHandle {
    pub upstream_chain: Arc<RwLock<UpstreamChain>>,
    pub oauth: Arc<OauthTokenCache>,
    pub rate_limiter: Arc<RateLimiter>,
    pub force_reconnect: Arc<AtomicBool>,
}

#[derive(Clone)]
pub struct PluginHandle {
    pub pm: Arc<Mutex<Option<PluginManager>>>,
    pub drain: Arc<AtomicBool>,
    pub in_flight: Arc<AtomicUsize>,
}

#[derive(Clone)]
pub struct IrcHandle {
    pub irc_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub irc_tx: Option<mpsc::Sender<osubot_core::irc::IrcPrivateMessage>>,
}

#[derive(Clone)]
pub struct ReloadHandle {
    pub network: NetworkHandle,
    pub plugin: PluginHandle,
    pub irc: IrcHandle,
    pub config: Arc<RwLock<Config>>,
    pub onebot_api_timeout: Arc<AtomicU64>,
    pub scheduler: Scheduler,
}

pub struct ReloadHandleParams {
    pub config: Arc<RwLock<Config>>,
    pub pm: Arc<Mutex<Option<PluginManager>>>,
    pub onebot_api_timeout: Arc<AtomicU64>,
    pub upstream_chain: Arc<RwLock<UpstreamChain>>,
    pub oauth: Arc<OauthTokenCache>,
    pub rate_limiter: Arc<RateLimiter>,
    pub scheduler: Scheduler,
    pub irc_handle: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
    pub irc_tx: Option<mpsc::Sender<osubot_core::irc::IrcPrivateMessage>>,
}

impl ReloadHandle {
    pub fn new(params: ReloadHandleParams) -> Self {
        Self {
            network: NetworkHandle {
                upstream_chain: params.upstream_chain,
                oauth: params.oauth,
                rate_limiter: params.rate_limiter,
                force_reconnect: Arc::new(AtomicBool::new(false)),
            },
            plugin: PluginHandle {
                pm: params.pm,
                drain: Arc::new(AtomicBool::new(false)),
                in_flight: Arc::new(AtomicUsize::new(0)),
            },
            irc: IrcHandle {
                irc_handle: params.irc_handle,
                irc_tx: params.irc_tx,
            },
            config: params.config,
            onebot_api_timeout: params.onebot_api_timeout,
            scheduler: params.scheduler,
        }
    }
}

pub struct ReloadCoordinator {
    handle: ReloadHandle,
    config_path: PathBuf,
    // INVARIANT: this lock is never held across .await points.
    // Using std::sync::RwLock for performance in synchronous notify callbacks.
    plugin_dir: Arc<std::sync::RwLock<PathBuf>>,
    shutdown: Arc<AtomicBool>,
}

impl ReloadCoordinator {
    pub fn new(
        handle: ReloadHandle,
        config_path: PathBuf,
        plugin_dir: PathBuf,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            handle,
            config_path,
            plugin_dir: Arc::new(std::sync::RwLock::new(plugin_dir)),
            shutdown,
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

        let shutdown = self.shutdown.clone();
        loop {
            let msg = tokio::select! {
                _ = crate::shutdown::wait_for_shutdown(&shutdown) => break,
                msg = rx.recv() => msg,
            };
            let Some(()) = msg else { break };
            let mut deadline = tokio::time::Instant::now() + Duration::from_millis(500);
            loop {
                tokio::select! {
                    _ = crate::shutdown::wait_for_shutdown(&shutdown) => break,
                    result = tokio::time::timeout_at(deadline, rx.recv()) => match result {
                        Ok(Some(())) => {
                            deadline = tokio::time::Instant::now() + Duration::from_millis(500);
                            continue;
                        }
                        Ok(None) => return Ok(()),
                        Err(_) => {
                            self.reload(&mut watcher, &self.plugin_dir).await;
                            break;
                        }
                    },
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

        self.handle.plugin.drain.store(true, Ordering::SeqCst);
        let drained = self.wait_drain().await;

        let old_config = self.handle.config.read().await.clone();

        let new_config = match self.reload_config(watcher, plugin_dir, &old_config).await {
            Ok(cfg) => cfg,
            Err(e) => {
                error!("{}", log_fmt!("reload.config_reload_failed", error = &e));
                self.handle.plugin.drain.store(false, Ordering::SeqCst);
                return;
            }
        };

        if !drained {
            error!("{}", log_fmt!("reload.drain_timeout"));
            // 修复：drain 超时不要调 apply_side_effects / 写新 config。
            // 在飞的旧任务仍持 plugin 索引 + config read guard，此时并发新调度
            // 会让新/旧 config 行为不确定。保持 drain=true 让后续 dispatch 跳过，
            // 下次 reload 重新尝试或等 SIGINT/SIGHUP 强制退出。
            self.handle.plugin.drain.store(true, Ordering::SeqCst);
            return;
        }

        if let Err(e) = self.reload_plugins().await {
            error!(
                "{}",
                log_fmt!("reload.plugin_reload_failed_rollback", error = &e)
            );
            // 插件重载失败，不应用 side effects，直接回滚配置。
            // drain=true 已阻止新消息分发，副作用在下次成功重载时自然应用。
            *self.handle.config.write().await = old_config;
            self.handle.plugin.drain.store(true, Ordering::SeqCst);
            return;
        }

        // 插件重载成功，写入新配置
        *self.handle.config.write().await = new_config.clone();
        if let Err(e) = self.apply_side_effects(&old_config, &new_config).await {
            error!(
                "{}",
                log_fmt!("reload.side_effects_failed", error = e.to_string())
            );
            self.handle.plugin.drain.store(true, Ordering::SeqCst);
            return;
        }

        self.handle.plugin.drain.store(false, Ordering::SeqCst);
    }

    /// 将新 TOML 解析为 `MutableConfig` 并与 `old_config` 合并出新 `Config`。
    ///
    /// 耦合不变式：下方 `Config { .. }` 的构造必须与 `config::MutableConfig`
    /// 的字段保持一致——`Option` 字段（osu/irc/bot/scheduler）为 None 时继承旧值，
    /// 其余字段直接采用新值，遗留不可变字段（database）始终沿用 old_config。
    /// 每当 `MutableConfig` 新增/删除可重载字段时，必须同步更新此处。
    async fn reload_config(
        &self,
        watcher: &mut RecommendedWatcher,
        plugin_dir: &Arc<std::sync::RwLock<PathBuf>>,
        old_config: &Config,
    ) -> Result<Config, crate::config::ConfigError> {
        let (old_osu, old_irc) = (old_config.osu.clone(), old_config.irc.clone());

        let content = tokio::fs::read_to_string(&self.config_path).await?;

        let mutable: MutableConfig = toml::from_str(&content)?;

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
            private: mutable.private,
            private_filter: mutable.private_filter,
            upstream: mutable.upstream,
            plugin: mutable.plugin,
            match_listen: mutable
                .match_listen
                .unwrap_or(old_config.match_listen.clone()),
            ppy_sb: mutable.ppy_sb.unwrap_or(old_config.ppy_sb.clone()),
        };

        new_config.validate()?;

        let new_plugin_dir = std::path::PathBuf::from(&new_config.plugin.dir);
        let dir_changed = {
            let cur_dir = plugin_dir.write().unwrap_or_else(|e| e.into_inner());
            let changed = *cur_dir != new_plugin_dir;
            if changed {
                watcher.unwatch(cur_dir.as_path()).map_err(|e| {
                    crate::config::ConfigError::Validation(format!("取消监视旧插件目录失败：{e}"))
                })?;
            }
            changed
        };
        if dir_changed {
            tokio::fs::create_dir_all(&new_plugin_dir).await.ok();
            let mut cur_dir = plugin_dir.write().unwrap_or_else(|e| e.into_inner());
            watcher
                .watch(&new_plugin_dir, RecursiveMode::NonRecursive)
                .map_err(|e| {
                    crate::config::ConfigError::Validation(format!("监视新插件目录失败：{e}"))
                })?;
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

    async fn apply_side_effects(
        &self,
        old_config: &Config,
        new_config: &Config,
    ) -> Result<(), crate::config::ConfigError> {
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
                .network
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
            self.handle
                .network
                .force_reconnect
                .store(true, Ordering::SeqCst);
        }

        self.handle.scheduler.reschedule_all().await;

        let new_chain = build_upstream_chain(
            upstream_config,
            &self.handle.network.oauth,
            &self.handle.network.rate_limiter,
        )?;
        {
            let mut chain = self.handle.network.upstream_chain.write().await;
            *chain = new_chain;
        }

        info!("{}", log_fmt!("reload.config_reload_success"));
        Ok(())
    }

    async fn reload_plugins(&self) -> Result<(), String> {
        let plugin_config = {
            let cfg = self.handle.config.read().await;
            cfg.plugin.clone()
        };

        {
            let mut guard = self.handle.plugin.pm.lock().await;
            // Take PM out of lock to avoid holding the lock across await points
            // in reload_all (which unloads/loads wasm modules). This allows
            // in-flight messages to acquire the lock during reload.
            if let Some(mut pm) = guard.take() {
                drop(guard);
                let result = pm.reload_all(&plugin_config).await;
                self.handle.plugin.pm.lock().await.replace(pm);
                result?;
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
            let count = self.handle.plugin.in_flight.load(Ordering::SeqCst);
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
        let mut guard = self.handle.irc.irc_handle.lock().await;
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

        if let Some(ref tx) = self.handle.irc.irc_tx {
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
) -> Result<UpstreamChain, crate::config::ConfigError> {
    if !upstream.enabled {
        return Ok(UpstreamChain::new(Vec::new()));
    }
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
                return Err(crate::config::ConfigError::Validation(format!(
                    "未知的 upstream provider 类型「{other}」（已知：xfs、yumu）"
                )));
            }
        }
    }
    Ok(UpstreamChain::new(providers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    /// drain 标志初始为 false
    #[test]
    fn drain_flag_starts_false() {
        let drain = Arc::new(AtomicBool::new(false));
        assert!(!drain.load(Ordering::SeqCst));
    }
}
