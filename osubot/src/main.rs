mod app_state;
mod background;
mod command;
mod config;
mod constants;
mod last_beatmap_cache;
mod onebot;
mod plugin_runtime;
mod reload;
mod runtime;
mod scheduler;
mod score_filter;
mod score_query;
mod shutdown;
mod ws_loop;
mod xfs_upstream;
mod yumu_upstream;

use app_state::AppState;
use config::Config;
use last_beatmap_cache::LastBeatmapCache;
use osubot_core::{
    api::{self, ApiError},
    dedup::RequestDedup,
    log_fmt,
    response::format_stats_with_change,
    storage::Storage,
    strings::user_str,
    types::{GameMode, Score},
    upstream::UpstreamChain,
    OauthTokenCache, RateLimiter,
};
use osubot_plugin::PluginManager;
use scheduler::Scheduler;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, OnceLock,
};
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use onebot::{parse_onebot_message, send_group_msg, OneBotApi, OneBotResponse, WriteSink};

/// Maximum number of scores to fetch when filters are active.
const SCORE_API_FETCH_LIMIT: u32 = 200;

pub(crate) struct InFlightGuard(Arc<AtomicUsize>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct UserRateLimit {
    last_command: std::time::Instant,
    command_timestamps: Vec<std::time::Instant>,
}

#[derive(Clone)]
pub(crate) struct BotContext {
    storage: Arc<Storage>,
    scheduler: Scheduler,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    command_rate_limits: Arc<dashmap::DashMap<i64, UserRateLimit>>,
    config: Arc<tokio::sync::RwLock<Config>>,
    write: Arc<Mutex<WriteSink>>,
    onebot_api: Arc<OneBotApi>,
    last_beatmap: LastBeatmapCache,
    upstream_chain: Arc<tokio::sync::RwLock<UpstreamChain>>,
    plugin_manager: Arc<tokio::sync::Mutex<Option<PluginManager>>>,
}

impl BotContext {
    /// 从 `AppState` + per-connection 状态派生 `BotContext`。
    /// 字段赋值与直接构造完全一致（参见原 main.rs:4344-4356）。
    pub fn for_dispatch(
        state: &AppState,
        write: Arc<Mutex<WriteSink>>,
        last_beatmap: LastBeatmapCache,
    ) -> Self {
        Self {
            storage: state.storage.clone(),
            scheduler: state.scheduler.clone(),
            oauth: state.oauth.clone(),
            rate_limiter: state.rate_limiter.clone(),
            command_rate_limits: state.user_rate_limits.clone(),
            config: state.config.clone(),
            write,
            onebot_api: state.onebot_api.clone(),
            last_beatmap,
            upstream_chain: state.upstream_chain.clone(),
            plugin_manager: state.plugin_manager.clone(),
        }
    }
}

/// Clone-able、不含 qq 的 `ApiError` 投影，用作 dedup 的错误类型。
/// （`reqwest::Error` 未实现 `Clone`，故 `ApiError` 本身无法 Clone。）
/// creator 转换一次；每个 waiter 用自己的 `qq` 格式化消息，避免 @ 错人。
#[derive(Clone, Debug)]
pub(crate) enum DedupApiError {
    NotFound,
    MissingApiKey,
    OAuthError,
    RateLimitedWithRetryAfter(Option<u64>),
    ClientRateLimited,
    Other,
}

impl DedupApiError {
    pub(crate) fn from_api_error(e: &ApiError) -> Self {
        match e {
            ApiError::NotFound => Self::NotFound,
            ApiError::MissingApiKey => Self::MissingApiKey,
            ApiError::OAuthError => Self::OAuthError,
            ApiError::RateLimitedWithRetryAfter(r) => Self::RateLimitedWithRetryAfter(*r),
            ApiError::ClientRateLimited => Self::ClientRateLimited,
            _ => Self::Other,
        }
    }

    /// 格式化通用用户错误消息（语义同 `api_error_msg`）。
    pub(crate) fn to_user_msg(&self, qq: i64) -> String {
        let (template, secs) = match self {
            Self::NotFound => (user_str("error.not_found"), None),
            Self::MissingApiKey => (user_str("error.api_key"), None),
            Self::OAuthError => (user_str("error.oauth"), None),
            Self::RateLimitedWithRetryAfter(None) => (user_str("error.rate_limit_generic"), None),
            Self::RateLimitedWithRetryAfter(Some(secs)) => {
                (user_str("error.rate_limit"), Some(*secs))
            }
            Self::ClientRateLimited => (user_str("error.client_rate_limit"), None),
            Self::Other => (user_str("error.query_failed"), None),
        };
        let mut msg = template.replace("{qq}", &qq.to_string());
        if let Some(secs) = secs {
            msg = msg.replace("{secs}", &secs.to_string());
        }
        msg
    }
}

impl From<&'static str> for DedupApiError {
    fn from(_: &str) -> Self {
        Self::Other
    }
}

