use chrono::{DateTime, Duration, Utc};
use std::sync::Arc;
use tokio::time;

use osubot_core::{
    api, types::{GameMode, UserActivity, UserChange},
    Storage,
};
use osubot_core::api::ApiError;

use crate::config::SchedulerConfig;

#[derive(Clone)]
pub struct Scheduler {
    storage: Arc<Storage>,
    api_key: String,
    client_id: String,
    config: SchedulerConfig,
}

impl Scheduler {
    pub fn new(storage: Arc<Storage>, api_key: String, client_id: String, config: SchedulerConfig) -> Self {
        Self {
            storage,
            api_key,
            client_id,
            config,
        }
    }

    /// Background task entry point - only processes due users/modes
    pub async fn run(&self) {
        loop {
            time::sleep(time::Duration::from_secs(60 * self.config.interval_minutes)).await;
            self.process_due_users().await;
        }
    }

    /// Process all due users/modes (next_update <= now), then set new next_update
    async fn process_due_users(&self) {
        let due = match self.storage.get_due_users() {
            Ok(d) => d,
            Err(_) => return,
        };

        for (username, mode) in due {
            let result = self.eval_activity(&username, mode).await;
            self.update_next_time(&username, mode, result.activity);
        }
    }

    /// Evaluate a single user's activity for a single mode (returns UpdateResult)
    async fn eval_activity(&self, username: &str, mode: GameMode) -> osubot_core::types::UpdateResult {
        // 1. Fetch current user stats
        let current = match api::fetch_user_stats(&self.api_key, &self.client_id, username, mode).await {
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
        let latest = match self.storage.get_latest_snapshot(username, mode) {
            Ok(snap) => snap,
            Err(_) => None,
        };

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
        let stats_changed = latest.as_ref().map_or(true, |l| l.rank != current.rank || l.pp != current.pp);
        let mut added_snapshot = false;
        if stats_changed {
            if self.storage.save_stats(username, mode, &current).is_ok() {
                added_snapshot = true;
            }
        }

        // 5. Get recent plays and write to database
        let recent_plays = match api::get_user_recent(&self.api_key, &self.client_id, username, mode).await {
            Ok(plays) => plays,
            Err(_) => Vec::new(),
        };

        // Convert API response to storage format (DateTime, timestamp)
        let records: Vec<(DateTime<Utc>, i64)> = recent_plays
            .iter()
            .map(|p| (Utc::now(), p.beatmap.lastplayed))
            .collect();

        let added_records = match self.storage.save_play_records(username, mode, &records) {
            Ok(count) => count,
            Err(_) => 0,
        };

        if added_records > 0 {
            // Has new records -> Active
            return osubot_core::types::UpdateResult {
                activity: UserActivity::Active,
                added_snapshot,
                added_records,
            };
        }

        // 6. Check if there are plays within last 4h (no new additions)
        let now = Utc::now();
        let has_recent = recent_plays.iter().any(|p| (now.timestamp() - p.beatmap.lastplayed) < 4 * 3600);
        if has_recent {
            return osubot_core::types::UpdateResult {
                activity: UserActivity::SemiActive,
                added_snapshot,
                added_records: 0,
            };
        }

        // 7. Calculate change (based on newly saved or old snapshot)
        let change = match self.storage.calculate_change(username, mode, &current) {
            Ok(c) => c,
            Err(_) => None,
        };

        // 8. Check last update time
        let last_update = match self.storage.get_last_update(username, mode) {
            Ok(t) => t,
            Err(_) => None,
        };
        let hours_since_update = last_update.map(|t| (now - t).num_hours()).unwrap_or(i64::MAX);

        if change.as_ref().map(|c| c.has_changes()).unwrap_or(false) {
            // Has changes
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

    /// Get user's change for a mode 4h ago snapshot and calculate change
    pub async fn get_user_change(&self, username: &str, mode: GameMode) -> Option<UserChange> {
        // Get current stats first to pass to calculate_change
        let current = match api::fetch_user_stats(&self.api_key, &self.client_id, username, mode).await {
            Ok(stats) => stats,
            Err(_) => return None,
        };

        self.storage.calculate_change(username, mode, &current).ok().flatten()
    }

    /// Trigger update for user (all 4 modes)
    pub fn trigger_update(&self, username: &str) {
        for mode in [GameMode::Osu, GameMode::Taiko, GameMode::Catch, GameMode::Mania] {
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