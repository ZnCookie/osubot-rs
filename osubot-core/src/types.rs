pub use osubot_types::{
    format_length, format_play_datetime, GameMode, Score, ScoreStatistics, ScoreUser,
};

#[derive(Debug, Clone)]
pub struct UserStats {
    pub user_id: i64,
    pub username: String,
    pub pp: f64,
    pub rank: i64,
    pub country_rank: i64,
    pub country_code: String,
    pub ranked_score: i64,
    pub accuracy: f64,
    pub playcount: i64,
    pub hits: i64,
    pub playtime: i64,
    pub rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandGroup {
    Query,
    Score,
    Profile,
    Highlight,
    Bind,
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
    ScoreOnBeatmap {
        mode: GameMode,
        username: Option<String>,
        qq: Option<i64>,
        beatmap_id: Option<u32>,
        score_id: Option<u64>,
        mods: Option<Vec<String>>,
        limit: u32,
        is_all: bool,
    },
    Pass {
        mode: GameMode,
        username: Option<String>,
        qq: Option<i64>,
        limit: u32,
        is_summary: bool,
    },
    Recent {
        mode: GameMode,
        username: Option<String>,
        qq: Option<i64>,
        limit: u32,
        is_summary: bool,
    },
}

impl Command {
    pub fn group_name(&self) -> CommandGroup {
        match self {
            Command::QuerySelf { .. }
            | Command::QueryUser { .. }
            | Command::QueryMentionedUser { .. } => CommandGroup::Query,
            Command::Pass { .. } | Command::Recent { .. } => CommandGroup::Score,
            Command::ProfileCard { .. } => CommandGroup::Profile,
            Command::ScoreOnBeatmap { .. } => CommandGroup::Score,
            Command::Highlight { .. } => CommandGroup::Highlight,
            Command::Bind { .. } | Command::Unbind => CommandGroup::Bind,
        }
    }
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
    SemiActive,
    Normal,
    NoRecent,
    Inactive,
    UserNotExists,
}

#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub activity: UserActivity,
}
