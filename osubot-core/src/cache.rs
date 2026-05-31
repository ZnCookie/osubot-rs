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

    let cutoff =
        SystemTime::now().checked_sub(std::time::Duration::from_secs(retention_days * 86400));

    let cutoff = match cutoff {
        Some(t) => t,
        None => {
            tracing::error!("failed to compute cutoff time for {} cleanup", label);
            return;
        }
    };

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::error!("failed to read {} dir for cleanup: {}", label, e);
            return;
        }
    };

    let mut deleted = 0u64;
    let mut errors = 0u64;

    while let Ok(Some(entry)) = entries.next_entry().await {
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
                    tracing::warn!("failed to delete {} file {}: {e}", label, path.display());
                    errors += 1;
                }
            }
        }
    }

    tracing::info!(
        "{} cleanup: {} deleted, {} errors (retention: {} days)",
        label,
        deleted,
        errors,
        retention_days
    );
}

fn replay_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("osubot")
        .join("replays")
}

fn beatmap_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("osubot")
        .join("beatmaps")
}

/// Clean up expired replay cache files.
pub async fn cleanup_replays(retention_days: u64) {
    cleanup_dir(replay_cache_dir(), "replay", retention_days).await;
}

/// Clean up expired beatmap cache files (.osu).
pub async fn cleanup_beatmaps(retention_days: u64) {
    cleanup_dir(beatmap_cache_dir(), "beatmap", retention_days).await;
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
}
