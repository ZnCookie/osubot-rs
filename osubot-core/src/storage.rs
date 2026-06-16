use crate::log_fmt;
use chrono::{DateTime, Local, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use turso::{params, Connection, Database, Result as DbResult};

use crate::types::{GameMode, UserChange, UserStats};

/// Returns UTC timestamp of today's 0:00 AM in local timezone
pub fn today_0am_utc() -> i64 {
    let local_now = chrono::Local::now();
    let today_local = local_now.date_naive();
    let today_0am_local = today_local
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 is always a valid NaiveTime");
    // .single() returns None when midnight doesn't exist (DST spring-forward)
    // or is ambiguous (DST fall-back); fall back to 1:00 AM in that case.
    let dt = Local
        .from_local_datetime(&today_0am_local)
        .single()
        .unwrap_or_else(|| {
            let fallback = today_0am_local + chrono::TimeDelta::hours(1);
            Local
                .from_local_datetime(&fallback)
                .earliest()
                .unwrap_or_else(Local::now)
        });
    dt.with_timezone(&Utc).timestamp()
}

pub struct Storage {
    pool: Vec<tokio::sync::Mutex<Connection>>,
    next: AtomicUsize,
    #[allow(dead_code)]
    db: Database,
}

impl Storage {
    pub async fn new(path: &str) -> DbResult<Self> {
        let db = turso::Builder::new_local(path).build().await?;
        const POOL_SIZE: usize = 8;
        let mut pool = Vec::with_capacity(POOL_SIZE);
        for _ in 0..POOL_SIZE {
            let conn = db.connect()?;
            conn.busy_timeout(Duration::from_secs(5))?;
            pool.push(tokio::sync::Mutex::new(conn));
        }

        pool[0]
            .lock()
            .await
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS user_bindings (
                qq INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                current_username TEXT NOT NULL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS user_stats_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                recorded_at TEXT DEFAULT CURRENT_TIMESTAMP,
                pp REAL,
                rank INTEGER,
                country_rank INTEGER,
                ranked_score INTEGER,
                accuracy REAL,
                playcount INTEGER,
                hits INTEGER,
                playtime INTEGER,
                UNIQUE(user_id, mode, recorded_at)
            );
            CREATE TABLE IF NOT EXISTS user_play_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                played_at INTEGER NOT NULL,
                UNIQUE(user_id, mode, played_at)
            );
            CREATE TABLE IF NOT EXISTS user_next_update (
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                next_update INTEGER NOT NULL,
                PRIMARY KEY(user_id, mode)
            );
            CREATE TABLE IF NOT EXISTS user_last_update (
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                last_update TEXT NOT NULL,
                PRIMARY KEY(user_id, mode)
            );
            CREATE TABLE IF NOT EXISTS pending_unbind (
                qq INTEGER PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS pending_binds (
                code TEXT PRIMARY KEY,
                qq_user_id INTEGER NOT NULL,
                group_id INTEGER NOT NULL,
                target_username TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS osu_user_ids (
                username TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_history_user ON user_stats_history(user_id, mode);
            CREATE INDEX IF NOT EXISTS idx_history_recorded ON user_stats_history(recorded_at);
            CREATE INDEX IF NOT EXISTS idx_play_records_user ON user_play_records(user_id, mode);",
            )
            .await?;

        Ok(Self {
            db,
            pool,
            next: AtomicUsize::new(0),
        })
    }

    async fn conn(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.pool.len();
        self.pool[idx].lock().await
    }

    // ==================== Binding Query ====================

    /// Bind QQ to user_id with current_username. Returns Err if user_id already bound to another QQ.
    pub async fn bind(
        &self,
        qq: i64,
        user_id: i64,
        current_username: &str,
    ) -> DbResult<std::result::Result<(), i64>> {
        let conn = self.conn().await;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result = async {
            let mut rows = conn
                .query(
                    "SELECT qq FROM user_bindings WHERE user_id = ?1",
                    params![user_id],
                )
                .await?;
            if let Some(row) = rows.next().await? {
                let existing_qq: i64 = row.get(0)?;
                if existing_qq != qq {
                    return Ok(Err(existing_qq));
                }
            }
            drop(rows);
            conn.execute(
                    "INSERT OR REPLACE INTO user_bindings (qq, user_id, current_username) VALUES (?1, ?2, ?3)",
                    params![qq, user_id, current_username],
                )
                .await?;
            Ok(Ok(()))
        }
        .await;
        match &result {
            Ok(Ok(())) | Ok(Err(_)) => {
                conn.execute("COMMIT", ()).await?;
            }
            _ => {
                conn.execute("ROLLBACK", ()).await?;
            }
        }
        result
    }

    pub async fn unbind(&self, qq: i64) -> DbResult<()> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id FROM user_bindings WHERE qq = ?1",
                params![qq],
            )
            .await?;
        let user_id: Option<i64> = rows.next().await?.map(|row| row.get(0)).transpose()?;

        conn.execute("DELETE FROM user_bindings WHERE qq = ?1", params![qq])
            .await?;

        if let Some(uid) = user_id {
            let mut rows = conn
                .query(
                    "SELECT COUNT(*) FROM user_bindings WHERE user_id = ?1",
                    params![uid],
                )
                .await?;
            if let Some(row) = rows.next().await? {
                let count: i64 = row.get(0)?;
                if count == 0 {
                    conn.execute(
                        "DELETE FROM user_next_update WHERE user_id = ?1",
                        params![uid],
                    )
                    .await?;
                }
            }
        }

        Ok(())
    }

    pub async fn set_user_id(&self, username: &str, user_id: i64) -> DbResult<()> {
        self.conn()
            .await
            .execute(
                "INSERT OR REPLACE INTO osu_user_ids (username, user_id) VALUES (LOWER(?1), ?2)",
                params![username, user_id],
            )
            .await?;
        Ok(())
    }

    /// Get cached osu! user ID (case-insensitive username lookup)
    pub async fn get_user_id(&self, username: &str) -> DbResult<Option<i64>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id FROM osu_user_ids WHERE LOWER(username) = LOWER(?1)",
                params![username],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Bound usernames that don't have a cached user_id yet
    pub async fn get_users_without_ids(&self) -> DbResult<Vec<String>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT b.current_username FROM user_bindings b
                 WHERE NOT EXISTS (
                     SELECT 1 FROM osu_user_ids o WHERE LOWER(o.username) = LOWER(b.current_username)
                 )",
                (),
            )
            .await?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().await? {
            result.push(row.get(0)?);
        }
        Ok(result)
    }

    pub async fn get_binding(&self, qq: i64) -> DbResult<Option<(i64, String)>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id, current_username FROM user_bindings WHERE qq = ?1",
                params![qq],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub async fn update_binding_username(&self, qq: i64, new_username: &str) -> DbResult<()> {
        self.conn()
            .await
            .execute(
                "UPDATE user_bindings SET current_username = ?1 WHERE qq = ?2",
                params![new_username, qq],
            )
            .await?;
        Ok(())
    }

    /// Update current_username for all QQs bound to a given user_id (username change detection).
    /// Returns the number of bindings updated.
    pub async fn update_binding_username_by_user_id(
        &self,
        user_id: i64,
        new_username: &str,
    ) -> DbResult<u64> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT current_username FROM user_bindings WHERE user_id = ?1 LIMIT 1",
                params![user_id],
            )
            .await?;
        let current: Option<String> = rows.next().await?.map(|row| row.get(0)).transpose()?;
        if current.as_deref() == Some(new_username) {
            return Ok(0);
        }
        let count = conn
            .execute(
                "UPDATE user_bindings SET current_username = ?1 WHERE user_id = ?2",
                params![new_username, user_id],
            )
            .await?;
        Ok(count)
    }

    // ==================== Snapshot Operations ====================

    pub async fn save_stats(
        &self,
        user_id: i64,
        mode: GameMode,
        stats: &UserStats,
    ) -> DbResult<()> {
        self.conn().await
            .execute(
                "INSERT OR IGNORE INTO user_stats_history (user_id, mode, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    user_id,
                    mode as i32,
                    Utc::now().to_rfc3339(),
                    stats.pp,
                    stats.rank,
                    stats.country_rank,
                    stats.ranked_score,
                    stats.accuracy,
                    stats.playcount,
                    stats.hits,
                    stats.playtime,
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn get_latest_snapshot(
        &self,
        user_id: i64,
        mode: GameMode,
    ) -> DbResult<Option<UserStats>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
                 FROM user_stats_history
                 WHERE user_id = ?1 AND mode = ?2
                 ORDER BY recorded_at DESC
                 LIMIT 1",
                params![user_id, mode as i32],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(UserStats {
                user_id: 0,
                username: String::new(),
                pp: row.get(0)?,
                rank: row.get(1)?,
                country_rank: row.get(2)?,
                country_code: "XX".to_string(),
                ranked_score: row.get(3)?,
                accuracy: row.get(4)?,
                playcount: row.get(5)?,
                hits: row.get(6)?,
                playtime: row.get(7)?,
                rank_change: None,
                country_rank_change: None,
                cover_url: None,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn get_snapshots_within_hours(
        &self,
        user_id: i64,
        mode: GameMode,
        hours: i64,
    ) -> DbResult<Vec<(DateTime<Utc>, UserStats)>> {
        let cutoff = Utc::now() - chrono::TimeDelta::hours(hours);
        let cutoff_str = cutoff.to_rfc3339();

        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
                 FROM user_stats_history
                 WHERE user_id = ?1 AND mode = ?2 AND recorded_at >= ?3
                 ORDER BY recorded_at ASC",
                params![user_id, mode as i32, cutoff_str],
            )
            .await?;

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let recorded_str: String = row.get(0)?;
            if let Ok(dt) = DateTime::parse_from_rfc3339(&recorded_str) {
                results.push((
                    dt.with_timezone(&Utc),
                    UserStats {
                        user_id: 0,
                        username: String::new(),
                        pp: row.get(1)?,
                        rank: row.get(2)?,
                        country_rank: row.get(3)?,
                        country_code: "XX".to_string(),
                        ranked_score: row.get(4)?,
                        accuracy: row.get(5)?,
                        playcount: row.get(6)?,
                        hits: row.get(7)?,
                        playtime: row.get(8)?,
                        rank_change: None,
                        country_rank_change: None,
                        cover_url: None,
                    },
                ));
            }
        }
        Ok(results)
    }

    pub async fn get_baseline_snapshots_for_users(
        &self,
        user_ids: &[i64],
        mode: GameMode,
        target_hours_ago: i64,
        max_lookback: i64,
    ) -> DbResult<HashMap<i64, UserStats>> {
        let unique_user_ids: Vec<i64> = user_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if unique_user_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let now = Utc::now();
        let target = now - chrono::TimeDelta::hours(target_hours_ago);
        let cutoff = now - chrono::TimeDelta::hours(max_lookback);
        let cutoff_str = cutoff.to_rfc3339();
        let placeholders = std::iter::repeat_n("?", unique_user_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT user_id, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
             FROM user_stats_history
             WHERE mode = ? AND recorded_at >= ? AND user_id IN ({placeholders})
             ORDER BY user_id, recorded_at ASC"
        );

        let mut args: Vec<turso::Value> = Vec::with_capacity(unique_user_ids.len() + 2);
        args.push((mode as i32).into());
        args.push(cutoff_str.into());
        for user_id in unique_user_ids {
            args.push(user_id.into());
        }

        let conn = self.conn().await;
        let mut rows = conn.query(&sql, args).await?;
        let mut closest: HashMap<i64, (u64, UserStats)> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let user_id: i64 = row.get(0)?;
            let recorded_str: String = row.get(1)?;
            let Ok(recorded_at) = DateTime::parse_from_rfc3339(&recorded_str) else {
                continue;
            };
            let distance = (recorded_at.with_timezone(&Utc) - target)
                .num_seconds()
                .unsigned_abs();
            let stats = UserStats {
                user_id: 0,
                username: String::new(),
                pp: row.get(2)?,
                rank: row.get(3)?,
                country_rank: row.get(4)?,
                country_code: "XX".to_string(),
                ranked_score: row.get(5)?,
                accuracy: row.get(6)?,
                playcount: row.get(7)?,
                hits: row.get(8)?,
                playtime: row.get(9)?,
                rank_change: None,
                country_rank_change: None,
                cover_url: None,
            };

            match closest.get(&user_id) {
                Some((best_distance, _)) if *best_distance <= distance => {}
                _ => {
                    closest.insert(user_id, (distance, stats));
                }
            }
        }

        Ok(closest
            .into_iter()
            .map(|(user_id, (_, stats))| (user_id, stats))
            .collect())
    }

    pub async fn get_baseline_snapshot(
        &self,
        user_id: i64,
        mode: GameMode,
    ) -> DbResult<Option<UserStats>> {
        let all = self.get_snapshots_within_hours(user_id, mode, 36).await?;
        if all.is_empty() {
            return Ok(None);
        }
        let now = Utc::now();
        let target = now - chrono::TimeDelta::hours(24);
        Ok(all
            .into_iter()
            .min_by_key(|(dt, _)| (*dt - target).num_seconds().unsigned_abs())
            .map(|(_, stats)| stats))
    }

    pub async fn get_closest_snapshot_to_hours_ago(
        &self,
        user_id: i64,
        mode: GameMode,
        target_hours_ago: i64,
        max_lookback: i64,
    ) -> DbResult<Option<(DateTime<Utc>, UserStats)>> {
        let now = Utc::now();
        let target_time = now - chrono::TimeDelta::hours(target_hours_ago);
        let earliest = now - chrono::TimeDelta::hours(max_lookback);

        let all = self
            .get_snapshots_within_hours(user_id, mode, max_lookback)
            .await?;

        let candidates: Vec<_> = all.into_iter().filter(|(dt, _)| *dt >= earliest).collect();

        if candidates.is_empty() {
            return Ok(None);
        }

        // SAFETY: candidates is non-empty (guarded by is_empty() check above)
        Ok(Some(
            candidates
                .into_iter()
                .min_by_key(|(dt, _)| (*dt - target_time).num_seconds().unsigned_abs() as i64)
                .unwrap(),
        ))
    }

    // ==================== Play Records Operations ====================

    pub async fn save_play_records(
        &self,
        user_id: i64,
        mode: GameMode,
        timestamps: &[i64],
    ) -> DbResult<i64> {
        let mut inserted: i64 = 0;
        let conn = self.conn().await;
        for &timestamp in timestamps {
            let count = conn
                .execute(
                    "INSERT OR IGNORE INTO user_play_records (user_id, mode, played_at) VALUES (?1, ?2, ?3)",
                    params![user_id, mode as i32, timestamp],
                )
                .await?;
            inserted += count as i64;
        }
        Ok(inserted)
    }

    /// Check if user has any play records since the given UTC timestamp
    pub async fn has_play_since(
        &self,
        user_id: i64,
        mode: GameMode,
        since_ts: i64,
    ) -> DbResult<bool> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM user_play_records WHERE user_id = ?1 AND mode = ?2 AND played_at >= ?3 LIMIT 1",
                params![user_id, mode as i32, since_ts],
            )
            .await?;
        Ok(rows.next().await?.is_some())
    }

    // ==================== Change Calculation ====================

    pub async fn calculate_change(
        &self,
        user_id: i64,
        mode: GameMode,
        current: &UserStats,
    ) -> DbResult<Option<UserChange>> {
        let snapshot = self
            .get_closest_snapshot_to_hours_ago(user_id, mode, 24, 36)
            .await?;

        match snapshot {
            None => Ok(None),
            Some((_, past)) => {
                let rank_change = if current.rank != 0 && past.rank != 0 {
                    Some(past.rank - current.rank)
                } else {
                    None
                };
                let country_rank_change = if current.country_rank != 0 && past.country_rank != 0 {
                    Some(past.country_rank - current.country_rank)
                } else {
                    None
                };
                let playcount_change = if current.playcount != 0 && past.playcount != 0 {
                    Some(current.playcount - past.playcount)
                } else {
                    None
                };
                let hits_change = if current.hits != 0 && past.hits != 0 {
                    Some(current.hits - past.hits)
                } else {
                    None
                };
                let playtime_change = if current.playtime != 0 && past.playtime != 0 {
                    Some(current.playtime - past.playtime)
                } else {
                    None
                };

                Ok(Some(UserChange {
                    rank_change,
                    country_rank_change,
                    pp_change: Some(current.pp - past.pp),
                    accuracy_change: Some(current.accuracy - past.accuracy),
                    playcount_change,
                    hits_change,
                    playtime_change,
                }))
            }
        }
    }

    // ==================== User Activity (Last Update Time) ====================

    pub async fn get_last_update(
        &self,
        user_id: i64,
        mode: GameMode,
    ) -> DbResult<Option<DateTime<Utc>>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT last_update FROM user_last_update WHERE user_id = ?1 AND mode = ?2",
                params![user_id, mode as i32],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let last_update_str: String = row.get(0)?;
            if let Ok(dt) = DateTime::parse_from_rfc3339(&last_update_str) {
                return Ok(Some(dt.with_timezone(&Utc)));
            }
        }
        Ok(None)
    }

    pub async fn set_last_update(
        &self,
        user_id: i64,
        mode: GameMode,
        time: DateTime<Utc>,
    ) -> DbResult<()> {
        self.conn().await
            .execute(
                "INSERT OR REPLACE INTO user_last_update (user_id, mode, last_update) VALUES (?1, ?2, ?3)",
                params![user_id, mode as i32, time.to_rfc3339()],
            )
            .await?;
        Ok(())
    }

    // ==================== Next Update (for scheduler dynamic intervals) ====================

    pub async fn get_next_update(
        &self,
        user_id: i64,
        mode: GameMode,
    ) -> DbResult<Option<DateTime<Utc>>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT next_update FROM user_next_update WHERE user_id = ?1 AND mode = ?2",
                params![user_id, mode as i32],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let ts: i64 = row.get(0)?;
            return Ok(Some(Utc.timestamp_opt(ts, 0).single().unwrap_or_else(
                || {
                    tracing::warn!(ts, "{}", log_fmt!("storage.parse_next_update_failed"));
                    Utc::now()
                },
            )));
        }
        Ok(None)
    }

    pub async fn set_next_update(
        &self,
        user_id: i64,
        mode: GameMode,
        time: DateTime<Utc>,
    ) -> DbResult<()> {
        self.conn().await
            .execute(
                "INSERT OR REPLACE INTO user_next_update (user_id, mode, next_update) VALUES (?1, ?2, ?3)",
                params![user_id, mode as i32, time.timestamp()],
            )
            .await?;
        Ok(())
    }

    /// Reset all user next_update timestamps to now, so they are re-evaluated
    /// on the next scheduler tick with the new config intervals.
    pub async fn reset_all_next_updates(&self) -> DbResult<u64> {
        let now_ts = Utc::now().timestamp();
        self.conn()
            .await
            .execute(
                "UPDATE user_next_update SET next_update = ?1",
                params![now_ts],
            )
            .await
    }

    /// Get all user bindings (qq -> user_id, current_username mappings)
    pub async fn get_all_user_bindings(&self) -> DbResult<Vec<(i64, i64, String)>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT qq, user_id, current_username FROM user_bindings",
                (),
            )
            .await?;
        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            results.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(results)
    }

    // ==================== Due Users Query ====================

    pub async fn get_due_users(&self) -> DbResult<Vec<(i64, GameMode)>> {
        let now_ts = Utc::now().timestamp();

        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id, mode FROM user_next_update WHERE next_update <= ?1
                UNION
                SELECT b.user_id AS user_id, m.mode
                FROM user_bindings b
                CROSS JOIN (
                SELECT 0 AS mode
                UNION ALL SELECT 1
                UNION ALL SELECT 2
                UNION ALL SELECT 3
                ) AS m
                WHERE NOT EXISTS (
                    SELECT 1 FROM user_next_update n
                    WHERE n.user_id = b.user_id AND n.mode = m.mode
                )",
                params![now_ts],
            )
            .await?;

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let user_id: i64 = row.get(0)?;
            let mode_int: i32 = row.get(1)?;
            let mode = match mode_int {
                0 => GameMode::Osu,
                1 => GameMode::Taiko,
                2 => GameMode::Catch,
                3 => GameMode::Mania,
                _ => GameMode::Osu,
            };
            results.push((user_id, mode));
        }
        Ok(results)
    }

    // ==================== Pending Unbind Operations ====================

    /// Set a pending unbind confirmation for a user (expires in 5 minutes)
    pub async fn set_pending_unbind(&self, qq: i64) -> DbResult<()> {
        self.conn()
            .await
            .execute(
                "INSERT OR REPLACE INTO pending_unbind (qq, created_at) VALUES (?1, ?2)",
                params![qq, Utc::now().to_rfc3339()],
            )
            .await?;
        Ok(())
    }

    /// Get pending unbind timestamp if exists (returns None if expired or not exists)
    pub async fn get_pending_unbind(&self, qq: i64) -> DbResult<Option<DateTime<Utc>>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT created_at FROM pending_unbind WHERE qq = ?1",
                params![qq],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let created_at: String = row.get(0)?;
            if let Ok(dt) = DateTime::parse_from_rfc3339(&created_at) {
                let pending_time = dt.with_timezone(&Utc);
                if (Utc::now() - pending_time).num_seconds() < 300 {
                    return Ok(Some(pending_time));
                }
            }
        }
        Ok(None)
    }

    /// Remove pending unbind record
    pub async fn remove_pending_unbind(&self, qq: i64) -> DbResult<()> {
        self.conn()
            .await
            .execute("DELETE FROM pending_unbind WHERE qq = ?1", params![qq])
            .await?;
        Ok(())
    }

    /// Prune expired pending unbind records (older than 5 minutes).
    pub async fn prune_expired_pending_unbinds(&self) -> DbResult<u64> {
        let cutoff = (Utc::now() - chrono::TimeDelta::minutes(5)).to_rfc3339();
        self.conn()
            .await
            .execute(
                "DELETE FROM pending_unbind WHERE created_at < ?1",
                params![cutoff],
            )
            .await
    }

    /// Prune records older than retention_days from stats history and play records.
    /// Returns (deleted_stats, deleted_play_records).
    pub async fn prune_old_records(&self, retention_days: u64) -> DbResult<(u64, u64, u64)> {
        let retention_i64 = retention_days as i64;
        let cutoff_stats = Utc::now() - chrono::TimeDelta::days(retention_i64);
        let cutoff_stats_str = cutoff_stats.to_rfc3339();

        let conn = self.conn().await;

        let deleted_stats = conn
            .execute(
                "DELETE FROM user_stats_history WHERE recorded_at < ?1",
                params![cutoff_stats_str],
            )
            .await?;

        let cutoff_plays_ts = (Utc::now() - chrono::TimeDelta::days(retention_i64)).timestamp();

        let deleted_plays = conn
            .execute(
                "DELETE FROM user_play_records WHERE played_at < ?1",
                params![cutoff_plays_ts],
            )
            .await?;

        let deleted_next = conn
            .execute(
                "DELETE FROM user_next_update WHERE user_id NOT IN (SELECT DISTINCT user_id FROM user_bindings)",
                (),
            )
            .await?;

        Ok((deleted_stats, deleted_plays, deleted_next))
    }
}

