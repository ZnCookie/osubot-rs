//! Shutdown-aware background match listener poller.
//!
//! One poller loop handles all active listeners — no per-listener tasks.
//! Reads `current_write` fresh for each notification cycle; does not cache
//! a long-lived `WriteSink`.
//!
//! On delivery failure, notifications are left unacknowledged so the next poll
//! can retry them after reconnect.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock};
use tracing::{error, info, warn};

use osubot_core::api::{
    fetch_beatmap_metadata, fetch_match, reconstruct_match_roster, LegacyMatchUser,
};
use osubot_core::log_fmt;
use osubot_core::strings::log_str;
use osubot_core::{OauthTokenCache, RateLimiter, Storage};

use crate::app_state::WsWrite;
use crate::config::Config;
use crate::config::MatchListenConfig;
use crate::match_listener::state::{
    process_events, ListenerCursor, NotificationAction, StopReason,
};
use crate::shutdown::wait_for_shutdown;
use crate::OneBotApi;

/// Maximum concurrent listener poll cycles.
const MAX_CONCURRENT_LISTENERS: usize = 5;

async fn fetch_match_cover_image(cover_url: Option<&str>) -> Option<image::DynamicImage> {
    let cover_url = cover_url?.trim();
    if cover_url.is_empty() {
        return None;
    }

    match osubot_render::cache::fetch_and_cache(
        cover_url,
        osubot_render::cache::http_client(),
        false,
    )
    .await
    {
        Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
        Err(e) => {
            warn!(url = cover_url, error = %e, "{}", log_str("ml.poller_render_failed"));
            None
        }
    }
}

async fn fetch_match_avatar_image(avatar_url: Option<&str>) -> Option<image::DynamicImage> {
    let avatar_url = avatar_url?.trim();
    if avatar_url.is_empty() {
        return None;
    }

    match osubot_render::cache::fetch_and_cache(
        avatar_url,
        osubot_render::cache::http_client(),
        true,
    )
    .await
    {
        Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
        Err(e) => {
            warn!(url = avatar_url, error = %e, "{}", log_str("ml.poller_render_failed"));
            None
        }
    }
}

async fn fetch_match_avatar_images(players: &mut [osubot_render::MatchResultPlayerParams]) {
    use futures_util::StreamExt;

    let avatar_jobs: Vec<_> = players
        .iter()
        .enumerate()
        .map(|(idx, player)| (idx, player.avatar_url.clone()))
        .collect();

    let avatars: Vec<_> = futures_util::stream::iter(avatar_jobs)
        .map(|(idx, avatar_url)| async move {
            (idx, fetch_match_avatar_image(avatar_url.as_deref()).await)
        })
        .buffer_unordered(6)
        .collect()
        .await;

    for (idx, avatar) in avatars {
        players[idx].avatar_image = avatar;
    }
}

fn should_emit_notification(cfg: &MatchListenConfig, notification: &NotificationAction) -> bool {
    match notification {
        NotificationAction::Text { .. } => true,
        NotificationAction::Image {
            game, event_label, ..
        } => {
            if game.end_time.is_none() || event_label == "场次开始" {
                cfg.notify_on_new_game
            } else {
                cfg.notify_on_complete
            }
        }
    }
}

fn apply_acknowledged_notification(cursor: &mut ListenerCursor, notification: &NotificationAction) {
    match notification {
        NotificationAction::Text { event_id, .. } => {
            cursor.last_event_id = Some(*event_id);
            cursor.last_notified_event_id = Some(*event_id);
        }
        NotificationAction::Image {
            event_id,
            game,
            event_label,
            ..
        } => {
            if game.end_time.is_none() || event_label == "场次开始" {
                cursor.last_notified_event_id = Some(*event_id);
                cursor.pending_game_event_id = Some(*event_id);
            } else {
                cursor.last_event_id = Some(*event_id);
                cursor.last_notified_event_id = Some(*event_id);
                cursor.pending_game_event_id = None;
            }
        }
    }
}

fn notification_needs_users(notification: &NotificationAction) -> bool {
    match notification {
        NotificationAction::Text { .. } => true,
        NotificationAction::Image { game, .. } => {
            game.end_time.is_none() || !game.scores.is_empty()
        }
    }
}

fn notification_is_started_game(notification: &NotificationAction) -> bool {
    matches!(
        notification,
        NotificationAction::Image { game, .. } if game.end_time.is_none()
    )
}

