use chrono::{DateTime, Duration, Utc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;
use tokio::time;
use tracing::{error, info, warn};

use osubot_core::api::ApiError;
use osubot_core::log_fmt;
use osubot_core::{
    api,
    storage::today_0am_utc,
    types::{GameMode, UserActivity},
    OauthTokenCache, RateLimiter, Storage,
};

use crate::config::Config;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct Scheduler {
    storage: Arc<Storage>,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    config: Arc<RwLock<Config>>,
    last_cleanup: Arc<TokioMutex<Option<DateTime<Utc>>>>,
    shutdown: Arc<AtomicBool>,
}

impl Scheduler {
    pub fn new(
        storage: Arc<Storage>,
        oauth: Arc<OauthTokenCache>,
        rate_limiter: Arc<RateLimiter>,
        config: Arc<RwLock<Config>>,
    ) -> Self {
        Self {
            storage,
            oauth,
            rate_limiter,
            config,
            last_cleanup: Arc::new(TokioMutex::new(None)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Try to run cleanup if 24h have passed since last run.
    async fn try_cleanup(&self) {
        let scheduler_cfg = self.config.read().await.scheduler.clone();
        let mut last = self.last_cleanup.lock().await;
        let now = Utc::now();

        if let Some(last_run) = *last {
            if (now - last_run).num_hours() < 24 {
                return;
            }
        }

        match self
            .storage
            .prune_old_records(scheduler_cfg.retention_days)
            .await
        {
            Ok((stats, plays, next)) => {
                info!(
                    deleted_stats = stats,
                    deleted_plays = plays,
                    deleted_next = next,
                    "{}",
                    log_fmt!("scheduler.pruned_records")
                );
            }
            Err(e) => {
                error!(error = ?e, "{}", log_fmt!("scheduler.prune_records_failed"));
            }
        }

        // Also prune expired pending binds
        match self.storage.prune_expired_pending_binds().await {
            Ok(deleted) if deleted > 0 => {
                info!(deleted, "{}", log_fmt!("scheduler.pruned_pending_binds"));
            }
            Ok(_) => {}
            Err(e) => {
                error!(error = ?e, "{}", log_fmt!("scheduler.prune_pending_binds_failed"));
            }
        }

        // Prune expired pending unbinds
        match self.storage.prune_expired_pending_unbinds().await {
            Ok(deleted) if deleted > 0 => {
                info!(deleted, "{}", log_fmt!("scheduler.pruned_pending_unbinds"));
            }
            Ok(_) => {}
            Err(e) => {
                error!(error = ?e, "{}", log_fmt!("scheduler.prune_pending_unbinds_failed"));
            }
        }

        osubot_render::cleanup_expired(scheduler_cfg.cache_retention_days).await;
        osubot_core::cache::cleanup_replays(scheduler_cfg.cache_retention_days).await;
        osubot_core::cache::cleanup_beatmaps(scheduler_cfg.cache_retention_days).await;
        osubot_core::cache::cleanup_previews(scheduler_cfg.cache_retention_days).await;

        *last = Some(now);
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Background task entry point - only processes due users/modes
    pub async fn run(&self) {
        info!("{}", log_fmt!("scheduler.task_started"));
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                info!("{}", log_fmt!("scheduler.shutting_down"));
                break;
            }
            info!("{}", log_fmt!("scheduler.tick"));
            let interval_secs = {
                let cfg = self.config.read().await;
                60 * cfg.scheduler.interval_minutes.max(1)
            };
            tokio::select! {
                _ = time::sleep(time::Duration::from_secs(interval_secs)) => {}
                _ = wait_for_shutdown(self.shutdown.clone()) => break,
            }
            self.process_due_users().await;
            self.try_cleanup().await;
        }
    }

    /// Process all due users/modes (next_update <= now), then set new next_update
    async fn process_due_users(&self) {
        let due = match self.storage.get_due_users().await {
            Ok(d) => d,
            Err(e) => {
                error!(
                    "{}",
                    log_fmt!("scheduler.get_due_users_failed", error = format!("{:?}", e))
                );
                Vec::new()
            }
        };
        info!(
            "{}",
            log_fmt!("scheduler.due_users_count", count = due.len())
        );

        for (user_id, mode) in due {
            let result = self.eval_activity(user_id, mode).await;
            if result.success {
                self.update_next_time(user_id, mode, result.activity).await;
            } else {
                // Retry on next tick — rate limiter naturally throttles persistent failures
                if let Err(e) = self
                    .storage
                    .set_next_update(user_id, mode, Utc::now())
                    .await
                {
                    warn!(
                        "{}",
                        log_fmt!(
                            "scheduler.set_next_update_failed",
                            user_id = user_id,
                            mode = format!("{:?}", mode),
                            error = &e
                        )
                    );
                }
            }
        }
    }

    /// Evaluate a single user's activity for a single mode
    async fn eval_activity(
        &self,
        user_id: i64,
        mode: GameMode,
    ) -> osubot_core::types::UpdateResult {
        let now = Utc::now();

        // Check rate limit before API calls
        if !self.rate_limiter.try_acquire().await {
            return osubot_core::types::UpdateResult {
                activity: UserActivity::NoRecent,
                success: false,
            };
        }

        // Always fetch current stats and recent plays (API calls)
        let current =
            match api::fetch_user_stats_by_user_id(&self.rate_limiter, &self.oauth, user_id, mode)
                .await
            {
                Ok(stats) => {
                    // Username change detection — update bindings if user renamed
                    if let Ok(updated) = self
                        .storage
                        .update_binding_username_by_user_id(user_id, &stats.username)
                        .await
                    {
                        if updated > 0 {
                            info!(
                                user_id = user_id,
                                new_username = %stats.username,
                                updated_bindings = updated,
                                "{}",
                                log_fmt!("scheduler.username_change_detected")
                            );
                        }
                    }
                    // Refresh username→user_id cache
                    self.storage
                        .set_user_id(&stats.username, user_id)
                        .await
                        .ok();
                    stats
                }
                Err(ApiError::NotFound) => {
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::UserNotExists,
                        success: true,
                    };
                }
                Err(ApiError::RateLimitedWithRetryAfter(_)) => {
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::NoRecent,
                        success: false,
                    };
                }
                Err(ApiError::ClientRateLimited) => {
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::NoRecent,
                        success: false,
                    };
                }
                Err(_) => {
                    // Other transient errors: use shorter retry interval (6h instead of 48h)
                    return osubot_core::types::UpdateResult {
                        activity: UserActivity::NoRecent,
                        success: false,
                    };
                }
            };

        // Save snapshot only when stats changed (rank or playcount differ)
        match self.storage.get_latest_snapshot(user_id, mode).await {
            Ok(Some(prev)) => {
                if prev.rank != current.rank || prev.playcount != current.playcount {
                    if let Err(e) = self.storage.save_stats(user_id, mode, &current).await {
                        warn!(
                            "{}",
                            log_fmt!(
                                "scheduler.save_stats_error",
                                user_id = user_id,
                                mode = format!("{:?}", mode),
                                error = &e
                            )
                        );
                    }
                }
            }
            Ok(None) => {
                if let Err(e) = self.storage.save_stats(user_id, mode, &current).await {
                    warn!(
                        "{}",
                        log_fmt!(
                            "scheduler.save_stats_error",
                            user_id = user_id,
                            mode = format!("{:?}", mode),
                            error = &e
                        )
                    );
                }
            }
            Err(e) => {
                warn!(
                    "{}",
                    log_fmt!(
                        "scheduler.get_snapshot_error",
                        user_id = user_id,
                        mode = format!("{:?}", mode),
                        error = &e
                    )
                );
                if let Err(e) = self.storage.save_stats(user_id, mode, &current).await {
                    warn!(
                        "{}",
                        log_fmt!(
                            "scheduler.save_stats_error",
                            user_id = user_id,
                            mode = format!("{:?}", mode),
                            error = &e
                        )
                    );
                }
            }
        }

        // Always fetch and save recent plays (get_user_recent already takes user_id)
        let recent_plays =
            match api::get_user_recent(&self.rate_limiter, &self.oauth, user_id, mode, false, 100)
                .await
            {
                Ok(plays) => plays,
                Err(e) => {
                    error!(
                        "{}",
                        log_fmt!(
                            "scheduler.fetch_recent_failed",
                            user_id = user_id,
                            error = format!("{:?}", e)
                        )
                    );
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

        if let Err(e) = self
            .storage
            .save_play_records(user_id, mode, &records)
            .await
        {
            error!(
                "{}",
                log_fmt!(
                    "scheduler.save_records_failed",
                    user_id = user_id,
                    mode = format!("{:?}", mode),
                    error = &e
                )
            );
        }

        // Activity determination based on play records (no more API calls needed)
        let has_recent = self
            .storage
            .has_play_since(user_id, mode, (now - Duration::hours(4)).timestamp())
            .await
            .unwrap_or(false);

        let has_today = self
            .storage
            .has_play_since(user_id, mode, today_0am_utc())
            .await
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
                .get_last_update(user_id, mode)
                .await
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
            if let Err(e) = self.storage.set_last_update(user_id, mode, now).await {
                warn!(
                    "{}",
                    log_fmt!(
                        "scheduler.set_last_update_failed",
                        user_id = user_id,
                        mode = format!("{:?}", mode),
                        error = &e
                    )
                );
            }
        }

        osubot_core::types::UpdateResult {
            activity,
            success: true,
        }
    }

    /// Return next update interval based on activity
    async fn get_update_interval(&self, activity: UserActivity) -> Duration {
        let cfg = self.config.read().await.scheduler.clone();
        match activity {
            UserActivity::SemiActive => Duration::hours(cfg.semi_active_interval_hours),
            UserActivity::Normal => Duration::hours(cfg.normal_interval_hours),
            UserActivity::Inactive => Duration::hours(cfg.inactive_interval_hours),
            UserActivity::NoRecent => Duration::hours(cfg.no_recent_interval_hours),
            UserActivity::UserNotExists => Duration::hours(cfg.user_not_exists_interval_hours),
        }
    }

    /// Update user's next update time (called after eval)
    async fn update_next_time(&self, user_id: i64, mode: GameMode, activity: UserActivity) {
        let interval = self.get_update_interval(activity).await;
        let next = Utc::now() + interval;
        if let Err(e) = self.storage.set_next_update(user_id, mode, next).await {
            warn!(
                "{}",
                log_fmt!(
                    "scheduler.set_next_update_failed",
                    user_id = user_id,
                    mode = format!("{:?}", mode),
                    error = &e
                )
            );
        }
    }

    /// Trigger update for user (single mode only)
    pub async fn trigger_update(&self, user_id: i64, mode: GameMode) {
        if !self.is_in_cooldown(user_id, mode).await {
            if let Err(e) = self
                .storage
                .set_next_update(user_id, mode, Utc::now())
                .await
            {
                warn!(
                    "{}",
                    log_fmt!(
                        "scheduler.set_next_update_failed",
                        user_id = user_id,
                        mode = format!("{:?}", mode),
                        error = &e
                    )
                );
            }
        }
    }

    pub async fn reschedule_all(&self) {
        match self.storage.reset_all_next_updates().await {
            Ok(0) => {}
            Ok(count) => info!(count, "{}", log_fmt!("scheduler.config_changed_reset")),
            Err(e) => warn!(
                "{}",
                log_fmt!("scheduler.reset_schedule_failed", error = &e)
            ),
        }
    }

    /// Check if user/mode is in cooldown
    pub async fn is_in_cooldown(&self, user_id: i64, mode: GameMode) -> bool {
        if let Ok(Some(last_update)) = self.storage.get_last_update(user_id, mode).await {
            let cooldown = {
                let cfg = self.config.read().await;
                Duration::hours(cfg.scheduler.group_trigger_cooldown_hours)
            };
            let now = Utc::now();
            if now - last_update < cooldown {
                return true;
            }
        }
        false
    }
}

// NOTE: polls the shutdown flag every 200ms rather than using tokio::sync::Notify.
// The flag is an Arc<AtomicBool> shared with several subsystems; a 200ms shutdown
// latency is acceptable and avoids threading a Notify through every caller.
async fn wait_for_shutdown(flag: Arc<AtomicBool>) {
    loop {
        if flag.load(Ordering::Acquire) {
            return;
        }
        time::sleep(time::Duration::from_millis(200)).await;
    }
}
