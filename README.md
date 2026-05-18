# osubot-rs

通过 OneBot 11 协议查询 osu! 玩家数据的 QQ 机器人。

## 功能

- 查询自己/他人的 osu! 数据（pp、排名、国家排名、准确率、游玩次数等）
- 支持 4 种模式：std、taiko、catch、mania
- 绑定 osu! 账号后自动追踪排名变化
- 后台定时更新，活跃玩家更新更频繁

## 命令

| 命令 | 说明 |
|------|------|
| `~` | 查询自己 std |
| `~1` / `~2` / `~3` | 查询自己 taiko / catch / mania |
| `where <用户名>` | 查询他人 std |
| `where <用户名>,3` | 查询他人 mania |
| `查@<QQ用户>` | 查询他人 std |
| `绑定 <osu用户名>` | 绑定 osu! 账号 |
| `解绑` | 解除绑定 |

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
