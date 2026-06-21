mod app_state;
mod background;
mod config;
mod constants;
mod last_beatmap_cache;
mod plugin_runtime;
mod reload;
mod runtime;
mod scheduler;
mod ws_loop;
mod xfs_upstream;
mod yumu_upstream;

use app_state::AppState;
use config::Config;
use futures_util::{future::join_all, SinkExt};
use last_beatmap_cache::LastBeatmapCache;
use osubot_core::apply_mod_adjustment_to_stats;
use osubot_core::enrich_score_with_pp;
use osubot_core::{
    api::{self, ApiError},
    dedup::RequestDedup,
    highlight::{format_highlight, get_highlight, HighlightError},
    log_fmt, parse_command,
    response::{format_score, format_scores, format_stats_with_change},
    storage::Storage,
    strings::user_str,
    types::{format_play_datetime, Command, GameMode, Score, UserStats},
    upstream::UpstreamChain,
    OauthTokenCache, RateLimiter,
};
use osubot_plugin::{PluginActionResult, PluginManager};
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
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};

/// Type alias for the WebSocket write half used per-connection.
pub type WriteSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

/// Maximum number of scores to fetch when filters are active.
const SCORE_API_FETCH_LIMIT: u32 = 100;

pub(crate) struct InFlightGuard(Arc<AtomicUsize>);

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
pub(crate) struct OneBotResponse {
    status: Option<String>,
    data: Option<serde_json::Value>,
    echo: Option<String>,
}

pub struct UserRateLimit {
    last_command: std::time::Instant,
    command_timestamps: Vec<std::time::Instant>,
}

pub struct PendingEntry {
    sender: oneshot::Sender<serde_json::Value>,
    created_at: std::time::Instant,
}

pub struct OneBotApi {
    pending: Mutex<HashMap<String, PendingEntry>>,
    timeout: Arc<AtomicU64>,
}

impl OneBotApi {
    pub fn new(timeout_secs: Arc<AtomicU64>) -> Self {
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
pub(crate) fn parse_onebot_message(json: &str) -> Option<QQMessage> {
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

fn api_error_msg(qq: i64, e: &ApiError) -> String {
    match e {
        ApiError::NotFound => user_str("error.not_found").replace("{qq}", &qq.to_string()),
        ApiError::MissingApiKey => user_str("error.api_key").replace("{qq}", &qq.to_string()),
        ApiError::OAuthError => user_str("error.oauth").replace("{qq}", &qq.to_string()),
        ApiError::RateLimitedWithRetryAfter(Some(secs)) => user_str("error.rate_limit")
            .replace("{qq}", &qq.to_string())
            .replace("{secs}", &secs.to_string()),
        ApiError::RateLimitedWithRetryAfter(None) => {
            user_str("error.rate_limit_generic").replace("{qq}", &qq.to_string())
        }
        ApiError::ClientRateLimited => {
            user_str("error.client_rate_limit").replace("{qq}", &qq.to_string())
        }
        _ => user_str("error.query_failed").replace("{qq}", &qq.to_string()),
    }
}

/// Send an error message to the response channel.
async fn send_error(resp_tx: &mpsc::Sender<String>, qq: i64, key: &str) {
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

type BeatmapScoreDedup = RequestDedup<(i64, i64, GameMode), Score, String>;

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
    tracing::trace!("{}", log_fmt!("main.resolve_score_user_start"));

    if let Some(ref name) = username {
        // Look up by username
        tracing::trace!(
            "{}",
            log_fmt!("main.resolve_score_user_lookup", username = name)
        );
        match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, name, mode).await {
            Ok(stats) => {
                if let Err(e) = ctx
                    .storage
                    .set_user_id(&stats.username, stats.user_id)
                    .await
                {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "{}",
                        log_fmt!("main.cache_user_id_failed")
                    );
                }
                Some((stats.user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, username = %name, "{}", log_fmt!("main.resolve_score_user_api_failed"));
                let err_msg = match e {
                    ApiError::NotFound => user_str("error.not_found_named")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{name}", name),
                    ApiError::MissingApiKey => {
                        user_str("error.api_key").replace("{qq}", &msg.user_id.to_string())
                    }
                    ApiError::OAuthError => {
                        user_str("error.oauth").replace("{qq}", &msg.user_id.to_string())
                    }
                    ApiError::RateLimitedWithRetryAfter(Some(secs)) => user_str("error.rate_limit")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{secs}", &secs.to_string()),
                    ApiError::RateLimitedWithRetryAfter(None) => {
                        user_str("error.rate_limit_generic")
                            .replace("{qq}", &msg.user_id.to_string())
                    }
                    ApiError::ClientRateLimited => user_str("error.client_rate_limit")
                        .replace("{qq}", &msg.user_id.to_string()),
                    _ => user_str("error.data_fetch_failed")
                        .replace("{qq}", &msg.user_id.to_string()),
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
                    Some(user_str("bind.user_not_bound").replace("{qq}", &msg.user_id.to_string())),
                ),
            }
        } else {
            match ctx.resolve_binding(msg.user_id).await {
                Some((user_id, name)) => (user_id, name, None),
                None => (
                    0,
                    String::new(),
                    Some(user_str("bind.not_bound").replace("{qq}", &msg.user_id.to_string())),
                ),
            }
        };
        if let Some(err) = error_msg {
            let _ = resp_tx.send(err).await;
            return None;
        }
        tracing::info!(
            "{}",
            log_fmt!("main.resolve_score_user_fetch_stats", user_id = user_id)
        );
        match api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, user_id, mode).await {
            Ok(stats) => {
                if let Err(e) = ctx
                    .storage
                    .set_user_id(&stats.username, stats.user_id)
                    .await
                {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "{}",
                        log_fmt!("main.cache_user_id_failed")
                    );
                }
                Some((user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id, "{}", log_fmt!("main.resolve_score_user_lookup_bound_failed"));
                let _ = resp_tx
                    .send(
                        user_str("error.data_fetch_failed")
                            .replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                None
            }
        }
    }
}

struct ScoreQueryParams<'a> {
    username: &'a Option<String>,
    qq: &'a Option<i64>,
    is_pass: bool,
    beatmap_id: Option<u32>,
    score_id: Option<u64>,
    limit: u32,
    is_single: bool,
    limit_end: Option<u32>,
    filters: Option<&'a [String]>,
}

/// Comparison operator extracted from a `key<op>value` filter token.
/// `=` maps to `Eq` and `==` maps to `EqEq`. For numeric keys the two
/// have identical semantics (equality), but for the `mod` key `Eq` means
/// "subset" (score's mods must include all required mods) and `EqEq`
/// means "exact set" (score's mods must match the required set exactly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterOp {
    Eq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

/// Parse a single filter token of the form `key<op>value` where `<op>`
/// is one of `=`, `==`, `!=`, `<`, `<=`, `>`, `>=`. The operator must
/// be glued to both key and value (no surrounding spaces).
///
/// Returns `None` for malformed input (empty key, empty value, or no
/// recognized operator).
fn parse_filter_token(token: &str) -> Option<(String, FilterOp, String)> {
    // Two-character operators must be tried before single-character ones
    // to avoid `>=` being misread as `>` with value `=5`.
    const TWO_CHAR_OPS: &[(&str, FilterOp)] = &[
        ("==", FilterOp::EqEq),
        (">=", FilterOp::GtEq),
        ("<=", FilterOp::LtEq),
        ("!=", FilterOp::NotEq),
    ];
    for (op_str, op) in TWO_CHAR_OPS {
        if let Some(idx) = token.find(op_str) {
            let key = &token[..idx];
            let value = &token[idx + op_str.len()..];
            if key.is_empty() {
                continue;
            }
            if value.is_empty() {
                return None;
            }
            return Some((key.to_string(), *op, value.to_string()));
        }
    }

    const ONE_CHAR_OPS: &[(char, FilterOp)] = &[
        ('=', FilterOp::Eq),
        ('>', FilterOp::Gt),
        ('<', FilterOp::Lt),
    ];
    for (op_char, op) in ONE_CHAR_OPS {
        if let Some(idx) = token.find(*op_char) {
            let key = &token[..idx];
            let value = &token[idx + 1..];
            if key.is_empty() {
                continue;
            }
            if value.is_empty() {
                return None;
            }
            return Some((key.to_string(), *op, value.to_string()));
        }
    }

    None
}

/// Strict integer comparison.
fn cmp_i64(a: i64, b: i64, op: FilterOp) -> bool {
    match op {
        FilterOp::Eq | FilterOp::EqEq => a == b,
        FilterOp::NotEq => a != b,
        FilterOp::Lt => a < b,
        FilterOp::LtEq => a <= b,
        FilterOp::Gt => a > b,
        FilterOp::GtEq => a >= b,
    }
}

/// Float comparison. `Eq` / `NotEq` use the given `tol` tolerance
/// (to handle display precision and FP rounding). Ordering operators
/// (`<`, `<=`, `>`, `>=`) are strict — tolerance on inequality would
/// degrade `pp>500` into `pp>=499.5`, which is surprising.
fn cmp_f64(a: f64, b: f64, op: FilterOp, tol: f64) -> bool {
    match op {
        FilterOp::Eq | FilterOp::EqEq => (a - b).abs() < tol,
        FilterOp::NotEq => (a - b).abs() >= tol,
        FilterOp::Lt => a < b,
        FilterOp::LtEq => a <= b,
        FilterOp::Gt => a > b,
        FilterOp::GtEq => a >= b,
    }
}

/// Parse a `mod=` filter value (e.g. `HDDT`, `HD,DT`) into a list of
/// 2-character mod acronyms. Returns `None` if the input has an odd
/// number of characters after splitting on commas (treat as parse error).
/// An empty input returns `Some(vec![])`.
fn parse_mod_filter(value: &str) -> Option<Vec<String>> {
    if value.is_empty() {
        return Some(Vec::new());
    }
    // "NM" is treated as a "no mod" marker (label only; osu! has no NM mod).
    // It produces an empty required set so `mod==NM` matches scores with zero mods.
    if value.trim().eq_ignore_ascii_case("NM") {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for part in value.split(',') {
        let upper = part.trim().to_uppercase();
        let chars: Vec<char> = upper.chars().collect();
        if !chars.len().is_multiple_of(2) {
            return None;
        }
        for chunk in chars.chunks(2) {
            out.push(chunk.iter().collect::<String>());
        }
    }
    Some(out)
}

fn score_matches_filters(score: &Score, filters: &[String]) -> bool {
    for filter in filters {
        // Special case: "mod=" with empty value means "no required mods" → passes.
        // parse_filter_token rejects empty values, so we handle this here to
        // preserve the legacy behavior (split_once('=') used to allow it).
        if filter == "mod=" {
            continue;
        }
        let Some((key, op, value)) = parse_filter_token(filter) else {
            return false;
        };
        if !apply_filter(score, &key, op, &value) {
            return false;
        }
    }
    true
}

fn apply_filter(score: &Score, key: &str, op: FilterOp, value: &str) -> bool {
    match key {
        "miss" => value
            .parse::<i64>()
            .is_ok_and(|v| cmp_i64(score.statistics.count_miss, v, op)),
        "combo" => value
            .parse::<i64>()
            .is_ok_and(|v| cmp_i64(score.max_combo, v, op)),
        "pp" => value
            .parse::<f64>()
            .ok()
            .and_then(|v| score.pp.map(|p| cmp_f64(p, v, op, 0.5)))
            .unwrap_or(false),
        "score" => value
            .parse::<i64>()
            .is_ok_and(|v| cmp_i64(score.score_value, v, op)),
        "acc" | "accuracy" => value
            .parse::<f64>()
            .is_ok_and(|v| cmp_f64(score.accuracy * 100.0, v, op, 0.5)),
        "mod" => apply_mod_filter(score, op, value),
        _ => true, // 未知 key 静默忽略（与现有行为一致）
    }
}

fn apply_mod_filter(score: &Score, op: FilterOp, value: &str) -> bool {
    // Comparison operators (>, <, >=, <=) on the mod key have no
    // set-membership semantics, so they silently pass without
    // touching the score.
    if matches!(
        op,
        FilterOp::Gt | FilterOp::Lt | FilterOp::GtEq | FilterOp::LtEq
    ) {
        return true;
    }
    let required = match parse_mod_filter(value) {
        Some(v) => v,
        None => return false, // 奇数长度等解析失败
    };
    let present: Vec<String> = score
        .mods
        .iter()
        .map(|m| m.acronym().as_str().to_string())
        .collect();
    let subset = required.iter().all(|r| present.contains(r));
    match op {
        FilterOp::Eq => subset, // = 子集（必须包含所有列出的 mod）
        FilterOp::EqEq => {
            // == 精确集合（分数的 mod 集必须恰好等于）
            subset && present.len() == required.len()
        }
        FilterOp::NotEq => !subset, // != 子集取反
        FilterOp::Gt | FilterOp::Lt | FilterOp::GtEq | FilterOp::LtEq => unreachable!(),
    }
}

async fn handle_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    params: ScoreQueryParams<'_>,
    mode: GameMode,
) {
    tracing::trace!("{}", log_fmt!("main.handle_score_query_start"));

    // For self/bound users (no explicit username/qq), resolve user_id from DB
    // and parallelize the two API calls. For username/QQ lookups, use the
    // existing sequential resolve_score_user flow.
    let is_self = params.username.is_none() && params.qq.is_none();
    let include_fails = !params.is_pass;
    let raw_limit = params.limit_end.unwrap_or(params.limit);
    let has_client_filter = params.filters.is_some_and(|f| !f.is_empty())
        || params.beatmap_id.is_some()
        || params.score_id.is_some();
    let api_limit = if has_client_filter {
        raw_limit.max(SCORE_API_FETCH_LIMIT)
    } else {
        raw_limit
    };
    let (user_id, resolved_username, user_stats, score_result) = if is_self {
        let (uid, name) = match ctx.resolve_binding(msg.user_id).await {
            Some(binding) => binding,
            None => {
                let _ = resp_tx
                    .send(user_str("bind.not_bound").replace("{qq}", &msg.user_id.to_string()))
                    .await;
                return;
            }
        };

        tracing::trace!(
            "{}",
            log_fmt!(
                "main.handle_score_query_bound",
                user_id = uid,
                username = name
            )
        );
        ctx.scheduler.trigger_update(uid, mode).await;

        let is_pass = params.is_pass;
        let qq = msg.user_id;
        let rate_limiter = ctx.rate_limiter.clone();
        let oauth = ctx.oauth.clone();
        let rl2 = rate_limiter.clone();
        let oa2 = oauth.clone();

        let (stats_result, scores) = tokio::join!(
            api::fetch_user_stats_by_user_id(&rate_limiter, &oauth, uid, mode),
            score_dedup().run_or_wait((uid, is_pass, api_limit, mode), move || {
                let rate_limiter = rl2.clone();
                let oauth = oa2.clone();

                async move {
                    api::get_user_recent(&rate_limiter, &oauth, uid, mode, include_fails, api_limit)
                        .await
                        .map(Arc::new)
                        .map_err(|e| {
                            warn!(user_id = uid, mode = ?mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                            match e {
                                ApiError::NotFound => user_str("error.not_found")
                                    .replace("{qq}", &qq.to_string()),
                                ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                                    user_str("error.rate_limit")
                                        .replace("{qq}", &qq.to_string())
                                        .replace("{secs}", &secs.to_string())
                                }
                                ApiError::RateLimitedWithRetryAfter(None) => {
                                    user_str("error.rate_limit_generic")
                                        .replace("{qq}", &qq.to_string())
                                }
                                ApiError::ClientRateLimited => {
                                    user_str("error.client_rate_limit")
                                        .replace("{qq}", &qq.to_string())
                                }
                                e => {
                                    tracing::error!(error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                                user_str("error.data_fetch_failed")
                                    .replace("{qq}", &qq.to_string())
                                }
                            }
                        })
                }
            }),
        );

        let user_stats = match stats_result {
            Ok(stats) => {
                if let Err(e) = ctx
                    .storage
                    .set_user_id(&stats.username, stats.user_id)
                    .await
                {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "{}",
                        log_fmt!("main.cache_user_id_failed")
                    );
                }
                stats
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id = uid, "{}", log_fmt!("main.resolve_bound_user_failed"));
                let _ = resp_tx
                    .send(
                        user_str("error.data_fetch_failed")
                            .replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
        };

        (uid, name, user_stats, scores)
    } else {
        let qq = msg.user_id;
        let (uid, name, user_stats) =
            match resolve_score_user(ctx, msg, params.username, params.qq, mode, resp_tx).await {
                Some(u) => {
                    tracing::trace!(
                        "{}",
                        log_fmt!(
                            "main.resolve_score_user_resolved",
                            user_id = u.0,
                            username = &u.1
                        )
                    );
                    u
                }
                None => {
                    tracing::warn!("{}", log_fmt!("main.resolve_score_user_none"));
                    return;
                }
            };

        ctx.scheduler.trigger_update(uid, mode).await;
        let dedup_key = (uid, params.is_pass, api_limit, mode);
        let dedup_rate_limiter = ctx.rate_limiter.clone();
        let dedup_oauth = ctx.oauth.clone();
        let dedup_mode = mode;

        tracing::trace!(
            "{}",
            log_fmt!(
                "main.fetch_scores",
                user_id = uid,
                mode = &format!("{:?}", mode),
                limit = api_limit
            )
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
                        api_limit,
                    )
                    .await
                    .map(Arc::new)
                    .map_err(|e| {
                        warn!(user_id = uid, mode = ?dedup_mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                        match e {
                            ApiError::NotFound => {
                                user_str("error.not_found").replace("{qq}", &qq.to_string())
                            }
                            ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                                user_str("error.rate_limit")
                                    .replace("{qq}", &qq.to_string())
                                    .replace("{secs}", &secs.to_string())
                            }
                            ApiError::RateLimitedWithRetryAfter(None) => {
                                user_str("error.rate_limit_generic")
                                    .replace("{qq}", &qq.to_string())
                            }
                            ApiError::ClientRateLimited => user_str("error.client_rate_limit")
                                .replace("{qq}", &qq.to_string()),
                            e => {
                                tracing::error!(error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                                user_str("error.data_fetch_failed")
                                    .replace("{qq}", &qq.to_string())
                            }
                        }
                    })
                }
            })
            .await;

        (uid, name, user_stats, scores)
    };

