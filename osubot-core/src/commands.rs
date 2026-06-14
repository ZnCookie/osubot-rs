use crate::types::{Command, Condition, ConditionOp, GameMode, ScoreRange};

/// 解析用户消息为命令
/// 支持格式:
/// - `!s (:mode) (bid|score_id) (user) (+mod) (#num)` — 谱面成绩
/// - `!ss (:mode) (bid|score_id) (user) (+mod) (#num)` — 谱面全部成绩
/// - `!p (:mode) (user) (filter) (#num)` — BP
/// - `!ps (:mode) (user) (filter) (#num)` — 多条BP
/// - `!r (:mode) (user) (filter) (#num)` — 最近成绩
/// - `!rs (:mode) (user) (filter) (#num)` — 多条最近成绩
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

// 统一 token 扫描器：解析 !s/!ss/!p/!ps/!r/!rs 后的参数
#[derive(Debug, PartialEq)]
struct ParsedTokens {
    mode: GameMode,
    username: Option<String>,
    qq: Option<i64>,
    bid: Option<u32>,
    sid: Option<u64>,
    mods: Option<Vec<String>>,
    range: ScoreRange,
    filters: Vec<Condition>,
}

fn parse_scores_args(
    rest: &str,
    default_is_summary: bool,
    default_is_all: bool,
    allow_bid_sid: bool,
    allow_filters: bool,
    mentioned_user_id: Option<i64>,
) -> Option<ParsedTokens> {
    if rest.is_empty() {
        return Some(ParsedTokens {
            mode: GameMode::Osu,
            username: None,
            qq: mentioned_user_id,
            bid: None,
            sid: None,
            mods: None,
            range: if default_is_all {
                ScoreRange::all()
            } else {
                ScoreRange::default_count(default_is_summary)
            },
            filters: vec![],
        });
    }

    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut mode = GameMode::Osu;
    let mut username_parts: Vec<&str> = vec![];
    let mut qq: Option<i64> = None;
    let mut bid: Option<u32> = None;
    let mut sid: Option<u64> = None;
    let mut mods: Option<Vec<String>> = None;
    let mut range: Option<ScoreRange> = None;
    let mut filters: Vec<Condition> = vec![];
    let mut has_bid_sid = false;

    let mut i = 0;
    while i < tokens.len() {
        let token = tokens[i];
        // :mode
        if let Some(mode_str) = token.strip_prefix(':') {
            let m = if mode_str.is_empty() {
                GameMode::Osu
            } else {
                GameMode::from_mode_str(mode_str)?
            };
            mode = m;
            i += 1;
            continue;
        }
        // +mods
        if let Some(mod_str) = token.strip_prefix('+') {
            if mod_str.len() >= 2 && mod_str.chars().all(|c| c.is_ascii_alphabetic()) {
                let parsed: Vec<String> = mod_str
                    .chars()
                    .collect::<Vec<_>>()
                    .chunks(2)
                    .map(|chunk| chunk.iter().collect::<String>().to_uppercase())
                    .collect();
                mods = Some(parsed);
            }
            i += 1;
            continue;
        }
        // #range
        if let Some(range_str) = token.strip_prefix('#') {
            range = Some(parse_range(range_str, default_is_summary));
            i += 1;
            continue;
        }
        // @QQ
        if let Some(at) = token.strip_prefix('@') {
            if let Ok(parsed) = at.parse::<i64>() {
                qq = Some(parsed);
            } else {
                return None;
            }
            i += 1;
            continue;
        }
        // filter (only for !p/!r commands)
        if allow_filters {
            if let Some(cond) = parse_condition(token) {
                filters.push(cond);
                i += 1;
                continue;
            }
            // Try 3-token condition: field op value (e.g., "pp > 300", "star >= 7")
            // Guard: middle token must be a standalone operator and the field
            // must not be pure numeric (to avoid "123 > 456" creating a bogus filter).
            if i + 2 < tokens.len()
                && is_op_token(tokens[i + 1])
                && tokens[i].parse::<f64>().is_err()
            {
                let combined = format!("{}{}{}", tokens[i], tokens[i + 1], tokens[i + 2]);
                if let Some(cond) = parse_condition(&combined) {
                    filters.push(cond);
                    i += 3;
                    continue;
                }
            }
            // Try 2-token condition: operator split across boundary
            // (e.g., "star= 7" or "star =7")
            // Field name must not be pure numeric to avoid "123 >456" as a filter.
            if i + 1 < tokens.len() && tokens[i].parse::<f64>().is_err() {
                let left = token;
                let right = tokens[i + 1];
                let ends_with_op = left.ends_with(['=', '>', '<', '!']);
                let next_starts_with_op = right.starts_with(['=', '>', '<', '!']);
                if ends_with_op || next_starts_with_op {
                    let combined = format!("{}{}", left, right);
                    if let Some(cond) = parse_condition(&combined) {
                        filters.push(cond);
                        i += 2;
                        continue;
                    }
                }
            }
        }
        // numeric → bid/sid (only for !s/!ss)
        if allow_bid_sid {
            if let Ok(num) = token.parse::<u64>() {
                if !has_bid_sid {
                    if num >= 10_000_000 {
                        sid = Some(num);
                    } else if let Ok(b) = u32::try_from(num) {
                        bid = Some(b);
                    }
                    has_bid_sid = true;
                }
                i += 1;
                continue;
            }
        }
        // otherwise → username part
        username_parts.push(token);
        i += 1;
    }

    let username = if username_parts.is_empty() {
        None
    } else {
        Some(username_parts.join(" "))
    };

    // 互斥：不能同时提供用户名和 @QQ
    if username.is_some() && qq.is_some() {
        return None;
    }

    // fallback to mentioned_user_id
    let (final_username, final_qq) = if username.is_none() && qq.is_none() {
        (None, mentioned_user_id)
    } else {
        (username, qq)
    };

    let final_range = range.unwrap_or_else(|| {
        if default_is_all {
            ScoreRange::all()
        } else {
            ScoreRange::default_count(default_is_summary)
        }
    });

    Some(ParsedTokens {
        mode,
        username: final_username,
        qq: final_qq,
        bid,
        sid,
        mods,
        range: final_range,
        filters,
    })
}

