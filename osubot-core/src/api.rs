use crate::rate_limiter::RateLimiter;
use crate::types::{
    Beatmap, Beatmapset, Covers, GameMode, Grade, LazerStatistics, PlayerInfo, ScoreCard,
    ScoreInfo, UserStats,
};
use reqwest::Client;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::warn;

/// osu! API v2 beatmap info from recent plays
#[derive(Debug, serde::Deserialize)]
pub struct BeatmapInfo {
    pub id: i64,
}

/// osu! API v2 play count breakdown
#[derive(Debug, serde::Deserialize)]
pub struct PlayCount {
    #[serde(rename = "ok")]
    pub ok_count: i64,
    #[serde(rename = "me")]
    pub me_count: i64,
    #[serde(rename = "combo")]
    pub combo_count: i64,
    #[serde(rename = "miss")]
    pub miss_count: i64,
}

/// osu! API v2 play statistics
#[derive(Debug, serde::Deserialize)]
pub struct PlayStatistics {
    #[serde(rename = "count_100")]
    pub count_100: i64,
    #[serde(rename = "count_300")]
    pub count_300: i64,
    #[serde(rename = "count_50")]
    pub count_50: i64,
    #[serde(rename = "count_miss")]
    pub count_miss: i64,
}

/// osu! API v2 recent play entry
#[derive(Debug, serde::Deserialize)]
pub struct RecentPlay {
    pub beatmap: BeatmapInfo,
    pub statistics: PlayStatistics,
    #[serde(rename = "max_combo")]
    pub max_combo: Option<i64>,
    pub perfect: bool,
    pub created_at: String,
}

/// osu! API v2 beatmapset covers
#[derive(Debug, serde::Deserialize)]
pub struct ApiCovers {
    #[serde(rename = "cover")]
    pub cover: String,
    #[serde(rename = "cover@2x")]
    pub cover_2x: String,
    #[serde(rename = "card")]
    pub card: String,
    #[serde(rename = "card@2x")]
    pub card_2x: String,
    #[serde(rename = "list")]
    pub list: String,
    #[serde(rename = "list@2x")]
    pub list_2x: String,
    #[serde(rename = "slimcover")]
    pub slimcover: String,
    #[serde(rename = "slimcover@2x")]
    pub slimcover_2x: String,
}

/// osu! API v2 beatmapset
#[derive(Debug, serde::Deserialize)]
pub struct ApiBeatmapset {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub creator: String,
    #[serde(rename = "covers")]
    pub covers: ApiCovers,
}

/// osu! API v2 beatmap from score response
#[derive(Debug, serde::Deserialize)]
pub struct ApiBeatmap {
    pub id: i64,
    #[serde(rename = "difficulty_rating")]
    pub difficulty_rating: f64,
    pub version: String,
    pub cs: Option<f32>,
    pub ar: Option<f32>,
    pub od: Option<f32>,
    pub hp: Option<f32>,
    pub bpm: Option<f32>,
    #[serde(rename = "total_length")]
    pub total_length: i32,
    #[serde(rename = "hit_length")]
    pub hit_length: Option<i32>,
    #[serde(rename = "max_combo")]
    pub max_combo: Option<i32>,
    #[serde(rename = "circle_count")]
    pub circle_count: Option<i32>,
    #[serde(rename = "slider_count")]
    pub slider_count: Option<i32>,
    #[serde(rename = "spinner_count")]
    pub spinner_count: Option<i32>,
    #[serde(rename = "beatmapset")]
    pub beatmapset: Option<ApiBeatmapset>,
}

/// osu! API v2 user in score response
#[derive(Debug, serde::Deserialize)]
pub struct ApiScoreUser {
    pub username: String,
    #[serde(rename = "avatar_url")]
    pub avatar_url: String,
}

