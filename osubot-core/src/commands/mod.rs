mod common;
mod scoring;

#[cfg(test)]
mod tests;

use crate::types::{Command, GameMode, MatchListenAction};
use scoring::parse_scoring_command;

/// 解析用户消息为命令
pub fn parse_command(msg: &str, mentioned_user_id: Option<i64>) -> Option<Command> {
    let msg = msg.trim();
    let msg = msg.replace('～', "~").replace('，', ",").replace('！', "!");

    if msg.starts_with('~') {
        let rest = msg
            .trim_start_matches('~')
            .trim_start_matches(',')
            .trim_start_matches(' ')
            .trim_start_matches(',');
        if rest.is_empty() {
            return Some(Command::QuerySelf { mode: None });
        }
        let mode = GameMode::from_digit_str(rest)?;
        return Some(Command::QuerySelf { mode: Some(mode) });
    }

    if let Some(rest) = msg.strip_prefix("where qq=") {
        let parts: Vec<&str> = rest.split(',').collect();
        let qq: i64 = parts[0].trim().parse().ok()?;
        let mode = if parts.len() > 1 {
            Some(GameMode::from_digit_str(parts[1].trim())?)
        } else {
            None
        };
        return Some(Command::QueryMentionedUser { qq, mode });
    }

    if let Some(rest) = msg.strip_prefix("where ") {
        let parts: Vec<&str> = rest.split(',').collect();
        let first = parts[0].trim();
        if let Some(at) = first.strip_prefix('@') {
            if let Ok(qq) = at.parse::<i64>() {
                let mode = if parts.len() > 1 {
                    Some(GameMode::from_digit_str(parts[1].trim())?)
                } else {
                    None
                };
                return Some(Command::QueryMentionedUser { qq, mode });
            }
            return None;
        }
        let username = first.to_string();
        let mode = if parts.len() > 1 {
            Some(GameMode::from_digit_str(parts[1].trim())?)
        } else {
            None
        };
        return Some(Command::QueryUser { username, mode });
    }

    if msg.starts_with('查') {
        if let Some(qq) = mentioned_user_id {
            let rest = msg.strip_prefix('查').unwrap();
            let rest = rest.trim_start_matches(',').trim().trim_start_matches(',');
            let mode = if rest.is_empty() {
                None
            } else {
                Some(GameMode::from_digit_str(rest)?)
            };
            return Some(Command::QueryMentionedUser { qq, mode });
        }
        return None;
    }

    if let Some(username) = msg.strip_prefix("绑定 ") {
        let username = username.trim();
        if username.is_empty() {
            return None;
        }
        return Some(Command::Bind {
            username: username.to_string(),
        });
    }

    if msg == "解绑" {
        return Some(Command::Unbind);
    }

    if let Some(rest) = msg.strip_prefix("今日高光") {
        let rest = rest.trim();
        let mode = if rest.is_empty() || rest.starts_with(',') {
            let mode_str = rest.trim_start_matches(',').trim();
            if mode_str.is_empty() {
                None
            } else {
                GameMode::from_digit_str(mode_str)
            }
        } else {
            GameMode::from_digit_str(rest)
        };
        return Some(Command::Highlight { mode });
    }

    if msg == "!help" {
        return Some(Command::Help);
    }

    if let Some(rest) = msg.strip_prefix("!mode") {
        if rest.is_empty() || rest.chars().next().is_some_and(|c| c.is_whitespace()) {
            let rest = rest.trim();
            let mode = if rest.is_empty() {
                None
            } else {
                GameMode::from_digit_str(rest)
            };
            return Some(Command::SetDefaultMode { mode });
        }
    }

    if let Some(rest) = msg.strip_prefix("!profile") {
        let rest = rest.trim();
        if rest.is_empty() {
            if let Some(qq) = mentioned_user_id {
                return Some(Command::ProfileCard {
                    username: None,
                    qq: Some(qq),
                });
            }
            return Some(Command::ProfileCard {
                username: None,
                qq: None,
            });
        }
        if let Some(at) = rest.strip_prefix('@') {
            if let Ok(parsed) = at.parse::<i64>() {
                return Some(Command::ProfileCard {
                    username: None,
                    qq: Some(parsed),
                });
            }
            return None;
        }
        if let Some(qq_str) = rest.strip_prefix("qq=") {
            if let Ok(parsed) = qq_str.parse::<i64>() {
                return Some(Command::ProfileCard {
                    username: None,
                    qq: Some(parsed),
                });
            }
            return None;
        }
        return Some(Command::ProfileCard {
            username: Some(rest.to_string()),
            qq: None,
        });
    }

    if let Some(cmd) = parse_scoring_command(&msg, mentioned_user_id) {
        return cmd;
    }

    for prefix in ["!ml", "!li"] {
        if let Some(rest) = msg.strip_prefix(prefix) {
            let rest = rest.trim();
            if rest.is_empty() {
                return None;
            }
            return parse_ml_subcommand(rest);
        }
    }

    None
}

