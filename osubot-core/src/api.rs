use crate::rate_limiter::RateLimiter;
use crate::types::{GameMode, UserStats};
use reqwest::Client;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;

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

/// Get user's recent plays from osu! API v2 (requires numeric user ID)
pub async fn get_user_recent(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<Vec<RecentPlay>, ApiError> {
    let access_token = oauth.get_token().await?;
    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::RateLimited)?;
    let client = Client::new();

    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/scores/recent?mode={}",
        user_id,
        mode.api_value()
    );

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", access_token))
        .send()
        .await?;

    if resp.status() == 404 {
        return Err(ApiError::NotFound);
    }

    if !resp.status().is_success() {
        return Err(ApiError::InvalidResponse);
    }

    let plays: Vec<RecentPlay> = resp.json().await?;
    Ok(plays)
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
