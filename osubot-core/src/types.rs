#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    Osu = 0,
    Taiko = 1,
    Catch = 2,
    Mania = 3,
}

impl GameMode {
    pub fn from_str(s: &str) -> Option<GameMode> {
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
    Bind { username: String },
    Unbind,
    Help,
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
        self.rank_change.map_or(false, |c| c != 0)
            || self.country_rank_change.map_or(false, |c| c != 0)
            || self.pp_change.map_or(false, |c| c != 0.0)
            || self.accuracy_change.map_or(false, |c| c != 0.0)
            || self.playcount_change.map_or(false, |c| c != 0)
            || self.hits_change.map_or(false, |c| c != 0)
            || self.playtime_change.map_or(false, |c| c != 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UserActivity {
    Active,       // 新增游玩记录（最后4h有新记录）
    SemiActive,   // 有recent但无新增（最后4h有记录但无新增）
    Normal,       // 有快照无变化（最后8h内更新过但无变化）
    Inactive,     // 无变化（最后48h内无更新）
    NoRecent,     // 没变化（最后4-8h有快照但无变化）
    UserNotExists,
}

#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub activity: UserActivity,
    pub added_snapshot: bool,
    pub added_records: i32,
}
