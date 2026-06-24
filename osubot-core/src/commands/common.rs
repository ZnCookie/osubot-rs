use crate::types::GameMode;

pub(crate) const MAX_LIMIT: u32 = 200;
pub(crate) const SCORE_ID_THRESHOLD: u64 = 10_000_000;

/// Extract `:mode` suffix from rest. Returns (rest_without_mode, mode).
pub(crate) fn extract_mode(rest: &str) -> (String, Option<GameMode>) {
    if let Some(colon_pos) = rest.rfind(':') {
        let after_colon = &rest[colon_pos + 1..];
        let mode_token = after_colon.split_whitespace().next().unwrap_or("");
        let mode = GameMode::from_digit_str(mode_token);

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

/// Strict range parse: returns `Some((start, end))` only if both sides are valid `u32`.
/// Returns `None` for non-range tokens or tokens with unparseable parts.
pub(crate) fn try_parse_range(s: &str) -> Option<(u32, u32)> {
    let dash_pos = s.find('-')?;
    if dash_pos == 0 || dash_pos == s.len() - 1 {
        return None;
    }
    let start = s[..dash_pos].parse::<u32>().ok()?;
    let end = s[dash_pos + 1..].parse::<u32>().ok()?;
    Some((start, end))
}

/// Parse a limit string like "5" or "2-10" into (limit, limit_end).
pub(crate) fn parse_limit(num_str: &str) -> (u32, Option<u32>) {
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
pub(crate) fn extract_plus_suffix(
    rest: &str,
) -> (String, Option<Vec<String>>, Option<Vec<String>>) {
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

    let (first, rest_str) = match suffix.find(',') {
        Some(p) => (&suffix[..p], &suffix[p + 1..]),
        None => (suffix, ""),
    };

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
pub(crate) fn merge_mods_into_filters(
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

/// Common argument extraction results.
pub(crate) struct CommonArgs {
    pub(crate) mode: Option<GameMode>,
    pub(crate) raw_mods: Option<Vec<String>>,
    pub(crate) filters: Option<Vec<String>>,
    pub(crate) limit: u32,
    pub(crate) limit_end: Option<u32>,
    pub(crate) rest: String,
    pub(crate) explicit_position: bool,
}

/// Extract :mode, +mods, #N from a string in fixed order.
pub(crate) fn extract_common_args(s: &str, default_limit: u32) -> CommonArgs {
    let (s, mode) = extract_mode(s);
    let (s, raw_mods, mut filter_suffix) = extract_plus_suffix(&s);

    let (rest, limit, limit_end, explicit_position) = if let Some(hash_pos) = s.rfind('#') {
        let num_str = &s[hash_pos + 1..];
        let (l, le) = parse_limit(num_str);
        (s[..hash_pos].trim().to_string(), l, le, true)
    } else if let Some(last) = filter_suffix.as_mut().and_then(|f| f.last_mut()) {
        if let Some(hash_pos) = last.rfind('#') {
            let num_str = &last[hash_pos + 1..];
            let (l, le) = parse_limit(num_str);
            *last = last[..hash_pos].trim().to_string();
            if last.is_empty() {
                filter_suffix.as_mut().map(|f| f.pop());
            }
            (s.trim().to_string(), l, le, true)
        } else {
            (s.trim().to_string(), default_limit, None, false)
        }
    } else {
        (s.trim().to_string(), default_limit, None, false)
    };

    let filters = merge_mods_into_filters(raw_mods.clone(), filter_suffix);

    CommonArgs {
        mode,
        raw_mods,
        filters,
        limit,
        limit_end,
        rest,
        explicit_position,
    }
}

/// Parse `mm:ss:mmm` format to milliseconds.
pub(crate) fn parse_time_token(s: &str) -> Option<i64> {
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
