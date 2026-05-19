#![deny(clippy::all)]
#![allow(clippy::derive_partial_eq_without_eq)]

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
    pub lastplayed: i64, // Unix timestamp (seconds)
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

/// osu! API v2 recent play entry
#[derive(Debug, serde::Deserialize)]
pub struct RecentPlay {
    pub beatmap: BeatmapInfo,
    pub count: PlayCount,
    #[serde(rename = "maxcombo")]
    pub maxcombo: i64,
    #[serde(rename = "perfect")]
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
            refresh_interval: Duration::from_secs(4 * 3600),
        }
    }

    /// Get a valid OAuth token, refreshing if needed
    pub async fn get_token(&self, rate_limiter: &RateLimiter) -> Result<String, ApiError> {
        let mut guard = self.cache.lock().await;
        if let Some((ref token, fetched_at)) = *guard {
            if fetched_at.elapsed() < self.refresh_interval {
                return Ok(token.clone());
            }
        }

        rate_limiter.acquire().await.map_err(|_| ApiError::RateLimited)?;
        let client = Client::new();

        let params = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("grant_type", "client_credentials"),
            ("scope", "public"),
        ];

        let resp = client.post("https://osu.ppy.sh/oauth/token").form(&params).send().await?;

        if resp.status() != 200 {
            return Err(ApiError::OAuthError);
        }

        let token_data: OauthResponse = resp.json().await?;
        *guard = Some((token_data.access_token.clone(), Instant::now()));
        Ok(token_data.access_token)
    }
}

/// 从 osu! API v2 获取用户数据
pub async fn fetch_user_stats(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    username: &str,
    mode: GameMode,
) -> Result<UserStats, ApiError> {
    rate_limiter.acquire().await.map_err(|_| ApiError::RateLimited)?;

    let access_token = oauth.get_token(rate_limiter).await?;

    let client = Client::new();
    let mode_param = mode.api_value();

    // 纯数字用户名需要加 @ 前缀，否则 API 会当作 user ID 处理
    let url_username = if username.chars().all(|c| c.is_ascii_digit()) {
        format!("@{}", username)
    } else {
        username.to_string()
    };

    let url = format!("https://osu.ppy.sh/api/v2/users/{}/{}", url_username, mode_param);

    let resp =
        client.get(&url).header("Authorization", format!("Bearer {}", access_token)).send().await?;

    if resp.status() == 404 {
        return Err(ApiError::NotFound);
    }

    let data: OsuApiV2User = resp.json().await?;

    let stats = match data.statistics {
        Some(s) => s,
        None => {
            return Err(ApiError::NotFound);
        }
    };

    let rank_change = None;
    let country_rank_change = None;

    Ok(UserStats {
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
        rank_change,
        country_rank_change,
    })
}

/// Get user's recent plays from osu! API v2
pub async fn get_user_recent(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    username: &str,
    mode: GameMode,
) -> Result<Vec<RecentPlay>, ApiError> {
    rate_limiter.acquire().await.map_err(|_| ApiError::RateLimited)?;

    let access_token = oauth.get_token(rate_limiter).await?;
    let client = Client::new();

    // 纯数字用户名需要加 @ 前缀，否则 API 会当作 user ID 处理
    let url_username = if username.chars().all(|c| c.is_ascii_digit()) {
        format!("@{}", username)
    } else {
        username.to_string()
    };

    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/recent?mode={}",
        url_username,
        mode.api_value()
    );

    let resp =
        client.get(&url).header("Authorization", format!("Bearer {}", access_token)).send().await?;

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
    rate_limiter.acquire().await.map_err(|_| ApiError::RateLimited)?;

    let access_token = oauth.get_token(rate_limiter).await?;
    let client = Client::new();

    // 纯数字用户名需要加 @ 前缀，否则 API 会当作 user ID 处理
    let url_username = if username.chars().all(|c| c.is_ascii_digit()) {
        format!("@{}", username)
    } else {
        username.to_string()
    };

    let url = format!("https://osu.ppy.sh/api/v2/users/{}", url_username);

    let resp =
        client.get(&url).header("Authorization", format!("Bearer {}", access_token)).send().await?;

    if resp.status() == 404 {
        return Ok(None);
    }

    if !resp.status().is_success() {
        return Err(ApiError::InvalidResponse);
    }

    let user: OsuUserInfo = resp.json().await?;
    Ok(Some(user))
}
