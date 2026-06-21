use crate::types::{Command, GameMode};

const MAX_LIMIT: u32 = 100;
const SCORE_ID_THRESHOLD: u64 = 10_000_000;

/// Extract `:mode` suffix from rest. Returns (rest_without_mode, mode).
/// Finds the rightmost colon and extracts only the first token after it as mode.
/// The rest of the content (before and after the mode token) is preserved.
/// Invalid mode strings return None.
fn extract_mode(rest: &str) -> (String, Option<GameMode>) {
    if let Some(colon_pos) = rest.rfind(':') {
        let after_colon = &rest[colon_pos + 1..];
        let mode_token = after_colon.split_whitespace().next().unwrap_or("");
        let mode = GameMode::from_mode_str(mode_token);

        let before_colon = &rest[..colon_pos];
        let after_mode_token = &rest[colon_pos + 1 + mode_token.len()..];
        let new_rest = format!("{} {}", before_colon.trim(), after_mode_token.trim())
            .trim()
            .to_string();

        (new_rest, mode)
    } else {
        (rest.to_string(), None)
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
///
/// Only tokens starting with `+` are treated as plus-suffix tokens.
fn extract_plus_suffix(rest: &str) -> (String, Option<Vec<String>>, Option<Vec<String>>) {
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let plus_idx = match tokens.iter().rposition(|t| t.starts_with('+')) {
        Some(i) => i,
        None => return (rest.to_string(), None, None),
    };
    let token = tokens[plus_idx];
    let suffix = &token[1..];
    let new_rest: Vec<&str> = tokens
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != plus_idx)
        .map(|(_, t)| *t)
        .collect();
    let new_rest = new_rest.join(" ");

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

/// Common argument extraction results, fixed order: [:mode] → [+MODS] → [#N]
struct CommonArgs {
    mode: Option<GameMode>,
    raw_mods: Option<Vec<String>>,
    filters: Option<Vec<String>>,
    limit: u32,
    limit_end: Option<u32>,
    rest: String,
}

/// Extract :mode, +mods, #N from a string in fixed order.
///
/// Extraction order (matching documented argument order):
/// 1. extract_mode — `rfind(':')` to locate
/// 2. extract_plus_suffix — `rposition('+')` to locate
/// 3. #N extraction — `rfind('#')` from remaining string or filter_suffix
///
/// `default_limit` is used when no #N is found.
/// Parse `mm:ss:mmm` format to milliseconds.
/// - minutes: 1-2 digits
/// - seconds: exactly 2 digits (00-59)
/// - milliseconds: exactly 3 digits (000-999)
fn parse_time_token(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }
    if parts[0].is_empty() || parts[0].len() > 2 {
        return None;
    }
    if parts[1].len() != 2 {
        return None;
    }
    if parts[2].len() != 3 {
        return None;
    }

    let minutes: i64 = parts[0].parse().ok()?;
    let seconds: i64 = parts[1].parse().ok()?;
    let millis: i64 = parts[2].parse().ok()?;

    if seconds > 59 || millis > 999 {
        return None;
    }

    Some(minutes * 60_000 + seconds * 1_000 + millis)
}

fn extract_common_args(s: &str, default_limit: u32) -> CommonArgs {
    let (s, mode) = extract_mode(s);
    let (s, raw_mods, mut filter_suffix) = extract_plus_suffix(&s);

    // #N extraction: check rest first, then filter_suffix (for !s/!ss edge case)
    let (rest, limit, limit_end) = if let Some(hash_pos) = s.rfind('#') {
        let num_str = &s[hash_pos + 1..];
        let (l, le) = parse_limit(num_str);
        (s[..hash_pos].trim().to_string(), l, le)
    } else if let Some(last) = filter_suffix.as_mut().and_then(|f| f.last_mut()) {
        if let Some(hash_pos) = last.rfind('#') {
            let num_str = &last[hash_pos + 1..];
            let (l, le) = parse_limit(num_str);
            *last = last[..hash_pos].trim().to_string();
            if last.is_empty() {
                filter_suffix.as_mut().map(|f| f.pop());
            }
            (s.trim().to_string(), l, le)
        } else {
            (s.trim().to_string(), default_limit, None)
        }
    } else {
        (s.trim().to_string(), default_limit, None)
    };

    let filters = merge_mods_into_filters(raw_mods.clone(), filter_suffix);

    CommonArgs {
        mode,
        raw_mods,
        filters,
        limit,
        limit_end,
        rest,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScoringCmd {
    PassSummary,
    RecentSummary,
    ScoreAll,
    PassSingle,
    RecentSingle,
    ScoreSingle,
    BeatmapPreview,
}

/// Unified parser for !p/!r/!ps/!rs/!s/!ss/!rv commands.
///
/// Returns:
/// - `Some(Some(Command))` — successfully parsed
/// - `Some(None)` — prefix matched but arguments invalid
/// - `None` — no matching prefix
fn parse_scoring_command(msg: &str, mentioned_user_id: Option<i64>) -> Option<Option<Command>> {
    // Prefix matching: longest first to avoid !ps being eaten by !p
    const PREFIXES: &[(&str, ScoringCmd)] = &[
        ("!ps", ScoringCmd::PassSummary),
        ("!rs", ScoringCmd::RecentSummary),
        ("!ss", ScoringCmd::ScoreAll),
        ("!p", ScoringCmd::PassSingle),
        ("!r", ScoringCmd::RecentSingle),
        ("!s", ScoringCmd::ScoreSingle),
        ("!rv", ScoringCmd::BeatmapPreview),
    ];

    for &(prefix, cmd) in PREFIXES {
        let rest = match msg.strip_prefix(prefix) {
            Some(r) => r,
            None => continue,
        };
        // Reject !sabc, !rva etc.
        if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            continue;
        }
        let rest = rest.trim();

        return Some(parse_one_scoring_cmd(rest, cmd, mentioned_user_id));
    }
    None
}

fn parse_one_scoring_cmd(
    rest: &str,
    cmd: ScoringCmd,
    mentioned_user_id: Option<i64>,
) -> Option<Command> {
    match cmd {
        ScoringCmd::BeatmapPreview => parse_rv(rest),
        _ => parse_standard_score(rest, cmd, mentioned_user_id),
    }
}

fn parse_rv(rest: &str) -> Option<Command> {
    // Pass 1: extract --gif/-g and mm:ss:mmm time tokens
    let mut gif = false;
    let mut times: Vec<i64> = Vec::new();
    let mut pass1_tokens: Vec<&str> = Vec::new();

    for token in rest.split_whitespace() {
        if token == "--gif" || token == "-g" {
            gif = true;
        } else if let Some(ms) = parse_time_token(token) {
            times.push(ms);
            if times.len() > 2 {
                return None; // at most 2 time params
            }
        } else {
            pass1_tokens.push(token);
        }
    }

    let remaining = pass1_tokens.join(" ");

    if remaining.is_empty() && times.is_empty() {
        return Some(Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif,
            times: None,
        });
    }

    let args = extract_common_args(&remaining, 1);

    if args.rest.is_empty() && !times.is_empty() {
        return Some(Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: args.mode,
            mods: args.raw_mods,
            gif,
            times: Some(times),
        });
    }

    if args.rest.is_empty() {
        return Some(Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: args.mode,
            mods: args.raw_mods,
            gif,
            times: None,
        });
    }

    // Parse remaining tokens for score_id / beatmap_id
    let mut score_id: Option<u64> = None;
    let mut beatmap_id: Option<u32> = None;

    for token in args.rest.split_whitespace() {
        if let Ok(num) = token.parse::<u64>() {
            if num >= SCORE_ID_THRESHOLD {
                if score_id.is_some() || beatmap_id.is_some() {
                    return None;
                }
                score_id = Some(num);
            } else {
                if score_id.is_some() || beatmap_id.is_some() {
                    return None;
                }
                beatmap_id = Some(num as u32);
            }
        } else {
            return None;
        }
    }

    Some(Command::BeatmapPreview {
        score_id,
        beatmap_id,
        mode: args.mode,
        mods: args.raw_mods,
        gif,
        times: if times.is_empty() { None } else { Some(times) },
    })
}

