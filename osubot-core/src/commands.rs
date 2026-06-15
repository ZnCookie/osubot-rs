use crate::types::{Command, GameMode};

const MAX_LIMIT: u32 = 100;
const SCORE_ID_THRESHOLD: u64 = 10_000_000;

/// Extract `:mode` suffix from rest. Returns (rest_without_mode, mode).
/// Finds the rightmost colon and extracts only the first token after it as mode.
/// The rest of the content (before and after the mode token) is preserved.
/// Invalid mode strings silently fall back to GameMode::Osu.
fn extract_mode(rest: &str) -> (String, GameMode) {
    if let Some(colon_pos) = rest.rfind(':') {
        let after_colon = &rest[colon_pos + 1..];
        let mode_token = after_colon.split_whitespace().next().unwrap_or("");
        let mode = GameMode::from_mode_str(mode_token).unwrap_or(GameMode::Osu);

        // Reconstruct the rest without the :mode part
        let before_colon = &rest[..colon_pos];
        let after_mode_token = &rest[colon_pos + 1 + mode_token.len()..];
        let new_rest = format!("{} {}", before_colon.trim(), after_mode_token.trim())
            .trim()
            .to_string();

        (new_rest, mode)
    } else {
        (rest.to_string(), GameMode::Osu)
    }
}

/// Parse a limit string like "5" or "2-10" into (limit, limit_end).
/// Values are clamped to [1, MAX_LIMIT].
fn parse_limit(num_str: &str) -> (u32, Option<u32>) {
    if let Some(dash_pos) = num_str.find('-') {
        let start = num_str[..dash_pos]
            .parse::<u32>()
            .unwrap_or(1)
            .clamp(1, MAX_LIMIT);
        let end = num_str[dash_pos + 1..]
            .parse::<u32>()
            .unwrap_or(start)
            .clamp(start, MAX_LIMIT);
        (start, Some(end))
    } else {
        let n = num_str.parse::<u32>().unwrap_or(1).clamp(1, MAX_LIMIT);
        (n, None)
    }
}

/// Extract `+mods,conditions` suffix from rest.
/// Returns (rest_without_plus, mods, filters).
/// Parsing failure returns (original_rest, None, None) — caller ignores the + suffix.
/// Format: +MOD1MOD2,key=value,...
/// - mods are pairs of uppercase letters before any '=' token
/// - filters are key=value tokens after the first '=' token
fn extract_plus_suffix(rest: &str) -> (String, Option<Vec<String>>, Option<Vec<String>>) {
    let plus_pos = match rest.rfind('+') {
        Some(p) => p,
        None => return (rest.to_string(), None, None),
    };
    let suffix = &rest[plus_pos + 1..];
    let new_rest = rest[..plus_pos].trim();

    if suffix.is_empty() {
        return (new_rest.to_string(), None, None);
    }

    // Split at first comma
    let (first, rest_str) = match suffix.find(',') {
        Some(p) => (&suffix[..p], &suffix[p + 1..]),
        None => (suffix, ""),
    };

    // If first part contains '=', everything is filters
    let filter_str = if first.contains('=') {
        suffix
    } else {
        rest_str
    };

    let mods = if !first.is_empty() && !first.contains('=') {
        let chars: Vec<char> = first.chars().collect();
        if !chars.len().is_multiple_of(2) {
            return (new_rest.to_string(), None, None);
        }
        let mut mods = Vec::new();
        for chunk in chars.chunks(2) {
            let m: String = chunk.iter().collect();
            if !m.chars().all(|c| c.is_ascii_alphabetic()) {
                return (new_rest.to_string(), None, None);
            }
            mods.push(m.to_uppercase());
        }
        Some(mods)
    } else {
        None
    };

    let filters = if filter_str.is_empty() {
        None
    } else {
        let f: Vec<String> = filter_str.split(',').map(|s| s.to_string()).collect();
        if f.is_empty() || f.iter().any(|s| !s.contains('=')) {
            return (new_rest.to_string(), None, None);
        }
        Some(f)
    };

    (new_rest.to_string(), mods, filters)
}

