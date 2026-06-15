# osubot 插件开发文档

## 概述

插件编译为 WebAssembly（`wasm32-unknown-unknown` 或 `wasm32-wasip1`），通过 wasmtime 动态加载，与主程序完全隔离。依赖 `osubot-plugin-sdk` 开发。

## 插件结构

每个 `.wasm` 文件必须导出以下函数：

| 函数 | 签名 | 说明 |
|------|------|------|
| `alloc` | `(size: u32) -> *mut u8` | 分配内存 |
| `dealloc` | `(ptr, size: u32)` | 释放内存 |
| `plugin_metadata` | `() -> *const u8` | 返回 JSON 格式元数据 |
| `on_load` (可选) | `() -> *const u8` | 加载时初始化，返回值被丢弃 |
| `on_unload` (可选) | `() -> *const u8` | 卸载时清理，返回值被丢弃 |
| `on_command` (可选) | `(cmd_ptr: u32, cmd_len: u32) -> *const u8` | 匹配到注册命令时调用，返回 `PluginAction` |
| `on_message` (可选) | `(msg_ptr: u32, msg_len: u32) -> *const u8` | 收到群消息时调用，返回 `PluginAction` |
| `on_tick` (可选) | `(tick_ptr: u32, tick_len: u32) -> *const u8` | 定时任务触发，传入 `{"tick_id": u32}`，返回值被丢弃 |

### 内存协议

宿主和插件通过 WASM 线性内存交换 JSON：宿主调用 `alloc` 分配缓冲区 → 写入 JSON → 调导出函数传 `(ptr, len)`。插件返回值同样通过 `alloc` → 4 字节长度前缀 + JSON → 返回指针。

## 类型参考

### `PluginMetadata`

```rust
pub struct PluginMetadata {
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub commands: Vec<String>,  // 注册的命令，如 ["!hello", "!ping"]
}
```

### `PluginAction`

```rust
pub enum PluginAction {
    Handled(String),   // 已处理，使用此文本响应
    Next,              // 交给下一个插件
    Intercepted,       // 拦截（插件内部已异步处理）
}
```

### `Command`

`on_command` 收到的 JSON 包含：`command_type`、`group_id`、`user_id`、`message`、`mode`、`username`、`qq`、`beatmap_id`、`score_id`、`mods`、`limit`、`mentioned_user_id`。

### `QQMessage`

`on_message` 收到的 JSON 包含：`group_id`、`user_id`、`message`、`mentioned_user_id`。

## 宿主函数

插件通过 `PluginContext` 静态实例调用宿主能力。所有调用返回 `Result<T, String>`。

| 函数 | 返回 `T` | 说明 |
|------|----------|------|
| `send_group_msg(group_id, text)` | `()` | 发纯文本群消息 |
| `send_group_msg_segments(group_id, segments)` | `()` | 发富文本 segments 群消息 |
| `send_image(group_id, jpeg_base64)` | `()` | 发 JPEG 图片 |
| `http_request(url)` | `String` | HTTP GET 请求，返回响应体 |
| `http_request_with_method(url, method, body)` | `String` | HTTP 自定义方法请求（`body` 为 `Option<&str>`） |
| `db_get_binding(qq)` | `Option<(i64, String)>` | 查询 QQ 绑定的 osu! 用户（用户 ID, 用户名） |
| `osu_api_fetch_user(username, mode)` | `String` | 查询 osu! 用户统计，返回 API JSON |
| `register_tick(name, interval_secs)` | `u32` | 注册定时任务（最小 5 秒，最多 8 个/插件），返回 tick_id |
| `get_plugin_config()` | `serde_json::Value` | 获取插件自定义配置 |

## 开发流程

### 创建项目

```toml
[package]
name = "my-plugin"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
osubot-plugin-sdk = { git = "https://github.com/ZnCookie/osubot-rs" }
serde_json = "1"
```

### 编写插件

