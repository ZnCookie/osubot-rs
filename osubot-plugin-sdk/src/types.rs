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

/// 插件上下文，提供宿主函数调用入口。
///
/// 在 WASM 环境中，`PluginContext` 设计为单元结构体，通过 `static CTX` 全局访问，
/// 而不是以参数形式传入每个函数。这是因为：
///
/// - WASM 实例始终在单线程中运行（wasmtime 保证），无需 Send/Sync 同步
/// - 宿主（osubot）通过 wasmtime 的 `Caller` 上下文自动将调用路由到正确的插件实例，
///   插件侧无需感知实例 ID
/// - 消除所有函数签名的上下文参数，降低 FFI 边界的心智负担
///
/// 这仅在 WASM 单实例单线程模型下成立。若未来支持多线程 WASM，需重新评估此设计。
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
