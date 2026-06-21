use std::path::PathBuf;
use std::time::Duration;

use crate::cache::beatmap_cache_dir;
use crate::log_fmt;
use crate::rate_limiter::RateLimiter;

use super::{http_client, ApiError};

pub(crate) const API_VERSION: &str = "20260408";

const MAX_RETRY_AFTER_SECS: u64 = 300;

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

    let bytes = retry_on_transient(2, || async {
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

pub(crate) async fn json_body<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, ApiError> {
    let status = resp.status();
    let url = resp.url().to_string();
    let body = resp.text().await.map_err(ApiError::Http)?;
    serde_json::from_str::<T>(&body).map_err(|e| {
        tracing::warn!(%status, %url, body = %body, error = %e, "{}", log_fmt!("api.deserialize_failed"));
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

pub(crate) fn backoff_with_jitter(attempt: u32) -> Duration {
    use rand::RngExt;
    let base_delay = Duration::from_secs(1);
    let exp = base_delay * 2u32.pow(attempt.min(31));
    let exp_ms = exp.as_millis() as u64;
    let min_ms = exp_ms * 3 / 4;
    let range_ms = exp_ms / 2;
    let jitter_ms = if range_ms > 0 {
        rand::rng().random_range(0..=range_ms)
    } else {
        0
    };
    Duration::from_millis(min_ms + jitter_ms)
}

pub(crate) async fn retry_on_transient<F, Fut, T>(
    max_retries: u32,
    operation: F,
) -> Result<T, ApiError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    assert!(
        max_retries <= 30,
        "max_retries must be <= 30, got {max_retries}"
    );
    let mut attempt = 0u32;
    loop {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(ApiError::RateLimitedWithRetryAfter(Some(retry_after)))
                if attempt < max_retries =>
            {
                let delay = Duration::from_secs(retry_after);
                tracing::warn!(
                    attempt = attempt + 1,
                    max_retries,
                    delay_secs = retry_after,
                    "{}",
                    log_fmt!("api.retry_after")
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) if e.is_transient() && attempt < max_retries => {
                let delay = backoff_with_jitter(attempt);
                tracing::warn!(error = %e, attempt = attempt + 1, max_retries, delay_ms = delay.as_millis(), "{}", log_fmt!("api.retry_transient"));
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
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
    retry_on_transient(2, || {
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
    fn backoff_attempt_zero_within_first_window() {
        let delay = backoff_with_jitter(0);
        assert!(
            delay >= Duration::from_millis(750) && delay <= Duration::from_millis(1250),
            "got {delay:?}"
        );
    }

    #[test]
    fn backoff_attempt_one_doubles() {
        let delay = backoff_with_jitter(1);
        assert!(
            delay >= Duration::from_millis(1500) && delay <= Duration::from_millis(2500),
            "got {delay:?}"
        );
    }

    #[test]
    fn backoff_attempt_thirtyone_saturates() {
        let delay = backoff_with_jitter(31);
        assert!(
            delay >= Duration::from_secs(3600),
            "got {delay:?}, expected >= 1 hour"
        );
    }

    #[test]
    fn backoff_attempt_large_caps_at_thirtyone() {
        let delay_31 = backoff_with_jitter(31);
        let delay_100 = backoff_with_jitter(100);
        let diff = if delay_31 > delay_100 {
            delay_31 - delay_100
        } else {
            delay_100 - delay_31
        };
        assert!(
            diff < delay_31,
            "attempt=31 and attempt=100 should share same magnitude, got {delay_31:?} vs {delay_100:?}"
        );
    }
}
