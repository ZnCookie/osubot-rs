use crate::types::{Command, GameMode};

use super::common::{extract_common_args, parse_time_token, try_parse_range, SCORE_ID_THRESHOLD};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScoringCmd {
    PassSummary,
    RecentSummary,
    ScoreAll,
    PassSingle,
    RecentSingle,
    ScoreSingle,
    BeatmapPreview,
    BestSingle,
    BestList,
    BeatmapAudio,
}

pub(crate) fn parse_scoring_command(
    msg: &str,
    mentioned_user_id: Option<i64>,
) -> Option<Option<Command>> {
    const PREFIXES: &[(&str, ScoringCmd)] = &[
        ("!ps", ScoringCmd::PassSummary),
        ("!rs", ScoringCmd::RecentSummary),
        ("!ss", ScoringCmd::ScoreAll),
        ("!bs", ScoringCmd::BestList),
        ("!p", ScoringCmd::PassSingle),
        ("!r", ScoringCmd::RecentSingle),
        ("!s", ScoringCmd::ScoreSingle),
        ("!b", ScoringCmd::BestSingle),
        ("!a", ScoringCmd::BeatmapAudio),
        ("!rv", ScoringCmd::BeatmapPreview),
    ];

    for &(prefix, cmd) in PREFIXES {
        let rest = match msg.strip_prefix(prefix) {
            Some(r) => r,
            None => continue,
        };
        if rest.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '-') {
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
    let mut gif = false;
    let mut times: Vec<i64> = Vec::new();
    let mut pass1_tokens: Vec<&str> = Vec::new();

    for token in rest.split_whitespace() {
        if token == "--gif" || token == "-g" {
            gif = true;
        } else if let Some(ms) = parse_time_token(token) {
            times.push(ms);
            if times.len() > 2 {
                return None;
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
            username: None,
            qq: None,
            mods: None,
            gif,
            times: None,
            limit: 1,
            filters: None,
            explicit_position: false,
        });
    }

    let args = extract_common_args(&remaining, 1);

    if args.rest.is_empty() && !times.is_empty() {
        return Some(Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: args.mode,
            username: None,
            qq: None,
            mods: args.raw_mods,
            gif,
            times: Some(times),
            limit: 1,
            filters: None,
            explicit_position: false,
        });
    }

    if args.rest.is_empty() {
        return Some(Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: args.mode,
            username: None,
            qq: None,
            mods: args.raw_mods,
            gif,
            times: None,
            limit: 1,
            filters: None,
            explicit_position: false,
        });
    }

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
        username: None,
        qq: None,
        mods: args.raw_mods,
        gif,
        times: if times.is_empty() { None } else { Some(times) },
        limit: 1,
        filters: None,
        explicit_position: false,
    })
}

fn parse_standard_score(
    rest: &str,
    cmd: ScoringCmd,
    mentioned_user_id: Option<i64>,
) -> Option<Command> {
    let default_limit = match cmd {
        ScoringCmd::PassSummary | ScoringCmd::RecentSummary | ScoringCmd::ScoreAll => 20u32,
        ScoringCmd::PassSingle
        | ScoringCmd::RecentSingle
        | ScoringCmd::ScoreSingle
        | ScoringCmd::BeatmapAudio => 1u32,
        ScoringCmd::BestSingle => 1u32,
        ScoringCmd::BestList => 20u32,
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
            explicit_position: false,
        });
    }

    let args = extract_common_args(rest, default_limit);

    let rt = parse_remaining_tokens(&args.rest, mentioned_user_id)?;

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

    let single_cmd = matches!(
        cmd,
        ScoringCmd::PassSingle
            | ScoringCmd::RecentSingle
            | ScoringCmd::ScoreSingle
            | ScoringCmd::BestSingle
    );
    let range_given = rt.limit_end.is_some() || args.limit_end.is_some();

    // single 命令与 !a 不接受区间语法（区间请用对应 summary 命令）。
    if (single_cmd || matches!(cmd, ScoringCmd::BeatmapAudio)) && range_given {
        return None;
    }

    let final_limit_end = if single_cmd {
        None
    } else {
        rt.limit_end.or(args.limit_end)
    };

    let explicit_position = matches!(cmd, ScoringCmd::BeatmapAudio)
        && (rt.implicit_limit.is_some() || args.explicit_position);

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
        explicit_position,
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
    explicit_position: bool,
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
        ScoringCmd::BestSingle => Command::Best {
            mode: params.mode,
            username: params.username,
            qq,
            limit: params.limit,
            limit_end: params.limit_end,
            is_summary: false,
            filters: params.filters,
        },
        ScoringCmd::BestList => Command::Best {
            mode: params.mode,
            username: params.username,
            qq,
            limit: params.limit,
            limit_end: params.limit_end,
            is_summary: true,
            filters: params.filters,
        },
        ScoringCmd::BeatmapAudio => Command::BeatmapAudio {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            limit: params.limit,
            filters: params.filters,
            explicit_position: params.explicit_position,
        },
        ScoringCmd::BeatmapPreview => Command::BeatmapPreview {
            mode: params.mode,
            username: params.username,
            qq,
            beatmap_id: params.beatmap_id,
            score_id: params.score_id,
            mods: None,
            gif: false,
            times: None,
            limit: params.limit,
            filters: params.filters,
            explicit_position: params.explicit_position,
        },
    })
}

struct RemainingTokens {
    beatmap_id: Option<u32>,
    score_id: Option<u64>,
    username: Option<String>,
    qq: Option<i64>,
    filters: Option<Vec<String>>,
    implicit_limit: Option<u32>,
    limit_end: Option<u32>,
}

fn parse_remaining_tokens(rest: &str, mentioned_user_id: Option<i64>) -> Option<RemainingTokens> {
    use super::common::{MAX_LIMIT, SCORE_ID_THRESHOLD};

    if rest.is_empty() {
        return Some(RemainingTokens {
            beatmap_id: None,
            score_id: None,
            username: None,
            qq: mentioned_user_id,
            filters: None,
            implicit_limit: None,
            limit_end: None,
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
    let mut limit_end: Option<u32> = None;
    let mut has_invalid_mention = false;

    for token in &tokens {
        if let Some(qq_str) = token.strip_prefix("qq=") {
            if let Ok(parsed) = qq_str.parse::<i64>() {
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
            if let Some((start, end)) = try_parse_range(token) {
                if start > MAX_LIMIT {
                    return None;
                }
                let clamped_start = start.clamp(1, MAX_LIMIT);
                implicit_limit = Some(clamped_start);
                limit_end = Some(end.clamp(clamped_start, MAX_LIMIT));
                continue;
            }
            if let Ok(num) = token.parse::<u64>() {
                if num >= SCORE_ID_THRESHOLD {
                    score_id = Some(num);
                } else if num <= MAX_LIMIT as u64 {
                    implicit_limit = Some((num as u32).clamp(1, MAX_LIMIT));
                } else if beatmap_id.is_none() {
                    beatmap_id = Some(num as u32);
                } else {
                    // 第二个谱面级数字（201..SCORE_ID_THRESHOLD）与既有 beatmap_id 冲突，
                    // 语义无法确定，直接拒绝。
                    return None;
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

    if qq_id.is_some() && username.is_some() {
        return None;
    }

    // score_id 与 beatmap_id 不可共存：语义冲突，交由调用方拒绝。
    if score_id.is_some() && beatmap_id.is_some() {
        return None;
    }

    Some(RemainingTokens {
        beatmap_id,
        score_id,
        username,
        qq,
        filters,
        implicit_limit,
        limit_end,
    })
}
