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
        let first = parts[0].trim();
        // where @<QQ> — 按 QQ 查询
        if let Some(at) = first.strip_prefix('@') {
            if let Ok(qq) = at.parse::<i64>() {
                let mode = if parts.len() > 1 {
                    GameMode::from_mode_str(parts[1].trim())?
                } else {
                    GameMode::Osu
                };
                return Some(Command::QueryMentionedUser { qq, mode });
            }
            return None;
        }
        let username = first.to_string();
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
                GameMode::from_mode_str(mode_str).unwrap_or_else(|| {
                    tracing::debug!(
                        mode_str,
                        "今日高光: failed to parse mode, falling back to osu"
                    );
                    GameMode::Osu
                })
            }
        } else {
            GameMode::from_mode_str(rest).unwrap_or_else(|| {
                tracing::debug!(
                    mode_str = rest,
                    "今日高光: failed to parse mode, falling back to osu"
                );
                GameMode::Osu
            })
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

    // Beatmap score command: !s, !ss
    // Format: !s [<username>|@QQ] <beatmap_id|score_id> [:<mode>] [+<mods>] [#<N>]
    // Format: !ss [<username>|@QQ] <beatmap_id> [:<mode>]
    let s_cmds: &[(&str, bool)] = &[("!ss", true), ("!s", false)];
    for &(prefix, is_all) in s_cmds {
        if let Some(rest) = msg.strip_prefix(prefix) {
            if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                continue;
            }
            let rest = rest.trim();
            // Parse +mods suffix
            let (rest, mods) = if let Some(plus_pos) = rest.rfind('+') {
                let mod_str = &rest[plus_pos + 1..];
                if mod_str.len() >= 2 && mod_str.chars().all(|c| c.is_ascii_alphabetic()) {
                    let parsed: Vec<String> = mod_str
                        .chars()
                        .collect::<Vec<_>>()
                        .chunks(2)
                        .map(|chunk| chunk.iter().collect::<String>().to_uppercase())
                        .collect();
                    (rest[..plus_pos].trim(), Some(parsed))
                } else {
                    (rest, None)
                }
            } else {
                (rest, None)
            };
            // Parse #N suffix
            let (rest, limit) = if let Some(hash_pos) = rest.rfind('#') {
                let num_str = &rest[hash_pos + 1..];
                match num_str.parse::<u32>() {
                    Ok(n) if n >= 1 => (rest[..hash_pos].trim(), n.min(100)),
                    Ok(_) => (rest[..hash_pos].trim(), 1),
                    _ => (rest[..hash_pos].trim(), 1),
                }
            } else {
                (rest, 1)
            };
            // Parse :mode suffix
            let (username_part, mode) = if let Some(colon_pos) = rest.rfind(':') {
                let mode_str = &rest[colon_pos + 1..];
                if mode_str.is_empty() {
                    (rest[..colon_pos].trim(), GameMode::Osu)
                } else {
                    match GameMode::from_mode_str(mode_str) {
                        Some(mode) => (rest[..colon_pos].trim(), mode),
                        None => return None,
                    }
                }
            } else {
                (rest, GameMode::Osu)
            };
            // Parse beatmap_id / score_id / username from remaining
            let (beatmap_id, score_id, username, qq) = if username_part.is_empty() {
                (None, None, None, mentioned_user_id)
            } else {
                let tokens: Vec<&str> = username_part.split_whitespace().collect();
                let mut bid: Option<u32> = None;
                let mut sid: Option<u64> = None;
                let mut uname: Option<String> = None;
                let mut qq_id: Option<i64> = None;
                let mut name_parts: Vec<&str> = Vec::new();
                let mut hit_numeric = false;

                for token in tokens {
                    if let Some(at) = token.strip_prefix('@') {
                        if let Ok(parsed) = at.parse::<i64>() {
                            qq_id = Some(parsed);
                        } else {
                            return None;
                        }
                        continue;
                    }
                    if let Ok(num) = token.parse::<u64>() {
                        hit_numeric = true;
                        if num >= 10_000_000 {
                            sid = Some(num);
                        } else if num <= 9_999_999 && bid.is_none() {
                            bid = Some(num as u32);
                        }
                    } else if !hit_numeric {
                        name_parts.push(token);
                    } else {
                        return None;
                    }
                }
                if !name_parts.is_empty() {
                    uname = Some(name_parts.join(" "));
                }
                (bid, sid, uname, qq_id)
            };
            // 互斥：不能同时提供用户名和 @QQ
            if username.is_some() && qq.is_some() {
                return None;
            }
            // If no user and no mention, resolve as self
            let (username, qq) = if username.is_none() && qq.is_none() {
                if let Some(qq_val) = mentioned_user_id {
                    (None, Some(qq_val))
                } else {
                    (None, None)
                }
            } else {
                (username, qq)
            };

            return Some(Command::ScoreOnBeatmap {
                mode,
                username,
                qq,
                beatmap_id,
                score_id,
                mods,
                limit,
                is_all,
            });
        }
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
            } else if let Some(at) = username_part.strip_prefix('@') {
                if let Ok(parsed) = at.parse::<i64>() {
                    (None, Some(parsed))
                } else {
                    return None;
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
    fn test_profile_qq_in_text() {
        let cmd = parse_command("!profile @123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ProfileCard {
                username: None,
                qq: Some(123456),
            }
        );
    }

    #[test]
    fn test_profile_qq_in_text_non_numeric_returns_none() {
        assert!(parse_command("!profile @ZnCookie", None).is_none());
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
    fn test_pass_qq_in_text() {
        let cmd = parse_command("!p @123456", None).unwrap();
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
    fn test_pass_qq_in_text_non_numeric_returns_none() {
        assert!(parse_command("!p @ZnCookie", None).is_none());
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
    fn test_where_qq_in_text() {
        let cmd = parse_command("where @1234567", None).unwrap();
        assert_eq!(
            cmd,
            Command::QueryMentionedUser {
                qq: 1234567,
                mode: GameMode::Osu,
            }
        );
    }

    #[test]
    fn test_where_qq_in_text_with_mode() {
        let cmd = parse_command("where @1234567,1", None).unwrap();
        assert_eq!(
            cmd,
            Command::QueryMentionedUser {
                qq: 1234567,
                mode: GameMode::Taiko,
            }
        );
    }

    #[test]
    fn test_where_qq_in_text_non_numeric_returns_none() {
        assert!(parse_command("where @ZnCookie", None).is_none());
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

    #[test]
    fn test_score_on_beatmap_basic() {
        let cmd = parse_command("!s 123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                limit: 1,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_with_mods() {
        let cmd = parse_command("!s 123456 +HDDT", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: Some(vec!["HD".to_string(), "DT".to_string()]),
                limit: 1,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_with_mode_and_username() {
        let cmd = parse_command("!s ZnCookie 123456 :2", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Catch,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                limit: 1,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_score_id() {
        let cmd = parse_command("!s 12345678901", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: Some(12345678901),
                mods: None,
                limit: 1,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_all() {
        let cmd = parse_command("!ss 123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                limit: 1,
                is_all: true,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_no_conflict_with_ps() {
        let cmd = parse_command("!ps", None).unwrap();
        assert!(matches!(cmd, Command::Pass { .. }));
    }

    #[test]
    fn test_score_on_beatmap_with_limit() {
        let cmd = parse_command("!s 123456 #5", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                limit: 5,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_qq_in_text() {
        let cmd = parse_command("!s @123456 789012", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: Some(123456),
                beatmap_id: Some(789012),
                score_id: None,
                mods: None,
                limit: 1,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_username_and_qq_mutually_exclusive() {
        assert!(parse_command("!s ZnCookie @999 123", None).is_none());
        assert!(parse_command("!s @999 ZnCookie 123", None).is_none());
    }

    #[test]
    fn test_score_on_beatmap_at_non_numeric_returns_none() {
        assert!(parse_command("!s @ZnCookie 123456", None).is_none());
    }

    #[test]
    fn test_score_on_beatmap_multi_word_username() {
        let cmd = parse_command("!s My Name 123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: Some("My Name".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                limit: 1,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_multi_word_username_with_mode() {
        let cmd = parse_command("!s Zhang San 123456 :2 #3 +HD", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Catch,
                username: Some("Zhang San".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: Some(vec!["HD".to_string()]),
                limit: 3,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_single_word_username_still_works() {
        let cmd = parse_command("!s ZnCookie 123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                limit: 1,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_username_after_numeric_is_error() {
        assert!(parse_command("!s 123456 TrailingName", None).is_none());
    }
}