    let dedup_username = resolved_username.clone();
    let qq = msg.user_id;

    match score_result {
        Ok(mut scores) => {
            if scores.is_empty() {
                let empty_msg = if include_fails {
                    user_str("query.no_records").replace("{qq}", &msg.user_id.to_string())
                } else {
                    user_str("query.no_records_pass").replace("{qq}", &msg.user_id.to_string())
                };
                let _ = resp_tx.send(empty_msg).await;
                return;
            }
            ctx.last_beatmap
                .set(msg.group_id, scores[0].beatmap_id as u32);

            // beatmap_id client-side filter
            if let Some(bid) = params.beatmap_id {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| s.beatmap_id == bid as i64);
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            // score_id client-side filter
            if let Some(sid) = params.score_id {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| s.score_id == sid as i64);
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if let Some(filters) = params.filters {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| score_matches_filters(s, filters));
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if params.is_single {
                let index = (params.limit - 1) as usize;
                if index >= scores.len() {
                    let _ = resp_tx
                        .send(
                            user_str("query.index_out_of_range")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{pos}", &params.limit.to_string())
                                .replace("{name}", user_str("query.noun_replay"))
                                .replace("{total}", &scores.len().to_string()),
                        )
                        .await;
                    return;
                }
                let score = &scores[index];
                render_and_send_single_score(SingleScoreRenderParams {
                    ctx,
                    msg,
                    resp_tx,
                    score,
                    mode,
                    user_stats: &user_stats,
                    position: Some(index),
                    is_pass: params.is_pass,
                })
                .await;
            } else {
                if let Some(end) = params.limit_end {
                    let start = (params.limit - 1) as usize;
                    let end = end as usize;
                    if start >= scores.len() {
                        let _ = resp_tx
                            .send(
                                user_str("query.index_out_of_range")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{pos}", &params.limit.to_string())
                                    .replace("{name}", user_str("query.noun_replay"))
                                    .replace("{total}", &scores.len().to_string()),
                            )
                            .await;
                        return;
                    }
                    let end = end.min(scores.len());
                    let scores_arc = Arc::make_mut(&mut scores);
                    let _ = scores_arc.drain(..start);
                    scores_arc.truncate(end - start);
                }

                // Local PP re-computation + cover download: the osu! API
                // may return pp=null for failed scores, loved/pending
                // beatmaps, or unsupported mod combos.  Re-compute locally
                // and download covers in one pass so network I/O overlaps.
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
                    .calculate_change(user_id, mode, &user_stats)
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
                let pp_change = change.as_ref().and_then(|c| c.pp_change);
                let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
                let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);
                let score_label = if params.is_pass {
                    user_str("fmt.recent_pass")
                } else {
                    user_str("fmt.recent_play")
                };
                let score_count_text = user_str("fmt.score_count");
                let render_result = tokio::time::timeout(
                    std::time::Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
                    osubot_render::render_score_list_card(osubot_render::ScoreListCardParams {
                        scores: &scores,
                        username: &dedup_username,
                        mode,
                        label: score_label,
                        count_text: score_count_text,
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
                        tracing::info!(
                            "{}",
                            log_fmt!("main.score_list_card_rendered", bytes = jpeg_bytes.len())
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
                                let _ = resp_tx_img
                                    .send(
                                        user_str("error.image_send_failed")
                                            .replace("{qq}", &qq.to_string()),
                                    )
                                    .await;
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        warn!(error = %e, "{}", log_fmt!("main.render_score_list_failed_text"));
                        let response =
                            format_scores(&scores, &dedup_username, mode, params.is_pass);
                        let _ = resp_tx.send(response).await;
                    }
                    Err(_) => {
                        warn!("{}", log_fmt!("main.render_score_list_timeout_text"));
                        let response =
                            format_scores(&scores, &dedup_username, mode, params.is_pass);
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
    mode: GameMode,
) {
    let (username, qq, beatmap_id, score_id, filters, limit, limit_end, is_all) = match cmd {
        Command::ScoreOnBeatmap {
            username,
            qq,
            beatmap_id,
            score_id,
            filters,
            limit,
            limit_end,
            is_all,
            ..
        } => (
            username.as_deref(),
            *qq,
            *beatmap_id,
            *score_id,
            filters.as_deref(),
            *limit,
            *limit_end,
            *is_all,
        ),
        _ => return,
    };

    if let Some(sid) = score_id {
        info!(score_id = sid, "{}", log_fmt!("main.score_by_id"));
        let qq = msg.user_id;
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
                            ApiError::NotFound => {
                                user_str("query.score_not_found").replace("{qq}", &qq.to_string())
                            }
                            e => {
                                warn!(error = ?e, "{}", log_fmt!("main.get_score_by_id_failed"));
                                user_str("query.score_fetch_failed")
                                    .replace("{qq}", &qq.to_string())
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
            let _ = resp_tx
                .send(user_str("query.user_info_failed").replace("{qq}", &msg.user_id.to_string()))
                .await;
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
                if let Err(e) = ctx
                    .storage
                    .set_user_id(&stats.username, stats.user_id)
                    .await
                {
                    tracing::warn!(
                        username = %stats.username,
                        user_id = stats.user_id,
                        error = %e,
                        "{}",
                        log_fmt!("main.cache_user_id_failed")
                    );
                }
                ctx.scheduler.trigger_update(user_id, mode).await;
                stats
            }
            Err(e) => {
                warn!(user_id = user_id, error = ?e, "{}", log_fmt!("main.fetch_stats_score_id_failed"));
                let _ = resp_tx
                    .send(
                        user_str("error.data_fetch_failed")
                            .replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
        };
        render_and_send_single_score(SingleScoreRenderParams {
            ctx,
            msg,
            resp_tx,
            score: &score,
            mode,
            user_stats: &user_stats,
            position: None,
            is_pass: true,
        })
        .await;
        return;
    }

    let resolved_bid = match beatmap_id {
        Some(bid) => bid,
        None => match ctx.last_beatmap.get(msg.group_id) {
            Some(bid) => bid,
            None => {
                let _ = resp_tx
                    .send(
                        user_str("query.need_beatmap_or_cache")
                            .replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
        },
    };

    info!(
        beatmap_id = resolved_bid,
        mode = ?mode,
        filters = ?filters,
        limit,
        is_all,
        "{}",
        log_fmt!("main.score_on_beatmap")
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
    let qq = msg.user_id;

    if is_all {
        let raw_api_limit = limit_end.or(if limit > 1 { Some(limit) } else { None });
        let api_limit = match (raw_api_limit, filters.is_some_and(|f| !f.is_empty())) {
            (Some(n), true) => Some(n.max(SCORE_API_FETCH_LIMIT)),
            (other, _) => other,
        };
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
                        ApiError::NotFound => {
                            user_str("query.no_score_on_map").replace("{qq}", &qq.to_string())
                        }
                        e => {
                            warn!(error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                            user_str("query.score_fetch_failed")
                                .replace("{qq}", &qq.to_string())
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
            let _ = resp_tx
                .send(user_str("query.no_score_on_map").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }

        let mut scores = scores;
        if let Some(filters) = filters {
            scores.retain(|s| score_matches_filters(s, filters));
            if scores.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_match")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{name}", user_str("query.noun_score")),
                    )
                    .await;
                return;
            }
        }

        if let Some(end) = limit_end {
            let start = (limit - 1) as usize;
            let end = end as usize;
            if start >= scores.len() {
                let _ = resp_tx
                    .send(
                        user_str("query.index_out_of_range")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{pos}", &limit.to_string())
                            .replace("{name}", user_str("query.noun_score"))
                            .replace("{total}", &scores.len().to_string()),
                    )
                    .await;
                return;
            }
            let end = end.min(scores.len());
            let _ = scores.drain(..start);
            scores.truncate(end - start);
        }

        render_and_send_score_list(ctx, msg, resp_tx, &scores, &user_stats, &username_str, mode)
            .await;
    } else if limit == 1 && limit_end.is_none() {
        let active_filters = filters.filter(|f| !f.is_empty());
        if let Some(filters) = active_filters {
            let api_limit = SCORE_API_FETCH_LIMIT;
            let key = (_user_id, resolved_bid as i64, mode, Some(api_limit));
            let dedup_rscores = ctx.rate_limiter.clone();
            let dedup_oscores = ctx.oauth.clone();
            let scores_result =
                beatmap_scores_dedup()
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
                                Some(api_limit),
                            )
                            .await
                            .map_err(|e| match e {
                                ApiError::NotFound => user_str("query.no_score_on_map")
                                    .replace("{qq}", &qq.to_string()),
                                e => {
                                    warn!(error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                            user_str("query.score_fetch_failed")
                                .replace("{qq}", &qq.to_string())
                                }
                            })
                        }
                    })
                    .await;
            let mut scores = match scores_result {
                Ok(s) => s,
                Err(err_msg) => {
                    let _ = resp_tx.send(err_msg).await;
                    return;
                }
            };
            if scores.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_score_on_map").replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
            scores.retain(|s| score_matches_filters(s, filters));
            if scores.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_match")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{name}", user_str("query.noun_score")),
                    )
                    .await;
                return;
            }
            let score = scores.swap_remove(0);
            ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
            render_and_send_single_score(SingleScoreRenderParams {
                ctx,
                msg,
                resp_tx,
                score: &score,
                mode,
                user_stats: &user_stats,
                position: None,
                is_pass: true,
            })
            .await;
        } else {
            let key = (_user_id, resolved_bid as i64, mode);
            let dedup_rscore = ctx.rate_limiter.clone();
            let dedup_oscore = ctx.oauth.clone();
            let score_result = beatmap_score_dedup()
                .run_or_wait(key, move || {
                    let rate_limiter = dedup_rscore.clone();
                    let oauth = dedup_oscore.clone();

                    async move {
                        api::get_user_beatmap_score(
                            &rate_limiter,
                            &oauth,
                            resolved_bid as i64,
                            _user_id,
                            mode,
                            &None,
                        )
                        .await
                        .map_err(|e| match e {
                            ApiError::NotFound => {
                                user_str("query.no_score_on_map").replace("{qq}", &qq.to_string())
                            }
                            e => {
                                warn!(
                                    error = ?e,
                                    beatmap_id = resolved_bid,
                                    "{}",
                                    log_fmt!("main.get_user_beatmap_score_failed")
                                );
                                user_str("query.score_fetch_failed")
                                    .replace("{qq}", &qq.to_string())
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
            render_and_send_single_score(SingleScoreRenderParams {
                ctx,
                msg,
                resp_tx,
                score: &score,
                mode,
                user_stats: &user_stats,
                position: None,
                is_pass: true,
            })
            .await;
        }
    } else {
        let raw_limit = limit_end.unwrap_or(limit);
        let api_limit = if filters.is_some_and(|f| !f.is_empty()) {
            raw_limit.max(SCORE_API_FETCH_LIMIT)
        } else {
            raw_limit
        };
        let n = limit as usize;
        let key = (_user_id, resolved_bid as i64, mode, Some(api_limit));
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
                        Some(api_limit),
                    )
                    .await
                    .map_err(|e| match e {
                        ApiError::NotFound => {
                            user_str("query.no_score_on_map").replace("{qq}", &qq.to_string())
                        }
                        e => {
                            warn!(error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                            user_str("query.score_fetch_failed")
                                .replace("{qq}", &qq.to_string())
                        }
                    })
                }
            })
            .await;
        let mut scores = match scores_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };

        if let Some(filters) = filters {
            scores.retain(|s| score_matches_filters(s, filters));
            if scores.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_match")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{name}", user_str("query.noun_score")),
                    )
                    .await;
                return;
            }
        }

        if let Some(end) = limit_end {
            let start = (limit - 1) as usize;
            let end = end as usize;
            if start >= scores.len() {
                let _ = resp_tx
                    .send(
                        user_str("query.index_out_of_range")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{pos}", &limit.to_string())
                            .replace("{name}", user_str("query.noun_score"))
                            .replace("{total}", &scores.len().to_string()),
                    )
                    .await;
                return;
            }
            let end = end.min(scores.len());
            let _ = scores.drain(..start);
            scores.truncate(end - start);
            if scores.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_match")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{name}", user_str("query.noun_score")),
                    )
                    .await;
                return;
            }
            render_and_send_score_list(
                ctx,
                msg,
                resp_tx,
                &scores,
                &user_stats,
                &username_str,
                mode,
            )
            .await;
        } else {
            if scores.len() < n {
                let _ = resp_tx
                    .send(
                        user_str("query.index_out_of_range")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{pos}", &n.to_string())
                            .replace("{name}", user_str("query.noun_score"))
                            .replace("{total}", &scores.len().to_string()),
                    )
                    .await;
                return;
            }
            let score = scores.into_iter().nth(n - 1).expect("len checked above");
            ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
            render_and_send_single_score(SingleScoreRenderParams {
                ctx,
                msg,
                resp_tx,
                score: &score,
                mode,
                user_stats: &user_stats,
                position: Some(n - 1),
                is_pass: true,
            })
            .await;
        }
    }
}

