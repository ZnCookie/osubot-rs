use super::*;
use osubot_core::api::{fetch_match, LegacyMatchResponse};
use osubot_core::log_fmt;
use osubot_core::storage::{MatchListener, MatchListenerStartParams};
use osubot_core::strings::{log_str, user_str};
use osubot_core::types::{Command, MatchListenAction};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

/// Maximum listener lifetime in hours (v1 guardrail).
const MAX_LISTENER_LIFETIME_HOURS: i64 = 6;

/// Format a Chinese confirmation/error response for the !ml command.
///
/// Kept as a pure function so unit tests can assert Chinese wording without
/// touching storage/API/OneBot.
pub(crate) fn format_ml_response(action: &MatchListenAction, result: &MlActionResult) -> String {
    let qq = result.qq().to_string();
    match (action, result) {
        (_, MlActionResult::Error { message, .. }) => message.clone(),
        (MatchListenAction::Start { match_id, .. }, MlActionResult::Started { match_name, .. }) => {
            user_str("ml.start_success")
                .replace("{qq}", &qq)
                .replace("{match_id}", &match_id.to_string())
                .replace("{match_name}", match_name)
        }
        (MatchListenAction::Stop { match_id }, MlActionResult::Stopped { .. }) => {
            user_str("ml.stop_success")
                .replace("{qq}", &qq)
                .replace("{match_id}", &match_id.to_string())
        }
        (MatchListenAction::StopAll, MlActionResult::StoppedAll { count, .. }) => {
            user_str("ml.stop_all_success")
                .replace("{qq}", &qq)
                .replace("{count}", &count.to_string())
        }
        (MatchListenAction::List, MlActionResult::List { listeners, .. }) => {
            if listeners.is_empty() {
                user_str("ml.list_empty").replace("{qq}", &qq)
            } else {
                let header =
                    user_str("ml.list_header").replace("{count}", &listeners.len().to_string());
                let items: Vec<String> = listeners
                    .iter()
                    .enumerate()
                    .map(|(idx, l)| {
                        user_str("ml.list_item")
                            .replace("{idx}", &(idx + 1).to_string())
                            .replace("{match_id}", &l.match_id.to_string())
                            .replace("{match_name}", &format_match_name(l))
                            .replace("{username}", &l.creator_qq.to_string())
                            .replace("{status}", format_listener_status(l))
                    })
                    .collect();
                format!("{header}{}", items.join("\n"))
            }
        }
        (
            MatchListenAction::Status { match_id },
            MlActionResult::Status {
                match_name,
                status,
                players,
                games_played,
                ..
            },
        ) => {
            let header = user_str("ml.status_header").replace("{match_id}", &match_id.to_string());
            let name = user_str("ml.status_name").replace("{name}", match_name);
            let status_line = user_str("ml.status_status").replace("{status}", status);
            let players_line =
                user_str("ml.status_players").replace("{count}", &players.to_string());
            let games_line =
                user_str("ml.status_games").replace("{count}", &games_played.to_string());
            format!("{header}{name}{status_line}{players_line}{games_line}")
        }
        (MatchListenAction::Status { match_id }, MlActionResult::NotFound { .. }) => {
            user_str("ml.not_found")
                .replace("{qq}", &qq)
                .replace("{match_id}", &match_id.to_string())
        }
        _ => user_str("error.query_failed").replace("{qq}", &qq),
    }
}

/// Result of executing an !ml action.
#[derive(Debug, Clone)]
pub(crate) enum MlActionResult {
    Started {
        qq: i64,
        match_name: String,
    },
    Stopped {
        qq: i64,
    },
    StoppedAll {
        qq: i64,
        count: u64,
    },
    List {
        qq: i64,
        listeners: Vec<MatchListener>,
    },
    Status {
        qq: i64,
        match_name: String,
        status: String,
        players: usize,
        games_played: u64,
    },
    NotFound {
        qq: i64,
    },
    Error {
        qq: i64,
        message: String,
    },
}