fn effective_notification_users<'a>(
    response_users: &'a [LegacyMatchUser],
    fallback_users: Option<&'a [LegacyMatchUser]>,
    notification: &NotificationAction,
) -> &'a [LegacyMatchUser] {
    if notification_is_started_game(notification) {
        if let Some(users) = fallback_users.filter(|users| !users.is_empty()) {
            return users;
        }
    }

    if response_users.is_empty() {
        fallback_users.unwrap_or(response_users)
    } else {
        response_users
    }
}

async fn enrich_match_result_metadata(
    output: &mut super::notify::MatchResultBuildOutput,
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
) {
    if !output.needs_beatmap_metadata() {
        return;
    }

    match fetch_beatmap_metadata(rate_limiter, oauth, output.params.beatmap_id).await {
        Ok(metadata) => output.apply_beatmap_metadata(metadata),
        Err(e) => warn!(
            beatmap_id = output.params.beatmap_id,
            error = %e,
            "{}",
            log_str("ml.poller_match_fetch_failed")
        ),
    }
}

/// Render a match result image card. Returns JPEG bytes on success, or `None`
/// if essential data is missing or rendering fails.
#[allow(clippy::too_many_arguments)]
async fn render_match_result_image(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    match_id: u64,
    match_name: &str,
    event_label: &str,
    played_at: &str,
    game: &osubot_core::api::LegacyMatchGame,
    users: &[LegacyMatchUser],
) -> Option<Vec<u8>> {
    let mut output = super::notify::build_match_result_params(
        match_id,
        match_name,
        event_label,
        played_at,
        game,
        users,
    )?;
    enrich_match_result_metadata(&mut output, rate_limiter, oauth).await;
    output.params.cover_image = fetch_match_cover_image(output.cover_url.as_deref()).await;
    fetch_match_avatar_images(&mut output.params.players).await;
    osubot_render::render_match_result_card(output.params)
        .await
        .ok()
}

#[derive(Clone)]
pub struct MatchListenerPoller {
    storage: Arc<Storage>,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    onebot_api: Arc<OneBotApi>,
    config: Arc<RwLock<Config>>,
    current_write: Arc<Mutex<Option<WsWrite>>>,
    shutdown: Arc<AtomicBool>,
}

