# osubot-rs

通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。

## 功能

- 查询自己/他人的 osu! 数据（pp、排名、国家排名、准确率、游玩次数等）
- 支持 4 种模式：std、taiko、catch、mania
- 绑定 osu! 账号支持 IRC 鉴权，防止冒名绑定
- 后台定时更新，活跃玩家更新更频繁
- 今日高光：显示群内用户当日最飞升、最肝、最长游戏时间

## 命令

### 查询
- `~` — 查询自己 osu!std 数据
- `~<模式>` 或 `~,<模式>` — 查询自己指定模式数据，模式为 0~3（0=std, 1=taiko, 2=catch, 3=mania）
- `where <用户名>` — 查询指定用户的 osu!std 数据
- `where <用户名>,<模式>` — 查询指定用户的指定模式数据，模式为 0~3
- `查@<QQ用户>` — 查询被 @ 用户的 osu!std 数据（需在群消息中 @）
- `查@<QQ用户>,<模式>` — 查询被 @ 用户的指定模式数据

### 绑定
- `绑定 <osu用户名>` — 绑定 QQ 与 osu! 账号
- `解绑` — 解除当前 QQ 的 osu! 账号绑定（需二次确认）

当启用 IRC 鉴权时，绑定流程为两步验证：
1. 在群内发送 `绑定 <osu用户名>`，Bot 返回 6 位验证码
2. 用目标 osu! 账号登录 [osu! IRC](https://osu.ppy.sh/p/irc)，私聊 Bot 发送验证码
3. 验证通过后绑定成功，验证码 2 分钟有效，同一 QQ 同时只能有一个待验证请求

未启用 IRC 鉴权时（默认），绑定为直接绑定，无需验证。

### 高光
- `今日高光` — 查看当日最飞升、最肝、最长游戏时间（默认 osu!std）
- `今日高光,<模式>` — 查看指定模式的当日高光，模式为 0~3

## 配置

```bash
cp osubot.example.toml osubot.toml
# 编辑 osubot.toml，填入 osu! API 凭据和 OneBot WebSocket 地址
```

osu! API v2 凭据在 [osu! 设置](https://osu.ppy.sh/home/account/edit#oauth) 创建 OAuth 应用获取。

如需启用 IRC 鉴权绑定，需要在 `osubot.toml` 中配置 `[irc]` 段：
```toml
[irc]
enabled = true
server = "irc.ppy.sh"
port = 6667
nickname = "你的osu用户名"
password = "IRC 密码"  # 在 https://osu.ppy.sh/p/irc 获取
```
IRC 断线会自动重连（5 秒间隔，无限重试）。

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
- **日志**: tracing + tracing-subscriber 结构化日志（本地时区）

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
│       ├── irc.rs      # IRC 连接与消息监听
│       └── types.rs    # 数据类型定义
└── osubot.example.toml # 配置模板
```

### 调度器

后台调度器根据用户游玩记录动态调整更新频率：

| 活跃度 | 判断依据 | 更新间隔 |
|--------|----------|----------|
| 半活跃 | 4 小时内有游玩记录 | 4 小时 |
| 普通 | 当日（本地0点至今）有游玩记录 | 8 小时 |
| 无最近 | 当日无游玩记录，8h 内有活动 | 6 小时 |
| 不活跃 | 48 小时以上无游玩记录 | 48 小时 |

调度器通过 osu! API 的 `/users/{id}/scores/recent` 接口获取玩家最近游玩记录，写入数据库。每次调度都保存一份数据快照，不依赖"变化"判断。群内手动查询受 cooldown 限制，防止频繁请求。

### 排名变化

查询时会对比约 24 小时前的快照，显示 pp、排名、准确率、游玩次数等各项变化。

## 许可

本项目采用 GNU Affero General Public License v3.0 (AGPL-3.0) 许可。
