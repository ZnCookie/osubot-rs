use crate::rate_limiter::RateLimiter;
use crate::types::{GameMode, Score, ScoreStatistics, ScoreUser, UserStats};
use futures::stream::{self, StreamExt};
use osubot_types::{to_rosu_game_mode, PpBreakdown, PpIfAcc};
use reqwest::Client;
use rosu_mods::GameMods;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;

pub(crate) const API_VERSION: &str = "20260408";

pub(crate) fn http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client")
    })
}

fn beatmap_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("osubot")
        .join("beatmaps")
}

pub async fn download_beatmap_osu(beatmap_id: i64) -> Result<PathBuf, ApiError> {
    let cache_path = beatmap_cache_dir().join(format!("{}.osu", beatmap_id));

    // 检查缓存（同步 I/O 移到 blocking 线程）
    let cache_valid = tokio::task::spawn_blocking({
        let cache_path = cache_path.clone();
        move || -> bool {
            if !cache_path.exists() {
                return false;
            }
            match std::fs::metadata(&cache_path) {
                Ok(meta) if meta.len() > 0 => {
                    if let Ok(modified) = meta.modified() {
                        modified.elapsed().unwrap_or(std::time::Duration::MAX)
                            < std::time::Duration::from_secs(7 * 86400)
                    } else {
                        false
                    }
                }
                _ => false,
            }
        }
    })
    .await
    .unwrap_or(false);

    if cache_valid {
        return Ok(cache_path);
    }

    let client = http_client();
    let url = format!("https://osu.ppy.sh/osu/{}", beatmap_id);
    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        return Err(ApiError::InvalidResponse);
    }

    let bytes = resp.bytes().await?;

    // 写入缓存（同步 I/O 移到 blocking 线程）
    let write_path = cache_path.clone();
    tokio::task::spawn_blocking(move || {
        let _ = std::fs::create_dir_all(write_path.parent().unwrap());
        std::fs::write(&write_path, &bytes)
    })
    .await
    .map_err(|_| ApiError::InvalidResponse)?
    .map_err(|_| ApiError::InvalidResponse)?;

    Ok(cache_path)
}

pub fn calculate_pp_breakdown(
    osu_path: &std::path::Path,
    _mode: GameMode,
    mods: &GameMods,
    accuracy: f64,
    max_combo: i64,
    miss_count: i64,
) -> Option<PpBreakdown> {
    use rosu_pp::any::PerformanceAttributes;
    use rosu_pp::{Beatmap, Difficulty, GameMods as PpMods, Performance};

    let game_mods = mods.clone();
    let pp_mods = PpMods::from(game_mods.clone());

    let map = match Beatmap::from_path(osu_path) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "Failed to parse .osu file");
            return None;
        }
    };

    let diff_attrs = Difficulty::new().mods(pp_mods.clone()).calculate(&map);

    let perf_attrs = Performance::new(diff_attrs)
        .mods(pp_mods)
        .combo(max_combo as u32)
        .accuracy(accuracy * 100.0)
        .misses(miss_count as u32)
        .calculate();

    match perf_attrs {
        PerformanceAttributes::Osu(attrs) => Some(PpBreakdown {
            aim: Some(attrs.pp_aim),
            speed: Some(attrs.pp_speed),
            accuracy: attrs.pp_acc,
            flashlight: Some(attrs.pp_flashlight),
            difficulty: None,
        }),
        PerformanceAttributes::Taiko(attrs) => Some(PpBreakdown {
            aim: None,
            speed: None,
            accuracy: attrs.pp_acc,
            flashlight: None,
            difficulty: Some(attrs.pp_difficulty),
        }),
        PerformanceAttributes::Mania(attrs) => Some(PpBreakdown {
            aim: None,
            speed: None,
            accuracy: 0.0,
            flashlight: None,
            difficulty: Some(attrs.pp_difficulty),
        }),
        _ => None,
    }
}

