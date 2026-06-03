# osubot-rs

通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。

## 功能

- 查询自己/他人的 osu! 数据（pp、排名、国家排名、准确率、游玩次数等）
- 支持 4 种模式：std、taiko、catch、mania
- 绑定 osu! 账号支持 IRC 鉴权，防止冒名绑定
- 后台定时更新，活跃玩家更新更频繁
- 今日高光：显示群内用户当日最飞升、最肝、最长游戏时间
- 个人主页卡片：生成 osu! 个人主页渲染图片（!profile）
- 分数卡片：生成 Material Design 3 风格的成绩渲染图片（!p/!r），含 PP 分解（aim/speed/acc/flashlight/difficulty）、准确率推测表（95%~100% + IF FC）、UR（Unstable Rate）显示、mod 调整后的 AR/OD/CS/HP 双段进度条

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

### 分数查询
- `!p` — 最近通过的成绩卡片（图片）
- `!r` — 最近游玩的成绩卡片（图片）
- `!p #N` — 第 N 条通过记录
- `!ps` — 最近 10 条通过记录（文本摘要）
- `!rs` — 最近 10 条游玩记录（文本摘要）
- 支持用户名、@用户、模式后缀（`:0`~`:3`），如 `!p username:1`

### 个人主页卡片
- `!profile` — 生成自己的 osu! 个人主页卡片（图片）
- `!profile <用户名>` — 生成指定用户的个人主页卡片
- `!profile` + `@<QQ用户>` — 生成被 @ 用户的个人主页卡片

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

### 群黑白名单

控制哪些群可以使用 bot，默认黑名单模式（所有群可用）：
```toml
[group_filter]
mode = "blacklist"        # "blacklist" 或 "whitelist"
group_ids = [123456789]   # 黑名单=禁用这些群，白名单=仅允许这些群
```

### 群命令开关

按群控制命令开关，默认全部开启：
```toml
[groups.default]
query = true      # ~、where、查
score = true      # !p、!ps、!r、!rs
profile = true    # !profile
highlight = true  # 今日高光
bind = true       # 绑定、解绑

# 特定群覆盖（节名 = 群号）
[groups.123456789]
highlight = false  # 该群禁用今日高光
```

## 用户安装要求

发行包自带所有 C 运行时库（librsvg、cairo、glib2、pango），无需手动安装。唯一需要的是字体：

### 字体

`!profile` 使用系统字体渲染个人主页，CSS 字体栈为：
```
'Noto Sans', 'Noto Color Emoji', 'Source Han Sans CN', 'WenQuanYi Micro Hei', sans-serif
```

如需使用其他字体，修改 `osubot-render/styles/profile.css` 中的 `--font-content` 和 `--font-default` 变量。

**Linux (Arch):**
```bash
sudo pacman -S noto-fonts noto-fonts-cjk noto-fonts-emoji adobe-source-han-sans-cn-fonts
```

