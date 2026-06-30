use std::convert::TryFrom;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameMode {
    Osu = 0,
    Taiko = 1,
    Catch = 2,
    Mania = 3,
    RelaxOsu = 4,
    RelaxTaiko = 5,
    RelaxCatch = 6,
    AutoPilot = 8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidGameMode(pub i64);

impl fmt::Display for InvalidGameMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid game mode: {}", self.0)
    }
}

impl std::error::Error for InvalidGameMode {}

impl TryFrom<u8> for GameMode {
    type Error = InvalidGameMode;

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(GameMode::Osu),
            1 => Ok(GameMode::Taiko),
            2 => Ok(GameMode::Catch),
            3 => Ok(GameMode::Mania),
            4 => Ok(GameMode::RelaxOsu),
            5 => Ok(GameMode::RelaxTaiko),
            6 => Ok(GameMode::RelaxCatch),
            8 => Ok(GameMode::AutoPilot),
            _ => Err(InvalidGameMode(v as i64)),
        }
    }
}

impl TryFrom<i32> for GameMode {
    type Error = InvalidGameMode;

    fn try_from(v: i32) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(GameMode::Osu),
            1 => Ok(GameMode::Taiko),
            2 => Ok(GameMode::Catch),
            3 => Ok(GameMode::Mania),
            4 => Ok(GameMode::RelaxOsu),
            5 => Ok(GameMode::RelaxTaiko),
            6 => Ok(GameMode::RelaxCatch),
            8 => Ok(GameMode::AutoPilot),
            _ => Err(InvalidGameMode(v as i64)),
        }
    }
}

impl From<GameMode> for u8 {
    fn from(m: GameMode) -> u8 {
        m as u8
    }
}

impl From<GameMode> for i32 {
    fn from(m: GameMode) -> i32 {
        m as i32
    }
}

impl GameMode {
    /// 从数字字符串解析 GameMode（`"0"` → `Osu`，`"1"` → `Taiko`，`"2"` → `Catch`，`"3"` → `Mania`）。
    ///
    /// 仅接受 `"0"` ~ `"8"`（跳过 `"7"`），不接受 `"osu"` / `"taiko"` 等名称。
    /// 如需从名称解析，使用 `serde_json::from_str::<GameMode, _>(s)`。
    pub fn from_digit_str(s: &str) -> Option<GameMode> {
        match s.trim() {
            "0" => Some(GameMode::Osu),
            "1" => Some(GameMode::Taiko),
            "2" => Some(GameMode::Catch),
            "3" => Some(GameMode::Mania),
            "4" => Some(GameMode::RelaxOsu),
            "5" => Some(GameMode::RelaxTaiko),
            "6" => Some(GameMode::RelaxCatch),
            "8" => Some(GameMode::AutoPilot),
            _ => None,
        }
    }

    pub fn base_mode(&self) -> GameMode {
        match self {
            GameMode::RelaxOsu | GameMode::AutoPilot => GameMode::Osu,
            GameMode::RelaxTaiko => GameMode::Taiko,
            GameMode::RelaxCatch => GameMode::Catch,
            other => *other,
        }
    }

    pub fn is_relax(&self) -> bool {
        matches!(
            self,
            GameMode::RelaxOsu | GameMode::RelaxTaiko | GameMode::RelaxCatch
        )
    }

    pub fn is_autopilot(&self) -> bool {
        matches!(self, GameMode::AutoPilot)
    }

    pub fn is_sb_specific(&self) -> bool {
        self.is_relax() || self.is_autopilot()
    }

    pub fn name(&self) -> &'static str {
        match self {
            GameMode::Osu => "osu!",
            GameMode::Taiko => "taiko",
            GameMode::Catch => "catch",
            GameMode::Mania => "mania",
            GameMode::RelaxOsu => "osu!relax",
            GameMode::RelaxTaiko => "taikorelax",
            GameMode::RelaxCatch => "catchrelax",
            GameMode::AutoPilot => "autopilot",
        }
    }

    pub fn short_name(&self) -> &'static str {
        self.api_value()
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            GameMode::Osu => "osu",
            GameMode::Taiko => "taiko",
            GameMode::Catch => "catch",
            GameMode::Mania => "mania",
            GameMode::RelaxOsu => "osu relax",
            GameMode::RelaxTaiko => "taiko relax",
            GameMode::RelaxCatch => "catch relax",
            GameMode::AutoPilot => "autopilot",
        }
    }

    pub fn api_value(&self) -> &'static str {
        self.base_mode().api_value_inner()
    }

    fn api_value_inner(&self) -> &'static str {
        match self {
            GameMode::Osu => "osu",
            GameMode::Taiko => "taiko",
            GameMode::Catch => "fruits",
            GameMode::Mania => "mania",
            _ => unreachable!(),
        }
    }
}

