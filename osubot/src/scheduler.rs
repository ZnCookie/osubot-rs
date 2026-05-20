use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tokio::time;
use tracing::{error, info, warn};

use osubot_core::api::ApiError;
use osubot_core::{
    api,
    storage::today_0am_utc,
    types::{GameMode, UserActivity},
    OauthTokenCache, RateLimiter, Storage,
};

use crate::config::SchedulerConfig;

#[derive(Clone)]
pub struct Scheduler {
    storage: Arc<Storage>,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    config: SchedulerConfig,
    last_cleanup: Arc<TokioMutex<Option<DateTime<Utc>>>>,
}

impl Scheduler {
    pub fn new(
        storage: Arc<Storage>,
        oauth: Arc<OauthTokenCache>,
        rate_limiter: Arc<RateLimiter>,
        config: SchedulerConfig,
    ) -> Self {
        Self {
            storage,
            oauth,
            rate_limiter,
            config,
            last_cleanup: Arc::new(TokioMutex::new(None)),
        }
    }

    /// Try to run cleanup if 24h have passed since last run.
    async fn try_cleanup(&self) {
        let mut last = self.last_cleanup.lock().await;
        let now = Utc::now();

        if let Some(last_run) = *last {
            if (now - last_run).num_hours() < 24 {
                return;
            }
        }

        match self.storage.prune_old_records(self.config.retention_days) {
            Ok((stats, plays, next)) => {
                info!(
                    deleted_stats = stats,
                    deleted_plays = plays,
                    deleted_next = next,
                    "pruned old records"
                );
            }
            Err(e) => {
                error!(error = ?e, "failed to prune old records");
            }
        }

        // Also prune expired pending binds
        match self.storage.prune_expired_pending_binds() {
            Ok(deleted) if deleted > 0 => {
                info!(deleted, "pruned expired pending binds");
            }
            _ => {}
        }

        *last = Some(now);
    }

    /// Background task entry point - only processes due users/modes
    pub async fn run(&self) {
        info!("Scheduler task started");
        loop {
            info!("Scheduler tick");
            time::sleep(time::Duration::from_secs(60 * self.config.interval_minutes)).await;
            self.process_due_users().await;
        }
    }

    /// Process all due users/modes (next_update <= now), then set new next_update
    async fn process_due_users(&self) {
        let due = match self.storage.get_due_users() {
            Ok(d) => d,
            Err(e) => {
                error!("get_due_users failed: {:?}", e);
                return;
            }
        };
        info!("due users count: {}", due.len());

        for (username, mode) in due {
            let result = self.eval_activity(&username, mode).await;
            self.update_next_time(&username, mode, result.activity);
        }

        self.try_cleanup().await;
    }

