mod config;
mod constants;
mod last_beatmap_cache;
mod reload;
mod scheduler;
mod xfs_upstream;
mod yumu_upstream;

use reload::ReloadHandle;

use config::Config;
use futures_util::{future::join_all, SinkExt, StreamExt};
use last_beatmap_cache::LastBeatmapCache;
use osubot_core::apply_mod_adjustment_to_stats;
use osubot_core::enrich_score_with_pp;
use osubot_core::{
    api::{self, ApiError},
    dedup::RequestDedup,
    highlight::{format_highlight, get_highlight, HighlightError},
    parse_command,
    response::{format_score, format_scores, format_stats_with_change},
    storage::Storage,
    types::{format_play_datetime, Command, GameMode, Score, UserStats},
    upstream::UpstreamChain,
    OauthTokenCache, RateLimiter,
};
use osubot_plugin::{HostServices, PluginManager};
use osubot_render::cache as render_cache;
use osubot_render::PROFILE_VIEWPORT_WIDTH;
use osubot_render::SCORE_LIST_RENDER_TIMEOUT_SECS;
use osubot_render::{render_profile_card, render_score_card, render_score_list_card};
use scheduler::Scheduler;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use tracing_subscriber::{fmt, EnvFilter};

struct InFlightGuard(Arc<AtomicUsize>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone)]
struct QQMessage {
    group_id: i64,
    user_id: i64,
    message: String,
    mentioned_user_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct OneBotResponse {
    status: Option<String>,
    data: Option<serde_json::Value>,
    echo: Option<String>,
}

struct UserRateLimit {
    last_command: std::time::Instant,
    command_timestamps: Vec<std::time::Instant>,
}

struct PendingEntry {
    sender: oneshot::Sender<serde_json::Value>,
    created_at: std::time::Instant,
}

struct OneBotApi {
    pending: Mutex<HashMap<String, PendingEntry>>,
    timeout: Arc<AtomicU64>,
}

impl OneBotApi {
    fn new(timeout_secs: Arc<AtomicU64>) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            timeout: timeout_secs,
        }
    }
}

static NEXT_ECHO: AtomicU64 = AtomicU64::new(0);

fn next_echo() -> String {
    NEXT_ECHO.fetch_add(1, Ordering::Relaxed).to_string()
}

#[derive(Debug, Deserialize)]
struct OneBotMessage {
    #[serde(rename = "post_type")]
    post_type: String,
    #[serde(rename = "message_type")]
    message_type: Option<String>,
    #[serde(rename = "group_id")]
    group_id: Option<i64>,
    #[serde(rename = "user_id")]
    user_id: Option<i64>,
    #[serde(rename = "message")]
    message: Option<serde_json::Value>,
}

/// Parse a OneBot JSON message into a `QQMessage`.
/// Returns `None` if the message is not a group message or lacks required fields.
fn parse_onebot_message(json: &str) -> Option<QQMessage> {
    let msg: OneBotMessage = serde_json::from_str(json).ok()?;

    if msg.post_type != "message" || msg.message_type.as_deref() != Some("group") {
        return None;
    }

    let group_id = msg.group_id?;
    let user_id = msg.user_id?;

    let (message_text, mentioned_user_id) = extract_message_and_mention(&msg.message?);

    Some(QQMessage {
        group_id,
        user_id,
        message: message_text,
        mentioned_user_id,
    })
}

/// Extract plain text and a single @mention user ID from a OneBot message array.
/// Returns `(text, mentioned_user_id)` — the mention is `Some` only if exactly one user is @mentioned.
fn extract_message_and_mention(message: &serde_json::Value) -> (String, Option<i64>) {
    let arr = match message.as_array() {
        Some(a) => a,
        None => {
            let text = message.as_str().unwrap_or("").to_string();
            return (text, None);
        }
    };

    let mut text = String::new();
    let mut at_qqs: Vec<i64> = Vec::new();

    for segment in arr {
        match segment.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = segment
                    .get("data")
                    .and_then(|d| d.get("text"))
                    .and_then(|v| v.as_str())
                {
                    text.push_str(t);
                }
            }
            Some("at") => {
                if let Some(qq_val) = segment.get("data").and_then(|d| d.get("qq")) {
                    if let Some(qq) = qq_val.as_i64() {
                        at_qqs.push(qq);
                    } else if let Some(qq_str) = qq_val.as_str() {
                        if let Ok(qq) = qq_str.parse::<i64>() {
                            at_qqs.push(qq);
                        }
                        // qq="all" falls through — not a valid i64, ignored
                    }
                }
            }
            _ => {}
        }
    }

    let mentioned_user_id = if at_qqs.len() == 1 {
        Some(at_qqs[0])
    } else {
        None
    };

    (text, mentioned_user_id)
}

#[derive(Clone)]
struct BotContext {
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

fn api_error_msg(e: &ApiError) -> String {
    match e {
        ApiError::NotFound => "未找到该用户".to_string(),
        ApiError::MissingApiKey => "API Key 未配置".to_string(),
        ApiError::OAuthError => "OAuth 认证失败".to_string(),
        ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
            format!("查询繁忙，请 {} 秒后再试", secs)
        }
        ApiError::RateLimitedWithRetryAfter(None) => "查询繁忙，请稍后再试".to_string(),
        ApiError::ClientRateLimited => "本地请求限流，请稍后再试".to_string(),
        _ => "查询失败，请稍后重试".to_string(),
    }
}

impl BotContext {
    async fn resolve_binding(&self, qq: i64) -> Option<(i64, String)> {
        match self.storage.get_binding(qq) {
            Ok(Some(binding)) => Some(binding),
            Ok(None) => {
                let binding = self.upstream_chain.read().await.try_query(qq).await?;
                if let Err(e) = self.storage.set_user_id(&binding.1, binding.0) {
                    warn!("failed to persist user_id from upstream: {e}");
                }
                if let Err(e) = self.storage.bind(qq, binding.0, &binding.1) {
                    warn!("failed to persist binding from upstream: {e}");
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
                    if let Err(e) = self.storage.update_binding_username(qq, &stats.username) {
                        tracing::warn!(
                            qq = qq,
                            username = %stats.username,
                            error = %e,
                            "Failed to update binding username"
                        );
                    }
                }
                if let Err(e) = self.storage.set_user_id(&stats.username, user_id) {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = user_id,
                        error = %e,
                        "Failed to cache user_id"
                    );
                }
                let change = self
                    .storage
                    .calculate_change(user_id, mode, &stats)
                    .inspect_err(|e| {
                        tracing::warn!(
                            user_id = user_id,
                            mode = ?mode,
                            error = %e,
                            "Failed to calculate change"
                        )
                    })
                    .ok()
                    .flatten();
                info!(qq = qq, osu_id = user_id, username = %stats.username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "{log_label} success");
                let response = format_stats_with_change(&stats, &change, mode);
                let _ = resp_tx.send(response).await;
            }
            Err(e) => {
                warn!(qq = qq, osu_id = user_id, mode = ?mode, error = ?e, "{log_label} failed");
                let _ = resp_tx.send(api_error_msg(&e)).await;
            }
        }
    }
}

type ProfileDedup = RequestDedup<(i64, GameMode), Arc<Vec<u8>>, String>;

fn profile_dedup() -> &'static ProfileDedup {
    static DEDUP: OnceLock<ProfileDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

type ScoreDedup = RequestDedup<(i64, bool, u32, GameMode), Arc<Vec<Score>>, String>;

fn score_dedup() -> &'static ScoreDedup {
    static DEDUP: OnceLock<ScoreDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

type ScoreByIdDedup = RequestDedup<(i64, GameMode), Score, String>;

fn score_by_id_dedup() -> &'static ScoreByIdDedup {
    static DEDUP: OnceLock<ScoreByIdDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

type BeatmapScoreDedup = RequestDedup<(i64, i64, GameMode, Option<Vec<String>>), Score, String>;

fn beatmap_score_dedup() -> &'static BeatmapScoreDedup {
    static DEDUP: OnceLock<BeatmapScoreDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

type BeatmapScoresDedup = RequestDedup<(i64, i64, GameMode, Option<u32>), Vec<Score>, String>;

fn beatmap_scores_dedup() -> &'static BeatmapScoresDedup {
    static DEDUP: OnceLock<BeatmapScoresDedup> = OnceLock::new();
    DEDUP.get_or_init(RequestDedup::new)
}

async fn resolve_score_user(
    ctx: &BotContext,
    msg: &QQMessage,
    username: &Option<String>,
    qq: &Option<i64>,
    mode: GameMode,
    resp_tx: &mpsc::Sender<String>,
) -> Option<(i64, String, UserStats)> {
    tracing::trace!("resolve_score_user: starting");
    if let Some(ref name) = username {
        // Look up by username
        tracing::trace!("resolve_score_user: looking up by username '{}'", name);
        match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, name, mode).await {
            Ok(stats) => {
                if let Err(e) = ctx.storage.set_user_id(&stats.username, stats.user_id) {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "Failed to cache user_id"
                    );
                }
                Some((stats.user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, username = %name, "resolve_score_user: API lookup failed");
                let err_msg = match e {
                    ApiError::NotFound => format!("找不到用户 \"{}\"", name),
                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                    ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                        format!("请求过于频繁，请 {} 秒后再试", secs)
                    }
                    ApiError::RateLimitedWithRetryAfter(None) => {
                        "请求过于频繁，请稍后再试".to_string()
                    }
                    ApiError::ClientRateLimited => "本地请求限流，请稍后再试".to_string(),
                    _ => "获取数据失败，请稍后再试".to_string(),
                };
                let _ = resp_tx.send(err_msg).await;
                None
            }
        }
    } else {
        let (user_id, _stored_name, error_msg) = if let Some(mentioned_qq) = qq {
            match ctx.resolve_binding(*mentioned_qq).await {
                Some((user_id, name)) => (user_id, name, None),
                None => (
                    0,
                    String::new(),
                    Some("该用户还没有绑定 osu! 账号".to_string()),
                ),
            }
        } else {
            match ctx.resolve_binding(msg.user_id).await {
                Some((user_id, name)) => (user_id, name, None),
                None => (
                    0,
                    String::new(),
                    Some("你还没有绑定 osu! 账号，请使用 绑定 <用户名> 绑定".to_string()),
                ),
            }
        };
        if let Some(err) = error_msg {
            let _ = resp_tx.send(err).await;
            return None;
        }
        tracing::info!("resolve_score_user: fetching stats for user_id={}", user_id);
        match api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, user_id, mode).await {
            Ok(stats) => {
                if let Err(e) = ctx.storage.set_user_id(&stats.username, stats.user_id) {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "Failed to cache user_id"
                    );
                }
                Some((user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id, "resolve_score_user: API lookup failed for bound user");
                let _ = resp_tx
                    .send("获取用户数据失败，请稍后再试".to_string())
                    .await;
                None
            }
        }
    }
}

struct ScoreQueryParams<'a> {
    mode: GameMode,
    username: &'a Option<String>,
    qq: &'a Option<i64>,
    is_pass: bool,
    limit: u32,
    is_single: bool,
}

