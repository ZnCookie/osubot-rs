use std::sync::Arc;

use futures::stream::{self, StreamExt};

use crate::log_fmt;
use crate::rate_limiter::RateLimiter;
use crate::types::{GameMode, Score, UserStats};

use super::http;
use super::score_convert::api_score_to_score;
use super::{fullsize_cover_url, ApiError, OsuApiBeatmap, OsuApiBeatmapset, OsuApiScore};

/// osu! API v2 basic user info (for activity detection)
#[derive(Debug, serde::Deserialize)]
pub struct OsuUserInfo {
    pub id: i64,
    pub username: String,
}

async fn fetch_user_stats_internal(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    url: &str,
) -> Result<UserStats, ApiError> {
    let resp = http::authenticated_get(url, rate_limiter, oauth).await?;
    let data: super::OsuApiV2User = http::json_body(resp).await?;

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
        cover_url: data.cover.and_then(|c| c.custom_url.or(c.url)),
    })
}

fn url_encode_username(username: &str) -> String {
    if username.chars().all(|c| c.is_ascii_digit()) {
        format!("@{}", username)
    } else {
        urlencoding::encode(username).into_owned()
    }
}

pub async fn fetch_user_stats_by_username(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
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

pub async fn fetch_user_stats_by_user_id(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<UserStats, ApiError> {
    let mode_param = mode.api_value();
    let url = format!("https://osu.ppy.sh/api/v2/users/{}/{}", user_id, mode_param);
    fetch_user_stats_internal(rate_limiter, oauth, &url).await
}

pub async fn backfill_score_details(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<super::oauth::OauthTokenCache>,
    score: &mut Score,
    mode_str: &str,
) {
    if (score.ar == 0.0
        || score.od == 0.0
        || score.star_rating == 0.0
        || score.beatmap_max_combo == 0
        || score.status.is_empty())
        && score.beatmap_id > 0
    {
        if let Ok(bm) = fetch_beatmap(rate_limiter, oauth, score.beatmap_id).await {
            score.ar = bm.ar;
            score.od = bm.od;
            score.cs = bm.cs;
            score.hp = bm.hp;
            score.star_rating = bm.difficulty_rating;
            score.bpm = bm.bpm;
            score.length_seconds = bm.total_length;
            score.beatmap_max_combo = bm.max_combo;
            if score.version.is_empty() {
                score.version = bm.version;
            }
            if score.status.is_empty() {
                score.status = bm.status;
            }
            if score.beatmapset_id == 0 {
                score.beatmapset_id = bm.beatmapset_id;
            }
        }
    }

    if (score.artist.is_empty() || score.title.is_empty() || score.cover_url.is_empty())
        && score.beatmapset_id > 0
    {
        match fetch_beatmapset(rate_limiter, oauth, score.beatmapset_id).await {
            Ok(bs) => {
                score.artist = bs.artist;
                score.title = bs.title;
                score.creator = bs.creator;
                if score.cover_url.is_empty() {
                    score.cover_url = fullsize_cover_url(bs.covers.as_ref()).unwrap_or_default();
                }
                if score.fav_count.is_none() {
                    score.fav_count = Some(bs.favourite_count).filter(|&v| v > 0);
                }
                if score.play_count.is_none() {
                    score.play_count = Some(bs.play_count).filter(|&v| v > 0);
                }
            }
            Err(e) => {
                tracing::warn!(error = ?e, beatmapset_id = score.beatmapset_id, "{}", log_fmt!("api.backfill_beatmapset_failed"));
            }
        }
    }

    if score.score_value == 0 && score.score_id > 0 {
        match fetch_score_detail(rate_limiter, oauth, mode_str, score.score_id).await {
            Ok(Some(val)) => {
                score.score_value = val;
                tracing::trace!(
                    score_id = score.score_id,
                    score_value = val,
                    "{}",
                    log_fmt!("api.backfilled_score_value")
                );
            }
            Ok(None) => {
                tracing::trace!(
                    score_id = score.score_id,
                    "{}",
                    log_fmt!("api.score_detail_no_value")
                );
            }
            Err(e) => {
                tracing::warn!(error = ?e, score_id = score.score_id, "{}", log_fmt!("api.backfill_score_failed"));
            }
        }
    }
}

pub async fn get_user_recent(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<super::oauth::OauthTokenCache>,
    user_id: i64,
    mode: GameMode,
    include_fails: bool,
    limit: u32,
    backfill: bool,
) -> Result<Vec<Score>, ApiError> {
    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/scores/recent?mode={}&include_fails={}&limit={}&legacy_only=0",
        user_id,
        mode.api_value(),
        if include_fails { 1 } else { 0 },
        limit
    );

    let resp = http::authenticated_get(&url, rate_limiter, oauth).await?;

    let raw_json: serde_json::Value = http::json_body(resp).await?;

    let plays: Vec<OsuApiScore> = serde_json::from_value(raw_json).map_err(|e| {
        tracing::error!(error = %e, "{}", log_fmt!("api.parse_score_json_failed"));
        ApiError::InvalidResponse
    })?;
    let mut scores_raw: Vec<Score> = plays
        .into_iter()
        .map(|p| api_score_to_score(p, mode))
        .collect();
    // NOTE: created_at is an ISO 8601 string (YYYY-MM-DDTHH:MM:SSZ). Lexicographic
    // ordering equals chronological ordering only because the format is fixed-width.
    scores_raw.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    if backfill {
        let mode_str = mode.api_value().to_string();
        let scores: Vec<Score> = stream::iter(scores_raw)
            .map(|mut score| {
                let rl = rate_limiter.clone();
                let oa = oauth.clone();
                let ruleset = mode_str.clone();
                async move {
                    backfill_score_details(&rl, &oa, &mut score, &ruleset).await;
                    score
                }
            })
            .buffered(5)
            .collect()
            .await;
        Ok(scores)
    } else {
        Ok(scores_raw)
    }
}

pub async fn get_user_best(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<super::oauth::OauthTokenCache>,
    user_id: i64,
    mode: GameMode,
    limit: u32,
) -> Result<Vec<Score>, ApiError> {
    const PAGE_SIZE: u32 = 100;

    let mut all_scores: Vec<Score> = Vec::new();

    while all_scores.len() < limit as usize {
        let remaining = limit - all_scores.len() as u32;
        let fetch = remaining.min(PAGE_SIZE);
        let offset = all_scores.len() as u32;

        let url = format!(
            "https://osu.ppy.sh/api/v2/users/{}/scores/best?mode={}&limit={}&offset={}&legacy_only=0",
            user_id,
            mode.api_value(),
            fetch,
            offset,
        );

        let resp = http::authenticated_get(&url, rate_limiter, oauth).await?;
        let raw_json: serde_json::Value = http::json_body(resp).await?;

        let plays: Vec<OsuApiScore> = serde_json::from_value(raw_json).map_err(|e| {
            tracing::error!(error = %e, "{}", log_fmt!("api.parse_score_json_failed"));
            ApiError::InvalidResponse
        })?;

        let page_len = plays.len();
        all_scores.extend(plays.into_iter().map(|p| api_score_to_score(p, mode)));

        if page_len < fetch as usize {
            break;
        }
    }

    Ok(all_scores)
}

pub async fn get_user_beatmap_scores_all(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<super::oauth::OauthTokenCache>,
    beatmap_id: i64,
    user_id: i64,
    mode: GameMode,
    limit: Option<u32>,
    backfill: bool,
) -> Result<Vec<Score>, ApiError> {
    let mut url_primary = format!(
        "https://osu.ppy.sh/api/v2/beatmaps/{}/scores/users/{}/all?legacy_only=0",
        beatmap_id, user_id,
    );
    if mode != GameMode::Osu {
        url_primary.push_str(&format!("&mode={}", mode.api_value()));
    }
    if let Some(n) = limit {
        url_primary.push_str(&format!("&limit={}", n));
    }
    let url_retry = format!(
        "https://osu.ppy.sh/api/v2/beatmaps/{}/scores/users/{}/all?legacy_only=1",
        beatmap_id, user_id,
    );

    let process_response = |response: reqwest::Response, limit: Option<u32>| async move {
        let body = response.text().await.map_err(ApiError::Http)?;
        let raw: super::BeatmapScoresResponse = serde_json::from_str(&body).map_err(|e| {
            tracing::error!(error = %e, body, "{}", log_fmt!("api.beatmap_scores_parse_failed"));
            ApiError::InvalidResponse
        })?;
        let scores_raw: Vec<Score> = raw
            .scores
            .into_iter()
            .map(|s| api_score_to_score(s, mode))
            .collect();

        let scores = if backfill {
            let mode_str = mode.api_value().to_string();
            let scores: Vec<Score> = stream::iter(scores_raw)
                .map(|mut score| {
                    let rl = rate_limiter.clone();
                    let oa = oauth.clone();
                    let ruleset = mode_str.clone();
                    async move {
                        backfill_score_details(&rl, &oa, &mut score, &ruleset).await;
                        score
                    }
                })
                .buffered(5)
                .collect()
                .await;
            scores
        } else {
            scores_raw
        };

        if let Some(n) = limit {
            let mut limited = scores;
            limited.truncate(n as usize);
            Ok(limited)
        } else {
            Ok(scores)
        }
    };

    match http::authenticated_get(&url_primary, rate_limiter, oauth).await {
        Ok(resp) => process_response(resp, limit).await,
        Err(ApiError::NotFound) => {
            tracing::debug!(
                beatmap_id,
                user_id,
                ?mode,
                "{}",
                log_fmt!("api.beatmap_scores_404_retry")
            );
            let retry_resp = http::authenticated_get(&url_retry, rate_limiter, oauth).await?;
            process_response(retry_resp, limit).await
        }
        Err(e) => Err(e),
    }
}

pub async fn get_score_by_id(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<super::oauth::OauthTokenCache>,
    score_id: u64,
) -> Result<Score, ApiError> {
    let url = format!("https://osu.ppy.sh/api/v2/scores/{}", score_id,);

    let resp = http::authenticated_get(&url, rate_limiter, oauth).await?;

    let raw: OsuApiScore = http::json_body(resp).await?;
    let mode = raw.extra_mode();
    let mut score = api_score_to_score(raw, mode);
    backfill_score_details(rate_limiter, oauth, &mut score, mode.api_value()).await;
    Ok(score)
}

async fn fetch_score_detail(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    ruleset: &str,
    score_id: i64,
) -> Result<Option<i64>, ApiError> {
    let url = format!("https://osu.ppy.sh/api/v2/scores/{}/{}", ruleset, score_id);
    tracing::trace!(url = %url, "{}", log_fmt!("api.fetch_score_detail"));

    match http::authenticated_get(&url, rate_limiter, oauth).await {
        Ok(resp) => {
            let data: serde_json::Value = http::json_body(resp).await?;
            tracing::trace!(keys = ?data.as_object().map(|o| o.keys().collect::<Vec<_>>()), score = ?data.get("score"), total_score = ?data.get("total_score"), legacy_total_score = ?data.get("legacy_total_score"), classic_total_score = ?data.get("classic_total_score"), "{}", log_fmt!("api.score_detail_response"));
            let score_val = data
                .get("total_score")
                .and_then(|v| v.as_i64())
                .or_else(|| data.get("score").and_then(|v| v.as_i64()))
                .or_else(|| data.get("legacy_total_score").and_then(|v| v.as_i64()))
                .filter(|&v| v > 0);
            Ok(score_val)
        }
        Err(ApiError::NotFound) => {
            tracing::debug!(url = %url, status = "404", "{}", log_fmt!("api.score_detail_404"));
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

async fn fetch_beatmap(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    beatmap_id: i64,
) -> Result<OsuApiBeatmap, ApiError> {
    let url = format!("https://osu.ppy.sh/api/v2/beatmaps/{}", beatmap_id);

    let resp = http::authenticated_get(&url, rate_limiter, oauth).await?;
    http::json_body(resp).await
}

/// 根据 beatmap_id 获取其所属 beatmapset_id（用于构造预览音频 URL）。
pub async fn get_beatmapset_id(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    beatmap_id: i64,
) -> Result<i64, ApiError> {
    Ok(fetch_beatmap(rate_limiter, oauth, beatmap_id)
        .await?
        .beatmapset_id)
}

async fn fetch_beatmapset(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    beatmapset_id: i64,
) -> Result<OsuApiBeatmapset, ApiError> {
    let url = format!("https://osu.ppy.sh/api/v2/beatmapsets/{}", beatmapset_id,);

    let resp = http::authenticated_get(&url, rate_limiter, oauth).await?;
    http::json_body(resp).await
}

pub async fn get_user_info(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    username: &str,
) -> Result<Option<OsuUserInfo>, ApiError> {
    let url_username = url_encode_username(username);
    let url = format!("https://osu.ppy.sh/api/v2/users/{}", url_username);

    match http::authenticated_get(&url, rate_limiter, oauth).await {
        Ok(resp) => {
            let user: OsuUserInfo = http::json_body(resp).await?;
            Ok(Some(user))
        }
        Err(ApiError::NotFound) => Ok(None),
        Err(e) => Err(e),
    }
}

pub async fn fetch_user_profile(
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<super::UserProfile, ApiError> {
    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/{}?key=id",
        user_id,
        mode.api_value()
    );

    let resp = http::authenticated_get(&url, rate_limiter, oauth).await?;

    let data: super::OsuProfileResponse = http::json_body(resp).await?;

    let cover_url = data.cover.and_then(|c| c.custom_url.or(c.url));

    Ok(super::UserProfile {
        html: data.page.html,
        profile_hue: data.profile_hue.unwrap_or(333),
        username: data.username,
        avatar_url: data.avatar_url,
        cover_url,
    })
}
