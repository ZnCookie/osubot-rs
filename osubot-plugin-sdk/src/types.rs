use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PluginMetadata {
    pub name: &'static str,
    pub version: &'static str,
    pub author: &'static str,
    pub description: &'static str,
    pub commands: Vec<&'static str>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PluginAction {
    Handled(String),
    Next,
    Intercepted,
}

pub struct PluginContext;

#[derive(Debug, Serialize, Deserialize)]
pub struct QQMessage {
    pub group_id: i64,
    pub user_id: i64,
    pub message: String,
    pub mentioned_user_id: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Command {
    pub command_type: String,
    pub group_id: Option<i64>,
    pub user_id: Option<i64>,
    pub message: Option<String>,
    pub mode: Option<u8>,
    pub username: Option<String>,
    pub qq: Option<i64>,
    pub beatmap_id: Option<u32>,
    pub score_id: Option<u64>,
    pub mods: Option<Vec<String>>,
    pub limit: Option<u32>,
    pub mentioned_user_id: Option<i64>,
}