async fn handle_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    params: ScoreQueryParams<'_>,
) {
    tracing::trace!("handle_score_query: starting");

    // For self/bound users (no explicit username/qq), resolve user_id from DB
    // and parallelize the two API calls. For username/QQ lookups, use the
    // existing sequential resolve_score_user flow.
    let is_self = params.username.is_none() && params.qq.is_none();
    let include_fails = !params.is_pass;
    let (user_id, resolved_username, user_stats, score_result) = if is_self {
        let (uid, name) = match ctx.resolve_binding(msg.user_id).await {
            Some(binding) => binding,
            None => {
                let _ = resp_tx
                    .send("你还没有绑定 osu! 账号，请使用 绑定 <用户名> 绑定".to_string())
                    .await;
                return;
            }
        };

        tracing::trace!(
            "handle_score_query: bound user, user_id={}, username={}",
            uid,
            name
        );
        ctx.scheduler.trigger_update(uid, params.mode).await;

        let limit = params.limit;
        let mode = params.mode;
        let is_pass = params.is_pass;
        let rate_limiter = ctx.rate_limiter.clone();
        let oauth = ctx.oauth.clone();
        let rl2 = rate_limiter.clone();
        let oa2 = oauth.clone();

        let (stats_result, scores) = tokio::join!(
            api::fetch_user_stats_by_user_id(&rate_limiter, &oauth, uid, mode),
            score_dedup().run_or_wait((uid, is_pass, limit, mode), move || {
                let rate_limiter = rl2.clone();
                let oauth = oa2.clone();
                async move {
                    api::get_user_recent(&rate_limiter, &oauth, uid, mode, include_fails, limit)
                        .await
                        .map(Arc::new)
                        .map_err(|e| {
                            warn!(user_id = uid, mode = ?mode, error = ?e, "Score query failed");
                            match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                                    format!("请求过于频繁，请 {} 秒后再试", secs)
                                }
                                ApiError::RateLimitedWithRetryAfter(None) => {
                                    "请求过于频繁，请稍后再试".to_string()
                                }
                                ApiError::ClientRateLimited => {
                                    "本地请求限流，请稍后再试".to_string()
                                }
                                e => {
                                    tracing::error!(error = ?e, "Score query error details");
                                    "获取数据失败，请稍后再试".to_string()
                                }
                            }
                        })
                }
            }),
        );

        let user_stats = match stats_result {
            Ok(stats) => {
                if let Err(e) = ctx.storage.set_user_id(&stats.username, stats.user_id) {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "Failed to cache user_id"
                    );
                }
                stats
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id = uid, "resolve: API lookup failed for bound user");
                let _ = resp_tx
                    .send("获取用户数据失败，请稍后再试".to_string())
                    .await;
                return;
            }
        };

        (uid, name, user_stats, scores)
    } else {
        let (uid, name, user_stats) =
            match resolve_score_user(ctx, msg, params.username, params.qq, params.mode, resp_tx)
                .await
            {
                Some(u) => {
                    tracing::trace!(
                        "resolve_score_user: resolved user_id={}, username={}",
                        u.0,
                        u.1
                    );
                    u
                }
                None => {
                    tracing::warn!("resolve_score_user: returned None");
                    return;
                }
            };

        ctx.scheduler.trigger_update(uid, params.mode).await;
        let dedup_key = (uid, params.is_pass, params.limit, params.mode);
        let dedup_rate_limiter = ctx.rate_limiter.clone();
        let dedup_oauth = ctx.oauth.clone();
        let dedup_mode = params.mode;

        tracing::trace!(
            "Fetching scores for user_id={}, mode={:?}, limit={}",
            uid,
            params.mode,
            params.limit
        );
        let scores: Result<Arc<Vec<Score>>, String> = score_dedup()
            .run_or_wait(dedup_key, move || {
                let dedup_rate_limiter = dedup_rate_limiter.clone();
                let dedup_oauth = dedup_oauth.clone();
                async move {
                    api::get_user_recent(
                        &dedup_rate_limiter,
                        &dedup_oauth,
                        uid,
                        dedup_mode,
                        include_fails,
                        params.limit,
                    )
                    .await
                    .map(Arc::new)
                    .map_err(|e| {
                        warn!(user_id = uid, mode = ?dedup_mode, error = ?e, "Score query failed");
                        match e {
                            ApiError::NotFound => "未找到该用户".to_string(),
                            ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                                format!("请求过于频繁，请 {} 秒后再试", secs)
                            }
                            ApiError::RateLimitedWithRetryAfter(None) => {
                                "请求过于频繁，请稍后再试".to_string()
                            }
                            ApiError::ClientRateLimited => "本地请求限流，请稍后再试".to_string(),
                            e => {
                                tracing::error!(error = ?e, "Score query error details");
                                "获取数据失败，请稍后再试".to_string()
                            }
                        }
                    })
                }
            })
            .await;

        (uid, name, user_stats, scores)
    };

    let dedup_username = resolved_username.clone();

    match score_result {
        Ok(mut scores) => {
            if scores.is_empty() {
                let empty_msg = if include_fails {
                    "最近没有游玩记录（包括失败）"
                } else {
                    "最近没有游玩记录"
                };
                let _ = resp_tx.send(empty_msg.to_string()).await;
                return;
            }
            ctx.last_beatmap
                .set(msg.group_id, scores[0].beatmap_id as u32);
            if params.is_single {
                let index = (params.limit - 1) as usize;
                if index >= scores.len() {
                    let _ = resp_tx
                        .send(format!(
                            "没有第{}条记录，只有{}条",
                            params.limit,
                            scores.len()
                        ))
                        .await;
                    return;
                }
                let score = &scores[index];
                render_and_send_single_score(
                    ctx,
                    msg,
                    resp_tx,
                    score,
                    params.mode,
                    &user_stats,
                    Some(index),
                    params.is_pass,
                )
                .await;
            } else {
                // Local PP re-computation + cover download: the osu! API
                // may return pp=null for failed scores, loved/pending
                // beatmaps, or unsupported mod combos.  Re-compute locally
                // and download covers in one pass so network I/O overlaps.
                let mode = params.mode;
                let results =
                    futures_util::future::join_all(scores.iter().enumerate().map(|(i, s)| {
                        let cover_url = s.cover_url.clone();
                        let needs_enrich = s.pp.is_none() && s.beatmap_id > 0;
                        let score_clone = if needs_enrich { Some(s.clone()) } else { None };
                        async move {
                            let enriched = if let Some(mut sc) = score_clone {
                                osubot_core::enrich_score_with_pp(&mut sc, mode, false).await;
                                Some(sc)
                            } else {
                                None
                            };
                            let cover = if !cover_url.is_empty() {
                                match osubot_render::cache::fetch_and_cache(
                                    &cover_url,
                                    osubot_render::cache::http_client(),
                                )
                                .await
                                {
                                    Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
                                    Err(_) => None,
                                }
                            } else {
                                None
                            };
                            (i, enriched, cover)
                        }
                    }))
                    .await;

                let scores_mut = Arc::make_mut(&mut scores);
                let mut cover_images: Vec<Option<image::DynamicImage>> =
                    vec![None; scores_mut.len()];
                for (i, enriched, cover) in results {
                    if let Some(new_s) = enriched {
                        scores_mut[i] = new_s;
                    }
                    cover_images[i] = cover;
                }

                // 分数列表(!ps / !rs)固定主题色,不做动态色调提取。!p / !r 单 score card 仍走 extract_dominant_hue。

                let avatar_url = format!("https://a.ppy.sh/{}", user_stats.user_id);
                let hero_cover_url = user_stats.cover_url.clone().unwrap_or_default();
                let user_global_rank = if user_stats.rank > 0 {
                    Some(user_stats.rank)
                } else {
                    None
                };
                let user_country_rank = if user_stats.country_rank > 0 {
                    Some(user_stats.country_rank)
                } else {
                    None
                };
                let change = ctx
                    .storage
                    .calculate_change(user_id, params.mode, &user_stats)
                    .inspect_err(|e| {
                        tracing::warn!(
                            user_id = user_id,
                            mode = ?params.mode,
                            error = %e,
                            "Failed to calculate change"
                        )
                    })
                    .ok()
                    .flatten();
                let pp_change = change.as_ref().and_then(|c| c.pp_change);
                let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
                let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);
                let render_result = tokio::time::timeout(
                    std::time::Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
                    osubot_render::render_score_list_card(osubot_render::ScoreListCardParams {
                        scores: &scores,
                        username: &dedup_username,
                        mode: params.mode,
                        is_pass: params.is_pass,
                        avatar_url: &avatar_url,
                        cover_images,
                        user_pp: user_stats.pp,
                        user_global_rank,
                        user_country_rank,
                        country_code: &user_stats.country_code,
                        pp_change,
                        global_rank_change,
                        country_rank_change,
                        hero_cover_url: &hero_cover_url,
                    }),
                )
                .await;

                match render_result {
                    Ok(Ok(jpeg_bytes)) => {
                        tracing::info!("Score list card rendered, {} bytes", jpeg_bytes.len());
                        let jpeg = Arc::new(jpeg_bytes);
                        let write = ctx.write.clone();
                        let group_id = msg.group_id;
                        let resp_tx_img = resp_tx.clone();
                        tokio::spawn(async move {
                            if send_group_msg_with_image(&write, group_id, &jpeg)
                                .await
                                .is_err()
                            {
                                let _ = resp_tx_img.send("图片发送失败".to_string()).await;
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        warn!(error = %e, "render_score_list_card failed, falling back to text");
                        let response =
                            format_scores(&scores, &dedup_username, params.mode, params.is_pass);
                        let _ = resp_tx.send(response).await;
                    }
                    Err(_) => {
                        warn!("render_score_list_card timed out, falling back to text");
                        let response =
                            format_scores(&scores, &dedup_username, params.mode, params.is_pass);
                        let _ = resp_tx.send(response).await;
                    }
                }
            }
        }
        Err(err_msg) => {
            let _ = resp_tx.send(err_msg).await;
        }
    }
}

async fn handle_beatmap_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
) {
    let (mode, username, qq, beatmap_id, score_id, mods, limit, is_all) = match cmd {
        Command::ScoreOnBeatmap {
            mode,
            username,
            qq,
            beatmap_id,
            score_id,
            mods,
            limit,
            is_all,
        } => (
            *mode,
            username.as_deref(),
            *qq,
            *beatmap_id,
            *score_id,
            mods.clone(),
            *limit,
            *is_all,
        ),
        _ => return,
    };

    if let Some(sid) = score_id {
        info!(score_id = sid, "ScoreOnBeatmap by score_id");
        let dedup_rate_limiter = ctx.rate_limiter.clone();
        let dedup_oauth = ctx.oauth.clone();
        let sid_key = sid as i64;
        let score_result = score_by_id_dedup()
            .run_or_wait((sid_key, mode), move || {
                let rate_limiter = dedup_rate_limiter.clone();
                let oauth = dedup_oauth.clone();
                async move {
                    api::get_score_by_id(&rate_limiter, &oauth, sid)
                        .await
                        .map_err(|e| match e {
                            ApiError::NotFound => "未找到该成绩".to_string(),
                            e => {
                                warn!(error = ?e, "get_score_by_id failed");
                                "获取成绩失败，请稍后再试".to_string()
                            }
                        })
                }
            })
            .await;
        let score = match score_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };
        ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);

        let user_id = score.user.user_id.unwrap_or(0);
        if user_id == 0 {
            let _ = resp_tx.send("无法获取该玩家信息".to_string()).await;
            return;
        }
        let user_stats = match api::fetch_user_stats_by_user_id(
            &ctx.rate_limiter,
            &ctx.oauth,
            user_id,
            mode,
        )
        .await
        {
            Ok(stats) => {
                if let Err(e) = ctx.storage.set_user_id(&stats.username, stats.user_id) {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "Failed to cache user_id"
                    );
                }
                ctx.scheduler.trigger_update(user_id, mode).await;
                stats
            }
            Err(e) => {
                warn!(user_id = user_id, error = ?e, "fetch_user_stats_by_user_id failed for score_id query");
                let _ = resp_tx
                    .send("获取用户数据失败，请稍后再试".to_string())
                    .await;
                return;
            }
        };
        render_and_send_single_score(ctx, msg, resp_tx, &score, mode, &user_stats, None, true)
            .await;
        return;
    }

    let resolved_bid = match beatmap_id {
        Some(bid) => bid,
        None => match ctx.last_beatmap.get(msg.group_id) {
            Some(bid) => bid,
            None => {
                let _ = resp_tx
                    .send("请提供谱面 ID 或先查询一张图".to_string())
                    .await;
                return;
            }
        },
    };

    info!(
        beatmap_id = resolved_bid,
        mode = ?mode,
        mods = ?mods,
        limit,
        is_all,
        "ScoreOnBeatmap"
    );
    ctx.last_beatmap.set(msg.group_id, resolved_bid);

    let (_user_id, username_str, user_stats) = match resolve_score_user(
        ctx,
        msg,
        &username.map(|s| s.to_string()),
        &qq,
        mode,
        resp_tx,
    )
    .await
    {
        Some(result) => result,
        None => return,
    };

    ctx.scheduler.trigger_update(_user_id, mode).await;

    if is_all {
        let api_limit = if limit > 1 { Some(limit) } else { None };
        let key = (_user_id, resolved_bid as i64, mode, api_limit);
        let dedup_rall = ctx.rate_limiter.clone();
        let dedup_oall = ctx.oauth.clone();
        let scores_result = beatmap_scores_dedup()
            .run_or_wait(key, move || {
                let rate_limiter = dedup_rall.clone();
                let oauth = dedup_oall.clone();
                async move {
                    api::get_user_beatmap_scores_all(
                        &rate_limiter,
                        &oauth,
                        resolved_bid as i64,
                        _user_id,
                        mode,
                        api_limit,
                    )
                    .await
                    .map_err(|e| match e {
                        ApiError::NotFound => "该玩家在此谱面上没有成绩".to_string(),
                        e => {
                            warn!(error = ?e, "get_user_beatmap_scores_all failed");
                            "获取成绩失败，请稍后再试".to_string()
                        }
                    })
                }
            })
            .await;
        let scores = match scores_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };
        if scores.is_empty() {
            let _ = resp_tx.send("该玩家在此谱面上没有成绩".to_string()).await;
            return;
        }
        render_and_send_score_list(ctx, msg, resp_tx, &scores, &user_stats, &username_str, mode)
            .await;
    } else if limit == 1 {
        let key = (_user_id, resolved_bid as i64, mode, mods.clone());
        let dedup_rscore = ctx.rate_limiter.clone();
        let dedup_oscore = ctx.oauth.clone();
        let dedup_mods = mods.clone();
        let score_result = beatmap_score_dedup()
            .run_or_wait(key, move || {
                let rate_limiter = dedup_rscore.clone();
                let oauth = dedup_oscore.clone();
                let mods = dedup_mods.clone();
                async move {
                    api::get_user_beatmap_score(
                        &rate_limiter,
                        &oauth,
                        resolved_bid as i64,
                        _user_id,
                        mode,
                        &mods,
                    )
                    .await
                    .map_err(|e| match e {
                        ApiError::NotFound => {
                            if mods.is_some() {
                                "未找到符合该 mod 条件的成绩".to_string()
                            } else {
                                "该玩家在此谱面上没有成绩".to_string()
                            }
                        }
                        e => {
                            warn!(
                                error = ?e,
                                beatmap_id = resolved_bid,
                                ?mods,
                                "get_user_beatmap_score failed"
                            );
                            "获取成绩失败，请稍后再试".to_string()
                        }
                    })
                }
            })
            .await;
        let score = match score_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };
        ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
        render_and_send_single_score(ctx, msg, resp_tx, &score, mode, &user_stats, None, true)
            .await;
    } else {
        let n = limit as usize;
        let key = (_user_id, resolved_bid as i64, mode, Some(limit));
        let dedup_rscores = ctx.rate_limiter.clone();
        let dedup_oscores = ctx.oauth.clone();
        let scores_result = beatmap_scores_dedup()
            .run_or_wait(key, move || {
                let rate_limiter = dedup_rscores.clone();
                let oauth = dedup_oscores.clone();
                async move {
                    api::get_user_beatmap_scores_all(
                        &rate_limiter,
                        &oauth,
                        resolved_bid as i64,
                        _user_id,
                        mode,
                        Some(limit),
                    )
                    .await
                    .map_err(|e| match e {
                        ApiError::NotFound => "该玩家在此谱面上没有成绩".to_string(),
                        e => {
                            warn!(error = ?e, "get_user_beatmap_scores_all failed");
                            "获取成绩失败，请稍后再试".to_string()
                        }
                    })
                }
            })
            .await;
        let scores = match scores_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };
        if scores.len() < n {
            let _ = resp_tx
                .send(format!("没有第{}条成绩，仅有{}条", n, scores.len()))
                .await;
            return;
        }
        let score = scores.into_iter().nth(n - 1).expect("len checked above");
        ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
        render_and_send_single_score(
            ctx,
            msg,
            resp_tx,
            &score,
            mode,
            &user_stats,
            Some(n - 1),
            true,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn render_and_send_single_score(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    score: &Score,
    mode: GameMode,
    user_stats: &UserStats,
    position: Option<usize>,
    is_pass: bool,
) {
    let mut score = score.clone();
    enrich_score_with_pp(&mut score, mode, true).await;

    let ur_value = if mode == GameMode::Osu && score.score_id > 0 && score.has_replay {
        tracing::trace!(score_id = score.score_id, mode = ?mode, is_lazer = score.is_lazer, length = score.length_seconds, "Starting UR calculation");
        let rl = ctx.rate_limiter.clone();
        let oa = ctx.oauth.clone();
        let ur_params = osubot_core::ur::ScoreUrParams {
            score_id: score.score_id,
            legacy_score_id: score.legacy_score_id,
            beatmap_id: score.beatmap_id,
            mode,
            mods: score.mods.clone(),
        };
        let ur_timeout = Duration::from_secs(ctx.config.read().await.bot.ur_timeout_secs);
        match tokio::time::timeout(
            ur_timeout,
            osubot_core::ur::calculate_score_ur(&rl, &oa, ur_params),
        )
        .await
        {
            Ok(Some(ur_val)) => {
                tracing::debug!(
                    score_id = score.score_id,
                    total_ur = ur_val,
                    "UR calculation succeeded"
                );
                Some(ur_val)
            }
            Ok(None) => {
                tracing::warn!(score_id = score.score_id, "UR calculation returned None");
                None
            }
            Err(_) => {
                tracing::warn!(score_id = score.score_id, "UR calculation timed out");
                None
            }
        }
    } else {
        tracing::trace!(
            score_id = score.score_id,
            mode = ?mode,
            is_lazer = score.is_lazer,
            has_replay = score.has_replay,
            "Skipping UR calculation"
        );
        None
    };

    let (ar_eff, od_eff, cs_eff, hp_eff) = {
        let (a, o, c, h) = apply_mod_adjustment_to_stats(
            mode,
            score.ar,
            score.od,
            score.cs,
            score.hp,
            &score.mods,
        );
        let same = (a - score.ar).abs() < 0.01
            && (o - score.od).abs() < 0.01
            && (c - score.cs).abs() < 0.01
            && (h - score.hp).abs() < 0.01;
        if same {
            (None, None, None, None)
        } else {
            (Some(a), Some(o), Some(c), Some(h))
        }
    };

    let cover_image: Option<image::DynamicImage> = if !score.cover_url.is_empty() {
        match render_cache::fetch_and_cache(&score.cover_url, render_cache::http_client()).await {
            Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
            Err(_) => None,
        }
    } else {
        None
    };

    let play_time = format_play_datetime(&score.created_at);
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel_flag.clone();

    let change = ctx
        .storage
        .calculate_change(user_stats.user_id, mode, user_stats)
        .ok()
        .flatten();
    let pp_change = change.as_ref().and_then(|c| c.pp_change);
    let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
    let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);

    let render_timeout = Duration::from_secs(ctx.config.read().await.bot.render_timeout_secs);
    let render_result = tokio::time::timeout(
        render_timeout,
        render_score_card(osubot_render::ScoreCardParams {
            score: &score,
            username: &user_stats.username,
            mode,
            user_pp: user_stats.pp,
            user_global_rank: if user_stats.rank > 0 {
                Some(user_stats.rank)
            } else {
                None
            },
            user_country_rank: if user_stats.country_rank > 0 {
                Some(user_stats.country_rank)
            } else {
                None
            },
            country_code: &user_stats.country_code,
            avatar_url: &format!("https://a.ppy.sh/{}", user_stats.user_id),
            play_time: &play_time,
            fav_count: score.fav_count,
            play_count: score.play_count,
            pp_change,
            global_rank_change,
            country_rank_change,
            ranked_status: &score.status,
            ur_value,
            ar_eff,
            od_eff,
            cs_eff,
            hp_eff,
            cover_image,
            cancel_flag: Some(cancel_clone),
        }),
    )
    .await;

    match render_result {
        Ok(Ok(jpeg_bytes)) => {
            let write = ctx.write.clone();
            let group_id = msg.group_id;
            let resp_tx_img = resp_tx.clone();
            tokio::spawn(async move {
                if send_group_msg_with_image(&write, group_id, &jpeg_bytes)
                    .await
                    .is_err()
                {
                    let _ = resp_tx_img.send("图片发送失败".to_string()).await;
                }
            });
        }
        Ok(Err(e)) => {
            warn!(error = %e, "render_score_card failed, falling back to text");
            let text = format_score(&score, &user_stats.username, mode, position, is_pass);
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            cancel_flag.store(true, Ordering::Relaxed);
            warn!("render_score_card timed out, falling back to text");
            let text = format_score(&score, &user_stats.username, mode, position, is_pass);
            let _ = resp_tx.send(text).await;
        }
    }
}

