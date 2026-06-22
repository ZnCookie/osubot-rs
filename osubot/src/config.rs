use osubot_core::types::CommandGroup;
use osubot_plugin::config::PluginConfig;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(toml::de::Error),
    Validation(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(e) => write!(f, "config IO error: {e}"),
            ConfigError::Parse(e) => write!(f, "config parse error: {e}"),
            ConfigError::Validation(s) => write!(f, "config validation: {s}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io(e) => Some(e),
            ConfigError::Parse(e) => Some(e),
            ConfigError::Validation(_) => None,
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Parse(e)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub osu: OsuConfig,
    pub bot: BotConfig,
    pub database: DatabaseConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub irc: IrcConfig,
    #[serde(default)]
    pub group_filter: GroupFilterConfig,
    #[serde(default)]
    pub groups: GroupsConfig,
    #[serde(default)]
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
}

#[derive(Deserialize, Clone)]
pub struct OsuConfig {
    #[serde(alias = "api_key")]
    // NOTE: stored as plain String. Debug is manually redacted (see Debug impl below).
    // Not zeroized: the value also lives as plain String in OauthTokenCache and is
    // cloned at startup, so zeroize::Zeroizing here would give incomplete coverage.
    pub client_secret: String,
    pub client_id: String,
}

impl std::fmt::Debug for OsuConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OsuConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct BotConfig {
    pub onebot_url: String,
    /// 命令处理超时（秒），默认 120
    #[serde(default = "default_command_timeout_secs")]
    pub command_timeout_secs: u64,
    /// 渲染超时（秒），默认 60
    #[serde(default = "default_render_timeout_secs")]
    pub render_timeout_secs: u64,
    /// OneBot API 请求超时（秒），默认 5
    #[serde(default = "default_onebot_api_timeout_secs")]
    pub onebot_api_timeout_secs: u64,
    /// UR 计算超时（秒），默认 10
    #[serde(default = "default_ur_timeout_secs")]
    pub ur_timeout_secs: u64,
}

fn default_command_timeout_secs() -> u64 {
    120
}
fn default_render_timeout_secs() -> u64 {
    60
}
fn default_onebot_api_timeout_secs() -> u64 {
    5
}
fn default_ur_timeout_secs() -> u64 {
    10
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub path: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SchedulerConfig {
    #[serde(default = "default_interval_minutes")]
    pub interval_minutes: u64,
    #[serde(default = "default_semi_active_interval_hours")]
    pub semi_active_interval_hours: i64,
    #[serde(default = "default_normal_interval_hours")]
    pub normal_interval_hours: i64,
    #[serde(default = "default_inactive_interval_hours")]
    pub inactive_interval_hours: i64,
    #[serde(default = "default_no_recent_interval_hours")]
    pub no_recent_interval_hours: i64,
    #[serde(default = "default_user_not_exists_interval_hours")]
    pub user_not_exists_interval_hours: i64,
    #[serde(default = "default_group_trigger_cooldown_hours")]
    pub group_trigger_cooldown_hours: i64,
    #[serde(default = "default_retention_days")]
    pub retention_days: u64,
    #[serde(default = "default_cache_retention_days")]
    pub cache_retention_days: u64,
}

fn default_interval_minutes() -> u64 {
    1
}
fn default_semi_active_interval_hours() -> i64 {
    4
}
fn default_normal_interval_hours() -> i64 {
    8
}
fn default_inactive_interval_hours() -> i64 {
    48
}
fn default_no_recent_interval_hours() -> i64 {
    6
}
fn default_user_not_exists_interval_hours() -> i64 {
    24
}
fn default_group_trigger_cooldown_hours() -> i64 {
    1
}
fn default_retention_days() -> u64 {
    180
}
fn default_cache_retention_days() -> u64 {
    7
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            interval_minutes: 1,
            semi_active_interval_hours: 4,
            normal_interval_hours: 8,
            inactive_interval_hours: 48,
            no_recent_interval_hours: 6,
            user_not_exists_interval_hours: 24,
            group_trigger_cooldown_hours: 1,
            retention_days: 180,
            cache_retention_days: 7,
        }
    }
}

#[derive(Deserialize, Clone)]
pub struct IrcConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_irc_server")]
    pub server: String,
    #[serde(default = "default_irc_port")]
    pub port: u16,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub password: String,
}

