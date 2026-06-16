use chrono::{DateTime, FixedOffset, Utc};
use rosu_mods::GameMods;

fn default_true() -> bool {
    true
}

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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameMode {
    Osu = 0,
    Taiko = 1,
    Catch = 2,
    Mania = 3,
}

impl GameMode {
    /// 将用户输入的 mode 字符串解析为 [`GameMode`]。
    ///
    /// 接受 `"0"` / `"1"` / `"2"` / `"3"`（trim 后精确匹配），返回对应的 `Some(GameMode)`。
    /// 任何其他输入（包括空串、字母别名如 `"osu"` / `"taiko"` / `"std"`、范围外数字等）均返回 `None`。
    /// 调用方负责对 `None` 走 fallback（通常是 `Osu` 或提示用户）。
    pub fn from_mode_str(s: &str) -> Option<GameMode> {
        match s.trim() {
            "0" => Some(GameMode::Osu),
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

    /// 短名称，用于成绩响应中显示（兼容 yumubot 格式）
    pub fn short_name(&self) -> &'static str {
        self.api_value()
    }

    /// 面向用户的纯文本名称，用于 set/get 默认模式等用户消息。
    /// 与 `name()` 的区别：Osu 模式不带 `!`，避免出现"你的默认模式是 osu!"这种突兀措辞。
    pub fn display_name(&self) -> &'static str {
        match self {
            GameMode::Osu => "osu",
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

impl std::fmt::Display for GameMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl std::fmt::Debug for GameMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// PP 分解结果，包含各分项 PP 和谱面星级。
///
/// - `aim`/`speed`/`flashlight`：仅 Std 模式有值
/// - `difficulty`：Taiko/Mania 模式有值（总 PP 的主要构成）
/// - `accuracy`：Std/Taiko 有值，Mania/Catch 为 0.0
/// - `total_pp`：总 PP 值（NF/CL fast path 时为 0.0，因为 `score.pp` 已来自 API）
/// - `star_rating`：谱面星级
///   - 普通路径：来自 `PerformanceAttributes::stars()`（含 mod 调整）
///   - NF/CL fast path：使用 API 传入的原始星级（NF/CL 不影响难度）
///   - 转谱场景：返回转换后模式的星级
///   - `None`：PP 计算失败时
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PpBreakdown {
    /// Std 模式的 Aim 分项 PP，其他模式为 `None`
    #[serde(default)]
    pub aim: Option<f64>,
    /// Std 模式的 Speed 分项 PP，其他模式为 `None`
    #[serde(default)]
    pub speed: Option<f64>,
    /// Accuracy 分项 PP（Std/Taiko 有值，Mania/Catch 为 0.0）
    pub accuracy: f64,
    /// Std 模式的 Flashlight 分项 PP，其他模式为 `None`
    #[serde(default)]
    pub flashlight: Option<f64>,
    /// Taiko/Mania 模式的 Difficulty 分项 PP，Std/Catch 为 `None`
    #[serde(default)]
    pub difficulty: Option<f64>,
    /// 总 PP 值（NF/CL fast path 时为 0.0）
    pub total_pp: f64,
    /// 谱面星级（含 mod 调整），`None` 表示未计算
    #[serde(default)]
    pub star_rating: Option<f64>,
}

/// 不同准确率下的 PP 预测值。
///
/// 用于展示"如果达到 X% 准确率能获得多少 PP"。
/// Mania 模式下通过 hit counts 计算，其他模式通过 accuracy 直接计算。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PpIfAcc {
    pub acc_95: f64,
    pub acc_97: f64,
    pub acc_98: f64,
    pub acc_99: f64,
    pub acc_100: f64,
    /// 如果 Full Combo 能获得的 PP
    pub if_fc: f64,
    /// 理论 Perfect (SS) PP，无状态计算（仅谱面+mods）
    #[serde(default)]
    pub perfect_pp: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoreStatistics {
    pub count_geki: i64,
    pub count_300: i64,
    pub count_katu: i64,
    pub count_100: i64,
    pub count_50: i64,
    pub count_miss: i64,
    /// Osu slider tick hits / Catch large droplet hits (lazer)
    pub osu_large_tick_hits: i64,
    /// Osu small slider tick hits / Catch small droplet hits (lazer)
    pub osu_small_tick_hits: i64,
    /// Osu slider end hits (lazer)
    pub osu_slider_tail_hits: i64,
    /// Catch large droplets missed (lazer)
    #[serde(default)]
    pub osu_large_tick_misses: i64,
    /// Catch small droplets missed (lazer)
    #[serde(default)]
    pub osu_small_tick_misses: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoreUser {
    pub avatar_url: String,
    pub country_code: String,
    #[serde(default)]
    pub user_id: Option<i64>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub global_rank: Option<i64>,
    #[serde(default)]
    pub country_rank: Option<i64>,
    pub pp: f64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    #[serde(default)]
    pub pp: Option<f64>,
    #[serde(default)]
    pub pp_breakdown: Option<PpBreakdown>,
    #[serde(default)]
    pub pp_if_acc: Option<PpIfAcc>,
    /// 理论 Perfect (SS) PP，来自 PpIfAcc
    #[serde(default)]
    pub perfect_pp: Option<f64>,
    pub rank: String,
    #[serde(default = "default_true")]
    pub passed: bool,
    #[serde(skip)]
    pub mods: GameMods,
    pub is_perfect: bool,
    pub created_at: String,
    pub is_lazer: bool,
    pub has_replay: bool,
    #[serde(default)]
    pub legacy_score_id: Option<i64>,
    pub statistics: ScoreStatistics,
    pub cover_url: String,
    pub user: ScoreUser,
    #[serde(default)]
    pub fav_count: Option<i64>,
    #[serde(default)]
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
        let offset = FixedOffset::east_opt(offset_hours * 3600)
            .unwrap_or_else(|| FixedOffset::east_opt(8 * 3600).unwrap());
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
///
/// **Important:** This function expects a 0-1 fraction (e.g., 0.985 for 98.5%).
/// The osu! API v2 returns score accuracy as 0-1 fraction, but user stats
/// `hit_accuracy` as a percentage (e.g., 98.5). Divide by 100 before calling
/// if the source is a percentage.
pub fn format_accuracy(accuracy: f64) -> String {
    let accuracy = accuracy.clamp(0.0, 1.0);
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

    #[test]
    fn test_game_mode_display() {
        assert_eq!(format!("{}", GameMode::Osu), "osu!");
        assert_eq!(format!("{}", GameMode::Taiko), "taiko");
        assert_eq!(format!("{}", GameMode::Catch), "catch");
        assert_eq!(format!("{}", GameMode::Mania), "mania");
    }

    #[test]
    fn test_format_accuracy_fraction_input() {
        assert_eq!(format_accuracy(0.985), "98.5%");
        assert_eq!(format_accuracy(0.9899), "98.99%");
        assert_eq!(format_accuracy(1.0), "100%");
        assert_eq!(format_accuracy(0.0), "0%");
        assert_eq!(format_accuracy(0.5), "50%");
    }

    #[test]
    fn test_format_accuracy_edge_cases() {
        assert_eq!(format_accuracy(0.9899), "98.99%");
        assert_eq!(format_accuracy(-0.5), "0%");
        assert_eq!(format_accuracy(1.5), "100%");
        assert_eq!(format_accuracy(0.0), "0%");
        assert_eq!(format_accuracy(0.0001), "0.01%");
    }

    #[test]
    fn test_from_mode_str_invalid_returns_none() {
        assert_eq!(GameMode::from_mode_str("99"), None);
        assert_eq!(GameMode::from_mode_str("xyz"), None);
        assert_eq!(GameMode::from_mode_str("osu!"), None);
        assert_eq!(GameMode::from_mode_str("std"), None);
        assert_eq!(GameMode::from_mode_str("taiko"), None);
        assert_eq!(GameMode::from_mode_str("catch"), None);
        assert_eq!(GameMode::from_mode_str("mania"), None);
    }
}
