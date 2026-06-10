# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository. Keep this file concise — do not exceed 60 lines without user approval.

## 项目概述

osubot-rs 是通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。所有用户可见文本为中文。与用户沟通时尽量使用中文。通过 WebSocket 连接 QQ 机器人框架（go-cqhttp/Lagrange），解析群消息，调用 osu! API v2，返回文本或渲染图片。

## 构建与开发

```bash
cargo build --release --locked        # 构建
cargo run --release                   # 运行
cargo test --locked --workspace       # 测试
cargo clippy --locked -- -D warnings  # lint（CI 将警告视为错误）
cargo fmt --check                     # 格式检查
cargo test --locked -p <crate> <name> # 运行单个测试
```

## 架构

六 crate 工作空间：

```
osubot-types           ← 共享类型（Score、GameMode），依赖 chrono、rosu-mods
    ↑           ↑
osubot-core    osubot-render  ← 两者无依赖关系，均只依赖 osubot-types
    ↑           ↑
osubot-plugin-sdk      ← WASM 插件 SDK（编译到 wasm32-unknown-unknown 或 wasm32-wasip1，无 WASI 依赖），供插件作者使用
    ↑
osubot-plugin          ← WASM 运行时宿主（PluginManager、wasmtime 集成）
    ↑
    └─── osubot ───┘   ← 二进制入口
```

**osubot-core** 关键模块：`api.rs`（osu! API + OAuth 缓存 + 401 重试 + mod 完整解析（通过 rosu_mods::GameMods 保留 DT 倍率、DA 等 lazer 模组设置）+ PP 本地 fallback + 谱面成绩查询与 SoloScore 回填）、`storage.rs`（SQLite 用户绑定/快照/游玩记录）、`commands.rs`（中英文命令解析，`!ps`/`!rs` 优先于 `!p`/`!r` 避免前缀冲突，`!s`/`!ss` 谱面成绩命令支持多词用户名、@QQ、mod 过滤、limit 分页）、`response.rs`（PP/准确率/排名变化等文本格式化）、`highlight.rs`（今日高光业务逻辑）、`cache.rs`（replay/beatmap 文件缓存与定期清理）、`irc.rs`（IRC 客户端 + 指数退避自动重连）、`dedup.rs`（请求去重，相同请求只执行一次）、`rate_limiter.rs`（令牌桶限流，支持可配置 burst/rate）、`upstream.rs`（上游自动绑定抽象 trait + 链式查询 + 去重）、`ur.rs`（回放解析 + 误差排序贪心 note-key 匹配 + UR 计算 + replay 时序偏移修正）

**osubot-render**：信号量限制并发 1 个渲染任务。流程：下载缓存图片 → 提取封面主色调 → 生成内联 CSS 和 data URI 的 HTML → blitz 布局 → Vello CPU 光栅化（超高内容分块渲染）→ JPEG 编码。下载的封面/头像等图片缓存在 `~/.cache/osubot/resources/`；replay/`.osu` 谱面文件分别缓存在 `~/.cache/osubot/{replays,beatmaps}/`，均按 SHA256 命名；这三类缓存由调度器按 `cache_retention_days`（默认 7 天）定期清理。

**osubot（二进制）**：`main.rs` 拥有 WebSocket 循环和命令调度，每条消息 spawn 独立 tokio 任务，通过 `mpsc::channel(1)` 返回响应。`scheduler.rs` 按用户活跃度（4h~48h）轮询更新，触发更新通过 `trigger_update().await`。`reload.rs`（文件监控 + drain 机制 + `MutableConfig` 热重载）。`last_beatmap_cache.rs` 维护群内最近查询的谱面缓存（6h TTL），`!s` 不传谱面 ID 时自动使用。`xfs_upstream.rs` 通过 WebSocket 连接消防栓 bot 的 OneBot 服务，模拟查询消息解析用户名实现自动绑定。`yumu_upstream.rs` 通过 WebSocket 连接 yumu-bot 的 OneBot 服务，伪装用户 QQ 以 `!bi` 命令获取绑定信息。

**osubot-plugin**：`lib.rs`（PluginManager — WASM 插件加载/调度/epoch 超时隔离/生命周期管理/热重载 diff）、`bridge.rs`（HostServices + `host_call_impl` 七种宿主函数分发 + 网络请求 30s 超时）、`instance.rs`（PluginInstance — wasmtime 实例封装 + 内存协议）、`config.rs`（插件配置 TOML 反序列化）。插件开发文档见 `docs/plugin-development.md`。

## 关键模式

- **错误处理**：各 crate 用 `thiserror` 定义错误枚举，二进制层在调用处映射为中文错误文本，渲染失败回退为纯文本
- **异步**：CPU 密集任务用 `spawn_blocking`，取消用 `Arc<AtomicBool>` 在渲染循环边界检查
- **超时**：UR 10s、渲染 30s/60s、OneBot API 5s、限流获取 10s、插件 10s（epoch 30s 防线）
- **OneBot 协议**：通过 `echo` 字段 + `oneshot::channel` 关联请求响应，图片用 base64 CQ 段发送
- **调度器**：非 cron，用 SQLite 中的 `next_update` 时间戳，按活跃度（SemiActive/Normal/NoRecent/Inactive/UserNotExists）动态调整间隔
- **约定**：`Cargo.lock` 已提交，CI 命令用 `--locked`；CSS 通过 `include_str!` 内联；静态去重实例用 `OnceLock`
- **热重载**：`notify` 监控 config/`.wasm` → 500ms 防抖 → drain 等待 in_flight → 原子切换（`Arc<RwLock<Config>>` + `MutableConfig`）
- **插件调度**：`on_message`（按优先级）→ `parse_command` → `on_command`（按优先级）→ 默认处理器。`dispatch()` 用 `spawn_blocking` + `tokio::timeout` 隔离
