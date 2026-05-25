use crate::types::{Command, GameMode};

/// 解析用户消息为命令
/// 支持格式:
/// - `~` / `~0` - 查询自己 std
/// - `~1` / `~,1` - 查询自己 taiko
/// - `~2` / `~,2` - 查询自己 catch
/// - `~3` / `~,3` - 查询自己 mania
/// - `where <用户名>` - 查询他人 std
/// - `where <用户名>,<模式>` - 查询他人指定模式
/// - `查@<QQ用户>` - 查询他人 std (via QQ mention)
/// - `查@<QQ用户>,<模式>` - 查询他人指定模式 (via QQ mention)
/// - `绑定 <osu用户名>` - 绑定账号
/// - `解绑` - 解绑账号
pub fn parse_command(msg: &str, mentioned_user_id: Option<i64>) -> Option<Command> {
    let msg = msg.trim();
    // Normalize fullwidth characters to ASCII equivalents
    let msg = msg.replace('～', "~").replace('，', ",");

    // 查询自己: ~ 或 ~<模式>
    if msg.starts_with('~') {
        let rest = msg
            .trim_start_matches('~')
            .trim_start_matches(',')
            .trim_start_matches(' ')
            .trim_start_matches(',');
        if rest.is_empty() {
            return Some(Command::QuerySelf {
                mode: GameMode::Osu,
            });
        }
        let mode = GameMode::from_mode_str(rest)?;
        return Some(Command::QuerySelf { mode });
    }

    // 查询他人: where <用户名> [, 模式]
    if let Some(rest) = msg.strip_prefix("where ") {
        let parts: Vec<&str> = rest.split(',').collect();
        let username = parts[0].trim().to_string();
        let mode = if parts.len() > 1 {
            GameMode::from_mode_str(parts[1].trim())?
        } else {
            GameMode::Osu
        };
        return Some(Command::QueryUser { username, mode });
    }

    // 查询他人 via QQ mention: 查@<QQ用户> [, 模式]
    if msg.starts_with('查') {
        if let Some(qq) = mentioned_user_id {
            let rest = msg.strip_prefix('查').unwrap();
            let rest = rest.trim_start_matches(',').trim().trim_start_matches(',');
            let mode = if rest.is_empty() {
                GameMode::Osu
            } else {
                GameMode::from_mode_str(rest)?
            };
            return Some(Command::QueryMentionedUser { qq, mode });
        }
        // bare 查 with no mention → no command
        return None;
    }

    // 绑定: 绑定 <osu用户名>
    if let Some(username) = msg.strip_prefix("绑定 ") {
        let username = username.trim();
        if username.is_empty() {
            return None;
        }
        return Some(Command::Bind {
            username: username.to_string(),
        });
    }

    // 解绑
    if msg == "解绑" {
        return Some(Command::Unbind);
    }

    // 今日高光: 今日高光 [, 模式]
    if let Some(rest) = msg.strip_prefix("今日高光") {
        let rest = rest.trim();
        let mode = if rest.is_empty() || rest.starts_with(',') {
            let mode_str = rest.trim_start_matches(',').trim();
            if mode_str.is_empty() {
                GameMode::Osu
            } else {
                GameMode::from_mode_str(mode_str).unwrap_or(GameMode::Osu)
            }
        } else {
            GameMode::from_mode_str(rest).unwrap_or(GameMode::Osu)
        };
        return Some(Command::Highlight { mode });
    }

    // 个人主页卡片: !profile [用户名] or !profile + @mention
    if let Some(rest) = msg.strip_prefix("!profile") {
        let rest = rest.trim();
        if rest.is_empty() {
            // !profile alone — could be self or mention
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
        // !profile <username>
        return Some(Command::ProfileCard {
            username: Some(rest.to_string()),
            qq: None,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_self() {
        let cmd = parse_command("!profile", None).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: None,
                qq: None,
            }
        );
    }

    #[test]
    fn test_profile_mention() {
        let cmd = parse_command("!profile", Some(123456)).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: None,
                qq: Some(123456),
            }
        );
    }

    #[test]
    fn test_profile_with_username() {
        let cmd = parse_command("!profile ZnCookie", None).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: Some("ZnCookie".to_string()),
                qq: None,
            }
        );
    }

    #[test]
    fn test_profile_username_with_mention() {
        let cmd = parse_command("!profile ZnCookie", Some(123456)).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: Some("ZnCookie".to_string()),
                qq: None,
            }
        );
    }

    #[test]
    fn test_profile_with_spaces_around_username() {
        let cmd = parse_command("!profile  ZnCookie  ", None).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: Some("ZnCookie".to_string()),
                qq: None,
            }
        );
    }
}
