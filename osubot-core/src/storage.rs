use crate::log_fmt;
use chrono::{DateTime, Local, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use turso::{params, Connection, Database, Result as DbResult, Row};

use crate::types::{GameMode, Server, UserChange, UserStats};

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

/// RAII guard that removes the temp database file on drop.
#[cfg(test)]
pub(crate) struct TempDb {
    path: std::path::PathBuf,
    storage: Storage,
}

#[cfg(test)]
impl std::ops::Deref for TempDb {
    type Target = Storage;
    fn deref(&self) -> &Storage {
        &self.storage
    }
}

#[cfg(test)]
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Check if a table has a given column (used for migration checks).
/// `table` is validated against an allowlist to prevent SQL injection.
async fn has_column(
    pool: &[tokio::sync::Mutex<Connection>],
    table: &str,
    column: &str,
) -> DbResult<bool> {
    const ALLOWED_TABLES: &[&str] = &[
        "user_bindings",
        "match_listeners",
        "user_stats_history",
        "user_play_records",
    ];
    if !ALLOWED_TABLES.contains(&table) {
        return Err(turso::Error::Error(format!(
            "has_column: table '{table}' is not in the allowlist"
        )));
    }
    let conn = pool[0].lock().await;
    let mut rows = conn
        .query(&format!("PRAGMA table_info(\"{table}\")"), ())
        .await?;
    while let Some(row) = rows.next().await? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 将若干核心表重建为包含 `server` 列，并纳入主键 / 唯一约束。
///
/// 判断依据：检查 `sqlite_master` 中该表的 CREATE SQL 是否已包含 "server"。
/// - 全新数据库：建表语句已含 `server`，跳过重建。
/// - 旧数据库（曾通过 ALTER 加过列）：其 `sqlite_master.sql` 不含 "server"，触发重建。
///
/// 重建过程对存量数据统一补 `'official'`（历史数据均为官方服）。
async fn migrate_tables_add_server(pool: &[tokio::sync::Mutex<Connection>]) -> DbResult<()> {
    const SPECS: &[(&str, &str, &str)] = &[
        (
            "user_bindings",
            "CREATE TABLE user_bindings (
                qq INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                current_username TEXT NOT NULL,
                default_mode INTEGER NOT NULL DEFAULT 0,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP,
                server TEXT NOT NULL DEFAULT 'official',
                PRIMARY KEY(qq, server)
            )",
            "INSERT INTO user_bindings (qq, user_id, current_username, default_mode, created_at, server)
             SELECT qq, user_id, current_username, default_mode, created_at, 'official' FROM user_bindings_old",
        ),
        (
            "user_stats_history",
            "CREATE TABLE user_stats_history (
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
                server TEXT NOT NULL DEFAULT 'official',
                UNIQUE(user_id, mode, server, recorded_at)
            )",
            "INSERT INTO user_stats_history (id, user_id, mode, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime, server)
             SELECT id, user_id, mode, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime, 'official' FROM user_stats_history_old",
        ),
        (
            "user_play_records",
            "CREATE TABLE user_play_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                played_at INTEGER NOT NULL,
                server TEXT NOT NULL DEFAULT 'official',
                UNIQUE(user_id, mode, server, played_at)
            )",
            "INSERT INTO user_play_records (id, user_id, mode, played_at, server)
             SELECT id, user_id, mode, played_at, 'official' FROM user_play_records_old",
        ),
        (
            "user_next_update",
            "CREATE TABLE user_next_update (
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                next_update INTEGER NOT NULL,
                server TEXT NOT NULL DEFAULT 'official',
                PRIMARY KEY(user_id, mode, server)
            )",
            "INSERT INTO user_next_update (user_id, mode, next_update, server)
             SELECT user_id, mode, next_update, 'official' FROM user_next_update_old",
        ),
        (
            "osu_user_ids",
            "CREATE TABLE osu_user_ids (
                username TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                server TEXT NOT NULL DEFAULT 'official',
                PRIMARY KEY (username, server)
            )",
            "INSERT INTO osu_user_ids (username, user_id, server)
             SELECT username, user_id, 'official' FROM osu_user_ids_old",
        ),
    ];

    let conn = pool[0].lock().await;
    conn.execute("BEGIN IMMEDIATE", ()).await?;
    let result: DbResult<()> = async {
        for (table, create, insert) in SPECS {
            let needs_rebuild: bool = {
                let mut rows = conn
                    .query(
                        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                        params![table],
                    )
                    .await?;
                match rows.next().await? {
                    Some(row) => {
                        let sql: Option<String> = row.get(0)?;
                        sql.map(|s| !s.contains("server")).unwrap_or(true)
                    }
                    None => false,
                }
            };
            if needs_rebuild {
                // 幂等：若上次迁移中途崩溃残留 _old 表，先删除再重建
                conn.execute(&format!("DROP TABLE IF EXISTS {table}_old"), ())
                    .await?;
                conn.execute(&format!("ALTER TABLE {table} RENAME TO {table}_old"), ())
                    .await?;
                conn.execute(create, ()).await?;
                conn.execute(insert, ()).await?;
                conn.execute(&format!("DROP TABLE {table}_old"), ()).await?;
            }
        }
        Ok(())
    }
    .await;
    match result {
        Ok(()) => {
            conn.execute("COMMIT", ()).await?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", ()).await;
            Err(e)
        }
    }
}

/// 快照查询返回的 stats 子集（无 identity 列）。`UserStats` 的 identity 字段
/// 在快照场景下无意义，由调用方从查询上下文（user_id 形参）补齐。
#[derive(Debug, Clone)]
pub struct UserStatsSnapshot {
    pub pp: Option<f64>,
    pub rank: Option<i64>,
    pub country_rank: Option<i64>,
    pub ranked_score: Option<i64>,
    pub accuracy: Option<f64>,
    pub playcount: Option<i64>,
    pub hits: Option<i64>,
    pub playtime: Option<i64>,
}

impl UserStatsSnapshot {
    /// 把快照补成完整 `UserStats`（identity 字段由调用方提供）。
    /// 适用于：调用方已从查询上下文拿到 user_id / username / country_code，
    /// 想要构造一个完整 stats 传递到下游 API。
    pub fn into_user_stats(self, user_id: i64, username: &str, country_code: &str) -> UserStats {
        UserStats {
            user_id,
            username: username.to_string(),
            country_code: country_code.to_string(),
            pp: self.pp.unwrap_or(0.0),
            rank: self.rank.unwrap_or(0),
            country_rank: self.country_rank.unwrap_or(0),
            ranked_score: self.ranked_score.unwrap_or(0),
            accuracy: self.accuracy.unwrap_or(0.0),
            playcount: self.playcount.unwrap_or(0),
            hits: self.hits.unwrap_or(0),
            playtime: self.playtime.unwrap_or(0),
            rank_change: None,
            country_rank_change: None,
            cover_url: None,
        }
    }
}

