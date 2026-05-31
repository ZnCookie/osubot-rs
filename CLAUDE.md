# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目概述

osubot-rs 是通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。所有用户可见文本为中文。通过 WebSocket 连接 QQ 机器人框架（go-cqhttp/Lagrange），解析群消息，调用 osu! API v2，返回文本或渲染图片。

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

四 crate 工作空间，依赖方向严格单向：

```
osubot-types   ← 共享类型（Score、GameMode、格式化工具），仅依赖 chrono
    ↑
osubot-core    ← 领域逻辑（API 客户端、存储、命令解析、去重、限流、IRC、UR 计算）
    ↑
osubot-render  ← HTML 转图片渲染（blitz/Vello CPU，librsvg 处理 SVG）
    ↑
osubot         ← 二进制入口（WebSocket 循环、命令调度、后台调度器）
```

**osubot-core 关键模块**：`api.rs`（osu! API + OAuth 缓存 + 401 重试）、`storage.rs`（SQLite 用户绑定/快照/游玩记录）、`commands.rs`（中英文命令解析，`!ps`/`!rs` 优先于 `!p`/`!r` 避免前缀冲突）、`dedup.rs`（信号量去重，相同请求只执行一次）、`rate_limiter.rs`（令牌桶 60 突发/5 每秒）、`ur.rs`（回放解析 + 滚动 UR/PP 计算）

**osubot-render**：信号量限制并发 1 个渲染任务。流程：下载缓存图片 → 提取封面主色调 → 生成内联 CSS 和 data URI 的 HTML → blitz 布局 → Vello CPU 光栅化（超高内容分块渲染）→ JPEG 编码。图片缓存在 `$XDG_CACHE/osubot/resources/`，SHA256 命名。

**osubot（二进制）**：`main.rs` 拥有 WebSocket 循环和命令调度，每条消息 spawn 独立 tokio 任务，通过 `mpsc::channel(1)` 返回响应。`scheduler.rs` 按用户活跃度（4h~48h）轮询更新，命令处理时通过 `trigger_update` 刷新数据。

## 关键模式

- **错误处理**：各 crate 用 `thiserror` 定义错误枚举，二进制层在调用处映射为中文错误文本，渲染失败回退为纯文本
- **异步**：CPU 密集任务用 `spawn_blocking`，取消用 `Arc<AtomicBool>` 在渲染循环边界检查
- **超时**：UR 10s、渲染 30s/60s、OneBot API 5s、限流获取 10s
- **OneBot 协议**：通过 `echo` 字段 + `oneshot::channel` 关联请求响应，图片用 base64 CQ 段发送
- **调度器**：非 cron，用 SQLite 中的 `next_update` 时间戳，按活跃度（SemiActive/Normal/NoRecent/Inactive）动态调整间隔
- **约定**：`Cargo.lock` 已提交，CI 命令用 `--locked`；CSS 通过 `include_str!` 内联；静态去重实例用 `OnceLock`
