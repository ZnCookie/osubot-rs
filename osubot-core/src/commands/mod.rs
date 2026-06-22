mod common;
mod scoring;

#[cfg(test)]
mod tests;

use crate::types::{Command, GameMode};
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
        return Some(Command::ProfileCard {
            username: Some(rest.to_string()),
            qq: None,
        });
    }

    if let Some(cmd) = parse_scoring_command(&msg, mentioned_user_id) {
        return cmd;
    }

    None
}