fn row_to_snapshot(row: &Row, col_offset: usize) -> DbResult<UserStatsSnapshot> {
    Ok(UserStatsSnapshot {
        pp: row.get(col_offset)?,
        rank: row.get(col_offset + 1)?,
        country_rank: row.get(col_offset + 2)?,
        ranked_score: row.get(col_offset + 3)?,
        accuracy: row.get(col_offset + 4)?,
        playcount: row.get(col_offset + 5)?,
        hits: row.get(col_offset + 6)?,
        playtime: row.get(col_offset + 7)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationChannel {
    Group,
    Private,
}

impl NotificationChannel {
    pub fn try_from_str(s: &str) -> Self {
        match s {
            "private" => Self::Private,
            _ => Self::Group,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchListener {
    pub match_id: i64,
    pub group_id: Option<i64>,
    pub user_id: Option<i64>,
    pub notification_type: String,
    pub creator_qq: i64,
    pub match_name: String,
    pub last_event_id: Option<i64>,
    pub last_notified_event_id: Option<i64>,
    pub pending_game_event_id: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: i64,
    pub active: bool,
    pub last_notified_at: Option<DateTime<Utc>>,
}

impl MatchListener {
    pub fn notification_channel(&self) -> NotificationChannel {
        NotificationChannel::try_from_str(&self.notification_type)
    }
}

#[derive(Debug, Clone)]
pub struct MatchListenerStartParams {
    pub match_id: i64,
    pub group_id: Option<i64>,
    pub user_id: Option<i64>,
    pub notification_type: String,
    pub creator_qq: i64,
    pub match_name: String,
    pub expires_at: i64,
    pub initial_last_event_id: Option<i64>,
    pub initial_last_notified_event_id: Option<i64>,
}

fn parse_db_datetime(value: &str, field_name: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|error| {
            tracing::warn!(field_name, value, error = %error, "failed to parse datetime from storage");
            Utc::now()
        })
}

fn row_to_match_listener(row: &Row) -> DbResult<MatchListener> {
    let created_at: String = row.get(7)?;
    let last_notified_at: Option<String> = row.get(10)?;
    let active: i64 = row.get(9)?;
    let group_id: Option<i64> = row.get(1)?;
    let user_id: Option<i64> = row.get(11)?;
    let notification_type: String = row.get(12)?;

    Ok(MatchListener {
        match_id: row.get(0)?,
        group_id,
        user_id,
        notification_type,
        creator_qq: row.get(2)?,
        match_name: row.get(3)?,
        last_event_id: row.get(4)?,
        last_notified_event_id: row.get(5)?,
        pending_game_event_id: row.get(6)?,
        created_at: parse_db_datetime(&created_at, "match_listeners.created_at"),
        expires_at: row.get(8)?,
        active: active != 0,
        last_notified_at: last_notified_at
            .as_deref()
            .map(|value| parse_db_datetime(value, "match_listeners.last_notified_at")),
    })
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

        // ponytail: v1 follows existing CREATE TABLE IF NOT EXISTS style; schema_version can wait until the project has migrations.
        pool[0]
            .lock()
            .await
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS user_bindings (
                qq INTEGER NOT NULL,
                user_id INTEGER NOT NULL,
                current_username TEXT NOT NULL,
                default_mode INTEGER NOT NULL DEFAULT 0,
                created_at TEXT DEFAULT CURRENT_TIMESTAMP,
                server TEXT NOT NULL DEFAULT 'official',
                PRIMARY KEY(qq, server)
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
                server TEXT NOT NULL DEFAULT 'official',
                UNIQUE(user_id, mode, server, recorded_at)
            );
            CREATE TABLE IF NOT EXISTS user_play_records (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                played_at INTEGER NOT NULL,
                server TEXT NOT NULL DEFAULT 'official',
                UNIQUE(user_id, mode, server, played_at)
            );
            CREATE TABLE IF NOT EXISTS user_next_update (
                user_id INTEGER NOT NULL,
                mode INTEGER NOT NULL,
                next_update INTEGER NOT NULL,
                server TEXT NOT NULL DEFAULT 'official',
                PRIMARY KEY(user_id, mode, server)
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
            CREATE TABLE IF NOT EXISTS match_listeners (
                match_id INTEGER NOT NULL,
                group_id INTEGER NOT NULL,
                creator_qq INTEGER NOT NULL,
                match_name TEXT NOT NULL DEFAULT '',
                last_event_id INTEGER,
                last_notified_event_id INTEGER,
                pending_game_event_id INTEGER,
                created_at TEXT NOT NULL,
                expires_at INTEGER NOT NULL,
                active INTEGER NOT NULL DEFAULT 1,
                last_notified_at TEXT,
                PRIMARY KEY (match_id, group_id)
            );
            CREATE TABLE IF NOT EXISTS osu_user_ids (
                username TEXT NOT NULL,
                user_id INTEGER NOT NULL,
                server TEXT NOT NULL DEFAULT 'official',
                PRIMARY KEY (username, server)
            );
            CREATE INDEX IF NOT EXISTS idx_history_user ON user_stats_history(user_id, mode);
            CREATE INDEX IF NOT EXISTS idx_history_recorded ON user_stats_history(recorded_at);
            CREATE INDEX IF NOT EXISTS idx_play_records_user ON user_play_records(user_id, mode);
            CREATE INDEX IF NOT EXISTS idx_match_listeners_group_active ON match_listeners(group_id, active, expires_at);
            CREATE INDEX IF NOT EXISTS idx_match_listeners_creator_active ON match_listeners(creator_qq, active, expires_at);
            CREATE INDEX IF NOT EXISTS idx_match_listeners_polling ON match_listeners(active, expires_at, last_notified_at);
            ",
            )
            .await?;

        if !has_column(&pool, "user_bindings", "default_mode").await? {
            pool[0]
                .lock()
                .await
                .execute(
                    "ALTER TABLE user_bindings ADD COLUMN default_mode INTEGER NOT NULL DEFAULT 0",
                    (),
                )
                .await?;
        }
        if !has_column(&pool, "match_listeners", "match_name").await? {
            pool[0]
                .lock()
                .await
                .execute(
                    "ALTER TABLE match_listeners ADD COLUMN match_name TEXT NOT NULL DEFAULT ''",
                    (),
                )
                .await?;
        }
        if !has_column(&pool, "match_listeners", "user_id").await? {
            pool[0]
                .lock()
                .await
                .execute("ALTER TABLE match_listeners ADD COLUMN user_id INTEGER", ())
                .await?;
        }
        if !has_column(&pool, "match_listeners", "notification_type").await? {
            pool[0]
                .lock()
                .await
                .execute(
                    "ALTER TABLE match_listeners ADD COLUMN notification_type TEXT NOT NULL DEFAULT 'group'",
                    (),
                )
                .await?;
        }
        // Ensure index exists (for both new and upgraded DBs).
        // 启动期迁移：单次 PRAGMA table_info + ALTER TABLE + CREATE INDEX IF NOT EXISTS
        // 总成本 < 5ms，可接受。规模上去或迁移项增加时，建议改用 schema_version 表
        // 仅在版本不匹配时执行迁移。
        pool[0]
            .lock()
            .await
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_bindings_username ON user_bindings(LOWER(current_username))",
                (),
            )
            .await?;

        // ppy.sb 迁移：添加 server 列
        // ppy.sb 迁移：将相关表重建为包含 server 列，并纳入主键/唯一约束，
        // 以支持同一 qq / 用户在不同服务器（official / ppy.sb）下的数据隔离。
        migrate_tables_add_server(&pool).await?;

        Ok(Self {
            db,
            pool,
            next: AtomicUsize::new(0),
        })
    }

    /// Create a temporary on-disk storage for testing.
    /// Uses a temp file (not `:memory:`) because `Storage::new` requires a file path.
    /// The returned `TempDb` guard cleans up the file on drop.
    #[cfg(test)]
    pub(crate) async fn connect_for_testing() -> DbResult<TempDb> {
        static DB_COUNTER: AtomicU64 = AtomicU64::new(0);
        let counter = DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "osubot-storage-test-{}-{}.db",
            std::process::id(),
            counter
        ));
        let _ = std::fs::remove_file(&path);
        let storage = Storage::new(path.to_str().expect("valid UTF-8 path")).await?;
        Ok(TempDb { path, storage })
    }

    async fn conn(&self) -> tokio::sync::MutexGuard<'_, Connection> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.pool.len();
        self.pool[idx].lock().await
    }

    // ==================== Binding Query ====================

    /// Bind QQ to user_id with current_username. Returns Err if user_id already bound to another QQ.
    ///
    /// 关于 `default_mode` 字段：`INSERT OR REPLACE` 不会保留该列旧值，重绑后会落回 `DEFAULT 0`（Osu）。
    /// 这是 by design——重新绑定视为"建立新的 QQ↔osu 关联"，个人偏好不复用。
    pub async fn bind(
        &self,
        qq: i64,
        user_id: i64,
        current_username: &str,
        server: Server,
    ) -> DbResult<std::result::Result<(), i64>> {
        let conn = self.conn().await;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let result =
            async {
                let mut rows = conn
                    .query(
                        "SELECT qq FROM user_bindings WHERE user_id = ?1 AND server = ?2",
                        params![user_id, server.as_str()],
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
                    "INSERT OR REPLACE INTO user_bindings (qq, user_id, current_username, server) VALUES (?1, ?2, ?3, ?4)",
                    params![qq, user_id, current_username, server.as_str()],
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

    pub async fn unbind(&self, qq: i64, server: Server) -> DbResult<()> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id FROM user_bindings WHERE qq = ?1 AND server = ?2",
                params![qq, server.as_str()],
            )
            .await?;
        let user_id: Option<i64> = rows.next().await?.map(|row| row.get(0)).transpose()?;
        drop(rows);

        conn.execute(
            "DELETE FROM user_bindings WHERE qq = ?1 AND server = ?2",
            params![qq, server.as_str()],
        )
        .await?;

        if let Some(uid) = user_id {
            let mut rows = conn
                .query(
                    "SELECT COUNT(*) FROM user_bindings WHERE user_id = ?1",
                    params![uid],
                )
                .await?;
            let count: i64 = if let Some(row) = rows.next().await? {
                row.get(0)?
            } else {
                0
            };
            drop(rows);
            if count == 0 {
                conn.execute(
                    "DELETE FROM user_next_update WHERE user_id = ?1",
                    params![uid],
                )
                .await?;
            }
        }

        Ok(())
    }

    pub async fn set_user_id(&self, username: &str, user_id: i64, server: Server) -> DbResult<()> {
        self.conn()
            .await
            .execute(
                "INSERT OR REPLACE INTO osu_user_ids (username, user_id, server) VALUES (LOWER(?1), ?2, ?3)",
                params![username, user_id, server.as_str()],
            )
            .await?;
        Ok(())
    }

    /// Get cached osu! user ID (case-insensitive username lookup), scoped by server.
    pub async fn get_user_id(&self, username: &str, server: Server) -> DbResult<Option<i64>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id FROM osu_user_ids WHERE LOWER(username) = LOWER(?1) AND server = ?2",
                params![username, server.as_str()],
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

    pub async fn get_binding(&self, qq: i64, server: Server) -> DbResult<Option<(i64, String)>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id, current_username FROM user_bindings WHERE qq = ?1 AND server = ?2",
                params![qq, server.as_str()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some((row.get(0)?, row.get(1)?)))
        } else {
            Ok(None)
        }
    }

    pub async fn get_default_mode(&self, qq: i64, server: Server) -> DbResult<Option<GameMode>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT default_mode FROM user_bindings WHERE qq = ?1 AND server = ?2",
                params![qq, server.as_str()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            let mode_int: i32 = row.get(0)?;
            match GameMode::try_from(mode_int) {
                Ok(mode) => Ok(Some(mode)),
                Err(_) => {
                    tracing::error!(mode = mode_int, "invalid default_mode value in database");
                    Err(turso::Error::Error(
                        "invalid default_mode value in database".to_string(),
                    ))
                }
            }
        } else {
            Ok(None)
        }
    }

    pub async fn set_default_mode(
        &self,
        qq: i64,
        mode: GameMode,
        server: Server,
    ) -> DbResult<bool> {
        let rows = self
            .conn()
            .await
            .execute(
                "UPDATE user_bindings SET default_mode = ?1 WHERE qq = ?2 AND server = ?3",
                params![i32::from(mode), qq, server.as_str()],
            )
            .await?;
        Ok(rows > 0)
    }

    pub async fn find_qq_by_username(
        &self,
        username: &str,
        server: Server,
    ) -> DbResult<Option<i64>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT qq FROM user_bindings WHERE LOWER(current_username) = LOWER(?1) AND server = ?2",
                params![username, server.as_str()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub async fn update_binding_username(
        &self,
        qq: i64,
        server: Server,
        new_username: &str,
    ) -> DbResult<()> {
        self.conn()
            .await
            .execute(
                "UPDATE user_bindings SET current_username = ?1 WHERE qq = ?2 AND server = ?3",
                params![new_username, qq, server.as_str()],
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
        drop(rows);
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
        server: Server,
    ) -> DbResult<()> {
        self.conn().await
            .execute(
                "INSERT OR IGNORE INTO user_stats_history (user_id, mode, server, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    user_id,
                    i32::from(mode),
                    server.as_str(),
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
        server: Server,
    ) -> DbResult<Option<UserStatsSnapshot>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
                 FROM user_stats_history
                 WHERE user_id = ?1 AND mode = ?2 AND server = ?3
                 ORDER BY recorded_at DESC
                 LIMIT 1",
                params![user_id, i32::from(mode), server.as_str()],
            )
            .await?;
        if let Some(row) = rows.next().await? {
            Ok(Some(row_to_snapshot(&row, 0)?))
        } else {
            Ok(None)
        }
    }

    pub async fn get_snapshots_within_hours(
        &self,
        user_id: i64,
        mode: GameMode,
        hours: i64,
        server: Server,
    ) -> DbResult<Vec<(DateTime<Utc>, UserStatsSnapshot)>> {
        let cutoff = Utc::now() - chrono::TimeDelta::hours(hours);
        let cutoff_str = cutoff.to_rfc3339();

        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime
                 FROM user_stats_history
                 WHERE user_id = ?1 AND mode = ?2 AND server = ?3 AND recorded_at >= ?4
                 ORDER BY recorded_at ASC",
                params![user_id, i32::from(mode), server.as_str(), cutoff_str],
            )
            .await?;

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let recorded_str: String = row.get(0)?;
            if let Ok(dt) = DateTime::parse_from_rfc3339(&recorded_str) {
                results.push((dt.with_timezone(&Utc), row_to_snapshot(&row, 1)?));
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
        server: Server,
    ) -> DbResult<HashMap<i64, UserStatsSnapshot>> {
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
             WHERE mode = ? AND server = ? AND recorded_at >= ? AND user_id IN ({placeholders})
             ORDER BY user_id, recorded_at ASC"
        );

        let mut args: Vec<turso::Value> = Vec::with_capacity(unique_user_ids.len() + 3);
        args.push(i32::from(mode).into());
        args.push(server.as_str().into());
        args.push(cutoff_str.into());
        for user_id in unique_user_ids {
            args.push(user_id.into());
        }

        let conn = self.conn().await;
        let mut rows = conn.query(&sql, args).await?;
        let mut closest: HashMap<i64, (u64, UserStatsSnapshot)> = HashMap::new();
        while let Some(row) = rows.next().await? {
            let user_id: i64 = row.get(0)?;
            let recorded_str: String = row.get(1)?;
            let Ok(recorded_at) = DateTime::parse_from_rfc3339(&recorded_str) else {
                continue;
            };
            let distance = (recorded_at.with_timezone(&Utc) - target)
                .num_seconds()
                .unsigned_abs();
            let stats = row_to_snapshot(&row, 2)?;

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
        server: Server,
    ) -> DbResult<Option<UserStatsSnapshot>> {
        let all = self
            .get_snapshots_within_hours(user_id, mode, 36, server)
            .await?;
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
        server: Server,
    ) -> DbResult<Option<(DateTime<Utc>, UserStatsSnapshot)>> {
        let now = Utc::now();
        let target_time = now - chrono::TimeDelta::hours(target_hours_ago);
        let earliest = now - chrono::TimeDelta::hours(max_lookback);

        let all = self
            .get_snapshots_within_hours(user_id, mode, max_lookback, server)
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
        server: Server,
    ) -> DbResult<i64> {
        if timestamps.is_empty() {
            return Ok(0);
        }

        let placeholders = std::iter::repeat_n("(?, ?, ?, ?)", timestamps.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT OR IGNORE INTO user_play_records (user_id, mode, server, played_at) VALUES {placeholders}"
        );

        let mut args: Vec<turso::Value> = Vec::with_capacity(timestamps.len() * 4);
        for &timestamp in timestamps {
            args.push(user_id.into());
            args.push(i32::from(mode).into());
            args.push(server.as_str().into());
            args.push(timestamp.into());
        }

        let conn = self.conn().await;
        let count = conn.execute(&sql, args).await?;
        Ok(count as i64)
    }

    /// Check if user has any play records since the given UTC timestamp
    pub async fn has_play_since(
        &self,
        user_id: i64,
        mode: GameMode,
        since_ts: i64,
        server: Server,
    ) -> DbResult<bool> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT 1 FROM user_play_records WHERE user_id = ?1 AND mode = ?2 AND played_at >= ?3 AND server = ?4 LIMIT 1",
                params![user_id, i32::from(mode), since_ts, server.as_str()],
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
        server: Server,
    ) -> DbResult<Option<UserChange>> {
        let snapshot = self
            .get_closest_snapshot_to_hours_ago(user_id, mode, 24, 36, server)
            .await?;

        match snapshot {
            None => Ok(None),
            Some((_, past)) => {
                let rank_change = if current.rank != 0 && past.rank.is_some_and(|r| r != 0) {
                    Some(past.rank.unwrap() - current.rank)
                } else {
                    None
                };
                let country_rank_change =
                    if current.country_rank != 0 && past.country_rank.is_some_and(|r| r != 0) {
                        Some(past.country_rank.unwrap() - current.country_rank)
                    } else {
                        None
                    };
                let playcount_change =
                    if current.playcount != 0 && past.playcount.is_some_and(|r| r != 0) {
                        Some(current.playcount - past.playcount.unwrap())
                    } else {
                        None
                    };
                let hits_change = if current.hits != 0 && past.hits.is_some_and(|r| r != 0) {
                    Some(current.hits - past.hits.unwrap())
                } else {
                    None
                };
                let playtime_change =
                    if current.playtime != 0 && past.playtime.is_some_and(|r| r != 0) {
                        Some(current.playtime - past.playtime.unwrap())
                    } else {
                        None
                    };

                Ok(Some(UserChange {
                    rank_change,
                    country_rank_change,
                    pp_change: past.pp.map(|p| current.pp - p),
                    accuracy_change: past.accuracy.map(|a| current.accuracy - a),
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
                params![user_id, i32::from(mode)],
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
                params![user_id, i32::from(mode), time.to_rfc3339()],
            )
            .await?;
        Ok(())
    }

    // ==================== Next Update (for scheduler dynamic intervals) ====================

    pub async fn get_next_update(
        &self,
        user_id: i64,
        mode: GameMode,
        server: Server,
    ) -> DbResult<Option<DateTime<Utc>>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT next_update FROM user_next_update WHERE user_id = ?1 AND mode = ?2 AND server = ?3",
                params![user_id, i32::from(mode), server.as_str()],
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
        server: Server,
        time: DateTime<Utc>,
    ) -> DbResult<()> {
        self.conn().await
            .execute(
                "INSERT OR REPLACE INTO user_next_update (user_id, mode, server, next_update) VALUES (?1, ?2, ?3, ?4)",
                params![
                    user_id,
                    i32::from(mode),
                    server.as_str(),
                    time.timestamp()
                ],
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

    pub async fn get_due_users(&self) -> DbResult<Vec<(i64, GameMode, Server)>> {
        let now_ts = Utc::now().timestamp();

        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT user_id, mode, server FROM user_next_update WHERE next_update <= ?1
                UNION ALL
                SELECT b.user_id AS user_id, m.mode, b.server
                FROM user_bindings b
                CROSS JOIN (
                SELECT 0 AS mode
                UNION ALL SELECT 1
                UNION ALL SELECT 2
                UNION ALL SELECT 3
                ) AS m
                WHERE NOT EXISTS (
                    SELECT 1 FROM user_next_update n
                    WHERE n.user_id = b.user_id AND n.mode = m.mode AND n.server = b.server
                )",
                params![now_ts],
            )
            .await?;

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let user_id: i64 = row.get(0)?;
            let mode_int: i32 = row.get(1)?;
            let mode = GameMode::try_from(mode_int).unwrap_or(GameMode::Osu);
            let server_str: String = row.get(2)?;
            let server = Server::from_str(&server_str);
            results.push((user_id, mode, server));
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

    pub async fn start_match_listener(&self, params: MatchListenerStartParams) -> DbResult<()> {
        let MatchListenerStartParams {
            match_id,
            group_id,
            user_id,
            notification_type,
            creator_qq,
            match_name,
            expires_at,
            initial_last_event_id,
            initial_last_notified_event_id,
        } = params;
        let created_at = Utc::now().to_rfc3339();
        let now = Utc::now().timestamp();
        self.conn()
            .await
            .execute(
                "INSERT INTO match_listeners (
                    match_id,
                    group_id,
                    creator_qq,
                    match_name,
                    last_event_id,
                    last_notified_event_id,
                    pending_game_event_id,
                    created_at,
                    expires_at,
                    active,
                    last_notified_at,
                    user_id,
                    notification_type
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, 1, NULL, ?10, ?11)
                ON CONFLICT(match_id, group_id) DO UPDATE SET
                    creator_qq = excluded.creator_qq,
                    match_name = excluded.match_name,
                    created_at = CASE
                        WHEN match_listeners.active = 1 AND match_listeners.expires_at >= ?9 THEN match_listeners.created_at
                        ELSE excluded.created_at
                    END,
                    expires_at = excluded.expires_at,
                    active = 1,
                    last_event_id = CASE
                        WHEN match_listeners.active = 1 AND match_listeners.expires_at >= ?9 THEN match_listeners.last_event_id
                        ELSE excluded.last_event_id
                    END,
                    last_notified_event_id = CASE
                        WHEN match_listeners.active = 1 AND match_listeners.expires_at >= ?9 THEN match_listeners.last_notified_event_id
                        ELSE excluded.last_notified_event_id
                    END,
                    pending_game_event_id = CASE
                        WHEN match_listeners.active = 1 AND match_listeners.expires_at >= ?9 THEN match_listeners.pending_game_event_id
                        ELSE NULL
                    END,
                    last_notified_at = CASE
                        WHEN match_listeners.active = 1 AND match_listeners.expires_at >= ?9 THEN match_listeners.last_notified_at
                        ELSE NULL
                    END,
                    user_id = excluded.user_id,
                    notification_type = excluded.notification_type",
                params![match_id, group_id, creator_qq, match_name, initial_last_event_id, initial_last_notified_event_id, created_at, expires_at, now, user_id, notification_type],
            )
            .await?;
        Ok(())
    }

    pub async fn stop_match_listener(&self, match_id: i64, group_id: i64) -> DbResult<bool> {
        let rows = self
            .conn()
            .await
            .execute(
                "UPDATE match_listeners
                 SET active = 0
                 WHERE match_id = ?1 AND group_id = ?2 AND active = 1",
                params![match_id, group_id],
            )
            .await?;
        Ok(rows > 0)
    }

    /// Deactivate all active listeners keyed by `group_id`.
    ///
    /// For private-chat listeners the caller passes `-user_id` as `group_id`,
    /// so this function doubles as "stop all listeners for a user" without
    /// needing a separate query path.
    pub async fn stop_all_match_listeners_in_group(&self, group_id: i64) -> DbResult<u64> {
        self.conn()
            .await
            .execute(
                "UPDATE match_listeners
                 SET active = 0
                 WHERE group_id = ?1 AND active = 1",
                params![group_id],
            )
            .await
    }

    pub async fn get_match_listener(
        &self,
        match_id: i64,
        group_id: i64,
    ) -> DbResult<Option<MatchListener>> {
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT match_id, group_id, creator_qq, match_name, last_event_id, last_notified_event_id,
                        pending_game_event_id, created_at, expires_at, active, last_notified_at,
                        user_id, notification_type
                 FROM match_listeners
                 WHERE match_id = ?1 AND group_id = ?2",
                params![match_id, group_id],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            Ok(Some(row_to_match_listener(&row)?))
        } else {
            Ok(None)
        }
    }

    pub async fn list_active_match_listeners_by_group(
        &self,
        group_id: i64,
    ) -> DbResult<Vec<MatchListener>> {
        let now_ts = Utc::now().timestamp();
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT match_id, group_id, creator_qq, match_name, last_event_id, last_notified_event_id,
                        pending_game_event_id, created_at, expires_at, active, last_notified_at,
                        user_id, notification_type
                 FROM match_listeners
                 WHERE group_id = ?1 AND active = 1 AND expires_at >= ?2
                 ORDER BY created_at ASC, match_id ASC",
                params![group_id, now_ts],
            )
            .await?;

        let mut listeners = Vec::new();
        while let Some(row) = rows.next().await? {
            listeners.push(row_to_match_listener(&row)?);
        }
        Ok(listeners)
    }

    pub async fn list_active_match_listeners_due_for_polling(
        &self,
    ) -> DbResult<Vec<MatchListener>> {
        let now_ts = Utc::now().timestamp();
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT match_id, group_id, creator_qq, match_name, last_event_id, last_notified_event_id,
                        pending_game_event_id, created_at, expires_at, active, last_notified_at,
                        user_id, notification_type
                 FROM match_listeners
                 WHERE active = 1 AND expires_at >= ?1
                 ORDER BY COALESCE(last_notified_at, created_at) ASC, group_id ASC, match_id ASC",
                params![now_ts],
            )
            .await?;

        let mut listeners = Vec::new();
        while let Some(row) = rows.next().await? {
            listeners.push(row_to_match_listener(&row)?);
        }
        Ok(listeners)
    }

    pub async fn update_match_listener_progress(
        &self,
        match_id: i64,
        group_id: i64,
        last_event_id: Option<i64>,
        last_notified_event_id: Option<i64>,
        pending_game_event_id: Option<i64>,
        touch_last_notified_at: bool,
    ) -> DbResult<bool> {
        let last_notified_at = touch_last_notified_at.then(|| Utc::now().to_rfc3339());
        let rows = self
            .conn()
            .await
            .execute(
                "UPDATE match_listeners
                 SET last_event_id = ?3,
                     last_notified_event_id = ?4,
                     pending_game_event_id = ?5,
                     last_notified_at = CASE
                         WHEN ?6 = 1 THEN ?7
                         ELSE last_notified_at
                     END
                 WHERE match_id = ?1 AND group_id = ?2",
                params![
                    match_id,
                    group_id,
                    last_event_id,
                    last_notified_event_id,
                    pending_game_event_id,
                    if touch_last_notified_at { 1 } else { 0 },
                    last_notified_at,
                ],
            )
            .await?;
        Ok(rows > 0)
    }

    pub async fn count_active_match_listeners_in_group(&self, group_id: i64) -> DbResult<u64> {
        let now_ts = Utc::now().timestamp();
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(*)
                 FROM match_listeners
                 WHERE group_id = ?1 AND active = 1 AND expires_at >= ?2",
                params![group_id, now_ts],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let count: i64 = row.get(0)?;
            Ok(count as u64)
        } else {
            Ok(0)
        }
    }

    pub async fn count_active_match_listeners_for_user(&self, user_id: i64) -> DbResult<u64> {
        let now_ts = Utc::now().timestamp();
        let conn = self.conn().await;
        let mut rows = conn
            .query(
                "SELECT COUNT(*)
                 FROM match_listeners
                 WHERE user_id = ?1 AND notification_type = 'private' AND active = 1 AND expires_at >= ?2",
                params![user_id, now_ts],
            )
            .await?;

        if let Some(row) = rows.next().await? {
            let count: i64 = row.get(0)?;
            Ok(count as u64)
        } else {
            Ok(0)
        }
    }

    pub async fn is_match_listener_group_limit_reached(
        &self,
        group_id: i64,
        limit: u64,
    ) -> DbResult<bool> {
        Ok(self.count_active_match_listeners_in_group(group_id).await? >= limit)
    }

    pub async fn expire_old_match_listeners(&self) -> DbResult<u64> {
        let now_ts = Utc::now().timestamp();
        self.conn()
            .await
            .execute(
                "UPDATE match_listeners
                 SET active = 0
                 WHERE active = 1 AND expires_at < ?1",
                params![now_ts],
            )
            .await
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
                "INSERT INTO user_stats_history (user_id, mode, server, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    user_id,
                    i32::from(mode),
                    Server::Official.as_str(),
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
            .get_baseline_snapshots_for_users(&[101, 202, 303], mode, 24, 36, Server::Official)
            .await
            .expect("batch baseline query succeeds");

        assert_eq!(baselines.len(), 2);
        assert_eq!(
            baselines.get(&101).expect("user 101 baseline").pp,
            Some(101.23)
        );
        assert_eq!(
            baselines.get(&202).expect("user 202 baseline").pp,
            Some(202.25)
        );
        assert!(!baselines.contains_key(&303));
    }

    #[tokio::test]
    async fn batch_baseline_tolerates_duplicate_user_ids_and_handles_empty_input() {
        let storage = test_storage().await;
        let now = Utc::now();
        let mode = GameMode::Osu;

        let empty = storage
            .get_baseline_snapshots_for_users(&[], mode, 24, 36, Server::Official)
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
            .get_baseline_snapshots_for_users(&[404, 404, 404], mode, 24, 36, Server::Official)
            .await
            .expect("duplicate user IDs batch query succeeds");

        assert_eq!(baselines.len(), 1);
        assert_eq!(
            baselines.get(&404).expect("user 404 baseline").hits,
            Some(4_040)
        );
    }

    #[tokio::test]
    async fn test_set_and_get_default_mode() {
        let storage = Storage::connect_for_testing().await.unwrap();
        storage
            .bind(10001, 12345, "test_user", Server::Official)
            .await
            .unwrap()
            .unwrap();

        // 初始为 Osu (DEFAULT 0)
        assert_eq!(
            storage
                .get_default_mode(10001, Server::Official)
                .await
                .unwrap(),
            Some(GameMode::Osu)
        );

        assert!(storage
            .set_default_mode(10001, GameMode::Taiko, Server::Official)
            .await
            .unwrap());
        assert_eq!(
            storage
                .get_default_mode(10001, Server::Official)
                .await
                .unwrap(),
            Some(GameMode::Taiko)
        );

        assert!(storage
            .set_default_mode(10001, GameMode::Mania, Server::Official)
            .await
            .unwrap());
        assert_eq!(
            storage
                .get_default_mode(10001, Server::Official)
                .await
                .unwrap(),
            Some(GameMode::Mania)
        );
    }

    #[tokio::test]
    async fn test_find_qq_by_username_case_insensitive() {
        let storage = Storage::connect_for_testing().await.unwrap();
        storage
            .bind(10001, 12345, "TestUser", Server::Official)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            storage
                .find_qq_by_username("testuser", Server::Official)
                .await
                .unwrap(),
            Some(10001)
        );
        assert_eq!(
            storage
                .find_qq_by_username("TESTUSER", Server::Official)
                .await
                .unwrap(),
            Some(10001)
        );
        assert_eq!(
            storage
                .find_qq_by_username("TestUser", Server::Official)
                .await
                .unwrap(),
            Some(10001)
        );

        assert_eq!(
            storage
                .find_qq_by_username("nobody", Server::Official)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn test_set_default_mode_catch() {
        let storage = Storage::connect_for_testing().await.unwrap();
        storage
            .bind(10001, 12345, "test_user", Server::Official)
            .await
            .unwrap()
            .unwrap();

        assert!(storage
            .set_default_mode(10001, GameMode::Catch, Server::Official)
            .await
            .unwrap());
        assert_eq!(
            storage
                .get_default_mode(10001, Server::Official)
                .await
                .unwrap(),
            Some(GameMode::Catch)
        );
    }

    #[tokio::test]
    async fn test_default_mode_unbound_qq() {
        let storage = Storage::connect_for_testing().await.unwrap();
        assert_eq!(
            storage
                .get_default_mode(99999, Server::Official)
                .await
                .unwrap(),
            None
        );
        assert!(!storage
            .set_default_mode(99999, GameMode::Mania, Server::Official)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn test_default_mode_after_unbind_rebind() {
        let storage = Storage::connect_for_testing().await.unwrap();
        storage
            .bind(10001, 12345, "test_user", Server::Official)
            .await
            .unwrap()
            .unwrap();
        storage
            .set_default_mode(10001, GameMode::Taiko, Server::Official)
            .await
            .unwrap();
        assert_eq!(
            storage
                .get_default_mode(10001, Server::Official)
                .await
                .unwrap(),
            Some(GameMode::Taiko)
        );

        storage.unbind(10001, Server::Official).await.unwrap();
        assert_eq!(
            storage
                .get_default_mode(10001, Server::Official)
                .await
                .unwrap(),
            None
        );

        storage
            .bind(10001, 12345, "test_user", Server::Official)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            storage
                .get_default_mode(10001, Server::Official)
                .await
                .unwrap(),
            Some(GameMode::Osu)
        );
    }

    #[tokio::test]
    async fn test_find_qq_after_username_update() {
        let storage = Storage::connect_for_testing().await.unwrap();
        storage
            .bind(10001, 12345, "OldName", Server::Official)
            .await
            .unwrap()
            .unwrap();
        storage
            .bind(10001, 67890, "OtherName", Server::PpySb)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            storage
                .find_qq_by_username("OldName", Server::Official)
                .await
                .unwrap(),
            Some(10001)
        );

        storage
            .update_binding_username(10001, Server::Official, "NewName")
            .await
            .unwrap();

        assert_eq!(
            storage
                .find_qq_by_username("OldName", Server::Official)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            storage
                .find_qq_by_username("NewName", Server::Official)
                .await
                .unwrap(),
            Some(10001)
        );
        // 另一服务器的绑定不受影响
        assert_eq!(
            storage
                .find_qq_by_username("OtherName", Server::PpySb)
                .await
                .unwrap(),
            Some(10001)
        );
    }
}

