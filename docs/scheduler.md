# 调度器

调度器是 osubot 的后台任务系统，负责定期更新已绑定用户的 osu! 数据。

## 工作原理

调度器每 `interval_minutes`（默认 1 分钟）执行一次循环，检查所有绑定用户的 `next_update` 时间戳，处理到期的用户。

### 单次循环流程

1. 查询 `user_next_update` 表中 `next_update <= now` 的用户
2. 并发评估 5 个用户（`buffer_unordered(5)`）
3. 评估完成后设置新的 `next_update` 时间
4. 每 24 小时执行一次清理

### 用户评估

对每个到期的 (user_id, mode) 对：

1. **检查限流**：等待 osu! API 限流配额
2. **拉取数据**：调用 osu! API 获取最新统计数据和最近游玩记录
3. **改名检测**：若用户名发生变化，自动更新 `user_bindings` 表
4. **快照保存**：仅当 rank 或 playcount 与上次快照不同时保存（避免冗余数据）
5. **游玩记录**：保存最近 100 条游玩记录的时间戳

## 活跃度分级

调度器根据用户游玩活跃度动态调整更新频率：

| 活跃度 | 条件 | 更新间隔 |
|--------|------|----------|
| `SemiActive` | 4 小时内有游玩 | `semi_active_interval_hours`（默认 4h） |
| `Normal` | 当天有游玩 | `normal_interval_hours`（默认 8h） |
| `NoRecent` | 距上次更新 < 8h（短间隔重试） | `no_recent_interval_hours`（默认 6h） |
| `Inactive` | 超过 48 小时无游玩 | `inactive_interval_hours`（默认 48h） |
| `UserNotExists` | osu! API 返回 404 | `user_not_exists_interval_hours`（默认 24h） |

### 活跃度判定逻辑

```
有 4h 内游玩 → SemiActive
有今日游玩 → Normal
无今日游玩：
  距上次更新 < 8h → NoRecent（短间隔重试）
  距上次更新 8-48h → Normal
  距上次更新 > 48h → Inactive
```

## 触发更新

除了定时轮询，以下场景会触发即时更新：

- **用户首次查询**：绑定后首次查询会将 `next_update` 设为 now
- **群内查询**：查询命令会触发该用户的调度更新（有冷却时间 `group_trigger_cooldown_hours`，默认 1h）

## 配置

所有字段均有默认值，可选配置：

```toml
[scheduler]
interval_minutes = 1                  # 轮询间隔（分钟）
semi_active_interval_hours = 4        # SemiActive 用户更新间隔
normal_interval_hours = 8             # Normal 用户更新间隔
inactive_interval_hours = 48          # Inactive 用户更新间隔
no_recent_interval_hours = 6          # NoRecent 用户更新间隔（短重试）
user_not_exists_interval_hours = 24   # 用户不存在时的重试间隔
group_trigger_cooldown_hours = 1      # 群内查询触发更新的冷却时间
retention_days = 180                  # 统计快照保留天数
cache_retention_days = 7              # 缓存文件保留天数
# max_cache_size_bytes = 2147483648    # 渲染缓存最大大小（字节），不配则默认 2GB
```

## 清理机制

每 24 小时执行一次清理：

- **统计快照**：删除超过 `retention_days` 天的历史快照
- **游玩记录**：删除超过 `retention_days` 天的游玩记录
- **孤立调度记录**：删除已解绑用户的 `user_next_update` 行
- **过期绑定请求**：清理已过期的 `pending_binds` 和 `pending_unbinds`
- **缓存文件**：清理超过 `cache_retention_days` 天的渲染缓存、谱面文件、预览图、音频缓存
- **缓存大小**：若渲染缓存超过 `max_cache_size_bytes`，按时间从旧到新删除

## 配置热重载

修改 `osubot.toml` 后调度器会自动重置所有用户的 `next_update` 时间戳（`reschedule_all`），确保新配置立即生效。