    /// Evaluate a single user's activity for a single mode
    async fn eval_activity(
        &self,
        username: &str,
        mode: GameMode,
    ) -> osubot_core::types::UpdateResult {
        let now = Utc::now();

        // Check rate limit before API calls
        if !self.rate_limiter.try_acquire().await {
            return osubot_core::types::UpdateResult {
                activity: UserActivity::NoRecent,
                added_snapshot: false,
            };
        }

        // Always fetch current stats and recent plays (API calls)
        let current =
            match api::fetch_user_stats(&self.rate_limiter, &self.oauth, username, mode).await {
                Ok(stats) => stats,
                Err(ApiError::NotFound) => {
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::UserNotExists,
                        added_snapshot: false,
                    };
                }
                Err(_) => {
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::Inactive,
                        added_snapshot: false,
                    };
                }
            };

        // Save snapshot only when stats changed (rank or playcount differ)
        let added_snapshot = match self.storage.get_latest_snapshot(username, mode) {
            Ok(Some(prev)) => {
                if prev.rank != current.rank || prev.playcount != current.playcount {
                    self.storage.save_stats(username, mode, &current).is_ok()
                } else {
                    false
                }
            }
            Ok(None) => self.storage.save_stats(username, mode, &current).is_ok(),
            Err(e) => {
                warn!("get_latest_snapshot error for {username}/{mode:?}: {e}");
                self.storage.save_stats(username, mode, &current).is_ok()
            }
        };

        // Always fetch and save recent plays
        let recent_plays = match self.resolve_user_id(username).await {
            Some(user_id) => {
                match api::get_user_recent(&self.rate_limiter, &self.oauth, user_id, mode).await {
                    Ok(plays) => plays,
                    Err(e) => {
                        error!("Failed to fetch recent plays for {username}: {e:?}");
                        Vec::new()
                    }
                }
            }
            None => {
                warn!("Skipping recent plays for {username}: no user_id available");
                Vec::new()
            }
        };

        // Convert API response to storage format (Unix timestamps)
        let records: Vec<i64> = recent_plays
            .iter()
            .filter_map(|p| {
                let ts = DateTime::parse_from_rfc3339(&p.created_at)
                    .ok()?
                    .timestamp();
                Some(ts)
            })
            .collect();

        if let Err(e) = self.storage.save_play_records(username, mode, &records) {
            error!("Failed to save play records for {username}/{mode:?}: {e}");
        }

        // Activity determination based on play records (no more API calls needed)
        let has_recent = self
            .storage
            .has_play_since(username, mode, (now - Duration::hours(4)).timestamp())
            .unwrap_or(false);

        let has_today = self
            .storage
            .has_play_since(username, mode, today_0am_utc())
            .unwrap_or(false);

        let activity = if has_recent {
            UserActivity::SemiActive
        } else if has_today {
            UserActivity::Normal
        } else {
            // No play records today - use last_update time for fallback.
            // ApiError::NotFound already handles genuinely non-existent users above.
            let last_update = self
                .storage
                .get_last_update(username, mode)
                .unwrap_or_default();
            let hours_since = last_update
                .map(|t| (now - t).num_hours())
                .unwrap_or(i64::MAX);

            if hours_since < 8 {
                UserActivity::NoRecent
            } else if hours_since < 48 {
                UserActivity::Normal
            } else {
                UserActivity::Inactive
            }
        };

        // Update last_update only for actual play activity (not stale fallback Normal),
        // so the fallback branches can measure time-since-last-activity correctly.
        if activity == UserActivity::SemiActive || (activity == UserActivity::Normal && has_today) {
            if let Err(e) = self.storage.set_last_update(username, mode, now) {
                warn!("Failed to set last update for {username}/{mode:?}: {e}");
            }
        }

        osubot_core::types::UpdateResult {
            activity,
            added_snapshot,
        }
    }

    /// Return next update interval based on activity
    fn get_update_interval(&self, activity: UserActivity) -> Duration {
        match activity {
            UserActivity::SemiActive => Duration::hours(self.config.semi_active_interval_hours),
            UserActivity::Normal => Duration::hours(self.config.normal_interval_hours),
            UserActivity::Inactive => Duration::hours(self.config.inactive_interval_hours),
            UserActivity::NoRecent => Duration::hours(self.config.no_recent_interval_hours),
            UserActivity::UserNotExists => {
                Duration::hours(self.config.user_not_exists_interval_hours)
            }
        }
    }

    /// Update user's next update time (called after eval)
    fn update_next_time(&self, username: &str, mode: GameMode, activity: UserActivity) {
        let interval = self.get_update_interval(activity);
        let next = Utc::now() + interval;
        let _ = self.storage.set_next_update(username, mode, next);
    }

    /// Trigger update for user (all 4 modes)
    pub fn trigger_update(&self, username: &str) {
        for mode in [
            GameMode::Osu,
            GameMode::Taiko,
            GameMode::Catch,
            GameMode::Mania,
        ] {
            // Check cooldown
            if !self.is_in_cooldown(username, mode) {
                // Set next_update to now (immediate)
                let _ = self.storage.set_next_update(username, mode, Utc::now());
            }
        }
    }

    /// Check if user/mode is in cooldown
    pub fn is_in_cooldown(&self, username: &str, mode: GameMode) -> bool {
        if let Ok(Some(last_update)) = self.storage.get_last_update(username, mode) {
            let cooldown = Duration::hours(self.config.group_trigger_cooldown_hours);
            let now = Utc::now();
            if now - last_update < cooldown {
                return true;
            }
        }
        false
    }

    /// Get cached user_id, or fetch and cache on the fly
    async fn resolve_user_id(&self, username: &str) -> Option<i64> {
        match self.storage.get_user_id(username) {
            Ok(Some(id)) if id != 0 => return Some(id),
            Err(e) => warn!("Failed to look up user_id for {username}: {e}"),
            _ => {}
        }

        // On-the-fly fallback: fetch from API and cache
        match api::get_user_info(&self.rate_limiter, &self.oauth, username).await {
            Ok(Some(info)) => {
                if let Err(e) = self.storage.set_user_id(username, info.id) {
                    warn!("Failed to cache user_id for {username}: {e}");
                }
                Some(info.id)
            }
            Ok(None) => {
                warn!("User {username} not found on osu! (resolve_user_id)");
                None
            }
            Err(e) => {
                warn!("Failed to fetch user info for {username}: {e}");
                None
            }
        }
    }
}
