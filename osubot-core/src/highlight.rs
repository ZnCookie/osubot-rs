use crate::api::{self, ApiError};
use crate::log_fmt;
use crate::storage::Storage;
use crate::strings::user_str;
use crate::types::{GameMode, Server};
use crate::OauthTokenCache;
use crate::RateLimiter;
use std::collections::HashMap;
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
    Storage(turso::Error),
    NoData,
}

impl From<ApiError> for HighlightError {
    fn from(e: ApiError) -> Self {
        HighlightError::Api(e)
    }
}

impl From<turso::Error> for HighlightError {
    fn from(e: turso::Error) -> Self {
        HighlightError::Storage(e)
    }
}

/// Calculate today's highlights for given users
pub async fn get_highlight(
    storage: &Arc<Storage>,
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    user_data: &[(i64, i64, String)], // (qq, user_id, current_username)
    mode: GameMode,
    server: Server,
) -> Result<HighlightResult, HighlightError> {
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    let user_ids: Vec<i64> = user_data.iter().map(|(_, user_id, _)| *user_id).collect();
    let baselines = match storage
        .get_baseline_snapshots_for_users(&user_ids, mode, 24, 36, server)
        .await
    {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(
                ?e,
                ?mode,
                user_count = user_ids.len(),
                "{}",
                log_fmt!("highlight.baseline_query_failed")
            );
            let mut map = HashMap::new();
            for (_, user_id, _) in user_data {
                if let Ok(Some(s)) = storage.get_baseline_snapshot(*user_id, mode, server).await {
                    map.insert(*user_id, s);
                }
            }
            map
        }
    };

    let semaphore = Arc::new(Semaphore::new(8)); // max 8 concurrent API calls
    let mut join_set = JoinSet::new();
    for (_qq, user_id, username) in user_data {
        let Some(baseline) = baselines.get(user_id).cloned() else {
            continue;
        };
        let sem = semaphore.clone();
        let rate_limiter = rate_limiter.clone();
        let oauth = oauth.clone();
        let user_id = *user_id;
        let username = username.clone();

        join_set.spawn(async move {
            let _permit = match sem.acquire().await {
                Ok(p) => p,
                Err(_) => {
                    tracing::warn!(user_id, "highlight semaphore closed, skipping user");
                    return None;
                }
            };
            let result =
                api::fetch_user_stats_by_user_id(&rate_limiter, &oauth, user_id, mode).await;

            match result {
                Ok(current) => Some(UserHighlight {
                    username,
                    old_pp: baseline.pp.unwrap_or(0.0),
                    new_pp: current.pp,
                    pp_increase: current.pp - baseline.pp.unwrap_or(0.0),
                    old_hits: baseline.hits.unwrap_or(0),
                    new_hits: current.hits,
                    hits_increase: current.hits - baseline.hits.unwrap_or(0),
                    old_playtime: baseline.playtime.unwrap_or(0),
                    new_playtime: current.playtime,
                    playtime_increase: current.playtime - baseline.playtime.unwrap_or(0),
                }),
                Err(e) => {
                    tracing::warn!(?e, user_id, "{}", log_fmt!("highlight.fetch_failed"));
                    None
                }
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
#[must_use]
pub fn format_highlight(result: &HighlightResult) -> String {
    let mut s = String::new();

    s.push_str(user_str("highlight.most_pp"));
    if let Some(h) = &result.most_pp_increase {
        let pp = format!("{:.2}", h.pp_increase);
        let old = format!("{:.2}", h.old_pp);
        let new = format!("{:.2}", h.new_pp);
        let t = user_str("highlight.pp_increase");
        let t = t.replace("{username}", &h.username);
        let t = t.replace("{pp}", &pp);
        let t = t.replace("{old}", &old);
        let t = t.replace("{new}", &new);
        s.push_str(&t);
    } else {
        s.push_str(user_str("highlight.no_pp"));
    }

    s.push_str(user_str("highlight.most_hits"));
    if let Some(h) = &result.most_hits_increase {
        let t = user_str("highlight.hits_increase");
        let t = t.replace("{username}", &h.username);
        let t = t.replace("{hits}", &h.hits_increase.to_string());
        s.push_str(&t);
    } else {
        s.push_str(user_str("highlight.no_hits"));
    }

    s.push_str(user_str("highlight.most_playtime"));
    if let Some(h) = &result.most_playtime_increase {
        let hours = format!("{:.2}", h.playtime_increase as f64 / 3600.0);
        let t = user_str("highlight.playtime_increase");
        let t = t.replace("{username}", &h.username);
        let t = t.replace("{hours}", &hours);
        s.push_str(&t);
    } else {
        s.push_str(user_str("highlight.no_playtime"));
    }

    s
}
