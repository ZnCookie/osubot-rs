use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqlResult};
use std::path::Path;
use std::sync::Mutex;

use crate::types::{GameMode, UserChange, UserStats};

pub struct Storage {
    conn: Mutex<Connection>,
}

impl Storage {
    pub fn new<P: AsRef<Path>>(path: P) -> SqlResult<Self> {
        let conn = Connection::open(path)?;

        // Existing user_bindings table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_bindings (
                qq INTEGER PRIMARY KEY,
                osu_username TEXT NOT NULL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // User stats history table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_stats_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL,
                mode INTEGER NOT NULL,
                recorded_at TEXT DEFAULT CURRENT_TIMESTAMP,
                pp REAL,
                rank INTEGER,
                country_rank INTEGER,
                ranked_score INTEGER,
                accuracy REAL,
                playcount INTEGER,
                hits INTEGER,
                playtime INTEGER
            )",
            [],
        )?;

        // User play records table (for activity detection)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_play_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                username TEXT NOT NULL,
                mode INTEGER NOT NULL,
                played_at INTEGER NOT NULL,
                UNIQUE(username, mode, played_at)
            )",
            [],
        )?;

        // User next update table (for scheduler dynamic intervals)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_next_update (
                username TEXT NOT NULL,
                mode INTEGER NOT NULL,
                next_update INTEGER NOT NULL,
                PRIMARY KEY(username, mode)
            )",
            [],
        )?;

        // User last update table (stores exact time passed to set_last_update)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_last_update (
                username TEXT NOT NULL,
                mode INTEGER NOT NULL,
                last_update TEXT NOT NULL,
                PRIMARY KEY(username, mode)
            )",
            [],
        )?;

        // Pending unbind confirmations (for two-step unbind)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pending_unbind (
                qq INTEGER PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Pending binds for IRC auth (for two-step bind verification)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pending_binds (
                code TEXT PRIMARY KEY,
                qq_user_id INTEGER NOT NULL,
                group_id INTEGER NOT NULL,
                target_username TEXT NOT NULL,
                created_at TEXT NOT NULL,
                expires_at INTEGER NOT NULL
            )",
            [],
        )?;

        // Indexes
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_history_user ON user_stats_history(username, mode)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_history_recorded ON user_stats_history(recorded_at)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_play_records_user ON user_play_records(username, mode)",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ==================== Binding Query ====================

    /// Get QQ by osu username (case-insensitive)
    pub fn get_qq_by_osu_username(&self, username: &str) -> SqlResult<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT qq FROM user_bindings WHERE LOWER(osu_username) = LOWER(?1)")?;
        let mut rows = stmt.query(params![username])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Bind QQ to osu username. Returns Err if username already bound to another QQ.
    pub fn bind(&self, qq: i64, username: &str) -> SqlResult<Result<(), i64>> {
        // Check if username is already bound to another QQ
        if let Some(existing_qq) = self.get_qq_by_osu_username(username)? {
            if existing_qq != qq {
                // Username already bound to a different QQ
                return Ok(Err(existing_qq));
            }
        }

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO user_bindings (qq, osu_username) VALUES (?1, ?2)",
            params![qq, username],
        )?;
        Ok(Ok(()))
    }

    pub fn unbind(&self, qq: i64) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();

        // Get the osu username before deleting
        let username: Option<String> = conn
            .query_row(
                "SELECT osu_username FROM user_bindings WHERE qq = ?1",
                params![qq],
                |row| row.get(0),
            )
            .ok();

        conn.execute("DELETE FROM user_bindings WHERE qq = ?1", params![qq])?;

        // Clean up user_next_update if no other QQ binds to this username
        if let Some(ref u) = username {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM user_bindings WHERE osu_username = ?1",
                params![u],
                |row| row.get(0),
            )?;
            if count == 0 {
                conn.execute(
                    "DELETE FROM user_next_update WHERE username = ?1",
                    params![u],
                )?;
            }
        }

        Ok(())
    }

    pub fn get_binding(&self, qq: i64) -> SqlResult<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT osu_username FROM user_bindings WHERE qq = ?1")?;
        let mut rows = stmt.query(params![qq])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    // ==================== Snapshot Operations ====================

    pub fn save_stats(&self, username: &str, mode: GameMode, stats: &UserStats) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO user_stats_history (username, mode, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                username,
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
        )?;
        Ok(())
    }

    pub fn get_latest_snapshot(
        &self,
        username: &str,
        mode: GameMode,
    ) -> SqlResult<Option<UserStats>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT username, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
             FROM user_stats_history
             WHERE username = ?1 AND mode = ?2
             ORDER BY recorded_at DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![username, mode as i32])?;
        if let Some(row) = rows.next()? {
            Ok(Some(UserStats {
                username: row.get(0)?,
                pp: row.get(1)?,
                rank: row.get(2)?,
                country_rank: row.get(3)?,
                country_code: "XX".to_string(), // Historical data doesn't store country_code
                ranked_score: row.get(4)?,
                accuracy: row.get(5)?,
                playcount: row.get(6)?,
                hits: row.get(7)?,
                playtime: row.get(8)?,
                rank_change: None,
                country_rank_change: None,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_snapshots_within_hours(
        &self,
        username: &str,
        mode: GameMode,
        hours: i64,
    ) -> SqlResult<Vec<(DateTime<Utc>, UserStats)>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = Utc::now() - chrono::Duration::hours(hours);
        let cutoff_str = cutoff.to_rfc3339();

        let mut stmt = conn.prepare(
            "SELECT username, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
             FROM user_stats_history
             WHERE username = ?1 AND mode = ?2 AND recorded_at >= ?3
             ORDER BY recorded_at ASC",
        )?;

        let rows = stmt.query_map(params![username, mode as i32, cutoff_str], |row| {
            let recorded_str: String = row.get(1)?;
            let username: String = row.get(0)?;
            Ok((
                recorded_str,
                UserStats {
                    username,
                    pp: row.get(2)?,
                    rank: row.get(3)?,
                    country_rank: row.get(4)?,
                    country_code: "XX".to_string(), // Historical data doesn't store country_code
                    ranked_score: row.get(5)?,
                    accuracy: row.get(6)?,
                    playcount: row.get(7)?,
                    hits: row.get(8)?,
                    playtime: row.get(9)?,
                    rank_change: None,
                    country_rank_change: None,
                },
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (recorded_str, mut stats) = row?;
            if let Ok(dt) = DateTime::parse_from_rfc3339(&recorded_str) {
                stats.rank_change = None;
                stats.country_rank_change = None;
                results.push((dt.with_timezone(&Utc), stats));
            }
        }
        Ok(results)
    }

    pub fn get_closest_snapshot_to_hours_ago(
        &self,
        username: &str,
        mode: GameMode,
        target_hours_ago: i64,
        max_lookback: i64,
    ) -> SqlResult<Option<(DateTime<Utc>, UserStats)>> {
        let now = Utc::now();
        let target_time = now - chrono::Duration::hours(target_hours_ago);
        let earliest = now - chrono::Duration::hours(max_lookback);

        let all = self.get_snapshots_within_hours(username, mode, max_lookback)?;

        let candidates: Vec<_> = all.into_iter().filter(|(dt, _)| *dt >= earliest).collect();

        if candidates.is_empty() {
            return Ok(None);
        }

        Ok(Some(
            candidates
                .into_iter()
                .min_by_key(|(dt, _)| (*dt - target_time).num_seconds().unsigned_abs() as i64)
                .unwrap(),
        ))
    }

    // ==================== Play Records Operations ====================

    pub fn save_play_records(
        &self,
        username: &str,
        mode: GameMode,
        timestamps: &[i64],
    ) -> SqlResult<i32> {
        let conn = self.conn.lock().unwrap();
        let mut inserted = 0i32;

        for &timestamp in timestamps {
            let result = conn.execute(
                "INSERT OR IGNORE INTO user_play_records (username, mode, played_at) VALUES (?1, ?2, ?3)",
                params![username, mode as i32, timestamp],
            );
            if let Ok(count) = result {
                inserted += count as i32;
            }
        }
        Ok(inserted)
    }

    pub fn get_play_count_since(
        &self,
        username: &str,
        mode: GameMode,
        hours: i64,
    ) -> SqlResult<i64> {
        let conn = self.conn.lock().unwrap();
        let cutoff = Utc::now() - chrono::Duration::hours(hours);
        let cutoff_ts = cutoff.timestamp();

        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM user_play_records WHERE username = ?1 AND mode = ?2 AND played_at >= ?3",
        )?;
        let count: i64 =
            stmt.query_row(params![username, mode as i32, cutoff_ts], |row| row.get(0))?;
        Ok(count)
    }

    // ==================== Change Calculation ====================

    pub fn calculate_change(
        &self,
        username: &str,
        mode: GameMode,
        current: &UserStats,
    ) -> SqlResult<Option<UserChange>> {
        let snapshot = self.get_closest_snapshot_to_hours_ago(username, mode, 24, 36)?;

        match snapshot {
            None => Ok(None),
            Some((_, past)) => {
                let rank_change = if current.rank != 0 && past.rank != 0 {
                    Some(current.rank - past.rank)
                } else {
                    None
                };
                let country_rank_change = if current.country_rank != 0 && past.country_rank != 0 {
                    Some(current.country_rank - past.country_rank)
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

    pub fn get_last_update(
        &self,
        username: &str,
        mode: GameMode,
    ) -> SqlResult<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT last_update FROM user_last_update WHERE username = ?1 AND mode = ?2",
        )?;
        let mut rows = stmt.query(params![username, mode as i32])?;
        if let Some(row) = rows.next()? {
            let last_update_str: String = row.get(0)?;
            if let Ok(dt) = DateTime::parse_from_rfc3339(&last_update_str) {
                return Ok(Some(dt.with_timezone(&Utc)));
            }
        }
        Ok(None)
    }

    pub fn set_last_update(
        &self,
        username: &str,
        mode: GameMode,
        time: DateTime<Utc>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO user_last_update (username, mode, last_update) VALUES (?1, ?2, ?3)",
            params![username, mode as i32, time.to_rfc3339()],
        )?;
        Ok(())
    }

    // ==================== Next Update (for scheduler dynamic intervals) ====================

    pub fn get_next_update(
        &self,
        username: &str,
        mode: GameMode,
    ) -> SqlResult<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT next_update FROM user_next_update WHERE username = ?1 AND mode = ?2",
        )?;
        let mut rows = stmt.query(params![username, mode as i32])?;
        if let Some(row) = rows.next()? {
            let ts: i64 = row.get(0)?;
            return Ok(Some(
                Utc.timestamp_opt(ts, 0).single().unwrap_or_else(Utc::now),
            ));
        }
        Ok(None)
    }

    pub fn set_next_update(
        &self,
        username: &str,
        mode: GameMode,
        time: DateTime<Utc>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO user_next_update (username, mode, next_update) VALUES (?1, ?2, ?3)",
            params![username, mode as i32, time.timestamp()],
        )?;
        Ok(())
    }

    // ==================== All Mode Bindings Query ====================

    pub fn get_all_bindings(&self, qq: i64) -> SqlResult<Vec<(GameMode, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT osu_username FROM user_bindings WHERE qq = ?1")?;
        let mut rows = stmt.query(params![qq])?;
        let mut results = Vec::new();

        // We can't directly join, so we return all bindings for this QQ
        // Since user_bindings only stores one username per QQ, we return that for all modes
        if let Some(row) = rows.next()? {
            let username: String = row.get(0)?;
            for mode in [
                GameMode::Osu,
                GameMode::Taiko,
                GameMode::Catch,
                GameMode::Mania,
            ] {
                results.push((mode, username.clone()));
            }
        }
        Ok(results)
    }

    /// Get all user bindings (qq -> username mappings)
    pub fn get_all_user_bindings(&self) -> SqlResult<Vec<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT qq, osu_username FROM user_bindings")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // ==================== Due Users Query ====================

    pub fn get_due_users(&self) -> SqlResult<Vec<(String, GameMode)>> {
        let conn = self.conn.lock().unwrap();
        let now_ts = Utc::now().timestamp();

        // Part 1: users with next_update <= now (scheduled due users)
        // Part 2: bound users not yet in user_next_update (catch stray bindings)
        let mut stmt = conn.prepare(
            "SELECT username, mode FROM user_next_update WHERE next_update <= ?1
            UNION
            SELECT b.osu_username AS username, m.mode
            FROM user_bindings b
            CROSS JOIN (
            SELECT 0 AS mode
            UNION ALL SELECT 1
            UNION ALL SELECT 2
            UNION ALL SELECT 3
            ) AS m
            WHERE NOT EXISTS (
                SELECT 1 FROM user_next_update n
                WHERE n.username = b.osu_username AND n.mode = m.mode
            )",
        )?;
        let rows = stmt.query_map(params![now_ts], |row| {
            let username: String = row.get(0)?;
            let mode_int: i32 = row.get(1)?;
            let mode = match mode_int {
                0 => GameMode::Osu,
                1 => GameMode::Taiko,
                2 => GameMode::Catch,
                3 => GameMode::Mania,
                _ => GameMode::Osu,
            };
            Ok((username, mode))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // ==================== Pending Unbind Operations ====================

    /// Set a pending unbind confirmation for a user (expires in 5 minutes)
    pub fn set_pending_unbind(&self, qq: i64) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO pending_unbind (qq, created_at) VALUES (?1, ?2)",
            params![qq, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Get pending unbind timestamp if exists (returns None if expired or not exists)
    pub fn get_pending_unbind(&self, qq: i64) -> SqlResult<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT created_at FROM pending_unbind WHERE qq = ?1")?;
        let mut rows = stmt.query(params![qq])?;
        if let Some(row) = rows.next()? {
            let created_at: String = row.get(0)?;
            if let Ok(dt) = DateTime::parse_from_rfc3339(&created_at) {
                let pending_time = dt.with_timezone(&Utc);
                // Check if expired (5 minutes)
                if (Utc::now() - pending_time).num_seconds() < 300 {
                    return Ok(Some(pending_time));
                }
            }
        }
        Ok(None)
    }

    /// Remove pending unbind record
    pub fn remove_pending_unbind(&self, qq: i64) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM pending_unbind WHERE qq = ?1", params![qq])?;
        Ok(())
    }

    /// Prune records older than retention_days from stats history and play records.
    /// Returns (deleted_stats, deleted_play_records).
    pub fn prune_old_records(&self, retention_days: i64) -> SqlResult<(u64, u64, u64)> {
        let conn = self.conn.lock().unwrap();

        let cutoff_stats = Utc::now() - chrono::Duration::days(retention_days);
        let cutoff_stats_str = cutoff_stats.to_rfc3339();

        let deleted_stats = conn.execute(
            "DELETE FROM user_stats_history WHERE recorded_at < ?1",
            params![cutoff_stats_str],
        )? as u64;

        let cutoff_plays_ts = (Utc::now() - chrono::Duration::days(retention_days)).timestamp();

        let deleted_plays = conn.execute(
            "DELETE FROM user_play_records WHERE played_at < ?1",
            params![cutoff_plays_ts],
        )? as u64;

        // Clean up user_next_update rows for users no longer bound
        let deleted_next = conn.execute(
            "DELETE FROM user_next_update WHERE username NOT IN (SELECT DISTINCT osu_username FROM user_bindings)",
            [],
        )? as u64;

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
    pub fn add_pending_bind(
        &self,
        qq_user_id: i64,
        group_id: i64,
        target_username: &str,
    ) -> SqlResult<String> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let code: String = (0..6).map(|_| rng.gen_range(0..10).to_string()).collect();

        let now = Utc::now();
        let expires_at = now.timestamp() + 120; // 2 minutes

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO pending_binds (code, qq_user_id, group_id, target_username, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![code, qq_user_id, group_id, target_username, now.to_rfc3339(), expires_at],
        )?;
        Ok(code)
    }

    /// Get pending bind by code if not expired
    pub fn get_pending_bind(&self, code: &str) -> SqlResult<Option<PendingBind>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT code, qq_user_id, group_id, target_username, created_at, expires_at FROM pending_binds WHERE code = ?1"
        )?;
        let mut rows = stmt.query(params![code])?;
        if let Some(row) = rows.next()? {
            let expires_at: i64 = row.get(5)?;
            // Check if expired
            if Utc::now().timestamp() > expires_at {
                return Ok(None);
            }
            let created_at_str: String = row.get(4)?;
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
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
    pub fn remove_pending_bind(&self, code: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM pending_binds WHERE code = ?1", params![code])?;
        Ok(())
    }

    /// Prune expired pending binds
    pub fn prune_expired_pending_binds(&self) -> SqlResult<u64> {
        let conn = self.conn.lock().unwrap();
        let now_ts = Utc::now().timestamp();
        let deleted = conn.execute(
            "DELETE FROM pending_binds WHERE expires_at < ?1",
            params![now_ts],
        )? as u64;
        Ok(deleted)
    }

    /// Check if a QQ user already has an active (non-expired) pending bind
    pub fn has_pending_bind(&self, qq_user_id: i64) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        let now_ts = Utc::now().timestamp();
        let mut stmt = conn.prepare(
            "SELECT COUNT(*) FROM pending_binds WHERE qq_user_id = ?1 AND expires_at >= ?2",
        )?;
        let count: i64 = stmt.query_row(params![qq_user_id, now_ts], |row| row.get(0))?;
        Ok(count > 0)
    }
}