async fn render_and_send_score_list(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    scores: &[Score],
    user_stats: &UserStats,
    username: &str,
    mode: GameMode,
) {
    let results = join_all(scores.iter().enumerate().map(|(i, s)| {
        let cover_url = s.cover_url.clone();
        let needs_enrich = s.pp.is_none() && s.beatmap_id > 0;
        let score_clone = if needs_enrich { Some(s.clone()) } else { None };
        async move {
            let enriched = if let Some(mut sc) = score_clone {
                enrich_score_with_pp(&mut sc, mode, false).await;
                Some(sc)
            } else {
                None
            };
            let cover = if !cover_url.is_empty() {
                match render_cache::fetch_and_cache(&cover_url, render_cache::http_client()).await {
                    Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
                    Err(_) => None,
                }
            } else {
                None
            };
            (i, enriched, cover)
        }
    }))
    .await;

    let scores_vec: Vec<Score> = scores.to_vec();
    let mut scores_mut = scores_vec;
    let mut cover_images: Vec<Option<image::DynamicImage>> = vec![None; scores_mut.len()];
    for (i, enriched, cover) in results {
        if let Some(new_s) = enriched {
            scores_mut[i] = new_s;
        }
        cover_images[i] = cover;
    }

    let avatar_url = format!("https://a.ppy.sh/{}", user_stats.user_id);
    let hero_cover_url = user_stats.cover_url.clone().unwrap_or_default();
    let user_global_rank = if user_stats.rank > 0 {
        Some(user_stats.rank)
    } else {
        None
    };
    let user_country_rank = if user_stats.country_rank > 0 {
        Some(user_stats.country_rank)
    } else {
        None
    };

    let change = ctx
        .storage
        .calculate_change(user_stats.user_id, mode, user_stats)
        .inspect_err(|e| {
            tracing::warn!(
                user_id = user_stats.user_id,
                mode = ?mode,
                error = %e,
                "Failed to calculate change"
            )
        })
        .ok()
        .flatten();
    let pp_change = change.as_ref().and_then(|c| c.pp_change);
    let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
    let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);

    let render_result = tokio::time::timeout(
        Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
        render_score_list_card(osubot_render::ScoreListCardParams {
            scores: &scores_mut,
            username,
            mode,
            is_pass: true,
            avatar_url: &avatar_url,
            cover_images,
            user_pp: user_stats.pp,
            user_global_rank,
            user_country_rank,
            country_code: &user_stats.country_code,
            pp_change,
            global_rank_change,
            country_rank_change,
            hero_cover_url: &hero_cover_url,
        }),
    )
    .await;

    match render_result {
        Ok(Ok(jpeg_bytes)) => {
            let write = ctx.write.clone();
            let group_id = msg.group_id;
            let resp_tx_img = resp_tx.clone();
            tokio::spawn(async move {
                if send_group_msg_with_image(&write, group_id, &jpeg_bytes)
                    .await
                    .is_err()
                {
                    let _ = resp_tx_img.send("图片发送失败".to_string()).await;
                }
            });
        }
        Ok(Err(e)) => {
            warn!(error = %e, "render_score_list_card failed, falling back to text");
            let text = format_scores(&scores_mut, username, mode, true);
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            warn!("render_score_list_card timed out, falling back to text");
            let text = format_scores(&scores_mut, username, mode, true);
            let _ = resp_tx.send(text).await;
        }
    }
}