struct SingleScoreRenderParams<'a> {
    ctx: &'a BotContext,
    msg: &'a QQMessage,
    resp_tx: &'a mpsc::Sender<String>,
    score: &'a Score,
    mode: GameMode,
    user_stats: &'a UserStats,
    position: Option<usize>,
    is_pass: bool,
}

async fn render_and_send_single_score(params: SingleScoreRenderParams<'_>) {
    let SingleScoreRenderParams {
        ctx,
        msg,
        resp_tx,
        score,
        mode,
        user_stats,
        position,
        is_pass,
    } = params;
    let mut score = score.clone();
    enrich_score_with_pp(&mut score, mode, true).await;

    let ur_value = if mode == GameMode::Osu && score.score_id > 0 && score.has_replay {
        tracing::trace!(score_id = score.score_id, mode = ?mode, is_lazer = score.is_lazer, length = score.length_seconds, "{}", log_fmt!("main.ur_calculation_start"));
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
                    "{}",
                    log_fmt!("main.ur_calculation_succeeded")
                );
                Some(ur_val)
            }
            Ok(None) => {
                tracing::warn!(
                    score_id = score.score_id,
                    "{}",
                    log_fmt!("main.ur_calculation_none")
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    score_id = score.score_id,
                    "{}",
                    log_fmt!("main.ur_calculation_timeout")
                );
                None
            }
        }
    } else {
        tracing::trace!(
            score_id = score.score_id,
            mode = ?mode,
            is_lazer = score.is_lazer,
            has_replay = score.has_replay,
            "{}",
            log_fmt!("main.ur_calculation_skipped")
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
        .await
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

    let qq = msg.user_id;

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
                    let _ = resp_tx_img
                        .send(user_str("error.image_send_failed").replace("{qq}", &qq.to_string()))
                        .await;
                }
            });
        }
        Ok(Err(e)) => {
            warn!(error = %e, "{}", log_fmt!("main.render_score_card_failed_text"));
            let text = format_score(&score, &user_stats.username, mode, position, is_pass);
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            cancel_flag.store(true, Ordering::Relaxed);
            warn!("{}", log_fmt!("main.render_score_card_timeout_text"));
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
        .await
        .inspect_err(|e| {
            tracing::warn!(
                user_id = user_stats.user_id,
                mode = ?mode,
                error = %e,
                "{}",
                log_fmt!("main.calculate_change_failed")
            )
        })
        .ok()
        .flatten();
    let pp_change = change.as_ref().and_then(|c| c.pp_change);
    let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
    let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);

    let score_label = user_str("fmt.beatmap_score");
    let score_count_text = user_str("fmt.score_count");
    let render_result = tokio::time::timeout(
        Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
        render_score_list_card(osubot_render::ScoreListCardParams {
            scores: &scores_mut,
            username,
            mode,
            label: score_label,
            count_text: score_count_text,
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

    let qq = msg.user_id;

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
                    let _ = resp_tx_img
                        .send(user_str("error.image_send_failed").replace("{qq}", &qq.to_string()))
                        .await;
                }
            });
        }
        Ok(Err(e)) => {
            warn!(error = %e, "{}", log_fmt!("main.render_score_list_failed_text"));
            let text = format_scores(&scores_mut, username, mode, true);
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            warn!("{}", log_fmt!("main.render_score_list_timeout_text"));
            let text = format_scores(&scores_mut, username, mode, true);
            let _ = resp_tx.send(text).await;
        }
    }
}