```rust
#![no_main]
use osubot_plugin_sdk::*;

static CTX: PluginContext = PluginContext;

#[no_mangle]
pub unsafe extern "C" fn alloc(size: u32) -> *mut u8 { osubot_plugin_sdk::alloc(size) }

#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, size: u32) { osubot_plugin_sdk::dealloc(ptr, size) }

#[no_mangle]
pub extern "C" fn plugin_metadata() -> *const u8 {
    serialize_return(&PluginMetadata {
        name: "my-plugin", version: "0.1.0", author: "me",
        description: "My first plugin", commands: vec!["!ping"],
    })
}

#[no_mangle]
pub extern "C" fn on_command(cmd_ptr: u32, cmd_len: u32) -> *const u8 {
    let cmd_json = unsafe {
        let slice = core::slice::from_raw_parts(cmd_ptr as *const u8, cmd_len as usize);
        core::str::from_utf8(slice).unwrap_or("")
    };
    let cmd: Command = match serde_json::from_str(cmd_json) {
        Ok(c) => c,
        Err(_) => return serialize_return(&PluginAction::Next),
    };
    if cmd.command_type == "!ping" {
        let _ = CTX.send_group_msg(cmd.group_id.unwrap_or(0), "pong!");
        serialize_return(&PluginAction::Intercepted)
    } else {
        serialize_return(&PluginAction::Next)
    }
}
```

### 编译 & 部署

```bash
rustup target add wasm32-unknown-unknown
cargo build --target wasm32-unknown-unknown --release
# 产物: target/wasm32-unknown-unknown/release/my_plugin.wasm
```

`.wasm` 文件放到 `plugin.dir`（默认 `./plugins/`），配置 `osubot.toml`：

```toml
[[plugin.instances]]
name = "my-plugin"
path = "my_plugin.wasm"
enabled = true
priority = 50

[plugin.instances.config]        # 可选，通过 get_plugin_config() 访问
endpoint = "https://api.example.com"
key = "value"
```

## 事件处理顺序

```
群消息 → on_message (遍历插件) → parse_command → on_command (声明了该命令的插件) → 默认处理器
```

各钩子返回 `Handled`/`Intercepted` 会中断后续流程。`on_message` 的 `Intercepted` 会跳过命令解析和默认处理器，谨慎使用。

## 最佳实践

- **避免内置命令前缀**：`~` `where` `绑定` `解绑` `今日高光` `!p` `!r` `!ps` `!rs` `!s` `!ss` `!profile`。建议用 `!` + 特有名称
- **不要 panic**：所有宿主调用返回 `Result`，用 `?` 或 match 处理
- **保持轻量**：插件调用有 10 秒超时，耗时操作用 `http_request` 委托
- **热重载丢失内存状态**：重载/重连后插件实例重建，持久化数据用 `http_request` 存外部服务
- **`on_load` 分配的资源在 `on_unload` 释放**

## 故障隔离

- 内存上限 100MB（wasmtime StoreLimits）
- 10 秒超时（tokio::timeout），10 秒 epoch 中断最后防线
- 无文件系统/线程访问，仅通过宿主函数做网络和数据库操作
- 连续 5 次错误自动重载

## 版本兼容

| 接口 | 保证 |
|------|------|
| 导出函数签名 | `alloc`/`dealloc`/`plugin_metadata`/`on_*` 签名不变 |
| 宿主函数名 | 现有函数名不变 |
| JSON 输入/输出 | 现有字段保留，可能新增 |
| SDK Rust 类型 | 现有字段保留 |
| 配置 TOML | `[plugin]` / `[[plugin.instances]]` 格式不变 |

MINOR 升级保证兼容，MAJOR 升级提供迁移指南。

## 测试

参考 `osubot-plugin/src/lib.rs` 的 `#[cfg(test)]` 模块（20 个测试覆盖元数据解析、命令分发、tick 生命周期、reload 等）。测试需要编译好的 `.wasm` 文件，`examples/hello-plugin` 的构建产物可自动被检测使用。
