use serde::{Deserialize, Serialize};

use osubot_game_mode::GameMode;

/// Metadata exported by every plugin via `PLUGIN_METADATA` static.
///
/// The host reads this struct before loading the plugin to determine
/// which commands the plugin handles and to display plugin info.
#[derive(Debug, Serialize, Deserialize)]
pub struct PluginMetadata {
    /// Human-readable plugin name.
    pub name: String,
    /// Plugin version string (e.g. "1.0.0").
    pub version: String,
    /// Plugin author (e.g. "YourName").
    pub author: String,
    /// Short description of what the plugin does.
    pub description: String,
    /// Command names this plugin handles (with `!` prefix, e.g. `"!ping"`).
    /// The host dispatches matching commands to this plugin.
    pub commands: Vec<String>,
}

/// Return value from `on_message` / `on_command` lifecycle hooks.
///
/// Controls how the host should proceed after the plugin processes
/// a message or command.
#[derive(Debug, Serialize, Deserialize)]
pub enum PluginAction {
    /// Plugin produced a response; the host should send this text
    /// to the group and stop dispatching.
    Handled(String),
    /// Plugin chose not to handle this event; the host should try
    /// the next plugin (or fall through to the default handler).
    Next,
    /// Plugin wants to stop dispatching without sending a response
    /// (e.g. the message is spam or should be silently dropped).
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

/// Incoming group message received by the bot, passed to plugin lifecycle hooks.
#[derive(Debug, Serialize, Deserialize)]
pub struct QQMessage {
    /// Group chat ID where the message was sent.
    pub group_id: i64,
    /// QQ user ID of the sender.
    pub user_id: i64,
    /// Raw message text.
    pub message: String,
    /// If the message contained an @mention, the QQ ID of the mentioned user.
    pub mentioned_user_id: Option<i64>,
}

/// Parsed command dispatched from a group message, passed to `on_command`.
///
/// The host parses raw messages into this struct before calling plugins,
/// so plugin authors get structured data without manual parsing.
#[derive(Debug, Serialize, Deserialize)]
pub struct Command {
    /// Command type string (e.g. "!p", "!profile", "~").
    pub command_type: String,
    /// Group ID where the command was sent, if applicable.
    pub group_id: Option<i64>,
    /// QQ user ID of the command issuer.
    pub user_id: Option<i64>,
    /// The message text associated with this command.
    pub message: Option<String>,
    /// Game mode resolved for this command.
    ///
    /// 行为：
    /// - 对于模式敏感命令（`~` / `where` / `!p` / `!r` / `!s` / `!ps` / `!rs` / `!ss` / `今日高光`）：取用户在命令中显式指定的模式；若未指定，则取该用户的默认模式，未设置时回退到 `Osu`。最终永远为 `Some(...)`。
    /// - 对于其他命令（`绑定` / `解绑` / `!profile` / `!help` / `!mode` / `!rv`）：`None`，不代表任何用户输入。
    ///
    /// 如需区分"用户显式指定 vs 默认回退"，请通过 `command_type` 自行判断，或仅在模式敏感命令中读取 `mode`。
    pub mode: Option<GameMode>,
    /// osu! username found in the command, if any.
    pub username: Option<String>,
    /// QQ ID referenced in the command (`@` mention or `qq=` prefix).
    pub qq: Option<i64>,
    /// Beatmap ID extracted from score commands (`!s`, `!ss`).
    pub beatmap_id: Option<u32>,
    /// Score ID extracted from `!s <score_id>` syntax.
    pub score_id: Option<u64>,
    /// Filter conditions (e.g. `["mod=HDDT", "miss=1"]`) from score commands.
    /// Replaces the legacy `mods` field in the host payload.
    pub filters: Option<Vec<String>>,
    /// Pagination limit (`#N`) from score list commands.
    pub limit: Option<u32>,
    /// Pagination range end for `#N-M` syntax (e.g. `#2-10` yields `limit=2, limit_end=Some(10)`)
    pub limit_end: Option<u32>,
    /// Mentioned user ID from `@` mentions in the command.
    pub mentioned_user_id: Option<i64>,
}
