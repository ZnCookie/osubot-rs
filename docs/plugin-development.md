# osubot 插件开发文档

## 概述

osubot 插件是编译为 WebAssembly 的模块（`wasm32-unknown-unknown` 或 `wasm32-wasip1` 目标均可），通过 WASM 运行时（wasmtime）动态加载。插件与主程序完全隔离——崩溃、死循环、内存越界均不影响主程序运行。

### 架构

```
┌──────────────────────────────────────────────────────┐
│  osubot 主程序                                        │
│  ┌───────────────────────────────────────────────┐  │
│  │  PluginManager                                 │  │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐      │  │
│  │  │ 插件 A    │ │ 插件 B    │ │ 插件 C    │      │  │
│  │  │ (WASM)    │ │ (WASM)    │ │ (WASM)    │      │  │
│  │  └──────────┘ └──────────┘ └──────────┘      │  │
│  └───────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────┘
                         │
              ┌──────────┴──────────┐
              │  osubot-plugin-sdk   │  ← 插件编译时依赖
              │  (wasm32-unknown-    │    （推荐 wasm32-unknown-unknown）
              │   unknown / wasip1)  │
              └─────────────────────┘
```

---

## 插件结构

每个插件是一个 `.wasm` 文件，必须导出以下函数：

### 必需导出

| 函数 | 签名 | 说明 |
|------|------|------|
| `alloc` | `(size: u32) -> *mut u8` | 分配内存，供宿主写入输入数据 |
| `dealloc` | `(ptr: *mut u8, size: u32)` | 释放先前分配的内存 |
| `plugin_metadata` | `() -> *const u8` | 返回 JSON 格式的插件元数据 |

### 可选导出（生命周期钩子）

| 函数 | 签名 | 说明 |
|------|------|------|
| `on_load` | `() -> *const u8` | 加载时调用，用于初始化 |
| `on_unload` | `() -> *const u8` | 卸载时调用，用于清理 |
| `on_command` | `(cmd_ptr: u32, cmd_len: u32) -> *const u8` | 匹配到插件注册的命令时调用 |
| `on_message` | `(msg_ptr: u32, msg_len: u32) -> *const u8` | 收到任何群消息时调用 |
| `on_tick` | `(tick_ptr: u32, tick_len: u32) -> *const u8` | 定时任务触发时调用，传入 `{"tick_id": u32}` |

### 内存协议

插件和宿主通过 WASM 线性内存交换 JSON 数据：

- **宿主→插件**：宿主调用插件的 `alloc(size)` 分配缓冲区，写入 JSON 数据，然后调用目标导出函数，传入 `(ptr, len)`
- **插件→宿主**：插件调用 `alloc(size)` 分配缓冲区，写入 **4 字节长度前缀 + JSON 数据**，返回指针。宿主读取后调用插件的 `dealloc(ptr, size)` 释放

插件内部调用宿主函数时，宿主通过插件导出的 `alloc`/`dealloc` 管理返回数据。

---

## 类型参考

### `PluginMetadata`

返回给宿主声明插件信息：

```rust
pub struct PluginMetadata {
    pub name: String,        // 插件名，唯一标识
    pub version: String,     // 语义化版本号
    pub author: String,      // 作者
    pub description: String, // 描述
    pub commands: Vec<String>, // 注册的命令列表，如 ["!hello", "!ping"]
}
```

> **注意：** 插件优先级在 `[[plugin.instances]]` 配置的 `priority` 字段中设置（见部署配置），`PluginMetadata` 中不再包含优先级字段。

### `PluginAction`

每个事件钩子返回的动作，决定宿主后续行为：

```rust
pub enum PluginAction {
    /// 插件已处理，使用此文本作为响应，不再执行后续插件和默认处理器
    Handled(String),
    /// 放行，交给下一个插件或默认处理器
    Next,
    /// 拦截，不处理也不放行（插件内部已异步处理，无需宿主响应）
    Intercepted,
}
```

### `Command`

`on_command` 接收到的命令参数（JSON 反序列化）：

```rust
pub struct Command {
    pub command_type: String,     // 命令名，如 "~"、"!p"、"!s"、"绑定"
    pub group_id: Option<i64>,    // 群号
    pub user_id: Option<i64>,     // 发送者 QQ
    pub message: Option<String>,  // 原始消息文本
    pub mode: Option<u8>,         // 游戏模式 0=osu 1=taiko 2=catch 3=mania
    pub username: Option<String>, // 查询的用户名（如有）
    pub qq: Option<i64>,          // @的 QQ 号（如有）
    pub beatmap_id: Option<u32>,  // 谱面 ID（!s 命令）
    pub score_id: Option<u64>,    // 成绩 ID（!s 命令）
    pub mods: Option<Vec<String>>, // mod 列表（如 ["HD", "DT"]）
    pub limit: Option<u32>,       // 限制条数
    pub mentioned_user_id: Option<i64>, // @的用户（如有）
}
```

