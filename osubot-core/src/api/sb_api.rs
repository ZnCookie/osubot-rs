use crate::api::{http_client, ApiError};
use crate::rate_limiter::RateLimiter;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SbSearchPlayer {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct SbSearchPlayersData {
    players: Vec<SbSearchPlayer>,
}

#[derive(Debug, Deserialize)]
pub struct SbPlayerStats {
    pub mode: i32,
    #[serde(default)]
    pub pp: f64,
    #[serde(default, alias = "acc")]
    pub accuracy: f64,
    #[serde(default)]
    pub total_score: i64,
    #[serde(default)]
    pub ranked_score: i64,
    #[serde(default)]
    pub play_count: i64,
    #[serde(default)]
    pub play_time: i64,
    #[serde(default)]
    pub global_rank: i64,
    #[serde(default)]
    pub country_rank: i64,
    #[serde(default)]
    pub max_combo: i64,
    #[serde(default)]
    pub total_hits: i64,
    #[serde(default)]
    pub count_ssh: i64,
    #[serde(default)]
    pub count_ss: i64,
    #[serde(default)]
    pub count_sh: i64,
    #[serde(default)]
    pub count_s: i64,
    #[serde(default)]
    pub count_a: i64,
}

#[derive(Debug, Deserialize)]
pub struct SbPlayerInfo {
    pub id: i64,
    pub name: String,
    pub country: String,
    #[serde(default)]
    pub privilege: i64,
    #[serde(default)]
    pub preferred_mode: i32,
    #[serde(default)]
    pub silence_end: i64,
    #[serde(default)]
    pub donor_end: i64,
    #[serde(default)]
    pub creation_time: i64,
    #[serde(default)]
    pub latest_activity: i64,
}

#[derive(Debug, Deserialize)]
pub struct SbScoreBeatmap {
    pub id: i64,
    #[serde(default)]
    pub set_id: i64,
    #[serde(default)]
    pub md5: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub total_length: i64,
    #[serde(default)]
    pub max_combo: i64,
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub mode: i32,
    #[serde(default)]
    pub bpm: f64,
    #[serde(default)]
    pub cs: f64,
    #[serde(default)]
    pub ar: f64,
    #[serde(default, alias = "accuracy")]
    pub od: f64,
    #[serde(default)]
    pub hp: f64,
    #[serde(default, alias = "diff")]
    pub star_rating: f64,
    #[serde(default)]
    pub plays: i64,
    #[serde(default)]
    pub passes: i64,
}

#[derive(Debug, Deserialize)]
pub struct SbScore {
    pub id: i64,
    #[serde(default, alias = "map_md5")]
    pub map_md5: String,
    #[serde(default)]
    pub userid: i64,
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub pp: f64,
    #[serde(default, alias = "acc")]
    pub accuracy: f64,
    #[serde(default)]
    pub max_combo: i64,
    #[serde(default)]
    pub mods: i64,
    #[serde(default)]
    pub n300: i64,
    #[serde(default)]
    pub n100: i64,
    #[serde(default)]
    pub n50: i64,
    #[serde(default)]
    pub nmiss: i64,
    #[serde(default)]
    pub ngeki: i64,
    #[serde(default)]
    pub nkatu: i64,
    #[serde(default)]
    pub rank: String,
    #[serde(default)]
    pub mode: i32,
    #[serde(default)]
    pub play_time: String,
    #[serde(default)]
    pub time_elapsed: i64,
    #[serde(default)]
    pub perfect: bool,
    #[serde(default)]
    pub beatmap: Option<SbScoreBeatmap>,
}

#[derive(Debug, Deserialize)]
pub struct SbMapInfo {
    pub id: i64,
    #[serde(default)]
    pub set_id: i64,
    #[serde(default)]
    pub md5: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub total_length: i64,
    #[serde(default)]
    pub max_combo: i64,
    #[serde(default)]
    pub status: i32,
    #[serde(default)]
    pub mode: i32,
    #[serde(default)]
    pub bpm: f64,
    #[serde(default)]
    pub cs: f64,
    #[serde(default)]
    pub ar: f64,
    #[serde(default, alias = "accuracy")]
    pub od: f64,
    #[serde(default)]
    pub hp: f64,
    #[serde(default, alias = "diff")]
    pub star_rating: f64,
    #[serde(default)]
    pub plays: i64,
    #[serde(default)]
    pub passes: i64,
}

#[derive(Debug, Deserialize)]
struct SbPlayerData {
    player: SbPlayerDataInner,
}

#[derive(Debug, Deserialize)]
struct SbPlayerDataInner {
    info: SbPlayerInfo,
    #[serde(default)]
    stats: std::collections::HashMap<String, SbPlayerStats>,
    #[serde(default)]
    #[expect(dead_code, reason = "deserialization target")]
    clan: Option<serde_json::Value>,
}

const SB_API_BASE: &str = "https://api.ppy.sb";

async fn sb_get<T>(url: &str, rate_limiter: &RateLimiter) -> Result<T, ApiError>
where
    T: serde::de::DeserializeOwned,
{
    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;

    let resp = http_client().get(url).send().await?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(ApiError::NotFound);
    }
    if status.is_server_error() {
        return Err(ApiError::ServerError(status.as_u16()));
    }
    if status.is_client_error() {
        return Err(ApiError::ClientError(status.as_u16()));
    }
    if !status.is_success() {
        return Err(ApiError::InvalidResponse);
    }

    let body = resp.text().await.map_err(ApiError::Http)?;

    let raw: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ApiError::Deserialization(e.to_string()))?;

    if raw.get("status").and_then(|s| s.as_str()) != Some("success") {
        return Err(ApiError::InvalidResponse);
    }

    serde_json::from_value(raw).map_err(|e| ApiError::Deserialization(e.to_string()))
}

