# 贡献指南

## 1. 项目架构概览

### Workspace 成员

| Crate | 职责 | 依赖关系 |
|-------|------|----------|
| `osubot` | 主程序入口，消息处理、命令分发 | osubot-core, osubot-render, osubot-plugin, osubot-beatmap-preview |
| `osubot-core` | 核心业务逻辑（查询、绑定、调度） | osubot-types, osubot-game-mode |
| `osubot-types` | 共享类型定义（GameMode, Command 等） | osubot-game-mode |
| `osubot-render` | 卡片渲染（SVG→PNG/GIF） | osubot-core, osubot-types |
| `osubot-plugin-sdk` | WASM 插件开发 SDK | osubot-game-mode |
| `osubot-plugin` | 插件宿主（加载、调度、隔离） | osubot-core, osubot-game-mode, osubot-types, osubot-plugin-sdk |
| `osubot-beatmap-preview` | 谱面预览生成 | 无内部依赖 |
| `osubot-game-mode` | 游戏模式枚举 | 无内部依赖 |

### 核心数据流

```
QQ消息 → OneBot WebSocket → osubot (消息解析)
    ↓
命令分发 → 插件系统（on_command/on_message）
    ↓
osu! API 查询 → 数据处理 → 卡片渲染
    ↓
响应发送 → OneBot WebSocket → QQ群
```

## 2. 代码规范

### Rust 风格指南

遵循 [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) 和 [Clippy lints](https://rust-lang.github.io/rust-clippy/)。

### 项目特定约定

#### 命名规范

- **模块**：`snake_case`（如 `command_parser.rs`）
- **类型**：`PascalCase`（如 `GameMode`、`PluginAction`）
- **函数**：`snake_case`（如 `send_group_msg`）
- **常量**：`SCREAMING_SNAKE_CASE`（如 `PROTOCOL_VERSION`）
- **WASM 导出函数**：`snake_case`（如 `on_command`、`plugin_metadata`）

#### 错误处理

```rust
use crate::api::{http, ApiError};

// ✅ 推荐：使用 Result 传播错误
async fn query_user(username: &str) -> Result<UserStats, ApiError> {
    let url = format!("/api/v2/users/{username}");
    let resp = http::authenticated_get(&url, &rate_limiter, &oauth).await?;
    http::json_body::<UserStats>(resp).await
}

// ❌ 避免：unwrap/expect（除非确实不可能失败）
let stats = http::json_body::<UserStats>(resp).await.unwrap();
```

#### 日志使用

所有日志文本键值对定义在 `osubot-core/src/strings.rs`，使用 `log_fmt!` 宏输出：

```rust
use osubot_core::log_fmt;
use tracing::{info, warn, error, debug};

info!("{}", log_fmt!("main.bind_success", username = username));
warn!("{}", log_fmt!("api.rate_limited", retry_after = retry_after));
error!("{}", log_fmt!("db.query_failed", error = %e));
debug!("{}", log_fmt!("ws.message", len = message.len()));
```

日志键（如 `"main.bind_success"`）在 `osubot-core/src/strings.rs` 的 `LOG_STRINGS` phf map 中定义，配合 `log_fmt!` 的结构化 key=value 格式可被日志工具索引和过滤。

不要使用非结构化的内联字符串日志：

```rust
// ❌ 避免
info!("用户 {} 绑定成功", username);

// ✅ 推荐
info!("{}", log_fmt!("main.bind_success", username = username));
```

#### 注释规范

```rust
/// 查询 osu! 用户统计数据
///
/// # Arguments
/// * `username` - osu! 用户名（大小写不敏感）
/// * `mode` - 游戏模式
///
/// # Returns
/// 用户统计数据，用户不存在时返回 None
pub async fn get_user_stats(username: &str, mode: GameMode) -> Option<UserStats> {
    // 实现...
}
```

### Workspace 目录结构

项目按功能拆分为独立 crate，主要代码分布在：

```
osubot/                  # 主程序入口、消息循环、命令调度
├── src/main.rs          # 程序入口
├── src/command/         # 命令处理（handle_command）
├── src/score_query/     # 查分/试听/预览查询
├── src/onebot.rs        # OneBot 协议解析
├── src/ws_loop.rs       # WebSocket 事件循环与重连
├── src/config.rs        # 配置加载
├── src/reload.rs        # 热重载
├── src/plugin_runtime.rs# 插件运行时集成
├── src/runtime.rs       # 运行时初始化（日志、AppState）
├── src/scheduler.rs     # 后台调度（谱面解析、IRC）
├── src/background.rs    # 后台任务启动
├── src/score_filter.rs  # 评分过滤条件解析与匹配
├── src/app_state.rs     # 全局状态
├── src/constants.rs     # 常量
├── src/last_beatmap_cache.rs # 最近谱面缓存
├── src/shutdown.rs      # 优雅关闭
├── src/xfs_upstream.rs  # 消防栓上游绑定查询
└── src/yumu_upstream.rs # Yumu 上游绑定查询

osubot-core/             # 核心业务逻辑
├── src/api/             # osu! API v2 调用（OAuth/PP/谱面/成绩）
├── src/commands/        # 命令解析（parse_command）
├── src/storage.rs       # 数据库操作
├── src/ssrf.rs          # SSRF 防护
├── src/dedup.rs         # 请求去重
├── src/rate_limiter.rs  # API 限流
├── src/types.rs         # 核心类型（Command/UserStats/Score）
├── src/response.rs      # 响应格式化
├── src/strings.rs       # 用户可见文本
├── src/highlight.rs     # 今日高光
├── src/irc.rs           # IRC 绑定验证
├── src/cache.rs         # 内存缓存
├── src/upstream.rs      # 上游绑定查询链
├── src/ur.rs            # 用户等级映射
└── src/lib.rs           # crate 根

osubot-render/           # 卡片渲染（SVG→PNG/GIF）
osubot-plugin/           # WASM 插件宿主（加载/调度/热重载）
osubot-plugin-sdk/       # WASM 插件开发 SDK
osubot-types/            # 共享类型定义（Score/PpBreakdown）
osubot-game-mode/        # 游戏模式枚举
osubot-beatmap-preview/  # 谱面预览生成（谱面转 GIF）
```

### 禁止事项

- **不要** 在插件中使用 `unwrap()`/`expect()`（会 panic 导致 WASM 崩溃）
- **不要** 硬编码配置值（使用 `get_plugin_config()`）
- **不要** 在插件中直接访问文件系统（使用宿主函数）
- **不要** 提交敏感信息（API 密钥、密码等）

## 3. 测试指南

### 测试类型

#### 单元测试

在源码中编写，测试单个函数/模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_digit_str() {
        assert_eq!(GameMode::from_digit_str("0"), Some(GameMode::Osu));
        assert_eq!(GameMode::from_digit_str("1"), Some(GameMode::Taiko));
        assert_eq!(GameMode::from_digit_str("2"), Some(GameMode::Catch));
        assert_eq!(GameMode::from_digit_str("3"), Some(GameMode::Mania));
    }

    #[test]
    fn test_from_digit_str_invalid_returns_none() {
        assert_eq!(GameMode::from_digit_str("99"), None);
        assert_eq!(GameMode::from_digit_str("osu!"), None);
        assert_eq!(GameMode::from_digit_str(""), None);
    }

    // `parse_filter_token` 定义在 osubot/src/score_filter.rs 中（osubot crate 的单元测试）
    // 此处仅为示例，实际测试应放在对应的 `.rs` 文件内
    #[test]
    fn test_parse_filter_token() {
        let (key, _op, value) = parse_filter_token("pp>=200").expect("valid token should parse");
        assert_eq!(key, "pp");
        assert_eq!(value, "200");
    }
}
```

#### 集成测试

```rust
// 集成测试文件位于 osubot-core/tests/（如 pp_breakdown.rs）
use osubot_core::api::{calculate_pp_breakdown, PpCalcParams};
use osubot_types::GameMode;
use rosu_mods::{GameMod, GameMods};
use std::path::PathBuf;

