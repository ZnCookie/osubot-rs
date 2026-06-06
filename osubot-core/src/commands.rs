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
/// - `where qq=<QQ号>` - 查询QQ绑定的 osu! 用户 std
/// - `where qq=<QQ号>,<模式>` - 查询QQ绑定的 osu! 用户指定模式
/// - `绑定 <osu用户名>` - 绑定账号
/// - `解绑` - 解绑账号
pub fn parse_command(msg: &str, mentioned_user_id: Option<i64>) -> Option<Command> {
    let msg = msg.trim();
    // Normalize fullwidth characters to ASCII equivalents
    let msg = msg.replace('～', "~").replace('，', ",").replace('！', "!");

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

    // 查询他人 via QQ: where qq=<QQ号> [, 模式]
    if let Some(rest) = msg.strip_prefix("where qq=") {
        let parts: Vec<&str> = rest.split(',').collect();
        let qq: i64 = parts[0].trim().parse().ok()?;
        let mode = if parts.len() > 1 {
            GameMode::from_mode_str(parts[1].trim())?
        } else {
            GameMode::Osu
        };
        return Some(Command::QueryMentionedUser { qq, mode });
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
    // Must be checked before !p/!r to avoid "!profile" being matched as "!p" + "rofile"
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

    // Pass/Recent score commands: !p, !r, !ps, !rs
    // Format: !p [username] [:mode] [#N]
    // Negative lookahead: command letter must NOT be followed by another letter/digit/underscore/hyphen.
    // This prevents !pv, !profile, !rabc from matching while allowing !p, !p v, !p:3, !p#5.
    for (prefix, is_pass, default_limit) in [
        ("!ps", true, 20u32),
        ("!rs", false, 20u32),
        ("!p", true, 1u32),
        ("!r", false, 1u32),
    ] {
        if let Some(rest) = msg.strip_prefix(prefix) {
            // Negative lookahead: skip if command is immediately followed by a word character
            if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                continue;
            }
            let rest = rest.trim();
            // Parse #N suffix (from end)
            let (rest, limit) = if let Some(hash_pos) = rest.rfind('#') {
                let num_str = &rest[hash_pos + 1..];
                match num_str.parse::<u32>() {
                    Ok(n) if n >= 1 => (rest[..hash_pos].trim(), n.min(100)),
                    Ok(_) => (rest[..hash_pos].trim(), default_limit),
                    _ => (rest[..hash_pos].trim(), default_limit),
                }
            } else {
                (rest, default_limit)
            };
            // Parse :mode suffix (from end, after #N removal)
            let (username_part, mode) = if let Some(colon_pos) = rest.rfind(':') {
                let mode_str = &rest[colon_pos + 1..];
                if mode_str.is_empty() {
                    // Bare colon with no mode string — treat as no mode specified
                    (rest[..colon_pos].trim(), GameMode::Osu)
                } else {
                    match GameMode::from_mode_str(mode_str) {
                        Some(mode) => (rest[..colon_pos].trim(), mode),
                        None => return None, // Invalid mode string, ignore command
                    }
                }
            } else {
                (rest, GameMode::Osu)
            };
            let (username, qq) = if username_part.is_empty() {
                if let Some(qq) = mentioned_user_id {
                    (None, Some(qq))
                } else {
                    (None, None)
                }
            } else {
                (Some(username_part.to_string()), None)
            };
            return Some(if is_pass {
                Command::Pass {
                    mode,
                    username,
                    qq,
                    limit,
                    is_summary: default_limit > 1,
                }
            } else {
                Command::Recent {
                    mode,
                    username,
                    qq,
                    limit,
                    is_summary: default_limit > 1,
                }
            });
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::CommandGroup;

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

    #[test]
    fn test_pass_self() {
        let cmd = parse_command("!p", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_pass_mode() {
        let cmd = parse_command("!p :1", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Taiko,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_pass_username() {
        let cmd = parse_command("!p ZnCookie", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_pass_username_mode() {
        let cmd = parse_command("!p ZnCookie :2", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Catch,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_recent_self() {
        let cmd = parse_command("!r", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_recent_multiple() {
        let cmd = parse_command("!rs", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 20,
                is_summary: true,
            }
        );
    }

    #[test]
    fn test_pass_mention() {
        let cmd = parse_command("!p", Some(123456)).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: Some(123456),
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_pass_no_conflict_with_profile() {
        // !profile should still work
        let cmd = parse_command("!profile", None).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: None,
                qq: None
            }
        );
    }

    #[test]
    fn test_pass_mode_no_space() {
        let cmd = parse_command("!p:3", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Mania,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_pass_with_hash() {
        let cmd = parse_command("!p #2", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 2,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_pass_multiple_with_hash() {
        let cmd = parse_command("!ps #5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 5,
                is_summary: true,
            }
        );
    }

    #[test]
    fn test_pass_multiple_username_hash() {
        let cmd = parse_command("!ps ZnCookie #3", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 3,
                is_summary: true,
            }
        );
    }

    #[test]
    fn test_pass_multiple_username_mode_hash() {
        let cmd = parse_command("!ps ZnCookie :2 #5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Catch,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 5,
                is_summary: true,
            }
        );
    }

    #[test]
    fn test_hash_clamp_100() {
        let cmd = parse_command("!ps #200", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 100,
                is_summary: true,
            }
        );
    }

    #[test]
    fn test_hash_zero_ignored() {
        let cmd = parse_command("!p #0", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_hash_garbage_ignored_with_username() {
        let cmd = parse_command("!p ZnCookie #xyz", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_hash_garbage_ignored_self() {
        let cmd = parse_command("!p #xyz", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_recent_with_hash() {
        let cmd = parse_command("!r #3", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 3,
                is_summary: false,
            }
        );
    }

    #[test]
    fn test_recent_multiple_with_hash() {
        let cmd = parse_command("!rs #5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 5,
                is_summary: true,
            }
        );
    }

    #[test]
    fn parse_ps_invalid_mode_returns_none() {
        let result = parse_command("!ps :xyz", None);
        assert!(result.is_none());
    }

    #[test]
    fn test_fullwidth_exclamation() {
        let cmd = parse_command("！profile", None).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: None,
                qq: None,
            }
        );

        let cmd = parse_command("！p", None).unwrap();
        assert!(matches!(cmd, Command::Pass { .. }));

        let cmd = parse_command("！rs", None).unwrap();
        assert!(matches!(cmd, Command::Recent { .. }));
    }

    #[test]
    fn parse_ps_empty_mode_defaults_to_osu() {
        let result = parse_command("!ps :", None);
        assert_eq!(
            result,
            Some(Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 20,
                is_summary: true,
            })
        );
    }

    #[test]
    fn test_command_group_name() {
        assert_eq!(
            Command::QuerySelf {
                mode: GameMode::Osu
            }
            .group_name(),
            CommandGroup::Query
        );
        assert_eq!(
            Command::QueryUser {
                username: "x".into(),
                mode: GameMode::Osu
            }
            .group_name(),
            CommandGroup::Query
        );
        assert_eq!(
            Command::QueryMentionedUser {
                qq: 1,
                mode: GameMode::Osu
            }
            .group_name(),
            CommandGroup::Query
        );
        assert_eq!(
            Command::Bind {
                username: "x".into()
            }
            .group_name(),
            CommandGroup::Bind
        );
        assert_eq!(Command::Unbind.group_name(), CommandGroup::Bind);
        assert_eq!(
            Command::Highlight {
                mode: GameMode::Osu
            }
            .group_name(),
            CommandGroup::Highlight
        );
        assert_eq!(
            Command::ProfileCard {
                username: None,
                qq: None
            }
            .group_name(),
            CommandGroup::Profile
        );
        assert_eq!(
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false
            }
            .group_name(),
            CommandGroup::Score
        );
        assert_eq!(
            Command::Recent {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                is_summary: false
            }
            .group_name(),
            CommandGroup::Score
        );
    }

    #[test]
    fn test_pv_not_matched() {
        assert!(parse_command("!pv", None).is_none());
    }

    #[test]
    fn test_rabc_not_matched() {
        assert!(parse_command("!rabc", None).is_none());
    }

    #[test]
    fn test_punderscore_not_matched() {
        assert!(parse_command("!p_test", None).is_none());
    }

    #[test]
    fn test_phyphen_not_matched() {
        assert!(parse_command("!r-test", None).is_none());
    }

    #[test]
    fn test_where_qq_basic() {
        let cmd = parse_command("where qq=1234567", None).unwrap();
        assert_eq!(
            cmd,
            Command::QueryMentionedUser {
                qq: 1234567,
                mode: GameMode::Osu,
            }
        );
    }

    #[test]
    fn test_where_qq_with_mode() {
        let cmd = parse_command("where qq=1234567,1", None).unwrap();
        assert_eq!(
            cmd,
            Command::QueryMentionedUser {
                qq: 1234567,
                mode: GameMode::Taiko,
            }
        );
    }

    #[test]
    fn test_where_qq_invalid_number() {
        assert!(parse_command("where qq=abc", None).is_none());
    }

    #[test]
    fn test_where_qq_empty() {
        assert!(parse_command("where qq=", None).is_none());
    }

    #[test]
    fn test_where_qq_invalid_mode() {
        assert!(parse_command("where qq=123,99", None).is_none());
    }

    #[test]
    fn test_profile_not_matched_as_p() {
        // !profile should match ProfileCard, not Pass
        let cmd = parse_command("!profile", None).unwrap();
        assert!(matches!(cmd, Command::ProfileCard { .. }));
    }

    #[test]
    fn test_ps_not_affected() {
        // !ps should still work
        let cmd = parse_command("!ps", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 20,
                is_summary: true,
            }
        );
    }
}