/// osu! API v2 hit statistics
#[derive(Debug, serde::Deserialize)]
pub struct ApiLazerStatistics {
    #[serde(rename = "perfect")]
    pub perfect: i32,
    #[serde(rename = "great")]
    pub great: i32,
    #[serde(rename = "good")]
    pub good: i32,
    #[serde(rename = "ok")]
    pub ok: i32,
    #[serde(rename = "meh")]
    pub meh: i32,
    #[serde(rename = "miss")]
    pub miss: i32,
    #[serde(rename = "large_tick_hit")]
    pub large_tick_hit: i32,
    #[serde(rename = "large_tick_miss")]
    pub large_tick_miss: i32,
    #[serde(rename = "small_tick_hit")]
    pub small_tick_hit: i32,
    #[serde(rename = "small_tick_miss")]
    pub small_tick_miss: i32,
    #[serde(rename = "slider_tail_hit")]
    pub slider_tail_hit: i32,
    #[serde(rename = "large_bonus")]
    pub large_bonus: i32,
    #[serde(rename = "small_bonus")]
    pub small_bonus: i32,
    #[serde(rename = "ignore_hit")]
    pub ignore_hit: i32,
    #[serde(rename = "ignore_miss")]
    pub ignore_miss: i32,
    #[serde(rename = "legacy_combo_increase")]
    pub legacy_combo_increase: i32,
}

/// osu! API v2 score entry from /users/{id}/scores/recent
#[derive(Debug, serde::Deserialize)]
pub struct ApiScore {
    pub id: i64,
    pub score: i64,
    #[serde(rename = "max_combo")]
    pub max_combo: Option<i32>,
    pub accuracy: Option<f64>,
    pub pp: Option<f64>,
    pub rank: String,
    pub passed: bool,
    #[serde(rename = "ended_at")]
    pub ended_at: String,
    pub mods: Vec<String>,
    pub user: ApiScoreUser,
    pub beatmap: ApiBeatmap,
    pub statistics: ApiLazerStatistics,
    #[serde(rename = "maximum_statistics")]
    pub maximum_statistics: Option<ApiLazerStatistics>,
}