fn resource(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/resources")
        .join(name)
}

#[test]
fn std_breakdown_populates_aim_speed_acc() {
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        passed: true,
    })
    .expect("should return breakdown");
    assert!(pp.aim.unwrap() > 0.0, "aim should be > 0");
    assert!(pp.speed.unwrap() > 0.0, "speed should be > 0");
    assert!(pp.accuracy > 0.0, "accuracy pp should be > 0");
    assert_eq!(pp.difficulty, None, "std has no difficulty field");
    assert!(pp.total_pp > 0.0);
}
```

#### WASM 插件测试

参考 `osubot-plugin/src/lib.rs` 的测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_deserialize() {
        let json = r#"{"protocol_version":1,"name":"test","version":"1.0","author":"me","description":"desc","commands":["!ping"]}"#;
        let meta: PluginMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(meta.protocol_version, 1);
        assert_eq!(meta.name, "test");
        assert_eq!(meta.commands, vec!["!ping"]);
    }
}
```

### 运行测试

```bash
# 运行所有测试
cargo test --locked --workspace

# 运行特定 crate 的测试
cargo test -p osubot-core

# 运行特定测试函数
cargo test test_parse_game_mode

# 显示测试输出
cargo test -- --nocapture
```

### CI 检查项

每次 PR 会自动运行以下检查：

```bash
# 1. 代码格式检查
cargo fmt --check

# 2. Clippy lint（警告视为错误）
cargo clippy --locked -- -D warnings

# 3. 运行测试
cargo test --locked --workspace

# 4. 安全审计
cargo audit

# 5. 依赖检查（nightly）
cargo +nightly udeps
```

### 编写测试的最佳实践

1. **测试命名**：`test_<功能>_<场景>_<预期结果>`
   ```rust
   #[test]
   fn test_parse_command_invalid_mode_returns_error() { ... }
   ```

2. **测试组织**：使用 `mod tests` 将相关测试分组

3. **测试数据**：使用 `OnceLock` 或 `Arc` 初始化共享测试数据

4. **测试覆盖**：
   - 正常路径
   - 边界条件
   - 错误情况
   - 空值/null 处理

5. **WASM 插件测试**：
   - 使用 `#[cfg(test)]` 模块
   - 测试元数据解析
   - 测试命令分发逻辑
   - 参考 `examples/hello-plugin` 的测试