pub(crate) fn api_error_msg(qq: i64, e: &ApiError) -> String {
    DedupApiError::from_api_error(e).to_user_msg(qq)
}

/// Send an error message to the response channel.
pub(crate) async fn send_error(resp_tx: &mpsc::Sender<String>, qq: i64, key: &str) {
    let _ = resp_tx
        .send(user_str(key).replace("{qq}", &qq.to_string()))
        .await;
}

impl BotContext {
    async fn resolve_binding(&self, qq: i64) -> Option<(i64, String)> {
        match self.storage.get_binding(qq).await {
            Ok(Some(binding)) => Some(binding),
            Ok(None) => {
                let binding = self.upstream_chain.read().await.try_query(qq).await?;
                if let Err(e) = self.storage.set_user_id(&binding.1, binding.0).await {
                    warn!("{}", log_fmt!("main.persist_user_id_failed", error = &e));
                }
                if let Err(e) = self.storage.bind(qq, binding.0, &binding.1).await {
                    warn!("{}", log_fmt!("main.persist_binding_failed", error = &e));
                }
                Some(binding)
            }
            Err(_) => None,
        }
    }

    async fn fetch_stats_and_reply(
        &self,
        qq: i64,
        user_id: i64,
        username: &str,
        mode: GameMode,
        resp_tx: &mpsc::Sender<String>,
        log_label: &str,
    ) {
        self.scheduler.trigger_update(user_id, mode).await;
        match api::fetch_user_stats_by_user_id(&self.rate_limiter, &self.oauth, user_id, mode).await
        {
            Ok(stats) => {
                if stats.username != username {
                    if let Err(e) = self
                        .storage
                        .update_binding_username(qq, &stats.username)
                        .await
                    {
                        tracing::warn!(
                            qq = qq,
                            username = %stats.username,
                            error = %e,
                            "{}",
                            log_fmt!("main.update_binding_failed")
                        );
                    }
                }
                if let Err(e) = self.storage.set_user_id(&stats.username, user_id).await {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = user_id,
                        error = %e,
                        "{}",
                        log_fmt!("main.cache_user_id_failed")
                    );
                }
                let change = self
                    .storage
                    .calculate_change(user_id, mode, &stats)
                    .await
                    .inspect_err(|e| {
                        tracing::warn!(
                            user_id = user_id,
                            mode = ?mode,
                            error = %e,
                            "{}",
                            log_fmt!("main.calculate_change_failed")
                        )
                    })
                    .ok()
                    .flatten();
                info!(qq = qq, osu_id = user_id, username = %stats.username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "{}", log_fmt!("main.log_label_success", label = log_label));
                let response = format_stats_with_change(&stats, &change, mode);
                let _ = resp_tx.send(response).await;
            }
            Err(e) => {
                warn!(qq = qq, osu_id = user_id, mode = ?mode, error = ?e, "{}", log_fmt!("main.log_label_failed", label = log_label));
                let _ = resp_tx.send(api_error_msg(qq, &e)).await;
            }
        }
    }
}

pub(crate) type ProfileDedup = RequestDedup<(i64, GameMode), Arc<Vec<u8>>, String>;

pub(crate) fn profile_dedup() -> &'static ProfileDedup {
    static DEDUP: OnceLock<ProfileDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

pub(crate) type ScoreDedup = RequestDedup<(i64, bool, u32, GameMode), Arc<Vec<Score>>, String>;

pub(crate) fn score_dedup() -> &'static ScoreDedup {
    static DEDUP: OnceLock<ScoreDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

pub(crate) type BeatmapScoresDedup =
    RequestDedup<(i64, i64, GameMode, Option<u32>), Vec<Score>, String>;

pub(crate) fn beatmap_scores_dedup() -> &'static BeatmapScoresDedup {
    static DEDUP: OnceLock<BeatmapScoresDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

pub(crate) type BestScoresDedup = RequestDedup<(i64, GameMode, u32), Vec<Score>, String>;

pub(crate) fn best_scores_dedup() -> &'static BestScoresDedup {
    static DEDUP: OnceLock<BestScoresDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

pub(crate) type TodayBestScoresDedup = RequestDedup<(i64, GameMode, u32), Vec<Score>, String>;

pub(crate) fn today_best_scores_dedup() -> &'static TodayBestScoresDedup {
    static DEDUP: OnceLock<TodayBestScoresDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

