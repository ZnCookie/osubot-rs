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
}

impl TryFrom<u8> for GameMode {
    type Error = ();

    fn try_from(v: u8) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(GameMode::Osu),
            1 => Ok(GameMode::Taiko),
            2 => Ok(GameMode::Catch),
            3 => Ok(GameMode::Mania),
            _ => Err(()),
        }
    }
}

impl TryFrom<i32> for GameMode {
    type Error = ();

    fn try_from(v: i32) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(GameMode::Osu),
            1 => Ok(GameMode::Taiko),
            2 => Ok(GameMode::Catch),
            3 => Ok(GameMode::Mania),
            _ => Err(()),
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

    pub fn short_name(&self) -> &'static str {
        self.api_value()
    }

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
        match m {
            GameMode::Osu => rosu_mods::GameMode::Osu,
            GameMode::Taiko => rosu_mods::GameMode::Taiko,
            GameMode::Catch => rosu_mods::GameMode::Catch,
            GameMode::Mania => rosu_mods::GameMode::Mania,
        }
    }
}

#[cfg(feature = "rosu-conversions")]
impl From<GameMode> for rosu_pp::model::mode::GameMode {
    fn from(m: GameMode) -> Self {
        match m {
            GameMode::Osu => rosu_pp::model::mode::GameMode::Osu,
            GameMode::Taiko => rosu_pp::model::mode::GameMode::Taiko,
            GameMode::Catch => rosu_pp::model::mode::GameMode::Catch,
            GameMode::Mania => rosu_pp::model::mode::GameMode::Mania,
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
        assert_eq!(GameMode::try_from(4u8), Err(()));
        assert_eq!(GameMode::try_from(255u8), Err(()));
    }

    #[test]
    fn test_try_from_i32() {
        assert_eq!(GameMode::try_from(0i32), Ok(GameMode::Osu));
        assert_eq!(GameMode::try_from(-1i32), Err(()));
        assert_eq!(GameMode::try_from(4i32), Err(()));
    }

    #[test]
    fn test_into_u8() {
        let m: u8 = GameMode::Osu.into();
        assert_eq!(m, 0);
        let m: u8 = GameMode::Mania.into();
        assert_eq!(m, 3);
    }

    #[test]
    fn test_into_i32() {
        let m: i32 = GameMode::Catch.into();
        assert_eq!(m, 2);
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", GameMode::Osu), "osu!");
        assert_eq!(format!("{}", GameMode::Taiko), "taiko");
    }

    #[test]
    fn test_from_mode_str() {
        assert_eq!(GameMode::from_mode_str("0"), Some(GameMode::Osu));
        assert_eq!(GameMode::from_mode_str("99"), None);
        assert_eq!(GameMode::from_mode_str("osu"), None);
    }

    #[test]
    fn test_api_value() {
        assert_eq!(GameMode::Osu.api_value(), "osu");
        assert_eq!(GameMode::Catch.api_value(), "fruits");
    }

    #[test]
    fn test_serde_roundtrip() {
        let m = GameMode::Taiko;
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, "\"taiko\"");
        let deserialized: GameMode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, GameMode::Taiko);
    }
}