fn parse_standard_score(
    rest: &str,
    cmd: ScoringCmd,
    mentioned_user_id: Option<i64>,
) -> Option<Command> {
    let default_limit = match cmd {
        ScoringCmd::PassSummary | ScoringCmd::RecentSummary | ScoringCmd::ScoreAll => 20u32,
        ScoringCmd::PassSingle | ScoringCmd::RecentSingle | ScoringCmd::ScoreSingle => 1u32,
        ScoringCmd::BeatmapPreview => unreachable!(),
    };

    if rest.is_empty() {
        return make_score_cmd(ScoreCmdParams {
            cmd,
            mentioned_user_id,
            mode: None,
            beatmap_id: None,
            score_id: None,
            username: None,
            qq: None,
            limit: default_limit,
            limit_end: None,
            filters: None,
        });
    }

    let args = extract_common_args(rest, default_limit);

    // Parse remaining tokens (bid/sid/username/@QQ/filters)
    let rt = parse_remaining_tokens(&args.rest, mentioned_user_id)?;

    // Merge inline_filters with args.filters
    let filters = match (args.filters, rt.filters) {
        (Some(mut f), Some(extra)) => {
            f.extend(extra);
            Some(f)
        }
        (Some(f), None) => Some(f),
        (None, Some(f)) => Some(f),
        (None, None) => None,
    };

    let final_limit = rt.implicit_limit.unwrap_or(args.limit);

    // Single commands (!p/!r/!s) suppress limit_end — range only for summary commands
    let final_limit_end = if matches!(
        cmd,
        ScoringCmd::PassSingle | ScoringCmd::RecentSingle | ScoringCmd::ScoreSingle
    ) {
        None
    } else {
        args.limit_end
    };

    make_score_cmd(ScoreCmdParams {
        cmd,
        mentioned_user_id,
        mode: args.mode,
        beatmap_id: rt.beatmap_id,
        score_id: rt.score_id,
        username: rt.username,
        qq: rt.qq,
        limit: final_limit,
        limit_end: final_limit_end,
        filters,
    })
}