pub fn calculate_pp_if_acc(
    osu_path: &std::path::Path,
    _mode: GameMode,
    mods: &GameMods,
    accuracy: f64,
    max_combo: i64,
    beatmap_max_combo: i64,
    miss_count: i64,
) -> Option<PpIfAcc> {
    use rosu_pp::{Beatmap, Difficulty, GameMods as PpMods, Performance};

    let game_mods = mods.clone();
    let pp_mods = PpMods::from(game_mods.clone());
    let map = match Beatmap::from_path(osu_path) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "Failed to parse .osu file for if-acc");
            return None;
        }
    };

    let diff_attrs = Difficulty::new().mods(pp_mods.clone()).calculate(&map);
    let calc_pp = |acc: f64, combo: u32, misses: u32| -> f64 {
        let perf = Performance::new(diff_attrs.clone())
            .mods(pp_mods.clone())
            .combo(combo)
            .accuracy(acc)
            .misses(misses)
            .calculate();
        perf.pp()
    };

    let combo = max_combo as u32;
    let bm_combo = beatmap_max_combo as u32;
    let misses = miss_count as u32;

    Some(PpIfAcc {
        acc_95: calc_pp(95.0, combo, misses),
        acc_97: calc_pp(97.0, combo, misses),
        acc_98: calc_pp(98.0, combo, misses),
        acc_99: calc_pp(99.0, combo, misses),
        acc_100: calc_pp(100.0, combo, misses),
        if_fc: calc_pp(accuracy, bm_combo, 0),
    })
}

