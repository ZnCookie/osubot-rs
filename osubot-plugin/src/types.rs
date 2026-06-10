use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub enum PluginAction {
    Handled(String),
    Next,
    Intercepted,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