struct ScoreCmdParams {
    cmd: ScoringCmd,
    mentioned_user_id: Option<i64>,
    mode: Option<GameMode>,
    beatmap_id: Option<u32>,
    score_id: Option<u64>,
    username: Option<String>,
    qq: Option<i64>,
    limit: u32,
    limit_end: Option<u32>,
    filters: Option<Vec<String>>,
}

fn make_score_cmd(params: ScoreCmdParams) -> Option<Command> {
    let qq = params.qq.or(params.mentioned_user_id);
    Some(match params.cmd {
        ScoringCmd::PassSummary => Command::Pass {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            limit: params.limit,
            limit_end: params.limit_end,
            is_summary: true,
            filters: params.filters,
        },
        ScoringCmd::RecentSummary => Command::Recent {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            limit: params.limit,
            limit_end: params.limit_end,
            is_summary: true,
            filters: params.filters,
        },
        ScoringCmd::ScoreAll => Command::ScoreOnBeatmap {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            filters: params.filters,
            limit: params.limit,
            limit_end: params.limit_end,
            is_all: true,
        },
        ScoringCmd::PassSingle => Command::Pass {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            limit: params.limit,
            limit_end: params.limit_end,
            is_summary: false,
            filters: params.filters,
        },
        ScoringCmd::RecentSingle => Command::Recent {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            limit: params.limit,
            limit_end: params.limit_end,
            is_summary: false,
            filters: params.filters,
        },
        ScoringCmd::ScoreSingle => Command::ScoreOnBeatmap {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            filters: params.filters,
            limit: params.limit,
            limit_end: params.limit_end,
            is_all: false,
        },
        ScoringCmd::BeatmapPreview => unreachable!(),
    })
}

/// Parsed result from remaining tokens after common args extraction.
struct RemainingTokens {
    beatmap_id: Option<u32>,
    score_id: Option<u64>,
    username: Option<String>,
    qq: Option<i64>,
    filters: Option<Vec<String>>,
    implicit_limit: Option<u32>,
}

