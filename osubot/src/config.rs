use osubot_core::types::CommandGroup;
use osubot_plugin::config::PluginConfig;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

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

#[derive(Debug, Deserialize, Clone)]
pub struct OsuConfig {
    pub api_key: String,
    pub client_id: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct BotConfig {
    pub onebot_url: String,
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

#[derive(Debug, Deserialize, Clone)]
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
    pub profile: Option<bool>,
    pub highlight: Option<bool>,
    pub bind: Option<bool>,
}

impl GroupConfig {
    pub fn is_enabled(&self, group_name: CommandGroup) -> bool {
        let default = true;
        match group_name {
            CommandGroup::Query => self.query.unwrap_or(default),
            CommandGroup::Score => self.score.unwrap_or(default),
            CommandGroup::Profile => self.profile.unwrap_or(default),
            CommandGroup::Highlight => self.highlight.unwrap_or(default),
            CommandGroup::Bind => self.bind.unwrap_or(default),
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
                profile: override_cfg.profile.or(self.default.profile),
                highlight: override_cfg.highlight.or(self.default.highlight),
                bind: override_cfg.bind.or(self.default.bind),
            }
        } else {
            self.default.clone()
        }
    }
}

pub(crate) fn default_upstream_url() -> String {
    "wss://public-service.b11p.com/".to_string()
}

fn default_access_token() -> String {
    "bleatingsheep.org".to_string()
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

#[derive(Debug, Deserialize, Clone)]
pub struct ProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: String,
    #[serde(default = "default_rate_per_minute")]
    pub rate_per_minute: u32,
    #[serde(default = "default_burst")]
    pub burst: u32,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default = "default_access_token")]
    pub access_token: String,
    #[serde(default)]
    pub self_id: Option<i64>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct UpstreamConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

/// 热重载时会从新 TOML 解析的部分，遗留字段（osu/bot/database/irc）保持旧值不变。
/// 每新增一个可重载字段，或新增遗留不可变字段时，需同步更新 reload.rs 中的构造。
#[derive(Debug, Deserialize, Clone)]
pub struct MutableConfig {
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub group_filter: GroupFilterConfig,
    #[serde(default)]
    pub groups: GroupsConfig,
    #[serde(default)]
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub plugin: PluginConfig,
}

impl Config {
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let content = fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            osu: OsuConfig {
                api_key: std::env::var("OSU_API_KEY").unwrap_or_default(),
                client_id: std::env::var("OSU_CLIENT_ID").unwrap_or_default(),
            },
            bot: BotConfig {
                onebot_url: std::env::var("ONEBOT_URL")
                    .unwrap_or_else(|_| "ws://127.0.0.1:8080".to_string()),
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
        assert!(cfg.is_enabled(CommandGroup::Profile));
        assert!(cfg.is_enabled(CommandGroup::Highlight));
        assert!(cfg.is_enabled(CommandGroup::Bind));
    }

    #[test]
    fn test_group_config_is_enabled_disabled() {
        let cfg = GroupConfig {
            query: Some(true),
            score: Some(false),
            profile: None,
            highlight: Some(false),
            bind: None,
        };
        assert!(cfg.is_enabled(CommandGroup::Query));
        assert!(!cfg.is_enabled(CommandGroup::Score));
        assert!(cfg.is_enabled(CommandGroup::Profile));
        assert!(!cfg.is_enabled(CommandGroup::Highlight));
        assert!(cfg.is_enabled(CommandGroup::Bind));
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

        let g999 = cfg.get_group_config(999);
        assert!(g999.is_enabled(CommandGroup::Query));
        assert!(!g999.is_enabled(CommandGroup::Score));
        assert!(g999.is_enabled(CommandGroup::Highlight));
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
}