### `QQMessage`

`on_message` 接收到的原始消息（JSON 反序列化）：

```rust
pub struct QQMessage {
    pub group_id: i64,
    pub user_id: i64,
    pub message: String,
    pub mentioned_user_id: Option<i64>, // @的用户（如有）
}
```

---

## 宿主函数（Host Functions）

插件通过 `PluginContext` 调用宿主能力。所有调用返回 `Result<T, String>`，失败时返回错误描述字符串。

> **设计说明：** `PluginContext` 是单元结构体，通过 `static CTX` 全局访问而非以参数形式传递。WASM 实例始终在单线程中运行（wasmtime 保证），宿主通过 `Caller` 上下文自动路由到正确实例，因此 `static` 在此模型下是安全的。

> **输入限制：** 每次宿主函数调用中，函数名和 JSON payload 各不能超过 **1MB**（通过 `host_call_impl` 的 `name_len`/`payload_len` 参数校验）。传参过大将收到错误返回。

### `send_group_msg(group_id, text)`

向指定群发送文本消息。

```rust
ctx.send_group_msg(123456789, "Hello!")?;
```

### `send_image(group_id, jpeg_base64)`

向指定群发送 JPEG 图片。图片数据需预先编码为 base64。

```rust
// 假设已通过 http_request 获取图片并编码为 base64
ctx.send_image(123456789, &jpeg_base64)?;
```

### `http_request(url)`

发起 HTTP GET 请求，返回响应体文本。

```rust
let json = ctx.http_request("https://api.example.com/data")?;
```

### `db_get_binding(qq)`

查询 QQ 号绑定的 osu! 用户信息。

```rust
if let Some((user_id, username)) = ctx.db_get_binding(123456)? {
    // user_id: i64, username: String
}
```

### `osu_api_fetch_user(username, mode)`

查询 osu! 用户统计数据。

```rust
let stats_json = ctx.osu_api_fetch_user("Cookiezi", 0)?;
// 返回 osu! API v2 的 UserStats JSON
```

JSON 字段说明（以下为常用字段，完整字段见 osu! API v2 文档；版本升级时可能新增字段但不会删除现有字段）：

| 字段 | 类型 | 说明 |
|------|------|------|
| `user_id` | `i64` | 用户 ID |
| `username` | `String` | 用户名 |
| `pp` | `f64` | PP |
| `rank` | `i64` | 全球排名 |
| `country_rank` | `i64` | 国家排名 |
| `country_code` | `String` | 国家代码（如 `"CN"`） |
| `ranked_score` | `i64` | 排位分 |
| `accuracy` | `f64` | 准确率（如 `97.5`） |
| `playcount` | `i64` | 游玩次数 |
| `hits` | `i64` | 打击数 |
| `playtime` | `i64` | 游戏时间（秒） |
| `rank_change` | `i64` 或 `null` | 排名变化 |
| `country_rank_change` | `i64` 或 `null` | 国家排名变化 |
| `cover_url` | `String` 或 `null` | 封面图 URL |

### `register_tick(name, interval_secs)`

注册定时任务。

- 最小间隔：**5 秒**
- 每插件最多注册 **8 个** tick
- 同名 tick 重复注册会更新间隔（返回原有 tick_id），不消耗配额

```rust
let tick_id = ctx.register_tick("check-update", 3600)?;
// 每小时触发一次 on_tick，传入 {"tick_id": tick_id}
```

> **注意：** 插件重载（`reload_instance` 或 `reload_all`）会**清除所有 tick 注册**。重载后，插件需要在 `on_load` 中重新调用 `register_tick` 来重新注册定时任务。这是设计如此——重载后的插件实例索引可能变化，旧的 tick 注册不再有效。

### `get_plugin_config()`

获取插件配置（来自 `osubot.toml` 的 `config` 字段）。

```rust
let cfg: serde_json::Value = ctx.get_plugin_config()?;
if let Some(endpoint) = cfg.get("endpoint").and_then(|v| v.as_str()) {
    // 使用配置
}
```

---

## 开发流程

### 1. 创建项目

```toml
# Cargo.toml
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

### 2. 编写插件

```rust
#![no_main]

