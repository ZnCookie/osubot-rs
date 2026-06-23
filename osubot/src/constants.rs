// 超时常量已全部迁移到 `osubot.toml` 的 `[bot]` 段配置。
// 命令/渲染/UR/OneBot API 超时均可在配置中调整。

/// WebSocket 保活 ping 间隔（秒）
pub const PING_INTERVAL_SECS: u64 = 30;
/// 重连时等待 tick loop 完成的超时（秒）
pub const TICK_HANDLE_SHUTDOWN_SECS: u64 = 3;
