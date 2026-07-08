#![allow(dead_code, reason = "types will be used by command handler tasks")]

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;

use crate::rate_limiter::RateLimiter;
use crate::types::GameMode;

use super::{http_client, ApiError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbScope {
    Recent,
    Best,
}

impl SbScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            SbScope::Recent => "recent",
            SbScope::Best => "best",
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SbSearchResponse {
    pub status: String,
    pub results: i64,
    pub result: Vec<SbPlayerBrief>,
}

#[derive(Debug, Deserialize)]
pub struct SbPlayerBrief {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SbPlayerInfoResponse {
    pub status: String,
    pub player: SbPlayerData,
}

#[derive(Debug, Deserialize)]
pub struct SbPlayerData {
    pub info: SbPlayerInfo,
    pub stats: HashMap<String, SbPlayerStats>,
}

#[derive(Debug, Deserialize)]
pub struct SbPlayerInfo {
    pub id: i64,
    pub name: String,
    pub country: String,
    #[serde(rename = "preferred_mode")]
    pub preferred_mode: i32,
    #[serde(rename = "creation_time")]
    pub creation_time: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SbPlayerStats {
    pub mode: i32,
    pub pp: f64,
    #[serde(rename = "acc")]
    pub accuracy: f64,
    #[serde(rename = "tscore")]
    pub total_score: i64,
    #[serde(rename = "rscore")]
    pub ranked_score: i64,
    #[serde(rename = "plays")]
    pub play_count: i64,
    #[serde(rename = "playtime")]
    pub play_time: i64,
    #[serde(rename = "rank")]
    pub global_rank: i64,
    #[serde(rename = "country_rank")]
    pub country_rank: i64,
    #[serde(rename = "max_combo")]
    pub max_combo: i64,
    #[serde(rename = "total_hits")]
    pub total_hits: i64,
    #[serde(rename = "xh_count")]
    pub count_ssh: i64,
    #[serde(rename = "x_count")]
    pub count_ss: i64,
    #[serde(rename = "sh_count")]
    pub count_sh: i64,
    #[serde(rename = "s_count")]
    pub count_s: i64,
    #[serde(rename = "a_count")]
    pub count_a: i64,
}

#[derive(Debug, Deserialize)]
pub struct SbScoresResponse {
    pub status: String,
    pub scores: Vec<SbScore>,
    pub player: Option<SbScorePlayer>,
}

#[derive(Debug, Deserialize)]
pub struct SbScorePlayer {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct SbScore {
    pub id: Option<i64>,
    #[serde(rename = "map_md5")]
    pub map_md5: String,
    pub score: i64,
    pub pp: f64,
    #[serde(rename = "acc")]
    pub accuracy: f64,
    #[serde(rename = "max_combo")]
    pub max_combo: i64,
    pub mods: i64,
    #[serde(rename = "n300")]
    pub n300: i64,
    #[serde(rename = "n100")]
    pub n100: i64,
    #[serde(rename = "n50")]
    pub n50: i64,
    #[serde(rename = "nmiss")]
    pub nmiss: i64,
    #[serde(rename = "ngeki")]
    pub ngeki: i64,
    #[serde(rename = "nkatu")]
    pub nkatu: i64,
    #[serde(rename = "grade")]
    pub grade: String,
    pub status: i32,
    pub mode: i32,
    #[serde(rename = "play_time")]
    pub play_time: String,
    #[serde(rename = "time_elapsed")]
    pub time_elapsed: i64,
    pub perfect: bool,
    pub beatmap: Option<SbScoreBeatmap>,
}

#[derive(Debug, Deserialize)]
pub struct SbScoreBeatmap {
    pub id: i64,
    #[serde(rename = "set_id")]
    pub set_id: i64,
    pub md5: String,
    pub artist: String,
    pub title: String,
    pub version: String,
    pub creator: String,
    #[serde(rename = "total_length")]
    pub total_length: i64,
    #[serde(rename = "max_combo")]
    pub max_combo: i64,
    pub status: i32,
    pub mode: i32,
    pub bpm: f64,
    pub cs: f64,
    pub od: f64,
    pub ar: f64,
    pub hp: f64,
    #[serde(rename = "diff")]
    pub star_rating: f64,
    pub plays: i64,
    pub passes: i64,
}

#[derive(Debug, Deserialize)]
pub struct SbMapInfoResponse {
    pub status: String,
    pub map: SbBeatmap,
}

#[derive(Debug, Deserialize)]
pub struct SbBeatmap {
    pub id: i64,
    #[serde(rename = "set_id")]
    pub set_id: i64,
    pub md5: String,
    pub artist: String,
    pub title: String,
    pub version: String,
    pub creator: String,
    #[serde(rename = "total_length")]
    pub total_length: i64,
    #[serde(rename = "max_combo")]
    pub max_combo: i64,
    pub status: i32,
    pub mode: i32,
    pub bpm: f64,
    pub cs: f64,
    pub od: f64,
    pub ar: f64,
    pub hp: f64,
    #[serde(rename = "diff")]
    pub star_rating: f64,
    pub plays: i64,
    pub passes: i64,
}

pub struct SbPlayerInfoFull {
    pub id: i64,
    pub name: String,
    pub country: String,
    pub preferred_mode: i32,
    pub creation_time: i64,
    pub stats: HashMap<i32, SbPlayerStats>,
}

pub struct SbApi {
    base_url: String,
    rate_limiter: Arc<RateLimiter>,
}

impl SbApi {
    pub fn new(base_url: String, rate_limiter: Arc<RateLimiter>) -> Self {
        Self {
            base_url,
            rate_limiter,
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        self.rate_limiter
            .acquire()
            .await
            .map_err(|_| ApiError::ClientRateLimited)?;
        let url = format!("{}{}", self.base_url, path);
        let resp = http_client()
            .get(&url)
            .send()
            .await
            .map_err(ApiError::Http)?;
        let status = resp.status();
        let text = resp.text().await.map_err(ApiError::Http)?;
        if !status.is_success() {
            if status.as_u16() == 404 {
                return Err(ApiError::NotFound);
            }
            return Err(ApiError::ServerError(status.as_u16()));
        }
        let parsed: T = serde_json::from_str(&text).map_err(|e| {
            tracing::warn!(error = %e, text = %text, "sb_api: deserialization failed");
            ApiError::Deserialization(e.to_string())
        })?;
        Ok(parsed)
    }

    pub async fn search_player(&self, query: &str) -> Result<Vec<SbPlayerBrief>, ApiError> {
        let resp: SbSearchResponse = self
            .get(&format!(
                "/v1/search_players?q={}",
                urlencoding::encode(query)
            ))
            .await?;
        if resp.status != "success" {
            return Err(ApiError::InvalidResponse);
        }
        Ok(resp.result)
    }

    pub async fn get_player_info(&self, user_id: i64) -> Result<SbPlayerInfoFull, ApiError> {
        let resp: SbPlayerInfoResponse = self
            .get(&format!("/v1/get_player_info?id={}&scope=all", user_id))
            .await?;
        if resp.status != "success" {
            return Err(ApiError::NotFound);
        }
        let info = resp.player.info;
        let mut stats_map = HashMap::new();
        for (mode_str, stats) in resp.player.stats {
            if let Ok(mode) = mode_str.parse::<i32>() {
                stats_map.insert(mode, stats);
            }
        }
        Ok(SbPlayerInfoFull {
            id: info.id,
            name: info.name,
            country: info.country,
            preferred_mode: info.preferred_mode,
            creation_time: info.creation_time,
            stats: stats_map,
        })
    }

    pub async fn get_player_scores(
        &self,
        user_id: i64,
        scope: SbScope,
        mode: Option<GameMode>,
        limit: Option<u32>,
    ) -> Result<Vec<SbScore>, ApiError> {
        let mut url = format!(
            "/v1/get_player_scores?scope={}&id={}",
            scope.as_str(),
            user_id
        );
        if let Some(m) = mode {
            url.push_str(&format!("&mode={}", i32::from(m)));
        }
        if let Some(l) = limit {
            url.push_str(&format!("&limit={}", l));
        }
        let resp: SbScoresResponse = self.get(&url).await?;
        if resp.status != "success" {
            return Err(ApiError::InvalidResponse);
        }
        Ok(resp.scores)
    }

    pub async fn get_map_scores(
        &self,
        md5: &str,
        mode: Option<GameMode>,
        limit: Option<u32>,
    ) -> Result<Vec<SbScore>, ApiError> {
        let mut url = format!("/v1/get_map_scores?scope=best&md5={}", md5);
        if let Some(m) = mode {
            url.push_str(&format!("&mode={}", i32::from(m)));
        }
        if let Some(l) = limit {
            url.push_str(&format!("&limit={}", l));
        }
        let resp: SbScoresResponse = self.get(&url).await?;
        if resp.status != "success" {
            return Err(ApiError::InvalidResponse);
        }
        Ok(resp.scores)
    }

    pub async fn get_map_info(&self, beatmap_id: u32) -> Result<SbBeatmap, ApiError> {
        let resp: SbMapInfoResponse = self
            .get(&format!("/v1/get_map_info?id={}", beatmap_id))
            .await?;
        if resp.status != "success" {
            return Err(ApiError::NotFound);
        }
        Ok(resp.map)
    }
}