pub async fn search_player(
    username: &str,
    rate_limiter: &RateLimiter,
) -> Result<Vec<SbSearchPlayer>, ApiError> {
    let url = format!("{}/v1/search_players?q={}", SB_API_BASE, username);
    let data: SbSearchPlayersData = sb_get(&url, rate_limiter).await?;
    Ok(data.players)
}

pub async fn get_player_info(
    id: i64,
    rate_limiter: &RateLimiter,
) -> Result<
    (
        SbPlayerInfo,
        std::collections::HashMap<String, SbPlayerStats>,
    ),
    ApiError,
> {
    let url = format!(
        "{}/v1/get_player_info?id={}&scope=user_info",
        SB_API_BASE, id
    );
    let data: SbPlayerData = sb_get(&url, rate_limiter).await?;
    Ok((data.player.info, data.player.stats))
}

pub async fn get_player_scores(
    scope: &str,
    id: i64,
    mode: i32,
    limit: i32,
    rate_limiter: &RateLimiter,
) -> Result<Vec<SbScore>, ApiError> {
    let url = format!(
        "{}/v1/get_player_scores?scope={}&id={}&mode={}&limit={}",
        SB_API_BASE, scope, id, mode, limit
    );
    let data: SbScoresData = sb_get(&url, rate_limiter).await?;
    Ok(data.scores)
}

pub async fn get_map_scores(
    scope: &str,
    md5: &str,
    mode: i32,
    limit: i32,
    rate_limiter: &RateLimiter,
) -> Result<Vec<SbScore>, ApiError> {
    let url = format!(
        "{}/v1/get_map_scores?scope={}&md5={}&mode={}&limit={}",
        SB_API_BASE, scope, md5, mode, limit
    );
    let data: SbScoresData = sb_get(&url, rate_limiter).await?;
    Ok(data.scores)
}

pub async fn get_map_info(id: i64, rate_limiter: &RateLimiter) -> Result<SbMapInfo, ApiError> {
    let url = format!("{}/v1/get_map_info?id={}", SB_API_BASE, id);
    let data: SbMapInfoData = sb_get(&url, rate_limiter).await?;
    Ok(data.map)
}

#[derive(Debug, Deserialize)]
struct SbScoresData {
    scores: Vec<SbScore>,
}

#[derive(Debug, Deserialize)]
struct SbMapInfoData {
    map: SbMapInfo,
}