fn parse_range(s: &str, summary: bool) -> ScoreRange {
    if let Some((start, end)) = s.split_once('-') {
        let start = start.trim();
        let end = end.trim();
        let s: usize = start.parse().unwrap_or(1);
        let e: usize = end.parse().unwrap_or(s);
        if s > 0 && e >= s {
            ScoreRange {
                offset: s - 1,
                count: e - s + 1,
            }
        } else {
            ScoreRange {
                offset: 0,
                count: 1,
            }
        }
    } else if s.is_empty() {
        if summary {
            ScoreRange {
                offset: 0,
                count: 0,
            }
        } else {
            ScoreRange {
                offset: 0,
                count: 1,
            }
        }
    } else if let Ok(n) = s.parse::<usize>() {
        if summary && n > 0 {
            ScoreRange {
                offset: 0,
                count: n,
            }
        } else if summary {
            ScoreRange {
                offset: 0,
                count: 0,
            }
        } else {
            ScoreRange::single(n)
        }
    } else {
        ScoreRange {
            offset: 0,
            count: 1,
        }
    }
}

/// 判断 token 是否为单独的运算符（用于多 token 条件检测）
fn is_op_token(token: &str) -> bool {
    matches!(
        token,
        ">" | "<"
            | "="
            | "=="
            | "!="
            | ">="
            | "<="
            | "<>"
            | "≈"
            | "≌"
            | "≠"
            | "≥"
            | "≤"
            | "＞"
            | "＜"
            | "＝"
            | "＞＝"
            | "＜＝"
            | "！="
    )
}

