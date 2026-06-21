use serde::Deserialize;
use std::fmt;

#[derive(Debug, Clone)]
pub struct TickRegistration {
    pub plugin_idx: usize,
    pub name: String,
    pub interval_secs: u64,
    pub tick_id: u32,
}

pub struct OldPluginEntry {
    pub idx: usize,
    pub wasm_path: String,
    pub priority: u32,
    pub plugin_config: Option<serde_json::Value>,
    pub wasm_mtime: Option<std::time::SystemTime>,
}

#[derive(Debug, Deserialize)]
pub enum PluginAction {
    Handled(String),
    Next,
    Intercepted,
}

#[derive(Debug, Deserialize)]
/// 注意：此结构体与 osubot-plugin-sdk/src/types.rs 中的 PluginMetadata 独立定义。
/// SDK 端使用 `&'static str`，本端使用 `String`（序列化后通过 JSON 传递）。
/// 修改字段时需要两边同步更新。
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub commands: Vec<String>,
}

#[derive(Debug)]
#[non_exhaustive]
pub enum PluginError {
    Load(String),
    Dispatch(String),
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginError::Load(e) => write!(f, "Plugin load error: {e}"),
            PluginError::Dispatch(e) => write!(f, "Plugin dispatch error: {e}"),
        }
    }
}

impl std::error::Error for PluginError {}