impl fmt::Display for GameMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl fmt::Debug for GameMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

#[cfg(feature = "rosu-conversions")]
impl From<GameMode> for rosu_mods::GameMode {
    fn from(m: GameMode) -> Self {
        match m.base_mode() {
            GameMode::Osu => rosu_mods::GameMode::Osu,
            GameMode::Taiko => rosu_mods::GameMode::Taiko,
            GameMode::Catch => rosu_mods::GameMode::Catch,
            GameMode::Mania => rosu_mods::GameMode::Mania,
            _ => unreachable!(),
        }
    }
}

#[cfg(feature = "rosu-conversions")]
impl From<GameMode> for rosu_pp::model::mode::GameMode {
    fn from(m: GameMode) -> Self {
        match m.base_mode() {
            GameMode::Osu => rosu_pp::model::mode::GameMode::Osu,
            GameMode::Taiko => rosu_pp::model::mode::GameMode::Taiko,
            GameMode::Catch => rosu_pp::model::mode::GameMode::Catch,
            GameMode::Mania => rosu_pp::model::mode::GameMode::Mania,
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_try_from_u8() {
        assert_eq!(GameMode::try_from(0u8), Ok(GameMode::Osu));
        assert_eq!(GameMode::try_from(1u8), Ok(GameMode::Taiko));
        assert_eq!(GameMode::try_from(2u8), Ok(GameMode::Catch));
        assert_eq!(GameMode::try_from(3u8), Ok(GameMode::Mania));
        assert_eq!(GameMode::try_from(4u8), Ok(GameMode::RelaxOsu));
        assert_eq!(GameMode::try_from(5u8), Ok(GameMode::RelaxTaiko));
        assert_eq!(GameMode::try_from(6u8), Ok(GameMode::RelaxCatch));
        assert_eq!(GameMode::try_from(8u8), Ok(GameMode::AutoPilot));
        assert_eq!(GameMode::try_from(7u8), Err(InvalidGameMode(7)));
        assert_eq!(GameMode::try_from(255u8), Err(InvalidGameMode(255)));
    }

    #[test]
    fn test_try_from_i32() {
        assert_eq!(GameMode::try_from(0i32), Ok(GameMode::Osu));
        assert_eq!(GameMode::try_from(4i32), Ok(GameMode::RelaxOsu));
        assert_eq!(GameMode::try_from(8i32), Ok(GameMode::AutoPilot));
        assert_eq!(GameMode::try_from(7i32), Err(InvalidGameMode(7)));
        assert_eq!(GameMode::try_from(-1i32), Err(InvalidGameMode(-1)));
    }

    #[test]
    fn test_into_u8() {
        let m: u8 = GameMode::Osu.into();
        assert_eq!(m, 0);
        let m: u8 = GameMode::RelaxOsu.into();
        assert_eq!(m, 4);
        let m: u8 = GameMode::AutoPilot.into();
        assert_eq!(m, 8);
        let m: u8 = GameMode::Mania.into();
        assert_eq!(m, 3);
    }

    #[test]
    fn test_into_i32() {
        let m: i32 = GameMode::Catch.into();
        assert_eq!(m, 2);
        let m: i32 = GameMode::RelaxTaiko.into();
        assert_eq!(m, 5);
        let m: i32 = GameMode::AutoPilot.into();
        assert_eq!(m, 8);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", GameMode::Osu), "osu!");
        assert_eq!(format!("{}", GameMode::Taiko), "taiko");
        assert_eq!(format!("{}", GameMode::RelaxOsu), "osu!relax");
        assert_eq!(format!("{}", GameMode::AutoPilot), "autopilot");
    }

    #[test]
    fn test_from_digit_str() {
        assert_eq!(GameMode::from_digit_str("0"), Some(GameMode::Osu));
        assert_eq!(GameMode::from_digit_str("1"), Some(GameMode::Taiko));
        assert_eq!(GameMode::from_digit_str("2"), Some(GameMode::Catch));
        assert_eq!(GameMode::from_digit_str("3"), Some(GameMode::Mania));
        assert_eq!(GameMode::from_digit_str("4"), Some(GameMode::RelaxOsu));
        assert_eq!(GameMode::from_digit_str("5"), Some(GameMode::RelaxTaiko));
        assert_eq!(GameMode::from_digit_str("6"), Some(GameMode::RelaxCatch));
        assert_eq!(GameMode::from_digit_str("8"), Some(GameMode::AutoPilot));
        assert_eq!(GameMode::from_digit_str("7"), None);
    }

    #[test]
    fn test_from_digit_str_invalid_returns_none() {
        assert_eq!(GameMode::from_digit_str("99"), None);
        assert_eq!(GameMode::from_digit_str("xyz"), None);
        assert_eq!(GameMode::from_digit_str("osu!"), None);
        assert_eq!(GameMode::from_digit_str("std"), None);
        assert_eq!(GameMode::from_digit_str("taiko"), None);
        assert_eq!(GameMode::from_digit_str("catch"), None);
        assert_eq!(GameMode::from_digit_str("mania"), None);
        assert_eq!(GameMode::from_digit_str(""), None);
        assert_eq!(GameMode::from_digit_str(" 0 "), Some(GameMode::Osu));
    }

    #[test]
    fn test_api_value() {
        assert_eq!(GameMode::Osu.api_value(), "osu");
        assert_eq!(GameMode::Catch.api_value(), "fruits");
        assert_eq!(GameMode::RelaxOsu.api_value(), "osu");
        assert_eq!(GameMode::AutoPilot.api_value(), "osu");
        assert_eq!(GameMode::RelaxTaiko.api_value(), "taiko");
        assert_eq!(GameMode::RelaxCatch.api_value(), "fruits");
    }

    #[test]
    fn test_serde_roundtrip() {
        let m = GameMode::Taiko;
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, "\"taiko\"");
        let deserialized: GameMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, GameMode::Taiko);
    }