/// Main command dispatcher. Parses the command text, resolves the target user,
/// executes the appropriate query, and sends the response via `resp_tx`.
async fn handle_command(ctx: BotContext, msg: QQMessage, resp_tx: mpsc::Sender<String>) {
    // Plugin on_message dispatch — let plugins intercept raw messages before command parsing
    {
        let mut pm_guard = ctx.plugin_manager.lock().await;
        if let Some(ref mut pm) = *pm_guard {
            if !pm.is_empty() {
                let msg_payload = serde_json::json!({
                    "group_id": msg.group_id,
                    "user_id": msg.user_id,
                    "message": msg.message,
                    "mentioned_user_id": msg.mentioned_user_id,
                });
                match pm.handle_message(&msg_payload.to_string()).await {
                    osubot_plugin::PluginActionResult::Handled(response) => {
                        let _ = resp_tx.send(response).await;
                        return;
                    }
                    osubot_plugin::PluginActionResult::Intercepted => {
                        return;
                    }
                    osubot_plugin::PluginActionResult::Next => {}
                }
            }
        }
    }

    let cmd_opt = parse_command(&msg.message, msg.mentioned_user_id);

    // 未识别命令 — 走插件 on_command 分发，插件无法处理则直接结束
    if cmd_opt.is_none() {
        let cmd_name = msg.message.split_whitespace().next().unwrap_or("");
        if cmd_name.is_empty() {
            return;
        }
        let cmd_payload = serde_json::json!({
            "command_type": cmd_name,
            "group_id": msg.group_id,
            "user_id": msg.user_id,
            "message": msg.message,
            "mentioned_user_id": msg.mentioned_user_id,
        });
        let mut pm_guard = ctx.plugin_manager.lock().await;
        if let Some(ref mut pm) = *pm_guard {
            if !pm.is_empty() {
                match pm.handle_command(cmd_name, &cmd_payload.to_string()).await {
                    osubot_plugin::PluginActionResult::Handled(response) => {
                        let _ = resp_tx.send(response).await;
                    }
                    osubot_plugin::PluginActionResult::Intercepted => {}
                    osubot_plugin::PluginActionResult::Next => {}
                }
            }
        }
        return;
    }
    let cmd = cmd_opt.expect("guarded by cmd_opt.is_none() early-return");

    // 命令开关检查
    let group_cfg = {
        let cfg = ctx.config.read().await;
        cfg.groups.get_group_config(msg.group_id)
    };
    if !group_cfg.is_enabled(cmd.group_name()) {
        debug!(group_id = msg.group_id, command = ?cmd.group_name(), "命令已禁用，跳过");
        return;
    }

    // 用户命令频率限制（滑动窗口：3秒内最多5次）
    let rate_limited = {
        let mut entry = ctx
            .command_rate_limits
            .entry(msg.user_id)
            .or_insert(UserRateLimit {
                last_command: std::time::Instant::now(),
                command_timestamps: Vec::new(),
            });

        let now = std::time::Instant::now();
        // 清理超过3秒的记录
        entry
            .command_timestamps
            .retain(|t| now.duration_since(*t) < Duration::from_secs(3));
        entry.command_timestamps.push(now);
        entry.last_command = now;

        // 检查是否超过限制
        entry.command_timestamps.len() > 5
    };
    if rate_limited {
        let _ = resp_tx.send("操作太频繁，请稍后再试".to_string()).await;
        return;
    }

    // 定期清理不活跃的用户（每60秒清理30秒内无命令的用户）
    static LAST_CLEANUP: OnceLock<std::sync::Mutex<std::time::Instant>> = OnceLock::new();
    let last = LAST_CLEANUP.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
    if let Ok(mut last_time) = last.try_lock() {
        if last_time.elapsed() >= Duration::from_secs(60) {
            ctx.command_rate_limits
                .retain(|_, v| v.last_command.elapsed() < Duration::from_secs(30));
            *last_time = std::time::Instant::now();
        }
    }

    // Plugin command dispatch — let plugins intercept before default handler
    {
        let mut pm_guard = ctx.plugin_manager.lock().await;
        if let Some(ref mut pm) = *pm_guard {
            if !pm.is_empty() {
                let cmd_name = cmd.command_name();

                fn mode_to_u8(mode: &GameMode) -> u8 {
                    match mode {
                        GameMode::Osu => 0,
                        GameMode::Taiko => 1,
                        GameMode::Catch => 2,
                        GameMode::Mania => 3,
                    }
                }

                let mode = match &cmd {
                    Command::QuerySelf { mode }
                    | Command::QueryUser { mode, .. }
                    | Command::QueryMentionedUser { mode, .. }
                    | Command::Pass { mode, .. }
                    | Command::Recent { mode, .. }
                    | Command::Highlight { mode, .. }
                    | Command::ScoreOnBeatmap { mode, .. } => Some(mode_to_u8(mode)),
                    Command::ProfileCard { .. } | Command::Bind { .. } | Command::Unbind => None,
                };

                let username = match &cmd {
                    Command::QueryUser { username, .. } => Some(username.as_str()),
                    Command::Bind { username, .. } => Some(username.as_str()),
                    Command::ScoreOnBeatmap { username, .. }
                    | Command::Pass { username, .. }
                    | Command::Recent { username, .. }
                    | Command::ProfileCard { username, .. } => username.as_deref(),
                    _ => None,
                };

                let cmd_payload = serde_json::json!({
                    "command_type": cmd_name,
                    "group_id": msg.group_id,
                    "user_id": msg.user_id,
                    "message": msg.message,
                    "mentioned_user_id": msg.mentioned_user_id,
                    "mode": mode,
                    "username": username,
                    "qq": match &cmd {
                        Command::QueryMentionedUser { qq, .. } => Some(qq),
                        _ => None,
                    },
                    "beatmap_id": match &cmd {
                        Command::ScoreOnBeatmap { beatmap_id, .. } => *beatmap_id,
                        _ => None,
                    },
                    "score_id": match &cmd {
                        Command::ScoreOnBeatmap { score_id, .. } => *score_id,
                        _ => None,
                    },
                    "mods": match &cmd {
                        Command::ScoreOnBeatmap { mods, .. } => {
                            mods.as_ref().map(|m| m.iter().map(|m| m.to_string()).collect::<Vec<_>>())
                        }
                        _ => None,
                    },
                    "limit": match &cmd {
                        Command::ScoreOnBeatmap { limit, .. } | Command::Pass { limit, .. } | Command::Recent { limit, .. } => Some(*limit),
                        _ => None,
                    },
                });

                match pm.handle_command(cmd_name, &cmd_payload.to_string()).await {
                    osubot_plugin::PluginActionResult::Handled(response) => {
                        let _ = resp_tx.send(response).await;
                        return;
                    }
                    osubot_plugin::PluginActionResult::Intercepted => {
                        return;
                    }
                    osubot_plugin::PluginActionResult::Next => {}
                }
            }
        }
    }

    // Handle command and send response
    match cmd {
        Command::QuerySelf { mode } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "QuerySelf command");
            match ctx.storage.get_binding(msg.user_id) {
                Ok(Some((user_id, username))) => {
                    ctx.fetch_stats_and_reply(
                        msg.user_id,
                        user_id,
                        &username,
                        mode,
                        &resp_tx,
                        "QuerySelf",
                    )
                    .await;
                }
                Ok(None) => {
                    if let Some((user_id, username)) = ctx.resolve_binding(msg.user_id).await {
                        info!(user_id = msg.user_id, osu_id = user_id, username = %username, "QuerySelf auto-bound via upstream");
                        ctx.fetch_stats_and_reply(
                            msg.user_id,
                            user_id,
                            &username,
                            mode,
                            &resp_tx,
                            "QuerySelf (auto-bound)",
                        )
                        .await;
                    } else {
                        info!(user_id = msg.user_id, "QuerySelf but no binding");
                        let _ = resp_tx
                            .send("请先绑定 osu! 用户名，使用 绑定 <用户名>".to_string())
                            .await;
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "QuerySelf database error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::QueryUser { username, mode } => {
            info!(group_id = msg.group_id, username = %username, mode = ?mode, "QueryUser command");
            match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, &username, mode)
                .await
            {
                Ok(stats) => {
                    // Cache user_id for future lookups (even for unbound users)
                    if let Err(e) = ctx.storage.set_user_id(&stats.username, stats.user_id) {
                        tracing::warn!(
                            username = %stats.username,
                            user_id = stats.user_id,
                            error = %e,
                            "Failed to cache user_id"
                        );
                    }
                    if stats.username != username {
                        if let Err(e) = ctx.storage.set_user_id(&username, stats.user_id) {
                            tracing::warn!(
                                username = %username,
                                user_id = stats.user_id,
                                error = %e,
                                "Failed to cache user_id"
                            );
                        }
                    }
                    ctx.scheduler.trigger_update(stats.user_id, mode).await;
                    let change = ctx
                        .storage
                        .calculate_change(stats.user_id, mode, &stats)
                        .inspect_err(|e| {
                            tracing::warn!(
                                user_id = stats.user_id,
                                mode = ?mode,
                                error = %e,
                                "Failed to calculate change"
                            )
                        })
                        .ok()
                        .flatten();
                    let has_change = change.is_some();
                    info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QueryUser success");
                    let response = format_stats_with_change(&stats, &change, mode);
                    let _ = resp_tx.send(response).await;
                    if !has_change {
                        info!(username = %username, "QueryUser - no change data available");
                    }
                }
                Err(e) => {
                    warn!(username = %username, mode = ?mode, error = ?e, "QueryUser failed");
                    let _ = resp_tx.send(api_error_msg(&e)).await;
                }
            }
        }
        Command::QueryMentionedUser { qq, mode } => {
            info!(qq = qq, group_id = msg.group_id, mode = ?mode, "QueryMentionedUser command");
            match ctx.storage.get_binding(qq) {
                Ok(Some((user_id, username))) => {
                    ctx.fetch_stats_and_reply(
                        qq,
                        user_id,
                        &username,
                        mode,
                        &resp_tx,
                        "QueryMentionedUser",
                    )
                    .await;
                }
                Ok(None) => {
                    if let Some((user_id, username)) = ctx.resolve_binding(qq).await {
                        info!(qq = qq, osu_id = user_id, username = %username, "QueryMentionedUser auto-bound via upstream");
                        ctx.fetch_stats_and_reply(
                            qq,
                            user_id,
                            &username,
                            mode,
                            &resp_tx,
                            "QueryMentionedUser (auto-bound)",
                        )
                        .await;
                    } else {
                        info!(qq = qq, "QueryMentionedUser but no binding");
                        let _ = resp_tx
                            .send(
                                "该用户未绑定 osu! 账号，请使用 绑定 <osu用户名> 命令绑定"
                                    .to_string(),
                            )
                            .await;
                    }
                }
                Err(_) => {
                    error!(qq = qq, "QueryMentionedUser database error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::Bind { username } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, username = %username, "Bind command");
            match ctx.storage.get_binding(msg.user_id) {
                Ok(Some((_, existing_username))) => {
                    info!(user_id = msg.user_id, existing = %existing_username, "Bind but already bound");
                    let _ = resp_tx
                        .send(format!(
                            "[CQ:at,qq={}] 你已经绑定为{},如需修改请先解绑",
                            msg.user_id, existing_username
                        ))
                        .await;
                }
                Ok(None) => {
                    let irc_nickname = {
                        let cfg = ctx.config.read().await;
                        if cfg.irc.enabled {
                            Some(cfg.irc.nickname.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(nickname) = irc_nickname {
                        match ctx.storage.has_pending_bind(msg.user_id) {
                            Ok(true) => {
                                let _ = resp_tx
                                    .send(format!(
                                        "[CQ:at,qq={}] 你已有进行中的绑定请求，请等待当前验证码过期后再试",
                                        msg.user_id
                                    ))
                                    .await;
                                return;
                            }
                            Err(_) => {
                                error!(user_id = msg.user_id, "Failed to check pending bind");
                                let _ = resp_tx
                                    .send(format!(
                                        "[CQ:at,qq={}] 绑定失败，请稍后重试",
                                        msg.user_id
                                    ))
                                    .await;
                                return;
                            }
                            _ => {}
                        }
                        match ctx
                            .storage
                            .add_pending_bind(msg.user_id, msg.group_id, &username)
                        {
                            Ok(code) => {
                                info!(user_id = msg.user_id, username = %username, code = %code, "Pending bind created");
                                let _ = resp_tx
                                    .send(format!(
                                        "[CQ:at,qq={}] 您的验证码是 {}，请在两分钟内通过osu!发送私信给 {} 来完成验证",
                                        msg.user_id, code, nickname
                                    ))
                                    .await;
                            }
                            Err(_) => {
                                error!(user_id = msg.user_id, "Failed to create pending bind");
                                let _ = resp_tx
                                    .send(format!(
                                        "[CQ:at,qq={}] 绑定失败，请稍后重试",
                                        msg.user_id
                                    ))
                                    .await;
                            }
                        }
                    } else {
                        match api::get_user_info(&ctx.rate_limiter, &ctx.oauth, &username).await {
                            Ok(Some(user_info)) => {
                                if let Err(e) = ctx.storage.set_user_id(&username, user_info.id) {
                                    warn!("Failed to cache user_id for {username}: {e}");
                                }
                                match ctx.storage.bind(
                                    msg.user_id,
                                    user_info.id,
                                    &user_info.username,
                                ) {
                                    Ok(Ok(())) => {
                                        info!(user_id = msg.user_id, username = %user_info.username, "Bind success");
                                        let _ = resp_tx
                                            .send(format!(
                                                "[CQ:at,qq={}] 成功绑定为{}",
                                                msg.user_id, user_info.username
                                            ))
                                            .await;
                                    }
                                    Ok(Err(bound_qq)) => {
                                        info!(user_id = msg.user_id, username = %username, bound_qq = bound_qq, "Bind failed - username already bound");
                                        let _ = resp_tx
                                            .send(format!(
                                                "[CQ:at,qq={}] 该 osu! 用户已绑定其他QQ",
                                                msg.user_id
                                            ))
                                            .await;
                                    }
                                    Err(_) => {
                                        error!(user_id = msg.user_id, username = %username, "Bind failed");
                                        let _ = resp_tx
                                            .send(format!(
                                                "[CQ:at,qq={}] 绑定失败，请稍后重试",
                                                msg.user_id
                                            ))
                                            .await;
                                    }
                                }
                            }
                            Ok(None) => {
                                info!(username = %username, "Bind but user not found");
                                let _ = resp_tx
                                    .send(format!("[CQ:at,qq={}] 未找到该 osu! 用户", msg.user_id))
                                    .await;
                            }
                            Err(e) => {
                                warn!(username = %username, error = ?e, "Bind - user info check failed");
                                let err_msg = match e {
                                    ApiError::NotFound => "未找到该用户".to_string(),
                                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                    ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                                        format!("查询繁忙，请 {} 秒后再试", secs)
                                    }
                                    ApiError::RateLimitedWithRetryAfter(None) => {
                                        "查询繁忙，请稍后再试".to_string()
                                    }
                                    ApiError::ClientRateLimited => {
                                        "本地请求限流，请稍后再试".to_string()
                                    }
                                    _ => "查询失败，请稍后重试".to_string(),
                                };
                                let _ = resp_tx
                                    .send(format!("[CQ:at,qq={}] {}", msg.user_id, err_msg))
                                    .await;
                            }
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "Bind database error");
                    let _ = resp_tx
                        .send(format!("[CQ:at,qq={}] 数据库错误", msg.user_id))
                        .await;
                }
            }
        }
        Command::Unbind => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "Unbind command"
            );
            // Check if user has pending unbind confirmation (within 5 minutes)
            match ctx.storage.get_pending_unbind(msg.user_id) {
                Ok(Some(_)) => {
                    // Execute unbind and clear pending
                    match ctx.storage.unbind(msg.user_id) {
                        Ok(_) => {
                            if let Err(e) = ctx.storage.remove_pending_unbind(msg.user_id) {
                                tracing::warn!(
                                    user_id = msg.user_id,
                                    error = %e,
                                    "Failed to remove pending unbind"
                                );
                            }
                            info!(user_id = msg.user_id, "Unbind success");
                            let _ = resp_tx
                                .send(format!("[CQ:at,qq={}] 解绑成功", msg.user_id))
                                .await;
                        }
                        Err(_) => {
                            error!(user_id = msg.user_id, "Unbind failed");
                            let _ = resp_tx
                                .send(format!("[CQ:at,qq={}] 解绑失败，请稍后重试", msg.user_id))
                                .await;
                        }
                    }
                }
                Ok(None) => {
                    // Ask for confirmation and set pending
                    match ctx.storage.get_binding(msg.user_id) {
                        Ok(Some((_, current_username))) => {
                            if let Err(e) = ctx.storage.set_pending_unbind(msg.user_id) {
                                tracing::warn!(
                                    user_id = msg.user_id,
                                    error = %e,
                                    "Failed to set pending unbind"
                                );
                            }
                            info!(user_id = msg.user_id, username = %current_username, "Unbind confirmation requested");
                            let _ = resp_tx
                                .send(format!(
                                    "[CQ:at,qq={}] 确定要解除绑定 {} 吗？回复\"解绑\"确认",
                                    msg.user_id, current_username
                                ))
                                .await;
                        }
                        Ok(None) => {
                            info!(user_id = msg.user_id, "Unbind but no binding");
                            let _ = resp_tx
                                .send(format!(
                                    "[CQ:at,qq={}] 你还没有绑定任何 osu! 用户",
                                    msg.user_id
                                ))
                                .await;
                        }
                        Err(_) => {
                            error!(user_id = msg.user_id, "Unbind database error");
                            let _ = resp_tx
                                .send(format!("[CQ:at,qq={}] 数据库错误", msg.user_id))
                                .await;
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "Unbind pending check error");
                    let _ = resp_tx
                        .send(format!("[CQ:at,qq={}] 数据库错误", msg.user_id))
                        .await;
                }
            }
        }
        Command::Highlight { mode } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "Highlight command");

            let group_members =
                match get_group_member_list(&ctx.write, &ctx.onebot_api, msg.group_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(error = %e, "Failed to get group member list");
                        let _ = resp_tx
                            .send("无法获取群成员列表，请稍后重试".to_string())
                            .await;
                        return;
                    }
                };

            let all_bindings = match ctx.storage.get_all_user_bindings() {
                Ok(bindings) => bindings,
                Err(_) => {
                    error!("Highlight failed to get bindings");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                    return;
                }
            };

            let group_bindings: Vec<(i64, i64, String)> = all_bindings
                .into_iter()
                .filter(|(qq, _, _)| group_members.contains(qq))
                .collect();

            if group_bindings.is_empty() {
                let _ = resp_tx
                    .send("你群根本没有人绑定 osu! 账号".to_string())
                    .await;
                return;
            }

            match get_highlight(
                &ctx.storage,
                &ctx.rate_limiter,
                &ctx.oauth,
                &group_bindings,
                mode,
            )
            .await
            {
                Ok(result) => {
                    let response = format_highlight(&result);
                    let _ = resp_tx.send(response).await;
                }
                Err(e) => {
                    warn!(error = ?e, "Highlight fetch failed");
                    let err_msg = match e {
                        HighlightError::NoData => "你群根本没有人屙屎。".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    };
                    let _ = resp_tx.send(err_msg).await;
                }
            }
        }
        Command::ProfileCard { username, qq } => {
            let target_user_id = match username {
                Some(ref name) => {
                    if let Ok(Some(cached_id)) = ctx.storage.get_user_id(name) {
                        info!(username = %name, user_id = cached_id, "ProfileCard resolved from local cache");
                        cached_id
                    } else {
                        match api::fetch_user_stats_by_username(
                            &ctx.rate_limiter,
                            &ctx.oauth,
                            name,
                            GameMode::Osu,
                        )
                        .await
                        {
                            Ok(stats) => {
                                info!(username = %name, user_id = stats.user_id, "ProfileCard resolved by username");
                                if let Err(e) =
                                    ctx.storage.set_user_id(&stats.username, stats.user_id)
                                {
                                    tracing::warn!(
                                        username = %stats.username,
                                        user_id = stats.user_id,
                                        error = %e,
                                        "Failed to cache user_id"
                                    );
                                }
                                stats.user_id
                            }
                            Err(e) => {
                                warn!(username = %name, error = ?e, "ProfileCard username resolution failed");
                                let err_msg = match e {
                                    ApiError::NotFound => "未找到该用户".to_string(),
                                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                    ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                                        format!("查询繁忙，请 {} 秒后再试", secs)
                                    }
                                    ApiError::RateLimitedWithRetryAfter(None) => {
                                        "查询繁忙，请稍后再试".to_string()
                                    }
                                    ApiError::ClientRateLimited => {
                                        "本地请求限流，请稍后再试".to_string()
                                    }
                                    _ => "查询失败，请稍后重试".to_string(),
                                };
                                let _ = resp_tx.send(err_msg).await;
                                return;
                            }
                        }
                    }
                }
                None => {
                    match qq {
                        Some(mentioned_qq) => match ctx.storage.get_binding(mentioned_qq) {
                            Ok(Some((user_id, current_username))) => {
                                info!(qq = mentioned_qq, osu_id = user_id, username = %current_username, "ProfileCard mention");
                                user_id
                            }
                            Ok(None) => {
                                if let Some((uid, uname)) = ctx.resolve_binding(mentioned_qq).await
                                {
                                    info!(qq = mentioned_qq, osu_id = uid, username = %uname, "ProfileCard mention auto-bound");
                                    uid
                                } else {
                                    info!(qq = mentioned_qq, "ProfileCard mention but no binding");
                                    let _ = resp_tx
                                        .send("该用户未绑定 osu! 账号，请使用 绑定 <osu用户名> 命令绑定".to_string())
                                        .await;
                                    return;
                                }
                            }
                            Err(_) => {
                                error!(qq = mentioned_qq, "ProfileCard mention database error");
                                let _ = resp_tx.send("数据库错误".to_string()).await;
                                return;
                            }
                        },
                        None => match ctx.storage.get_binding(msg.user_id) {
                            Ok(Some((user_id, current_username))) => {
                                info!(user_id = msg.user_id, osu_id = user_id, username = %current_username, "ProfileCard self");
                                user_id
                            }
                            Ok(None) => {
                                if let Some((uid, uname)) = ctx.resolve_binding(msg.user_id).await {
                                    info!(user_id = msg.user_id, osu_id = uid, username = %uname, "ProfileCard self auto-bound");
                                    uid
                                } else {
                                    let _ = resp_tx
                                        .send("请先绑定 osu! 用户名，或使用 !profile <用户名> 查询他人".to_string())
                                        .await;
                                    return;
                                }
                            }
                            Err(_) => {
                                error!(user_id = msg.user_id, "ProfileCard database error");
                                let _ = resp_tx.send("数据库错误".to_string()).await;
                                return;
                            }
                        },
                    }
                }
            };

            info!(user_id = target_user_id, qq = ?qq, "ProfileCard command");

            let dedup_rate_limiter = ctx.rate_limiter.clone();
            let dedup_oauth = ctx.oauth.clone();
            let dedup_target_id = target_user_id;
            let render_result = profile_dedup()
                .run_or_wait((target_user_id, GameMode::Osu), move || async move {
                    let profile = api::fetch_user_profile(
                        &dedup_rate_limiter,
                        &dedup_oauth,
                        dedup_target_id,
                        GameMode::Osu,
                    )
                    .await
                    .map_err(|e| match e {
                        ApiError::NotFound => "未找到该用户".to_string(),
                        ApiError::MissingApiKey => "API Key 未配置".to_string(),
                        ApiError::OAuthError => "OAuth 认证失败".to_string(),
                        ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                            format!("查询繁忙，请 {} 秒后再试", secs)
                        }
                        ApiError::RateLimitedWithRetryAfter(None) => {
                            "查询繁忙，请稍后再试".to_string()
                        }
                        ApiError::ClientRateLimited => "本地请求限流，请稍后再试".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    })?;
                    info!(
                        user_id = dedup_target_id,
                        html_len = profile.html.len(),
                        hue = profile.profile_hue,
                        "ProfileCard HTML fetched"
                    );
                    let profile_render = render_profile_card(
                        &profile.html,
                        profile.profile_hue,
                        &profile.avatar_url,
                        &profile.username,
                        PROFILE_VIEWPORT_WIDTH,
                        1200,
                    );
                    let render_timeout =
                        Duration::from_secs(ctx.config.read().await.bot.render_timeout_secs);
                    tokio::time::timeout(render_timeout, profile_render)
                        .await
                        .map_err(|_| {
                            warn!(user_id = target_user_id, "ProfileCard render timed out");
                            "渲染超时，请稍后重试".to_string()
                        })?
                        .map(Arc::new)
                        .map_err(|e| {
                            warn!(user_id = target_user_id, error = %e, "render failed");
                            "渲染失败，请稍后重试".to_string()
                        })
                })
                .await;

            match render_result {
                Ok(jpeg_bytes) => {
                    info!(
                        user_id = target_user_id,
                        jpeg_len = jpeg_bytes.len(),
                        "ProfileCard rendered"
                    );
                    let write = ctx.write.clone();
                    let group_id = msg.group_id;
                    let resp_tx = resp_tx.clone();
                    tokio::spawn(async move {
                        if send_group_msg_with_image(&write, group_id, &jpeg_bytes)
                            .await
                            .is_err()
                        {
                            let _ = resp_tx.send("图片发送失败".to_string()).await;
                        }
                    });
                }
                Err(msg) => {
                    warn!(user_id = target_user_id, msg = %msg, "ProfileCard failed");
                    let _ = resp_tx.send(msg).await;
                }
            }
        }
        Command::ScoreOnBeatmap { .. } => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "ScoreOnBeatmap command"
            );
            handle_beatmap_score_query(&ctx, &msg, &resp_tx, &cmd).await;
        }
        Command::Pass {
            mode,
            username,
            qq,
            limit,
            is_summary,
        } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, limit = limit, "Pass command");
            handle_score_query(
                &ctx,
                &msg,
                &resp_tx,
                ScoreQueryParams {
                    mode,
                    username: &username,
                    qq: &qq,
                    is_pass: true,
                    limit,
                    is_single: !is_summary,
                },
            )
            .await;
        }
        Command::Recent {
            mode,
            username,
            qq,
            limit,
            is_summary,
        } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, limit = limit, "Recent command");
            handle_score_query(
                &ctx,
                &msg,
                &resp_tx,
                ScoreQueryParams {
                    mode,
                    username: &username,
                    qq: &qq,
                    is_pass: false,
                    limit,
                    is_single: !is_summary,
                },
            )
            .await;
        }
    }
}