impl MlActionResult {
    fn qq(&self) -> i64 {
        match self {
            MlActionResult::Started { qq, .. }
            | MlActionResult::Stopped { qq, .. }
            | MlActionResult::StoppedAll { qq, .. }
            | MlActionResult::List { qq, .. }
            | MlActionResult::Status { qq, .. }
            | MlActionResult::NotFound { qq, .. }
            | MlActionResult::Error { qq, .. } => *qq,
        }
    }
}

fn format_match_name(l: &MatchListener) -> String {
    if l.match_name.trim().is_empty() {
        user_str("ml.unknown").to_string()
    } else {
        l.match_name.clone()
    }
}

fn format_listener_status(l: &MatchListener) -> &'static str {
    if l.active {
        user_str("ml.listener_active_status")
    } else {
        user_str("ml.listener_stopped_status")
    }
}

/// Handle the `!ml` command.
pub(super) async fn handle_match_listen_command(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
) {
    let Command::MatchListen(action) = cmd else {
        return;
    };
    let group_id = msg.group_id.unwrap_or(0);
    let user_id = msg.user_id;

    info!(
        group_id, user_id, action = ?action,
        "{}", log_str("ml.command_received")
    );

    let result = execute_ml_action(ctx, action, group_id, user_id).await;
    let response = format_ml_response(action, &result);

    if let MlActionResult::Error { message, .. } = &result {
        warn!(
            group_id,
            user_id,
            message,
            "{}",
            log_fmt!("ml.command_received", action = format!("{:?}", action))
        );
    }

    let _ = resp_tx.send(response).await;
}

async fn execute_ml_action(
    ctx: &BotContext,
    action: &MatchListenAction,
    group_id: i64,
    user_id: i64,
) -> MlActionResult {
    match action {
        MatchListenAction::Start {
            match_id,
            skip_rounds,
        } => execute_start(ctx, *match_id, *skip_rounds, group_id, user_id).await,
        MatchListenAction::Stop { match_id } => {
            execute_stop(ctx, *match_id, group_id, user_id).await
        }
        MatchListenAction::StopAll => execute_stop_all(ctx, group_id, user_id).await,
        MatchListenAction::List => execute_list(ctx, group_id, user_id).await,
        MatchListenAction::Status { match_id } => {
            execute_status(ctx, *match_id, group_id, user_id).await
        }
    }
}

