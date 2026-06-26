# 数据库 Schema

osubot 使用 SQLite（通过 turso/libsql）存储数据。数据库文件路径在 `osubot.toml` 的 `[database]` 中配置。

## 表结构

### `user_bindings`

QQ 号与 osu! 账号的绑定关系。

| 列 | 类型 | 说明 |
|----|------|------|
| `qq` | `INTEGER PRIMARY KEY` | QQ 号 |
| `user_id` | `INTEGER NOT NULL` | osu! 用户 ID |
| `current_username` | `TEXT NOT NULL` | 当前 osu! 用户名 |
| `default_mode` | `INTEGER NOT NULL DEFAULT 0` | 默认游戏模式（0=osu! 1=taiko 2=catch 3=mania） |
| `created_at` | `TEXT DEFAULT CURRENT_TIMESTAMP` | 绑定时间 |

索引：`idx_bindings_username ON user_bindings(LOWER(current_username))`（大小写不敏感用户名查询）

### `osu_user_ids`

用户名到 osu! 用户 ID 的缓存映射，由调度器在拉取数据时自动维护。

| 列 | 类型 | 说明 |
|----|------|------|
| `username` | `TEXT PRIMARY KEY` | osu! 用户名 |
| `user_id` | `INTEGER NOT NULL` | osu! 用户 ID |

### `user_stats_history`

用户统计数据的历史快照，由调度器定期保存。

| 列 | 类型 | 说明 |
|----|------|------|
| `id` | `INTEGER PRIMARY KEY AUTOINCREMENT` | 自增 ID |
| `user_id` | `INTEGER NOT NULL` | osu! 用户 ID |
| `mode` | `INTEGER NOT NULL` | 游戏模式 |
| `recorded_at` | `TEXT DEFAULT CURRENT_TIMESTAMP` | 记录时间 |
| `pp` | `REAL` | PP 值 |
| `rank` | `INTEGER` | 全球排名 |
| `country_rank` | `INTEGER` | 国家排名 |
| `ranked_score` | `INTEGER` | Ranked 总分 |
| `accuracy` | `REAL` | 准确率 |
| `playcount` | `INTEGER` | 游玩次数 |
| `hits` | `INTEGER` | 总 hit 数 |
| `playtime` | `INTEGER` | 总游玩时间（秒） |

唯一约束：`UNIQUE(user_id, mode, recorded_at)`

索引：
- `idx_history_user ON user_stats_history(user_id, mode)`
- `idx_history_recorded ON user_stats_history(recorded_at)`

> 调度器仅在 rank 或 playcount 发生变化时保存新快照，避免冗余数据。

### `user_play_records`

用户最近游玩记录的时间戳，用于活跃度判定。

| 列 | 类型 | 说明 |
|----|------|------|
| `id` | `INTEGER PRIMARY KEY AUTOINCREMENT` | 自增 ID |
| `user_id` | `INTEGER NOT NULL` | osu! 用户 ID |
| `mode` | `INTEGER NOT NULL` | 游戏模式 |
| `played_at` | `INTEGER NOT NULL` | 游玩时间（Unix 时间戳） |

唯一约束：`UNIQUE(user_id, mode, played_at)`

索引：`idx_play_records_user ON user_play_records(user_id, mode)`

### `user_next_update`

调度器用于跟踪每个用户/模式对的下次更新时间。

| 列 | 类型 | 说明 |
|----|------|------|
| `user_id` | `INTEGER NOT NULL` | osu! 用户 ID |
| `mode` | `INTEGER NOT NULL` | 游戏模式 |
| `next_update` | `INTEGER NOT NULL` | 下次更新时间（Unix 时间戳） |

主键：`(user_id, mode)`

### `user_last_update`

记录每个用户/模式对的最后更新时间，用于活跃度判定和冷却时间计算。

| 列 | 类型 | 说明 |
|----|------|------|
| `user_id` | `INTEGER NOT NULL` | osu! 用户 ID |
| `mode` | `INTEGER NOT NULL` | 游戏模式 |
| `last_update` | `TEXT NOT NULL` | 最后更新时间 |

主键：`(user_id, mode)`

### `pending_unbind`

等待二次确认的解绑请求。

| 列 | 类型 | 说明 |
|----|------|------|
| `qq` | `INTEGER PRIMARY KEY` | QQ 号 |
| `created_at` | `TEXT DEFAULT CURRENT_TIMESTAMP` | 请求时间 |

过期的请求由调度器每 24 小时清理。

### `pending_binds`

等待验证码确认的绑定请求（IRC 鉴权模式）。

| 列 | 类型 | 说明 |
|----|------|------|
| `code` | `TEXT PRIMARY KEY` | 验证码 |
| `qq_user_id` | `INTEGER NOT NULL` | 请求绑定的 QQ 号 |
| `group_id` | `INTEGER NOT NULL` | 请求来源群号 |
| `target_username` | `TEXT NOT NULL` | 目标 osu! 用户名 |
| `created_at` | `TEXT NOT NULL` | 请求时间 |
| `expires_at` | `INTEGER NOT NULL` | 过期时间（Unix 时间戳） |

过期的请求由调度器每 24 小时清理。

## 迁移

数据库使用内联迁移（在 `storage.rs` 中通过 `CREATE TABLE IF NOT EXISTS` 和 `ALTER TABLE` 实现），无需单独的迁移文件。启动时会自动创建表和索引，对已有数据库执行必要的列添加。
