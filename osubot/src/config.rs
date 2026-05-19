#![deny(clippy::all)]
#![allow(clippy::derive_partial_eq_without_eq, reason = "第三方库 derive 需要")]

use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub osu: OsuConfig,
    pub bot: BotConfig,
    pub database: DatabaseConfig,
    pub scheduler: SchedulerConfig,
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
    #[serde(default = "default_active_interval_hours")]
    pub active_interval_hours: i64,
    #[serde(default = "default_semi_active_interval_hours")]
    pub semi_active_interval_hours: i64,
    #[serde(default = "default_normal_interval_hours")]
    pub normal_interval_hours: i64,
    #[serde(default = "default_inactive_interval_hours")]
    pub inactive_interval_hours: i64,
    #[serde(default = "default_group_trigger_cooldown_hours")]
    pub group_trigger_cooldown_hours: i64,
    #[serde(default = "default_retention_days")]
    pub retention_days: i64,
}

fn default_interval_minutes() -> u64 {
    1
}
fn default_active_interval_hours() -> i64 {
    2
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
fn default_group_trigger_cooldown_hours() -> i64 {
    1
}
fn default_retention_days() -> i64 {
    180
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            interval_minutes: 1,
            active_interval_hours: 2,
            semi_active_interval_hours: 4,
            normal_interval_hours: 8,
            inactive_interval_hours: 48,
            group_trigger_cooldown_hours: 1,
            retention_days: 180,
        }
    }
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
        }
    }
}