use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMsg};
use tracing_subscriber::fmt::time::LocalTime;
type WriteSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMsg,
>;

/// Send a text message to a QQ group via the OneBot WebSocket connection.
async fn send_group_msg(write: &Arc<Mutex<WriteSink>>, group_id: i64, message: &str) {
    let json = serde_json::json!({
        "action": "send_group_msg",
        "params": {
            "group_id": group_id,
            "message": message
        }
    });
    let mut sink = write.lock().await;
    if let Err(e) = sink.send(WsMsg::Text(json.to_string().into())).await {
        tracing::error!("发送群消息失败: {}", e);
    }
}

/// Send a message with a base64-encoded image to a QQ group via the OneBot WebSocket connection.
async fn send_group_msg_with_image(
    write: &Arc<Mutex<WriteSink>>,
    group_id: i64,
    image_data: &[u8],
) -> Result<(), WsError> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(image_data);
    let segments = serde_json::json!([
        {
            "type": "image",
            "data": {
                "file": format!("base64://{}", b64)
            }
        }
    ]);
    let json = serde_json::json!({
        "action": "send_group_msg",
        "params": {
            "group_id": group_id,
            "message": segments
        }
    });
    let mut sink = write.lock().await;
    sink.send(WsMsg::Text(json.to_string().into()))
        .await
        .inspect_err(|e| {
            warn!(error = %e, group_id = group_id, "Failed to send group image message");
        })
}