/// Convert `+MODS` from extract_plus_suffix into "mod=MODS" filter
/// and merge with existing filters.
/// If filters already contain a `mod=` or `mod==` entry, skip adding.
fn merge_mods_into_filters(
    mods: Option<Vec<String>>,
    filters: Option<Vec<String>>,
) -> Option<Vec<String>> {
    let mod_str = match mods {
        Some(m) if !m.is_empty() => Some(m.join("")),
        _ => None,
    };
    let mod_str = match mod_str {
        Some(s) => s,
        None => return filters.filter(|f| !f.is_empty()),
    };
    let mut filters = filters.unwrap_or_default();

    // Don't add mod= if filters already have mod=, mod==, or mod!=
    if !filters
        .iter()
        .any(|s| s.starts_with("mod=") || s.starts_with("mod==") || s.starts_with("mod!="))
    {
        filters.push(format!("mod={}", mod_str));
    }

    if filters.is_empty() {
        None
    } else {
        Some(filters)
    }
}

/// Parse `!s`/`!ss` score-on-beatmap commands.
/// Returns `Some(Some(Command))` on success,
/// `Some(None)` if the prefix matched but parsing failed,
/// `None` if no `!s`/`!ss` prefix matched.
fn parse_score_on_beatmap(msg: &str, mentioned_user_id: Option<i64>) -> Option<Option<Command>> {
    let s_cmds: &[(&str, bool)] = &[("!ss", true), ("!s", false)];
    for &(prefix, is_all) in s_cmds {
        if let Some(rest) = msg.strip_prefix(prefix) {
            if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                continue;
            }
            let rest = rest.trim();

            if rest.is_empty() {
                return Some(Some(Command::ScoreOnBeatmap {
                    mode: GameMode::Osu,
                    username: None,
                    qq: mentioned_user_id,
                    beatmap_id: None,
                    score_id: None,
                    mods: None,
                    filters: None,
                    limit: if is_all { 20 } else { 1 },
                    limit_end: None,
                    is_all,
                }));
            }

            // Step 1: Extract +mods,conditions suffix
            let (rest, raw_mods, mut filters) = extract_plus_suffix(rest);

            // Step 2: Extract :mode suffix
            let (rest, mode) = extract_mode(&rest);

            // Step 3: Extract #N or #N-M suffix from rest or from last filter
            let (rest, limit, limit_end) = if let Some(hash_pos) = rest.rfind('#') {
                let num_str = &rest[hash_pos + 1..];
                let (l, le) = parse_limit(num_str);
                (rest[..hash_pos].trim().to_string(), Some(l), le)
            } else if let Some(last_filter) = filters.as_mut().and_then(|f| f.last_mut()) {
                if let Some(hash_pos) = last_filter.rfind('#') {
                    let num_str = &last_filter[hash_pos + 1..];
                    let (l, le) = parse_limit(num_str);
                    *last_filter = last_filter[..hash_pos].trim().to_string();
                    if last_filter.is_empty() {
                        filters.as_mut().map(|f| f.pop());
                    }
                    (rest.to_string(), Some(l), le)
                } else {
                    (rest.to_string(), None, None)
                }
            } else {
                (rest.to_string(), None, None)
            };

            // Merge +MODS into filters for client-side consistency (after #N extraction)
            filters = merge_mods_into_filters(raw_mods, filters);

            // Step 4: Parse rest → beatmap_id/score_id + username + inline filters
            let rest = rest.trim();
            let (beatmap_id, score_id, username, qq, filters, implicit_limit) = if rest.is_empty() {
                (None, None, None, mentioned_user_id, filters, None)
            } else {
                let tokens: Vec<&str> = rest.split_whitespace().collect();
                let mut bid: Option<u32> = None;
                let mut sid: Option<u64> = None;
                let mut uname_parts: Vec<&str> = Vec::new();
                let mut qq_id: Option<i64> = None;
                let mut found_eq = false;
                let mut passed_numeric = false;
                let mut extra_filters: Vec<String> = Vec::new();
                let mut implicit_limit: Option<u32> = None;
                let mut has_invalid_mention = false;

                for token in &tokens {
                    if token.contains('=') {
                        found_eq = true;
                        for part in token.split(',') {
                            extra_filters.push(part.to_string());
                        }
                        continue;
                    }
                    if let Some(at) = token.strip_prefix('@') {
                        if let Ok(parsed) = at.parse::<i64>() {
                            qq_id = Some(parsed);
                        } else {
                            has_invalid_mention = true;
                        }
                        continue;
                    }
                    if let Ok(num) = token.parse::<u64>() {
                        if !found_eq {
                            if num >= SCORE_ID_THRESHOLD {
                                sid = Some(num);
                            } else if bid.is_none() {
                                bid = Some(num as u32);
                            } else {
                                implicit_limit = Some(num.clamp(1, MAX_LIMIT as u64) as u32);
                            }
                        }
                        passed_numeric = true;
                    } else if !found_eq && passed_numeric {
                        uname_parts.push(token);
                    }
                }

                let uname = if uname_parts.is_empty() {
                    None
                } else {
                    Some(uname_parts.join(" "))
                };

                let all_filters = match filters {
                    Some(mut f) => {
                        f.extend(extra_filters);
                        if f.is_empty() {
                            None
                        } else {
                            Some(f)
                        }
                    }
                    None => {
                        if extra_filters.is_empty() {
                            None
                        } else {
                            Some(extra_filters)
                        }
                    }
                };

                if has_invalid_mention {
                    return Some(None);
                }

                if qq_id.is_some() && uname.is_none() {
                    (bid, sid, None, qq_id, all_filters, implicit_limit)
                } else if qq_id.is_none() {
                    (
                        bid,
                        sid,
                        uname,
                        mentioned_user_id,
                        all_filters,
                        implicit_limit,
                    )
                } else {
                    return Some(None);
                }
            };

            let final_limit = limit
                .or(implicit_limit)
                .unwrap_or(if is_all { 20 } else { 1 });

            return Some(Some(Command::ScoreOnBeatmap {
                mode,
                username,
                qq,
                beatmap_id,
                score_id,
                mods: None,
                filters,
                limit: final_limit,
                limit_end,
                is_all,
            }));
        }
    }
    None
}

