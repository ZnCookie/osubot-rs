# osubot-rs

通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。

## 功能

- 查询自己/他人的 osu! 数据（pp、排名、国家排名、准确率、游玩次数等）
- 支持 4 种模式：std、taiko、catch、mania
- 绑定 osu! 账号后自动追踪排名变化
- 后台定时更新，活跃玩家更新更频繁
- 今日高光：显示群内用户当日最飞升、最肝、最长游戏时间

## 命令

### 查询
- `~` — 查询自己 osu!std 数据
- `~1` — 查询自己 taiko 数据
- `~2` — 查询自己 catch 数据
- `~3` — 查询自己 mania 数据
- `where <用户名>` — 查询指定用户的 osu!std 数据
- `where <用户名>,<模式>` — 查询指定用户的指定模式数据，模式为 0~3
- `查@<QQ用户>` — 查询被 @ 用户的 osu!std 数据（需在群消息中 @）
- `查@<QQ用户>,<模式>` — 查询被 @ 用户的指定模式数据

### 绑定
- `绑定 <osu用户名>` — 绑定 QQ 与 osu! 账号
- `解绑` — 解除当前 QQ 的 osu! 账号绑定（需二次确认）

### 高光
- `今日高光` — 查看当日最飞升、最肝、最长游戏时间（默认 osu!std）
- `今日高光,<模式>` — 查看指定模式的当日高光，模式为 0~3

## 配置

```bash
cp osubot.example.toml osubot.toml
# 编辑 osubot.toml，填入 osu! API 凭据和 OneBot WebSocket 地址
```

osu! API v2 凭据在 [osu! 设置](https://osu.ppy.sh/home/account/edit#oauth) 创建 OAuth 应用获取。

## 运行

```bash
cargo run --release
```

## 技术细节

- **语言**: Rust (stable, edition 2021)
- **异步运行时**: Tokio 单线程事件循环
- **存储**: SQLite (rusqlite)，存储用户绑定、数据快照和游玩记录
- **WebSocket**: tokio-tungstenite 连接 OneBot 11 正向 WebSocket
- **API**: osu! API v2，OAuth client credentials 认证
- **日志**: tracing + tracing-subscriber 结构化日志

### 项目结构

```
osubot-rs/
├── osubot/             # 主程序
│   └── src/
│       ├── main.rs     # WebSocket 连接、消息循环、命令分发
│       ├── config.rs   # TOML 配置加载
│       └── scheduler.rs # 后台定时更新调度器
├── osubot-core/        # 核心库
│   └── src/
│       ├── commands.rs # 命令解析
│       ├── api.rs      # osu! API v2 调用
│       ├── highlight.rs # 今日高光业务逻辑
│       ├── response.rs # 响应格式化
│       ├── storage.rs  # SQLite 存储
│       └── types.rs    # 数据类型定义
└── osubot.example.toml # 配置模板
```

### 调度器

后台调度器根据用户活跃度动态调整更新频率：

| 活跃度 | 更新间隔 |
|--------|----------|
| 活跃（有新记录） | 2 小时 |
| 半活跃（有记录无新增） | 4 小时 |
| 普通（有变化） | 8 小时 |
| 不活跃 | 48 小时 |

群内手动查询受 cooldown 限制，防止频繁请求。

### 排名变化

查询自己时会对比约 4 小时前的快照，显示 pp、排名、准确率、游玩次数等各项变化。

## 许可

本项目采用 GNU Affero General Public License v3.0 (AGPL-3.0) 许可。
