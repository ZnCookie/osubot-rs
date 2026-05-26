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

#[derive(Debug, Clone)]
pub struct UserStats {
    pub user_id: i64,
    pub username: String,
    pub pp: f64,
    pub rank: i64,
    pub country_rank: i64,
    pub country_code: String, // e.g., "CN", "US", "JP"
    pub ranked_score: i64,
    pub accuracy: f64,
    pub playcount: i64,
    pub hits: i64,
    pub playtime: i64, // seconds
    pub rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    QuerySelf {
        mode: GameMode,
    },
    QueryUser {
        username: String,
        mode: GameMode,
    },
    QueryMentionedUser {
        qq: i64,
        mode: GameMode,
    },
    Bind {
        username: String,
    },
    Unbind,
    Highlight {
        mode: GameMode,
    },
    ProfileCard {
        username: Option<String>,
        qq: Option<i64>,
    },
    ScoreCard {
        username: Option<String>,
        mode: GameMode,
        include_fails: bool,
    },
}

#[derive(Debug, Clone)]
pub struct UserChange {
    pub rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
    pub pp_change: Option<f64>,
    pub accuracy_change: Option<f64>,
    pub playcount_change: Option<i64>,
    pub hits_change: Option<i64>,
    pub playtime_change: Option<i64>,
}

impl UserChange {
    pub fn has_changes(&self) -> bool {
        self.rank_change.is_some_and(|c| c != 0)
            || self.country_rank_change.is_some_and(|c| c != 0)
            || self.pp_change.is_some_and(|c| c != 0.0)
            || self.accuracy_change.is_some_and(|c| c != 0.0)
            || self.playcount_change.is_some_and(|c| c != 0)
            || self.hits_change.is_some_and(|c| c != 0)
            || self.playtime_change.is_some_and(|c| c != 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UserActivity {
    SemiActive, // 4h内有游玩记录
    Normal,     // 当日有游玩记录，或近期有活动但无今日记录
    NoRecent,   // 当日无游玩记录，8h内有活动
    Inactive,   // 48h以上无游玩记录
    UserNotExists,
}

#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub activity: UserActivity,
}

/// Grade ranks for scores
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    X,
    SS,
    S,
    A,
    B,
    C,
    D,
    F,
}

impl Grade {
    pub fn from_rank_str(s: &str) -> Grade {
        match s {
            "X" => Grade::X,
            "SS" => Grade::SS,
            "S" => Grade::S,
            "A" => Grade::A,
            "B" => Grade::B,
            "C" => Grade::C,
            "D" => Grade::D,
            _ => Grade::F,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Grade::X => "X",
            Grade::SS => "SS",
            Grade::S => "S",
            Grade::A => "A",
            Grade::B => "B",
            Grade::C => "C",
            Grade::D => "D",
            Grade::F => "F",
        }
    }
}

/// Beatmapset cover image URLs
#[derive(Debug, Clone)]
pub struct Covers {
    pub cover: String,
    pub cover_2x: String,
    pub card: String,
    pub card_2x: String,
    pub list: String,
    pub list_2x: String,
    pub slimcover: String,
    pub slimcover_2x: String,
}

/// Beatmapset info from osu! API v2
#[derive(Debug, Clone)]
pub struct Beatmapset {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub creator: String,
    pub covers: Covers,
}

/// Beatmap info from osu! API v2 (nested in score.beatmap)
#[derive(Debug, Clone)]
pub struct Beatmap {
    pub id: i64,
    pub difficulty_rating: f64,
    pub version: String,
    pub cs: f32,
    pub ar: f32,
    pub od: f32,
    pub hp: f32,
    pub bpm: f32,
    pub total_length: i32,
    pub hit_length: i32,
    pub max_combo: i32,
    pub circle_count: Option<i32>,
    pub slider_count: Option<i32>,
    pub spinner_count: Option<i32>,
    pub beatmapset: Option<Beatmapset>,
}

/// Hit statistics per judgment type
#[derive(Debug, Clone)]
pub struct LazerStatistics {
    pub perfect: i32,
    pub great: i32,
    pub good: i32,
    pub ok: i32,
    pub meh: i32,
    pub miss: i32,
    pub large_tick_hit: i32,
    pub large_tick_miss: i32,
    pub small_tick_hit: i32,
    pub small_tick_miss: i32,
    pub slider_tail_hit: i32,
    pub large_bonus: i32,
    pub small_bonus: i32,
    pub ignore_hit: i32,
    pub ignore_miss: i32,
    pub legacy_combo_increase: i32,
}

/// Score info parsed from API
#[derive(Debug, Clone)]
pub struct ScoreInfo {
    pub score: i64,
    pub pp: f64,
    pub accuracy: f64,
    pub max_combo: i32,
    pub grade: Grade,
    pub passed: bool,
    pub rank: String,
    pub ended_at: String,
    pub mods: Vec<String>,
    pub statistics: LazerStatistics,
    pub maximum_statistics: Option<LazerStatistics>,
}

/// Player info from score
#[derive(Debug, Clone)]
pub struct PlayerInfo {
    pub username: String,
    pub avatar_url: String,
}

/// Complete score card data for rendering
#[derive(Debug, Clone)]
pub struct ScoreCard {
    pub beatmap: Beatmap,
    pub player: PlayerInfo,
    pub score: ScoreInfo,
}

#[derive(Debug, Clone)]
pub struct QQMessage {
    pub group_id: i64,
    pub user_id: i64,
    pub message: String,
    pub mentioned_user_id: Option<i64>,
}