#[cfg(test)]
mod match_listener {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const GROUP_ID: i64 = 123_456_789;
    const CREATOR_QQ: i64 = 987_654_321;
    const MATCH_ID: i64 = 12_345_678;
    static MATCH_LISTENER_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

    async fn create_listener(
        storage: &Storage,
        match_id: i64,
        group_id: i64,
        creator_qq: i64,
        expires_at: i64,
    ) {
        storage
            .start_match_listener(MatchListenerStartParams {
                match_id,
                group_id: Some(group_id),
                user_id: None,
                notification_type: "group".to_string(),
                creator_qq,
                match_name: format!("MP #{match_id}"),
                expires_at,
                initial_last_event_id: None,
                initial_last_notified_event_id: None,
            })
            .await
            .expect("start match listener");
    }

    #[tokio::test]
    async fn persists_listener_across_storage_reopen() {
        let id = MATCH_LISTENER_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        let db_path = std::env::temp_dir().join(format!(
            "osubot-storage-match-listener-{}-{id}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&db_path);

        let storage = Storage::new(db_path.to_str().expect("temp db path is valid UTF-8"))
            .await
            .expect("create storage");
        let expires_at = Utc::now().timestamp() + 3600;

        create_listener(&storage, MATCH_ID, GROUP_ID, CREATOR_QQ, expires_at).await;
        drop(storage);

        let reopened = Storage::new(db_path.to_str().expect("valid UTF-8 path"))
            .await
            .expect("reopen storage");

        let listeners = reopened
            .list_active_match_listeners_by_group(GROUP_ID)
            .await
            .expect("list active listeners");

        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].match_id, MATCH_ID);
        assert_eq!(listeners[0].group_id, Some(GROUP_ID));
        assert_eq!(listeners[0].match_name, format!("MP #{MATCH_ID}"));

        std::fs::remove_file(&db_path).expect("cleanup reopened db path");
    }

