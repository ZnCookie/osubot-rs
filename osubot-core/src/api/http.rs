use std::path::PathBuf;
use std::time::Duration;

use crate::cache::{beatmap_audio_cache_dir, beatmap_cache_dir};
use crate::log_fmt;
use crate::rate_limiter::RateLimiter;

use super::{http_client, ApiError};

pub(crate) const API_VERSION: &str = "20260408";

const MAX_RETRY_AFTER_SECS: u64 = 300;

const BODY_LOG_PREVIEW_BYTES: usize = 512;
const BODY_LOG_PREVIEW_SUFFIX: &str = "...[truncated]";

/// Knobs for [`retry_with_backoff`]: max attempts, and the backoff schedule.
///
/// Backoff uses exponential growth capped at `max_backoff`, with a 25% jitter
/// window (75%..=125% of the computed delay) to avoid thundering herds.
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
}

impl RetryConfig {
    /// Default for osu! API calls: 4 retries, 0.5s..=30s backoff.
    pub(crate) fn api_default() -> Self {
        Self {
            max_retries: 4,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
        }
    }

    /// Default for image fetches: 4 retries, 0.5s..=15s backoff.
    pub fn image_default() -> Self {
        Self {
            max_retries: 4,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(15),
        }
    }
}

/// What to do after a retryable error from the operation.
pub enum RetryAction {
    /// Retry after a backoff delay computed from [`RetryConfig`].
    Backoff,
    /// Retry after waiting the given number of seconds (e.g. server `Retry-After`).
    Wait(u64),
    /// Stop retrying and return the error to the caller.
    Abort,
}

fn truncate_for_log(body: &str, max: usize) -> String {
    if body.len() <= max {
        body.to_string()
    } else {
        // 在 UTF-8 char 边界上截断，避免切坏 multi-byte 字符
        let mut end = max;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        let mut s = String::with_capacity(end + BODY_LOG_PREVIEW_SUFFIX.len());
        s.push_str(&body[..end]);
        s.push_str(BODY_LOG_PREVIEW_SUFFIX);
        s
    }
}

pub async fn download_beatmap_osu(beatmap_id: i64) -> Result<PathBuf, ApiError> {
    let cache_path = beatmap_cache_dir().join(format!("{}.osu", beatmap_id));

    let cache_valid = tokio::task::spawn_blocking({
        let cache_path = cache_path.clone();
        move || -> bool {
            if !cache_path.exists() {
                return false;
            }
            match std::fs::metadata(&cache_path) {
                Ok(meta) if meta.len() > 0 => {
                    if let Ok(modified) = meta.modified() {
                        modified.elapsed().unwrap_or(std::time::Duration::MAX)
                            < std::time::Duration::from_secs(7 * 86400)
                    } else {
                        false
                    }
                }
                _ => false,
            }
        }
    })
    .await
    .unwrap_or(false);

    if cache_valid {
        return Ok(cache_path);
    }

    let client = http_client();
    let url = format!("https://osu.ppy.sh/osu/{}", beatmap_id);

    let bytes = retry_on_transient(4, || async {
        let resp = client.get(&url).send().await?;

        classify_http_error(&resp)?;

        resp.bytes().await.map_err(ApiError::Http)
    })
    .await?;

    let write_path = cache_path.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = std::fs::create_dir_all(
            write_path
                .parent()
                .expect("cache path always has parent dirs (beatmap_cache_dir()/id.osu)"),
        ) {
            tracing::warn!("{}", log_fmt!("api.cache_dir_failed", error = &e));
        }
        if let Err(e) = std::fs::write(&write_path, &bytes) {
            tracing::warn!(
                "{}",
                log_fmt!(
                    "api.write_beatmap_cache_failed",
                    path = format!("{}", write_path.display()),
                    error = &e
                )
            );
        }
    })
    .await
    .ok();

    Ok(cache_path)
}

/// Download the 30s preview MP3 for a beatmapset, with on-disk caching (~7 day TTL,
/// mirroring `download_beatmap_osu`). Returns bytes for base64 record sending.
/// Preview audio content is stable per beatmapset, so reads hit the cache directly.
pub async fn download_beatmap_preview_mp3(beatmapset_id: i64) -> Result<Vec<u8>, ApiError> {
    let cache_path = beatmap_audio_cache_dir().join(format!("{}.mp3", beatmapset_id));

    let cache_valid = tokio::task::spawn_blocking({
        let cache_path = cache_path.clone();
        move || -> bool {
            if !cache_path.exists() {
                return false;
            }
            match std::fs::metadata(&cache_path) {
                Ok(meta) if meta.len() > 0 => {
                    if let Ok(modified) = meta.modified() {
                        modified.elapsed().unwrap_or(std::time::Duration::MAX)
                            < std::time::Duration::from_secs(7 * 86400)
                    } else {
                        false
                    }
                }
                _ => false,
            }
        }
    })
    .await
    .unwrap_or(false);

    if cache_valid {
        if let Ok(bytes) = tokio::fs::read(&cache_path).await {
            return Ok(bytes);
        }
    }

    let client = http_client();
    let url = format!("https://b.ppy.sh/preview/{}.mp3", beatmapset_id);

    let bytes = retry_on_transient(4, || async {
        let resp = client.get(&url).send().await?;

        classify_http_error(&resp)?;

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(ApiError::Http)
    })
    .await?;

    tokio::task::spawn_blocking({
        let write_path = cache_path.clone();
        let bytes = bytes.clone();
        move || {
            if let Err(e) = std::fs::create_dir_all(
                write_path
                    .parent()
                    .expect("cache path always has parent dirs (beatmap_audio_cache_dir()/id.mp3)"),
            ) {
                tracing::warn!("{}", log_fmt!("api.cache_dir_failed", error = &e));
            }
            if let Err(e) = std::fs::write(&write_path, &bytes) {
                tracing::warn!(
                    "{}",
                    log_fmt!(
                        "api.write_beatmap_cache_failed",
                        path = format!("{}", write_path.display()),
                        error = &e
                    )
                );
            }
        }
    })
    .await
    .ok();

    Ok(bytes)
}

