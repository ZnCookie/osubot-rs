//! Pure match listener state machine.
//!
//! Takes the previous cursor state (from storage) plus fetched match events
//! (from the osu! API) and returns notification actions plus updated cursor
//! state. This module performs no I/O — no HTTP, DB, rendering, or OneBot.
//!
//! // ponytail: v1 uses a narrow pending-game cursor rule instead of a full
//! replay/backfill engine. If a game event has no `end_time`, the cursor holds
//! at that event until a later poll observes completion. It still emits a
//! game-start image once, matching YumuBot's onGameStart behavior.

use osubot_core::api::{LegacyMatchEvent, LegacyMatchGame, LegacyMatchResponse};

/// Previous listener cursor state read from storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerCursor {
    pub last_event_id: Option<u64>,
    pub last_notified_event_id: Option<u64>,
    pub pending_game_event_id: Option<u64>,
}

/// A notification action the poller should perform.
#[derive(Debug, Clone, PartialEq)]
pub enum NotificationAction {
    /// Lobby event (player joined/left, host changed, match created, etc.) — text only.
    Text {
        event_id: u64,
        event_type: String,
        text: String,
    },
    /// Completed game — render and send an image card.
    Image {
        event_id: u64,
        event_label: String,
        played_at: String,
        game: Box<LegacyMatchGame>,
    },
}

/// Why the listener should stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The match was disbanded via API event.
    MatchDisbanded,
}

/// Output of the state machine: notifications to send + updated cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct StateMachineOutput {
    pub notifications: Vec<NotificationAction>,
    pub new_last_event_id: Option<u64>,
    pub new_last_notified_event_id: Option<u64>,
    pub new_pending_game_event_id: Option<u64>,
    pub stop_reason: Option<StopReason>,
}

/// Process a batch of fetched match events against the previous cursor.
///
/// Events are processed in ascending ID order. The cursor advances past
/// lobby events and completed games. When an in-progress game (no `end_time`)
/// is encountered, processing stops and `pending_game_event_id` is set —
/// the cursor will not advance past that event until completion is observed.
pub fn process_events(
    cursor: &ListenerCursor,
    response: &LegacyMatchResponse,
) -> StateMachineOutput {
    let mut output = StateMachineOutput {
        notifications: Vec::new(),
        new_last_event_id: cursor.last_event_id,
        new_last_notified_event_id: cursor.last_notified_event_id,
        new_pending_game_event_id: cursor.pending_game_event_id,
        stop_reason: None,
    };

    for event in &response.events {
        // Skip already-processed events.
        if let Some(last) = cursor.last_event_id {
            if event.id <= last {
                continue;
            }
        }

        // If we have a pending in-progress game, only process that event
        // (check if it now has end_time). Other events after it must wait.
        if let Some(pending_id) = cursor.pending_game_event_id {
            if event.id > pending_id && output.new_pending_game_event_id.is_some() {
                // Still waiting for the pending game to complete; stop here.
                break;
            }
        }

        match classify_event(event) {
            EventKind::Lobby => {
                if !suppressed_lobby_event(event.detail.event_type.as_str()) {
                    output.notifications.push(NotificationAction::Text {
                        event_id: event.id,
                        event_type: event.detail.event_type.clone(),
                        text: event.detail.text.clone(),
                    });
                }
                output.new_last_event_id = Some(event.id);
                output.new_last_notified_event_id = Some(event.id);
            }
            EventKind::CompletedGame => {
                if let Some(game) = event.game.clone() {
                    output.notifications.push(NotificationAction::Image {
                        event_id: event.id,
                        event_label: "场次结束".to_string(),
                        played_at: game
                            .end_time
                            .clone()
                            .unwrap_or_else(|| event.timestamp.clone()),
                        game: Box::new(game),
                    });
                }
                output.new_last_event_id = Some(event.id);
                output.new_last_notified_event_id = Some(event.id);
                // Clear pending marker if this was the pending game.
                if output.new_pending_game_event_id == Some(event.id) {
                    output.new_pending_game_event_id = None;
                }
            }
            EventKind::InProgressGame => {
                if let Some(game) = event.game.clone() {
                    if output.new_pending_game_event_id != Some(event.id) {
                        output.notifications.push(NotificationAction::Image {
                            event_id: event.id,
                            event_label: "场次开始".to_string(),
                            played_at: event.timestamp.clone(),
                            game: Box::new(game),
                        });
                        output.new_last_notified_event_id = Some(event.id);
                    }
                }
                // Hold cursor: set pending marker but do NOT advance last_event_id.
                output.new_pending_game_event_id = Some(event.id);
                // Stop processing further events.
                break;
            }
            EventKind::MatchDisbanded => {
                output.notifications.push(NotificationAction::Text {
                    event_id: event.id,
                    event_type: event.detail.event_type.clone(),
                    text: event.detail.text.clone(),
                });
                output.new_last_event_id = Some(event.id);
                output.new_last_notified_event_id = Some(event.id);
                output.stop_reason = Some(StopReason::MatchDisbanded);
                // No point processing events after disband.
                break;
            }
        }
    }

    output
}