impl From<ApiScore> for ScoreCard {
    fn from(api: ApiScore) -> Self {
        let grade = if api.passed {
            Grade::from_rank_str(&api.rank)
        } else {
            Grade::F
        };

        let beatmap = Beatmap {
            id: api.beatmap.id,
            difficulty_rating: api.beatmap.difficulty_rating,
            version: api.beatmap.version,
            cs: {
                let val = api.beatmap.cs;
                if val.is_none() {
                    warn!(
                        "API returned null for beatmap cs (beatmap_id={})",
                        api.beatmap.id
                    );
                }
                val.unwrap_or(0.0)
            },
            ar: {
                let val = api.beatmap.ar;
                if val.is_none() {
                    warn!(
                        "API returned null for beatmap ar (beatmap_id={})",
                        api.beatmap.id
                    );
                }
                val.unwrap_or(0.0)
            },
            od: {
                let val = api.beatmap.od;
                if val.is_none() {
                    warn!(
                        "API returned null for beatmap od (beatmap_id={})",
                        api.beatmap.id
                    );
                }
                val.unwrap_or(0.0)
            },
            hp: {
                let val = api.beatmap.hp;
                if val.is_none() {
                    warn!(
                        "API returned null for beatmap hp (beatmap_id={})",
                        api.beatmap.id
                    );
                }
                val.unwrap_or(0.0)
            },
            bpm: {
                let val = api.beatmap.bpm;
                if val.is_none() {
                    warn!(
                        "API returned null for beatmap bpm (beatmap_id={})",
                        api.beatmap.id
                    );
                }
                val.unwrap_or(0.0)
            },
            total_length: api.beatmap.total_length,
            hit_length: {
                let val = api.beatmap.hit_length;
                if val.is_none() {
                    warn!(
                        "API returned null for beatmap hit_length (beatmap_id={})",
                        api.beatmap.id
                    );
                }
                val.unwrap_or(api.beatmap.total_length)
            },
            max_combo: {
                let val = api.beatmap.max_combo;
                if val.is_none() {
                    warn!(
                        "API returned null for beatmap max_combo (beatmap_id={})",
                        api.beatmap.id
                    );
                }
                val.unwrap_or(0)
            },
            circle_count: api.beatmap.circle_count,
            slider_count: api.beatmap.slider_count,
            spinner_count: api.beatmap.spinner_count,
            beatmapset: api.beatmap.beatmapset.map(|bs| Beatmapset {
                id: bs.id,
                title: bs.title,
                artist: bs.artist,
                creator: bs.creator,
                covers: Covers {
                    cover: bs.covers.cover,
                    cover_2x: bs.covers.cover_2x,
                    card: bs.covers.card,
                    card_2x: bs.covers.card_2x,
                    list: bs.covers.list,
                    list_2x: bs.covers.list_2x,
                    slimcover: bs.covers.slimcover,
                    slimcover_2x: bs.covers.slimcover_2x,
                },
            }),
        };

        let statistics = LazerStatistics {
            perfect: api.statistics.perfect,
            great: api.statistics.great,
            good: api.statistics.good,
            ok: api.statistics.ok,
            meh: api.statistics.meh,
            miss: api.statistics.miss,
            large_tick_hit: api.statistics.large_tick_hit,
            large_tick_miss: api.statistics.large_tick_miss,
            small_tick_hit: api.statistics.small_tick_hit,
            small_tick_miss: api.statistics.small_tick_miss,
            slider_tail_hit: api.statistics.slider_tail_hit,
            large_bonus: api.statistics.large_bonus,
            small_bonus: api.statistics.small_bonus,
            ignore_hit: api.statistics.ignore_hit,
            ignore_miss: api.statistics.ignore_miss,
            legacy_combo_increase: api.statistics.legacy_combo_increase,
        };

        let maximum_statistics = api.maximum_statistics.map(|ms| LazerStatistics {
            perfect: ms.perfect,
            great: ms.great,
            good: ms.good,
            ok: ms.ok,
            meh: ms.meh,
            miss: ms.miss,
            large_tick_hit: ms.large_tick_hit,
            large_tick_miss: ms.large_tick_miss,
            small_tick_hit: ms.small_tick_hit,
            small_tick_miss: ms.small_tick_miss,
            slider_tail_hit: ms.slider_tail_hit,
            large_bonus: ms.large_bonus,
            small_bonus: ms.small_bonus,
            ignore_hit: ms.ignore_hit,
            ignore_miss: ms.ignore_miss,
            legacy_combo_increase: ms.legacy_combo_increase,
        });

        ScoreCard {
            beatmap,
            player: PlayerInfo {
                username: api.user.username,
                avatar_url: api.user.avatar_url,
            },
            score: ScoreInfo {
                score: api.score,
                pp: api.pp.unwrap_or(0.0),
                accuracy: api.accuracy.unwrap_or(0.0),
                max_combo: api.max_combo.unwrap_or(0),
                grade,
                passed: api.passed,
                rank: api.rank,
                ended_at: api.ended_at,
                mods: api.mods,
                statistics,
                maximum_statistics,
            },
        }
    }
}

/// osu! API v2 basic user info (for activity detection)
#[derive(Debug, serde::Deserialize)]
pub struct OsuUserInfo {
    pub id: i64,
    pub username: String,
    pub is_active: bool,
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

/// osu! OAuth token response
#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct OauthResponse {
    access_token: String,
    token_type: String,
    expires_in: i64,
}

/// osu! API v2 user response — top-level fields
#[derive(Debug, serde::Deserialize)]
struct OsuApiV2User {
    id: i64,
    username: String,
    country_code: Option<String>, // e.g., "CN", "US", "JP"
    statistics: Option<OsuStatistics>,
}

/// osu! API v2 statistics sub-object
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

/// Cached osu! OAuth token — refreshes every 4 hours
pub struct OauthTokenCache {
    client_id: String,
    client_secret: String,
    cache: Mutex<Option<(String, Instant)>>,
    refresh_interval: Duration,
}

impl OauthTokenCache {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            cache: Mutex::new(None),
            refresh_interval: Duration::from_secs(20 * 3600),
        }
    }

    /// Mark the cached token as invalid, forcing refresh on next get_token call
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.try_lock() {
            *guard = None;
        }
    }

    /// Get a valid OAuth token, refreshing if needed
    pub async fn get_token(&self) -> Result<String, ApiError> {
        let mut guard = self.cache.lock().await;
        if let Some((ref token, fetched_at)) = *guard {
            if fetched_at.elapsed() < self.refresh_interval {
                return Ok(token.clone());
            }
        }

        let client = Client::new();

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
        *guard = Some((token_data.access_token.clone(), Instant::now()));
        Ok(token_data.access_token)
    }
}