/// Parse the subcommand part of `!ml` after the prefix is stripped.
fn parse_ml_subcommand(rest: &str) -> Option<Command> {
    let (rest, skip_rounds) = parse_ml_skip_suffix(rest)?;
    let rest = rest.trim();

    // Subcommand: stop <match_id|url|mpid> or stop all
    if let Some(arg) = strip_ml_operation_prefix(rest, &["stop", "p", "end", "e", "off", "f"]) {
        let arg = arg.trim();
        if arg == "all" {
            return Some(Command::MatchListen(MatchListenAction::StopAll));
        }
        return parse_ml_match_id(arg)
            .map(|match_id| Command::MatchListen(MatchListenAction::Stop { match_id }));
    }
    if is_ml_operation(rest, &["stop", "p", "end", "e", "off", "f"]) {
        return None;
    }

    // Subcommand: status <match_id|url|mpid>
    if let Some(arg) = strip_ml_operation_prefix(rest, &["status"]) {
        let arg = arg.trim();
        return parse_ml_match_id(arg)
            .map(|match_id| Command::MatchListen(MatchListenAction::Status { match_id }));
    }
    if rest == "status" {
        return None;
    }

    // Subcommand: list
    if is_ml_operation(rest, &["list", "l", "info", "i"]) {
        return Some(Command::MatchListen(MatchListenAction::List));
    }

    if let Some(arg) = strip_ml_operation_suffix(rest, &["start", "s", "on", "o"]) {
        return parse_ml_match_id(arg).map(|match_id| {
            Command::MatchListen(MatchListenAction::Start {
                match_id,
                skip_rounds,
            })
        });
    }

    if let Some(arg) = strip_ml_operation_suffix(rest, &["stop", "p", "end", "e", "off", "f"]) {
        return parse_ml_match_id(arg)
            .map(|match_id| Command::MatchListen(MatchListenAction::Stop { match_id }));
    }

    if let Some(arg) = strip_ml_operation_suffix(rest, &["list", "l", "info", "i"]) {
        if parse_ml_match_id(arg).is_some() {
            return Some(Command::MatchListen(MatchListenAction::List));
        }
    }

    // Default to Start action with a parsed match ID
    parse_ml_match_id(rest).map(|match_id| {
        Command::MatchListen(MatchListenAction::Start {
            match_id,
            skip_rounds,
        })
    })
}

fn parse_ml_skip_suffix(rest: &str) -> Option<(&str, u32)> {
    // parse_command() 会先尝试成绩查询 grammar，再尝试 `!ml`。因此这里接受
    // trailing `#N` 只会影响 `!ml` 自己的 skip 语法，不会抢走 `!p #3` 等 scoring 命令。
    let rest = rest.trim();
    let Some((command, skip)) = rest.rsplit_once('#') else {
        return Some((rest, 0));
    };

    let skip_rounds = skip.trim().parse::<u32>().ok()?;
    if !(1..=100).contains(&skip_rounds) {
        return None;
    }
    Some((command.trim(), skip_rounds))
}

fn is_ml_operation(input: &str, operations: &[&str]) -> bool {
    operations.contains(&input)
}

fn strip_ml_operation_prefix<'a>(input: &'a str, operations: &[&str]) -> Option<&'a str> {
    operations
        .iter()
        .find_map(|operation| input.strip_prefix(&format!("{operation} ")))
}

fn strip_ml_operation_suffix<'a>(input: &'a str, operations: &[&str]) -> Option<&'a str> {
    let (arg, operation) = input.rsplit_once(' ')?;
    is_ml_operation(operation.trim(), operations).then_some(arg.trim())
}

/// Parse a match identifier: raw numeric, mp{id}, or community match URL.
///
/// # ponytail
/// v1 rejects lazer room URLs (`/multiplayer/rooms/`) intentionally.
/// Lazer rooms use a different API endpoint and are out of scope for legacy match listening.
fn parse_ml_match_id(input: &str) -> Option<u64> {
    let input = input.trim();

    // Raw numeric ID
    if let Ok(id) = input.parse::<u64>() {
        return Some(id);
    }

    // mp{id}
    if let Some(id_str) = input.strip_prefix("mp") {
        if let Ok(id) = id_str.parse::<u64>() {
            return Some(id);
        }
    }

    // https://osu.ppy.sh/community/matches/{id}
    if let Some(path) = input.strip_prefix("https://osu.ppy.sh/community/matches/") {
        if let Ok(id) = path.trim_end_matches('/').parse::<u64>() {
            return Some(id);
        }
    }

    // ponytail: v1 rejects lazer room URLs intentionally; they use a different API
    // and are out of scope for legacy match listening.
    if input.contains("osu.ppy.sh/multiplayer/rooms/") {
        return None;
    }

    None
}