    #[tokio::test]
    async fn advance_cursor_round_trips_last_event_id() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let expires_at = Utc::now().timestamp() + 3600;
        create_listener(&storage, MATCH_ID, GROUP_ID, CREATOR_QQ, expires_at).await;

        assert!(storage
            .update_match_listener_progress(MATCH_ID, GROUP_ID, Some(100), None, None, false)
            .await
            .expect("advance cursor"));

        let listener = storage
            .get_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("get listener")
            .expect("listener exists");

        assert_eq!(listener.last_event_id, Some(100));
    }

    #[tokio::test]
    async fn detects_group_limit() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let expires_at = Utc::now().timestamp() + 3600;

        for offset in 0..3 {
            create_listener(
                &storage,
                MATCH_ID + offset,
                GROUP_ID,
                CREATOR_QQ + offset,
                expires_at,
            )
            .await;
        }

        assert_eq!(
            storage
                .count_active_match_listeners_in_group(GROUP_ID)
                .await
                .expect("count group listeners"),
            3
        );
        assert!(storage
            .is_match_listener_group_limit_reached(GROUP_ID, 3)
            .await
            .expect("check group limit"));
        assert!(
            storage
                .is_match_listener_group_limit_reached(GROUP_ID, 4)
                .await
                .expect("check fourth listener detectability")
                == false
        );
    }

    #[tokio::test]
    async fn excludes_expired_listener_from_polling() {
        let storage = Storage::connect_for_testing().await.unwrap();

        create_listener(
            &storage,
            MATCH_ID,
            GROUP_ID,
            CREATOR_QQ,
            Utc::now().timestamp() - 60,
        )
        .await;
        create_listener(
            &storage,
            MATCH_ID + 1,
            GROUP_ID,
            CREATOR_QQ + 1,
            Utc::now().timestamp() + 3600,
        )
        .await;

        let expired_rows = storage
            .expire_old_match_listeners()
            .await
            .expect("expire old listeners");
        assert_eq!(expired_rows, 1);

        let polling_list = storage
            .list_active_match_listeners_due_for_polling()
            .await
            .expect("list due listeners");

        assert_eq!(polling_list.len(), 1);
        assert_eq!(polling_list[0].match_id, MATCH_ID + 1);
        assert!(polling_list[0].active);
        assert!(polling_list
            .iter()
            .all(|listener| listener.expires_at >= Utc::now().timestamp()));
    }

    #[tokio::test]
    async fn stops_single_and_group_listeners() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let expires_at = Utc::now().timestamp() + 3600;

        create_listener(&storage, MATCH_ID, GROUP_ID, CREATOR_QQ, expires_at).await;
        create_listener(&storage, MATCH_ID + 1, GROUP_ID, CREATOR_QQ + 1, expires_at).await;
        create_listener(
            &storage,
            MATCH_ID + 2,
            GROUP_ID + 1,
            CREATOR_QQ + 2,
            expires_at,
        )
        .await;

        assert!(storage
            .stop_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("stop single listener"));
        assert_eq!(
            storage
                .list_active_match_listeners_by_group(GROUP_ID)
                .await
                .expect("list group after stop")
                .len(),
            1
        );

        assert_eq!(
            storage
                .stop_all_match_listeners_in_group(GROUP_ID)
                .await
                .expect("stop group listeners"),
            1
        );
        assert!(storage
            .list_active_match_listeners_by_group(GROUP_ID)
            .await
            .expect("list after stop all")
            .is_empty());
        assert_eq!(
            storage
                .list_active_match_listeners_by_group(GROUP_ID + 1)
                .await
                .expect("other group untouched")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn advances_notified_event_and_pending_game_marker() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let expires_at = Utc::now().timestamp() + 3600;
        create_listener(&storage, MATCH_ID, GROUP_ID, CREATOR_QQ, expires_at).await;

        assert!(storage
            .update_match_listener_progress(MATCH_ID, GROUP_ID, None, Some(88), Some(99), true)
            .await
            .expect("update listener progress"));

        let listener = storage
            .get_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("get listener after updates")
            .expect("listener exists after updates");

        assert_eq!(listener.last_notified_event_id, Some(88));
        assert_eq!(listener.pending_game_event_id, Some(99));
        assert!(listener.last_notified_at.is_some());

        assert!(storage
            .update_match_listener_progress(MATCH_ID, GROUP_ID, None, None, None, false)
            .await
            .expect("clear pending event"));

        let listener = storage
            .get_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("get listener after clear")
            .expect("listener exists after clear");
        assert_eq!(listener.pending_game_event_id, None);
    }

    #[tokio::test]
    async fn restarting_inactive_listener_resets_cursor_and_pending_state() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let expires_at = Utc::now().timestamp() + 3600;
        create_listener(&storage, MATCH_ID, GROUP_ID, CREATOR_QQ, expires_at).await;

        assert!(storage
            .update_match_listener_progress(
                MATCH_ID,
                GROUP_ID,
                Some(100),
                Some(99),
                Some(101),
                true,
            )
            .await
            .expect("set listener progress"));
        assert!(storage
            .stop_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("stop listener"));

        create_listener(
            &storage,
            MATCH_ID,
            GROUP_ID,
            CREATOR_QQ + 1,
            expires_at + 3600,
        )
        .await;

        let listener = storage
            .get_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("get restarted listener")
            .expect("listener exists after restart");

        assert!(listener.active);
        assert_eq!(listener.creator_qq, CREATOR_QQ + 1);
        assert_eq!(listener.last_event_id, None);
        assert_eq!(listener.last_notified_event_id, None);
        assert_eq!(listener.pending_game_event_id, None);
        assert_eq!(listener.last_notified_at, None);
    }

    #[tokio::test]
    async fn restarting_expired_active_listener_resets_cursor_and_pending_state() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let expires_at = Utc::now().timestamp() - 60;
        create_listener(&storage, MATCH_ID, GROUP_ID, CREATOR_QQ, expires_at).await;

        assert!(storage
            .update_match_listener_progress(
                MATCH_ID,
                GROUP_ID,
                Some(100),
                Some(99),
                Some(101),
                true,
            )
            .await
            .expect("set listener progress"));

        create_listener(
            &storage,
            MATCH_ID,
            GROUP_ID,
            CREATOR_QQ + 1,
            Utc::now().timestamp() + 3600,
        )
        .await;

        let listener = storage
            .get_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("get restarted listener")
            .expect("listener exists after restart");

        assert!(listener.active);
        assert_eq!(listener.creator_qq, CREATOR_QQ + 1);
        assert_eq!(listener.last_event_id, None);
        assert_eq!(listener.last_notified_event_id, None);
        assert_eq!(listener.pending_game_event_id, None);
        assert_eq!(listener.last_notified_at, None);
    }

    #[tokio::test]
    async fn test_start_match_listener_private() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let params = MatchListenerStartParams {
            match_id: 100,
            group_id: Some(0),
            user_id: Some(12345),
            notification_type: "private".to_string(),
            creator_qq: 12345,
            match_name: "Test Match".to_string(),
            expires_at: Utc::now().timestamp() + 3600,
            initial_last_event_id: None,
            initial_last_notified_event_id: None,
        };
        storage.start_match_listener(params).await.unwrap();
        let listeners = storage
            .list_active_match_listeners_due_for_polling()
            .await
            .unwrap();
        assert_eq!(listeners.len(), 1);
        assert_eq!(listeners[0].group_id, Some(0));
        assert_eq!(listeners[0].user_id, Some(12345));
        assert_eq!(listeners[0].notification_type, "private");
    }

    #[tokio::test]
    async fn update_match_listener_progress_updates_all_cursor_fields_atomically() {
        let storage = Storage::connect_for_testing().await.unwrap();
        let expires_at = Utc::now().timestamp() + 3600;
        create_listener(&storage, MATCH_ID, GROUP_ID, CREATOR_QQ, expires_at).await;

        assert!(storage
            .update_match_listener_progress(
                MATCH_ID,
                GROUP_ID,
                Some(100),
                Some(99),
                Some(101),
                true
            )
            .await
            .expect("atomic cursor update"));

        let listener = storage
            .get_match_listener(MATCH_ID, GROUP_ID)
            .await
            .expect("get listener after atomic update")
            .expect("listener exists after atomic update");

        assert_eq!(listener.last_event_id, Some(100));
        assert_eq!(listener.last_notified_event_id, Some(99));
        assert_eq!(listener.pending_game_event_id, Some(101));
        assert!(listener.last_notified_at.is_some());
    }

    #[tokio::test]
    async fn server_bindings_are_isolated_per_server() {
        let storage = Storage::connect_for_testing().await.unwrap();

        fn local_stats(pp: f64) -> crate::types::UserStats {
            crate::types::UserStats {
                user_id: 0,
                username: String::new(),
                pp,
                rank: 0,
                country_rank: 0,
                country_code: "XX".to_string(),
                ranked_score: 0,
                accuracy: 0.0,
                playcount: 0,
                hits: 0,
                playtime: 0,
                rank_change: None,
                country_rank_change: None,
                cover_url: None,
            }
        }

        // 同一 qq 分别绑定官方服与 ppy.sb 服，应互不覆盖
        storage
            .bind(123, 111, "official_user", Server::Official)
            .await
            .expect("bind official")
            .expect("official bind ok");
        storage
            .bind(123, 222, "ppy_user", Server::PpySb)
            .await
            .expect("bind ppy.sb")
            .expect("ppy.sb bind ok");

        assert_eq!(
            storage.get_binding(123, Server::Official).await.unwrap(),
            Some((111, "official_user".to_string()))
        );
        assert_eq!(
            storage.get_binding(123, Server::PpySb).await.unwrap(),
            Some((222, "ppy_user".to_string()))
        );

        // 两服各自的统计快照互不串扰
        storage
            .save_stats(111, GameMode::Osu, &local_stats(100.0), Server::Official)
            .await
            .unwrap();
        storage
            .save_stats(222, GameMode::Osu, &local_stats(200.0), Server::PpySb)
            .await
            .unwrap();

        let official = storage
            .get_latest_snapshot(111, GameMode::Osu, Server::Official)
            .await
            .unwrap()
            .expect("official snapshot");
        let ppy = storage
            .get_latest_snapshot(222, GameMode::Osu, Server::PpySb)
            .await
            .unwrap()
            .expect("ppy.sb snapshot");
        assert_eq!(official.pp, Some(100.0));
        assert_eq!(ppy.pp, Some(200.0));
    }

    #[tokio::test]
    async fn user_id_cache_isolated_per_server() {
        let storage = Storage::connect_for_testing().await.unwrap();

        // 同一用户名在两服对应不同 user_id（ppy.sb 的 id 空间与官方不同）
        storage
            .set_user_id("mrekk", 1, Server::Official)
            .await
            .unwrap();
        storage
            .set_user_id("mrekk", 999, Server::PpySb)
            .await
            .unwrap();

        assert_eq!(
            storage
                .get_user_id("mrekk", Server::Official)
                .await
                .unwrap(),
            Some(1)
        );
        assert_eq!(
            storage.get_user_id("mrekk", Server::PpySb).await.unwrap(),
            Some(999)
        );

        // 官方路径不会因 ppy.sb 的缓存而拿到错误 id
        assert_ne!(
            storage
                .get_user_id("mrekk", Server::Official)
                .await
                .unwrap(),
            storage.get_user_id("mrekk", Server::PpySb).await.unwrap()
        );
    }

    #[tokio::test]
    async fn migration_adds_server_column_preserving_data() {
        use turso::Builder;

        static DB_COUNTER: AtomicU64 = AtomicU64::new(0);
        let counter = DB_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "osubot-migrate-test-{}-{}.db",
            std::process::id(),
            counter
        ));
        let _ = std::fs::remove_file(&path);

        // 用旧 schema（无 server 列）写入数据，模拟升级前的数据库
        {
            let db = Builder::new_local(path.to_str().unwrap()).build().await.unwrap();
            let conn = db.connect().unwrap();
            conn.execute_batch(
                "CREATE TABLE user_bindings (qq INTEGER NOT NULL, user_id INTEGER NOT NULL, current_username TEXT NOT NULL, default_mode INTEGER NOT NULL DEFAULT 0, created_at TEXT DEFAULT CURRENT_TIMESTAMP, PRIMARY KEY(qq));
                 CREATE TABLE user_stats_history (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, mode INTEGER NOT NULL, recorded_at TEXT DEFAULT CURRENT_TIMESTAMP, pp REAL, rank INTEGER, country_rank INTEGER, ranked_score INTEGER, accuracy REAL, playcount INTEGER, hits INTEGER, playtime INTEGER, UNIQUE(user_id, mode, recorded_at));
                 CREATE TABLE user_play_records (id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, mode INTEGER NOT NULL, played_at INTEGER NOT NULL, UNIQUE(user_id, mode, played_at));
                 CREATE TABLE user_next_update (user_id INTEGER NOT NULL, mode INTEGER NOT NULL, next_update INTEGER NOT NULL, PRIMARY KEY(user_id, mode));
                 CREATE TABLE osu_user_ids (username TEXT PRIMARY KEY, user_id INTEGER NOT NULL);",
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO user_bindings (qq, user_id, current_username) VALUES (123, 111, 'official_user')",
                (),
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO osu_user_ids (username, user_id) VALUES ('mrekk', 1)",
                (),
            )
            .await
            .unwrap();
            conn.execute(
                "INSERT INTO user_next_update (user_id, mode, next_update) VALUES (111, 0, 123456)",
                (),
            )
            .await
            .unwrap();
            drop(conn);
            drop(db);
        }

        // 触发迁移（重建为带 server 列的 schema）
        let storage = Storage::new(path.to_str().unwrap()).await.unwrap();

        // 存量数据应保留
        assert_eq!(
            storage.get_binding(123, Server::Official).await.unwrap(),
            Some((111, "official_user".to_string()))
        );
        assert_eq!(
            storage.get_user_id("mrekk", Server::Official).await.unwrap(),
            Some(1)
        );
        // server 列生效：另一服查不到该缓存（不会被官方查询误用）
        assert_eq!(
            storage.get_user_id("mrekk", Server::PpySb).await.unwrap(),
            None
        );

        let _ = std::fs::remove_file(&path);
    }
}