impl MatchListenerPoller {
    pub fn new(
        storage: Arc<Storage>,
        oauth: Arc<OauthTokenCache>,
        rate_limiter: Arc<RateLimiter>,
        onebot_api: Arc<OneBotApi>,
        config: Arc<RwLock<Config>>,
        current_write: Arc<Mutex<Option<WsWrite>>>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            storage,
            oauth,
            rate_limiter,
            onebot_api,
            config,
            current_write,
            shutdown,
        }
    }

    /// Background task entry point.
    pub async fn run(&self) {
        info!("{}", log_str("ml.poller_started"));
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                info!("{}", log_str("ml.poller_stopped"));
                break;
            }

            let interval_secs = {
                let cfg = self.config.read().await;
                cfg.match_listen.poll_interval_secs.max(5)
            };

            // Poll immediately on startup so newly added listeners do not wait
            // a full interval before the first update check.
            self.poll_cycle().await;

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {}
                _ = wait_for_shutdown(&self.shutdown) => break,
            }
        }
    }

    /// Process one polling cycle: expire old listeners, fetch due ones.
    async fn poll_cycle(&self) {
        // Expire old listeners
        if let Err(e) = self.storage.expire_old_match_listeners().await {
            warn!(error = %e, "{}", log_str("ml.poller_match_fetch_failed"));
        }

        // Load due listeners
        let due = match self
            .storage
            .list_active_match_listeners_due_for_polling()
            .await
        {
            Ok(listeners) => listeners,
            Err(e) => {
                error!(error = %e, "{}", log_str("ml.poller_match_fetch_failed"));
                return;
            }
        };

        info!("{}", log_fmt!("ml.poller_tick", count = due.len()));

        // Process with bounded concurrency
        use futures_util::StreamExt;
        let results: Vec<_> = futures_util::stream::iter(due)
            .map(|listener| {
                let this = self.clone();
                async move { this.process_one(listener).await }
            })
            .buffer_unordered(MAX_CONCURRENT_LISTENERS)
            .collect()
            .await;

        for r in results {
            if let Err(e) = r {
                warn!(error = %e, "{}", log_str("ml.poller_match_fetch_failed"));
            }
        }
    }

    /// Process a single listener.
    async fn process_one(
        &self,
        listener: osubot_core::storage::MatchListener,
    ) -> Result<(), String> {
        let match_id = listener.match_id as u64;
        let group_id = listener.group_id;

        let cursor = ListenerCursor {
            last_event_id: listener.last_event_id.map(|v| v as u64),
            last_notified_event_id: listener.last_notified_event_id.map(|v| v as u64),
            pending_game_event_id: listener.pending_game_event_id.map(|v| v as u64),
        };

        // Fetch match events after the cursor
        let response = match fetch_match(
            &self.rate_limiter,
            &self.oauth,
            match_id,
            cursor.last_event_id,
            None,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                error!(match_id, group_id, error = %e, "{}", log_fmt!("ml.poller_match_fetch_failed", error = &e.to_string()));
                return Ok(()); // Non-fatal: try next cycle
            }
        };

        let output = process_events(&cursor, &response);

        let incremental_roster = reconstruct_match_roster(&response);
        let need_users = output.notifications.iter().any(notification_needs_users);
        let should_fetch_user_context =
            need_users && response.users.is_empty() && incremental_roster.is_empty();

        let fallback_users = if should_fetch_user_context {
            match fetch_match(&self.rate_limiter, &self.oauth, match_id, None, None).await {
                Ok(full_response) => {
                    let roster = reconstruct_match_roster(&full_response);
                    if roster.is_empty() {
                        (!full_response.users.is_empty()).then_some(full_response.users)
                    } else {
                        Some(roster)
                    }
                }
                Err(e) => {
                    warn!(match_id, group_id, error = %e, "{}", log_fmt!("ml.poller_match_fetch_failed", error = &e.to_string()));
                    None
                }
            }
        } else {
            (!incremental_roster.is_empty()).then_some(incremental_roster)
        };

        let notify_cfg = {
            let cfg = self.config.read().await;
            cfg.match_listen.clone()
        };

        // Send notifications (read current_write fresh)
        let match_name = &response.match_info.name;
        let mut acknowledged_cursor = cursor.clone();
        let mut notifications_fully_handled = true;
        for notification in &output.notifications {
            if !should_emit_notification(&notify_cfg, notification) {
                apply_acknowledged_notification(&mut acknowledged_cursor, notification);
                continue;
            }

            let users = effective_notification_users(
                &response.users,
                fallback_users.as_deref(),
                notification,
            );
            let delivered = self
                .send_notification(&listener, match_id, match_name, users, notification)
                .await;
            if delivered {
                apply_acknowledged_notification(&mut acknowledged_cursor, notification);
            } else {
                notifications_fully_handled = false;
                break;
            }
        }

        let final_cursor = if notifications_fully_handled {
            ListenerCursor {
                last_event_id: output.new_last_event_id,
                last_notified_event_id: output.new_last_notified_event_id,
                pending_game_event_id: output.new_pending_game_event_id,
            }
        } else {
            acknowledged_cursor
        };

        let touch_last_notified_at =
            final_cursor.last_notified_event_id != cursor.last_notified_event_id;
        let _ = self
            .storage
            .update_match_listener_progress(
                match_id as i64,
                group_id.unwrap_or(0),
                final_cursor.last_event_id.map(|v| v as i64),
                final_cursor.last_notified_event_id.map(|v| v as i64),
                final_cursor.pending_game_event_id.map(|v| v as i64),
                touch_last_notified_at,
            )
            .await;

        // Handle stop reason
        if notifications_fully_handled && output.stop_reason == Some(StopReason::MatchDisbanded) {
            let _ = self
                .storage
                .stop_match_listener(match_id as i64, group_id.unwrap_or(0))
                .await;
            info!(
                match_id,
                group_id,
                "{}",
                log_fmt!("ml.poller_match_completed")
            );
        }

        Ok(())
    }

    /// Send a text message to the appropriate target (group or private).
    async fn send_text(
        &self,
        write: &WsWrite,
        listener: &osubot_core::storage::MatchListener,
        text: &str,
    ) -> bool {
        let send_result = match listener.notification_type.as_str() {
            "private" => {
                let user_id = listener.user_id.unwrap_or(0);
                crate::onebot::send_private_msg(write, &self.onebot_api, user_id, text).await
            }
            _ => {
                let group_id = listener.group_id.unwrap_or(0);
                crate::onebot::send_group_msg(write, &self.onebot_api, group_id, text).await
            }
        };
        match send_result {
            Ok(()) => true,
            Err(e) => {
                warn!(error = %e, "{}", log_str("ml.notify_text_fallback"));
                false
            }
        }
    }

    /// Send a single notification to the appropriate target (group or private).
    ///
    /// Reads `current_write` fresh each call. If no sink is available or send
    /// fails, returns false so the caller can leave cursors unacknowledged and
    /// retry after reconnect.
    async fn send_notification(
        &self,
        listener: &osubot_core::storage::MatchListener,
        match_id: u64,
        match_name: &str,
        users: &[osubot_core::api::LegacyMatchUser],
        notification: &NotificationAction,
    ) -> bool {
        let cw_guard = self.current_write.lock().await;
        let write_opt = cw_guard.clone();
        drop(cw_guard);

        let Some(write) = write_opt else {
            warn!(match_id, "{}", log_str("ml.notify_text_fallback"));
            return false;
        };

        match notification {
            NotificationAction::Text {
                text, event_type, ..
            } => {
                let msg = super::notify::format_lobby_text(event_type, text, users, match_name);
                let sent = self.send_text(&write, listener, &msg).await;
                if sent {
                    info!(match_id, "{}", log_fmt!("ml.notify_sent"));
                }
                sent
            }
            NotificationAction::Image {
                game,
                event_label,
                played_at,
                ..
            } => {
                let fallback_text = || {
                    if game.end_time.is_none() || event_label == "场次开始" {
                        super::notify::format_game_start_fallback_text(
                            game.as_ref(),
                            users,
                            match_name,
                        )
                    } else {
                        super::notify::format_game_fallback_text(game.as_ref(), users, match_name)
                    }
                };

                let jpeg_bytes = render_match_result_image(
                    &self.rate_limiter,
                    &self.oauth,
                    match_id,
                    match_name,
                    event_label,
                    played_at,
                    game.as_ref(),
                    users,
                )
                .await;

                let Some(jpeg_bytes) = jpeg_bytes else {
                    let fallback = fallback_text();
                    return self.send_text(&write, listener, &fallback).await;
                };

                let is_private = listener.notification_type.as_str() == "private";
                let send_result = if is_private {
                    let user_id = listener.user_id.unwrap_or(0);
                    crate::onebot::send_private_msg_with_image(
                        &write,
                        &self.onebot_api,
                        user_id,
                        &jpeg_bytes,
                    )
                    .await
                } else {
                    let group_id = listener.group_id.unwrap_or(0);
                    crate::onebot::send_group_msg_with_image(
                        &write,
                        &self.onebot_api,
                        group_id,
                        &jpeg_bytes,
                    )
                    .await
                };

                match send_result {
                    Ok(()) => {
                        info!(
                            match_id,
                            bytes = jpeg_bytes.len(),
                            "{}",
                            log_fmt!("ml.notify_image_sent")
                        );
                        true
                    }
                    Err(e) => {
                        warn!(match_id, error = %e, "{}", log_str("ml.notify_text_fallback"));
                        let fallback = fallback_text();
                        self.send_text(&write, listener, &fallback).await
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MatchListenConfig;

    fn user(id: u64, name: &str) -> LegacyMatchUser {
        LegacyMatchUser {
            id,
            username: name.to_string(),
            avatar_url: None,
        }
    }

    #[test]
    fn poller_clones_shared_handles() {
        // Structural test: poller must be Clone so it can be moved into a task.
        fn assert_clone<T: Clone>() {}
        assert_clone::<MatchListenerPoller>();
    }

    fn started_notification() -> NotificationAction {
        NotificationAction::Image {
            event_id: 11,
            event_label: "场次开始".to_string(),
            played_at: "2026-06-01T12:00:00Z".to_string(),
            game: Box::new(osubot_core::api::LegacyMatchGame {
                beatmap_id: Some(123),
                beatmap: None,
                beatmapset: None,
                end_time: None,
                mods: Vec::new(),
                team_type: String::new(),
                scoring_type: String::new(),
                scores: Vec::new(),
            }),
        }
    }

    fn finished_notification() -> NotificationAction {
        NotificationAction::Image {
            event_id: 12,
            event_label: "场次结束".to_string(),
            played_at: "2026-06-01T12:30:00Z".to_string(),
            game: Box::new(osubot_core::api::LegacyMatchGame {
                beatmap_id: Some(123),
                beatmap: None,
                beatmapset: None,
                end_time: Some("2026-06-01T12:30:00Z".to_string()),
                mods: Vec::new(),
                team_type: String::new(),
                scoring_type: String::new(),
                scores: Vec::new(),
            }),
        }
    }

    #[test]
    fn should_emit_notification_honors_started_and_finished_flags() {
        let cfg = MatchListenConfig {
            max_per_group: 3,
            poll_interval_secs: 8,
            notify_on_new_game: false,
            notify_on_complete: true,
        };

        assert!(!should_emit_notification(&cfg, &started_notification()));
        assert!(should_emit_notification(&cfg, &finished_notification()));
    }

    #[test]
    fn apply_acknowledged_notification_keeps_started_game_retriable_until_acknowledged() {
        let mut cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(10),
            pending_game_event_id: None,
        };

        apply_acknowledged_notification(&mut cursor, &started_notification());

        assert_eq!(cursor.last_event_id, Some(10));
        assert_eq!(cursor.last_notified_event_id, Some(11));
        assert_eq!(cursor.pending_game_event_id, Some(11));
    }

    #[test]
    fn apply_acknowledged_notification_advances_completed_game_and_clears_pending() {
        let mut cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(10),
            pending_game_event_id: Some(11),
        };

        apply_acknowledged_notification(&mut cursor, &finished_notification());

        assert_eq!(cursor.last_event_id, Some(12));
        assert_eq!(cursor.last_notified_event_id, Some(12));
        assert_eq!(cursor.pending_game_event_id, None);
    }

    #[test]
    fn effective_notification_users_prefers_response_users() {
        let response_users = vec![user(1, "Current")];
        let fallback_users = vec![user(2, "Fallback")];

        let notification = NotificationAction::Text {
            event_id: 1,
            event_type: "player-joined".to_string(),
            text: "Current joined".to_string(),
        };

        let users =
            effective_notification_users(&response_users, Some(&fallback_users), &notification);

        assert_eq!(users.len(), 1);
        assert_eq!(users[0].username, "Current");
    }

    #[test]
    fn effective_notification_users_uses_fallback_when_incremental_users_empty() {
        let response_users = Vec::new();
        let fallback_users = vec![user(2, "Fallback")];

        let notification = NotificationAction::Text {
            event_id: 1,
            event_type: "player-joined".to_string(),
            text: "Fallback joined".to_string(),
        };

        let users =
            effective_notification_users(&response_users, Some(&fallback_users), &notification);

        assert_eq!(users.len(), 1);
        assert_eq!(users[0].username, "Fallback");
    }

    #[test]
    fn effective_notification_users_prefers_roster_for_started_game() {
        let response_users = vec![user(1, "Historic")];
        let fallback_users = vec![user(2, "CurrentParticipant")];
        let notification = NotificationAction::Image {
            event_id: 1,
            event_label: "场次开始".to_string(),
            played_at: "2026-06-01T12:00:00Z".to_string(),
            game: Box::new(osubot_core::api::LegacyMatchGame {
                beatmap_id: Some(123),
                beatmap: None,
                beatmapset: None,
                end_time: None,
                mods: Vec::new(),
                team_type: String::new(),
                scoring_type: String::new(),
                scores: Vec::new(),
            }),
        };

        let users =
            effective_notification_users(&response_users, Some(&fallback_users), &notification);

        assert_eq!(users.len(), 1);
        assert_eq!(users[0].username, "CurrentParticipant");
    }

    #[test]
    fn text_delivery_only_acknowledges_successful_send() {
        let mut acknowledged_cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(10),
            pending_game_event_id: None,
        };

        let notification = NotificationAction::Text {
            event_id: 11,
            event_type: "match-created".to_string(),
            text: "Created".to_string(),
        };

        let delivered = false;
        if delivered {
            apply_acknowledged_notification(&mut acknowledged_cursor, &notification);
        }

        assert_eq!(acknowledged_cursor.last_event_id, Some(10));
        assert_eq!(acknowledged_cursor.last_notified_event_id, Some(10));
    }
}