impl std::fmt::Debug for IrcConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrcConfig")
            .field("enabled", &self.enabled)
            .field("server", &self.server)
            .field("port", &self.port)
            .field("nickname", &self.nickname)
            .field("password", &"<redacted>")
            .finish()
    }
}

fn default_irc_server() -> String {
    "irc.ppy.sh".to_string()
}

fn default_irc_port() -> u16 {
    6667
}

impl Default for IrcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: default_irc_server(),
            port: default_irc_port(),
            nickname: String::new(),
            password: String::new(),
        }
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    Blacklist,
    Whitelist,
}

/// 群聊黑白名单配置
#[derive(Debug, Deserialize, Clone)]
pub struct GroupFilterConfig {
    #[serde(default = "default_filter_mode")]
    pub mode: FilterMode,
    #[serde(default)]
    pub group_ids: Vec<i64>,
}

fn default_filter_mode() -> FilterMode {
    FilterMode::Blacklist
}

impl Default for GroupFilterConfig {
    fn default() -> Self {
        Self {
            mode: default_filter_mode(),
            group_ids: Vec::new(),
        }
    }
}

impl GroupFilterConfig {
    pub fn is_group_allowed(&self, group_id: i64) -> bool {
        match self.mode {
            FilterMode::Whitelist => self.group_ids.contains(&group_id),
            FilterMode::Blacklist => !self.group_ids.contains(&group_id),
        }
    }
}

/// 单个群的命令开关配置
#[derive(Debug, Deserialize, Clone, Default)]
pub struct GroupConfig {
    pub query: Option<bool>,
    pub score: Option<bool>,
    pub beatmap_preview: Option<bool>,
    pub profile: Option<bool>,
    pub highlight: Option<bool>,
    pub bind: Option<bool>,
    pub mode: Option<bool>,
    pub help: Option<bool>,
}

impl GroupConfig {
    pub fn is_enabled(&self, group_name: CommandGroup) -> bool {
        let default = true;
        match group_name {
            CommandGroup::Query => self.query.unwrap_or(default),
            CommandGroup::Score => self.score.unwrap_or(default),
            CommandGroup::BeatmapPreview => self.beatmap_preview.unwrap_or(default),
            CommandGroup::Profile => self.profile.unwrap_or(default),
            CommandGroup::Highlight => self.highlight.unwrap_or(default),
            CommandGroup::Bind => self.bind.unwrap_or(default),
            CommandGroup::Mode => self.mode.unwrap_or(default),
            CommandGroup::Help => self.help.unwrap_or(default),
        }
    }
}

/// 命令开关配置（default + 每群覆盖）
#[derive(Debug, Deserialize, Clone, Default)]
pub struct GroupsConfig {
    #[serde(default)]
    pub default: GroupConfig,
    #[serde(flatten)]
    pub overrides: HashMap<String, GroupConfig>,
}

impl GroupsConfig {
    pub fn get_group_config(&self, group_id: i64) -> GroupConfig {
        let key = group_id.to_string();
        if let Some(override_cfg) = self.overrides.get(&key) {
            GroupConfig {
                query: override_cfg.query.or(self.default.query),
                score: override_cfg.score.or(self.default.score),
                beatmap_preview: override_cfg
                    .beatmap_preview
                    .or(self.default.beatmap_preview),
                profile: override_cfg.profile.or(self.default.profile),
                highlight: override_cfg.highlight.or(self.default.highlight),
                bind: override_cfg.bind.or(self.default.bind),
                mode: override_cfg.mode.or(self.default.mode),
                help: override_cfg.help.or(self.default.help),
            }
        } else {
            self.default.clone()
        }
    }
}

pub(crate) fn default_upstream_url() -> String {
    "wss://public-service.b11p.com/".to_string()
}

fn default_timeout_secs() -> u64 {
    10
}

fn default_rate_per_minute() -> u32 {
    10
}

fn default_burst() -> u32 {
    20
}

#[derive(Deserialize, Clone)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(default = "default_rate_per_minute")]
    pub rate_per_minute: u32,
    #[serde(default = "default_burst")]
    pub burst: u32,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub self_id: Option<i64>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("provider_type", &self.provider_type)
            .field("rate_per_minute", &self.rate_per_minute)
            .field("burst", &self.burst)
            .field("url", &self.url)
            .field("access_token", &"<redacted>")
            .field("self_id", &self.self_id)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct UpstreamConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