pub(crate) async fn json_body<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, ApiError> {
    let status = resp.status();
    let url = resp.url().to_string();
    let body = resp.text().await.map_err(ApiError::Http)?;
    serde_json::from_str::<T>(&body).map_err(|e| {
        tracing::warn!(%status, %url, body = %truncate_for_log(&body, BODY_LOG_PREVIEW_BYTES), error = %e, "{}", log_fmt!("api.deserialize_failed"));
        ApiError::Deserialization(e.to_string())
    })
}

pub(crate) fn classify_http_error(resp: &reqwest::Response) -> Result<(), ApiError> {
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.min(MAX_RETRY_AFTER_SECS));

        return Err(ApiError::RateLimitedWithRetryAfter(retry_after));
    }
    if status.is_server_error() {
        return Err(ApiError::ServerError(status.as_u16()));
    }
    if !status.is_success() {
        return Err(ApiError::InvalidResponse);
    }
    Ok(())
}

pub(crate) fn compute_backoff(attempt: u32, config: &RetryConfig) -> Duration {
    use rand::RngExt;
    let exp = config.initial_backoff * 2u32.pow(attempt.min(5));
    let capped = exp.min(config.max_backoff);
    let ms = capped.as_millis() as u64;
    let min_ms = ms * 3 / 4;
    let range_ms = ms / 2;
    let jitter_ms = if range_ms > 0 {
        rand::rng().random_range(0..=range_ms)
    } else {
        0
    };
    Duration::from_millis(min_ms + jitter_ms)
}

pub async fn retry_with_backoff<T, E, F, Fut>(
    config: &RetryConfig,
    classify: impl Fn(&E) -> RetryAction,
    operation: F,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    debug_assert!(
        config.max_retries <= 30,
        "max_retries must be <= 30, got {}",
        config.max_retries
    );
    let max_retries = config.max_retries.min(30);
    let mut attempt = 0u32;
    loop {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(e) => match classify(&e) {
                RetryAction::Wait(secs) if attempt < max_retries => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries,
                        delay_secs = secs,
                        "{}",
                        log_fmt!("api.retry_after")
                    );
                    tokio::time::sleep(Duration::from_secs(secs)).await;
                    attempt += 1;
                }
                RetryAction::Backoff if attempt < max_retries => {
                    let delay = compute_backoff(attempt, config);
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries,
                        delay_ms = delay.as_millis(),
                        "{}",
                        log_fmt!("api.retry_transient")
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                _ => return Err(e),
            },
        }
    }
}

pub(crate) async fn retry_on_transient<F, Fut, T>(
    max_retries: u32,
    operation: F,
) -> Result<T, ApiError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    let config = RetryConfig {
        max_retries,
        initial_backoff: Duration::from_millis(500),
        max_backoff: Duration::from_secs(30),
    };
    retry_with_backoff(
        &config,
        |e| match e {
            ApiError::RateLimitedWithRetryAfter(Some(secs)) => RetryAction::Wait(*secs),
            ApiError::RateLimitedWithRetryAfter(None) => RetryAction::Wait(60),
            e if e.is_transient() => RetryAction::Backoff,
            _ => RetryAction::Abort,
        },
        operation,
    )
    .await
}

