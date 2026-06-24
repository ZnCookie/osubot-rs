use crate::log_fmt;
use std::path::PathBuf;
use std::time::SystemTime;

/// Shared cleanup helper that deletes files in `dir` older than `retention_days`.
/// Skips cleanup when `retention_days` is 0 or the directory does not exist.
async fn cleanup_dir(dir: PathBuf, label: &str, retention_days: u64) {
    if retention_days == 0 {
        return;
    }

    match tokio::fs::try_exists(&dir).await {
        Ok(false) | Err(_) => return,
        Ok(true) => {}
    }

    let retention_secs = retention_days.saturating_mul(86400);
    let cutoff = SystemTime::now().checked_sub(std::time::Duration::from_secs(retention_secs));

    let cutoff = match cutoff {
        Some(t) => t,
        None => {
            tracing::error!("{}", log_fmt!("cache.cutoff_time_failed", name = label));
            return;
        }
    };

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!(
                "{}",
                log_fmt!("cache.read_dir_failed", name = label, error = &e)
            );
            return;
        }
    };

    let mut deleted = 0u64;
    let mut errors = 0u64;

    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(
                    "{}",
                    log_fmt!("cache.read_entry_failed", name = label, error = &e)
                );
                continue;
            }
        };
        let path = entry.path();

        let modified = match entry.metadata().await.and_then(|m| m.modified()) {
            Ok(time) => time,
            Err(_) => continue,
        };

        if modified < cutoff {
            match tokio::fs::remove_file(&path).await {
                Ok(()) => deleted += 1,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(
                        "{}",
                        log_fmt!(
                            "cache.delete_failed",
                            name = label,
                            path = format!("{}", path.display()),
                            error = &e
                        )
                    );
                    errors += 1;
                }
            }
        }
    }

    tracing::info!(
        "{}",
        log_fmt!(
            "cache.cleanup_summary",
            name = label,
            deleted = deleted,
            errors = errors,
            days = retention_days
        )
    );
}

pub(crate) fn replay_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("osubot")
        .join("replays")
}

pub(crate) fn beatmap_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("osubot")
        .join("beatmaps")
}

pub fn preview_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("osubot")
        .join("previews")
}

pub(crate) fn beatmap_audio_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("osubot")
        .join("beatmap_audio")
}

/// Clean up expired replay cache files.
pub async fn cleanup_replays(retention_days: u64) {
    cleanup_dir(replay_cache_dir(), "replay", retention_days).await;
}

/// Clean up expired beatmap cache files (.osu).
pub async fn cleanup_beatmaps(retention_days: u64) {
    cleanup_dir(beatmap_cache_dir(), "beatmap", retention_days).await;
}

/// Clean up expired beatmap preview cache files (rendered GIF/PNG).
pub async fn cleanup_previews(retention_days: u64) {
    cleanup_dir(preview_cache_dir(), "preview", retention_days).await;
}

/// Clean up expired beatmap preview audio cache files (.mp3).
pub async fn cleanup_beatmap_audio(retention_days: u64) {
    cleanup_dir(beatmap_audio_cache_dir(), "beatmap_audio", retention_days).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cleanup_replays_noop_when_disabled() {
        cleanup_replays(0).await;
    }

    #[tokio::test]
    async fn test_cleanup_beatmaps_noop_when_disabled() {
        cleanup_beatmaps(0).await;
    }

    #[tokio::test]
    async fn test_cleanup_previews_noop_when_disabled() {
        cleanup_previews(0).await;
    }

    #[tokio::test]
    async fn test_cleanup_beatmap_audio_noop_when_disabled() {
        cleanup_beatmap_audio(0).await;
    }
}
