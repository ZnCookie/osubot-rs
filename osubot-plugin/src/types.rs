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
/// 字段集必须与 `osubot-plugin-sdk/src/types.rs::PluginMetadata` 保持一致。
/// `protocol_version` 在加载时与 `PROTOCOL_VERSION` 对比校验。
pub struct PluginMetadata {
    pub protocol_version: u32,
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
