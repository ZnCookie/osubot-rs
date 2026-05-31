mod config;
mod scheduler;

use config::Config;
use futures_util::{SinkExt, StreamExt};
use osubot_core::{
    api::{self, ApiError},
    dedup::RequestDedup,
    highlight::{format_highlight, get_highlight, HighlightError},
    parse_command,
    response::{format_score, format_scores, format_stats_with_change},
    storage::Storage,
    types::{format_play_datetime, Command, GameMode, Score, UserStats},
    OauthTokenCache, RateLimiter,
};
use osubot_render::PROFILE_VIEWPORT_WIDTH;
use osubot_render::{render_profile_card, render_score_card};
use scheduler::Scheduler;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, OnceLock,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, EnvFilter};

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
    command_count: u32,
}

struct OneBotApi {
    pending: Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>,
}

impl OneBotApi {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
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
    write: Arc<Mutex<WriteSink>>,
    onebot_api: Arc<OneBotApi>,
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

async fn resolve_score_user(
    ctx: &BotContext,
    msg: &QQMessage,
    username: &Option<String>,
    qq: &Option<i64>,
    mode: GameMode,
    resp_tx: &mpsc::Sender<String>,
) -> Option<(i64, String, UserStats)> {
    tracing::info!("resolve_score_user: starting");
    if let Some(ref name) = username {
        // Look up by username
        tracing::info!("resolve_score_user: looking up by username '{}'", name);
        match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, name, mode).await {
            Ok(stats) => {
                ctx.storage.set_user_id(&stats.username, stats.user_id).ok();
                Some((stats.user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, username = %name, "resolve_score_user: API lookup failed");
                let err_msg = match e {
                    ApiError::NotFound => format!("找不到用户 \"{}\"", name),
                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                    ApiError::RateLimited => "请求过于频繁，请稍后再试".to_string(),
                    _ => "获取数据失败，请稍后再试".to_string(),
                };
                let _ = resp_tx.send(err_msg).await;
                None
            }
        }
    } else {
        let (user_id, _stored_name, error_msg) = if let Some(mentioned_qq) = qq {
            match ctx.storage.get_binding(*mentioned_qq) {
                Ok(Some((user_id, name))) => (user_id, name, None),
                Ok(None) => (
                    0,
                    String::new(),
                    Some("该用户还没有绑定 osu! 账号".to_string()),
                ),
                Err(_) => (0, String::new(), Some("数据库错误".to_string())),
            }
        } else {
            match ctx.storage.get_binding(msg.user_id) {
                Ok(Some((user_id, name))) => (user_id, name, None),
                Ok(None) => (
                    0,
                    String::new(),
                    Some("你还没有绑定 osu! 账号，请使用 绑定 <用户名> 绑定".to_string()),
                ),
                Err(_) => (0, String::new(), Some("数据库错误".to_string())),
            }
        };
        if let Some(err) = error_msg {
            let _ = resp_tx.send(err).await;
            return None;
        }
        tracing::info!("resolve_score_user: fetching stats for user_id={}", user_id);
        match api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, user_id, mode).await {
            Ok(stats) => {
                ctx.storage.set_user_id(&stats.username, stats.user_id).ok();
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
    tracing::debug!("handle_score_query: starting");
    let (user_id, resolved_username, user_stats) = match resolve_score_user(
        ctx,
        msg,
        params.username,
        params.qq,
        params.mode,
        resp_tx,
    )
    .await
    {
        Some(u) => {
            tracing::debug!(
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
    ctx.scheduler.trigger_update(user_id, params.mode);
    let include_fails = !params.is_pass;
    let dedup_key = (user_id, params.is_pass, params.limit, params.mode);
    let dedup_username = resolved_username.clone();
    let dedup_rate_limiter = ctx.rate_limiter.clone();
    let dedup_oauth = ctx.oauth.clone();
    let dedup_mode = params.mode;
    let dedup_user_id = user_id;

    tracing::debug!(
        "Fetching scores for user_id={}, mode={:?}, limit={}",
        user_id,
        params.mode,
        params.limit
    );
    let score_result: Result<Arc<Vec<Score>>, String> = score_dedup()
        .run_or_wait(dedup_key, move || {
            let dedup_rate_limiter = dedup_rate_limiter.clone();
            let dedup_oauth = dedup_oauth.clone();
            async move {
                api::get_user_recent(
                    &dedup_rate_limiter,
                    &dedup_oauth,
                    dedup_user_id,
                    dedup_mode,
                    include_fails,
                    params.limit,
                )
                .await
                .map(Arc::new)
                .map_err(|e| {
                    warn!(user_id = dedup_user_id, mode = ?dedup_mode, error = ?e, "Score query failed");
                    match e {
                        ApiError::NotFound => "未找到该用户".to_string(),
                        ApiError::RateLimited => "请求过于频繁，请稍后再试".to_string(),
                        e => {
                            tracing::error!(error = ?e, "Score query error details");
                            format!("获取数据失败: {}", e)
                        }
                    }
                })
            }
        })
        .await;

    match score_result {
        Ok(scores) => {
            if scores.is_empty() {
                let empty_msg = if include_fails {
                    "最近没有游玩记录（包括失败）"
                } else {
                    "最近没有游玩记录"
                };
                let _ = resp_tx.send(empty_msg.to_string()).await;
                return;
            }
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
                let position = Some(index);
                let score = &scores[index];

                // Try rendering score card image (with overall timeout)
                let play_time = format_play_datetime(&score.created_at);
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
                    .ok()
                    .flatten();

                let pp_change = change.as_ref().and_then(|c| c.pp_change.map(|v| v as i64));
                let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
                let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);
                tracing::debug!(
                    "Starting score card render for {} (pp={}, rank={:?}, country_rank={:?}, pp_change={:?})",
                    dedup_username,
                    user_stats.pp,
                    user_global_rank,
                    user_country_rank,
                    pp_change
                );
                // 计算 UR（异步，10秒超时，失败不影响渲染）
                // 只有 osu! 模式、非 lazer 分数、有效 score_id 才尝试
                tracing::debug!(score_id = score.score_id, mode = ?params.mode, is_lazer = score.is_lazer, length = score.length_seconds, "Starting UR calculation");
                let ur_result = if params.mode == GameMode::Osu
                    && score.score_id > 0
                    && score.has_replay
                {
                    let rl = ctx.rate_limiter.clone();
                    let oa = ctx.oauth.clone();
                    let mods = score.mods.clone();
                    let ur = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        osubot_core::ur::calculate_score_ur(
                            &rl,
                            &oa,
                            score.score_id,
                            score.legacy_score_id,
                            score.beatmap_id,
                            params.mode,
                            &mods,
                        ),
                    )
                    .await;
                    match ur {
                        Ok(Some(ur_val)) => {
                            tracing::debug!(
                                score_id = score.score_id,
                                total_ur = ur_val,
                                "UR calculation succeeded"
                            );
                            Some(ur_val)
                        }
                        Ok(None) => {
                            tracing::warn!(
                                score_id = score.score_id,
                                "UR calculation returned None"
                            );
                            None
                        }
                        Err(_) => {
                            tracing::warn!(score_id = score.score_id, "UR calculation timed out");
                            None
                        }
                    }
                } else {
                    if score.is_lazer {
                        tracing::debug!(
                            score_id = score.score_id,
                            "Skipping UR calculation: lazer score"
                        );
                    } else {
                        tracing::debug!(score_id = score.score_id, mode = ?params.mode, "Skipping UR calculation (not osu! mode or invalid score_id)");
                    }
                    None
                };

                // 计算 PP 分解和 if-acc（单张成绩卡片需要）
                let mut score = score.clone();
                osubot_core::enrich_score_with_pp(&mut score, params.mode).await;

                // 计算 mod 调整后的 AR/OD/CS/HP
                let (ar_eff, od_eff, cs_eff, hp_eff) = {
                    let (a, o, c, h) = osubot_core::apply_mod_adjustment_to_stats(
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
                    match osubot_render::cache::fetch_and_cache(
                        &score.cover_url,
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

                let ur_value = ur_result;

                // 30s outer timeout for score card rendering.
                // The cancel flag is shared with render_score_card; if this
                // timeout fires, it sets the flag so the blocking render
                // detects it at loop boundaries and aborts early.
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let cancel_clone = cancel.clone();
                let render_result = tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    render_score_card(osubot_render::ScoreCardParams {
                        score: &score,
                        username: &dedup_username,
                        mode: params.mode,
                        user_pp: user_stats.pp,
                        user_global_rank,
                        user_country_rank,
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
                        tracing::info!(
                            "Score card rendered successfully, {} bytes",
                            jpeg_bytes.len()
                        );
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
                        warn!(error = %e, "render_score_card failed, falling back to text");
                        let response =
                            format_score(&score, &dedup_username, position, params.is_pass);
                        let _ = resp_tx.send(response).await;
                    }
                    Err(_) => {
                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        warn!("render_score_card timed out after 30s, falling back to text");
                        let response =
                            format_score(&score, &dedup_username, position, params.is_pass);
                        let _ = resp_tx.send(response).await;
                    }
                }
            } else {
                let response = format_scores(&scores, &dedup_username, params.mode, params.is_pass);
                let _ = resp_tx.send(response).await;
            }
        }
        Err(err_msg) => {
            let _ = resp_tx.send(err_msg).await;
        }
    }
}

async fn handle_command(
    ctx: BotContext,
    msg: QQMessage,
    resp_tx: mpsc::Sender<String>,
    irc_nickname: Option<String>,
) {
    let cmd = match parse_command(&msg.message, msg.mentioned_user_id) {
        Some(cmd) => cmd,
        None => return,
    };

    match cmd {
        Command::QuerySelf { mode } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "QuerySelf command");
            match ctx.storage.get_binding(msg.user_id) {
                Ok(Some((user_id, current_username))) => {
                    // Trigger update for the queried mode (bypassing cooldown)
                    ctx.scheduler.trigger_update(user_id, mode);
                    match api::fetch_user_stats_by_user_id(
                        &ctx.rate_limiter,
                        &ctx.oauth,
                        user_id,
                        mode,
                    )
                    .await
                    {
                        Ok(stats) => {
                            if stats.username != current_username {
                                ctx.storage
                                    .update_binding_username(msg.user_id, &stats.username)
                                    .ok();
                            }
                            ctx.storage.set_user_id(&stats.username, user_id).ok();
                            let change = ctx
                                .storage
                                .calculate_change(user_id, mode, &stats)
                                .ok()
                                .flatten();
                            info!(user_id = user_id, username = %stats.username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QuerySelf success");
                            let response = format_stats_with_change(&stats, &change, mode);
                            let _ = resp_tx.send(response).await;
                        }
                        Err(e) => {
                            warn!(user_id = user_id, mode = ?mode, error = ?e, "QuerySelf failed");
                            let err_msg = match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            };
                            let _ = resp_tx.send(err_msg).await;
                        }
                    }
                }
                Ok(None) => {
                    info!(user_id = msg.user_id, "QuerySelf but no binding");
                    let _ = resp_tx
                        .send("请先绑定 osu! 用户名，使用 绑定 <用户名>".to_string())
                        .await;
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
                    ctx.storage.set_user_id(&stats.username, stats.user_id).ok();
                    if stats.username != username {
                        ctx.storage.set_user_id(&username, stats.user_id).ok();
                    }
                    ctx.scheduler.trigger_update(stats.user_id, mode);
                    let change = ctx
                        .storage
                        .calculate_change(stats.user_id, mode, &stats)
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
                    let err_msg = match e {
                        ApiError::NotFound => "未找到该用户".to_string(),
                        ApiError::MissingApiKey => "API Key 未配置".to_string(),
                        ApiError::OAuthError => "OAuth 认证失败".to_string(),
                        ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    };
                    let _ = resp_tx.send(err_msg).await;
                }
            }
        }
        Command::QueryMentionedUser { qq, mode } => {
            info!(qq = qq, group_id = msg.group_id, mode = ?mode, "QueryMentionedUser command");
            match ctx.storage.get_binding(qq) {
                Ok(Some((user_id, current_username))) => {
                    ctx.scheduler.trigger_update(user_id, mode);
                    match api::fetch_user_stats_by_user_id(
                        &ctx.rate_limiter,
                        &ctx.oauth,
                        user_id,
                        mode,
                    )
                    .await
                    {
                        Ok(stats) => {
                            if stats.username != current_username {
                                ctx.storage
                                    .update_binding_username(qq, &stats.username)
                                    .ok();
                            }
                            ctx.storage.set_user_id(&stats.username, user_id).ok();
                            let change = ctx
                                .storage
                                .calculate_change(user_id, mode, &stats)
                                .ok()
                                .flatten();
                            info!(user_id = user_id, username = %stats.username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QueryMentionedUser success");
                            let response = format_stats_with_change(&stats, &change, mode);
                            let _ = resp_tx.send(response).await;
                        }
                        Err(e) => {
                            warn!(user_id = user_id, mode = ?mode, error = ?e, "QueryMentionedUser failed");
                            let err_msg = match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后重试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            };
                            let _ = resp_tx.send(err_msg).await;
                        }
                    }
                }
                Ok(None) => {
                    info!(qq = qq, "QueryMentionedUser but no binding");
                    let _ = resp_tx
                        .send(
                            "该用户未绑定 osu! 账号，请使用 绑定 <osu用户名> 命令绑定".to_string(),
                        )
                        .await;
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
                            "你已经绑定为{},如需修改请先解绑",
                            existing_username
                        ))
                        .await;
                }
                Ok(None) => {
                    if let Some(nickname) = irc_nickname {
                        match ctx.storage.has_pending_bind(msg.user_id) {
                            Ok(true) => {
                                let _ = resp_tx
                                    .send(
                                        "你已有进行中的绑定请求，请等待当前验证码过期后再试"
                                            .to_string(),
                                    )
                                    .await;
                                return;
                            }
                            Err(_) => {
                                error!(user_id = msg.user_id, "Failed to check pending bind");
                                let _ = resp_tx.send("绑定失败，请稍后重试".to_string()).await;
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
                                        "您的验证码是 {}，请在两分钟内通过osu!发送私信给 {} 来完成验证",
                                        code, nickname
                                    ))
                                    .await;
                            }
                            Err(_) => {
                                error!(user_id = msg.user_id, "Failed to create pending bind");
                                let _ = resp_tx.send("绑定失败，请稍后重试".to_string()).await;
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
                                            .send(format!("成功绑定为{}", user_info.username))
                                            .await;
                                    }
                                    Ok(Err(bound_qq)) => {
                                        info!(user_id = msg.user_id, username = %username, bound_qq = bound_qq, "Bind failed - username already bound");
                                        let _ = resp_tx
                                            .send("该 osu! 用户已绑定其他QQ".to_string())
                                            .await;
                                    }
                                    Err(_) => {
                                        error!(user_id = msg.user_id, username = %username, "Bind failed");
                                        let _ =
                                            resp_tx.send("绑定失败，请稍后重试".to_string()).await;
                                    }
                                }
                            }
                            Ok(None) => {
                                info!(username = %username, "Bind but user not found");
                                let _ = resp_tx.send("未找到该 osu! 用户".to_string()).await;
                            }
                            Err(e) => {
                                warn!(username = %username, error = ?e, "Bind - user info check failed");
                                let err_msg = match e {
                                    ApiError::NotFound => "未找到该用户".to_string(),
                                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                    ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                    _ => "查询失败，请稍后重试".to_string(),
                                };
                                let _ = resp_tx.send(err_msg).await;
                            }
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "Bind database error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::Unbind => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "Unbind command"
            );
            match ctx.storage.get_pending_unbind(msg.user_id) {
                Ok(Some(_)) => match ctx.storage.unbind(msg.user_id) {
                    Ok(_) => {
                        ctx.storage.remove_pending_unbind(msg.user_id).ok();
                        info!(user_id = msg.user_id, "Unbind success");
                        let _ = resp_tx.send("解绑成功".to_string()).await;
                    }
                    Err(_) => {
                        error!(user_id = msg.user_id, "Unbind failed");
                        let _ = resp_tx.send("解绑失败，请稍后重试".to_string()).await;
                    }
                },
                Ok(None) => match ctx.storage.get_binding(msg.user_id) {
                    Ok(Some((_, current_username))) => {
                        ctx.storage.set_pending_unbind(msg.user_id).ok();
                        info!(user_id = msg.user_id, username = %current_username, "Unbind confirmation requested");
                        let _ = resp_tx
                            .send(format!(
                                "确定要解除绑定 {} 吗？回复\"解绑\"确认",
                                current_username
                            ))
                            .await;
                    }
                    Ok(None) => {
                        info!(user_id = msg.user_id, "Unbind but no binding");
                        let _ = resp_tx.send("你还没有绑定任何 osu! 用户".to_string()).await;
                    }
                    Err(_) => {
                        error!(user_id = msg.user_id, "Unbind database error");
                        let _ = resp_tx.send("数据库错误".to_string()).await;
                    }
                },
                Err(_) => {
                    error!(user_id = msg.user_id, "Unbind pending check error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
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
                                ctx.storage.set_user_id(&stats.username, stats.user_id).ok();
                                stats.user_id
                            }
                            Err(e) => {
                                warn!(username = %name, error = ?e, "ProfileCard username resolution failed");
                                let err_msg = match e {
                                    ApiError::NotFound => "未找到该用户".to_string(),
                                    ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                    ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                    ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
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
                        Some(mentioned_qq) => {
                            match ctx.storage.get_binding(mentioned_qq) {
                                Ok(Some((user_id, current_username))) => {
                                    info!(qq = mentioned_qq, osu_id = user_id, username = %current_username, "ProfileCard mention");
                                    user_id
                                }
                                Ok(None) => {
                                    info!(qq = mentioned_qq, "ProfileCard mention but no binding");
                                    let _ = resp_tx
                                    .send("该用户未绑定 osu! 账号，请使用 绑定 <osu用户名> 命令绑定".to_string())
                                    .await;
                                    return;
                                }
                                Err(_) => {
                                    error!(qq = mentioned_qq, "ProfileCard mention database error");
                                    let _ = resp_tx.send("数据库错误".to_string()).await;
                                    return;
                                }
                            }
                        }
                        None => {
                            match ctx.storage.get_binding(msg.user_id) {
                                Ok(Some((user_id, current_username))) => {
                                    info!(user_id = msg.user_id, osu_id = user_id, username = %current_username, "ProfileCard self");
                                    user_id
                                }
                                Ok(None) => {
                                    let _ = resp_tx
                                    .send("请先绑定 osu! 用户名，或使用 !profile <用户名> 查询他人".to_string())
                                    .await;
                                    return;
                                }
                                Err(_) => {
                                    error!(user_id = msg.user_id, "ProfileCard database error");
                                    let _ = resp_tx.send("数据库错误".to_string()).await;
                                    return;
                                }
                            }
                        }
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
                        ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    })?;
                    info!(
                        user_id = dedup_target_id,
                        html_len = profile.html.len(),
                        hue = profile.profile_hue,
                        "ProfileCard HTML fetched"
                    );
                    render_profile_card(
                        &profile.html,
                        profile.profile_hue,
                        &profile.avatar_url,
                        &profile.username,
                        PROFILE_VIEWPORT_WIDTH,
                        1200,
                    )
                    .await
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

use tokio_tungstenite::tungstenite::Message as WsMsg;
use tracing_subscriber::fmt::time::LocalTime;
type WriteSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMsg,
>;

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

async fn send_group_msg_with_image(
    write: &Arc<Mutex<WriteSink>>,
    group_id: i64,
    image_data: &[u8],
) -> Result<(), ()> {
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
        .map_err(|e| {
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

    api.pending.lock().await.insert(echo.clone(), tx);

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

    let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
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
        storage.remove_pending_bind(code).ok();
        let msg = "绑定失败（绑定的不是本人）";
        send_group_msg(&write, pending.group_id, msg).await;
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
                    storage.remove_pending_bind(code).ok();
                    info!(qq = pending.qq_user_id, username = %info.username, "Bind verified and completed");
                    let msg = format!("成功绑定为{}", info.username);
                    send_group_msg(&write, pending.group_id, &msg).await;
                }
                Ok(Err(_)) => {
                    storage.remove_pending_bind(code).ok();
                    let msg = "绑定失败（该 osu! 用户已绑定其他 QQ）";
                    send_group_msg(&write, pending.group_id, msg).await;
                }
                Err(_) => {
                    storage.remove_pending_bind(code).ok();
                    let msg = "绑定失败，请稍后重试";
                    send_group_msg(&write, pending.group_id, msg).await;
                }
            }
        }
        Ok(None) => {
            storage.remove_pending_bind(code).ok();
            warn!("User {} not found during IRC bind", pending.target_username);
            let msg = "绑定失败（用户不存在）";
            send_group_msg(&write, pending.group_id, msg).await;
        }
        Err(e) => {
            storage.remove_pending_bind(code).ok();
            warn!(
                "Failed to fetch user info for {} during IRC bind: {e}",
                pending.target_username
            );
            let msg = "绑定失败，请稍后重试";
            send_group_msg(&write, pending.group_id, msg).await;
        }
    }
}

#[tokio::main]
async fn main() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("osubot=debug,osubot_core=debug,info"));

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

    info!("osubot starting...");
    info!("OneBot URL: {}", config.bot.onebot_url);

    let storage = Arc::new(Storage::new(&config.database.path).expect("Failed to open database"));

    let oauth = Arc::new(OauthTokenCache::new(
        config.osu.client_id.clone(),
        config.osu.api_key.clone(),
    ));

    let rate_limiter = Arc::new(RateLimiter::new());

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
        config.scheduler.clone(),
    );

    let scheduler_clone = scheduler.clone();
    let _scheduler_handle = tokio::spawn(async move {
        scheduler_clone.run().await;
    });

    let irc_enabled = config.irc.enabled;

    if irc_enabled && (config.irc.nickname.is_empty() || config.irc.password.is_empty()) {
        panic!("IRC is enabled but nickname or password is not set in osubot.toml");
    }

    let irc_nickname = if irc_enabled {
        Some(config.irc.nickname.clone())
    } else {
        None
    };

    let (irc_tx, mut irc_rx) = mpsc::channel::<osubot_core::irc::IrcPrivateMessage>(100);

    if irc_enabled {
        let irc_config = osubot_core::IrcConfig::new(
            config.irc.enabled,
            &config.irc.server,
            config.irc.port,
            &config.irc.nickname,
            &config.irc.password,
        );
        let irc_client = osubot_core::irc::IrcClient::new(irc_config, irc_tx);
        tokio::spawn(async move {
            if let Err(e) = irc_client.run().await {
                error!(error = %e, "IRC client error");
            }
        });
    }

    if config.osu.api_key.is_empty() || config.osu.api_key == "your-api-key-here" {
        warn!("API Key not configured. Please set osu.api_key in osubot.toml");
    }

    let onebot_api = Arc::new(OneBotApi::new());

    let user_rate_limits: Arc<std::sync::Mutex<HashMap<i64, UserRateLimit>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));
    let mut rate_limit_msg_counter: u64 = 0;

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

    let mut reconnect_delay = 1u64;
    loop {
        info!(url = %config.bot.onebot_url, "正在连接 OneBot WebSocket");
        let ws_stream = match connect_async(&config.bot.onebot_url).await {
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
            *cw = Some(write.clone());
        }

        // Message loop
        loop {
            match read.next().await {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(resp) = serde_json::from_str::<OneBotResponse>(&text) {
                        if resp.status.is_some() {
                            if let Some(echo) = resp.echo {
                                let mut pending = onebot_api.pending.lock().await;
                                if let Some(tx) = pending.remove(&echo) {
                                    let _ = tx.send(resp.data.unwrap_or(serde_json::Value::Null));
                                }
                                continue;
                            }
                        }
                    }

                    if let Some(qq_msg) = parse_onebot_message(&text) {
                        // Per-user rate limiting: max 5 commands per 3 seconds
                        {
                            let mut limits =
                                user_rate_limits.lock().unwrap_or_else(|e| e.into_inner());
                            let entry = limits.entry(qq_msg.user_id).or_insert(UserRateLimit {
                                last_command: std::time::Instant::now(),
                                command_count: 0,
                            });

                            if entry.last_command.elapsed() < std::time::Duration::from_secs(3) {
                                entry.command_count += 1;
                                if entry.command_count > 5 {
                                    continue; // skip this message
                                }
                            } else {
                                entry.last_command = std::time::Instant::now();
                                entry.command_count = 1;
                            }
                        }

                        // Periodic cleanup of stale rate limit entries (every 100 messages)
                        rate_limit_msg_counter += 1;
                        if rate_limit_msg_counter.is_multiple_of(100) {
                            if let Ok(mut limits) = user_rate_limits.lock() {
                                limits.retain(|_, v| {
                                    v.last_command.elapsed() < std::time::Duration::from_secs(60)
                                });
                            }
                        }

                        let (resp_tx, mut resp_rx) = mpsc::channel::<String>(1);

                        let write_clone = write.clone();
                        let group_id = qq_msg.group_id;
                        tokio::spawn(async move {
                            if let Some(response) = resp_rx.recv().await {
                                send_group_msg(&write_clone, group_id, &response).await;
                            }
                        });

                        let ctx = BotContext {
                            storage: storage.clone(),
                            scheduler: scheduler.clone(),
                            oauth: oauth.clone(),
                            rate_limiter: rate_limiter.clone(),
                            write: write.clone(),
                            onebot_api: onebot_api.clone(),
                        };
                        let irc_nickname = irc_nickname.clone();
                        tokio::spawn(async move {
                            if tokio::time::timeout(
                                Duration::from_secs(120),
                                handle_command(ctx, qq_msg, resp_tx, irc_nickname),
                            )
                            .await
                            .is_err()
                            {
                                tracing::warn!("命令处理超时（120秒）");
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

        // Clear the write handle so the IRC bridge doesn't use a stale connection
        {
            let mut cw = current_write.lock().await;
            *cw = None;
        }

        tokio::time::sleep(Duration::from_secs(reconnect_delay)).await;
        reconnect_delay = (reconnect_delay * 2).min(60);
    }
}
