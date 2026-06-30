pub use osubot_game_mode::GameMode;
pub use osubot_types::{format_length, format_play_datetime, Score, ScoreStatistics, ScoreUser};

/// Actions for the `!ml` (Match Listen) command.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchListenAction {
    Start { match_id: u64, skip_rounds: u32 },
    Stop { match_id: u64 },
    StopAll,
    List,
    Status { match_id: u64 },
}

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
    pub avatar_url: Option<String>,
}

/// Group category of a command, used for per-group enable/disable in config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandGroup {
    Query,
    Score,
    BeatmapPreview,
    BeatmapAudio,
    Profile,
    Highlight,
    Bind,
    Mode,
    Help,
    MatchListen,
}

/// osu! server variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Server {
    Official,
    PpsySb,
}

/// Parsed command from user input, with all extracted parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    QuerySelf {
        mode: Option<GameMode>,
        server: Server,
    },
    QueryUser {
        username: String,
        mode: Option<GameMode>,
        server: Server,
    },
    QueryMentionedUser {
        qq: i64,
        mode: Option<GameMode>,
        server: Server,
    },
    Bind {
        username: String,
        server: Server,
    },
    Unbind {
        server: Server,
    },
    Highlight {
        mode: Option<GameMode>,
        server: Server,
    },
    ProfileCard {
        username: Option<String>,
        qq: Option<i64>,
        server: Server,
    },
    ScoreOnBeatmap {
        mode: Option<GameMode>,
        username: Option<String>,
        qq: Option<i64>,
        beatmap_id: Option<u32>,
        score_id: Option<u64>,
        filters: Option<Vec<String>>,
        limit: u32,
        limit_end: Option<u32>,
        is_all: bool,
        server: Server,
    },
    Pass {
        mode: Option<GameMode>,
        username: Option<String>,
        qq: Option<i64>,
        beatmap_id: Option<u32>,
        score_id: Option<u64>,
        limit: u32,
        limit_end: Option<u32>,
        is_summary: bool,
        filters: Option<Vec<String>>,
        server: Server,
    },
    Recent {
        mode: Option<GameMode>,
        username: Option<String>,
        qq: Option<i64>,
        beatmap_id: Option<u32>,
        score_id: Option<u64>,
        limit: u32,
        limit_end: Option<u32>,
        is_summary: bool,
        filters: Option<Vec<String>>,
        server: Server,
    },
    Best {
        mode: Option<GameMode>,
        username: Option<String>,
        qq: Option<i64>,
        beatmap_id: Option<u32>,
        score_id: Option<u64>,
        limit: u32,
        limit_end: Option<u32>,
        is_summary: bool,
        filters: Option<Vec<String>>,
        server: Server,
    },
    TodayBest {
        mode: Option<GameMode>,
        username: Option<String>,
        qq: Option<i64>,
        beatmap_id: Option<u32>,
        score_id: Option<u64>,
        limit: u32,
        limit_end: Option<u32>,
        is_summary: bool,
        filters: Option<Vec<String>>,
        server: Server,
    },
    SetDefaultMode {
        mode: Option<GameMode>,
        server: Server,
    },
    BeatmapPreview {
        score_id: Option<u64>,
        beatmap_id: Option<u32>,
        mode: Option<GameMode>,
        username: Option<String>,
        qq: Option<i64>,
        mods: Option<Vec<String>>,
        gif: bool,
        times: Option<Vec<i64>>,
        limit: u32,
        filters: Option<Vec<String>>,
        explicit_position: bool,
        server: Server,
    },
    BeatmapAudio {
        mode: Option<GameMode>,
        username: Option<String>,
        qq: Option<i64>,
        beatmap_id: Option<u32>,
        score_id: Option<u64>,
        limit: u32,
        filters: Option<Vec<String>>,
        explicit_position: bool,
        server: Server,
    },
    Help,
    MatchListen(MatchListenAction),
}

impl Command {
    /// Returns the [`CommandGroup`] this command belongs to.
    pub fn group_name(&self) -> CommandGroup {
        match self {
            Command::QuerySelf { .. }
            | Command::QueryUser { .. }
            | Command::QueryMentionedUser { .. } => CommandGroup::Query,
            Command::Pass { .. }
            | Command::Recent { .. }
            | Command::Best { .. }
            | Command::TodayBest { .. } => CommandGroup::Score,
            Command::BeatmapPreview { .. } => CommandGroup::BeatmapPreview,
            Command::BeatmapAudio { .. } => CommandGroup::BeatmapAudio,
            Command::ProfileCard { .. } => CommandGroup::Profile,
            Command::ScoreOnBeatmap { .. } => CommandGroup::Score,
            Command::Highlight { .. } => CommandGroup::Highlight,
            Command::Bind { .. } | Command::Unbind { .. } => CommandGroup::Bind,
            Command::SetDefaultMode { .. } => CommandGroup::Mode,
            Command::Help => CommandGroup::Help,
            Command::MatchListen(..) => CommandGroup::MatchListen,
        }
    }

    /// Returns the canonical command trigger string (e.g. `"~"`, `"where"`, `"绑定"`).
    pub fn command_name(&self) -> &'static str {
        match self {
            Command::QuerySelf { .. } => "~",
            Command::QueryUser { .. } => "where",
            Command::QueryMentionedUser { .. } => "where",
            Command::Bind { .. } => "绑定",
            Command::Unbind { .. } => "解绑",
            Command::Highlight { .. } => "今日高光",
            Command::ProfileCard { .. } => "!profile",
            Command::Pass { .. } => "!p",
            Command::Recent { .. } => "!r",
            Command::Best { .. } => "!b",
            Command::TodayBest { .. } => "!t",
            Command::ScoreOnBeatmap { .. } => "!s",
            Command::SetDefaultMode { .. } => "!mode",
            Command::BeatmapPreview { .. } => "!rv",
            Command::BeatmapAudio { .. } => "!a",
            Command::Help => "!help",
            Command::MatchListen(..) => "!ml",
        }
    }

    /// Override the server field on any Command variant that has one.
    pub fn set_server(&mut self, target: Server) {
        match self {
            Command::QuerySelf { server, .. }
            | Command::QueryUser { server, .. }
            | Command::QueryMentionedUser { server, .. }
            | Command::Bind { server, .. }
            | Command::Unbind { server, .. }
            | Command::Highlight { server, .. }
            | Command::ProfileCard { server, .. }
            | Command::ScoreOnBeatmap { server, .. }
            | Command::Pass { server, .. }
            | Command::Recent { server, .. }
            | Command::Best { server, .. }
            | Command::TodayBest { server, .. }
            | Command::SetDefaultMode { server, .. }
            | Command::BeatmapPreview { server, .. }
            | Command::BeatmapAudio { server, .. } => *server = target,
            Command::Help | Command::MatchListen(..) => {}
        }
    }

    pub fn server(&self) -> Server {
        match self {
            Command::QuerySelf { server, .. }
            | Command::QueryUser { server, .. }
            | Command::QueryMentionedUser { server, .. }
            | Command::Bind { server, .. }
            | Command::Unbind { server, .. }
            | Command::Highlight { server, .. }
            | Command::ProfileCard { server, .. }
            | Command::ScoreOnBeatmap { server, .. }
            | Command::Pass { server, .. }
            | Command::Recent { server, .. }
            | Command::Best { server, .. }
            | Command::TodayBest { server, .. }
            | Command::SetDefaultMode { server, .. }
            | Command::BeatmapPreview { server, .. }
            | Command::BeatmapAudio { server, .. } => *server,
            Command::Help | Command::MatchListen(..) => Server::Official,
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