**Windows:**
- 下载安装 [Noto Sans](https://fonts.google.com/noto/specimen/Noto+Sans)、[Noto Color Emoji](https://fonts.google.com/noto/specimen/Noto+Color+Emoji)、[Source Han Sans CN](https://github.com/adobe-fonts/source-han-sans/releases)
- 或将字体文件放到 `C:\Windows\Fonts\`

### 开发者编译依赖

如需从源码编译，需要安装以下开发包：

**Linux (Arch):**
```bash
sudo pacman -S librsvg cairo glib2 pango pkgconf
```

**Windows (MSYS2):**
```bash
pacman -S mingw-w64-x86_64-librsvg mingw-w64-x86_64-cairo mingw-w64-x86_64-glib2 mingw-w64-x86_64-pango mingw-w64-x86_64-pkgconf
```

## 运行

```bash
cargo run --release
```

## 技术细节

- **语言**: Rust (stable, edition 2021)
- **异步运行时**: Tokio 多线程事件循环
- **存储**: SQLite (rusqlite)，存储用户绑定、数据快照和游玩记录
- **WebSocket**: tokio-tungstenite 连接 OneBot 11 正向 WebSocket
- **API**: osu! API v2，OAuth client credentials 认证
- **PP 计算**: rosu-pp v4，支持 PP 分解和准确率推测
- **渲染**: Blitz + Vello CPU（HTML 转位图）、librsvg/cairo（SVG 光栅化）
- **JPEG 质量**: 成绩卡片 90、个人主页 80、内嵌图片 85-90
- **准确率显示**: floor 截断（与 osu!lazer 官方行为一致），尾随 0 自动去除
- **模组解析**: rosu_mods::GameMods 完整解析，支持 DT 自定义倍率、DA、lazer 独占模组等
- **日志**: tracing + tracing-subscriber 结构化日志（本地时区）

### 项目结构

```
osubot-rs/
├── osubot/             # 主程序
│   └── src/
│       ├── main.rs     # WebSocket 连接、消息循环、命令分发
│       ├── config.rs   # TOML 配置加载
│       ├── constants.rs # 超时常量定义
│       └── scheduler.rs # 后台定时更新调度器
├── osubot-core/        # 核心库
│   └── src/
│       ├── commands.rs  # 命令解析
│       ├── api.rs       # osu! API v2 调用 + OAuth 缓存
│       ├── storage.rs   # SQLite 存储
│       ├── response.rs  # 响应格式化
│       ├── highlight.rs # 今日高光业务逻辑
│       ├── ur.rs        # 回放解析 + UR 计算
│       ├── dedup.rs     # 请求去重
│       ├── rate_limiter.rs # 令牌桶限流
│       ├── cache.rs     # replay/beatmap 文件缓存
│       ├── irc.rs       # IRC 连接与消息监听
│       └── types.rs     # 数据类型定义
├── osubot-render/      # 渲染引擎（个人主页 + 分数卡片）
│   ├── src/
│   │   ├── lib.rs       # 渲染入口与编排
│   │   ├── render.rs    # HTML → 位图渲染 (Blitz + Vello CPU)
│   │   ├── score_style.rs # 分数卡片 HTML 模板
│   │   ├── style.rs     # 个人主页 CSS 注入
│   │   ├── cache.rs     # 图片缓存 + SVG 光栅化
│   │   ├── encode.rs    # JPEG 编码
│   │   └── error.rs     # 错误类型
│   └── styles/
│       ├── score.css    # 分数卡片样式（MD3）
│       └── profile.css  # 个人主页样式
└── osubot-types/       # 共享类型（Score、GameMode、格式化工具）
```

### 调度器

后台调度器根据用户游玩记录动态调整更新频率：

| 活跃度 | 判断依据 | 更新间隔 |
|--------|----------|----------|
| 半活跃 | 4 小时内有游玩记录 | 4 小时 |
| 普通 | 当日（本地0点至今）有游玩记录 | 8 小时 |
| 无最近 | 当日无游玩记录，8h 内有活动 | 6 小时 |
| 不活跃 | 48 小时以上无游玩记录 | 48 小时 |
| 用户不存在 | API 返回 NotFound | 24 小时 |

调度器通过 osu! API 的 `/users/{id}/scores/recent` 接口获取玩家最近游玩记录，写入数据库。每次调度都保存一份数据快照，不依赖"变化"判断。群内手动查询会触发 `trigger_update`（1 小时冷却），确保交互后及时刷新数据。

### 排名变化

查询时会对比约 24 小时前的快照，显示 pp、排名、准确率、游玩次数等各项变化。

### 请求去重与限流

并发的相同请求（如多人同时查询同一用户）通过 `RequestDedup` 只执行一次 API 调用。所有 osu! API 请求经过令牌桶限流（60 突发/1 每秒），防止触发 API 速率限制。

## 许可

本项目采用 GNU Affero General Public License v3.0 (AGPL-3.0) 许可。