// ==================== Pending Bind Operations ====================

pub struct PendingBind {
    pub code: String,
    pub qq_user_id: i64,
    pub group_id: i64,
    pub target_username: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: i64,
}

impl Storage {
    /// Add a pending bind and return the generated code
    pub async fn add_pending_bind(
        &self,
        qq_user_id: i64,
        group_id: i64,
        target_username: &str,
    ) -> DbResult<String> {
        let code: String = {
            use rand::RngExt;
            let mut rng = rand::rng();
            (0..6)
                .map(|_| rng.random_range(0..10).to_string())
                .collect()
        };

        let now = Utc::now();
        let expires_at = now.timestamp() + 120;

        self.conn().await
            .execute(
                "INSERT OR REPLACE INTO pending_binds (code, qq_user_id, group_id, target_username, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![code.clone(), qq_user_id, group_id, target_username, now.to_rfc3339(), expires_at],
            )
            .await?;
        Ok(code)
    }

    /// Get pending bind by code if not expired
    pub async fn get_pending_bind(&self, code: &str) -> DbResult<Option<PendingBind>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT code, qq_user_id, group_id, target_username, created_at, expires_at FROM pending_binds WHERE code = ?1",
                params![code],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let expires_at: i64 = row.get(5)?;
            if Utc::now().timestamp() > expires_at {
                return Ok(None);
            }
            let created_at_str: String = row.get(4)?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|e| {
                    tracing::warn!(created_at_str, error = %e, "{}", log_fmt!("storage.parse_pending_bind_created_failed"));
                    Utc::now()
                });
            Ok(Some(PendingBind {
                code: row.get(0)?,
                qq_user_id: row.get(1)?,
                group_id: row.get(2)?,
                target_username: row.get(3)?,
                created_at,
                expires_at,
            }))
        } else {
            Ok(None)
        }
    }

    /// Remove pending bind by code
    pub async fn remove_pending_bind(&self, code: &str) -> DbResult<()> {
        self.conn()
            .await
            .execute("DELETE FROM pending_binds WHERE code = ?1", params![code])
            .await?;
        Ok(())
    }

    /// Prune expired pending binds
    pub async fn prune_expired_pending_binds(&self) -> DbResult<u64> {
        let now_ts = Utc::now().timestamp();
        self.conn()
            .await
            .execute(
                "DELETE FROM pending_binds WHERE expires_at < ?1",
                params![now_ts],
            )
            .await
    }

    /// Check if a QQ user already has an active (non-expired) pending bind
    pub async fn has_pending_bind(&self, qq_user_id: i64) -> DbResult<bool> {
        let now_ts = Utc::now().timestamp();
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT EXISTS(SELECT 1 FROM pending_binds WHERE qq_user_id = ?1 AND expires_at >= ?2)",
                params![qq_user_id, now_ts],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let exists: bool = row.get(0)?;
            Ok(exists)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static DB_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_stats(pp: f64, hits: i64, playtime: i64) -> UserStats {
        UserStats {
            user_id: 0,
            username: String::new(),
            pp,
            rank: 0,
            country_rank: 0,
            country_code: "XX".to_string(),
            ranked_score: 0,
            accuracy: 0.0,
            playcount: 0,
            hits,
            playtime,
            rank_change: None,
            country_rank_change: None,
            cover_url: None,
        }
    }

    async fn test_storage() -> Storage {
        let id = DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "osubot-storage-batch-baseline-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        Storage::new(path.to_str().expect("temp db path is valid UTF-8"))
            .await
            .expect("create test storage")
    }

    async fn insert_snapshot(
        storage: &Storage,
        user_id: i64,
        mode: GameMode,
        recorded_at: DateTime<Utc>,
        stats: UserStats,
    ) {
        storage
            .conn()
            .await
            .execute(
                "INSERT INTO user_stats_history (user_id, mode, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    user_id,
                    mode as i32,
                    recorded_at.to_rfc3339(),
                    stats.pp,
                    stats.rank,
                    stats.country_rank,
                    stats.ranked_score,
                    stats.accuracy,
                    stats.playcount,
                    stats.hits,
                    stats.playtime,
                ],
            )
            .await
            .expect("insert snapshot");
    }

    #[tokio::test]
    async fn batch_baseline_returns_closest_snapshot_for_each_user() {
        let storage = test_storage().await;
        let now = Utc::now();
        let mode = GameMode::Osu;

        insert_snapshot(
            &storage,
            101,
            mode,
            now - chrono::TimeDelta::hours(30),
            test_stats(101.30, 1_030, 10_130),
        )
        .await;
        insert_snapshot(
            &storage,
            101,
            mode,
            now - chrono::TimeDelta::hours(23),
            test_stats(101.23, 1_023, 10_123),
        )
        .await;
        insert_snapshot(
            &storage,
            202,
            mode,
            now - chrono::TimeDelta::hours(25),
            test_stats(202.25, 2_025, 20_225),
        )
        .await;
        insert_snapshot(
            &storage,
            202,
            GameMode::Taiko,
            now - chrono::TimeDelta::hours(24),
            test_stats(999.0, 9_999, 99_999),
        )
        .await;
        insert_snapshot(
            &storage,
            303,
            mode,
            now - chrono::TimeDelta::hours(40),
            test_stats(303.40, 3_040, 30_340),
        )
        .await;

        let baselines = storage
            .get_baseline_snapshots_for_users(&[101, 202, 303], mode, 24, 36)
            .await
            .expect("batch baseline query succeeds");

        assert_eq!(baselines.len(), 2);
        assert_eq!(baselines.get(&101).expect("user 101 baseline").pp, 101.23);
        assert_eq!(baselines.get(&202).expect("user 202 baseline").pp, 202.25);
        assert!(!baselines.contains_key(&303));
    }

    #[tokio::test]
    async fn batch_baseline_tolerates_duplicate_user_ids_and_handles_empty_input() {
        let storage = test_storage().await;
        let now = Utc::now();
        let mode = GameMode::Osu;

        let empty = storage
            .get_baseline_snapshots_for_users(&[], mode, 24, 36)
            .await
            .expect("empty batch query succeeds");
        assert!(empty.is_empty());

        insert_snapshot(
            &storage,
            404,
            mode,
            now - chrono::TimeDelta::hours(24),
            test_stats(404.0, 4_040, 40_400),
        )
        .await;

        let baselines = storage
            .get_baseline_snapshots_for_users(&[404, 404, 404], mode, 24, 36)
            .await
            .expect("duplicate user IDs batch query succeeds");

        assert_eq!(baselines.len(), 1);
        assert_eq!(baselines.get(&404).expect("user 404 baseline").hits, 4_040);
    }
}