/// 热重载时会从新 TOML 解析的部分，遗留字段（database）保持旧值不变。
/// osu/irc/bot 用 Option 区分"未写 section"（None，继承旧值）和"写了 section"（Some，使用新值）。
/// scheduler 也用 Option 避免无 section 时用默认值覆盖用户配置。
/// 每新增一个可重载字段，或新增遗留不可变字段时，需同步更新 reload.rs 中的构造。
#[derive(Debug, Deserialize, Clone)]
pub struct MutableConfig {
    #[serde(default)]
    pub osu: Option<OsuConfig>,
    #[serde(default)]
    pub scheduler: Option<SchedulerConfig>,
    #[serde(default)]
    pub group_filter: GroupFilterConfig,
    #[serde(default)]
    pub groups: GroupsConfig,
    #[serde(default)]
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
    #[serde(default)]
    pub bot: Option<BotConfig>,
    #[serde(default)]
    pub irc: Option<IrcConfig>,
}

impl Config {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.bot.onebot_url.is_empty() {
            return Err(ConfigError::Validation("onebot_url 为空".into()));
        }
        if self.bot.command_timeout_secs < 5 {
            return Err(ConfigError::Validation(format!(
                "command_timeout_secs 过小（{} < 5 秒）",
                self.bot.command_timeout_secs
            )));
        }
        if self.bot.render_timeout_secs < 5 {
            return Err(ConfigError::Validation(format!(
                "render_timeout_secs 过小（{} < 5 秒）",
                self.bot.render_timeout_secs
            )));
        }
        if self.bot.onebot_api_timeout_secs < 2 {
            return Err(ConfigError::Validation(format!(
                "onebot_api_timeout_secs 过小（{} < 2 秒）",
                self.bot.onebot_api_timeout_secs
            )));
        }
        if self.bot.ur_timeout_secs < 3 {
            return Err(ConfigError::Validation(format!(
                "ur_timeout_secs 过小（{} < 3 秒）",
                self.bot.ur_timeout_secs
            )));
        }
        if self.bot.command_timeout_secs > 3600 {
            return Err(ConfigError::Validation(format!(
                "command_timeout_secs 过大（{} > 3600 秒）",
                self.bot.command_timeout_secs
            )));
        }
        if self.bot.render_timeout_secs > 600 {
            return Err(ConfigError::Validation(format!(
                "render_timeout_secs 过大（{} > 600 秒）",
                self.bot.render_timeout_secs
            )));
        }
        if self.bot.onebot_api_timeout_secs > 120 {
            return Err(ConfigError::Validation(format!(
                "onebot_api_timeout_secs 过大（{} > 120 秒）",
                self.bot.onebot_api_timeout_secs
            )));
        }
        if self.bot.ur_timeout_secs > 300 {
            return Err(ConfigError::Validation(format!(
                "ur_timeout_secs 过大（{} > 300 秒）",
                self.bot.ur_timeout_secs
            )));
        }
        if self.scheduler.interval_minutes > 1440 {
            return Err(ConfigError::Validation(format!(
                "scheduler.interval_minutes 过大（{} > 1440 分钟 = 1 天）",
                self.scheduler.interval_minutes
            )));
        }
        // 5 个 i64 间隔字段下界检查：负值会让 Duration::hours(负) 行为异常
        for (name, val) in [
            (
                "semi_active_interval_hours",
                self.scheduler.semi_active_interval_hours,
            ),
            (
                "normal_interval_hours",
                self.scheduler.normal_interval_hours,
            ),
            (
                "inactive_interval_hours",
                self.scheduler.inactive_interval_hours,
            ),
            (
                "no_recent_interval_hours",
                self.scheduler.no_recent_interval_hours,
            ),
            (
                "user_not_exists_interval_hours",
                self.scheduler.user_not_exists_interval_hours,
            ),
        ] {
            if val < 0 {
                return Err(ConfigError::Validation(format!(
                    "scheduler.{name} 不能为负（{val}）"
                )));
            }
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            osu: OsuConfig {
                client_secret: std::env::var("OSU_CLIENT_SECRET")
                    .or_else(|_| std::env::var("OSU_API_KEY"))
                    .unwrap_or_default(),
                client_id: std::env::var("OSU_CLIENT_ID").unwrap_or_default(),
            },
            bot: BotConfig {
                onebot_url: std::env::var("ONEBOT_URL")
                    .unwrap_or_else(|_| "ws://127.0.0.1:8080".to_string()),
                command_timeout_secs: 120,
                render_timeout_secs: 60,
                onebot_api_timeout_secs: 5,
                ur_timeout_secs: 10,
            },
            database: DatabaseConfig {
                path: std::env::var("DATABASE_PATH").unwrap_or_else(|_| "osubot.db".to_string()),
            },
            scheduler: SchedulerConfig::default(),
            irc: IrcConfig::default(),
            group_filter: GroupFilterConfig::default(),
            groups: GroupsConfig::default(),
            upstream: UpstreamConfig::default(),
            plugin: PluginConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_filter_default() {
        let cfg = GroupFilterConfig::default();
        assert_eq!(cfg.mode, FilterMode::Blacklist);
        assert!(cfg.group_ids.is_empty());
        assert!(cfg.is_group_allowed(123456));
    }

    #[test]
    fn test_group_filter_blacklist() {
        let cfg = GroupFilterConfig {
            mode: FilterMode::Blacklist,
            group_ids: vec![111, 222],
        };
        assert!(!cfg.is_group_allowed(111));
        assert!(!cfg.is_group_allowed(222));
        assert!(cfg.is_group_allowed(333));
    }

    #[test]
    fn test_group_filter_whitelist() {
        let cfg = GroupFilterConfig {
            mode: FilterMode::Whitelist,
            group_ids: vec![111, 222],
        };
        assert!(cfg.is_group_allowed(111));
        assert!(cfg.is_group_allowed(222));
        assert!(!cfg.is_group_allowed(333));
    }

    #[test]
    fn test_group_config_is_enabled_default_true() {
        let cfg = GroupConfig::default();
        assert!(cfg.is_enabled(CommandGroup::Query));
        assert!(cfg.is_enabled(CommandGroup::Score));
        assert!(cfg.is_enabled(CommandGroup::BeatmapPreview));
        assert!(cfg.is_enabled(CommandGroup::Profile));
        assert!(cfg.is_enabled(CommandGroup::Highlight));
        assert!(cfg.is_enabled(CommandGroup::Bind));
        assert!(cfg.is_enabled(CommandGroup::Mode));
        assert!(cfg.is_enabled(CommandGroup::Help));
    }

    #[test]
    fn test_group_config_is_enabled_disabled() {
        let cfg = GroupConfig {
            query: Some(true),
            score: Some(false),
            beatmap_preview: None,
            profile: None,
            highlight: Some(false),
            bind: None,
            mode: None,
            help: None,
        };
        assert!(cfg.is_enabled(CommandGroup::Query));
        assert!(!cfg.is_enabled(CommandGroup::Score));
        assert!(cfg.is_enabled(CommandGroup::Profile));
        assert!(!cfg.is_enabled(CommandGroup::Highlight));
        assert!(cfg.is_enabled(CommandGroup::Bind));
        assert!(cfg.is_enabled(CommandGroup::Mode));
        assert!(cfg.is_enabled(CommandGroup::Help));
    }

    #[test]
    fn test_groups_config_get_default() {
        let cfg = GroupsConfig::default();
        let group = cfg.get_group_config(999);
        assert!(group.is_enabled(CommandGroup::Query));
        assert!(group.is_enabled(CommandGroup::Score));
    }

    #[test]
    fn test_groups_config_override() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "123".to_string(),
            GroupConfig {
                highlight: Some(false),
                bind: Some(false),
                ..Default::default()
            },
        );
        let cfg = GroupsConfig {
            default: GroupConfig {
                query: Some(true),
                score: Some(false),
                ..Default::default()
            },
            overrides,
        };

