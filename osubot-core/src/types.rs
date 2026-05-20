#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    QuerySelf { mode: GameMode },
    QueryUser { username: String, mode: GameMode },
    QueryMentionedUser { qq: i64, mode: GameMode },
    Bind { username: String },
    Unbind,
    Help,
    Highlight { mode: GameMode },
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
    pub added_snapshot: bool,
}
