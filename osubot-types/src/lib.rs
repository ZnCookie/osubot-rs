use chrono::{DateTime, FixedOffset, Utc};
use rosu_mods::GameMods;

/// Format an integer with comma separators (e.g., 1234567 -> "1,234,567")
pub fn format_number(value: i64) -> String {
    let is_negative = value < 0;
    let abs_str = value.unsigned_abs().to_string();
    let mut result = String::new();
    for (i, c) in abs_str.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    if is_negative {
        result.push('-');
    }
    result.chars().rev().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameMode {
    Osu = 0,
    Taiko = 1,
    Catch = 2,
    Mania = 3,
}

impl GameMode {
    pub fn from_mode_str(s: &str) -> Option<GameMode> {
        match s.trim() {
            "0" | "" => Some(GameMode::Osu),
            "1" => Some(GameMode::Taiko),
            "2" => Some(GameMode::Catch),
            "3" => Some(GameMode::Mania),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            GameMode::Osu => "osu!",
            GameMode::Taiko => "taiko",
            GameMode::Catch => "catch",
            GameMode::Mania => "mania",
        }
    }

    pub fn api_value(&self) -> &'static str {
        match self {
            GameMode::Osu => "osu",
            GameMode::Taiko => "taiko",
            GameMode::Catch => "fruits",
            GameMode::Mania => "mania",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PpBreakdown {
    pub aim: Option<f64>,
    pub speed: Option<f64>,
    pub accuracy: f64,
    pub flashlight: Option<f64>,
    pub difficulty: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct PpIfAcc {
    pub acc_95: f64,
    pub acc_97: f64,
    pub acc_98: f64,
    pub acc_99: f64,
    pub acc_100: f64,
    pub if_fc: f64,
}

#[derive(Debug, Clone)]
pub struct ScoreStatistics {
    pub count_300: i64,
    pub count_100: i64,
    pub count_50: i64,
    pub count_miss: i64,
}

#[derive(Debug, Clone)]
pub struct ScoreUser {
    pub avatar_url: String,
    pub country_code: String,
    pub global_rank: Option<i64>,
    pub country_rank: Option<i64>,
    pub pp: f64,
}

#[derive(Debug, Clone)]
pub struct Score {
    pub score_id: i64,
    pub beatmap_id: i64,
    pub beatmapset_id: i64,
    pub artist: String,
    pub title: String,
    pub version: String,
    pub creator: String,
    pub star_rating: f64,
    pub bpm: f64,
    pub ar: f64,
    pub od: f64,
    pub cs: f64,
    pub hp: f64,
    pub length_seconds: i64,
    pub score_value: i64,
    pub accuracy: f64,
    pub max_combo: i64,
    pub beatmap_max_combo: i64,
    pub pp: Option<f64>,
    pub pp_breakdown: Option<PpBreakdown>,
    pub pp_if_acc: Option<PpIfAcc>,
    pub rank: String,
    pub mods: GameMods,
    pub is_perfect: bool,
    pub created_at: String,
    pub is_lazer: bool,
    pub has_replay: bool,
    pub legacy_score_id: Option<i64>,
    pub statistics: ScoreStatistics,
    pub cover_url: String,
    pub user: ScoreUser,
    pub fav_count: Option<i64>,
    pub play_count: Option<i64>,
    pub status: String,
}

/// Format beatmap length as M:SS
pub fn format_length(seconds: i64) -> String {
    let minutes = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", minutes, secs)
}

/// Format ISO 8601 timestamp as YYYY/MM/DD HH:MM:SS (default UTC+8)
pub fn format_play_datetime(created_at: &str) -> String {
    format_play_datetime_with_offset(created_at, 8)
}

/// Format ISO 8601 timestamp with specified UTC offset (in hours)
pub fn format_play_datetime_with_offset(created_at: &str, offset_hours: i32) -> String {
    if let Ok(dt) = created_at.parse::<DateTime<Utc>>() {
        let offset = FixedOffset::east_opt(offset_hours * 3600).unwrap();
        let local = dt.with_timezone(&offset);
        return local.format("%Y/%m/%d %H:%M:%S").to_string();
    }
    created_at.to_string()
}

/// Convert osubot GameMode to rosu_mods GameMode
pub fn to_rosu_game_mode(mode: GameMode) -> rosu_mods::GameMode {
    match mode {
        GameMode::Osu => rosu_mods::GameMode::Osu,
        GameMode::Taiko => rosu_mods::GameMode::Taiko,
        GameMode::Catch => rosu_mods::GameMode::Catch,
        GameMode::Mania => rosu_mods::GameMode::Mania,
    }
}

/// Format GameMods as space-separated mod acronyms (e.g., "HD DT")
pub fn format_mods(mods: &GameMods) -> String {
    if mods.is_empty() {
        return String::new();
    }
    mods.iter()
        .map(|m| m.acronym().as_str().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Remove trailing zeros from a formatted number string (e.g. "98.50" -> "98.5", "98.00" -> "98")
pub fn trim_trailing_zeros(s: &str) -> String {
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Format accuracy (0-1 fraction) as truncated percentage using floor.
/// Matches official osu!lazer behavior: 0.989999 → "98.99%", not "99.00%".
pub fn format_accuracy(accuracy: f64) -> String {
    let pct = (accuracy * 10000.0).floor() / 100.0;
    format!("{}%", trim_trailing_zeros(&format!("{:.2}", pct)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_length() {
        assert_eq!(format_length(222), "3:42");
        assert_eq!(format_length(60), "1:00");
        assert_eq!(format_length(0), "0:00");
    }

    #[test]
    fn test_format_play_datetime_iso() {
        // 默认 UTC+8
        assert_eq!(
            format_play_datetime("2025-05-27T14:30:22Z"),
            "2025/05/27 22:30:22"
        );
    }

    #[test]
    fn test_format_play_datetime_utc() {
        assert_eq!(
            format_play_datetime_with_offset("2025-05-27T14:30:22Z", 0),
            "2025/05/27 14:30:22"
        );
    }

    #[test]
    fn test_format_play_datetime_fallback() {
        assert_eq!(format_play_datetime("not a date"), "not a date");
    }

    #[test]
    fn test_format_number_zero() {
        assert_eq!(format_number(0), "0");
    }

    #[test]
    fn test_format_number_negative() {
        assert_eq!(format_number(-1), "-1");
        assert_eq!(format_number(-1000), "-1,000");
    }

    #[test]
    fn test_format_number_i64_min() {
        assert_eq!(format_number(i64::MIN), "-9,223,372,036,854,775,808");
    }

    #[test]
    fn test_format_number_i64_max() {
        assert_eq!(format_number(i64::MAX), "9,223,372,036,854,775,807");
    }

    #[test]
    fn test_format_number_positive() {
        assert_eq!(format_number(1000), "1,000");
        assert_eq!(format_number(1000000), "1,000,000");
    }
}