/// Internal helper for fetching user stats from osu! API
async fn fetch_user_stats_internal(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    url: &str,
) -> Result<UserStats, ApiError> {
    let mut retry_count = 0;
    let max_retries = 5;
    let base_delay = Duration::from_secs(1);

    let client = Client::new();

    loop {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await?;

        if resp.status() == 401 {
            oauth.invalidate();
            if retry_count < max_retries {
                let delay =
                    Duration::from_secs(base_delay.as_secs() * 2_u64.pow(retry_count as u32));
                retry_count += 1;
                tokio::time::sleep(delay).await;
                continue;
            }
            return Err(ApiError::OAuthError);
        }

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

        return Ok(UserStats {
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
        });
    }
}

/// Encode username for osu! API URL: pure-digit usernames get @ prefix so the API
/// treats them as username lookups rather than user ID lookups.
fn url_encode_username(username: &str) -> String {
    if username.chars().all(|c| c.is_ascii_digit()) {
        format!("@{}", username)
    } else {
        username.to_string()
    }
}

/// Fetch user stats by username (for where <username> command)
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

/// Fetch user stats by numeric user_id (for internal/scheduler use)
/// user_id goes directly to API without @ prefix (API treats as user_id lookup)
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

/// Get user's recent scores from osu! API v2 (requires numeric user ID)
pub async fn get_user_recent(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    user_id: i64,
    mode: GameMode,
    include_fails: bool,
) -> Result<Vec<ApiScore>, ApiError> {
    let mut retry_count = 0;
    let max_retries = 5;
    let base_delay = Duration::from_secs(1);

    let client = Client::new();

    let mode_str = mode.api_value();
    let mut url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/scores/recent?mode={}&limit=1",
        user_id, mode_str
    );
    if include_fails {
        url.push_str("&include_fails=true");
    }

    loop {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await?;

        if resp.status() == 401 {
            oauth.invalidate();
            if retry_count < max_retries {
                let delay =
                    Duration::from_secs(base_delay.as_secs() * 2_u64.pow(retry_count as u32));
                retry_count += 1;
                tokio::time::sleep(delay).await;
                continue;
            }
            return Err(ApiError::OAuthError);
        }

        if resp.status() == 404 {
            return Err(ApiError::NotFound);
        }

        if !resp.status().is_success() {
            return Err(ApiError::InvalidResponse);
        }

        let scores: Vec<ApiScore> = resp.json().await?;
        return Ok(scores);
    }
}

/// Get basic user info from osu! API v2 (for activity detection)
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
    let client = Client::new();

    let url_username = url_encode_username(username);
    let url = format!("https://osu.ppy.sh/api/v2/users/{}", url_username);

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
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

/// osu! API v2 user profile page response
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

/// Fetched user profile data for rendering
pub struct UserProfile {
    pub html: String,
    pub profile_hue: u16,
    pub username: String,
    pub avatar_url: String,
}

/// Fetch user profile page HTML from osu! API v2 by user ID.
/// Returns the BBcode HTML fragment and profile hue for CSS theming.
pub async fn fetch_user_profile(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<UserProfile, ApiError> {
    let mut retry_count = 0;
    let max_retries = 5;
    let base_delay = Duration::from_secs(1);

    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/{}?key=id",
        user_id,
        mode.api_value()
    );

    let client = Client::new();

    loop {
        let access_token = oauth.get_token().await?;
        rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::RateLimited)?;

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", access_token))
            .send()
            .await?;

        if resp.status() == 401 {
            oauth.invalidate();
            if retry_count < max_retries {
                let delay =
                    Duration::from_secs(base_delay.as_secs() * 2_u64.pow(retry_count as u32));
                retry_count += 1;
                tokio::time::sleep(delay).await;
                continue;
            }
            return Err(ApiError::OAuthError);
        }

        if resp.status() == 404 {
            return Err(ApiError::NotFound);
        }

        if !resp.status().is_success() {
            return Err(ApiError::InvalidResponse);
        }

        let data: OsuProfileResponse = resp.json().await?;

        return Ok(UserProfile {
            html: data.page.html,
            profile_hue: data.profile_hue.unwrap_or(333),
            username: data.username,
            avatar_url: data.avatar_url,
        });
    }
}