use osubot_plugin_sdk::*;

static CTX: PluginContext = PluginContext;

#[no_mangle]
pub unsafe extern "C" fn alloc(size: u32) -> *mut u8 {
    osubot_plugin_sdk::alloc(size)
}

#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, size: u32) {
    osubot_plugin_sdk::dealloc(ptr, size)
}

#[no_mangle]
pub extern "C" fn plugin_metadata() -> *const u8 {
    serialize_return(&PluginMetadata {
        name: "my-plugin",
        version: "0.1.0",
        author: "me",
        description: "My first plugin",
        commands: vec!["!ping"],
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

### 3. 编译

```bash
# 安装 wasm32-unknown-unknown 目标（推荐，SDK 无 WASI 依赖）
rustup target add wasm32-unknown-unknown

# 编译
cargo build --target wasm32-unknown-unknown --release

# 编译产物
ls target/wasm32-unknown-unknown/release/my_plugin.wasm
```

> SDK 使用自定义 `alloc`/`dealloc`，不依赖 WASI 导入，因此两个目标均可使用：
> - **`wasm32-unknown-unknown`**（推荐）：无标准库依赖，编译更快，二进制更小
> - **`wasm32-wasip1`**：包含 WASI 导入但 SDK 不使用，编译稍慢

### 4. 部署

将 `.wasm` 文件放到 `osubot.toml` 中 `plugin.dir` 指定的目录（默认 `./plugins/`）。

```toml
[plugin]
dir = "./plugins"

# 插件连续错误/panic 多少次后自动重载（默认 5）
lost_instances_threshold = 5

# 插件连续重载失败多少次后停止自动重载（默认 3）
reload_failures_threshold = 3

[[plugin.instances]]
name = "my-plugin"
path = "my_plugin.wasm"
enabled = true
priority = 50

# 可选：插件自定义配置（通过 get_plugin_config() 访问）
[plugin.instances.config]
endpoint = "https://api.example.com"
key = "value"
```

---

## 事件处理顺序

```
收到群消息
  │
  ├─→ on_message (遍历所有插件，按优先级)
  │     ├─ Handled → 使用响应，结束
  │     ├─ Intercepted → 结束
  │     └─ Next → 继续下一个插件
  │
  ├─→ parse_command 解析为 Command
  │
  ├─→ on_command (只通知声明了该命令的插件)
  │     ├─ Handled → 使用响应，结束
  │     ├─ Intercepted → 结束
  │     └─ Next → 执行默认处理器
  │
  └─→ 默认处理器（osubot 内置功能）
```

> **警告：** `on_message` 返回 `Intercepted` 会**中断整个事件处理流程**——不会解析命令、不会通知 `on_command`、不会执行默认处理器。请仅在明确需要完全接管消息时使用。`on_command` 返回 `Intercepted` 则仅跳过默认处理器，影响范围更小。

---

## 版本兼容性保证

以下接口保证语义化版本中的 **次要版本升级**（MINOR）时向下兼容：

| 接口 | 保证 |
|------|------|
| WASM 导出函数签名 | `alloc(u32)->ptr` / `dealloc(ptr, u32)` / `plugin_metadata()->ptr` / `on_*(u32, u32)->ptr` |
| 宿主函数名 | `send_group_msg` / `send_image` / `http_request` / `db_get_binding` / `osu_api_fetch_user` / `register_tick` / `get_plugin_config` |
| 宿主函数 JSON 输入格式 | 现有字段不会删除或改类型 |
| 宿主函数 JSON 输出格式 | 现有字段保留，可能新增字段 |
| SDK Rust 类型 | 现有字段不会删除，可能新增字段 |
| 配置 TOML 格式 | `[plugin]` / `[[plugin.instances]]` / 现有字段 |

**主版本升级**（MAJOR）可能会破坏兼容性，届时会提供迁移指南。

---

## 最佳实践

### 错误处理

不要 panic。所有可能失败的调用返回 `Result`，用 `?` 传播或 match：

```rust
if let Err(e) = ctx.send_group_msg(gid, text) {
    // 记录错误，继续执行
}
```

### 超时

每个插件调用有 10 秒超时。超过超时的插件实例会被丢弃（不再调用）。保持 `on_command` / `on_message` 轻量，不要在插件中做耗时计算。

> **注意：** 超时后，后台阻塞线程（如 HTTP 请求）会继续运行直到完成，之后被丢弃。频繁超时可能耗尽线程池，请避免在插件中发起大量长时间请求。

### 命令前缀

避免使用 osubot 内置命令的前缀：
- `~` — 查询自己
- `where` — 查询他人
- `绑定` / `解绑`
- `今日高光`
- `!profile` / `!p` / `!r` / `!ps` / `!rs` / `!s` / `!ss`

建议使用 `!` + 特有名称，如 `!weather`、`!translate`。

### 连接重连

osubot 与 OneBot 服务端的 WebSocket 连接断开时会自动重连。每次重连后，所有插件会被重新加载，**插件的内存状态（KV 存储、累加数据等）会丢失**。如果需要持久化数据，请使用 `http_request` 或 `db_get_binding` 等宿主函数将数据存储到外部服务。

### 资源释放

宿主会为插件分配和释放内存。如果插件在 `on_load` 中分配了额外资源（如打开了 TCP 连接），请在 `on_unload` 中释放。

---

## 热重载

osubot 使用文件监控（`notify` crate）自动检测以下变更：

- **配置文件（`osubot.toml`）变更**：自动热重载 `[plugin]` 和 `[scheduler]`、`[group_filter]`、`[groups]`、`[upstream]` 配置段
- **插件 `.wasm` 文件变更**：自动重新加载对应插件实例
- **注意**：`[osu]`、`[bot]`、`[database]`、`[irc]` 等字段仅在启动时读取，热重载不会修改

热重载流程：
1. 文件监控检测到变更，聚合 500ms 内的重复事件
2. 设置 drain 标志，等待进行中的命令处理任务完成（最长 30 秒）
3. 原子切换配置和插件状态
4. 清除 drain 标志，继续处理新消息

> **注意：** 热重载期间插件实例会重新创建，内存状态丢失。需要持久化的数据请使用 `http_request` 或 `db_get_binding` 等宿主函数存储到外部服务。

---

## 部署注意事项

插件运行在 WASM 沙箱中，提供以下隔离保证：

### WASM 沙箱隔离

- **内存隔离**：每个插件实例独立线性内存，上限 **100MB**，超出限制只会导致该插件崩溃，不影响主程序
- **CPU 隔离**：通过 epoch 中断机制防止死循环，每次调用有超时控制（10 秒 dispatch + 30 秒 epoch 最后防线）
- **无文件系统访问**：插件无法直接读写宿主机文件
- **无线程/进程创建**：插件无法创建系统线程

### 网络访问

- 插件可通过 `http_request` 发起 HTTP/HTTPS 请求到 **任意 URL**
- 请求有 30 秒超时限制，避免长时间挂起
- 所有请求经过全局限流，防止一个插件耗尽 API 配额

### 数据库访问

- 插件可通过 `db_get_binding` 查询任意 QQ 号绑定的 osu! 用户信息（用户名和用户 ID）

### 建议

- **仅部署来自可信来源的 `.wasm` 文件**
- 审查插件源代码后再部署
- 不要在插件配置中存储敏感信息（如 API 密钥）

---

## 测试插件

osubot 的 `osubot-plugin` crate 提供了集成测试框架，可用于验证插件行为。参考 `osubot-plugin/src/lib.rs` 中的 `#[cfg(test)]` 模块，其中包含 18 个测试覆盖了：

- 元数据解析
- 命令/消息分发（Handled / Next / Intercepted 三种返回值）
- tick 生命周期（注册、触发、重载清理）
- compact（索引合并）
- reload_all（增/删/改/空插件、优先级变更、mtime 变更检测）

### 编写测试的基本流程

```rust
#[test]
fn test_my_plugin() {
    // 1. 编译 .wasm（或使用 cargo build 构建产物）
    let wasm_path = find_my_plugin_wasm();

    // 2. 创建 tokio runtime 和 HostServices
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let services = make_services(&rt);

    // 3. 配置并加载插件
    let config = PluginConfigInput { ... };
    let mut pm = rt.block_on(PluginManager::new(&config, services)).unwrap();

    // 4. 验证行为
    let cmd_json = r#"{"command_type":"!ping","group_id":1,"user_id":1,...}"#;
    let action = rt.block_on(pm.handle_command("!ping", cmd_json));
    assert!(matches!(action, PluginActionResult::Handled(_)));

    // 5. 清理
    rt.block_on(pm.shutdown());
    drop(_guard);
}
```

> **提示：** 测试依赖于已编译的 `.wasm` 文件。`osubot-plugin` 的测试会自动检测 `examples/hello-plugin/target/wasm32-unknown-unknown/debug/hello_plugin.wasm`，若不存在则自动执行 `cargo build` 编译。