        let g123 = cfg.get_group_config(123);
        assert!(g123.is_enabled(CommandGroup::Query));
        assert!(!g123.is_enabled(CommandGroup::Score));
        assert!(g123.is_enabled(CommandGroup::Profile));
        assert!(!g123.is_enabled(CommandGroup::Highlight));
        assert!(!g123.is_enabled(CommandGroup::Bind));
        assert!(g123.is_enabled(CommandGroup::Mode));

        let g999 = cfg.get_group_config(999);
        assert!(g999.is_enabled(CommandGroup::Query));
        assert!(!g999.is_enabled(CommandGroup::Score));
        assert!(g999.is_enabled(CommandGroup::Highlight));
        assert!(g999.is_enabled(CommandGroup::Mode));
    }

    #[test]
    fn test_config_from_toml_missing_groups() {
        let toml_str = r#"
            [osu]
            api_key = "test"
            client_id = "test"
            [bot]
            onebot_url = "ws://localhost"
            [database]
            path = "test.db"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.group_filter.mode, FilterMode::Blacklist);
        assert!(config.group_filter.group_ids.is_empty());
        assert!(config.groups.default.query.is_none());
        assert!(config.groups.overrides.is_empty());
    }

    #[test]
    fn test_config_from_toml_with_groups() {
        let toml_str = r#"
            [osu]
            api_key = "test"
            client_id = "test"
            [bot]
            onebot_url = "ws://localhost"
            [database]
            path = "test.db"

            [group_filter]
            mode = "whitelist"
            group_ids = [111, 222]

            [groups.default]
            highlight = false

            [groups.111]
            highlight = true
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.group_filter.mode, FilterMode::Whitelist);
        assert_eq!(config.group_filter.group_ids, vec![111, 222]);
        assert_eq!(config.groups.default.highlight, Some(false));
        assert_eq!(config.groups.overrides["111"].highlight, Some(true));

        let g111 = config.groups.get_group_config(111);
        assert!(g111.is_enabled(CommandGroup::Highlight));
        let g222 = config.groups.get_group_config(222);
        assert!(!g222.is_enabled(CommandGroup::Highlight));
    }

    #[test]
    fn test_config_from_toml_with_upstream() {
        let toml_str = r#"
            [osu]
            api_key = "test"
            client_id = "test"
            [bot]
            onebot_url = "ws://localhost"
            [database]
            path = "test.db"
            [upstream]
            enabled = true
            [[upstream.providers]]
            type = "xfs"
            [[upstream.providers]]
            type = "yumu"
            url = "ws://custom-yumu:1234"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        let upstream = config.upstream;
        assert!(upstream.enabled);
        assert_eq!(upstream.providers.len(), 2);
        assert_eq!(upstream.providers[0].provider_type, "xfs");
        assert_eq!(upstream.providers[0].url, None);
        assert_eq!(upstream.providers[1].provider_type, "yumu");
        assert_eq!(
            upstream.providers[1].url,
            Some("ws://custom-yumu:1234".into())
        );
    }

    #[test]
    fn test_group_filter_invalid_mode_fails() {
        let toml_str = r#"
            [osu]
            api_key = "test"
            client_id = "test"
            [bot]
            onebot_url = "ws://localhost"
            [database]
            path = "test.db"
            [group_filter]
            mode = "typo"
            group_ids = [111]
        "#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_group_filter_uppercase_whitelist_fails() {
        let toml_str = r#"
            [osu]
            api_key = "test"
            client_id = "test"
            [bot]
            onebot_url = "ws://localhost"
            [database]
            path = "test.db"
            [group_filter]
            mode = "Whitelist"
            group_ids = [111]
        "#;
        let result: Result<Config, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_accepts_client_secret_canonical_name() {
        let toml_str = r#"
            [osu]
            client_secret = "my-secret"
            client_id = "my-id"
            [bot]
            onebot_url = "ws://localhost"
            [database]
            path = "test.db"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.osu.client_secret, "my-secret");
        assert_eq!(config.osu.client_id, "my-id");
    }

    #[test]
    fn test_config_accepts_api_key_as_alias() {
        let toml_str = r#"
            [osu]
            api_key = "old-api-key"
            client_id = "my-id"
            [bot]
            onebot_url = "ws://localhost"
            [database]
            path = "test.db"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.osu.client_secret, "old-api-key");
    }

    #[test]
    fn test_debug_does_not_leak_client_secret() {
        let osu = OsuConfig {
            client_secret: "super-secret-key".into(),
            client_id: "12345".into(),
        };
        let s = format!("{:?}", osu);
        assert!(!s.contains("super-secret-key"));
        assert!(s.contains("<redacted>"));
    }

    #[test]
    fn test_debug_does_not_leak_irc_password() {
        let irc = IrcConfig {
            enabled: false,
            server: "irc.ppy.sh".into(),
            port: 6667,
            nickname: "test".into(),
            password: "super-secret-pass".into(),
        };
        let s = format!("{:?}", irc);
        assert!(!s.contains("super-secret-pass"));
        assert!(s.contains("<redacted>"));
    }

    #[test]
    fn test_debug_does_not_leak_access_token() {
        let provider = ProviderConfig {
            provider_type: "xfa".into(),
            rate_per_minute: 10,
            burst: 20,
            url: None,
            access_token: Some("secret-token".into()),
            self_id: None,
            timeout_secs: 10,
        };
        let s = format!("{:?}", provider);
        assert!(!s.contains("secret-token"));
        assert!(s.contains("<redacted>"));
    }
}