/// Parse remaining tokens into beatmap_id/score_id/username/@QQ/filters.
/// Returns `None` on invalid input (invalid mention, username+QQ conflict).
/// Shared by all scoring commands.
fn parse_remaining_tokens(rest: &str, mentioned_user_id: Option<i64>) -> Option<RemainingTokens> {
    if rest.is_empty() {
        return Some(RemainingTokens {
            beatmap_id: None,
            score_id: None,
            username: None,
            qq: mentioned_user_id,
            filters: None,
            implicit_limit: None,
        });
    }

    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let mut beatmap_id: Option<u32> = None;
    let mut score_id: Option<u64> = None;
    let mut name_parts: Vec<&str> = Vec::new();
    let mut qq_id: Option<i64> = None;
    let mut found_eq = false;
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
        if !found_eq {
            if let Ok(num) = token.parse::<u64>() {
                if num >= SCORE_ID_THRESHOLD {
                    score_id = Some(num);
                } else if beatmap_id.is_none() {
                    beatmap_id = Some(num as u32);
                } else {
                    let clamped = num.clamp(1, MAX_LIMIT as u64) as u32;
                    implicit_limit = Some(clamped);
                }
            } else {
                name_parts.push(token);
            }
        }
    }

    if has_invalid_mention {
        return None;
    }

    let username = if name_parts.is_empty() {
        None
    } else {
        Some(name_parts.join(" "))
    };
    let filters = if extra_filters.is_empty() {
        None
    } else {
        Some(extra_filters)
    };
    let qq = qq_id.or(mentioned_user_id);

    // username and QQ are mutually exclusive
    if qq_id.is_some() && username.is_some() {
        return None;
    }

    Some(RemainingTokens {
        beatmap_id,
        score_id,
        username,
        qq,
        filters,
        implicit_limit,
    })
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
            return Some(Command::QuerySelf { mode: None });
        }
        let mode = GameMode::from_mode_str(rest)?;
        return Some(Command::QuerySelf { mode: Some(mode) });
    }

    // 查询他人 via QQ: where qq=<QQ号> [, 模式]
    if let Some(rest) = msg.strip_prefix("where qq=") {
        let parts: Vec<&str> = rest.split(',').collect();
        let qq: i64 = parts[0].trim().parse().ok()?;
        let mode = if parts.len() > 1 {
            Some(GameMode::from_mode_str(parts[1].trim())?)
        } else {
            None
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
                    Some(GameMode::from_mode_str(parts[1].trim())?)
                } else {
                    None
                };
                return Some(Command::QueryMentionedUser { qq, mode });
            }
            return None;
        }
        let username = first.to_string();
        let mode = if parts.len() > 1 {
            Some(GameMode::from_mode_str(parts[1].trim())?)
        } else {
            None
        };
        return Some(Command::QueryUser { username, mode });
    }

    // 查询他人 via QQ mention: 查@<QQ用户> [, 模式]
    if msg.starts_with('查') {
        if let Some(qq) = mentioned_user_id {
            let rest = msg.strip_prefix('查').unwrap();
            let rest = rest.trim_start_matches(',').trim().trim_start_matches(',');
            let mode = if rest.is_empty() {
                None
            } else {
                Some(GameMode::from_mode_str(rest)?)
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
                None
            } else {
                GameMode::from_mode_str(mode_str)
            }
        } else {
            GameMode::from_mode_str(rest)
        };
        return Some(Command::Highlight { mode });
    }

    // 帮助: !help
    if msg == "!help" {
        return Some(Command::Help);
    }

    // 查询/设置默认模式: !mode 或 !mode <N>
    // 无效的 mode 值等价于查询（mode=None），与 !p :99 等命令的行为一致
    if let Some(rest) = msg.strip_prefix("!mode") {
        if rest.is_empty() || rest.chars().next().is_some_and(|c| c.is_whitespace()) {
            let rest = rest.trim();
            let mode = if rest.is_empty() {
                None
            } else {
                GameMode::from_mode_str(rest)
            };
            return Some(Command::SetDefaultMode { mode });
        }
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

    // Unified scoring command parser: !p/!r/!ps/!rs/!s/!ss/!rv
    if let Some(cmd) = parse_scoring_command(&msg, mentioned_user_id) {
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
    fn test_help_command() {
        let cmd = parse_command("!help", None).unwrap();
        assert_eq!(cmd, Command::Help);
    }

    #[test]
    fn test_pass_self() {
        let cmd = parse_command("!p", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: Some(GameMode::Taiko),
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: Some(123456),
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: Some(123456),
                beatmap_id: None,
                score_id: None,
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
                mode: Some(GameMode::Mania),
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
                limit: 5,
                is_summary: true,
                limit_end: None,
                filters: None,
            }
        );
    }

    #[test]
    fn test_ps_invalid_mode_returns_none() {
        let result = parse_command("!ps :xyz", None).unwrap();
        assert_eq!(
            result,
            Command::Pass {
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
    fn parse_ps_empty_mode_returns_none() {
        let result = parse_command("!ps :", None);
        assert_eq!(
            result,
            Some(Command::Pass {
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
            Command::QuerySelf { mode: None }.group_name(),
            CommandGroup::Query
        );
        assert_eq!(
            Command::QueryUser {
                username: "x".into(),
                mode: None
            }
            .group_name(),
            CommandGroup::Query
        );
        assert_eq!(
            Command::QueryMentionedUser { qq: 1, mode: None }.group_name(),
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
            Command::Highlight { mode: None }.group_name(),
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
                limit: 1,
                is_summary: false,
                limit_end: None,
                filters: None,
            }
            .group_name(),
            CommandGroup::Score
        );
        assert_eq!(
            Command::SetDefaultMode { mode: None }.group_name(),
            CommandGroup::Mode
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
                mode: None,
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
                mode: Some(GameMode::Taiko),
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
                mode: None,
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
                mode: Some(GameMode::Taiko),
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: Some(12345678901),
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: Some(123456),
                beatmap_id: Some(789012),
                score_id: None,
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
                mode: None,
                username: Some("My Name".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: Some("Zhang San".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Taiko),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
                limit: 20,
                limit_end: None,
                is_summary: true,
                filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
            }
        );
    }

    #[test]
    fn test_pass_new_format_hash_without_hash() {
        // !r :3 miss=1,combo=500 5  → 5 在过滤器之后被忽略（应使用 #5）
        let cmd = parse_command("!r :3 miss=1,combo=500 5", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: Some(GameMode::Mania),
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
            }
        );
    }

    #[test]
    fn test_pass_new_format_mode_alias_rejected() {
        // String mode aliases are not supported — :std returns None
        let cmd = parse_command("!p :std ZnCookie", None).unwrap();
        match cmd {
            Command::Pass { mode, .. } => assert_eq!(mode, None),
            _ => panic!("expected Command::Pass"),
        }
    }

    #[test]
    fn test_pass_new_format_invalid_mode_returns_none() {
        let cmd = parse_command("!p :99", None).unwrap();
        assert_eq!(
            cmd,
            Command::Pass {
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: Some("Zhang San".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: Some(123456),
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: Some(GameMode::Taiko),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: None,
                score_id: None,
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
                mode: Some(GameMode::Taiko),
                username: None,
                qq: Some(999999),
                beatmap_id: None,
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Taiko),
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Mania),
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: Some(1234567890),
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: Some("Zhang San".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: Some(GameMode::Catch),
                username: None,
                qq: Some(123456),
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: Some("ZnCookie".to_string()),
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
                filters: None,
                limit: 1,
                limit_end: None,
                is_all: false,
            }
        );
    }

    #[test]
    fn test_s_new_format_mode_alias_rejected() {
        // String mode aliases are not supported — :mania returns None
        let cmd = parse_command("!s :mania 123456", None).unwrap();
        match cmd {
            Command::ScoreOnBeatmap { mode, .. } => assert_eq!(mode, None),
            _ => panic!("expected Command::ScoreOnBeatmap"),
        }
    }

    #[test]
    fn test_s_new_format_invalid_mode_returns_none() {
        let cmd = parse_command("!s :99 123456", None).unwrap();
        assert_eq!(
            cmd,
            Command::ScoreOnBeatmap {
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(123456),
                score_id: None,
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
                mode: None,
                username: None,
                qq: None,
                beatmap_id: None,
                score_id: None,
                filters: None,
                limit: 20,
                limit_end: None,
                is_all: true,
            }
        );
    }

    #[test]
    fn test_get_default_mode() {
        let cmd = parse_command("!mode", None).unwrap();
        assert_eq!(cmd, Command::SetDefaultMode { mode: None });
    }

    #[test]
    fn test_set_default_mode_osu() {
        let cmd = parse_command("!mode 0", None).unwrap();
        assert_eq!(
            cmd,
            Command::SetDefaultMode {
                mode: Some(GameMode::Osu)
            }
        );
    }

    #[test]
    fn test_set_default_mode_mania() {
        let cmd = parse_command("!mode 3", None).unwrap();
        assert_eq!(
            cmd,
            Command::SetDefaultMode {
                mode: Some(GameMode::Mania)
            }
        );
    }

    #[test]
    fn test_set_default_mode_invalid_is_query() {
        let cmd = parse_command("!mode 5", None).unwrap();
        assert_eq!(cmd, Command::SetDefaultMode { mode: None });
    }

    #[test]
    fn test_set_default_mode_trailing_space() {
        let cmd = parse_command("!mode ", None).unwrap();
        assert_eq!(cmd, Command::SetDefaultMode { mode: None });
    }

    #[test]
    fn test_set_default_mode_multi_spaces() {
        let cmd = parse_command("!mode  0", None).unwrap();
        assert_eq!(
            cmd,
            Command::SetDefaultMode {
                mode: Some(GameMode::Osu)
            }
        );
    }

    #[test]
    fn test_set_default_mode_fullwidth_exclamation() {
        let cmd = parse_command("！mode 0", None).unwrap();
        assert_eq!(
            cmd,
            Command::SetDefaultMode {
                mode: Some(GameMode::Osu)
            }
        );
    }

    #[test]
    fn test_set_default_mode_string_name_gives_query() {
        let cmd = parse_command("!mode osu", None).unwrap();
        assert_eq!(cmd, Command::SetDefaultMode { mode: None });
    }

    #[test]
    fn test_set_default_mode_taiko() {
        let cmd = parse_command("!mode 1", None).unwrap();
        assert_eq!(
            cmd,
            Command::SetDefaultMode {
                mode: Some(GameMode::Taiko)
            }
        );
    }

    #[test]
    fn test_set_default_mode_catch() {
        let cmd = parse_command("!mode 2", None).unwrap();
        assert_eq!(
            cmd,
            Command::SetDefaultMode {
                mode: Some(GameMode::Catch)
            }
        );
    }

    #[test]
    fn test_set_default_mode_extra_args_gives_query() {
        let cmd = parse_command("!mode 0 extra", None).unwrap();
        assert_eq!(cmd, Command::SetDefaultMode { mode: None });
    }

    #[test]
    fn test_mode_newline_only() {
        let cmd = parse_command("!mode\n", None).unwrap();
        assert_eq!(cmd, Command::SetDefaultMode { mode: None });
    }

    #[test]
    fn test_mode_tab_separator() {
        let cmd = parse_command("!mode\t0", None).unwrap();
        assert_eq!(
            cmd,
            Command::SetDefaultMode {
                mode: Some(GameMode::Osu)
            }
        );
    }

    #[test]
    fn test_mode_no_space_not_mode_command() {
        // "!mode1" 不以 "!mode " 开头，也不等于 "!mode"，不是 !mode 命令
        assert!(parse_command("!mode1", None).is_none());
    }

    #[test]
    fn test_rv_bare() {
        let cmd = parse_command("!rv", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: false,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_beatmap_id() {
        let cmd = parse_command("!rv 12345", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: Some(12345),
                mode: None,
                mods: None,
                gif: false,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_score_id() {
        let cmd = parse_command("!rv 12345678901", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: Some(12345678901),
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: false,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_mods() {
        let cmd = parse_command("!rv +HD", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: Some(vec!["HD".to_string()]),
                gif: false,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_mods_and_beatmap() {
        let cmd = parse_command("!rv 99999 +HDDT", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: Some(99999),
                mode: None,
                mods: Some(vec!["HD".to_string(), "DT".to_string()]),
                gif: false,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_mode() {
        let cmd = parse_command("!rv :1", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: Some(GameMode::Taiko),
                mods: None,
                gif: false,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_mods_and_mode() {
        let cmd = parse_command("!rv +HD :3", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: Some(GameMode::Mania),
                mods: Some(vec!["HD".to_string()]),
                gif: false,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_rejected_suffix_alphanumeric() {
        assert!(parse_command("!rva", None).is_none());
    }

    #[test]
    fn test_rv_rejected_suffix_hyphen() {
        assert!(parse_command("!rv-bid", None).is_none());
    }

    #[test]
    fn test_rv_rejected_suffix_underscore() {
        assert!(parse_command("!rv_bid", None).is_none());
    }

    #[test]
    fn test_rv_non_numeric_arg() {
        assert!(parse_command("!rv abc", None).is_none());
    }

    #[test]
    fn test_rv_with_gif() {
        let cmd = parse_command("!rv --gif", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: true,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_gif_short() {
        let cmd = parse_command("!rv -g", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: true,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_beatmap_id_and_gif() {
        let cmd = parse_command("!rv 12345 --gif", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: Some(12345),
                mode: None,
                mods: None,
                gif: true,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_mods_and_gif() {
        let cmd = parse_command("!rv +HD --gif", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: Some(vec!["HD".to_string()]),
                gif: true,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_mode_and_gif() {
        let cmd = parse_command("!rv :1 --gif", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: Some(GameMode::Taiko),
                mods: None,
                gif: true,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_with_all_args_and_gif() {
        let cmd = parse_command("!rv 99999 +DT :3 --gif", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: Some(99999),
                mode: Some(GameMode::Mania),
                mods: Some(vec!["DT".to_string()]),
                gif: true,
                times: None,
            }
        );
    }

    #[test]
    fn test_rv_negative_arg() {
        assert!(parse_command("!rv -1", None).is_none());
    }

    #[test]
    fn test_rv_group_is_beatmap_preview() {
        let cmd = parse_command("!rv", None).unwrap();
        assert_eq!(cmd.group_name(), CommandGroup::BeatmapPreview);
    }

    #[test]
    fn test_rv_single_time() {
        let cmd = parse_command("!rv 01:30:000", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: false,
                times: Some(vec![90_000]),
            }
        );
    }

    #[test]
    fn test_rv_two_times() {
        let cmd = parse_command("!rv 01:00:000 02:00:000", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: false,
                times: Some(vec![60_000, 120_000]),
            }
        );
    }

    #[test]
    fn test_rv_beatmap_id_with_time() {
        let cmd = parse_command("!rv 12345 01:30:000", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: Some(12345),
                mode: None,
                mods: None,
                gif: false,
                times: Some(vec![90_000]),
            }
        );
    }

    #[test]
    fn test_rv_time_with_mods_and_mode() {
        let cmd = parse_command("!rv :1 +HD 01:30:000", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: Some(GameMode::Taiko),
                mods: Some(vec!["HD".to_string()]),
                gif: false,
                times: Some(vec![90_000]),
            }
        );
    }

    #[test]
    fn test_rv_two_times_invalid_order() {
        let cmd = parse_command("!rv 02:00:000 01:00:000", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: false,
                times: Some(vec![120_000, 60_000]),
            }
        );
    }

    #[test]
    fn test_rv_three_times_rejected() {
        assert!(parse_command("!rv 01:00:000 02:00:000 03:00:000", None).is_none());
    }

    #[test]
    fn test_rv_time_with_non_numeric() {
        assert!(parse_command("!rv 01:30:000 abc", None).is_none());
    }

    #[test]
    fn test_rv_time_with_gif() {
        let cmd = parse_command("!rv 01:30:000 --gif", None).unwrap();
        assert_eq!(
            cmd,
            Command::BeatmapPreview {
                score_id: None,
                beatmap_id: None,
                mode: None,
                mods: None,
                gif: true,
                times: Some(vec![90_000]),
            }
        );
    }

    #[test]
    fn test_p_with_hash_and_mode_order_independent() {
        let cmd1 = parse_command("!p :1 #5", None);
        let cmd2 = parse_command("!p #5 :1", None);
        assert_eq!(cmd1, cmd2);
        if let Some(Command::Pass { mode, limit, .. }) = cmd1 {
            assert_eq!(mode, Some(GameMode::Taiko));
            assert_eq!(limit, 5);
        } else {
            panic!("expected Pass command");
        }
    }

    #[test]
    fn test_r_with_beatmap_id() {
        let cmd = parse_command("!r 12345", None).unwrap();
        assert_eq!(
            cmd,
            Command::Recent {
                mode: None,
                username: None,
                qq: None,
                beatmap_id: Some(12345),
                score_id: None,
                limit: 1,
                limit_end: None,
                is_summary: false,
                filters: None,
            }
        );
    }

    #[test]
    fn test_parse_time_token_valid() {
        assert_eq!(parse_time_token("01:30:000"), Some(90_000));
        assert_eq!(parse_time_token("00:00:500"), Some(500));
        assert_eq!(parse_time_token("02:00:319"), Some(120_319));
        assert_eq!(parse_time_token("0:00:000"), Some(0));
        assert_eq!(
            parse_time_token("99:59:999"),
            Some(99 * 60_000 + 59_000 + 999)
        );
    }

    #[test]
    fn test_parse_time_token_invalid() {
        assert_eq!(parse_time_token("abc"), None);
        assert_eq!(parse_time_token("01:30:00"), None);
        assert_eq!(parse_time_token("1:30:0000"), None);
        assert_eq!(parse_time_token(""), None);
        assert_eq!(parse_time_token("01:30:000 "), None);
        assert_eq!(parse_time_token("01:60:000"), None);
        assert_eq!(parse_time_token("01:30:1000"), None);
    }
}