fn suppressed_lobby_event(event_type: &str) -> bool {
    matches!(event_type, "player-joined" | "player-left" | "host-changed")
}

/// Classify an event into a processing kind.
fn classify_event(event: &LegacyMatchEvent) -> EventKind {
    let event_type = event.detail.event_type.as_str();
    if event_type == "match-disbanded" {
        return EventKind::MatchDisbanded;
    }

    match &event.game {
        None => EventKind::Lobby,
        Some(game) => {
            if game.end_time.is_some() {
                EventKind::CompletedGame
            } else {
                EventKind::InProgressGame
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum EventKind {
    Lobby,
    CompletedGame,
    InProgressGame,
    MatchDisbanded,
}

#[cfg(test)]
mod tests {
    use super::*;
    use osubot_core::api::{
        LegacyMatchEvent, LegacyMatchEventDetail, LegacyMatchGame, LegacyMatchGameScore,
        LegacyMatchInfo, LegacyMatchResponse,
    };

    fn make_lobby_event(id: u64, event_type: &str, text: &str) -> LegacyMatchEvent {
        LegacyMatchEvent {
            id,
            timestamp: "2026-06-01T12:00:00Z".to_string(),
            user_id: Some(11),
            detail: LegacyMatchEventDetail {
                event_type: event_type.to_string(),
                text: text.to_string(),
            },
            game: None,
        }
    }

    fn make_game_event(id: u64, end_time: Option<&str>) -> LegacyMatchEvent {
        LegacyMatchEvent {
            id,
            timestamp: "2026-06-01T12:30:00Z".to_string(),
            user_id: None,
            detail: LegacyMatchEventDetail {
                event_type: "other".to_string(),
                text: "Blue Team won".to_string(),
            },
            game: Some(LegacyMatchGame {
                beatmap_id: Some(987654),
                beatmap: None,
                beatmapset: None,
                end_time: end_time.map(|s| s.to_string()),
                mods: Vec::new(),
                team_type: "team-vs".to_string(),
                scoring_type: "score".to_string(),
                scores: vec![LegacyMatchGameScore {
                    user_id: 11,
                    score: 654321,
                    total_score: None,
                    legacy_total_score: None,
                    classic_total_score: None,
                    accuracy: 0.9825,
                    max_combo: 0,
                    mods: Vec::new(),
                    team: "blue".to_string(),
                    rank: "A".to_string(),
                    passed: true,
                }],
            }),
        }
    }

    fn make_disbanded_event(id: u64) -> LegacyMatchEvent {
        make_lobby_event(id, "match-disbanded", "Match disbanded")
    }

    fn make_response(events: Vec<LegacyMatchEvent>, latest_event_id: u64) -> LegacyMatchResponse {
        LegacyMatchResponse {
            match_info: LegacyMatchInfo {
                id: 424242,
                name: "OWC Finals".to_string(),
                start_time: None,
                end_time: None,
            },
            users: Vec::new(),
            first_event_id: Some(1),
            latest_event_id,
            cursor_string: None,
            events,
        }
    }

    fn empty_cursor() -> ListenerCursor {
        ListenerCursor {
            last_event_id: None,
            last_notified_event_id: None,
            pending_game_event_id: None,
        }
    }

    #[test]
    fn first_poll_lobby_events_produce_text_notifications() {
        let cursor = empty_cursor();
        let response = make_response(
            vec![
                make_lobby_event(10, "match-created", "Alpha created the match"),
                make_lobby_event(11, "player-joined", "Bravo joined"),
            ],
            11,
        );

        let output = process_events(&cursor, &response);

        assert_eq!(output.notifications.len(), 1);
        assert!(matches!(
            &output.notifications[0],
            NotificationAction::Text { event_id: 10, event_type, .. } if event_type == "match-created"
        ));
        assert_eq!(output.new_last_event_id, Some(11));
        assert_eq!(output.new_last_notified_event_id, Some(11));
        assert_eq!(output.new_pending_game_event_id, None);
        assert_eq!(output.stop_reason, None);
    }

    #[test]
    fn suppresses_player_joined_broadcast_but_advances_cursor() {
        let cursor = empty_cursor();
        let response = make_response(
            vec![make_lobby_event(11, "player-joined", "Bravo joined")],
            11,
        );

        let output = process_events(&cursor, &response);

        assert!(output.notifications.is_empty());
        assert_eq!(output.new_last_event_id, Some(11));
        assert_eq!(output.new_last_notified_event_id, Some(11));
    }

    #[test]
    fn suppresses_player_left_broadcast_but_advances_cursor() {
        let cursor = empty_cursor();
        let response = make_response(vec![make_lobby_event(11, "player-left", "Bravo left")], 11);

        let output = process_events(&cursor, &response);

        assert!(output.notifications.is_empty());
        assert_eq!(output.new_last_event_id, Some(11));
        assert_eq!(output.new_last_notified_event_id, Some(11));
    }

    #[test]
    fn suppresses_host_changed_broadcast_but_advances_cursor() {
        let cursor = empty_cursor();
        let response = make_response(
            vec![make_lobby_event(
                11,
                "host-changed",
                "Alpha became the host",
            )],
            11,
        );

        let output = process_events(&cursor, &response);

        assert!(output.notifications.is_empty());
        assert_eq!(output.new_last_event_id, Some(11));
        assert_eq!(output.new_last_notified_event_id, Some(11));
    }

    #[test]
    fn emits_image_for_completed_game() {
        let cursor = empty_cursor();
        let response = make_response(vec![make_game_event(10, Some("2026-06-01T12:29:58Z"))], 10);

        let output = process_events(&cursor, &response);

        assert_eq!(output.notifications.len(), 1);
        assert!(matches!(
            &output.notifications[0],
            NotificationAction::Image { event_id: 10, event_label, .. } if event_label == "场次结束"
        ));
        assert_eq!(output.new_last_event_id, Some(10));
        assert_eq!(output.new_last_notified_event_id, Some(10));
        assert_eq!(output.new_pending_game_event_id, None);
    }

    #[test]
    fn holds_cursor_for_in_progress_game() {
        let cursor = empty_cursor();
        let response = make_response(
            vec![
                make_lobby_event(10, "player-joined", "Alpha joined"),
                make_game_event(11, None), // in-progress
                make_lobby_event(12, "player-joined", "Bravo joined"), // should NOT be processed
            ],
            12,
        );

        let output = process_events(&cursor, &response);

        // player-joined is silent; in-progress game still emits start info and holds cursor.
        assert_eq!(output.notifications.len(), 1);
        assert!(matches!(
            &output.notifications[0],
            NotificationAction::Image { event_id: 11, event_label, played_at, .. }
                if event_label == "场次开始" && played_at == "2026-06-01T12:30:00Z"
        ));
        // Cursor advanced past event 10 but NOT past event 11.
        assert_eq!(output.new_last_event_id, Some(10));
        assert_eq!(output.new_last_notified_event_id, Some(11));
        assert_eq!(output.new_pending_game_event_id, Some(11));
        assert_eq!(output.stop_reason, None);
    }

    #[test]
    fn does_not_repeat_start_image_for_existing_pending_game() {
        let cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(11),
            pending_game_event_id: Some(11),
        };
        let response = make_response(vec![make_game_event(11, None)], 11);

        let output = process_events(&cursor, &response);

        assert!(output.notifications.is_empty());
        assert_eq!(output.new_last_event_id, Some(10));
        assert_eq!(output.new_last_notified_event_id, Some(11));
        assert_eq!(output.new_pending_game_event_id, Some(11));
    }

    #[test]
    fn advances_cursor_when_in_progress_game_completes() {
        // Previous poll left a pending game at event 11.
        let cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(10),
            pending_game_event_id: Some(11),
        };
        // Same event 11 now has end_time set, plus a new lobby event 12.
        let response = make_response(
            vec![
                make_game_event(11, Some("2026-06-01T12:29:58Z")), // now completed
                make_lobby_event(12, "player-joined", "Bravo joined"),
            ],
            12,
        );

        let output = process_events(&cursor, &response);

        // Event 11 is > last_event_id(10), so it's processed.
        // It's now a completed game → image notification.
        // Event 12 is player-joined → silent, but cursor still advances.
        assert_eq!(output.notifications.len(), 1);
        assert!(matches!(
            &output.notifications[0],
            NotificationAction::Image { event_id: 11, event_label, .. } if event_label == "场次结束"
        ));
        assert_eq!(output.new_last_event_id, Some(12));
        assert_eq!(output.new_last_notified_event_id, Some(12));
        assert_eq!(output.new_pending_game_event_id, None);
    }

    #[test]
    fn match_disbanded_emits_text_and_stop() {
        let cursor = empty_cursor();
        let response = make_response(
            vec![
                make_lobby_event(10, "match-created", "Created"),
                make_disbanded_event(11),
                make_lobby_event(12, "player-joined", "Should not appear"),
            ],
            12,
        );

        let output = process_events(&cursor, &response);

        assert_eq!(output.notifications.len(), 2);
        assert!(matches!(
            &output.notifications[1],
            NotificationAction::Text { event_id: 11, event_type, .. } if event_type == "match-disbanded"
        ));
        assert_eq!(output.stop_reason, Some(StopReason::MatchDisbanded));
        assert_eq!(output.new_last_event_id, Some(11));
        // Event 12 should not be processed.
        assert!(!output
            .notifications
            .iter()
            .any(|n| matches!(n, NotificationAction::Text { event_id: 12, .. })));
    }

    #[test]
    fn skips_already_processed_events() {
        let cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(10),
            pending_game_event_id: None,
        };
        let response = make_response(
            vec![
                make_lobby_event(10, "match-created", "Already processed"),
                make_lobby_event(11, "player-joined", "New event"),
            ],
            11,
        );

        let output = process_events(&cursor, &response);

        // Event 11 is player-joined and should stay silent while cursor advances.
        assert!(output.notifications.is_empty());
        assert_eq!(output.new_last_event_id, Some(11));
    }

    #[test]
    fn empty_events_produces_no_actions() {
        let cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(10),
            pending_game_event_id: None,
        };
        let response = make_response(Vec::new(), 10);

        let output = process_events(&cursor, &response);

        assert!(output.notifications.is_empty());
        assert_eq!(output.new_last_event_id, Some(10));
        assert_eq!(output.stop_reason, None);
    }

    #[test]
    fn pending_game_with_no_new_events_keeps_pending() {
        let cursor = ListenerCursor {
            last_event_id: Some(10),
            last_notified_event_id: Some(10),
            pending_game_event_id: Some(11),
        };
        // API returns no new events (game still in progress).
        let response = make_response(Vec::new(), 11);

        let output = process_events(&cursor, &response);

        assert!(output.notifications.is_empty());
        assert_eq!(output.new_pending_game_event_id, Some(11));
        assert_eq!(output.new_last_event_id, Some(10));
    }
}