/// Parse `!p`/`!r`/`!ps`/`!rs` pass/recent score commands.
/// Returns `Some(Some(Command))` on success,
/// `Some(None)` if the prefix matched but parsing failed,
/// `None` if no pass/recent prefix matched.
fn parse_pass_recent(msg: &str, mentioned_user_id: Option<i64>) -> Option<Option<Command>> {
    for (prefix, is_pass, default_limit) in [
        ("!ps", true, 20u32),
        ("!rs", false, 20u32),
        ("!p", true, 1u32),
        ("!r", false, 1u32),
    ] {
        if let Some(rest) = msg.strip_prefix(prefix) {
            if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                continue;
            }
            let rest = rest.trim();

            let is_summary = default_limit > 1;

            if rest.is_empty() {
                return Some(Some(if is_pass {
                    Command::Pass {
                        mode: GameMode::Osu,
                        username: None,
                        qq: mentioned_user_id,
                        limit: default_limit,
                        limit_end: None,
                        is_summary,
                        filters: None,
                    }
                } else {
                    Command::Recent {
                        mode: GameMode::Osu,
                        username: None,
                        qq: mentioned_user_id,
                        limit: default_limit,
                        limit_end: None,
                        is_summary,
                        filters: None,
                    }
                }));
            }

            // Step 1: Extract +mods,conditions suffix
            let (rest, mods, filters) = extract_plus_suffix(rest);
            let filters = merge_mods_into_filters(mods, filters);

            // Step 2: Extract #N or #N-M suffix (or implicit number)
            let (rest, limit, limit_end) = if let Some(hash_pos) = rest.rfind('#') {
                let num_str = &rest[hash_pos + 1..];
                let (l, le) = parse_limit(num_str);
                (rest[..hash_pos].trim().to_string(), l, le)
            } else {
                let tokens: Vec<&str> = rest.split_whitespace().collect();
                if let Some(last) = tokens.last() {
                    if let Ok(n) = last.parse::<u32>() {
                        let (l, _) = parse_limit(&n.to_string());
                        let without_last = tokens[..tokens.len() - 1].join(" ");
                        (without_last, l, None)
                    } else {
                        (rest.to_string(), default_limit, None)
                    }
                } else {
                    (rest.to_string(), default_limit, None)
                }
            };

            // Step 3: Extract :mode suffix
            let (rest, mode) = extract_mode(&rest);

            // Step 4: Parse rest → username + inline filters
            let rest = rest.trim();
            let (username, qq, filters) = if rest.is_empty() {
                (None, mentioned_user_id, filters)
            } else {
                let tokens: Vec<&str> = rest.split_whitespace().collect();
                let mut name_parts: Vec<&str> = Vec::new();
                let mut qq_id: Option<i64> = None;
                let mut found_eq = false;
                let mut extra_filters: Vec<String> = Vec::new();
                let mut has_invalid_mention = false;

                for token in &tokens {
                    if let Some(at) = token.strip_prefix('@') {
                        if let Ok(parsed) = at.parse::<i64>() {
                            qq_id = Some(parsed);
                        } else {
                            has_invalid_mention = true;
                        }
                        continue;
                    }
                    if token.contains('=') {
                        found_eq = true;
                        for part in token.split(',') {
                            extra_filters.push(part.to_string());
                        }
                    } else if !found_eq {
                        name_parts.push(token);
                    }
                }

                if has_invalid_mention {
                    return Some(None);
                }

                let uname = if name_parts.is_empty() {
                    None
                } else {
                    Some(name_parts.join(" "))
                };

                let all_filters = match filters {
                    Some(mut f) => {
                        f.extend(extra_filters);
                        if f.is_empty() {
                            None
                        } else {
                            Some(f)
                        }
                    }
                    None => {
                        if extra_filters.is_empty() {
                            None
                        } else {
                            Some(extra_filters)
                        }
                    }
                };

                if qq_id.is_some() && uname.is_none() {
                    (None, qq_id, all_filters)
                } else if qq_id.is_none() {
                    (uname, mentioned_user_id, all_filters)
                } else {
                    return Some(None);
                }
            };

            let (final_limit, final_limit_end) = if default_limit == 1 {
                (limit, None)
            } else {
                (limit, limit_end)
            };

            return Some(Some(if is_pass {
                Command::Pass {
                    mode,
                    username,
                    qq,
                    limit: final_limit,
                    limit_end: final_limit_end,
                    is_summary,
                    filters,
                }
            } else {
                Command::Recent {
                    mode,
                    username,
                    qq,
                    limit: final_limit,
                    limit_end: final_limit_end,
                    is_summary,
                    filters,
                }
            }));
        }
    }
    None
}

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

    // Dispatch to sub-parsers for score commands
    if let Some(cmd) = parse_score_on_beatmap(&msg, mentioned_user_id) {
        return cmd;
    }

    if let Some(cmd) = parse_pass_recent(&msg, mentioned_user_id) {
        return cmd;
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
            }
        );
    }

    #[test]
    fn parse_ps_invalid_mode_falls_back_to_osu() {
        let result = parse_command("!ps :xyz", None).unwrap();
        assert_eq!(
            result,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 20,
                limit_end: None,
                is_summary: true,
                filters: None,
            }
        );
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
                limit_end: None,
                filters: None,
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
                is_summary: false,
                limit_end: None,
                filters: None,
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
                is_summary: false,
                limit_end: None,
                filters: None,
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
                limit_end: None,
                filters: None,
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
                filters: None,
                limit_end: None,
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
                mods: None,
                filters: Some(vec!["mod=HDDT".to_string()]),
                limit: 1,
                is_all: false,
                limit_end: None,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_with_mode_and_username() {
        let cmd = parse_command("!s :2 123456 ZnCookie", None).unwrap();
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
                filters: None,
                limit_end: None,
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
                filters: None,
                limit_end: None,
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
                limit: 20,
                is_all: true,
                filters: None,
                limit_end: None,
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
                filters: None,
                limit_end: None,
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
                filters: None,
                limit_end: None,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_username_and_qq_mutually_exclusive() {
        assert!(parse_command("!s 123 ZnCookie @999", None).is_none());
        assert!(parse_command("!s 123 @999 ZnCookie", None).is_none());
    }

    #[test]
    fn test_score_on_beatmap_at_non_numeric_returns_none() {
        assert!(parse_command("!s 123456 @ZnCookie", None).is_none());
    }

    #[test]
    fn test_score_on_beatmap_multi_word_username() {
        let cmd = parse_command("!s 123456 My Name", None).unwrap();
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
                filters: None,
                limit_end: None,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_multi_word_username_with_mode() {
        let cmd = parse_command("!s :2 123456 Zhang San #3 +HD", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Catch,
                username: Some("Zhang San".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: Some(vec!["mod=HD".to_string()]),
                limit: 3,
                is_all: false,
                limit_end: None,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_single_word_username_still_works() {
        let cmd = parse_command("!s 123456 ZnCookie", None).unwrap();
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
                filters: None,
                limit_end: None,
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_username_before_numeric() {
        // In new format, username comes after beatmap_id
        let cmd = parse_command("!s 123456 ZnCookie", None).unwrap();
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
                filters: None,
                limit_end: None,
            }
        );
    }

    // === Unified format tests ===

    #[test]
    fn test_pass_new_format_mode_user() {
        let cmd = parse_command("!p :1 ZnCookie", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Taiko,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_mode_user_hash() {
        let cmd = parse_command("!p :2 ZnCookie #5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Catch,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 5,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_filters() {
        let cmd = parse_command("!ps ZnCookie miss=1,combo=500", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 20,
                limit_end: None,
                is_summary: true,
                filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
            }
        );
    }

    #[test]
    fn test_pass_new_format_hash_without_hash() {
        // !r :3 miss=1,combo=500 5  → 5 当作 #5
        let cmd = parse_command("!r :3 miss=1,combo=500 5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: GameMode::Mania,
                username: None,
                qq: None,
                limit: 5,
                limit_end: None,
                is_summary: false,
                filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
            }
        );
    }

    #[test]
    fn test_pass_new_format_mode_alias_rejected() {
        // String mode aliases are not supported — :std falls back to Osu
        let cmd = parse_command("!p :std ZnCookie", None).unwrap();
        match cmd {
            Command::Pass { mode, .. } => assert_eq!(mode, GameMode::Osu),
            _ => panic!("expected Command::Pass"),
        }
    }

    #[test]
    fn test_pass_new_format_invalid_mode_falls_back() {
        let cmd = parse_command("!p :99", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_range_ps() {
        let cmd = parse_command("!ps #2-10", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 2,
                limit_end: Some(10),
                is_summary: true,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_range_pr_ignored() {
        let cmd = parse_command("!p #2-10", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 2,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_multi_word_username() {
        let cmd = parse_command("!ps Zhang San miss=1", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("Zhang San".to_string()),
                qq: None,
                limit: 20,
                limit_end: None,
                is_summary: true,
                filters: Some(vec!["miss=1".to_string()]),
            }
        );
    }

    #[test]
    fn test_pass_new_format_qq_user() {
        let cmd = parse_command("!ps @123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: Some(123456),
                limit: 20,
                limit_end: None,
                is_summary: true,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_mods_and_filters() {
        let cmd = parse_command("!p +HDHR,miss=1", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: Some(vec!["miss=1".to_string(), "mod=HDHR".to_string()]),
            }
        );
    }

    #[test]
    fn test_pass_new_format_mods_only() {
        let cmd = parse_command("!p +HDHR", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: Some(vec!["mod=HDHR".to_string()]),
            }
        );
    }

    #[test]
    fn test_pass_new_format_bare_command() {
        let cmd = parse_command("!p", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_mode_user_limit() {
        // !p :1 ZnCookie #5
        let cmd = parse_command("!p :1 ZnCookie #5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Taiko,
                username: Some("ZnCookie".to_string()),
                qq: None,
                limit: 5,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    #[test]
    fn test_pass_new_format_mention_fallback() {
        // When no user specified and mentioned_user_id is present
        let cmd = parse_command("!p :1", Some(999999)).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Taiko,
                username: None,
                qq: Some(999999),
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    // === Unified !s/!ss format tests ===

    #[test]
    fn test_s_new_format_basic() {
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
                filters: None,
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_mode_beatmap_user() {
        let cmd = parse_command("!s :2 123456 ZnCookie", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Catch,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: None,
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_mods_filters() {
        let cmd = parse_command("!s :1 123456 ZnCookie +HDHR,miss=1 #5", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Taiko,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: Some(vec!["miss=1".to_string(), "mod=HDHR".to_string()]),
                limit: 5,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_score_id() {
        // >= 10_000_000 is score_id
        let cmd = parse_command("!s :3 1234567890", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Mania,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: Some(1234567890),
                mods: None,
                filters: None,
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_ss_new_format_range() {
        let cmd = parse_command("!ss 123456 #2-10", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: None,
                limit: 2,
                limit_end: Some(10),
                is_all: true,
            }
        );
    }

    #[test]
    fn test_s_new_format_multi_word_username() {
        let cmd = parse_command("!s :2 123456 Zhang San +DT,miss=1", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Catch,
                username: Some("Zhang San".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: Some(vec!["miss=1".to_string(), "mod=DT".to_string()]),
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_qq_user() {
        let cmd = parse_command("!s :2 123456 @123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Catch,
                username: None,
                qq: Some(123456),
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: None,
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_inline_filters_no_plus() {
        let cmd = parse_command("!s 123456 ZnCookie miss=1,combo=500", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_bare_command() {
        let cmd = parse_command("!s", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
                mods: None,
                filters: None,
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_mode_alias_rejected() {
        // String mode aliases are not supported — :mania falls back to Osu
        let cmd = parse_command("!s :mania 123456", None).unwrap();
        match cmd {
            Command::ScoreOnBeatmap { mode, .. } => assert_eq!(mode, GameMode::Osu),
            _ => panic!("expected Command::ScoreOnBeatmap"),
        }
    }

    #[test]
    fn test_s_new_format_invalid_mode_falls_back() {
        let cmd = parse_command("!s :99 123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: None,
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_implicit_hash() {
        // !s 123456 5 → beatmap_id=123456, limit=5
        let cmd = parse_command("!s 123456 5", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: None,
                limit: 5,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_implicit_limit_clamped() {
        // !s 123456 999999 → limit should be clamped to MAX_LIMIT (100)
        let cmd = parse_command("!s 123456 999999", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                filters: None,
                limit: 100,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_ss_new_format_bare_command() {
        let cmd = parse_command("!ss", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
                mods: None,
                filters: None,
                limit: 20,
                limit_end: None,
                is_all: true,
            }
        );
    }
}
