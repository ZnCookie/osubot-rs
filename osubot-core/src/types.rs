pub use osubot_types::{
    format_length, format_play_datetime, GameMode, Score, ScoreStatistics, ScoreUser,
};

/// Snapshot of a user's osu! profile statistics.
#[derive(Debug, Clone, serde::Serialize)]
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

/// Group category of a command, used for per-group enable/disable in config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandGroup {
    Query,
    Score,
    Profile,
    Highlight,
    Bind,
}

/// #N 范围选择结果
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreRange {
    /// 0-based offset（#1 → offset=0 后修正）
    pub offset: usize,
    /// 数量（0 = 取到末尾）
    pub count: usize,
}

impl ScoreRange {
    /// 单条：第 N 条
    pub fn single(n: usize) -> Self {
        Self {
            offset: n.saturating_sub(1),
            count: 1,
        }
    }
    /// 默认数量（!p/!r 默认为 1，!ps/!rs 默认为 20，!s 默认为 1，!ss 默认为 0）
    pub fn default_count(summary: bool) -> Self {
        if summary {
            Self {
                offset: 0,
                count: 20,
            }
        } else {
            Self {
                offset: 0,
                count: 1,
            }
        }
    }
    /// !ss 用：取全部
    pub fn all() -> Self {
        Self {
            offset: 0,
            count: 0,
        }
    }
    /// 是否取全部
    pub fn is_all(&self) -> bool {
        self.count == 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionOp {
    Eq,  // =, ≈
    XEq, // ==, ≌
    Ne,  // !=, <>, ≠
    Gt,  // >
    Ge,  // >=, ≥
    Lt,  // <
    Le,  // <=, ≤
}

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub field: String,
    pub operator: ConditionOp,
    pub value: String,
}

/// Parsed command from user input, with all extracted parameters.
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
        range: ScoreRange,
        is_all: bool,
    },
    Pass {
        mode: GameMode,
        username: Option<String>,
        qq: Option<i64>,
        range: ScoreRange,
        is_summary: bool,
        filters: Vec<Condition>,
    },
    Recent {
        mode: GameMode,
        username: Option<String>,
        qq: Option<i64>,
        range: ScoreRange,
        is_summary: bool,
        filters: Vec<Condition>,
    },
}

impl Command {
    /// Returns the [`CommandGroup`] this command belongs to.
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

    /// Returns the canonical command trigger string (e.g. `"~"`, `"where"`, `"绑定"`).
    pub fn command_name(&self) -> &'static str {
        match self {
            Command::QuerySelf { .. } => "~",
            Command::QueryUser { .. } => "where",
            Command::QueryMentionedUser { .. } => "where",
            Command::Bind { .. } => "绑定",
            Command::Unbind => "解绑",
            Command::Highlight { .. } => "今日高光",
            Command::ProfileCard { .. } => "!profile",
            Command::Pass { .. } => "!p",
            Command::Recent { .. } => "!r",
            Command::ScoreOnBeatmap { .. } => "!s",
        }
    }
}

/// Delta between two snapshots of a user's statistics.
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
    /// Returns true if any field has a non-zero change.
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

/// Activity level classification used by the scheduler to determine update intervals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UserActivity {
    SemiActive,
    Normal,
    NoRecent,
    Inactive,
    UserNotExists,
}

/// Result of a scheduled update operation, carrying the user's new activity level.
#[derive(Debug, Clone)]
pub struct UpdateResult {
    pub activity: UserActivity,
    pub success: bool,
}
