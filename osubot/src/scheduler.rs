use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tokio::time;
use tracing::{error, info, warn};

use osubot_core::api::ApiError;
use osubot_core::{
    api,
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

    /// Evaluate a single user's activity for a single mode (returns UpdateResult)
    async fn eval_activity(
        &self,
        username: &str,
        mode: GameMode,
    ) -> osubot_core::types::UpdateResult {
        let now = Utc::now();

        if !self.rate_limiter.try_acquire().await {
            return osubot_core::types::UpdateResult {
                activity: UserActivity::Inactive,
                added_snapshot: false,
                added_records: 0,
            };
        }

        // 1. Fetch current user stats
        let current =
            match api::fetch_user_stats(&self.rate_limiter, &self.oauth, username, mode).await {
                Ok(stats) => stats,
                Err(ApiError::NotFound) => {
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::UserNotExists,
                        added_snapshot: false,
                        added_records: 0,
                    };
                }
                Err(_) => {
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::Inactive,
                        added_snapshot: false,
                        added_records: 0,
                    };
                }
            };

        // 2. Get latest snapshot
        let latest = self
            .storage
            .get_latest_snapshot(username, mode)
            .unwrap_or_default();

        // 3. [Early判定 Inactive] PlayCount same且 PP == 0
        if let Some(ref snap) = latest {
            if current.playcount == snap.playcount && current.pp == 0.0 {
                return osubot_core::types::UpdateResult {
                    activity: UserActivity::Inactive,
                    added_snapshot: false,
                    added_records: 0,
                };
            }
        }

        // 4. Save new snapshot if stats changed (save first, then calculate change)
        let stats_changed = latest
            .as_ref()
            .is_none_or(|l| l.rank != current.rank || l.pp != current.pp);
        let mut added_snapshot = false;
        if stats_changed && self.storage.save_stats(username, mode, &current).is_ok() {
            added_snapshot = true;
        }

        // 5. Get recent plays and write to database
        let recent_plays =
            match api::get_user_recent(&self.rate_limiter, &self.oauth, username, mode).await {
                Ok(plays) => {
                    info!(
                        "Fetched {} recent plays for {} mode {:?}",
                        plays.len(),
                        username,
                        mode
                    );
                    plays
                }
                Err(e) => {
                    error!("Failed to fetch recent plays: {:?}", e);
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

        info!(
            "Parsed {} valid timestamps for {} mode {:?}",
            records.len(),
            username,
            mode
        );

        let added_records = self
            .storage
            .save_play_records(username, mode, &records)
            .unwrap_or_default();

        if added_records > 0 {
            // Has new records -> Active
            if let Err(e) = self.storage.set_last_update(username, mode, now) {
                warn!("Failed to set last update for {username}/{mode:?}: {e}");
            }
            return osubot_core::types::UpdateResult {
                activity: UserActivity::Active,
                added_snapshot,
                added_records,
            };
        }

        // 6. Check if there are plays within last 4h (no new additions)
        let has_recent = recent_plays.iter().any(|p| {
            DateTime::parse_from_rfc3339(&p.created_at)
                .map(|t| (now - t.with_timezone(&Utc)).num_hours() < 4)
                .unwrap_or(false)
        });
        if has_recent {
            if let Err(e) = self.storage.set_last_update(username, mode, now) {
                warn!("Failed to set last update for {username}/{mode:?}: {e}");
            }
            return osubot_core::types::UpdateResult {
                activity: UserActivity::SemiActive,
                added_snapshot,
                added_records: 0,
            };
        }

        // 7. Calculate change (based on newly saved or old snapshot)
        let change = self
            .storage
            .calculate_change(username, mode, &current)
            .unwrap_or_default();

        // 8. Check last update time
        let last_update = self
            .storage
            .get_last_update(username, mode)
            .unwrap_or_default();
        let hours_since_update = last_update
            .map(|t| (now - t).num_hours())
            .unwrap_or(i64::MAX);

        if change.as_ref().map(|c| c.has_changes()).unwrap_or(false) {
            // Has changes
            if let Err(e) = self.storage.set_last_update(username, mode, now) {
                warn!("Failed to set last update for {username}/{mode:?}: {e}");
            }
            if hours_since_update < 4 {
                return osubot_core::types::UpdateResult {
                    activity: UserActivity::NoRecent,
                    added_snapshot: true,
                    added_records: 0,
                };
            }
            return osubot_core::types::UpdateResult {
                activity: UserActivity::Normal,
                added_snapshot: true,
                added_records: 0,
            };
        }

        // No changes
        if hours_since_update < 8 {
            return osubot_core::types::UpdateResult {
                activity: UserActivity::NoRecent,
                added_snapshot: false,
                added_records: 0,
            };
        }
        if hours_since_update < 48 {
            if let Err(e) = self.storage.set_last_update(username, mode, now) {
                warn!("Failed to set last update for {username}/{mode:?}: {e}");
            }
            return osubot_core::types::UpdateResult {
                activity: UserActivity::Normal,
                added_snapshot: false,
                added_records: 0,
            };
        }
        osubot_core::types::UpdateResult {
            activity: UserActivity::Inactive,
            added_snapshot: false,
            added_records: 0,
        }
    }

    /// Return next update interval based on activity
    fn get_update_interval(&self, activity: UserActivity) -> Duration {
        match activity {
            UserActivity::Active => Duration::hours(self.config.active_interval_hours),
            UserActivity::SemiActive => Duration::hours(self.config.semi_active_interval_hours),
            UserActivity::Normal => Duration::hours(self.config.normal_interval_hours),
            UserActivity::Inactive => Duration::hours(self.config.inactive_interval_hours),
            UserActivity::NoRecent => Duration::hours(6),
            UserActivity::UserNotExists => Duration::hours(24),
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
}