pub(crate) async fn authenticated_get(
    url: &str,
    rate_limiter: &RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
) -> Result<reqwest::Response, ApiError> {
    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(4, || {
        super::oauth::retry_on_401(oauth, 5, || async {
            let token = oauth.get_token().await?;
            let resp = http_client()
                .get(url)
                .header("Authorization", format!("Bearer {}", token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;
            if resp.status() == 404 {
                return Err(ApiError::NotFound);
            }
            classify_http_error(&resp)?;
            Ok(resp)
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_backoff_attempt_zero_within_first_window() {
        let config = RetryConfig::api_default();
        let delay = compute_backoff(0, &config);
        // initial_backoff=500ms, 2^0=1, exp=500ms, min=375ms, max=625ms
        assert!(
            delay >= Duration::from_millis(375) && delay <= Duration::from_millis(625),
            "got {delay:?}"
        );
    }

    #[test]
    fn compute_backoff_attempt_one_doubles() {
        let config = RetryConfig::api_default();
        let delay = compute_backoff(1, &config);
        // exp=1000ms, min=750ms, max=1250ms
        assert!(
            delay >= Duration::from_millis(750) && delay <= Duration::from_millis(1250),
            "got {delay:?}"
        );
    }

    #[test]
    fn compute_backoff_attempt_five_saturates() {
        let config = RetryConfig::api_default();
        let delay = compute_backoff(5, &config);
        // exp=500ms*2^5=16000ms, capped at max_backoff=30s → 16s
        // min=12000ms, range=8000ms, total=12000..=20000ms (12..=20s)
        assert!(
            delay >= Duration::from_secs(12) && delay <= Duration::from_secs(20),
            "got {delay:?}"
        );
    }

    #[test]
    fn compute_backoff_attempt_large_saturates() {
        let config = RetryConfig::api_default();
        let delay_5 = compute_backoff(5, &config);
        let delay_100 = compute_backoff(100, &config);
        let diff = if delay_5 > delay_100 {
            delay_5 - delay_100
        } else {
            delay_100 - delay_5
        };
        assert!(
            diff < delay_5,
            "attempt=5 and attempt=100 should share same magnitude, got {delay_5:?} vs {delay_100:?}"
        );
    }

    #[test]
    fn truncate_for_log_short_body_unchanged() {
        assert_eq!(truncate_for_log("hello", 10), "hello");
    }

    #[test]
    fn truncate_for_log_long_body_truncated() {
        let s = "a".repeat(600) + "ß";
        let out = truncate_for_log(&s, 10);
        assert!(out.ends_with(BODY_LOG_PREVIEW_SUFFIX));
        // 10 个 'a'（全 ASCII）+ 后缀
        assert!(out.starts_with("aaaaaaaaaa"));
    }

    #[test]
    fn truncate_for_log_respects_utf8_boundary() {
        // "ß" 占 2 字节；若按字节硬切在 11，会切坏
        let s = "aaaaaaaaaaß".to_string(); // 10 个 'a' (10B) + "ß" (2B) = 12B
        let out = truncate_for_log(&s, 11);
        // 应回退到 10 字节边界
        assert!(out.starts_with("aaaaaaaaaa"));
        assert!(out.ends_with(BODY_LOG_PREVIEW_SUFFIX));
    }

    #[tokio::test]
    async fn retry_with_backoff_succeeds_on_first_try() {
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
        };
        let result: Result<i32, &str> = retry_with_backoff(
            &config,
            |e| match *e {
                "transient" => RetryAction::Backoff,
                _ => RetryAction::Abort,
            },
            || async { Ok(42) },
        )
        .await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn retry_with_backoff_retries_then_succeeds() {
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
        };
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let result: Result<i32, &str> = retry_with_backoff(
            &config,
            |e| match *e {
                "transient" => RetryAction::Backoff,
                _ => RetryAction::Abort,
            },
            || {
                let a = &attempt;
                async move {
                    if a.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                        Err("transient")
                    } else {
                        Ok(99)
                    }
                }
            },
        )
        .await;
        assert_eq!(result.unwrap(), 99);
    }

    #[tokio::test]
    async fn retry_with_backoff_exhausts_retries() {
        let config = RetryConfig {
            max_retries: 2,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
        };
        let result: Result<i32, &str> = retry_with_backoff(
            &config,
            |e| match *e {
                "transient" => RetryAction::Backoff,
                _ => RetryAction::Abort,
            },
            || async { Err("transient") },
        )
        .await;
        assert_eq!(result.unwrap_err(), "transient");
    }

    #[tokio::test]
    async fn retry_with_backoff_wait_action_sleeps_then_retries() {
        let config = RetryConfig {
            max_retries: 2,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
        };
        let attempt = std::sync::atomic::AtomicU32::new(0);
        let start = std::time::Instant::now();
        let result: Result<i32, &str> = retry_with_backoff(
            &config,
            |e| match *e {
                "wait" => RetryAction::Wait(1),
                _ => RetryAction::Abort,
            },
            || {
                let a = &attempt;
                async move {
                    if a.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        Err("wait")
                    } else {
                        Ok(77)
                    }
                }
            },
        )
        .await;
        assert_eq!(result.unwrap(), 77);
        // Wait(1) should have slept ~1 second
        assert!(
            start.elapsed() >= Duration::from_millis(900),
            "Wait action should sleep ~1s"
        );
    }

    #[tokio::test]
    async fn retry_with_backoff_aborts_on_non_transient() {
        let config = RetryConfig {
            max_retries: 3,
            initial_backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(100),
        };
        let result: Result<i32, &str> = retry_with_backoff(
            &config,
            |e| match *e {
                "transient" => RetryAction::Backoff,
                _ => RetryAction::Abort,
            },
            || async { Err("fatal") },
        )
        .await;
        assert_eq!(result.unwrap_err(), "fatal");
    }
}