async fn call_onebot_api(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let echo = next_echo();
    let (tx, rx) = oneshot::channel();

    api.pending.lock().await.insert(
        echo.clone(),
        PendingEntry {
            sender: tx,
            created_at: std::time::Instant::now(),
        },
    );

    let json = serde_json::json!({
        "action": action,
        "params": params,
        "echo": echo,
    });

    {
        let mut sink = write.lock().await;
        sink.send(WsMsg::Text(json.to_string().into()))
            .await
            .map_err(|e| e.to_string())?;
    }

    let timeout_dur = Duration::from_secs(api.timeout.load(Ordering::Relaxed));
    let result = tokio::time::timeout(timeout_dur, rx).await;
    api.pending.lock().await.remove(&echo);

    match result {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(_)) => Err("请求已取消".to_string()),
        Err(_) => Err("请求超时".to_string()),
    }
}

async fn get_group_member_list(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    group_id: i64,
) -> Result<HashSet<i64>, String> {
    let value = call_onebot_api(
        write,
        api,
        "get_group_member_list",
        serde_json::json!({"group_id": group_id}),
    )
    .await?;

    let data = value.as_array().ok_or("无效的响应数据")?;

    let mut members = HashSet::new();
    for member in data {
        if let Some(user_id) = member.get("user_id").and_then(|v| v.as_i64()) {
            members.insert(user_id);
        }
    }
    Ok(members)
}

