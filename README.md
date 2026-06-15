# osubot-rs

通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。

## 功能

- 查询自己/他人的 osu! 数据（pp、排名、国家排名、准确率、游玩次数等）
- 支持 4 种模式：0=std, 1=taiko, 2=catch, 3=mania
- 绑定 osu! 账号支持 IRC 鉴权，防止冒名绑定
- 上游自动绑定：未绑定时自动向其他 osu! bot 查询，无需手动操作
- 后台定时更新，活跃玩家更新更频繁
- 今日高光：显示群内用户当日最飞升、最肝、最长游戏时间
- 个人主页卡片：生成 osu! 个人主页渲染图片（!profile）
- 分数卡片：生成 Material Design 3 风格的成绩渲染图片（!p/!r/!s），含 PP 分解（aim/speed/acc/flashlight/difficulty）、准确率推测表（95%~100% + IF FC）、UR（Unstable Rate）显示、mod 调整后的 AR/OD/CS/HP 双段进度条
- 谱面成绩查询：查询指定谱面上的所有成绩或最佳成绩（!s/!ss），支持 mod 过滤和渲染成绩列表

## 命令

### 查询
- `~` — 查询自己 osu!std 数据
- `~<模式>` 或 `~,<模式>` — 查询自己指定模式数据，模式为 `0`/`1`/`2`/`3`（0=std, 1=taiko, 2=catch, 3=mania，无冒号）
- `where <用户名>` — 查询指定用户的 osu!std 数据
- `where <用户名>,<模式>` — 查询指定用户的指定模式数据，模式格式同上（无冒号）
- `where @<QQ号>` — 查询该 QQ 号绑定的 osu! 用户数据（`qq=` 前缀的旧写法仍兼容）
- `where @<QQ号>,<模式>` — 查询该 QQ 号绑定用户的指定模式数据，模式格式同上
- `查@<QQ用户>` — 查询被 @ 用户的 osu!std 数据（需在群消息中 @）
- `查@<QQ用户>,<模式>` — 查询被 @ 用户的指定模式数据，模式格式同上

### 绑定
- `绑定 <osu用户名>` — 绑定 QQ 与 osu! 账号
- `解绑` — 解除当前 QQ 的 osu! 账号绑定（需二次确认）