/// Enrich a score with PP breakdown and if-acc values.
/// Downloads the .osu file if needed, calculates PP decomposition,
/// and sets the `pp_breakdown` and `pp_if_acc` fields on the score.
pub async fn enrich_score_with_pp(score: &mut Score, mode: GameMode) {
    if score.beatmap_id <= 0 {
        return;
    }

    let osu_path = match download_beatmap_osu(score.beatmap_id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = ?e,
                beatmap_id = score.beatmap_id,
                "Failed to download .osu for PP calculation"
            );
            return;
        }
    };

    let mods_clone = score.mods.clone();
    let accuracy = score.accuracy;
    let max_combo = score.max_combo;
    let beatmap_max_combo = score.beatmap_max_combo;
    let count_miss = score.statistics.count_miss;
    let is_catch = mode == GameMode::Catch;

    let (pp_breakdown, pp_if_acc) = tokio::task::spawn_blocking(move || {
        let breakdown = if !is_catch {
            calculate_pp_breakdown(
                &osu_path,
                mode,
                &mods_clone,
                accuracy,
                max_combo,
                count_miss,
            )
        } else {
            None
        };
        let if_acc = calculate_pp_if_acc(
            &osu_path,
            mode,
            &mods_clone,
            accuracy * 100.0,
            max_combo,
            beatmap_max_combo,
            count_miss,
        );
        (breakdown, if_acc)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = ?e, "PP calculation task panicked");
        (None, None)
    });

    score.pp_breakdown = pp_breakdown;
    score.pp_if_acc = pp_if_acc;

    if score.pp.is_none() {
        if let Some(ref bd) = score.pp_breakdown {
            let total = bd.aim.unwrap_or(0.0)
                + bd.speed.unwrap_or(0.0)
                + bd.accuracy
                + bd.flashlight.unwrap_or(0.0)
                + bd.difficulty.unwrap_or(0.0);
            if total > 0.0 {
                score.pp = Some(total);
            }
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct OsuUserInfo {
    pub id: i64,
    pub username: String,
    pub is_active: bool,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct OsuApiBeatmap {
    id: i64,
    #[serde(rename = "beatmapset_id")]
    beatmapset_id: i64,
    #[serde(default)]
    version: String,
    #[serde(default)]
    ar: f64,
    #[serde(default, alias = "accuracy")]
    od: f64,
    #[serde(default)]
    cs: f64,
    #[serde(default, alias = "drain")]
    hp: f64,
    #[serde(default)]
    bpm: f64,
    #[serde(default)]
    total_length: i64,
    #[serde(default)]
    difficulty_rating: f64,
    #[serde(default)]
    max_combo: i64,
    #[serde(default)]
    passcount: i64,
    #[serde(default)]
    playcount: i64,
    #[serde(default)]
    status: String,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiBeatmapset {
    #[serde(default)]
    artist: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    creator: String,
    #[serde(default)]
    covers: Option<serde_json::Value>,
    #[serde(default)]
    favourite_count: i64,
    #[serde(default)]
    play_count: i64,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum OsuApiMod {
    Object {
        acronym: String,
        #[serde(default)]
        settings: Option<serde_json::Value>,
    },
    String(String),
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScore {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    total_score: Option<i64>,
    #[serde(default)]
    legacy_total_score: Option<i64>,
    #[serde(default)]
    accuracy: f64,
    #[serde(default)]
    max_combo: i64,
    #[serde(default)]
    pp: Option<f64>,
    #[serde(default)]
    rank: String,
    #[serde(default)]
    perfect: bool,
    #[serde(default, alias = "created_at")]
    ended_at: String,
    #[serde(default)]
    is_lazer: bool,
    #[serde(default)]
    has_replay: bool,
    #[serde(default)]
    legacy_score_id: Option<i64>,
    beatmap: OsuApiBeatmap,
    beatmapset: OsuApiBeatmapset,
    #[serde(default)]
    mods: Vec<OsuApiMod>,
    #[serde(default)]
    statistics: OsuApiScoreStatistics,
    #[serde(default)]
    user: Option<serde_json::Value>,
}

/// lazer: great/ok/meh/miss, legacy: count_300/count_100/count_50/count_miss
#[derive(Debug, serde::Deserialize, Default)]
struct OsuApiScoreStatistics {
    #[serde(default, alias = "great")]
    count_300: i64,
    #[serde(default, alias = "ok")]
    count_100: i64,
    #[serde(default, alias = "meh")]
    count_50: i64,
    #[serde(default, alias = "miss")]
    count_miss: i64,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScoreUser {
    avatar_url: Option<String>,
    country_code: Option<String>,
    statistics: Option<OsuApiScoreUserStatistics>,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScoreUserStatistics {
    global_rank: Option<i64>,
    country_rank: Option<i64>,
    pp: Option<f64>,
}

/// Convert osu! API v2 mod objects into rosu_mods::GameMods with full settings.
fn api_mods_to_game_mods(api_mods: &[OsuApiMod], mode: GameMode) -> GameMods {
    use rosu_mods::serde::GameModSeed;
    use serde::de::DeserializeSeed;
    let ros_mode = to_rosu_game_mode(mode);
    let seed = GameModSeed::Mode {
        mode: ros_mode,
        deny_unknown_fields: false,
    };
    let mut mods = GameMods::new();
    for m in api_mods {
        let gamemod = match m {
            OsuApiMod::String(s) => {
                let json_str = format!("\"{s}\"");
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|_| rosu_mods::GameMod::new(s, ros_mode))
            }
            OsuApiMod::Object {
                acronym,
                settings: Some(settings),
            } => {
                let json = serde_json::json!({"acronym": acronym, "settings": settings});
                let json_str = json.to_string();
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|_| rosu_mods::GameMod::new(acronym, ros_mode))
            }
            OsuApiMod::Object {
                acronym,
                settings: None,
            } => {
                let json_str = serde_json::json!({"acronym": acronym}).to_string();
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|_| rosu_mods::GameMod::new(acronym, ros_mode))
            }
        };
        mods.insert(gamemod);
    }
    mods
}

/// Apply mod adjustments to base AR/OD/CS/HP values.
/// Returns the effective in-game values after mods (DT, HT, HR, EZ, etc).
/// No beatmap download needed — uses BeatmapAttributesBuilder with base stats.
pub fn apply_mod_adjustment_to_stats(
    ar: f64,
    od: f64,
    cs: f64,
    hp: f64,
    mods: &GameMods,
) -> (f64, f64, f64, f64) {
    if mods.is_empty() {
        return (ar, od, cs, hp);
    }
    use rosu_pp::model::beatmap::BeatmapAttributesBuilder;
    let adjusted = BeatmapAttributesBuilder::new()
        .ar(ar as f32, false)
        .od(od as f32, false)
        .cs(cs as f32, false)
        .hp(hp as f32, false)
        .mods(mods.clone())
        .build()
        .apply_clock_rate();
    (
        adjusted.ar,
        adjusted.od,
        adjusted.cs as f64,
        adjusted.hp as f64,
    )
}

fn api_score_to_score(api: OsuApiScore, mode: GameMode) -> Score {
    let beatmap = api.beatmap;
    let beatmapset = api.beatmapset;

    let cover_url = beatmapset
        .covers
        .as_ref()
        .and_then(|v| v.get("cover")?.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let score_value = if api.score > 0 {
        api.score
    } else {
        api.total_score.or(api.legacy_total_score).unwrap_or(0)
    };

    let user = api
        .user
        .and_then(|v| {
            let u: OsuApiScoreUser = match serde_json::from_value(v.clone()) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(error = %e, user_json = %v, "Failed to parse user from score response");
                    return None;
                }
            };
            Some(ScoreUser {
                avatar_url: u.avatar_url.unwrap_or_default(),
                country_code: u.country_code.unwrap_or_default(),
                global_rank: u.statistics.as_ref().and_then(|s| s.global_rank),
                country_rank: u.statistics.as_ref().and_then(|s| s.country_rank),
                pp: u.statistics.as_ref().and_then(|s| s.pp).unwrap_or(0.0),
            })
        })
        .unwrap_or(ScoreUser {
            avatar_url: String::new(),
            country_code: String::new(),
            global_rank: None,
            country_rank: None,
            pp: 0.0,
        });

    Score {
        score_id: api.id,
        beatmap_id: beatmap.id,
        beatmapset_id: beatmap.beatmapset_id,
        artist: beatmapset.artist,
        title: beatmapset.title,
        version: beatmap.version,
        creator: beatmapset.creator,
        star_rating: beatmap.difficulty_rating,
        bpm: beatmap.bpm,
        ar: beatmap.ar,
        od: beatmap.od,
        cs: beatmap.cs,
        hp: beatmap.hp,
        length_seconds: beatmap.total_length,
        score_value,
        accuracy: api.accuracy,
        max_combo: api.max_combo,
        beatmap_max_combo: beatmap.max_combo,
        pp: api.pp,
        pp_breakdown: None,
        pp_if_acc: None,
        rank: api.rank,
        mods: api_mods_to_game_mods(&api.mods, mode),
        is_perfect: api.perfect,
        created_at: api.ended_at,
        is_lazer: api.is_lazer,
        has_replay: api.has_replay,
        legacy_score_id: api.legacy_score_id,
        statistics: ScoreStatistics {
            count_300: api.statistics.count_300,
            count_100: api.statistics.count_100,
            count_50: api.statistics.count_50,
            count_miss: api.statistics.count_miss,
        },
        cover_url,
        user,
        fav_count: Some(beatmapset.favourite_count).filter(|&v| v > 0),
        play_count: Some(beatmapset.play_count).filter(|&v| v > 0),
        status: beatmap.status,
    }
}

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("User not found")]
    NotFound,
    #[error("Invalid API response")]
    InvalidResponse,
    #[error("API key missing")]
    MissingApiKey,
    #[error("OAuth token error")]
    OAuthError,
    #[error("Rate limited - too many requests")]
    RateLimited,
}

#[derive(Debug, serde::Deserialize)]
struct OauthResponse {
    access_token: String,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiV2User {
    id: i64,
    username: String,
    country_code: Option<String>, // e.g., "CN", "US", "JP"
    statistics: Option<OsuStatistics>,
}

#[derive(Debug, serde::Deserialize)]
struct OsuStatistics {
    pp: Option<f64>,
    #[serde(rename = "global_rank")]
    rank: Option<i64>,
    #[serde(rename = "country_rank")]
    country_rank: Option<i64>,
    #[serde(rename = "ranked_score")]
    ranked_score: Option<i64>,
    #[serde(rename = "hit_accuracy")]
    accuracy: Option<f64>,
    #[serde(rename = "play_count")]
    playcount: Option<i64>,
    #[serde(rename = "total_hits")]
    hits: Option<i64>,
    #[serde(rename = "play_time")]
    playtime: Option<i64>, // seconds in v2
}

pub struct OauthTokenCache {
    client_id: String,
    client_secret: String,
    cache: Mutex<Option<(String, Instant)>>,
    refresh_lock: Mutex<()>,
    refresh_interval: Duration,
}

impl OauthTokenCache {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
            refresh_interval: Duration::from_secs(20 * 3600),
        }
    }

    pub async fn invalidate(&self) {
        let _guard = self.refresh_lock.lock().await;
        let mut guard = self.cache.lock().await;
        *guard = None;
    }

    pub async fn get_token(&self) -> Result<String, ApiError> {
        // 第一次检查：缓存有效则直接返回
        {
            let guard = self.cache.lock().await;
            if let Some((ref token, fetched_at)) = *guard {
                if fetched_at.elapsed() < self.refresh_interval {
                    return Ok(token.clone());
                }
            }
        } // guard 在此释放

        // 获取刷新锁，防止并发刷新（thundering herd）
        let _refresh_guard = self.refresh_lock.lock().await;

        // 第二次检查：可能已被其他并发请求刷新
        {
            let guard = self.cache.lock().await;
            if let Some((ref token, fetched_at)) = *guard {
                if fetched_at.elapsed() < self.refresh_interval {
                    return Ok(token.clone());
                }
            }
        }

        // 缓存过期，发 HTTP 请求（不持有锁）
        let client = http_client();
        let params = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "client_credentials"),
            ("scope", "public"),
        ];
        let resp = client
            .post("https://osu.ppy.sh/oauth/token")
            .form(&params)
            .send()
            .await?;

        if resp.status() != 200 {
            return Err(ApiError::OAuthError);
        }

        let token_data: OauthResponse = resp.json().await?;

        // 重新获取锁，写入缓存
        {
            let mut guard = self.cache.lock().await;
            *guard = Some((token_data.access_token.clone(), Instant::now()));
        }

        Ok(token_data.access_token)
    }
}