async fn handle_irc_message(
    storage: Arc<Storage>,
    irc_msg: osubot_core::irc::IrcPrivateMessage,
    write: Arc<Mutex<WriteSink>>,
    rate_limiter: Arc<RateLimiter>,
    oauth: Arc<OauthTokenCache>,
) {
    let code = irc_msg.message.trim();

    let pending = match storage.get_pending_bind(code) {
        Ok(Some(p)) => p,
        Ok(None) => {
            warn!(code = code, sender = %irc_msg.sender, "No matching pending bind for IRC code");
            return;
        }
        Err(_) => {
            error!("Database error looking up pending bind");
            return;
        }
    };

    if irc_msg.sender.to_lowercase() != pending.target_username.replace(' ', "_").to_lowercase() {
        if let Err(e) = storage.remove_pending_bind(code) {
            tracing::warn!(code = %code, error = %e, "Failed to remove pending bind (username mismatch)");
        }
        let msg = format!(
            "[CQ:at,qq={}] 绑定失败（绑定的不是本人）",
            pending.qq_user_id
        );
        send_group_msg(&write, pending.group_id, &msg).await;
        return;
    }

    match api::get_user_info(&rate_limiter, &oauth, &pending.target_username).await {
        Ok(Some(info)) => {
            if let Err(e) = storage.set_user_id(&pending.target_username, info.id) {
                warn!(
                    "Failed to cache user_id for {}: {e}",
                    pending.target_username
                );
            }
            match storage.bind(pending.qq_user_id, info.id, &info.username) {
                Ok(Ok(())) => {
                    if let Err(e) = storage.remove_pending_bind(code) {
                        tracing::warn!(code = %code, error = %e, "Failed to remove pending bind (bind success)");
                    }
                    info!(qq = pending.qq_user_id, username = %info.username, "Bind verified and completed");
                    let msg = format!(
                        "[CQ:at,qq={}] 成功绑定为{}",
                        pending.qq_user_id, info.username
                    );
                    send_group_msg(&write, pending.group_id, &msg).await;
                }
                Ok(Err(_)) => {
                    if let Err(e) = storage.remove_pending_bind(code) {
                        tracing::warn!(code = %code, error = %e, "Failed to remove pending bind (already bound)");
                    }
                    let msg = format!(
                        "[CQ:at,qq={}] 绑定失败（该 osu! 用户已绑定其他 QQ）",
                        pending.qq_user_id
                    );
                    send_group_msg(&write, pending.group_id, &msg).await;
                }
                Err(_) => {
                    if let Err(e) = storage.remove_pending_bind(code) {
                        tracing::warn!(code = %code, error = %e, "Failed to remove pending bind (db error)");
                    }
                    let msg = format!("[CQ:at,qq={}] 绑定失败，请稍后重试", pending.qq_user_id);
                    send_group_msg(&write, pending.group_id, &msg).await;
                }
            }
        }
        Ok(None) => {
            if let Err(e) = storage.remove_pending_bind(code) {
                tracing::warn!(code = %code, error = %e, "Failed to remove pending bind (user not found)");
            }
            warn!("User {} not found during IRC bind", pending.target_username);
            let msg = format!("[CQ:at,qq={}] 绑定失败（用户不存在）", pending.qq_user_id);
            send_group_msg(&write, pending.group_id, &msg).await;
        }
        Err(e) => {
            if let Err(e2) = storage.remove_pending_bind(code) {
                tracing::warn!(code = %code, error = %e2, "Failed to remove pending bind (api error)");
            }
            warn!(
                "Failed to fetch user info for {} during IRC bind: {e}",
                pending.target_username
            );
            let msg = format!("[CQ:at,qq={}] 绑定失败，请稍后重试", pending.qq_user_id);
            send_group_msg(&write, pending.group_id, &msg).await;
        }
    }
}