async fn execute_start(
    ctx: &BotContext,
    match_id: u64,
    skip_rounds: u32,
    group_id: i64,
    user_id: i64,
) -> MlActionResult {
    let now = chrono::Utc::now().timestamp();

    // Idempotent retry semantics: if the same match is already active in this group,
    // treat repeated start as success so users can retry after a lost confirmation.
    match ctx
        .storage
        .get_match_listener(match_id as i64, group_id)
        .await
    {
        Ok(Some(l)) if is_listener_effectively_active(&l, now) => {
            return MlActionResult::Started {
                qq: user_id,
                match_name: format!("MP #{match_id}"),
            };
        }
        _ => {}
    }

    let config = ctx.config.read().await;
    let max_per_group = config.match_listen.max_per_group as u64;
    drop(config);

    // Check group limit
    match ctx
        .storage
        .count_active_match_listeners_in_group(group_id)
        .await
    {
        Ok(count) => {
            if count >= max_per_group {
                return MlActionResult::Error {
                    qq: user_id,
                    message: user_str("ml.limit_exceeded")
                        .replace("{limit}", &max_per_group.to_string()),
                };
            }
        }
        Err(e) => {
            error!(group_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
            return MlActionResult::Error {
                qq: user_id,
                message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
            };
        }
    }

    // Fetch match to get initial cursor + match name.
    // Use a wider window so YumuBot-compatible #N skip and current-game replay
    // can be decided from actual game events instead of only latest_event_id.
    let response = match fetch_match(&ctx.rate_limiter, &ctx.oauth, match_id, None, Some(101)).await
    {
        Ok(r) => r,
        Err(e) => {
            error!(match_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
            return MlActionResult::Error {
                qq: user_id,
                message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
            };
        }
    };

    let match_name = response.match_info.name.clone();
    let initial_cursor = initial_cursor_from_response(&response, skip_rounds);
    let now = chrono::Utc::now();
    let expires_at = (now + chrono::Duration::hours(MAX_LISTENER_LIFETIME_HOURS)).timestamp();

    // Persist listener
    if let Err(e) = ctx
        .storage
        .start_match_listener(MatchListenerStartParams {
            match_id: match_id as i64,
            group_id,
            creator_qq: user_id,
            match_name: match_name.clone(),
            expires_at,
            initial_last_event_id: initial_cursor.last_event_id,
            initial_last_notified_event_id: initial_cursor.last_notified_event_id,
        })
        .await
    {
        error!(group_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
        return MlActionResult::Error {
            qq: user_id,
            message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
        };
    }
    MlActionResult::Started {
        qq: user_id,
        match_name,
    }
}

fn is_listener_effectively_active(listener: &MatchListener, now: i64) -> bool {
    listener.active && listener.expires_at >= now
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct InitialMatchCursor {
    last_event_id: Option<i64>,
    last_notified_event_id: Option<i64>,
}

fn initial_cursor_from_response(
    response: &LegacyMatchResponse,
    skip_rounds: u32,
) -> InitialMatchCursor {
    let latest_event_id = response.latest_event_id as i64;
    let skipped_game_event_id = (skip_rounds > 0)
        .then(|| {
            response
                .events
                .iter()
                .filter(|event| event.game.is_some())
                .nth((skip_rounds - 1) as usize)
                .map(|event| event.id as i64)
        })
        .flatten();

    if let Some(event_id) = skipped_game_event_id {
        return InitialMatchCursor {
            last_event_id: Some(event_id),
            last_notified_event_id: Some(event_id),
        };
    }

    let current_in_progress_game_id = response
        .events
        .iter()
        .rev()
        .find(|event| {
            event
                .game
                .as_ref()
                .is_some_and(|game| game.end_time.is_none())
        })
        .map(|event| event.id as i64);

    if let Some(event_id) = current_in_progress_game_id {
        let before_game = event_id.saturating_sub(1);
        return InitialMatchCursor {
            last_event_id: Some(before_game),
            last_notified_event_id: Some(before_game),
        };
    }

    InitialMatchCursor {
        last_event_id: Some(latest_event_id),
        last_notified_event_id: Some(latest_event_id),
    }
}

async fn execute_stop(
    ctx: &BotContext,
    match_id: u64,
    group_id: i64,
    user_id: i64,
) -> MlActionResult {
    match ctx
        .storage
        .stop_match_listener(match_id as i64, group_id)
        .await
    {
        Ok(true) => MlActionResult::Stopped { qq: user_id },
        Ok(false) => MlActionResult::NotFound { qq: user_id },
        Err(e) => {
            error!(group_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
            MlActionResult::Error {
                qq: user_id,
                message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
            }
        }
    }
}

async fn execute_stop_all(ctx: &BotContext, group_id: i64, user_id: i64) -> MlActionResult {
    match ctx
        .storage
        .stop_all_match_listeners_in_group(group_id)
        .await
    {
        Ok(count) => MlActionResult::StoppedAll { qq: user_id, count },
        Err(e) => {
            error!(group_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
            MlActionResult::Error {
                qq: user_id,
                message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
            }
        }
    }
}

async fn execute_list(ctx: &BotContext, group_id: i64, user_id: i64) -> MlActionResult {
    match ctx
        .storage
        .list_active_match_listeners_by_group(group_id)
        .await
    {
        Ok(listeners) => MlActionResult::List {
            qq: user_id,
            listeners,
        },
        Err(e) => {
            error!(group_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
            MlActionResult::Error {
                qq: user_id,
                message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
            }
        }
    }
}

async fn execute_status(
    ctx: &BotContext,
    match_id: u64,
    group_id: i64,
    user_id: i64,
) -> MlActionResult {
    let now = chrono::Utc::now().timestamp();

    // Check if this group is listening
    match ctx
        .storage
        .get_match_listener(match_id as i64, group_id)
        .await
    {
        Ok(Some(l)) if is_listener_effectively_active(&l, now) => {}
        Ok(_) => return MlActionResult::NotFound { qq: user_id },
        Err(e) => {
            error!(group_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
            return MlActionResult::Error {
                qq: user_id,
                message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
            };
        }
    }

    // Fetch current match state
    let response = match fetch_match(&ctx.rate_limiter, &ctx.oauth, match_id, None, None).await {
        Ok(r) => r,
        Err(e) => {
            error!(match_id, error = %e, "{}", log_str("ml.fetch_match_failed_log"));
            return MlActionResult::Error {
                qq: user_id,
                message: user_str("ml.fetch_failed").replace("{qq}", &user_id.to_string()),
            };
        }
    };

    let match_name = response.match_info.name.clone();
    let status = if response.match_info.end_time.is_some() {
        user_str("ml.match_status_finished")
    } else {
        user_str("ml.match_status_in_progress")
    };
    let players = response.users.len();
    let games_played = response
        .events
        .iter()
        .filter(|event| event.game.is_some())
        .count() as u64;

    MlActionResult::Status {
        qq: user_id,
        match_name,
        status: status.to_string(),
        players,
        games_played,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osubot_core::api::{
        LegacyMatchEvent, LegacyMatchEventDetail, LegacyMatchGame, LegacyMatchInfo,
        LegacyMatchResponse,
    };
    use osubot_core::types::MatchListenAction;

    fn make_lobby_event(id: u64) -> LegacyMatchEvent {
        LegacyMatchEvent {
            id,
            timestamp: "2026-06-01T12:00:00Z".to_string(),
            user_id: None,
            detail: LegacyMatchEventDetail {
                event_type: "player-joined".to_string(),
                text: "Alpha joined".to_string(),
            },
            game: None,
        }
    }

    fn make_game_event(id: u64, completed: bool) -> LegacyMatchEvent {
        LegacyMatchEvent {
            id,
            timestamp: "2026-06-01T12:30:00Z".to_string(),
            user_id: None,
            detail: LegacyMatchEventDetail {
                event_type: "other".to_string(),
                text: "Game".to_string(),
            },
            game: Some(LegacyMatchGame {
                beatmap_id: Some(987),
                beatmap: None,
                beatmapset: None,
                end_time: completed.then(|| "2026-06-01T12:35:00Z".to_string()),
                mods: Vec::new(),
                team_type: String::new(),
                scoring_type: String::new(),
                scores: Vec::new(),
            }),
        }
    }

    fn make_response(events: Vec<LegacyMatchEvent>, latest_event_id: u64) -> LegacyMatchResponse {
        LegacyMatchResponse {
            match_info: LegacyMatchInfo {
                id: 42,
                name: "Test Match".to_string(),
                start_time: None,
                end_time: None,
            },
            users: Vec::new(),
            first_event_id: events.first().map(|event| event.id),
            latest_event_id,
            cursor_string: None,
            events,
        }
    }

    #[test]
    fn start_listener_returns_confirmation() {
        let action = MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        };
        let result = MlActionResult::Started {
            qq: 100,
            match_name: "OWC Finals".to_string(),
        };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("12345678"));
        assert!(resp.contains("OWC Finals"));
        assert!(resp.contains("已开始监听"));
    }

    #[test]
    fn repeated_start_keeps_success_semantics() {
        let action = MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        };
        let result = MlActionResult::Started {
            qq: 100,
            match_name: "MP #12345678".to_string(),
        };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("已开始监听"));
        assert!(!resp.contains("已在监听"));
    }

    #[test]
    fn listener_is_effectively_active_only_when_unexpired() {
        let active_unexpired = MatchListener {
            match_id: 1,
            group_id: 2,
            creator_qq: 3,
            match_name: "Test Match".to_string(),
            last_event_id: None,
            last_notified_event_id: None,
            pending_game_event_id: None,
            created_at: chrono::Utc::now(),
            expires_at: 110,
            active: true,
            last_notified_at: None,
        };
        let active_expired = MatchListener {
            expires_at: 90,
            ..active_unexpired.clone()
        };

        assert!(is_listener_effectively_active(&active_unexpired, 100));
        assert!(!is_listener_effectively_active(&active_expired, 100));
    }

    #[test]
    fn initial_cursor_replays_current_in_progress_game() {
        let response = make_response(vec![make_lobby_event(10), make_game_event(11, false)], 11);

        let cursor = initial_cursor_from_response(&response, 0);

        assert_eq!(cursor.last_event_id, Some(10));
        assert_eq!(cursor.last_notified_event_id, Some(10));
    }

    #[test]
    fn initial_cursor_skips_requested_existing_games() {
        let response = make_response(
            vec![
                make_lobby_event(10),
                make_game_event(11, true),
                make_game_event(12, false),
            ],
            12,
        );

        let cursor = initial_cursor_from_response(&response, 1);

        assert_eq!(cursor.last_event_id, Some(11));
        assert_eq!(cursor.last_notified_event_id, Some(11));
    }

    #[test]
    fn initial_cursor_defaults_to_latest_when_no_current_game() {
        let response = make_response(vec![make_lobby_event(10), make_game_event(11, true)], 11);

        let cursor = initial_cursor_from_response(&response, 0);

        assert_eq!(cursor.last_event_id, Some(11));
        assert_eq!(cursor.last_notified_event_id, Some(11));
    }

    #[test]
    fn stop_listener_returns_confirmation() {
        let action = MatchListenAction::Stop { match_id: 12345678 };
        let result = MlActionResult::Stopped { qq: 100 };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("12345678"));
        assert!(resp.contains("已停止监听"));
    }

    #[test]
    fn stop_all_only_affects_current_group() {
        let action = MatchListenAction::StopAll;
        let result = MlActionResult::StoppedAll { qq: 100, count: 2 };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("2"));
        assert!(resp.contains("已停止监听本群所有比赛"));
    }

    #[test]
    fn list_empty_returns_chinese() {
        let action = MatchListenAction::List;
        let result = MlActionResult::List {
            qq: 100,
            listeners: vec![],
        };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("当前没有监听任何比赛"));
    }

    #[test]
    fn status_missing_returns_not_found() {
        let action = MatchListenAction::Status { match_id: 999 };
        let result = MlActionResult::NotFound { qq: 100 };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("未找到比赛"));
        assert!(resp.contains("999"));
    }

    #[test]
    fn status_active_returns_details() {
        let action = MatchListenAction::Status { match_id: 12345678 };
        let result = MlActionResult::Status {
            qq: 100,
            match_name: "Test Match".to_string(),
            status: "进行中".to_string(),
            players: 4,
            games_played: 5,
        };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("Test Match"));
        assert!(resp.contains("进行中"));
    }

    #[test]
    fn limit_exceeded_returns_chinese_error() {
        let action = MatchListenAction::Start {
            match_id: 1,
            skip_rounds: 0,
        };
        let result = MlActionResult::Error {
            qq: 100,
            message: user_str("ml.limit_exceeded").replace("{limit}", "3"),
        };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("监听数量已达上限"));
        assert!(resp.contains("3"));
    }

    #[test]
    fn already_listening_returns_chinese_error() {
        let action = MatchListenAction::Start {
            match_id: 1,
            skip_rounds: 0,
        };
        let result = MlActionResult::Error {
            qq: 100,
            message: user_str("ml.already_listening").replace("{match_id}", "1"),
        };
        let resp = format_ml_response(&action, &result);
        assert!(resp.contains("已在监听"));
    }

    #[test]
    fn fetch_failed_error_does_not_double_mention() {
        let action = MatchListenAction::Start {
            match_id: 42,
            skip_rounds: 0,
        };
        let result = MlActionResult::Error {
            qq: 100,
            message: user_str("ml.fetch_failed").replace("{qq}", "100"),
        };
        let resp = format_ml_response(&action, &result);

        assert_eq!(resp.matches("[CQ:at,qq=100]").count(), 1);
        assert!(resp.contains("获取比赛信息失败"));
    }
}