/// Execute an async operation with retry on 401 (OAuth invalidation + exponential backoff).
pub(crate) async fn retry_on_401<F, Fut, T>(
    oauth: &OauthTokenCache,
    max_retries: u32,
    operation: F,
) -> Result<T, ApiError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    let base_delay = Duration::from_secs(1);
    for attempt in 0..=max_retries {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(ApiError::OAuthError) | Err(ApiError::RateLimited) if attempt < max_retries => {
                oauth.invalidate().await;
                tokio::time::sleep(base_delay * 2u32.pow(attempt)).await;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

async fn fetch_user_stats_internal(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    url: &str,
) -> Result<UserStats, ApiError> {
    let client = http_client();

    retry_on_401(oauth, 5, || async {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("x-api-version", API_VERSION)
            .send()
            .await?;

        if resp.status() == 404 {
            return Err(ApiError::NotFound);
        }

        if !resp.status().is_success() {
            return Err(ApiError::InvalidResponse);
        }

        let data: OsuApiV2User = resp.json().await?;

        let stats = match data.statistics {
            Some(s) => s,
            None => return Err(ApiError::NotFound),
        };

        Ok(UserStats {
            user_id: data.id,
            username: data.username,
            pp: stats.pp.unwrap_or(0.0),
            rank: stats.rank.unwrap_or(0),
            country_rank: stats.country_rank.unwrap_or(0),
            country_code: data.country_code.unwrap_or_else(|| "XX".to_string()),
            ranked_score: stats.ranked_score.unwrap_or(0),
            accuracy: stats.accuracy.unwrap_or(0.0),
            playcount: stats.playcount.unwrap_or(0),
            hits: stats.hits.unwrap_or(0),
            playtime: stats.playtime.unwrap_or(0),
            rank_change: None,
            country_rank_change: None,
        })
    })
    .await
}

/// 纯数字用户名加 @ 前缀，避免被当作 user_id 查询
fn url_encode_username(username: &str) -> String {
    if username.chars().all(|c| c.is_ascii_digit()) {
        format!("@{}", username)
    } else {
        username.to_string()
    }
}

/// 通过用户名获取用户详细统计（pp、排名、准确率等）
pub async fn fetch_user_stats_by_username(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    username: &str,
    mode: GameMode,
) -> Result<UserStats, ApiError> {
    let mode_param = mode.api_value();
    let url_username = url_encode_username(username);
    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/{}",
        url_username, mode_param
    );
    fetch_user_stats_internal(rate_limiter, oauth, &url).await
}

/// 通过用户 ID 获取用户详细统计
pub async fn fetch_user_stats_by_user_id(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<UserStats, ApiError> {
    let mode_param = mode.api_value();
    let url = format!("https://osu.ppy.sh/api/v2/users/{}/{}", user_id, mode_param);
    fetch_user_stats_internal(rate_limiter, oauth, &url).await
}

/// 获取用户最近游玩成绩列表（pass/recent），自动回填缺失数据
pub async fn get_user_recent(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    user_id: i64,
    mode: GameMode,
    include_fails: bool,
    limit: u32,
) -> Result<Vec<Score>, ApiError> {
    let client = http_client();

    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/scores/recent?mode={}&include_fails={}&limit={}&legacy_only=0",
        user_id,
        mode.api_value(),
        if include_fails { 1 } else { 0 },
        limit
    );

    retry_on_401(oauth, 5, || async {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("x-api-version", API_VERSION)
            .send()
            .await?;

        if resp.status() == 404 {
            return Err(ApiError::NotFound);
        }

        if !resp.status().is_success() {
            return Err(ApiError::InvalidResponse);
        }

        let raw_json: serde_json::Value = resp.json().await?;

        let plays: Vec<OsuApiScore> = serde_json::from_value(raw_json).map_err(|e| {
            tracing::error!(error = %e, "Failed to parse score JSON");
            ApiError::InvalidResponse
        })?;
        let scores_raw: Vec<Score> = plays
            .into_iter()
            .map(|p| api_score_to_score(p, mode))
            .collect();
        let mode_str = mode.api_value().to_string();

        let scores: Vec<Score> = stream::iter(scores_raw)
            .map(|mut score| {
                let rl = rate_limiter.clone();
                let oa = oauth.clone();
                let ruleset = mode_str.clone();
                let _score_mode = mode;
                async move {
                    // 回填谱面 od/hp/max_combo
                    if ((score.od == 0.0 && score.hp == 0.0) || score.beatmap_max_combo == 0)
                        && score.beatmap_id > 0
                    {
                        match fetch_beatmap(&rl, &oa, score.beatmap_id).await {
                            Ok(bm) => {
                                if score.od == 0.0 && score.hp == 0.0 {
                                    score.od = bm.od;
                                    score.hp = bm.hp;
                                }
                                if score.beatmap_max_combo == 0 {
                                    score.beatmap_max_combo = bm.max_combo;
                                }
                                tracing::debug!(
                                    beatmap_id = score.beatmap_id,
                                    od = bm.od,
                                    hp = bm.hp,
                                    max_combo = bm.max_combo,
                                    "Backfilled beatmap stats"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = ?e,
                                    beatmap_id = score.beatmap_id,
                                    "Failed to backfill beatmap stats"
                                );
                            }
                        }
                    }

                    // 回填 lazer 分数值
                    if score.score_value == 0 && score.score_id > 0 {
                        match fetch_score_detail(&rl, &oa, &ruleset, score.score_id).await {
                            Ok(Some(val)) => {
                                score.score_value = val;
                                tracing::debug!(
                                    score_id = score.score_id,
                                    score_value = val,
                                    "Backfilled score value from detail endpoint"
                                );
                            }
                            Ok(None) => {
                                tracing::debug!(
                                    score_id = score.score_id,
                                    "Score detail endpoint returned no value"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    error = ?e,
                                    score_id = score.score_id,
                                    "Failed to backfill score value"
                                );
                            }
                        }
                    }

                    score
                }
            })
            .buffer_unordered(5)
            .collect()
            .await;

        Ok(scores)
    })
    .await
}

/// 通过 score ID 获取单条成绩详情（用于 lazer 分数值回填）
async fn fetch_score_detail(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    ruleset: &str,
    score_id: i64,
) -> Result<Option<i64>, ApiError> {
    let client = http_client();
    let url = format!("https://osu.ppy.sh/api/v2/scores/{}/{}", ruleset, score_id);
    tracing::debug!(url = %url, "Fetching score detail");

    retry_on_401(oauth, 5, || async {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("x-api-version", API_VERSION)
            .send()
            .await?;

        if resp.status() == 404 {
            tracing::debug!(url = %url, status = %resp.status(), "Score detail endpoint 404");
            return Ok(None);
        }

        if !resp.status().is_success() {
            tracing::warn!(url = %url, status = %resp.status(), "Score detail endpoint error");
            return Err(ApiError::InvalidResponse);
        }

        let data: serde_json::Value = resp.json().await?;
        tracing::trace!(
            keys = ?data.as_object().map(|o| o.keys().collect::<Vec<_>>()),
            score = ?data.get("score"),
            total_score = ?data.get("total_score"),
            legacy_total_score = ?data.get("legacy_total_score"),
            classic_total_score = ?data.get("classic_total_score"),
            "Score detail endpoint response"
        );
        let score_val = data
            .get("total_score")
            .and_then(|v| v.as_i64())
            .or_else(|| data.get("score").and_then(|v| v.as_i64()))
            .or_else(|| data.get("legacy_total_score").and_then(|v| v.as_i64()))
            .filter(|&v| v > 0);
        Ok(score_val)
    })
    .await
}

/// 通过 beatmap ID 获取谱面信息（用于 od/hp/max_combo 回填）
async fn fetch_beatmap(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    beatmap_id: i64,
) -> Result<OsuApiBeatmap, ApiError> {
    let client = http_client();
    let url = format!("https://osu.ppy.sh/api/v2/beatmaps/{}", beatmap_id);

    retry_on_401(oauth, 5, || async {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("x-api-version", API_VERSION)
            .send()
            .await?;

        if resp.status() == 404 {
            return Err(ApiError::NotFound);
        }

        if !resp.status().is_success() {
            return Err(ApiError::InvalidResponse);
        }

        Ok(resp.json().await?)
    })
    .await
}

/// 通过用户名获取 osu! 用户基本信息
pub async fn get_user_info(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    username: &str,
) -> Result<Option<OsuUserInfo>, ApiError> {
    let access_token = oauth.get_token().await?;
    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::RateLimited)?;
    let client = http_client();

    let url_username = url_encode_username(username);
    let url = format!("https://osu.ppy.sh/api/v2/users/{}", url_username);

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("x-api-version", API_VERSION)
        .send()
        .await?;

    if resp.status() == 404 {
        return Ok(None);
    }

    if !resp.status().is_success() {
        return Err(ApiError::InvalidResponse);
    }

    let user: OsuUserInfo = resp.json().await?;
    Ok(Some(user))
}

