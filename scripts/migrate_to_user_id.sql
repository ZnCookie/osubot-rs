-- ============================================================
-- WARNING: This migration is DESTRUCTIVE
-- Backup your database before running:
--   cp osubot.db osubot.db.pre_migration
-- ============================================================

-- Migration: username -> user_id for all internal tables
-- This script migrates the following tables from username-based
-- storage to user_id-based storage:
--   - user_bindings
--   - user_stats_history
--   - user_play_records
--   - user_next_update
--   - user_last_update
--
-- The osu_user_ids table (username -> user_id mapping) is NOT
-- changed and serves as the source of truth for the migration.

-- Step 1: Backup reminder
-- Before running this script, create a backup:
--   sqlite3 osubot.db ".backup osubot.db.pre_migration"

-- Step 2: Add temporary user_id_temp columns to each affected table
ALTER TABLE user_bindings ADD COLUMN user_id_temp INTEGER;
ALTER TABLE user_stats_history ADD COLUMN user_id_temp INTEGER;
ALTER TABLE user_play_records ADD COLUMN user_id_temp INTEGER;
ALTER TABLE user_next_update ADD COLUMN user_id_temp INTEGER;
ALTER TABLE user_last_update ADD COLUMN user_id_temp INTEGER;

-- Step 3: Backfill user_bindings (join on osu_username = username from osu_user_ids)
UPDATE user_bindings SET user_id_temp = (
    SELECT user_id FROM osu_user_ids
    WHERE LOWER(osu_user_ids.username) = LOWER(user_bindings.osu_username)
);

-- Step 4: Backfill user_stats_history
UPDATE user_stats_history SET user_id_temp = (
    SELECT u.user_id FROM osu_user_ids u
    WHERE LOWER(u.username) = LOWER(user_stats_history.username)
);

-- Step 5: Backfill user_play_records
UPDATE user_play_records SET user_id_temp = (
    SELECT u.user_id FROM osu_user_ids u
    WHERE LOWER(u.username) = LOWER(user_play_records.username)
);

-- Step 6: Backfill user_next_update
UPDATE user_next_update SET user_id_temp = (
    SELECT u.user_id FROM osu_user_ids u
    WHERE LOWER(u.username) = LOWER(user_next_update.username)
);

-- Step 7: Backfill user_last_update
UPDATE user_last_update SET user_id_temp = (
    SELECT u.user_id FROM osu_user_ids u
    WHERE LOWER(u.username) = LOWER(user_last_update.username)
);

-- Step 8: Verify no NULLs remain after backfill
-- Each of these queries should return 0 rows
-- SELECT COUNT(*) FROM user_bindings WHERE user_id_temp IS NULL;
-- SELECT COUNT(*) FROM user_stats_history WHERE user_id_temp IS NULL;
-- SELECT COUNT(*) FROM user_play_records WHERE user_id_temp IS NULL;
-- SELECT COUNT(*) FROM user_next_update WHERE user_id_temp IS NULL;
-- SELECT COUNT(*) FROM user_last_update WHERE user_id_temp IS NULL;

-- Step 9: Recreate user_bindings table with new schema
CREATE TABLE user_bindings_new (
    qq INTEGER PRIMARY KEY,
    user_id INTEGER NOT NULL,
    current_username TEXT NOT NULL,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);
INSERT INTO user_bindings_new SELECT qq, user_id_temp, osu_username, created_at FROM user_bindings;
DROP TABLE user_bindings;
ALTER TABLE user_bindings_new RENAME TO user_bindings;

-- Step 10: Recreate user_stats_history table with new schema
CREATE TABLE user_stats_history_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    mode INTEGER NOT NULL,
    recorded_at TEXT DEFAULT CURRENT_TIMESTAMP,
    pp REAL, rank INTEGER, country_rank INTEGER,
    ranked_score INTEGER, accuracy REAL,
    playcount INTEGER, hits INTEGER, playtime INTEGER,
    UNIQUE(user_id, mode, recorded_at)
);
INSERT INTO user_stats_history_new SELECT id, user_id_temp, mode, recorded_at, pp, rank, country_rank, ranked_score, accuracy, playcount, hits, playtime FROM user_stats_history;
DROP TABLE user_stats_history;
ALTER TABLE user_stats_history_new RENAME TO user_stats_history;

-- Step 11: Recreate user_play_records table with new schema
CREATE TABLE user_play_records_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id INTEGER NOT NULL,
    mode INTEGER NOT NULL,
    played_at INTEGER NOT NULL,
    UNIQUE(user_id, mode, played_at)
);
INSERT INTO user_play_records_new SELECT id, user_id_temp, mode, played_at FROM user_play_records;
DROP TABLE user_play_records;
ALTER TABLE user_play_records_new RENAME TO user_play_records;

-- Step 12: Recreate user_next_update table with new schema
CREATE TABLE user_next_update_new (
    user_id INTEGER NOT NULL,
    mode INTEGER NOT NULL,
    next_update INTEGER NOT NULL,
    PRIMARY KEY(user_id, mode)
);
INSERT INTO user_next_update_new SELECT user_id_temp, mode, next_update FROM user_next_update;
DROP TABLE user_next_update;
ALTER TABLE user_next_update_new RENAME TO user_next_update;

-- Step 13: Recreate user_last_update table with new schema
CREATE TABLE user_last_update_new (
    user_id INTEGER NOT NULL,
    mode INTEGER NOT NULL,
    last_update TEXT NOT NULL,
    PRIMARY KEY(user_id, mode)
);
INSERT INTO user_last_update_new SELECT user_id_temp, mode, last_update FROM user_last_update;
DROP TABLE user_last_update;
ALTER TABLE user_last_update_new RENAME TO user_last_update;

-- Step 14: Recreate indexes
CREATE INDEX idx_history_user ON user_stats_history(user_id, mode);
CREATE INDEX idx_history_recorded ON user_stats_history(recorded_at);
CREATE INDEX idx_play_records_user ON user_play_records(user_id, mode);