/// 解析本次命令的"目标 QQ"，用于在命令未显式指定模式时回退到该用户的 `default_mode`。
///
/// 设计语义：`default_mode` 是**被查询目标用户**的偏好（"我喜欢用 taiko 模式展示成绩"），
/// 而不是查询发起者的偏好。这意味着：
/// - `!p`（自己）→ target = msg.user_id → 用发起者自己的 default_mode
/// - `!p ZnCookie` / `where ZnCookie` / `!p @123456` → target = ZnCookie 的 QQ → 用 ZnCookie 的 default_mode
/// - `今日高光` → target = msg.user_id → 用发起者自己的 default_mode
///
/// 因此 `A !mode 1` 之后，不仅 A 自己的 `!p` 走 taiko，其他人对 A 用 `!p` / `where A` 也会走 taiko。
/// 这是 by design：让用户配置一次就适用于所有查询该用户名的场景，避免每次查询都要带 `:1`。
async fn resolve_cmd_target_qq(cmd: &Command, msg: &QQMessage, storage: &Storage) -> Option<i64> {
    match cmd {
        Command::QuerySelf { .. } => Some(msg.user_id),
        Command::QueryUser { username, .. } => match storage.find_qq_by_username(username).await {
            Ok(qq) => qq,
            Err(e) => {
                warn!(username = %username, error = %e, "{}", log_fmt!("main.find_qq_by_username_error", username = username, error = &e.to_string()));
                None
            }
        },
        Command::QueryMentionedUser { qq, .. } => Some(*qq),
        Command::Pass { qq: Some(qq), .. }
        | Command::Recent { qq: Some(qq), .. }
        | Command::ScoreOnBeatmap { qq: Some(qq), .. } => Some(*qq),
        Command::Pass {
            qq: None,
            username: Some(username),
            ..
        }
        | Command::Recent {
            qq: None,
            username: Some(username),
            ..
        }
        | Command::ScoreOnBeatmap {
            qq: None,
            username: Some(username),
            ..
        } => match storage.find_qq_by_username(username).await {
            Ok(qq) => qq,
            Err(e) => {
                warn!(username = %username, error = %e, "{}", log_fmt!("main.find_qq_by_username_error", username = username, error = &e.to_string()));
                None
            }
        },
        Command::Highlight { .. } => Some(msg.user_id),
        Command::Pass {
            qq: None,
            username: None,
            ..
        }
        | Command::Recent {
            qq: None,
            username: None,
            ..
        }
        | Command::ScoreOnBeatmap {
            qq: None,
            username: None,
            ..
        } => Some(msg.user_id),
        _ => None,
    }
}

/// 命令是否涉及模式（决定是否需要 `default_mode` 兜底）。
fn mode_sensitive(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::QuerySelf { .. }
            | Command::QueryUser { .. }
            | Command::QueryMentionedUser { .. }
            | Command::Pass { .. }
            | Command::Recent { .. }
            | Command::ScoreOnBeatmap { .. }
            | Command::Highlight { .. }
    )
}

/// 从命令中提取显式指定的 mode（未指定返回 None）。
fn extract_explicit_mode(cmd: &Command) -> Option<GameMode> {
    match cmd {
        Command::QuerySelf { mode }
        | Command::QueryUser { mode, .. }
        | Command::QueryMentionedUser { mode, .. }
        | Command::Pass { mode, .. }
        | Command::Recent { mode, .. }
        | Command::Highlight { mode, .. }
        | Command::ScoreOnBeatmap { mode, .. } => *mode,
        _ => None,
    }
}

/// 解析本次命令最终使用的模式。
///
/// 优先级：命令中显式指定（`!p :1`） > 目标用户的 `default_mode` > `Osu` 回退。
/// "目标用户"由 [`resolve_cmd_target_qq`] 决定——通常是**被查询者**的 QQ，
/// 因此 `A !p B` 在 A 未指定模式时使用 B 的 default_mode，而非 A 的。
async fn resolve_mode(
    storage: &Storage,
    target_qq: Option<i64>,
    explicit_mode: Option<GameMode>,
) -> GameMode {
    match explicit_mode {
        Some(mode) => mode,
        None => match target_qq {
            Some(qq) => match storage.get_default_mode(qq).await {
                Ok(Some(mode)) => mode,
                Ok(None) => GameMode::Osu,
                Err(e) => {
                    warn!(user_id = qq, error = %e, "{}", log_fmt!("main.get_default_mode_error"));
                    GameMode::Osu
                }
            },
            None => GameMode::Osu,
        },
    }
}

/// Build a JSON payload for the plugin command dispatch.
fn build_cmd_payload(
    cmd: &Command,
    cmd_name: &str,
    msg: &QQMessage,
    resolved_mode: Option<GameMode>,
) -> serde_json::Value {
    let mode = resolved_mode.map(|m| match m {
        GameMode::Osu => 0,
        GameMode::Taiko => 1,
        GameMode::Catch => 2,
        GameMode::Mania => 3,
    });
    let username = match cmd {
        Command::QueryUser { username, .. } => Some(username.as_str()),
        Command::Bind { username, .. } => Some(username.as_str()),
        Command::ScoreOnBeatmap { username, .. }
        | Command::Pass { username, .. }
        | Command::Recent { username, .. }
        | Command::ProfileCard { username, .. } => username.as_deref(),
        Command::BeatmapPreview { .. } => None,
        _ => None,
    };
    serde_json::json!({
        "command_type": cmd_name,
        "group_id": msg.group_id,
        "user_id": msg.user_id,
        "message": msg.message,
        "mentioned_user_id": msg.mentioned_user_id,
        "mode": mode,
        "username": username,
        "qq": match cmd {
            Command::QueryMentionedUser { qq, .. } => Some(*qq),
            Command::Pass { qq, .. }
            | Command::Recent { qq, .. }
            | Command::ScoreOnBeatmap { qq, .. }
            | Command::ProfileCard { qq, .. } => *qq,
            Command::BeatmapPreview { .. } => None,
            _ => None,
        },
        "beatmap_id": match cmd {
            Command::ScoreOnBeatmap { beatmap_id, .. }
            | Command::Pass { beatmap_id, .. }
            | Command::Recent { beatmap_id, .. }
            | Command::BeatmapPreview { beatmap_id, .. } => *beatmap_id,
            _ => None,
        },
        "score_id": match cmd {
            Command::ScoreOnBeatmap { score_id, .. }
            | Command::Pass { score_id, .. }
            | Command::Recent { score_id, .. }
            | Command::BeatmapPreview { score_id, .. } => *score_id,
            _ => None,
        },
        "limit": match cmd {
            Command::ScoreOnBeatmap { limit, .. } | Command::Pass { limit, .. } | Command::Recent { limit, .. } => Some(*limit),
            _ => None,
        },
        "filters": match cmd {
            Command::ScoreOnBeatmap { filters, .. }
            | Command::Pass { filters, .. }
            | Command::Recent { filters, .. } => filters.clone(),
            _ => None,
        },
        "limit_end": match cmd {
            Command::ScoreOnBeatmap { limit_end, .. }
            | Command::Pass { limit_end, .. }
            | Command::Recent { limit_end, .. } => *limit_end,
            _ => None,
        },
    })
}