当启用 IRC 鉴权时，绑定流程为两步验证：
1. 在群内发送 `绑定 <osu用户名>`，Bot 返回 6 位验证码
2. 用目标 osu! 账号登录 [osu! IRC](https://osu.ppy.sh/p/irc)，私聊 Bot 发送验证码
3. 验证通过后绑定成功，验证码 2 分钟有效，同一 QQ 同时只能有一个待验证请求

未启用 IRC 鉴权时（默认），绑定为直接绑定，无需验证。

### 上游自动绑定

配置 `[upstream]` 后，当用户使用 `~`、`where @QQ`、`!p` 等需要绑定的命令时，若本地未绑定，bot 会自动向上游服务器查询并完成绑定（无需用户手动操作）。查询失败时静默回退到手动绑定提示。

```toml
[upstream]
enabled = true

# xfs（消防栓）：通过 OneBot 中继查询，需要 access_token
[[upstream.providers]]
type = "xfs"

# yumu（yumu-bot）：直接连接 yumu 的 OneBot 服务，无需 access_token
[[upstream.providers]]
type = "yumu"
```

### 高光
- `今日高光` — 查看当日最飞升、最肝、最长游戏时间（默认 osu!std）
- `今日高光,<模式>` — 查看指定模式的当日高光，模式为 `0`/`1`/`2`/`3`（0=std, 1=taiko, 2=catch, 3=mania，无冒号）

### 分数查询

格式：`!p`/`!r`/`!ps`/`!rs` [`: <模式>`] [`<用户>`] [`<条件>`] [`#<N>`]

- `!p` — 最近通过的成绩卡片（图片）
- `!r` — 最近游玩的成绩卡片（图片）
- `!ps` — 最近 20 条通过记录（成绩列表图片）
- `!rs` — 最近 20 条游玩记录（成绩列表图片）
- 模式支持 `:0`/`:1`/`:2`/`:3`（:0=std, :1=taiko, :2=catch, :3=mania）
- 条件过滤：`key=value`，多个用逗号分隔，如 `miss=1,combo=500`
- 支持的过滤键：`miss`（Miss 数）、`combo`（最大连击）、`pp`（PP 值）、`score`（得分）、`acc`/`accuracy`（准确率百分比）、`mod`（模组）
- 支持的操作符：`=`（默认，等值或子集）、`==`（等值或精确集合）、`!=`（不等值或子集取反）、`>`、`<`、`>=`、`<=`
  - 数值键接受全部 7 个操作符
  - `mod` 仅 `=`（子集包含）、`==`（精确集合）、`!=`（子集取反）；其他比较操作符静默忽略
  - 浮点键（`pp`/`acc`）的 `==`/`!=` 容差 0.5（与图片整数显示精度对齐）；`>`/`<`/`>=`/`<=` 严格比较
  - 操作符必须紧贴 key（`miss>5`），不允许空格
  - 未列出的 key 不会过滤掉成绩
- `#` 可省略：`!r :3 miss=1 5` 中 `5` 等价于 `#5`
- `#N-M` 范围格式仅 `!ps`/`!rs` 支持
- 支持 @用户 替代用户名，如 `!p @某人 :2`

**示例：**
```
!p
!p :1
!p :2 ZnCookie #5
!p @123456 :3
!ps ZnCookie miss=1,combo=500
!ps ZnCookie mod=HDDT
!ps ZnCookie mod=HDDT,miss=1
!r :3 miss=1,combo=500 5
!ps :2 @123456 #10
!ps miss==0
!ps miss>0
!ps pp>500
!ps pp>=500
!ps acc>=99
!ps mod==DT
!ps mod==NM    # 精确匹配「无 mod」（HDDT 也会被 mod==DT 拒绝）
!ps mod!=DT
```

### 谱面成绩查询

格式：`!s`/`!ss` [`: <模式>`] [`<谱面ID|成绩ID>`] [`<用户>`] [`<条件>`] [`#<N>`]

- `!s <谱面ID>` — 查询自己在该谱面上的最佳成绩（图片）
- `!s <成绩ID>` — 通过成绩 ID 查询单条成绩详情（图片）
- 谱面ID和成绩ID自动区分：`< 10,000,000` 为谱面ID，`≥ 10,000,000` 为成绩ID
- `!s <谱面ID> +<mods>` — 按 mod 过滤最佳成绩，如 `!s 123456 +HDDT`
- `#N-M` 范围格式支持（如 `!ss 123456 #2-10`）
- `!ss <谱面ID>` — 查询自己在该谱面上的所有成绩（成绩列表图片）
- `!s <谱面ID> <用户名>` — 查询指定用户的谱面成绩
- `!s <谱面ID> @<QQ用户>` — 查询被 @ 用户的谱面成绩
- `!s 或 !ss` 可不带谱面 ID，自动使用群内最近查询的谱面（6 小时缓存）
- `!p`/`!r`/`!ps`/`!rs`/`!s`/`!ss` 查询后都会更新此缓存
- 条件过滤与 `!p`/`!r` 系列相同

**示例：**
```
!s 123456
!s :1 123456
!s :2 123456 ZnCookie
!s 123456 +HDHR
!s 123456 ZnCookie +DT,miss=1
!ss 123456 #2-10
```

### 个人主页卡片
- `!profile` — 生成自己的 osu! 个人主页卡片（图片）
- `!profile <用户名>` — 生成指定用户的个人主页卡片
- `!profile @<QQ用户>` — 生成被 @ 用户的个人主页卡片

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
IRC 断线会自动重连（指数退避：5s → 10s → ... → 最长 5 分钟，无限重试）。

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
score = true      # !p、!ps、!r、!rs、!s、!ss
profile = true    # !profile
highlight = true  # 今日高光
bind = true       # 绑定、解绑

# 特定群覆盖（节名 = 群号）
[groups.123456789]
highlight = false  # 该群禁用今日高光
```

### 插件配置

WASM 插件目录和实例配置：

```toml
[plugin]
dir = "./plugins"

[[plugin.instances]]
name = "my-plugin"
path = "my_plugin.wasm"
enabled = false     # 默认禁用，需显式启用
priority = 50       # 优先级（数字越大越先执行）
```

### 超时配置

`[bot]` 段支持以下超时字段（均可热重载）：

```toml
[bot]
# command_timeout_secs = 120   # 命令处理超时（秒）
# render_timeout_secs = 30     # 渲染超时（秒）
# onebot_api_timeout_secs = 5  # OneBot API 请求超时（秒）
# ur_timeout_secs = 10         # UR 计算超时（秒）
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
- **存储**: Turso (libsql)，存储用户绑定、数据快照和游玩记录
- **WebSocket**: tokio-tungstenite 连接 OneBot 11 正向 WebSocket
- **API**: osu! API v2，OAuth client credentials 认证
- **PP 计算**: rosu-pp v4，支持 PP 分解（aim/speed/acc/flashlight/difficulty）、准确率推测（95%~100% + IF FC）、转换谱面星级计算（osu! → taiko/catch/mania）、NF/CL 快速路径
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
│       ├── last_beatmap_cache.rs # 群谱面查询缓存
│       ├── reload.rs    # 热重载（文件监控 + drain + MutableConfig）
│       ├── scheduler.rs # 后台定时更新调度器
│       ├── xfs_upstream.rs # 消防栓上游绑定查询
│       └── yumu_upstream.rs # yumu 上游绑定查询
├── osubot-core/        # 核心库
│   └── src/
│       ├── commands.rs  # 命令解析
│       ├── api.rs       # osu! API v2 调用 + OAuth 缓存
│       ├── storage.rs   # Turso/libsql 存储
│       ├── response.rs  # 响应格式化
│       ├── highlight.rs # 今日高光业务逻辑
│       ├── ur.rs        # 回放解析 + UR 计算
│       ├── dedup.rs     # 请求去重
│       ├── rate_limiter.rs # 令牌桶限流
│       ├── upstream.rs    # 上游绑定抽象层
│       ├── cache.rs       # replay/beatmap 文件缓存
│       ├── irc.rs       # IRC 连接与消息监听
│       └── types.rs     # 数据类型定义
├── osubot-render/      # 渲染引擎（个人主页 + 分数卡片 + 成绩列表）
│   ├── src/
│   │   ├── lib.rs       # 渲染入口与编排
│   │   ├── render.rs    # HTML → 位图渲染 (Blitz + Vello CPU)
│   │   ├── score_style.rs # 分数卡片 HTML 模板
│   │   ├── score_list_style.rs # 成绩列表卡片 HTML 模板
│   │   ├── style.rs     # 个人主页 CSS 注入
│   │   ├── cache.rs     # 图片缓存 + SVG 光栅化
│   │   ├── encode.rs    # JPEG 编码
│   │   └── error.rs     # 错误类型
│   └── styles/
│       ├── score.css    # 分数卡片样式（MD3）
│       ├── score_list.css # 成绩列表卡片样式
│       └── profile.css  # 个人主页样式
├── osubot-plugin/      # WASM 插件运行时
│   └── src/
│       ├── lib.rs       # PluginManager（加载/调度/热重载）
│       ├── bridge.rs    # 宿主函数（HostServices + 7 种宿主调用分发）
│       ├── instance.rs  # PluginInstance（wasmtime 封装）
│       ├── config.rs    # 插件配置 TOML 反序列化
│       └── types.rs     # PluginAction/PluginMetadata
├── osubot-plugin-sdk/  # WASM 插件 SDK（编译到 wasm32-unknown-unknown 或 wasm32-wasip1，无 WASI 依赖），供插件作者使用
│   └── src/
│       ├── lib.rs       # 宿主调用封装 + alloc/dealloc
│       └── types.rs     # PluginMetadata、PluginAction、Command
├── osubot-types/       # 共享类型（Score、GameMode）
├── examples/
│   └── hello-plugin/   # 示例插件
└── docs/
    └── plugin-development.md # 插件开发文档
```

### 调度器

后台调度器根据用户游玩记录动态调整更新频率：

| 活跃度 | 判断依据 | 更新间隔 |
|--------|----------|----------|
| 半活跃 | 4 小时内有游玩记录 | 4 小时 |
| 普通 | 4~48 小时内有游玩记录（含当日有记录但 4h 内无） | 8 小时 |
| 无最近 | 4h 内无游玩、当日无游玩，但 8h 内有最近活动 | 6 小时 |
| 不活跃 | 48 小时以上无活动 | 48 小时 |
| 用户不存在 | API 返回 NotFound | 24 小时 |

调度器通过 osu! API 的 `/users/{id}/scores/recent` 接口获取玩家最近游玩记录，写入数据库。仅在 rank 或 playcount 与前一个快照不同时保存数据快照，避免了无变化时的冗余写入。群内手动查询会触发 `trigger_update`（1 小时冷却），确保交互后及时刷新数据。

### 排名变化

查询时会对比约 24 小时前的快照，显示 pp、排名、准确率、游玩次数等各项变化。

### 请求去重与限流

并发的相同请求（如多人同时查询同一用户）通过 `RequestDedup` 只执行一次 API 调用。所有 osu! API 请求经过令牌桶限流（60 突发/1 每秒），防止触发 API 速率限制。上游绑定查询也经过独立令牌桶限流（默认 10/分钟），防止频繁连接上游服务器。

## 插件系统

osubot 支持通过 WASM 动态加载插件扩展功能。每个插件编译为独立的 `.wasm` 文件，通过 wasmtime 运行，与主程序完全隔离——崩溃、死循环、内存越界均不影响主程序运行。

### 文档

插件开发指南见 [`docs/plugin-development.md`](docs/plugin-development.md)，包含完整的 SDK 接口参考、类型定义、开发流程和版本兼容性保证。

### 事件调度

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

### 热重载

- 配置文件（`osubot.toml`）或 `.wasm` 文件变更时自动热重载
- `notify` 文件监控 + 500ms 防抖，避免频繁触发
- 热重载时等待进行中的命令完成（drain 机制，最长 30 秒），然后原子切换状态
- 插件实例重建，内存状态丢失（建议通过宿主函数持久化数据）

### 故障隔离

- 每个插件调用有 10 秒超时（tokio::timeout），epoch 中断作为 30 秒最后防线
- 内存上限 100MB（wasmtime StoreLimits）
- 连续 5 次错误/超时/panic 自动重载插件实例

### 快速开始

```bash
# 安装 WASM 目标（SDK 无 WASI 依赖，两个目标均可，推荐 wasm32-unknown-unknown）
rustup target add wasm32-unknown-unknown

# 依赖 SDK
cargo add osubot-plugin-sdk --git https://github.com/ZnCookie/osubot-rs

# 编译为 .wasm
cargo build --target wasm32-unknown-unknown --release
```

### 示例

`examples/hello-plugin/` 是一个完整的示例插件，响应 `!hello`、`!ping` 命令，演示 `on_load`、`on_message`、`on_tick`、`on_unload` 生命周期钩子的使用。

## 许可

本项目采用 GNU Affero General Public License v3.0 (AGPL-3.0) 许可。