#[derive(Debug, serde::Deserialize)]
struct OsuProfileResponse {
    page: ProfilePage,
    profile_hue: Option<u16>,
    username: String,
    avatar_url: String,
}

#[derive(Debug, serde::Deserialize)]
struct ProfilePage {
    html: String,
}

pub struct UserProfile {
    pub html: String,
    pub profile_hue: u16,
    pub username: String,
    pub avatar_url: String,
}

/// 获取用户主页数据（用于 !profile 卡片渲染）
pub async fn fetch_user_profile(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<UserProfile, ApiError> {
    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/{}?key=id",
        user_id,
        mode.api_value()
    );

    let client = http_client();

    retry_on_401(oauth, 5, || async {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("x-api-version", API_VERSION)
            .send()
            .await?;

        if resp.status() == 404 {
            return Err(ApiError::NotFound);
        }

        if !resp.status().is_success() {
            return Err(ApiError::InvalidResponse);
        }

        let data: OsuProfileResponse = resp.json().await?;

        Ok(UserProfile {
            html: data.page.html,
            profile_hue: data.profile_hue.unwrap_or(333),
            username: data.username,
            avatar_url: data.avatar_url,
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_full_score() -> OsuApiScore {
        OsuApiScore {
            id: 1001,
            score: 1234567,
            total_score: None,
            legacy_total_score: None,
            accuracy: 0.9876,
            max_combo: 543,
            pp: Some(300.5),
            rank: "S".to_string(),
            perfect: false,
            ended_at: "2024-01-01T00:00:00Z".to_string(),
            is_lazer: false,
            has_replay: true,
            legacy_score_id: None,
            beatmap: OsuApiBeatmap {
                id: 2001,
                beatmapset_id: 3001,
                version: "Insane".to_string(),
                difficulty_rating: 5.5,
                bpm: 180.0,
                ar: 9.0,
                od: 8.0,
                cs: 4.0,
                hp: 5.0,
                total_length: 200,
                max_combo: 800,
                passcount: 100,
                playcount: 500,
                status: "ranked".to_string(),
            },
            beatmapset: OsuApiBeatmapset {
                artist: "TestArtist".to_string(),
                title: "TestTitle".to_string(),
                creator: "Mapper".to_string(),
                covers: None,
                favourite_count: 100,
                play_count: 5000,
            },
            mods: vec![
                OsuApiMod::String("HD".to_string()),
                OsuApiMod::String("DT".to_string()),
            ],
            statistics: OsuApiScoreStatistics {
                count_300: 500,
                count_100: 10,
                count_50: 0,
                count_miss: 1,
            },
            user: None,
        }
    }

    #[test]
    fn test_api_score_to_score_happy_path() {
        let api = make_full_score();
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.beatmap_id, 2001);
        assert_eq!(score.beatmapset_id, 3001);
        assert_eq!(score.artist, "TestArtist");
        assert_eq!(score.title, "TestTitle");
        assert_eq!(score.version, "Insane");
        assert_eq!(score.creator, "Mapper");
        assert!((score.star_rating - 5.5).abs() < 0.0001);
        assert!((score.bpm - 180.0).abs() < 0.0001);
        assert!((score.ar - 9.0).abs() < 0.0001);
        assert!((score.od - 8.0).abs() < 0.0001);
        assert!((score.cs - 4.0).abs() < 0.0001);
        assert!((score.hp - 5.0).abs() < 0.0001);
        assert_eq!(score.length_seconds, 200);
        assert_eq!(score.score_value, 1234567);
        assert!((score.accuracy - 0.9876).abs() < 0.0001);
        assert_eq!(score.max_combo, 543);
        assert_eq!(score.beatmap_max_combo, 800);
        assert_eq!(score.pp, Some(300.5));
        assert_eq!(score.rank, "S");
        let mod_acronyms: Vec<String> = score
            .mods
            .iter()
            .map(|m| m.acronym().as_str().to_string())
            .collect();
        assert!(mod_acronyms.iter().any(|m| m == "HD"));
        assert!(mod_acronyms.iter().any(|m| m == "DT"));
        assert_eq!(mod_acronyms.len(), 2);
        assert!(!score.is_perfect);
        assert_eq!(score.created_at, "2024-01-01T00:00:00Z");
        assert!(!score.is_lazer);
        assert_eq!(score.statistics.count_300, 500);
        assert_eq!(score.statistics.count_100, 10);
        assert_eq!(score.statistics.count_50, 0);
        assert_eq!(score.statistics.count_miss, 1);
        assert_eq!(score.cover_url, "");
        assert_eq!(score.user.avatar_url, "");
        assert_eq!(score.user.country_code, "");
        assert_eq!(score.user.global_rank, None);
        assert_eq!(score.user.country_rank, None);
        assert!((score.user.pp - 0.0).abs() < 0.0001);
        assert_eq!(score.status, "ranked");
    }

    #[test]
    fn test_api_score_to_score_pp_null() {
        let mut api = make_full_score();
        api.pp = None;
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.pp, None);
    }

    #[test]
    fn test_api_score_to_score_is_perfect() {
        let mut api = make_full_score();
        api.perfect = true;
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.is_perfect);
    }

    #[test]
    fn test_api_score_to_score_empty_mods() {
        let mut api = make_full_score();
        api.mods = vec![];
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.mods.is_empty());
    }

    #[test]
    fn test_api_score_to_score_nested_user_data() {
        let mut api = make_full_score();
        api.beatmapset.covers = Some(serde_json::json!({
            "cover": "https://example.com/cover.jpg"
        }));
        api.user = Some(serde_json::json!({
            "avatar_url": "https://example.com/avatar.png",
            "country_code": "CN",
            "statistics": {
                "global_rank": 1234,
                "country_rank": 56,
                "pp": 9876.5
            }
        }));
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.cover_url, "https://example.com/cover.jpg");
        assert_eq!(score.user.avatar_url, "https://example.com/avatar.png");
        assert_eq!(score.user.country_code, "CN");
        assert_eq!(score.user.global_rank, Some(1234));
        assert_eq!(score.user.country_rank, Some(56));
        assert!((score.user.pp - 9876.5).abs() < 0.0001);
    }
}
