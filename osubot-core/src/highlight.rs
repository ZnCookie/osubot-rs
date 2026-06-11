use crate::api::{self, ApiError};
use crate::storage::Storage;
use crate::types::{GameMode, UserStats};
use crate::OauthTokenCache;
use crate::RateLimiter;
use chrono::Utc;
use std::sync::Arc;

/// Highlight result for a single user
#[derive(Debug, Clone)]
pub struct UserHighlight {
    pub username: String,
    pub old_pp: f64,
    pub new_pp: f64,
    pub pp_increase: f64,
    pub old_hits: i64,
    pub new_hits: i64,
    pub hits_increase: i64,
    pub old_playtime: i64,
    pub new_playtime: i64,
    pub playtime_increase: i64, // seconds
}

/// Overall highlight result containing top highlights
#[derive(Debug, Default)]
pub struct HighlightResult {
    pub most_pp_increase: Option<UserHighlight>,
    pub most_hits_increase: Option<UserHighlight>,
    pub most_playtime_increase: Option<UserHighlight>,
}

/// Error type for highlight operations
#[derive(Debug)]
pub enum HighlightError {
    Api(ApiError),
    Storage(rusqlite::Error),
    NoData,
}

impl From<ApiError> for HighlightError {
    fn from(e: ApiError) -> Self {
        HighlightError::Api(e)
    }
}

impl From<rusqlite::Error> for HighlightError {
    fn from(e: rusqlite::Error) -> Self {
        HighlightError::Storage(e)
    }
}

/// Get the snapshot closest to 24 hours ago (within 36 hour window) for a user
fn get_baseline_snapshot(
    storage: &Storage,
    user_id: i64,
    mode: GameMode,
) -> Result<Option<UserStats>, rusqlite::Error> {
    let all = storage.get_snapshots_within_hours(user_id, mode, 36)?;

    if all.is_empty() {
        return Ok(None);
    }

    let now = Utc::now();
    let target = now - chrono::Duration::hours(24);

    let closest = all
        .into_iter()
        .min_by_key(|(dt, _)| (*dt - target).num_seconds().unsigned_abs() as i64);

    Ok(closest.map(|(_, stats)| stats))
}

/// Calculate today's highlights for given users
pub async fn get_highlight(
    storage: &Arc<Storage>,
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    user_data: &[(i64, i64, String)], // (qq, user_id, current_username)
    mode: GameMode,
) -> Result<HighlightResult, HighlightError> {
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let semaphore = Arc::new(Semaphore::new(8)); // max 8 concurrent API calls

    let mut join_set = JoinSet::new();
    for (_qq, user_id, username) in user_data {
        let storage = storage.clone();
        let sem = semaphore.clone();
        let rate_limiter = rate_limiter.clone();
        let oauth = oauth.clone();
        let user_id = *user_id;
        let username = username.clone();

        join_set.spawn(async move {
            let baseline = match get_baseline_snapshot(&storage, user_id, mode) {
                Ok(Some(s)) => s,
                Ok(None) => return None,
                Err(e) => {
                    tracing::warn!(
                        ?e,
                        user_id,
                        ?mode,
                        "baseline snapshot query failed, excluding user from highlight"
                    );
                    return None;
                }
            };

            let _permit = sem.acquire().await.expect("semaphore unexpectedly closed");
            let result =
                api::fetch_user_stats_by_user_id(&rate_limiter, &oauth, user_id, mode).await;

            match result {
                Ok(current) => Some(UserHighlight {
                    username,
                    old_pp: baseline.pp,
                    new_pp: current.pp,
                    pp_increase: current.pp - baseline.pp,
                    old_hits: baseline.hits,
                    new_hits: current.hits,
                    hits_increase: current.hits - baseline.hits,
                    old_playtime: baseline.playtime,
                    new_playtime: current.playtime,
                    playtime_increase: current.playtime - baseline.playtime,
                }),
                Err(_) => None,
            }
        });
    }

    let mut user_highlights: Vec<UserHighlight> = Vec::new();
    while let Some(result) = join_set.join_next().await {
        if let Ok(Some(h)) = result {
            user_highlights.push(h);
        }
    }

    if user_highlights.is_empty() {
        return Err(HighlightError::NoData);
    }

    let most_pp_increase = user_highlights
        .iter()
        .filter(|h| h.pp_increase > 0.0)
        .max_by(|a, b| {
            a.pp_increase
                .partial_cmp(&b.pp_increase)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned();

    let most_hits_increase = user_highlights
        .iter()
        .filter(|h| h.hits_increase > 0)
        .max_by_key(|h| h.hits_increase)
        .cloned();

    let most_playtime_increase = user_highlights
        .iter()
        .filter(|h| h.playtime_increase > 0)
        .max_by_key(|h| h.playtime_increase)
        .cloned();

    Ok(HighlightResult {
        most_pp_increase,
        most_hits_increase,
        most_playtime_increase,
    })
}

/// Format highlight result as response string
pub fn format_highlight(result: &HighlightResult) -> String {
    let mut s = String::new();

    s.push_str("最飞升：\n");
    if let Some(h) = &result.most_pp_increase {
        s.push_str(&format!(
            "{} 增加了 {:.2} PP。\n({:.2} -> {:.2})\n",
            h.username, h.pp_increase, h.old_pp, h.new_pp
        ));
    } else {
        s.push_str("你群没有人飞升。\n");
    }

    s.push_str("最肝：\n");
    if let Some(h) = &result.most_hits_increase {
        s.push_str(&format!("{} 打了 {} 下。\n", h.username, h.hits_increase));
    } else {
        s.push_str("你群没有人肝。\n");
    }

    s.push_str("最长游戏时间：\n");
    if let Some(h) = &result.most_playtime_increase {
        let hours = h.playtime_increase as f64 / 3600.0;
        s.push_str(&format!("{} 玩儿了 {:.2} 小时。\n", h.username, hours));
    } else {
        s.push_str("你群没有人玩。\n");
    }

    s
}
