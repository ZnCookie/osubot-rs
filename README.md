# osubot-rs

通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。

## 功能

查询 osu! 数据、绑定账号、分数卡片渲染（含 PP 分解/UR/准确率推测）、谱面成绩查询、个人主页卡片、今日高光、插件系统。

## 命令

| 命令 | 说明 |
|------|------|
| `~` / `~0`…`~3` | 查询自己 |
| `where <用户名>[,模式]` / `where @<QQ号>[,模式]` | 查询指定用户 |
| `绑定 <osu用户名>` / `解绑` | 绑定/解绑 |
| `今日高光[,模式]` | 当日高光 |
| `!p` / `!r` / `!ps` / `!rs` | 最近成绩查询 |
| `!s` / `!ss` | 谱面成绩查询 |
| `!profile` | 个人主页卡片 |

详细语法、过滤、示例见 [`docs/commands.md`](docs/commands.md)。

## 配置

```bash
cp osubot.example.toml osubot.toml
```

osu! API v2 凭据在 [osu! 设置](https://osu.ppy.sh/home/account/edit#oauth) 创建 OAuth 应用获取。IRC 鉴权、群黑白名单、群命令开关、超时等配置说明见 `osubot.example.toml`。

## 安装

### 用户

发行包自带 C 运行时库，只需安装字体：

```bash
# Arch Linux
sudo pacman -S noto-fonts noto-fonts-cjk noto-fonts-emoji
```

### 编译

```bash
sudo pacman -S librsvg cairo glib2 pango pkgconf  # Arch Linux 编译依赖
cargo run --release
```

## 技术栈

Rust + Tokio + Turso + Blitz/Vello 渲染 + WASM 插件系统。

## 项目结构

```
osubot-rs/
├── osubot/              # 主程序
├── osubot-core/         # 核心库（API/命令解析/响应格式化/调度/IRC）
├── osubot-render/       # 渲染引擎（HTML → 位图）
├── osubot-plugin/       # WASM 插件运行时
├── osubot-plugin-sdk/   # 插件 SDK
├── osubot-types/        # 共享类型
└── examples/hello-plugin/
```

## 许可

AGPL-3.0