pub(crate) async fn handle_irc_message(
    storage: Arc<Storage>,
    irc_msg: osubot_core::irc::IrcPrivateMessage,
    write: Arc<Mutex<WriteSink>>,
    rate_limiter: Arc<RateLimiter>,
    oauth: Arc<OauthTokenCache>,
) {
    let code = irc_msg.message.trim();

    let pending = match storage.get_pending_bind(code).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            warn!(code = code, sender = %irc_msg.sender, "{}", log_fmt!("main.irc_no_pending_bind"));
            return;
        }
        Err(_) => {
            error!("{}", log_fmt!("main.irc_pending_bind_db_error"));
            return;
        }
    };

    if irc_msg.sender.to_lowercase() != pending.target_username.replace(' ', "_").to_lowercase() {
        if let Err(e) = storage.remove_pending_bind(code).await {
            tracing::warn!(code = %code, error = %e, "{}", log_fmt!("main.irc_pending_bind_username_mismatch"));
        }
        let msg = user_str("bind.wrong_person").replace("{qq}", &pending.qq_user_id.to_string());
        send_group_msg(&write, pending.group_id, &msg).await;
        return;
    }

    match api::get_user_info(&rate_limiter, &oauth, &pending.target_username).await {
        Ok(Some(info)) => {
            if let Err(e) = storage.set_user_id(&pending.target_username, info.id).await {
                warn!(error = %e, "{}", log_fmt!("main.cache_user_id_failed"));
            }
            match storage
                .bind(pending.qq_user_id, info.id, &info.username)
                .await
            {
                Ok(Ok(())) => {
                    if let Err(e) = storage.remove_pending_bind(code).await {
                        tracing::warn!(code = %code, error = %e, "{}", log_fmt!("main.remove_pending_bind_failed", error = &e));
                    }
                    info!(qq = pending.qq_user_id, username = %info.username, "{}", log_fmt!("main.irc_bind_verified"));
                    let msg = user_str("bind.success")
                        .replace("{qq}", &pending.qq_user_id.to_string())
                        .replace("{name}", &info.username);
                    send_group_msg(&write, pending.group_id, &msg).await;
                }
                Ok(Err(_)) => {
                    if let Err(e) = storage.remove_pending_bind(code).await {
                        tracing::warn!(code = %code, error = %e, "{}", log_fmt!("main.remove_pending_bind_failed", error = &e));
                    }
                    let msg = user_str("bind.irc_already_bound_other")
                        .replace("{qq}", &pending.qq_user_id.to_string());
                    send_group_msg(&write, pending.group_id, &msg).await;
                }
                Err(_) => {
                    if let Err(e) = storage.remove_pending_bind(code).await {
                        tracing::warn!(code = %code, error = %e, "{}", log_fmt!("main.remove_pending_bind_failed", error = &e));
                    }
                    let msg = user_str("bind.failed_retry")
                        .replace("{qq}", &pending.qq_user_id.to_string());
                    send_group_msg(&write, pending.group_id, &msg).await;
                }
            }
        }
        Ok(None) => {
            if let Err(e) = storage.remove_pending_bind(code).await {
                tracing::warn!(code = %code, error = %e, "{}", log_fmt!("main.remove_pending_bind_failed", error = &e));
            }
            warn!(
                "{}",
                log_fmt!(
                    "main.irc_bind_user_not_found",
                    username = &pending.target_username
                )
            );
            let msg = user_str("bind.irc_user_not_found")
                .replace("{qq}", &pending.qq_user_id.to_string());
            send_group_msg(&write, pending.group_id, &msg).await;
        }
        Err(e) => {
            if let Err(e2) = storage.remove_pending_bind(code).await {
                tracing::warn!(code = %code, error = %e2, "{}", log_fmt!("main.remove_pending_bind_failed", error = &e2));
            }
            warn!(
                "{}",
                log_fmt!(
                    "main.irc_bind_fetch_failed",
                    username = &pending.target_username,
                    error = &e
                )
            );
            let msg =
                user_str("bind.failed_retry").replace("{qq}", &pending.qq_user_id.to_string());
            send_group_msg(&write, pending.group_id, &msg).await;
        }
    }
}

#[tokio::main]
async fn main() {
    runtime::init_tracing();
    info!("{}", log_fmt!("main.startup"));
    osubot_render::ensure_cache_dir().await;

    let handles = runtime::build_runtime_handles().await;

    background::backfill_user_ids(&handles).await;
    let scheduler_h = background::spawn_scheduler(&handles);
    let irc_h = background::spawn_irc(&handles);
    let cleanup_h = background::spawn_onebot_cleanup(&handles);
    let watcher_h = background::spawn_watcher(&handles);
    background::spawn_shutdown_signal(handles.app_state.shutdown.clone());

    // Extract state + drain/in_flight before handles is consumed by spawn_irc_bridge.
    // AppState derives Clone (all fields are Arc), so this is cheap.
    let state = handles.app_state.clone();
    let drain = handles.reload_handle.plugin.drain.clone();
    let in_flight = handles.reload_handle.plugin.in_flight.clone();

    let irc_bridge_h = background::spawn_irc_bridge(handles);

    ws_loop::run_ws_reconnect_loop(state.clone(), drain, in_flight).await;

    plugin_runtime::shutdown_all(&state.plugin_manager).await;
    state.scheduler.shutdown();

    let drain_timeout = std::time::Duration::from_secs(10);
    let _ = tokio::time::timeout(drain_timeout, async {
        let _ = tokio::join!(scheduler_h, irc_h, cleanup_h, watcher_h, irc_bridge_h);
    })
    .await;
}
