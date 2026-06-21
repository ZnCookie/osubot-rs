# osubot-rs

通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。

## 功能

查询 osu! 数据、绑定账号、分数卡片渲染（含 PP 分解/UR/准确率推测）、谱面成绩查询、个人主页卡片、今日高光、插件系统等。

## 命令

详细语法、过滤、示例见 [`docs/commands.md`](docs/commands.md)。

## 配置

```bash
cp osubot.example.toml osubot.toml
```

osu! API v2 凭据在 [osu! 设置](https://osu.ppy.sh/home/account/edit#oauth) 创建 OAuth 应用获取。

IRC 鉴权、群黑白名单、群命令开关、超时等配置说明见 `osubot.example.toml`。

也支持通过环境变量配置（优先级低于配置文件）：`OSU_CLIENT_SECRET`/`OSU_API_KEY`、`OSU_CLIENT_ID`、`ONEBOT_URL`、`DATABASE_PATH`。

## 安装

### 从Github Action下载

发行包自带 C 运行时库，只需安装字体：

```bash
# Arch Linux
sudo pacman -S noto-fonts noto-fonts-cjk noto-fonts-emoji
```

### 从源代码编译

除了安装字体，你还需要安装依赖：

```bash
sudo pacman -S librsvg cairo glib2 pango pkgconf  # Arch Linux 编译依赖
cargo run --release
```

## 许可

AGPL-3.0