fn parse_condition(token: &str) -> Option<Condition> {
    let ops = [
        "==", "≌", "!=", "<>", "≠", ">=", "≤", "<=", "≥", ">", "<", "=", "≈",
        // Fullwidth variants
        "！=", "＞＝", "＜＝", "＞", "＜", "＝",
    ];
    for op in &ops {
        if let Some((field, value)) = token.split_once(op) {
            let operator = match *op {
                "=" | "≈" | "＝" => ConditionOp::Eq,
                "==" | "≌" => ConditionOp::XEq,
                "!=" | "<>" | "≠" | "！=" => ConditionOp::Ne,
                ">" | "＞" => ConditionOp::Gt,
                ">=" | "≥" | "＞＝" => ConditionOp::Ge,
                "<" | "＜" => ConditionOp::Lt,
                "<=" | "≤" | "＜＝" => ConditionOp::Le,
                _ => continue,
            };
            if !field.is_empty() && !value.is_empty() {
                return Some(Condition {
                    field: field.to_string(),
                    operator,
                    value: value.to_string(),
                });
            }
        }
    }
    None
}

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

    if let Some(rest) = msg.strip_prefix("!ss") {
        if !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            let rest = rest.trim();
            let parsed = parse_scores_args(rest, true, true, true, false, mentioned_user_id)?;
            return Some(Command::ScoreOnBeatmap {
                mode: parsed.mode,
                username: parsed.username,
                qq: parsed.qq,
                beatmap_id: parsed.bid,
                score_id: parsed.sid,
                mods: parsed.mods,
                range: parsed.range,
            });
        }
    }

    if let Some(rest) = msg.strip_prefix("!s") {
        if !rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            let rest = rest.trim();
            let parsed = parse_scores_args(rest, false, false, true, false, mentioned_user_id)?;
            return Some(Command::ScoreOnBeatmap {
                mode: parsed.mode,
                username: parsed.username,
                qq: parsed.qq,
                beatmap_id: parsed.bid,
                score_id: parsed.sid,
                mods: parsed.mods,
                range: parsed.range,
            });
        }
    }

    for (prefix, is_pass, is_summary) in [
        ("!ps", true, true),
        ("!rs", false, true),
        ("!p", true, false),
        ("!r", false, false),
    ] {
        if let Some(rest) = msg.strip_prefix(prefix) {
            if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                continue;
            }
            let rest = rest.trim();
            let parsed =
                parse_scores_args(rest, is_summary, false, false, true, mentioned_user_id)?;
            let cmd = if is_pass {
                Command::Pass {
                    mode: parsed.mode,
                    username: parsed.username,
                    qq: parsed.qq,
                    range: parsed.range,
                    is_summary,
                    filters: parsed.filters,
                }
            } else {
                Command::Recent {
                    mode: parsed.mode,
                    username: parsed.username,
                    qq: parsed.qq,
                    range: parsed.range,
                    is_summary,
                    filters: parsed.filters,
                }
            };
            return Some(cmd);
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 20
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 1,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 5
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 3
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 5
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 200
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 2,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 5
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 20
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
            }
            .group_name(),
            CommandGroup::Score
        );
        assert_eq!(
            Command::Recent {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 20
                },
                is_summary: true,
                filters: vec![],
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 0,
                    count: 0
                },
            }
        );
    }

    #[test]
    fn test_score_on_beatmap_all_explicit_zero() {
        let cmd = parse_command("!ss 123456 #", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                range: ScoreRange {
                    offset: 0,
                    count: 0
                },
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
                range: ScoreRange {
                    offset: 4,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 2,
                    count: 1
                },
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
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
            }
        );
    }

    #[test]
    fn test_pass_with_filter() {
        let cmd = parse_command("!p ZnCookie star>7", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![Condition {
                    field: "star".into(),
                    operator: ConditionOp::Gt,
                    value: "7".into(),
                }],
            }
        );
    }

    #[test]
    fn test_pass_with_filter_spaces() {
        let cmd = parse_command("!p ZnCookie star > 7", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![Condition {
                    field: "star".into(),
                    operator: ConditionOp::Gt,
                    value: "7".into(),
                }],
            }
        );
    }

    #[test]
    fn test_pass_with_filter_fullwidth_op() {
        let cmd = parse_command("!p ZnCookie star＞7", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![Condition {
                    field: "star".into(),
                    operator: ConditionOp::Gt,
                    value: "7".into(),
                }],
            }
        );
    }

    #[test]
    fn test_pass_with_filter_fullwidth_ge() {
        let cmd = parse_command("!p ZnCookie star＞＝7", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![Condition {
                    field: "star".into(),
                    operator: ConditionOp::Ge,
                    value: "7".into(),
                }],
            }
        );
    }

    #[test]
    fn test_recent_with_filter_multi_token() {
        let cmd = parse_command("!r ZnCookie pp >= 300", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: GameMode::Osu,
                username: Some("ZnCookie".to_string()),
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                is_summary: false,
                filters: vec![Condition {
                    field: "pp".into(),
                    operator: ConditionOp::Ge,
                    value: "300".into(),
                }],
            }
        );
    }

    #[test]
    fn test_summary_hash_zero_is_all() {
        let cmd = parse_command("!ps #0", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 0
                },
                is_summary: true,
                filters: vec![],
            }
        );
    }

    #[test]
    fn test_pass_with_range() {
        let cmd = parse_command("!ps #1-5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 5
                },
                is_summary: true,
                filters: vec![],
            }
        );
    }

    #[test]
    fn test_pass_single_number() {
        let cmd = parse_command("!p #3", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                range: ScoreRange {
                    offset: 2,
                    count: 1
                },
                is_summary: false,
                filters: vec![],
            }
        );
    }

    #[test]
    fn test_s_with_range() {
        let cmd = parse_command("!s 123456 #1-3", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: GameMode::Osu,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
                mods: None,
                range: ScoreRange {
                    offset: 0,
                    count: 3
                },
            }
        );
    }

    #[test]
    fn test_recent_with_filter_and_range() {
        let cmd = parse_command("!rs :1 ZnCookie pp>300 #1-5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: GameMode::Taiko,
                username: Some("ZnCookie".to_string()),
                qq: None,
                range: ScoreRange {
                    offset: 0,
                    count: 5
                },
                is_summary: true,
                filters: vec![Condition {
                    field: "pp".into(),
                    operator: ConditionOp::Gt,
                    value: "300".into(),
                }],
            }
        );
    }

    #[test]
    fn test_any_token_order() {
        let cmd1 = parse_command("!p ZnCookie :1 #3", None).unwrap();
        let cmd2 = parse_command("!p #3 :1 ZnCookie", None).unwrap();
        assert_eq!(cmd1, cmd2);
    }

    #[test]
    fn test_recent_no_arg_defaults() {
        let cmd = parse_command("!r", None).unwrap();
        assert!(matches!(
            cmd,
            Command::Recent {
                range: ScoreRange {
                    offset: 0,
                    count: 1
                },
                ..
            }
        ));
    }

    #[test]
    fn test_rs_default_range() {
        let cmd = parse_command("!rs", None).unwrap();
        assert!(matches!(
            cmd,
            Command::Recent {
                range: ScoreRange {
                    offset: 0,
                    count: 20
                },
                ..
            }
        ));
    }
}