#[tokio::main]
async fn main() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("osubot=info,osubot_core=info,info"));

    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_timer(LocalTime::rfc_3339())
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("Starting osubot...");

    osubot_render::ensure_cache_dir().await;

    let config = Config::from_path("osubot.toml").expect("Failed to load config");
    let config = Arc::new(tokio::sync::RwLock::new(config));

    info!("osubot starting...");
    info!("OneBot URL: {}", config.read().await.bot.onebot_url);

    let db_path = config.read().await.database.path.clone();
    let storage = Arc::new(Storage::new(&db_path).expect("Failed to open database"));

    let (client_id, client_secret) = {
        let cfg = config.read().await;
        (cfg.osu.client_id.clone(), cfg.osu.client_secret.clone())
    };
    let oauth = Arc::new(OauthTokenCache::new(client_id, client_secret));

    let rate_limiter = Arc::new(RateLimiter::new());

    // Trigger lazy initialization of the shared reqwest HTTP client early, so any
    // build failure (e.g. missing TLS backend) is surfaced at startup rather than
    // crashing the process mid-flight on the first API call.
    let _ = osubot_core::api::http_client();

    let onebot_api_timeout = {
        let cfg = config.read().await;
        Arc::new(AtomicU64::new(cfg.bot.onebot_api_timeout_secs))
    };

    let upstream_chain = {
        let cfg = config.read().await;
        Arc::new(tokio::sync::RwLock::new(reload::build_upstream_chain(
            &cfg.upstream,
            &oauth,
            &rate_limiter,
        )))
    };

    match storage.get_users_without_ids() {
        Ok(users) if !users.is_empty() => {
            info!("Backfilling user IDs for {} users...", users.len());
            for username in &users {
                match api::get_user_info(&rate_limiter, &oauth, username).await {
                    Ok(Some(info)) => {
                        if let Err(e) = storage.set_user_id(username, info.id) {
                            warn!("Failed to cache user_id for {username}: {e}");
                        } else {
                            info!("Cached user_id for {username}: {}", info.id);
                        }
                    }
                    Ok(None) => {
                        warn!("User {username} not found during backfill");
                    }
                    Err(e) => {
                        warn!("Failed to fetch user info for {username} during backfill: {e}");
                    }
                }
            }
        }
        Ok(_) => info!("All bound users already have cached user IDs"),
        Err(e) => error!("Failed to query users without IDs: {e}"),
    }

    let scheduler = Scheduler::new(
        storage.clone(),
        oauth.clone(),
        rate_limiter.clone(),
        config.clone(),
    );

    let scheduler_clone = scheduler.clone();
    let _scheduler_handle = tokio::spawn(async move {
        scheduler_clone.run().await;
    });

    let (irc_tx, mut irc_rx) = mpsc::channel::<osubot_core::irc::IrcPrivateMessage>(100);

    let irc_handle: Arc<std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(None));

    let irc_tx_for_reload = irc_tx.clone();
    let irc_handle_for_reload = irc_handle.clone();

    {
        let cfg = config.read().await;
        let irc_enabled = cfg.irc.enabled;

        if irc_enabled && (cfg.irc.nickname.is_empty() || cfg.irc.password.is_empty()) {
            panic!("IRC is enabled but nickname or password is not set in osubot.toml");
        }

        if irc_enabled {
            let irc_config = osubot_core::IrcConfig::new(
                cfg.irc.enabled,
                &cfg.irc.server,
                cfg.irc.port,
                &cfg.irc.nickname,
                &cfg.irc.password,
            );
            let irc_client = osubot_core::irc::IrcClient::new(irc_config, irc_tx);
            *irc_handle.lock().unwrap() = Some(tokio::spawn(async move {
                if let Err(e) = irc_client.run().await {
                    error!(error = %e, "IRC client error");
                }
            }));
        }
    };

    {
        let cfg = config.read().await;
        if cfg.osu.client_secret.is_empty() || cfg.osu.client_secret == "your-client-secret-here" {
            warn!("osu! API v2 client_secret not configured. Please set osu.client_secret in osubot.toml");
        }
    }

    let onebot_api = Arc::new(OneBotApi::new(onebot_api_timeout.clone()));

    let onebot_cleanup = onebot_api.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut pending = onebot_cleanup.pending.lock().await;
            let before = pending.len();
            pending.retain(|_, entry| entry.created_at.elapsed() < Duration::from_secs(30));
            let removed = before.saturating_sub(pending.len());
            if removed > 0 {
                tracing::warn!(removed, "cleaned up stale pending OneBot API entries");
            }
        }
    });

    // Ensure plugin directory exists
    let plugin_dir = {
        let cfg = config.read().await;
        let dir = cfg.plugin.dir.clone();
        std::fs::create_dir_all(&dir).ok();
        dir
    };

    // Create ReloadHandle (pm starts as None, updated in reconnect loop)
    let pm: Arc<tokio::sync::Mutex<Option<PluginManager>>> =
        Arc::new(tokio::sync::Mutex::new(None));
    let reload_handle = ReloadHandle::new(
        config.clone(),
        pm.clone(),
        onebot_api_timeout,
        upstream_chain.clone(),
        oauth.clone(),
        rate_limiter.clone(),
        scheduler.clone(),
        irc_handle_for_reload,
        Some(irc_tx_for_reload),
    );

    // Extract drain/in_flight/force_reconnect refs for message loop
    let drain = reload_handle.drain.clone();
    let in_flight = reload_handle.in_flight.clone();
    let force_reconnect = reload_handle.force_reconnect.clone();

    // Start file watcher coordinator
    let coordinator = reload::ReloadCoordinator::new(
        reload_handle,
        std::path::PathBuf::from("osubot.toml"),
        std::path::PathBuf::from(plugin_dir),
    );
    // Coordinator::start() 内部已通过 error! 日志处理异常退出
    let watcher_monitor_handle = coordinator.start();
    tokio::spawn(async move {
        let _ = watcher_monitor_handle.await;
        warn!("文件监控任务已退出，热重载功能不可用");
    });

    let user_rate_limits: Arc<dashmap::DashMap<i64, UserRateLimit>> =
        Arc::new(dashmap::DashMap::new());

    // Shared write handle: the IRC bridge reads from this on each message,
    // and the reconnection loop updates it when a new connection is established.
    let current_write: Arc<Mutex<Option<Arc<Mutex<WriteSink>>>>> = Arc::new(Mutex::new(None));

    let cw_for_irc = current_write.clone();
    let storage_for_irc = storage.clone();
    let rate_limiter_for_irc = rate_limiter.clone();
    let oauth_for_irc = oauth.clone();
    tokio::spawn(async move {
        while let Some(irc_msg) = irc_rx.recv().await {
            let write_opt = { cw_for_irc.lock().await.clone() };
            if let Some(write) = write_opt {
                let storage = storage_for_irc.clone();
                let rate_limiter = rate_limiter_for_irc.clone();
                let oauth = oauth_for_irc.clone();
                tokio::spawn(async move {
                    handle_irc_message(storage, irc_msg, write, rate_limiter, oauth).await;
                });
            } else {
                warn!("No active WebSocket connection, dropping IRC message");
            }
        }
    });

    let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("收到关闭信号，正在优雅关闭...");
        shutdown_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let mut reconnect_delay = 1u64;
    loop {
        if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            tracing::info!("正在关闭，不再重连");
            break;
        }
        let onebot_url = config.read().await.bot.onebot_url.clone();
        info!(url = %onebot_url, "正在连接 OneBot WebSocket");
        let ws_stream = match connect_async(&onebot_url).await {
            Ok((stream, _)) => {
                reconnect_delay = 1;
                stream
            }
            Err(e) => {
                error!(error = %e, delay = reconnect_delay, "WebSocket 连接失败，{}秒后重试", reconnect_delay);
                tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
                reconnect_delay = (reconnect_delay * 2).min(60);
                continue;
            }
        };

        info!("WebSocket 连接已建立");

        let (write, mut read) = ws_stream.split();
        let write = Arc::new(Mutex::new(write));

        // Update the shared write handle for the IRC bridge
        {
            let mut cw = current_write.lock().await;
            let old = cw.replace(write.clone());
            if let Some(old) = old {
                let mut sink = old.lock().await;
                if let Err(e) = sink.close().await {
                    tracing::debug!(error = %e, "failed to close old WebSocket sink");
                }
            }
        }

        let connection_alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // Periodic ping to keep connection alive
        let ping_write = write.clone();
        let ping_shutdown = shutdown.clone();
        let ping_connection_alive = connection_alive.clone();
        let ping_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            loop {
                interval.tick().await;
                if ping_shutdown.load(std::sync::atomic::Ordering::Relaxed)
                    || !ping_connection_alive.load(std::sync::atomic::Ordering::Relaxed)
                {
                    break;
                }
                let mut sink = ping_write.lock().await;
                if let Err(e) = sink.send(Message::Ping(vec![].into())).await {
                    tracing::debug!(error = %e, "WebSocket ping failed");
                    break;
                }
            }
        });

        let last_beatmap = last_beatmap_cache::LastBeatmapCache::new();

        let plugin_cfg = {
            let cfg = config.read().await;
            cfg.plugin.clone()
        };

        let new_pm = if plugin_cfg.instances.iter().any(|p| p.enabled) {
            let (plugin_tx, mut plugin_rx) = mpsc::channel::<(i64, serde_json::Value)>(256);

            let write_consumer = write.clone();
            tokio::spawn(async move {
                while let Some((group_id, message)) = plugin_rx.recv().await {
                    let json = serde_json::json!({
                        "action": "send_group_msg",
                        "params": {
                            "group_id": group_id,
                            "message": message
                        }
                    });
                    let mut sink = write_consumer.lock().await;
                    if let Err(e) = sink.send(Message::Text(json.to_string().into())).await {
                        tracing::debug!(error = %e, "plugin send channel closed");
                    }
                }
            });

            let msg_fn: Arc<dyn Fn(i64, serde_json::Value) -> Result<(), String> + Send + Sync> =
                Arc::new(move |group_id, message| {
                    plugin_tx
                        .try_send((group_id, message))
                        .map_err(|e| format!("plugin message channel busy: {e}"))
                });

            let services = HostServices {
                http_client: reqwest::Client::new(),
                blocking_http_client: reqwest::blocking::Client::new(),
                rate_limiter: rate_limiter.clone(),
                oauth: oauth.clone(),
                storage: storage.clone(),
                send_msg_fn: msg_fn,
                runtime_handle: tokio::runtime::Handle::current(),
                instance_idx: 0,
                tick_registry: Arc::new(std::sync::Mutex::new(Vec::new())),
                tick_id_counter: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                instance_config: None,
                limiter: osubot_plugin::StoreLimitsBuilder::new()
                    .memory_size(100 * 1024 * 1024)
                    .build(),
            };

            match PluginManager::new(&plugin_cfg, services).await {
                Ok(mgr) => {
                    info!("Plugin manager initialized with {} plugins", mgr.len());
                    Some(mgr)
                }
                Err(e) => {
                    warn!("Plugin initialization failed: {e}");
                    None
                }
            }
        } else {
            None
        };

        // Update shared pm (same Arc as coordinator)
        {
            let mut guard = pm.lock().await;
            *guard = new_pm;
        }

        // Spawn plugin tick loop
        let pm_for_tick = pm.clone();
        let tick_drain = drain.clone();
        let tick_handle = tokio::spawn(async move {
            let mut last_fired: HashMap<(usize, u32), std::time::Instant> = HashMap::new();
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                // 热重载 drain 期间暂停 tick 分派，避免 phase 1 收集的索引
                // 在 phase 2 使用前被 reload_all()→compact() 重映射
                if tick_drain.load(Ordering::SeqCst) {
                    continue;
                }
                let now = std::time::Instant::now();
                // 第一阶段：收集到期 tick（短暂持锁，读取后立即释放）
                let due_ticks: Vec<(usize, u32)> = {
                    let mut guard = pm_for_tick.lock().await;
                    guard
                        .as_mut()
                        .map(|pm| {
                            let all_ticks = pm.get_ticks();
                            let valid_keys: std::collections::HashSet<_> =
                                all_ticks.iter().map(|(idx, _, tid)| (*idx, *tid)).collect();
                            last_fired.retain(|k, _| valid_keys.contains(k));
                            all_ticks
                                .into_iter()
                                .filter(|(idx, interval_secs, tid)| {
                                    let key = (*idx, *tid);
                                    last_fired.get(&key).is_none_or(|last| {
                                        now.duration_since(*last)
                                            >= Duration::from_secs(*interval_secs)
                                    })
                                })
                                .map(|(idx, _, tid)| (idx, tid))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                // 第二阶段：逐个触发 tick
                // 采用 take → execute（无锁）→ put 模式：
                // dispatch 内部有 spawn_blocking + timeout 的 .await 点，
                // 若在此期间持有 pm 锁，会阻塞消息分发和热重载。
                for (plugin_idx, tick_id) in due_ticks {
                    if tick_drain.load(Ordering::SeqCst) {
                        break;
                    }

                    // 取出实例（短暂持锁）
                    // 取出后立即再次检查 drain：若已设置，说明 reload_all()→compact()
                    // 正在重排索引，此时放回实例会因旧 plugin_idx 错位而污染其他槽位。
                    // 直接丢弃实例——热重载会重建所有实例，不需要我们放回。
                    let instance = {
                        let mut guard = pm_for_tick.lock().await;
                        let inst = guard.as_mut().and_then(|pm| pm.take_instance(plugin_idx));
                        if tick_drain.load(Ordering::SeqCst) {
                            drop(inst); // 丢弃实例，热重载会重建
                            break; // 跳出 for 循环，不再处理后续到期 tick
                        }
                        inst
                    };

                    let Some(mut inst) = instance else {
                        continue;
                    };

                    // 检查导出（不持锁）
                    if !inst.has_export("on_tick") {
                        let mut guard = pm_for_tick.lock().await;
                        if let Some(ref mut pm) = *guard {
                            pm.put_instance(plugin_idx, inst);
                        }
                        last_fired.insert((plugin_idx, tick_id), now);
                        continue;
                    }

                    // 执行 tick（不持锁，允许消息分发/热重载在此期间获取 pm）
                    let timeout_dur = inst.timeout;
                    let result = tokio::time::timeout(
                        timeout_dur,
                        tokio::task::spawn_blocking(move || {
                            let res = inst.on_tick(tick_id);
                            (res, inst)
                        }),
                    )
                    .await;

                    // 处理结果并放回实例
                    {
                        let mut guard = pm_for_tick.lock().await;
                        if let Some(ref mut pm) = *guard {
                            match result {
                                Ok(Ok((Ok(()), inst))) | Ok(Ok((Err(_), inst))) => {
                                    // 成功或插件错误：放回实例
                                    // （handle_tick 原本也忽略 PluginError，
                                    //   lost_instances 跟踪由其他 dispatch 路径处理）
                                    pm.put_instance(plugin_idx, inst);
                                }
                                Ok(Err(_)) | Err(_) => {
                                    // spawn_blocking join error 或 timeout：实例已丢失，重载
                                    let _ = pm.reload_instance(plugin_idx);
                                }
                            }
                        }
                    }

                    last_fired.insert((plugin_idx, tick_id), now);
                }
            }
        });

        // Message loop
        loop {
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            // onebot_url hot-reload triggers a forced reconnect
            if force_reconnect.load(Ordering::SeqCst) {
                info!("强制重连：onebot_url 已热重载变更");
                break;
            }
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(resp) = serde_json::from_str::<OneBotResponse>(&text) {
                        if resp.status.is_some() {
                            if let Some(echo) = resp.echo {
                                let mut pending = onebot_api.pending.lock().await;
                                if let Some(entry) = pending.remove(&echo) {
                                    let _ = entry
                                        .sender
                                        .send(resp.data.unwrap_or(serde_json::Value::Null));
                                }
                                continue;
                            }
                        }
                    }

                    if let Some(qq_msg) = parse_onebot_message(&text) {
                        // 群黑白名单检查
                        {
                            let cfg = config.read().await;
                            if !cfg.group_filter.is_group_allowed(qq_msg.group_id) {
                                debug!(group_id = qq_msg.group_id, mode = ?cfg.group_filter.mode, "群被过滤，跳过");
                                continue;
                            }
                        }

                        let (resp_tx, mut resp_rx) = mpsc::channel::<String>(1);

                        // Drain check: 热重载期间拒绝新任务
                        if drain.load(Ordering::SeqCst) {
                            info!(group_id = qq_msg.group_id, "热重载中，跳过消息");
                            continue;
                        }

                        let write_clone = write.clone();
                        let group_id = qq_msg.group_id;
                        let in_flight1 = in_flight.clone();
                        in_flight.fetch_add(1, Ordering::SeqCst);
                        tokio::spawn(async move {
                            let _guard = InFlightGuard(in_flight1);
                            if let Some(response) = resp_rx.recv().await {
                                send_group_msg(&write_clone, group_id, &response).await;
                            }
                        });

                        let ctx = BotContext {
                            storage: storage.clone(),
                            scheduler: scheduler.clone(),
                            oauth: oauth.clone(),
                            rate_limiter: rate_limiter.clone(),
                            command_rate_limits: user_rate_limits.clone(),
                            config: config.clone(),
                            write: write.clone(),
                            onebot_api: onebot_api.clone(),
                            last_beatmap: last_beatmap.clone(),
                            upstream_chain: upstream_chain.clone(),
                            plugin_manager: pm.clone(),
                        };
                        let in_flight2 = in_flight.clone();
                        in_flight.fetch_add(1, Ordering::SeqCst);
                        tokio::spawn(async move {
                            let _guard = InFlightGuard(in_flight2);
                            let command_timeout = Duration::from_secs(
                                ctx.config.read().await.bot.command_timeout_secs,
                            );
                            if tokio::time::timeout(
                                command_timeout,
                                handle_command(ctx, qq_msg, resp_tx.clone()),
                            )
                            .await
                            .is_err()
                            {
                                tracing::warn!("命令处理超时（{}秒）", command_timeout.as_secs());
                                let _ = resp_tx.send("命令处理超时，请稍后重试".to_string()).await;
                            }
                        });
                    }
                }
                Some(Ok(Message::Close(_))) => {
                    warn!("WebSocket 连接关闭，{}秒后重连", reconnect_delay);
                    break;
                }
                Some(Err(e)) => {
                    error!(error = %e, "WebSocket 错误，{}秒后重连", reconnect_delay);
                    break;
                }
                None => {
                    warn!("WebSocket 流结束，{}秒后重连", reconnect_delay);
                    break;
                }
                _ => {}
            }
        }

        connection_alive.store(false, std::sync::atomic::Ordering::Relaxed);
        force_reconnect.store(false, Ordering::SeqCst);
        ping_handle.abort();
        // 等待 tick 完成（插件 dispatch 超时 10s，留足余量），超时后强制 abort
        let _ = tokio::time::timeout(Duration::from_secs(15), tick_handle).await;
        // timeout 返回 Err 表示超时，此时 JoinHandle 已被 drop（等同于 abort）

        // Clear the write handle so the IRC bridge doesn't use a stale connection
        {
            let mut cw = current_write.lock().await;
            *cw = None;
        }

        // Shutdown plugin instances for this connection
        {
            let mut guard = pm.lock().await;
            if let Some(ref mut mgr) = *guard {
                mgr.shutdown().await;
            }
        }

        tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
        reconnect_delay = (reconnect_delay * 2).min(60);
    }

    // Shutdown plugin instances on SIGINT (calls on_unload hooks)
    {
        let mut guard = pm.lock().await;
        if let Some(ref mut mgr) = *guard {
            mgr.shutdown().await;
        }
    }

    scheduler.shutdown();
}
