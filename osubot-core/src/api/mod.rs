mod beatmap_attrs;
mod http;
mod oauth;
mod osu_api;
mod pp;
mod score_convert;
mod stable_grade;

pub use beatmap_attrs::apply_mod_adjustment_to_stats;
pub use http::download_beatmap_osu;
pub(crate) use http::{classify_http_error, retry_on_transient, API_VERSION};
pub(crate) use oauth::retry_on_401;
pub use oauth::OauthTokenCache;
pub use osu_api::{
    fetch_user_profile, fetch_user_stats_by_user_id, fetch_user_stats_by_username, get_score_by_id,
    get_user_beatmap_score, get_user_beatmap_scores_all, get_user_info, get_user_recent,
    OsuUserInfo,
};
pub use pp::{calculate_pp_breakdown, calculate_pp_if_acc, enrich_score_with_pp, PpCalcParams};

use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;

pub fn http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client")
    })
}

fn default_true() -> bool {
    true
}

/// 从 covers JSON 提取 fullsize 背景图 URL（仿 yumu-bot: list → fullsize）
fn fullsize_cover_url(covers: Option<&serde_json::Value>) -> Option<String> {
    let covers = covers?;
    if let Some(list_url) = covers.get("list").and_then(|v| v.as_str()) {
        return Some(list_url.replace("@2x", "").replace("list", "fullsize"));
    }
    covers
        .get("cover")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[derive(Debug, serde::Deserialize)]
#[expect(
    dead_code,
    reason = "only needed for serde deserialization + test construction"
)]
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
struct BeatmapUserScore {
    score: Option<OsuApiScore>,
}

#[derive(Debug, serde::Deserialize)]
struct BeatmapScoresResponse {
    scores: Vec<OsuApiScore>,
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
    is_lazer: Option<bool>,
    #[serde(default)]
    build_id: Option<i64>,
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
    #[serde(default = "default_true")]
    passed: bool,
    #[serde(default)]
    perfect: bool,
    #[serde(default, alias = "created_at")]
    ended_at: String,
    #[serde(default)]
    has_replay: bool,
    #[serde(default)]
    legacy_score_id: Option<i64>,
    #[serde(default)]
    beatmap_id: i64,
    #[serde(default)]
    beatmapset_id: i64,
    #[serde(default)]
    beatmap: Option<OsuApiBeatmap>,
    #[serde(default)]
    beatmapset: Option<OsuApiBeatmapset>,
    #[serde(default)]
    mods: Vec<OsuApiMod>,
    #[serde(default)]
    statistics: OsuApiScoreStatistics,
    #[serde(default)]
    user: Option<serde_json::Value>,
    #[serde(default)]
    ruleset_id: i64,
}

impl OsuApiScore {
    fn extra_mode(&self) -> GameMode {
        GameMode::try_from(self.ruleset_id as i32).unwrap_or(GameMode::Osu)
    }
}

#[derive(Debug, serde::Deserialize, Default)]
struct OsuApiScoreStatistics {
    #[serde(default, alias = "perfect")]
    count_geki: i64,
    #[serde(default, alias = "great")]
    count_300: i64,
    #[serde(default, alias = "good")]
    count_katu: i64,
    #[serde(default)]
    count_100: i64,
    #[serde(default, alias = "meh")]
    count_50: i64,
    #[serde(default, alias = "miss")]
    count_miss: i64,
    #[serde(default)]
    ok: i64,
    #[serde(default, alias = "large_tick_miss")]
    osu_large_tick_misses: i64,
    #[serde(default, alias = "small_tick_miss")]
    osu_small_tick_misses: i64,
    #[serde(default, alias = "large_tick_hit")]
    osu_large_tick_hits: i64,
    #[serde(default, alias = "small_tick_hit")]
    osu_small_tick_hits: i64,
    #[serde(default, alias = "slider_tail_hit")]
    osu_slider_tail_hits: i64,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScoreUser {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    username: Option<String>,
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

#[derive(Debug, serde::Deserialize)]
struct OsuApiV2User {
    id: i64,
    username: String,
    country_code: Option<String>,
    statistics: Option<OsuStatistics>,
    cover: Option<OsuUserCover>,
}

#[derive(Debug, serde::Deserialize)]
struct OsuUserCover {
    custom_url: Option<String>,
    url: Option<String>,
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
    playtime: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct OauthResponse {
    access_token: String,
}

#[derive(Debug, serde::Deserialize)]
struct OsuProfileResponse {
    page: ProfilePage,
    profile_hue: Option<u16>,
    username: String,
    avatar_url: String,
    cover: Option<Cover>,
}

#[derive(Debug, serde::Deserialize)]
struct Cover {
    url: Option<String>,
    custom_url: Option<String>,
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
    pub cover_url: Option<String>,
}

use crate::types::GameMode;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("User not found")]
    NotFound,
    #[error("Invalid API response")]
    InvalidResponse,
    #[error("Server error ({0})")]
    ServerError(u16),
    #[error("Response deserialization failed: {0}")]
    Deserialization(String),
    #[error("API key missing")]
    MissingApiKey,
    #[error("OAuth token error")]
    OAuthError,
    #[error("Rate limited - retry after {0:?} seconds")]
    RateLimitedWithRetryAfter(Option<u64>),
    #[error("Client rate limited - local token bucket exhausted")]
    ClientRateLimited,
}

impl ApiError {
    pub(crate) fn is_transient(&self) -> bool {
        match self {
            ApiError::Http(_)
            | ApiError::ServerError(_)
            | ApiError::RateLimitedWithRetryAfter(_) => true,
            ApiError::NotFound
            | ApiError::InvalidResponse
            | ApiError::Deserialization(_)
            | ApiError::MissingApiKey
            | ApiError::OAuthError
            | ApiError::ClientRateLimited => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_transient_classification() {
        assert!(!ApiError::NotFound.is_transient());
        assert!(!ApiError::InvalidResponse.is_transient());
        assert!(!ApiError::MissingApiKey.is_transient());
        assert!(!ApiError::OAuthError.is_transient());
        assert!(ApiError::RateLimitedWithRetryAfter(Some(60)).is_transient());
        assert!(ApiError::RateLimitedWithRetryAfter(None).is_transient());
        assert!(!ApiError::ClientRateLimited.is_transient());
        assert!(ApiError::ServerError(500).is_transient());
        assert!(ApiError::ServerError(503).is_transient());
        assert!(!ApiError::Deserialization("bad json".into()).is_transient());
    }

    #[test]
    fn test_server_error_display() {
        let e = ApiError::ServerError(502);
        assert_eq!(format!("{}", e), "Server error (502)");
    }

    #[test]
    fn test_deserialization_display() {
        let e = ApiError::Deserialization("missing field `id`".into());
        assert!(format!("{}", e).contains("missing field `id`"));
    }
}