/// Main command dispatcher. Parses the command text, resolves the target user,
/// executes the appropriate query, and sends the response via `resp_tx`.
pub(crate) async fn handle_command(ctx: BotContext, msg: QQMessage, resp_tx: mpsc::Sender<String>) {
    // ==== Plugin on_message dispatch ====
    {
        let msg_payload = serde_json::json!({
            "group_id": msg.group_id,
            "user_id": msg.user_id,
            "message": msg.message,
            "mentioned_user_id": msg.mentioned_user_id,
        });
        let msg_payload_str = msg_payload.to_string();
        let action = PluginManager::dispatch_message(&ctx.plugin_manager, &msg_payload_str).await;
        match action {
            PluginActionResult::Handled(response) => {
                let _ = resp_tx.send(response).await;
                return;
            }
            PluginActionResult::Intercepted => return,
            PluginActionResult::Next => {}
        }
    }

    // ==== Plugin on_command dispatch (brief locks managed inside dispatch_command) ====
    let cmd_opt = parse_command(&msg.message, msg.mentioned_user_id);

    // Pre-resolve mode once for both plugin dispatch and native handlers.
    // 仅对 mode-sensitive 命令触发 DB 查询；SetDefaultMode / Bind / Unbind / Help / ProfileCard
    // 不涉及模式，跳过 resolve_cmd_target_qq 和 get_default_mode。
    let resolved_mode = match cmd_opt.as_ref() {
        Some(cmd) if mode_sensitive(cmd) => {
            let target_qq = resolve_cmd_target_qq(cmd, &msg, &ctx.storage).await;
            let explicit_mode = extract_explicit_mode(cmd);
            Some(resolve_mode(&ctx.storage, target_qq, explicit_mode).await)
        }
        _ => None,
    };

    if let Some(ref cmd) = cmd_opt {
        let cmd_name = cmd.command_name();
        let cmd_payload = build_cmd_payload(cmd, cmd_name, &msg, resolved_mode);
        let cmd_payload_str = cmd_payload.to_string();
        let action =
            PluginManager::dispatch_command(&ctx.plugin_manager, cmd_name, &cmd_payload_str).await;
        match action {
            PluginActionResult::Handled(response) => {
                let _ = resp_tx.send(response).await;
                return;
            }
            PluginActionResult::Intercepted => return,
            PluginActionResult::Next => {}
        }
    }

    // ==== Fallback: old text dispatch (native commands) ====

    // 未识别命令 — 插件已拒绝，直接结束
    if cmd_opt.is_none() {
        return;
    }
    let cmd = cmd_opt.expect("guarded by cmd_opt.is_none() early-return");

    // 命令开关检查
    let group_cfg = {
        let cfg = ctx.config.read().await;
        cfg.groups.get_group_config(msg.group_id)
    };
    if !group_cfg.is_enabled(cmd.group_name()) {
        debug!(group_id = msg.group_id, command = ?cmd.group_name(), "{}", log_fmt!("main.command_disabled"));
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
        let _ = resp_tx
            .send(user_str("error.rate_limit_generic").replace("{qq}", &msg.user_id.to_string()))
            .await;
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

    // Handle command and send response
    let mode = resolved_mode.unwrap_or(GameMode::Osu);
    match cmd {
        Command::QuerySelf { .. } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "{}", log_fmt!("main.query_self"));
            match ctx.storage.get_binding(msg.user_id).await {
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
                        info!(user_id = msg.user_id, osu_id = user_id, username = %username, "{}", log_fmt!("main.query_self_auto_bound"));
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
                        info!(
                            user_id = msg.user_id,
                            "{}",
                            log_fmt!("main.query_self_no_binding")
                        );
                        let _ = resp_tx
                            .send(
                                user_str("bind.not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
                Err(_) => {
                    error!(
                        user_id = msg.user_id,
                        "{}",
                        log_fmt!("main.query_self_db_error")
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        Command::QueryUser { username, .. } => {
            info!(group_id = msg.group_id, username = %username, mode = ?mode, "{}", log_fmt!("main.query_user"));
            match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, &username, mode)
                .await
            {
                Ok(stats) => {
                    // Cache user_id for future lookups (even for unbound users)
                    if let Err(e) = ctx
                        .storage
                        .set_user_id(&stats.username, stats.user_id)
                        .await
                    {
                        tracing::warn!(
                            username = %stats.username,
                            user_id = stats.user_id,
                            error = %e,
                            "{}",
                            log_fmt!("main.cache_user_id_failed")
                        );
                    }
                    if stats.username != username {
                        if let Err(e) = ctx.storage.set_user_id(&username, stats.user_id).await {
                            tracing::warn!(
                                username = %username,
                                user_id = stats.user_id,
                                error = %e,
                                "{}",
                                log_fmt!("main.cache_user_id_failed")
                            );
                        }
                    }
                    ctx.scheduler.trigger_update(stats.user_id, mode).await;
                    let change = ctx
                        .storage
                        .calculate_change(stats.user_id, mode, &stats)
                        .await
                        .inspect_err(|e| {
                            tracing::warn!(
                                user_id = stats.user_id,
                                mode = ?mode,
                                error = %e,
                                "{}",
                                log_fmt!("main.calculate_change_failed")
                            )
                        })
                        .ok()
                        .flatten();
                    let has_change = change.is_some();
                    info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "{}", log_fmt!("main.query_user_success"));
                    let response = format_stats_with_change(&stats, &change, mode);
                    let _ = resp_tx.send(response).await;
                    if !has_change {
                        info!(username = %username, "{}", log_fmt!("main.query_user_no_change"));
                    }
                }
                Err(e) => {
                    warn!(username = %username, mode = ?mode, error = ?e, "{}", log_fmt!("main.query_user_failed"));
                    let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                }
            }
        }
        Command::QueryMentionedUser { qq, .. } => {
            info!(qq = qq, group_id = msg.group_id, mode = ?mode, "{}", log_fmt!("main.query_mentioned_user"));
            match ctx.storage.get_binding(qq).await {
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
                        info!(qq = qq, osu_id = user_id, username = %username, "{}", log_fmt!("main.query_mentioned_auto_bound"));
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
                        info!(qq = qq, "{}", log_fmt!("main.query_mentioned_no_binding"));
                        let _ = resp_tx
                            .send(
                                user_str("bind.mentioned_not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
                Err(_) => {
                    error!(qq = qq, "{}", log_fmt!("main.query_mentioned_db_error"));
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        Command::Bind { username } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, username = %username, "{}", log_fmt!("main.bind_command"));
            match ctx.storage.get_binding(msg.user_id).await {
                Ok(Some((_, existing_username))) => {
                    info!(user_id = msg.user_id, existing = %existing_username, "{}", log_fmt!("main.bind_already_bound"));
                    let _ = resp_tx
                        .send(
                            user_str("bind.already_bound")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", &existing_username),
                        )
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
                        match ctx.storage.has_pending_bind(msg.user_id).await {
                            Ok(true) => {
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.pending_exists")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                                return;
                            }
                            Err(_) => {
                                error!(
                                    user_id = msg.user_id,
                                    "{}",
                                    log_fmt!("main.bind_check_pending_failed")
                                );
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.failed_retry")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                                return;
                            }
                            _ => {}
                        }
                        match ctx
                            .storage
                            .add_pending_bind(msg.user_id, msg.group_id, &username)
                            .await
                        {
                            Ok(code) => {
                                info!(user_id = msg.user_id, username = %username, code = %code, "{}", log_fmt!("main.bind_pending_created"));
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.code_sent")
                                            .replace("{qq}", &msg.user_id.to_string())
                                            .replace("{code}", &code)
                                            .replace("{target}", &nickname),
                                    )
                                    .await;
                            }
                            Err(_) => {
                                error!(
                                    user_id = msg.user_id,
                                    "{}",
                                    log_fmt!("main.bind_create_pending_failed")
                                );
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.failed_retry")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                            }
                        }
                    } else {
                        match api::get_user_info(&ctx.rate_limiter, &ctx.oauth, &username).await {
                            Ok(Some(user_info)) => {
                                if let Err(e) =
                                    ctx.storage.set_user_id(&username, user_info.id).await
                                {
                                    warn!(error = %e, "{}", log_fmt!("main.cache_user_id_failed"));
                                }
                                match ctx
                                    .storage
                                    .bind(msg.user_id, user_info.id, &user_info.username)
                                    .await
                                {
                                    Ok(Ok(())) => {
                                        info!(user_id = msg.user_id, username = %user_info.username, "{}", log_fmt!("main.bind_success"));
                                        let _ = resp_tx
                                            .send(
                                                user_str("bind.success")
                                                    .replace("{qq}", &msg.user_id.to_string())
                                                    .replace("{name}", &user_info.username),
                                            )
                                            .await;
                                    }
                                    Ok(Err(bound_qq)) => {
                                        info!(user_id = msg.user_id, username = %username, bound_qq = bound_qq, "{}", log_fmt!("main.bind_failed_already_bound"));
                                        let _ = resp_tx
                                            .send(
                                                user_str("bind.already_bound_other")
                                                    .replace("{qq}", &msg.user_id.to_string()),
                                            )
                                            .await;
                                    }
                                    Err(_) => {
                                        error!(user_id = msg.user_id, username = %username, "{}", log_fmt!("main.bind_failed"));
                                        let _ = resp_tx
                                            .send(
                                                user_str("bind.failed_retry")
                                                    .replace("{qq}", &msg.user_id.to_string()),
                                            )
                                            .await;
                                    }
                                }
                            }
                            Ok(None) => {
                                info!(username = %username, "{}", log_fmt!("main.bind_user_not_found"));
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.user_not_found")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                            }
                            Err(e) => {
                                warn!(username = %username, error = ?e, "{}", log_fmt!("main.bind_user_info_failed"));
                                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                            }
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "{}", log_fmt!("main.bind_db_error"));
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        Command::Unbind => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "{}",
                log_fmt!("main.unbind_command")
            );
            // Check if user has pending unbind confirmation (within 5 minutes)
            match ctx.storage.get_pending_unbind(msg.user_id).await {
                Ok(Some(_)) => {
                    // Execute unbind and clear pending
                    match ctx.storage.unbind(msg.user_id).await {
                        Ok(_) => {
                            if let Err(e) = ctx.storage.remove_pending_unbind(msg.user_id).await {
                                tracing::warn!(
                                    user_id = msg.user_id,
                                    error = %e,
                                    "{}",
                                    log_fmt!("main.unbind_remove_pending_failed")
                                );
                            }
                            info!(user_id = msg.user_id, "{}", log_fmt!("main.unbind_success"));
                            let _ = resp_tx
                                .send(
                                    user_str("bind.unbind_success")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        }
                        Err(_) => {
                            error!(user_id = msg.user_id, "{}", log_fmt!("main.unbind_failed"));
                            let _ = resp_tx
                                .send(
                                    user_str("bind.unbind_failed")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        }
                    }
                }
                Ok(None) => {
                    // Ask for confirmation and set pending
                    match ctx.storage.get_binding(msg.user_id).await {
                        Ok(Some((_, current_username))) => {
                            if let Err(e) = ctx.storage.set_pending_unbind(msg.user_id).await {
                                tracing::warn!(
                                    user_id = msg.user_id,
                                    error = %e,
                                    "{}",
                                    log_fmt!("main.unbind_set_pending_failed")
                                );
                            }
                            info!(user_id = msg.user_id, username = %current_username, "{}", log_fmt!("main.unbind_confirmation"));
                            let _ = resp_tx
                                .send(
                                    user_str("bind.confirm_unbind")
                                        .replace("{qq}", &msg.user_id.to_string())
                                        .replace("{name}", &current_username),
                                )
                                .await;
                        }
                        Ok(None) => {
                            info!(
                                user_id = msg.user_id,
                                "{}",
                                log_fmt!("main.unbind_no_binding")
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("bind.not_bound_any")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        }
                        Err(_) => {
                            error!(
                                user_id = msg.user_id,
                                "{}",
                                log_fmt!("main.unbind_db_error")
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("error.db_error")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        }
                    }
                }
                Err(_) => {
                    error!(
                        user_id = msg.user_id,
                        "{}",
                        log_fmt!("main.unbind_pending_check_error")
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        Command::Highlight { .. } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "{}", log_fmt!("main.highlight_command"));

            let group_members =
                match get_group_member_list(&ctx.write, &ctx.onebot_api, msg.group_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(error = %e, "{}", log_fmt!("main.highlight_group_member_failed"));
                        let _ = resp_tx
                            .send(
                                user_str("error.get_group_member_failed")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                        return;
                    }
                };

            let all_bindings = match ctx.storage.get_all_user_bindings().await {
                Ok(bindings) => bindings,
                Err(_) => {
                    error!("{}", log_fmt!("main.highlight_fetch_bindings_failed"));
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                    return;
                }
            };

            let group_bindings: Vec<(i64, i64, String)> = all_bindings
                .into_iter()
                .filter(|(qq, _, _)| group_members.contains(qq))
                .collect();

            if group_bindings.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_bound_users").replace("{qq}", &msg.user_id.to_string()),
                    )
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
                    warn!(error = ?e, "{}", log_fmt!("main.highlight_fetch_failed"));
                    let err_msg = match e {
                        HighlightError::NoData => user_str("highlight.no_data").to_string(),
                        _ => {
                            user_str("error.query_failed").replace("{qq}", &msg.user_id.to_string())
                        }
                    };
                    let _ = resp_tx.send(err_msg).await;
                }
            }
        }
        Command::SetDefaultMode { mode } => match mode {
            Some(mode) => {
                info!(
                    user_id = msg.user_id,
                    ?mode,
                    "{}",
                    log_fmt!(
                        "main.set_default_mode",
                        user_id = &msg.user_id.to_string(),
                        mode = &format!("{:?}", mode)
                    )
                );
                match ctx.storage.set_default_mode(msg.user_id, mode).await {
                    Ok(true) => {
                        let _ = resp_tx
                            .send(
                                user_str("mode.set_success")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{mode}", mode.display_name()),
                            )
                            .await;
                    }
                    Ok(false) => {
                        let _ = resp_tx
                            .send(
                                user_str("mode.not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                    Err(e) => {
                        error!(user_id = msg.user_id, error = %e, "{}", log_fmt!("main.set_default_mode_error", user_id = &msg.user_id.to_string(), error = &e.to_string()));
                        let _ = resp_tx
                            .send(
                                user_str("error.db_error")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
            }
            None => {
                info!(
                    user_id = msg.user_id,
                    "{}",
                    log_fmt!("main.get_default_mode", user_id = &msg.user_id.to_string())
                );
                match ctx.storage.get_binding(msg.user_id).await {
                    Ok(Some(_)) => match ctx.storage.get_default_mode(msg.user_id).await {
                        Ok(Some(mode)) => {
                            let _ = resp_tx
                                .send(
                                    user_str("mode.get_success")
                                        .replace("{qq}", &msg.user_id.to_string())
                                        .replace("{mode}", mode.display_name()),
                                )
                                .await;
                        }
                        Ok(None) => {
                            let _ = resp_tx
                                .send(
                                    user_str("mode.get_success")
                                        .replace("{qq}", &msg.user_id.to_string())
                                        .replace("{mode}", GameMode::Osu.display_name()),
                                )
                                .await;
                        }
                        Err(e) => {
                            error!(
                                user_id = msg.user_id,
                                error = %e,
                                "{}",
                                log_fmt!("main.get_default_mode_error", user_id = &msg.user_id.to_string(), error = &e.to_string())
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("error.db_error")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        }
                    },
                    Ok(None) => {
                        let _ = resp_tx
                            .send(
                                user_str("bind.not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                    Err(e) => {
                        error!(
                            user_id = msg.user_id,
                            error = %e,
                            "{}",
                            log_fmt!("main.get_default_mode_error", user_id = &msg.user_id.to_string(), error = &e.to_string())
                        );
                        let _ = resp_tx
                            .send(
                                user_str("error.db_error")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
            }
        },
        Command::ProfileCard { username, qq } => {
            let target_user_id = match username {
                Some(ref name) => {
                    if let Ok(Some(cached_id)) = ctx.storage.get_user_id(name).await {
                        info!(username = %name, user_id = cached_id, "{}", log_fmt!("main.profile_card_cached"));
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
                                info!(username = %name, user_id = stats.user_id, "{}", log_fmt!("main.profile_card_by_username"));
                                if let Err(e) = ctx
                                    .storage
                                    .set_user_id(&stats.username, stats.user_id)
                                    .await
                                {
                                    tracing::warn!(
                                        username = %stats.username,
                                        user_id = stats.user_id,
                                        error = %e,
                                        "{}",
                                        log_fmt!("main.cache_user_id_failed")
                                    );
                                }
                                stats.user_id
                            }
                            Err(e) => {
                                warn!(username = %name, error = ?e, "{}", log_fmt!("main.profile_card_resolution_failed"));
                                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                                return;
                            }
                        }
                    }
                }
                None => match qq {
                    Some(mentioned_qq) => match ctx.storage.get_binding(mentioned_qq).await {
                        Ok(Some((user_id, current_username))) => {
                            info!(qq = mentioned_qq, osu_id = user_id, username = %current_username, "{}", log_fmt!("main.profile_card_mention"));
                            user_id
                        }
                        Ok(None) => {
                            if let Some((uid, uname)) = ctx.resolve_binding(mentioned_qq).await {
                                info!(qq = mentioned_qq, osu_id = uid, username = %uname, "{}", log_fmt!("main.profile_card_mention_bound"));
                                uid
                            } else {
                                info!(
                                    qq = mentioned_qq,
                                    "{}",
                                    log_fmt!("main.profile_card_mention_no_binding")
                                );
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.mentioned_not_bound")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                                return;
                            }
                        }
                        Err(_) => {
                            error!(
                                qq = mentioned_qq,
                                "{}",
                                log_fmt!("main.profile_card_mention_db_error")
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("error.db_error")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                            return;
                        }
                    },
                    None => match ctx.storage.get_binding(msg.user_id).await {
                        Ok(Some((user_id, current_username))) => {
                            info!(user_id = msg.user_id, osu_id = user_id, username = %current_username, "{}", log_fmt!("main.profile_card_self"));
                            user_id
                        }
                        Ok(None) => {
                            if let Some((uid, uname)) = ctx.resolve_binding(msg.user_id).await {
                                info!(user_id = msg.user_id, osu_id = uid, username = %uname, "{}", log_fmt!("main.profile_card_self_bound"));
                                uid
                            } else {
                                let _ = resp_tx
                                    .send(
                                        user_str("query.profile_not_bound")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                                return;
                            }
                        }
                        Err(_) => {
                            error!(
                                user_id = msg.user_id,
                                "{}",
                                log_fmt!("main.profile_card_db_error")
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("error.db_error")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                            return;
                        }
                    },
                },
            };

            info!(user_id = target_user_id, qq = ?qq, "{}", log_fmt!("main.profile_card_command"));
            let qq = msg.user_id;

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
                        ApiError::NotFound => {
                            user_str("error.not_found").replace("{qq}", &qq.to_string())
                        }
                        ApiError::MissingApiKey => {
                            user_str("error.api_key").replace("{qq}", &qq.to_string())
                        }
                        ApiError::OAuthError => {
                            user_str("error.oauth").replace("{qq}", &qq.to_string())
                        }
                        ApiError::RateLimitedWithRetryAfter(Some(secs)) => {
                            user_str("error.rate_limit")
                                .replace("{qq}", &qq.to_string())
                                .replace("{secs}", &secs.to_string())
                        }
                        ApiError::RateLimitedWithRetryAfter(None) => {
                            user_str("error.rate_limit_generic")
                                .replace("{qq}", &qq.to_string())
                        }
                        ApiError::ClientRateLimited => user_str("error.client_rate_limit")
                            .replace("{qq}", &qq.to_string()),
                        _ => user_str("error.query_failed").replace("{qq}", &qq.to_string()),
                    })?;
                    info!(
                        user_id = dedup_target_id,
                        html_len = profile.html.len(),
                        hue = profile.profile_hue,
                        "{}",
                        log_fmt!("main.profile_card_html_fetched")
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
                            warn!(user_id = target_user_id, "{}", log_fmt!("main.profile_card_render_timeout"));
                            user_str("error.render_timeout").replace("{qq}", &qq.to_string())
                        })?
                        .map(Arc::new)
                        .map_err(|e| {
                            warn!(user_id = target_user_id, error = %e, "{}", log_fmt!("main.profile_card_render_failed"));
                            user_str("error.render_failed").replace("{qq}", &qq.to_string())
                        })
                })
                .await;

            match render_result {
                Ok(jpeg_bytes) => {
                    info!(
                        user_id = target_user_id,
                        jpeg_len = jpeg_bytes.len(),
                        "{}",
                        log_fmt!("main.profile_card_rendered")
                    );
                    let write = ctx.write.clone();
                    let group_id = msg.group_id;
                    let resp_tx = resp_tx.clone();

                    tokio::spawn(async move {
                        if send_group_msg_with_image(&write, group_id, &jpeg_bytes)
                            .await
                            .is_err()
                        {
                            let _ = resp_tx
                                .send(
                                    user_str("error.image_send_failed")
                                        .replace("{qq}", &qq.to_string()),
                                )
                                .await;
                        }
                    });
                }
                Err(msg) => {
                    warn!(user_id = target_user_id, msg = %msg, "{}", log_fmt!("main.profile_card_failed"));
                    let _ = resp_tx.send(msg).await;
                }
            }
        }
        Command::ScoreOnBeatmap { .. } => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "{}",
                log_fmt!("main.score_on_beatmap_cmd")
            );
            handle_beatmap_score_query(&ctx, &msg, &resp_tx, &cmd, mode).await;
        }
        Command::Pass {
            mode: _,
            username,
            qq,
            beatmap_id,
            score_id,
            limit,
            limit_end,
            is_summary,
            filters,
        } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, limit = limit, "{}", log_fmt!("main.pass_command"));
            handle_score_query(
                &ctx,
                &msg,
                &resp_tx,
                ScoreQueryParams {
                    username: &username,
                    qq: &qq,
                    is_pass: true,
                    beatmap_id,
                    score_id,
                    limit,
                    is_single: !is_summary,
                    limit_end,
                    filters: filters.as_deref(),
                },
                mode,
            )
            .await;
        }
        Command::Recent {
            mode: _,
            username,
            qq,
            beatmap_id,
            score_id,
            limit,
            limit_end,
            is_summary,
            filters,
        } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, limit = limit, "{}", log_fmt!("main.recent_command"));
            handle_score_query(
                &ctx,
                &msg,
                &resp_tx,
                ScoreQueryParams {
                    username: &username,
                    qq: &qq,
                    is_pass: false,
                    beatmap_id,
                    score_id,
                    limit,
                    is_single: !is_summary,
                    limit_end,
                    filters: filters.as_deref(),
                },
                mode,
            )
            .await;
        }
        Command::BeatmapPreview {
            score_id,
            beatmap_id,
            mode,
            mods,
            gif,
            times,
        } => {
            let qq = msg.user_id;
            let group_id = msg.group_id;

            // 1. Resolve beatmap_id
            let resolved_bid_i64: i64 = match (score_id, beatmap_id) {
                (None, Some(bid)) => bid as i64,
                (Some(sid), None) => {
                    let dedup_rate_limiter = ctx.rate_limiter.clone();
                    let dedup_oauth = ctx.oauth.clone();
                    let qq_for_dedup = qq;
                    let sid_owned = sid;
                    let result = score_by_id_dedup()
                        .run_or_wait((sid_owned as i64, GameMode::Osu), move || {
                            let rl = dedup_rate_limiter.clone();
                            let oauth = dedup_oauth.clone();
                            let qq_inner = qq_for_dedup;
                            async move {
                                api::get_score_by_id(&rl, &oauth, sid_owned)
                                    .await
                                    .map_err(|e| match e {
                                        ApiError::NotFound => user_str("query.score_not_found")
                                            .replace("{qq}", &qq_inner.to_string()),
                                        _ => user_str("query.score_fetch_failed")
                                            .replace("{qq}", &qq_inner.to_string()),
                                    })
                            }
                        })
                        .await;
                    match result {
                        Ok(score) => score.beatmap_id,
                        Err(err_msg) => {
                            let _ = resp_tx.send(err_msg).await;
                            return;
                        }
                    }
                }
                (None, None) => match ctx.last_beatmap.get(group_id) {
                    Some(bid) => bid as i64,
                    None => {
                        send_error(&resp_tx, qq, "query.need_beatmap_or_cache").await;
                        return;
                    }
                },
                (Some(_), Some(_)) => {
                    send_error(&resp_tx, qq, "error.data_fetch_failed").await;
                    return;
                }
            };
            let resolved_bid = match u32::try_from(resolved_bid_i64) {
                Ok(b) => b,
                Err(_) => {
                    send_error(&resp_tx, qq, "error.data_fetch_failed").await;
                    return;
                }
            };
            ctx.last_beatmap.set(group_id, resolved_bid);

            // 2. Download .osu file
            let beatmap_path = match api::download_beatmap_osu(resolved_bid_i64).await {
                Ok(p) => p,
                Err(e) => {
                    let _ = resp_tx.send(api_error_msg(qq, &e)).await;
                    return;
                }
            };

            // 3. Parse beatmap (in spawn_blocking — CPU-bound)
            let parse_result = tokio::task::spawn_blocking({
                let path = beatmap_path.clone();
                move || -> std::result::Result<osubot_beatmap_preview::Beatmap, osubot_beatmap_preview::PreviewError> {
                    let meta = std::fs::metadata(&path)
                        .map_err(|e| osubot_beatmap_preview::PreviewError::new(
                            format!("read beatmap metadata: {e}")))?;
                    if meta.len() > 50 * 1024 * 1024 {
                        return Err(osubot_beatmap_preview::PreviewError::new(
                            "beatmap file too large (>50MB)"));
                    }
                    let bytes = std::fs::read(&path)
                        .map_err(|e| osubot_beatmap_preview::PreviewError::new(
                            format!("read beatmap file: {e}")))?;
                    osubot_beatmap_preview::parse_beatmap_from_bytes(&bytes)
                }
            })
            .await;

            let mut beatmap = match parse_result {
                Ok(Ok(b)) => b,
                Ok(Err(e)) => {
                    warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_parse_failed", error = &e.to_string()));
                    send_error(&resp_tx, qq, "error.data_fetch_failed").await;
                    return;
                }
                Err(_) => {
                    send_error(&resp_tx, qq, "error.render_failed").await;
                    return;
                }
            };

            // 4. Parse mods (join vec with "+" then call library's parse_mods)
            let mod_settings = match mods {
                Some(m) if !m.is_empty() => {
                    let joined = m.join("+");
                    match osubot_beatmap_preview::parse_mods(&joined) {
                        Ok(s) if s.has_any_mod() => Some(s),
                        Ok(_) => None,
                        Err(e) => {
                            warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_mods_parse_failed", error = &e.to_string()));
                            send_error(&resp_tx, qq, "error.data_fetch_failed").await;
                            return;
                        }
                    }
                }
                _ => None,
            };

            // 5. Compute target mode and validate mod compatibility.
            // `target_mode` is needed for mode-aware mod checks (DA/EZ/HR in
            // osu!, IN/HO in mania) and is also used to pick the renderer.
            let target_mode = mode.map(|m| m as i32).unwrap_or_else(|| beatmap.mode());
            if let Some(ref s) = mod_settings {
                let validation_errors = osubot_beatmap_preview::validate_mods(s, Some(target_mode));
                if let Some(first) = validation_errors.first() {
                    warn!(error = %first, "{}", log_fmt!("main.beatmap_preview_mods_invalid", error = &first));
                    let msg =
                        user_str("error.beatmap_preview_mods_invalid").replace("{error}", first);
                    let _ = resp_tx.send(msg).await;
                    return;
                }
            }

            // 6. Convert mode if user explicitly requested one different from beatmap mode
            if target_mode != beatmap.mode() {
                if beatmap.mode() != 0 {
                    warn!(
                        source_mode = beatmap.mode(),
                        target_mode = target_mode,
                        "{}",
                        log_fmt!(
                            "main.beatmap_preview_convert_unsupported",
                            source_mode = beatmap.mode(),
                            target_mode = target_mode
                        )
                    );
                    send_error(&resp_tx, qq, "error.beatmap_preview_convert_unsupported").await;
                    return;
                }
                let mods_for_conv = mod_settings.clone();
                let convert_result = tokio::task::spawn_blocking(move || {
                    osubot_beatmap_preview::convert_beatmap(
                        &beatmap,
                        target_mode,
                        mods_for_conv.as_ref(),
                    )
                })
                .await;
                beatmap = match convert_result {
                    Ok(Ok(b)) => b,
                    Ok(Err(e)) => {
                        warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_convert_failed", error = &e.to_string()));
                        send_error(&resp_tx, qq, "error.data_fetch_failed").await;
                        return;
                    }
                    Err(_) => {
                        send_error(&resp_tx, qq, "error.render_failed").await;
                        return;
                    }
                };
            }

            // 7. Determine output format and path
            let use_gif = gif || target_mode == 0;
            let fmt = if use_gif { "gif" } else { "png" };
            let mod_suffix = match &mod_settings {
                Some(s) if s.has_any_mod() => s
                    .tokens
                    .iter()
                    .map(|t| t.to_lowercase())
                    .collect::<Vec<_>>()
                    .join("+"),
                _ => String::new(),
            };
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let filename = if mod_suffix.is_empty() {
                format!("{}_{:x}.{}", resolved_bid, nanos, fmt)
            } else {
                format!("{}_{}_{:x}.{}", resolved_bid, mod_suffix, nanos, fmt)
            };
            let output_path = osubot_core::cache::preview_cache_dir().join(&filename);

            // 7.5. Compute times_ms from user-specified anchor/range
            let times_ms: Option<Vec<i64>> = match times {
                None => None,
                Some(t) if t.len() == 1 => {
                    let anchor = t[0];
                    let half_window = 30_000_i64;
                    let window_start = (anchor - half_window).max(0);
                    let window_end = (anchor + half_window).min(beatmap.end_time());
                    let window_end = window_end.max(window_start);
                    Some(generate_linear_samples(window_start, window_end, 4))
                }
                Some(t) if t.len() == 2 => {
                    let start = t[0].min(t[1]);
                    let end = t[0].max(t[1]).min(beatmap.end_time());
                    let end = end.max(start);
                    Some(generate_linear_samples(start, end, 4))
                }
                _ => None,
            };

            // 8. Render (spawn_blocking with timeout)
            let mode_for_render = target_mode;
            let output_path_for_render = output_path.clone();
            let mods_for_render = mod_settings.clone();
            let use_gif_for_render = use_gif;
            let render_join = tokio::task::spawn_blocking(move || {
                render_beatmap_preview(
                    &beatmap,
                    mode_for_render,
                    mods_for_render.as_ref(),
                    &output_path_for_render,
                    use_gif_for_render,
                    times_ms,
                )
            });
            let render_timeout =
                Duration::from_secs(ctx.config.read().await.bot.render_timeout_secs);
            let timed = tokio::time::timeout(render_timeout, render_join).await;

            match timed {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_render_failed", error = &e.to_string()));
                    send_error(&resp_tx, qq, "error.render_failed").await;
                    return;
                }
                Ok(Err(_)) => {
                    send_error(&resp_tx, qq, "error.render_failed").await;
                    return;
                }
                Err(_) => {
                    warn!("{}", log_fmt!("main.beatmap_preview_render_timeout"));
                    send_error(&resp_tx, qq, "error.render_timeout").await;
                    return;
                }
            }

            // 9. Read rendered file and send
            let image_data = match tokio::fs::read(&output_path).await {
                Ok(d) => d,
                Err(e) => {
                    warn!(error = %e, path = ?output_path, "{}", log_fmt!("main.beatmap_preview_read_failed", error = &e.to_string()));
                    send_error(&resp_tx, qq, "error.render_failed").await;
                    return;
                }
            };

            let write = ctx.write.clone();
            if let Err(e) = send_group_msg_with_image(&write, group_id, &image_data).await {
                warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_send_failed", error = &e.to_string()));
            }
        }
        Command::Help => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "{}",
                log_fmt!("main.help_command")
            );
            let _ = resp_tx
                .send(user_str("sys.help").replace("{qq}", &msg.user_id.to_string()))
                .await;
        }
    }
}

use tokio_tungstenite::tungstenite::{Error as WsError, Message as WsMsg};
/// Send a text message to a QQ group via the OneBot WebSocket connection.
pub(crate) async fn send_group_msg(write: &Arc<Mutex<WriteSink>>, group_id: i64, message: &str) {
    let json = serde_json::json!({
        "action": "send_group_msg",
        "params": {
            "group_id": group_id,
            "message": message
        }
    });
    let mut sink = write.lock().await;
    if let Err(e) = sink.send(WsMsg::Text(json.to_string().into())).await {
        tracing::error!("{}", log_fmt!("main.send_group_msg_failed", error = &e));
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
            warn!(error = %e, group_id = group_id, "{}", log_fmt!("main.send_image_failed"));
        })
}

/// Render beatmap preview to file. Returns Ok(()) on success.
fn render_beatmap_preview(
    beatmap: &osubot_beatmap_preview::Beatmap,
    target_mode: i32,
    mods: Option<&osubot_beatmap_preview::ModSettings>,
    output_path: &std::path::Path,
    use_gif: bool,
    times_ms: Option<Vec<i64>>,
) -> std::result::Result<(), osubot_beatmap_preview::PreviewError> {
    let fmt = if use_gif { "gif" } else { "png" };

    std::fs::create_dir_all(
        output_path
            .parent()
            .expect("preview output path must have a parent dir"),
    )
    .map_err(|e| {
        osubot_beatmap_preview::PreviewError::new(format!("[{fmt}] create output dir: {e}"))
    })?;

    let result = match target_mode {
        0 => osubot_beatmap_preview::render_standard_gif(beatmap, mods, times_ms, output_path),
        1 if use_gif => {
            osubot_beatmap_preview::render_taiko_gif(beatmap, mods, times_ms.clone(), output_path)
        }
        1 => osubot_beatmap_preview::render_taiko_grid(beatmap, output_path, mods).map(|_| ()),
        2 if use_gif => {
            osubot_beatmap_preview::render_catch_gif(beatmap, mods, times_ms.clone(), output_path)
        }
        2 => osubot_beatmap_preview::render_catch_grid(beatmap, output_path, mods).map(|_| ()),
        3 if use_gif => {
            osubot_beatmap_preview::render_mania_gif(beatmap, mods, times_ms.clone(), output_path)
        }
        3 => osubot_beatmap_preview::render_mania_grid(beatmap, output_path, mods).map(|_| ()),
        _ => Err(osubot_beatmap_preview::PreviewError::new(format!(
            "unsupported mode: {target_mode}"
        ))),
    };
    result.map_err(|e| osubot_beatmap_preview::PreviewError::new(format!("[{fmt}] {e}")))
}

/// Generate `n` linearly-spaced sampling points in `[start, end]`.
fn generate_linear_samples(start: i64, end: i64, n: usize) -> Vec<i64> {
    if n <= 1 || start >= end {
        return vec![start];
    }
    let step = (end - start) / (n - 1) as i64;
    (0..n).map(|i| start + step * i as i64).collect()
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
        Ok(Err(_)) => Err(user_str("error.request_cancelled").to_string()),
        Err(_) => Err(user_str("error.request_timeout").to_string()),
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

    let data = value.as_array().ok_or(user_str("error.invalid_response"))?;

    let mut members = HashSet::new();
    for member in data {
        if let Some(user_id) = member.get("user_id").and_then(|v| v.as_i64()) {
            members.insert(user_id);
        }
    }
    Ok(members)
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
    background::spawn_scheduler(&handles);
    background::spawn_irc(&handles);
    background::spawn_onebot_cleanup(&handles);
    background::spawn_watcher(&handles).await;
    background::spawn_shutdown_signal(handles.app_state.shutdown.clone());

    // Extract state + drain/in_flight before handles is consumed by spawn_irc_bridge.
    // AppState derives Clone (all fields are Arc), so this is cheap.
    let state = handles.app_state.clone();
    let drain = handles.reload_handle.drain.clone();
    let in_flight = handles.reload_handle.in_flight.clone();

    background::spawn_irc_bridge(handles);

    ws_loop::run_ws_reconnect_loop(state.clone(), drain, in_flight).await;

    plugin_runtime::shutdown_all(&state.plugin_manager).await;
    state.scheduler.shutdown();
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use osubot_core::types::{ScoreStatistics, ScoreUser};
    use rosu_mods::GameMods;

    fn make_score(mods: GameMods) -> Score {
        Score {
            score_id: 1,
            beatmap_id: 1,
            beatmapset_id: 1,
            artist: String::new(),
            title: String::new(),
            version: String::new(),
            creator: String::new(),
            star_rating: 0.0,
            bpm: 0.0,
            ar: 0.0,
            od: 0.0,
            cs: 0.0,
            hp: 0.0,
            length_seconds: 0,
            score_value: 0,
            accuracy: 1.0,
            max_combo: 0,
            beatmap_max_combo: 0,
            pp: None,
            pp_breakdown: None,
            pp_if_acc: None,
            perfect_pp: None,
            rank: String::new(),
            passed: true,
            mods,
            is_perfect: false,
            created_at: String::new(),
            is_lazer: false,
            has_replay: false,
            legacy_score_id: None,
            statistics: ScoreStatistics {
                count_geki: 0,
                count_300: 0,
                count_katu: 0,
                count_100: 0,
                count_50: 0,
                count_miss: 0,
                osu_large_tick_hits: 0,
                osu_small_tick_hits: 0,
                osu_slider_tail_hits: 0,
                osu_large_tick_misses: 0,
                osu_small_tick_misses: 0,
            },
            cover_url: String::new(),
            user: ScoreUser {
                avatar_url: String::new(),
                country_code: String::new(),
                user_id: None,
                username: None,
                global_rank: None,
                country_rank: None,
                pp: 0.0,
            },
            fav_count: None,
            play_count: None,
            status: String::new(),
        }
    }

    fn mods_with(acronyms: &[&str]) -> GameMods {
        let mut mods = GameMods::new();
        for a in acronyms {
            let m = rosu_mods::GameMod::new(*a, rosu_mods::GameMode::Osu);
            mods.insert(m);
        }
        mods
    }

    #[test]
    fn parse_token_eq() {
        let r = parse_filter_token("miss=0");
        assert_eq!(r, Some(("miss".to_string(), FilterOp::Eq, "0".to_string())));
    }

    #[test]
    fn parse_token_eqeq() {
        let r = parse_filter_token("miss==0");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::EqEq, "0".to_string()))
        );
    }

    #[test]
    fn parse_token_noteq() {
        let r = parse_filter_token("miss!=0");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::NotEq, "0".to_string()))
        );
    }

    #[test]
    fn parse_token_gt() {
        let r = parse_filter_token("miss>0");
        assert_eq!(r, Some(("miss".to_string(), FilterOp::Gt, "0".to_string())));
    }

    #[test]
    fn parse_token_lt() {
        let r = parse_filter_token("miss<10");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::Lt, "10".to_string()))
        );
    }

    #[test]
    fn parse_token_gteq() {
        // >= must be matched before > alone
        let r = parse_filter_token("miss>=5");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::GtEq, "5".to_string()))
        );
    }

    #[test]
    fn parse_token_lteq() {
        let r = parse_filter_token("miss<=10");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::LtEq, "10".to_string()))
        );
    }

    #[test]
    fn parse_token_negative_value() {
        let r = parse_filter_token("miss>-5");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::Gt, "-5".to_string()))
        );
    }

    #[test]
    fn parse_token_mod() {
        let r = parse_filter_token("mod=HDDT");
        assert_eq!(
            r,
            Some(("mod".to_string(), FilterOp::Eq, "HDDT".to_string()))
        );
    }

    #[test]
    fn parse_token_mod_eqeq() {
        let r = parse_filter_token("mod==DT");
        assert_eq!(
            r,
            Some(("mod".to_string(), FilterOp::EqEq, "DT".to_string()))
        );
    }

    #[test]
    fn parse_token_mod_noteq() {
        let r = parse_filter_token("mod!=DT");
        assert_eq!(
            r,
            Some(("mod".to_string(), FilterOp::NotEq, "DT".to_string()))
        );
    }

    #[test]
    fn parse_token_empty_value_rejected() {
        assert_eq!(parse_filter_token("miss="), None);
        assert_eq!(parse_filter_token("miss>="), None);
        assert_eq!(parse_filter_token("miss=="), None);
    }

    #[test]
    fn parse_token_empty_key_rejected() {
        assert_eq!(parse_filter_token("=0"), None);
        assert_eq!(parse_filter_token(">0"), None);
        assert_eq!(parse_filter_token("==0"), None);
    }

    #[test]
    fn parse_token_no_operator_rejected() {
        assert_eq!(parse_filter_token("miss0"), None);
        assert_eq!(parse_filter_token(""), None);
    }

    // === Integer key × 6 operators ===

    fn score_with_miss(miss: i64) -> Score {
        let mut s = make_score(GameMods::new());
        s.statistics.count_miss = miss;
        s
    }

    fn score_with_combo(combo: i64) -> Score {
        let mut s = make_score(GameMods::new());
        s.max_combo = combo;
        s
    }

    fn score_with_pp(pp: f64) -> Score {
        let mut s = make_score(GameMods::new());
        s.pp = Some(pp);
        s
    }

    fn score_with_acc(acc: f64) -> Score {
        let mut s = make_score(GameMods::new());
        s.accuracy = acc;
        s
    }

    fn score_with_score_value(v: i64) -> Score {
        let mut s = make_score(GameMods::new());
        s.score_value = v;
        s
    }

    #[test]
    fn miss_eq() {
        let s = score_with_miss(5);
        assert!(score_matches_filters(&s, &["miss=5".to_string()]));
        assert!(score_matches_filters(&s, &["miss==5".to_string()]));
        assert!(!score_matches_filters(&s, &["miss=4".to_string()]));
    }

    #[test]
    fn miss_noteq() {
        let s = score_with_miss(5);
        assert!(score_matches_filters(&s, &["miss!=4".to_string()]));
        assert!(!score_matches_filters(&s, &["miss!=5".to_string()]));
    }

    #[test]
    fn miss_ordering() {
        let s = score_with_miss(5);
        assert!(score_matches_filters(&s, &["miss>4".to_string()]));
        assert!(score_matches_filters(&s, &["miss>=5".to_string()]));
        assert!(!score_matches_filters(&s, &["miss>5".to_string()]));
        assert!(score_matches_filters(&s, &["miss<6".to_string()]));
        assert!(score_matches_filters(&s, &["miss<=5".to_string()]));
        assert!(!score_matches_filters(&s, &["miss<5".to_string()]));
    }

    #[test]
    fn combo_eq() {
        let s = score_with_combo(500);
        assert!(score_matches_filters(&s, &["combo=500".to_string()]));
        assert!(!score_matches_filters(&s, &["combo=501".to_string()]));
    }

    #[test]
    fn combo_noteq() {
        let s = score_with_combo(500);
        assert!(score_matches_filters(&s, &["combo!=501".to_string()]));
    }

    #[test]
    fn combo_ordering() {
        let s = score_with_combo(500);
        assert!(score_matches_filters(&s, &["combo>499".to_string()]));
        assert!(score_matches_filters(&s, &["combo>=500".to_string()]));
        assert!(score_matches_filters(&s, &["combo<501".to_string()]));
        assert!(score_matches_filters(&s, &["combo<=500".to_string()]));
    }

    #[test]
    fn score_value_eq() {
        let s = score_with_score_value(1_000_000);
        assert!(score_matches_filters(&s, &["score=1000000".to_string()]));
        assert!(!score_matches_filters(&s, &["score=999999".to_string()]));
    }

    #[test]
    fn score_value_ordering() {
        let s = score_with_score_value(1_000_000);
        assert!(score_matches_filters(&s, &["score>999999".to_string()]));
        assert!(!score_matches_filters(&s, &["score<999999".to_string()]));
    }

    // === Float key (pp) — tolerance 0.5 ===

    #[test]
    fn pp_eq_tolerance() {
        let s = score_with_pp(500.4);
        // |500.4 - 500| = 0.4 < 0.5
        assert!(score_matches_filters(&s, &["pp=500".to_string()]));
        assert!(score_matches_filters(&s, &["pp==500".to_string()]));
        let s = score_with_pp(500.6);
        // |500.6 - 500| = 0.6 >= 0.5
        assert!(!score_matches_filters(&s, &["pp=500".to_string()]));
    }

    #[test]
    fn pp_noteq_tolerance() {
        let s = score_with_pp(500.6);
        assert!(score_matches_filters(&s, &["pp!=500".to_string()]));
        let s = score_with_pp(500.4);
        assert!(!score_matches_filters(&s, &["pp!=500".to_string()]));
    }

    #[test]
    fn pp_ordering_strict() {
        let s = score_with_pp(500.0);
        // Strict: 500.0 is NOT > 500.0
        assert!(!score_matches_filters(&s, &["pp>500".to_string()]));
        assert!(score_matches_filters(&s, &["pp>=500".to_string()]));
        assert!(!score_matches_filters(&s, &["pp<500".to_string()]));
        assert!(score_matches_filters(&s, &["pp<=500".to_string()]));
    }

    #[test]
    fn pp_ordering_tolerance_does_not_apply() {
        // 499.6 is within == tolerance of 500 (|499.6-500|=0.4 < 0.5),
        // so it matches `pp=500`. But strict ordering must reject
        // `pp>500`: 499.6 is not strictly > 500. If `>` were degraded
        // to `a > b - tol = a > 499.5`, the assertion would fail.
        let s = score_with_pp(499.6);
        assert!(score_matches_filters(&s, &["pp=500".to_string()]));
        assert!(!score_matches_filters(&s, &["pp>500".to_string()]));

        // Symmetric case: 500.4 is within == tolerance of 500 and
        // strictly < 501. Strict `<500` must reject it. If `<` were
        // degraded to `a < b + tol = a < 500.5`, the assertion would fail.
        let s2 = score_with_pp(500.4);
        assert!(score_matches_filters(&s2, &["pp=500".to_string()]));
        assert!(!score_matches_filters(&s2, &["pp<500".to_string()]));
    }

    // === Float key (acc) ===

    #[test]
    fn acc_eq_tolerance() {
        // accuracy is stored as fraction; 95.5% → 0.955
        let s = score_with_acc(0.954);
        // 0.954 * 100 = 95.4, |95.4 - 95.5| = 0.1 < 0.5
        assert!(score_matches_filters(&s, &["acc=95.5".to_string()]));
        let s = score_with_acc(0.946);
        // 94.6, |94.6 - 95.5| = 0.9 >= 0.5
        assert!(!score_matches_filters(&s, &["acc=95.5".to_string()]));
    }

    #[test]
    fn acc_alias_accuracy() {
        let s = score_with_acc(0.955);
        assert!(score_matches_filters(&s, &["accuracy=95.5".to_string()]));
    }

    #[test]
    fn acc_ordering_strict() {
        let s = score_with_acc(0.95);
        assert!(score_matches_filters(&s, &["acc>90".to_string()]));
        assert!(score_matches_filters(&s, &["acc>=95".to_string()]));
        assert!(!score_matches_filters(&s, &["acc>95".to_string()]));
    }

    #[test]
    fn mod_filter_matches_single_mod() {
        let score = make_score(mods_with(&["HD"]));
        assert!(score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_does_not_match_missing_mod() {
        let score = make_score(mods_with(&["DT"]));
        assert!(!score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_subset_match() {
        // score has HDDT; mod=HD should still match (subset)
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_combined_concat() {
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&score, &["mod=HDDT".to_string()]));
    }

    #[test]
    fn mod_filter_combined_concat_no_match() {
        // score has only HD; mod=HDDT requires DT too
        let score = make_score(mods_with(&["HD"]));
        assert!(!score_matches_filters(&score, &["mod=HDDT".to_string()]));
    }

    #[test]
    fn mod_filter_comma_separated() {
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&score, &["mod=HD,DT".to_string()]));
    }

    #[test]
    fn mod_filter_no_mods_score() {
        let score = make_score(GameMods::new());
        assert!(!score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_combines_with_other_keys() {
        // AND semantics: mod=HD AND miss=0
        let score = make_score(mods_with(&["HD"]));
        assert!(score_matches_filters(
            &score,
            &["mod=HD".to_string(), "miss=0".to_string()]
        ));
    }

    #[test]
    fn mod_filter_odd_length_fails_match() {
        // mod=HDT (odd length) → entry cannot be parsed → false
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(!score_matches_filters(&score, &["mod=HDT".to_string()]));
    }

    #[test]
    fn mod_filter_empty_value_no_op() {
        // mod= with no mods → no required mods → passes
        let score = make_score(GameMods::new());
        assert!(score_matches_filters(&score, &["mod=".to_string()]));
    }

    #[test]
    fn mod_eqeq_exact_match() {
        // 纯 DT 分数匹配 mod==DT
        let s = make_score(mods_with(&["DT"]));
        assert!(score_matches_filters(&s, &["mod==DT".to_string()]));
    }

    #[test]
    fn mod_eqeq_does_not_match_superset() {
        // HDDT 不匹配 mod==DT
        let s = make_score(mods_with(&["HD", "DT"]));
        assert!(!score_matches_filters(&s, &["mod==DT".to_string()]));
    }

    #[test]
    fn mod_eqeq_empty_required() {
        // mod==NM: required set is empty → exact match means score has no mods
        // (and no extras). Spec README example: !ps mod==NM.

        // No mods → matches mod==NM
        let s = make_score(GameMods::new());
        assert!(score_matches_filters(&s, &["mod==NM".to_string()]));

        // Has any mod → does NOT match mod==NM
        let s = make_score(mods_with(&["HD"]));
        assert!(!score_matches_filters(&s, &["mod==NM".to_string()]));
    }

    #[test]
    fn mod_eq_subset_still_works() {
        // mod=DT (单 =) 是子集匹配
        let s = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&s, &["mod=DT".to_string()]));
    }

    #[test]
    fn mod_noteq_negation_of_subset() {
        // 纯 HD 不包含 DT → 匹配 mod!=DT
        let s = make_score(mods_with(&["HD"]));
        assert!(score_matches_filters(&s, &["mod!=DT".to_string()]));

        // 纯 DT 包含 DT → 不匹配 mod!=DT
        let s = make_score(mods_with(&["DT"]));
        assert!(!score_matches_filters(&s, &["mod!=DT".to_string()]));

        // HDDT 包含 DT → 不匹配 mod!=DT
        let s = make_score(mods_with(&["HD", "DT"]));
        assert!(!score_matches_filters(&s, &["mod!=DT".to_string()]));
    }

    #[test]
    fn mod_comparison_ops_silently_pass() {
        // mod>DT 等在 mod 键上无意义 → 静默忽略
        let s = make_score(mods_with(&["HD"]));
        for op_filter in &["mod>DT", "mod<DT", "mod>=DT", "mod<=DT"] {
            assert!(
                score_matches_filters(&s, &[op_filter.to_string()]),
                "{op_filter} should silently pass"
            );
        }
    }

    #[test]
    fn cmp_i64_eq() {
        assert!(cmp_i64(5, 5, FilterOp::Eq));
        assert!(!cmp_i64(5, 6, FilterOp::Eq));
    }

    #[test]
    fn cmp_i64_noteq() {
        assert!(cmp_i64(5, 6, FilterOp::NotEq));
        assert!(!cmp_i64(5, 5, FilterOp::NotEq));
    }

    #[test]
    fn cmp_i64_ordering() {
        assert!(cmp_i64(6, 5, FilterOp::Gt));
        assert!(!cmp_i64(5, 5, FilterOp::Gt));
        assert!(cmp_i64(5, 5, FilterOp::GtEq));
        assert!(cmp_i64(5, 6, FilterOp::Lt));
        assert!(cmp_i64(5, 5, FilterOp::LtEq));
        assert!(!cmp_i64(5, 5, FilterOp::Lt));
    }

    #[test]
    fn cmp_i64_negative() {
        assert!(cmp_i64(-3, -5, FilterOp::Gt));
        assert!(cmp_i64(-5, -5, FilterOp::Eq));
    }

    #[test]
    fn cmp_f64_eq_uses_tolerance() {
        assert!(cmp_f64(500.4, 500.0, FilterOp::Eq, 0.5));
        assert!(!cmp_f64(500.6, 500.0, FilterOp::Eq, 0.5));
        assert!(cmp_f64(500.0, 500.0, FilterOp::Eq, 0.5));
    }

    #[test]
    fn cmp_f64_noteq_uses_tolerance() {
        assert!(cmp_f64(500.6, 500.0, FilterOp::NotEq, 0.5));
        assert!(!cmp_f64(500.4, 500.0, FilterOp::NotEq, 0.5));
    }

    #[test]
    fn cmp_f64_ordering_strict() {
        // > and >= use strict comparison (no tolerance)
        assert!(cmp_f64(500.6, 500.0, FilterOp::Gt, 0.5));
        assert!(cmp_f64(500.4, 500.0, FilterOp::Gt, 0.5));
        assert!(cmp_f64(500.0, 500.0, FilterOp::GtEq, 0.5));
        assert!(cmp_f64(499.4, 500.0, FilterOp::Lt, 0.5));
        assert!(cmp_f64(499.6, 500.0, FilterOp::Lt, 0.5));
        assert!(cmp_f64(500.0, 500.0, FilterOp::LtEq, 0.5));
    }
}