    #[test]
    fn test_base_mode() {
        assert_eq!(GameMode::Osu.base_mode(), GameMode::Osu);
        assert_eq!(GameMode::Taiko.base_mode(), GameMode::Taiko);
        assert_eq!(GameMode::Catch.base_mode(), GameMode::Catch);
        assert_eq!(GameMode::Mania.base_mode(), GameMode::Mania);
        assert_eq!(GameMode::RelaxOsu.base_mode(), GameMode::Osu);
        assert_eq!(GameMode::RelaxTaiko.base_mode(), GameMode::Taiko);
        assert_eq!(GameMode::RelaxCatch.base_mode(), GameMode::Catch);
        assert_eq!(GameMode::AutoPilot.base_mode(), GameMode::Osu);
    }

    #[test]
    fn test_is_relax() {
        assert!(GameMode::RelaxOsu.is_relax());
        assert!(GameMode::RelaxTaiko.is_relax());
        assert!(GameMode::RelaxCatch.is_relax());
        assert!(!GameMode::AutoPilot.is_relax());
        assert!(!GameMode::Osu.is_relax());
    }

    #[test]
    fn test_is_autopilot() {
        assert!(GameMode::AutoPilot.is_autopilot());
        assert!(!GameMode::RelaxOsu.is_autopilot());
        assert!(!GameMode::Osu.is_autopilot());
    }

    #[test]
    fn test_is_sb_specific() {
        assert!(GameMode::RelaxOsu.is_sb_specific());
        assert!(GameMode::RelaxTaiko.is_sb_specific());
        assert!(GameMode::RelaxCatch.is_sb_specific());
        assert!(GameMode::AutoPilot.is_sb_specific());
        assert!(!GameMode::Osu.is_sb_specific());
        assert!(!GameMode::Taiko.is_sb_specific());
        assert!(!GameMode::Catch.is_sb_specific());
        assert!(!GameMode::Mania.is_sb_specific());
    }
}
