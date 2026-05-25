use chrono::{DateTime, Local, TimeZone, Utc};
use rusqlite::{params, Connection, Result as SqlResult};
use std::path::Path;
use std::sync::Mutex;

use crate::types::{GameMode, UserChange, UserStats};

/// Returns UTC timestamp of today's 0:00 AM in local timezone
pub fn today_0am_utc() -> i64 {
    let local_now = chrono::Local::now();
    let today_local = local_now.date_naive();
    let today_0am_local = today_local.and_hms_opt(0, 0, 0).unwrap();
    // .single() returns None when midnight doesn't exist (DST spring-forward)
    // or is ambiguous (DST fall-back); fall back to 1:00 AM in that case.
    let dt = Local
        .from_local_datetime(&today_0am_local)
        .single()
        .unwrap_or_else(|| {
            let fallback = today_0am_local + chrono::Duration::hours(1);
            Local.from_local_datetime(&fallback).earliest().unwrap()
        });
    dt.with_timezone(&Utc).timestamp()
}

pub struct Storage {
    conn: Mutex<Connection>,
}

/// Check if the database has the old (username-based) schema
fn has_old_schema(conn: &Connection) -> SqlResult<bool> {
    // Check if user_bindings exists and has osu_username column (old schema marker)
    let mut stmt = conn.prepare("PRAGMA table_info(user_bindings)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let col_name: String = row.get(1)?;
        if col_name == "osu_username" {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Migrate from old username-based schema to user_id-based schema.
fn migrate_old_schema(conn: &Connection) -> SqlResult<()> {
    tracing::info!("Detected old database schema, migrating...");

    // Migrate user_bindings: map osu_username → user_id via osu_user_ids cache
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_bindings_new (
            qq INTEGER PRIMARY KEY,
            user_id INTEGER NOT NULL,
            current_username TEXT NOT NULL,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        );
        INSERT INTO user_bindings_new (qq, user_id, current_username, created_at)
        SELECT b.qq, COALESCE(o.user_id, 0), b.osu_username, b.created_at
        FROM user_bindings b
        LEFT JOIN osu_user_ids o ON LOWER(o.username) = LOWER(b.osu_username);
        DROP TABLE user_bindings;
        ALTER TABLE user_bindings_new RENAME TO user_bindings;",
    )?;

    let unmigrated: i64 = conn.query_row(
        "SELECT COUNT(*) FROM user_bindings WHERE user_id = 0",
        [],
        |row| row.get(0),
    )?;
    if unmigrated > 0 {
        tracing::warn!(
            "{} binding(s) could not be migrated (username→user_id lookup failed). These users need to re-bind.",
            unmigrated
        );
    }

    // Drop derived data tables and old indexes — will be recreated below
    conn.execute_batch(
        "DROP TABLE IF EXISTS user_stats_history;
         DROP TABLE IF EXISTS user_play_records;
         DROP TABLE IF EXISTS user_next_update;
         DROP TABLE IF EXISTS user_last_update;
         DROP INDEX IF EXISTS idx_history_user;
         DROP INDEX IF EXISTS idx_play_records_user;",
    )?;

    tracing::info!("Schema migration complete.");
    Ok(())
}

impl Storage {
    pub fn new<P: AsRef<Path>>(path: P) -> SqlResult<Self> {
        let conn = Connection::open(path)?;

        if has_old_schema(&conn)? {
            migrate_old_schema(&conn)?;
        }

        // Schema: user_bindings (qq → user_id, current_username)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_bindings (
                qq INTEGER PRIMARY KEY,
                user_id INTEGER NOT NULL,
                current_username TEXT NOT NULL,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // user_stats_history (user_id, mode, recorded_at → stats)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_stats_history (
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
            )",
            [],
        )?;

        // user_play_records (user_id, mode, played_at)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_play_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                played_at INTEGER NOT NULL,
                UNIQUE(user_id, mode, played_at)
            )",
            [],
        )?;

        // user_next_update (user_id, mode → next_update)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_next_update (
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                next_update INTEGER NOT NULL,
                PRIMARY KEY(user_id, mode)
            )",
            [],
        )?;

        // user_last_update (user_id, mode → last_update)
        conn.execute(
            "CREATE TABLE IF NOT EXISTS user_last_update (
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                last_update TEXT NOT NULL,
                PRIMARY KEY(user_id, mode)
            )",
            [],
        )?;

        // Unchanged tables...
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pending_unbind (
                qq INTEGER PRIMARY KEY,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

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

        conn.execute(
            "CREATE TABLE IF NOT EXISTS osu_user_ids (
                username TEXT PRIMARY KEY,
                user_id INTEGER NOT NULL
            )",
            [],
        )?;

        // New indexes on user_id (drop old username indexes)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_history_user ON user_stats_history(user_id, mode)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_history_recorded ON user_stats_history(recorded_at)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_play_records_user ON user_play_records(user_id, mode)",
            [],
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ==================== Binding Query ====================

    /// Bind QQ to user_id with current_username. Returns Err if user_id already bound to another QQ.
    pub fn bind(
        &self,
        qq: i64,
        user_id: i64,
        current_username: &str,
    ) -> SqlResult<Result<(), i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT qq FROM user_bindings WHERE user_id = ?1")?;
        let mut rows = stmt.query(params![user_id])?;
        if let Some(row) = rows.next()? {
            let existing_qq: i64 = row.get(0)?;
            if existing_qq != qq {
                return Ok(Err(existing_qq));
            }
        }
        drop(rows);
        drop(stmt);
        conn.execute(
            "INSERT OR REPLACE INTO user_bindings (qq, user_id, current_username) VALUES (?1, ?2, ?3)",
            params![qq, user_id, current_username],
        )?;
        Ok(Ok(()))
    }

    pub fn unbind(&self, qq: i64) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();

        // Get the user_id before deleting
        let user_id: Option<i64> = conn
            .query_row(
                "SELECT user_id FROM user_bindings WHERE qq = ?1",
                params![qq],
                |row| row.get(0),
            )
            .ok();

        conn.execute("DELETE FROM user_bindings WHERE qq = ?1", params![qq])?;

        // Clean up user_next_update if no other bindings for this user_id
        if let Some(uid) = user_id {
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM user_bindings WHERE user_id = ?1",
                params![uid],
                |row| row.get(0),
            )?;
            if count == 0 {
                conn.execute(
                    "DELETE FROM user_next_update WHERE user_id = ?1",
                    params![uid],
                )?;
            }
        }

        Ok(())
    }

    pub fn set_user_id(&self, username: &str, user_id: i64) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO osu_user_ids (username, user_id) VALUES (LOWER(?1), ?2)",
            params![username, user_id],
        )?;
        Ok(())
    }

    /// Get cached osu! user ID (case-insensitive username lookup)
    pub fn get_user_id(&self, username: &str) -> SqlResult<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT user_id FROM osu_user_ids WHERE LOWER(username) = LOWER(?1)")?;
        let mut rows = stmt.query(params![username])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Bound usernames that don't have a cached user_id yet
    pub fn get_users_without_ids(&self) -> SqlResult<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT b.current_username FROM user_bindings b
             WHERE NOT EXISTS (
                 SELECT 1 FROM osu_user_ids o WHERE LOWER(o.username) = LOWER(b.current_username)
             )",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn get_binding(&self, qq: i64) -> SqlResult<Option<(i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT user_id, current_username FROM user_bindings WHERE qq = ?1")?;
        let mut rows = stmt.query(params![qq])?;
        if let Some(row) = rows.next()? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub fn update_binding_username(&self, qq: i64, new_username: &str) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE user_bindings SET current_username = ?1 WHERE qq = ?2",
            params![new_username, qq],
        )?;
        Ok(())
    }

    /// Update current_username for all QQs bound to a given user_id (username change detection).
    /// Returns the number of bindings updated.
    pub fn update_binding_username_by_user_id(
        &self,
        user_id: i64,
        new_username: &str,
    ) -> SqlResult<usize> {
        let conn = self.conn.lock().unwrap();
        // Only UPDATE if username actually differs. SQLite returns rows-matched,
        // not rows-changed, so an unconditional UPDATE would spuriously signal a change.
        let current: Option<String> = conn
            .query_row(
                "SELECT current_username FROM user_bindings WHERE user_id = ?1 LIMIT 1",
                params![user_id],
                |row| row.get(0),
            )
            .ok();
        if current.as_deref() == Some(new_username) {
            return Ok(0);
        }
        let count = conn.execute(
            "UPDATE user_bindings SET current_username = ?1 WHERE user_id = ?2",
            params![new_username, user_id],
        )?;
        Ok(count)
    }

    // ==================== Snapshot Operations ====================

    pub fn save_stats(&self, user_id: i64, mode: GameMode, stats: &UserStats) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
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
        )?;
        Ok(())
    }

    pub fn get_latest_snapshot(
        &self,
        user_id: i64,
        mode: GameMode,
    ) -> SqlResult<Option<UserStats>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
             FROM user_stats_history
             WHERE user_id = ?1 AND mode = ?2
             ORDER BY recorded_at DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![user_id, mode as i32])?;
        if let Some(row) = rows.next()? {
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
            }))
        } else {
            Ok(None)
        }
    }

    pub fn get_snapshots_within_hours(
        &self,
        user_id: i64,
        mode: GameMode,
        hours: i64,
    ) -> SqlResult<Vec<(DateTime<Utc>, UserStats)>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = Utc::now() - chrono::Duration::hours(hours);
        let cutoff_str = cutoff.to_rfc3339();

        let mut stmt = conn.prepare(
            "SELECT recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
             FROM user_stats_history
             WHERE user_id = ?1 AND mode = ?2 AND recorded_at >= ?3
             ORDER BY recorded_at ASC",
        )?;

        let rows = stmt.query_map(params![user_id, mode as i32, cutoff_str], |row| {
            let recorded_str: String = row.get(0)?;
            Ok((
                recorded_str,
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
        user_id: i64,
        mode: GameMode,
        target_hours_ago: i64,
        max_lookback: i64,
    ) -> SqlResult<Option<(DateTime<Utc>, UserStats)>> {
        let now = Utc::now();
        let target_time = now - chrono::Duration::hours(target_hours_ago);
        let earliest = now - chrono::Duration::hours(max_lookback);

        let all = self.get_snapshots_within_hours(user_id, mode, max_lookback)?;

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
        user_id: i64,
        mode: GameMode,
        timestamps: &[i64],
    ) -> SqlResult<i32> {
        let conn = self.conn.lock().unwrap();
        let mut inserted = 0i32;

        for &timestamp in timestamps {
            let result = conn.execute(
                "INSERT OR IGNORE INTO user_play_records (user_id, mode, played_at) VALUES (?1, ?2, ?3)",
                params![user_id, mode as i32, timestamp],
            );
            if let Ok(count) = result {
                inserted += count as i32;
            }
        }
        Ok(inserted)
    }

    /// Check if user has any play records since the given UTC timestamp
    pub fn has_play_since(&self, user_id: i64, mode: GameMode, since_ts: i64) -> SqlResult<bool> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT 1 FROM user_play_records WHERE user_id = ?1 AND mode = ?2 AND played_at >= ?3 LIMIT 1",
        )?;
        let exists = stmt.query_row(params![user_id, mode as i32, since_ts], |_| Ok(()));
        Ok(exists.is_ok())
    }

    // ==================== Change Calculation ====================

    pub fn calculate_change(
        &self,
        user_id: i64,
        mode: GameMode,
        current: &UserStats,
    ) -> SqlResult<Option<UserChange>> {
        let snapshot = self.get_closest_snapshot_to_hours_ago(user_id, mode, 24, 36)?;

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
        user_id: i64,
        mode: GameMode,
    ) -> SqlResult<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT last_update FROM user_last_update WHERE user_id = ?1 AND mode = ?2")?;
        let mut rows = stmt.query(params![user_id, mode as i32])?;
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
        user_id: i64,
        mode: GameMode,
        time: DateTime<Utc>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO user_last_update (user_id, mode, last_update) VALUES (?1, ?2, ?3)",
            params![user_id, mode as i32, time.to_rfc3339()],
        )?;
        Ok(())
    }

    // ==================== Next Update (for scheduler dynamic intervals) ====================

    pub fn get_next_update(
        &self,
        user_id: i64,
        mode: GameMode,
    ) -> SqlResult<Option<DateTime<Utc>>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT next_update FROM user_next_update WHERE user_id = ?1 AND mode = ?2")?;
        let mut rows = stmt.query(params![user_id, mode as i32])?;
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
        user_id: i64,
        mode: GameMode,
        time: DateTime<Utc>,
    ) -> SqlResult<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO user_next_update (user_id, mode, next_update) VALUES (?1, ?2, ?3)",
            params![user_id, mode as i32, time.timestamp()],
        )?;
        Ok(())
    }

    /// Get all user bindings (qq -> user_id, current_username mappings)
    pub fn get_all_user_bindings(&self) -> SqlResult<Vec<(i64, i64, String)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT qq, user_id, current_username FROM user_bindings")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // ==================== Due Users Query ====================

    pub fn get_due_users(&self) -> SqlResult<Vec<(i64, GameMode)>> {
        let conn = self.conn.lock().unwrap();
        let now_ts = Utc::now().timestamp();

        let mut stmt = conn.prepare(
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
        )?;
        let rows = stmt.query_map(params![now_ts], |row| {
            let user_id: i64 = row.get(0)?;
            let mode_int: i32 = row.get(1)?;
            let mode = match mode_int {
                0 => GameMode::Osu,
                1 => GameMode::Taiko,
                2 => GameMode::Catch,
                3 => GameMode::Mania,
                _ => GameMode::Osu,
            };
            Ok((user_id, mode))
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
    pub fn prune_old_records(&self, retention_days: u64) -> SqlResult<(u64, u64, u64)> {
        let conn = self.conn.lock().unwrap();

        let retention_i64 = retention_days as i64;
        let cutoff_stats = Utc::now() - chrono::Duration::days(retention_i64);
        let cutoff_stats_str = cutoff_stats.to_rfc3339();

        let deleted_stats = conn.execute(
            "DELETE FROM user_stats_history WHERE recorded_at < ?1",
            params![cutoff_stats_str],
        )? as u64;

        let cutoff_plays_ts = (Utc::now() - chrono::Duration::days(retention_i64)).timestamp();

        let deleted_plays = conn.execute(
            "DELETE FROM user_play_records WHERE played_at < ?1",
            params![cutoff_plays_ts],
        )? as u64;

        // Clean up user_next_update rows for users no longer bound
        let deleted_next = conn.execute(
            "DELETE FROM user_next_update WHERE user_id NOT IN (SELECT DISTINCT user_id FROM user_bindings)",
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
