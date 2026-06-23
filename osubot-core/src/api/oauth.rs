use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::{http_client, ApiError, OauthResponse};

/// Token 实际过期前多少秒主动刷新，避免边界处 401。
const REFRESH_SAFETY_MARGIN: Duration = Duration::from_secs(60);
/// expires_in 缺失或为 0 时的兜底过期时间（保守：5 分钟）。
const FALLBACK_EXPIRES_IN: Duration = Duration::from_secs(300);

pub struct OauthTokenCache {
    client_id: RwLock<String>,
    client_secret: RwLock<String>,
    cache: Mutex<Option<CachedToken>>,
    refresh_lock: Mutex<()>,
}

struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

impl OauthTokenCache {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id: RwLock::new(client_id),
            client_secret: RwLock::new(client_secret),
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    pub fn is_configured(&self) -> bool {
        let cid = self.client_id.read().unwrap_or_else(|e| {
            tracing::warn!("{}", crate::log_fmt!("api.rwlock_recovering"));
            e.into_inner()
        });
        let cs = self.client_secret.read().unwrap_or_else(|e| {
            tracing::warn!("{}", crate::log_fmt!("api.rwlock_recovering"));
            e.into_inner()
        });
        !cid.is_empty() && !cs.is_empty()
    }

    pub async fn update_credentials(&self, client_id: String, client_secret: String) {
        let _guard = self.refresh_lock.lock().await;
        *self.client_id.write().unwrap_or_else(|e| {
            tracing::warn!("{}", crate::log_fmt!("api.rwlock_recovering"));
            e.into_inner()
        }) = client_id;
        *self.client_secret.write().unwrap_or_else(|e| {
            tracing::warn!("{}", crate::log_fmt!("api.rwlock_recovering"));
            e.into_inner()
        }) = client_secret;
        let mut cache = self.cache.lock().await;
        *cache = None;
    }

    pub async fn invalidate(&self) {
        let _guard = self.refresh_lock.lock().await;
        let mut guard = self.cache.lock().await;
        *guard = None;
    }

    pub async fn get_token(&self) -> Result<String, ApiError> {
        {
            let guard = self.cache.lock().await;
            if let Some(ct) = guard.as_ref() {
                if Instant::now() < ct.expires_at {
                    return Ok(ct.access_token.clone());
                }
            }
        }

        let _refresh_guard = self.refresh_lock.lock().await;

        {
            let guard = self.cache.lock().await;
            if let Some(ct) = guard.as_ref() {
                if Instant::now() < ct.expires_at {
                    return Ok(ct.access_token.clone());
                }
            }
        }

        let client = http_client();
        let (cid, cs) = {
            let cid = self.client_id.read().unwrap_or_else(|e| {
                tracing::warn!("{}", crate::log_fmt!("api.rwlock_recovering"));
                e.into_inner()
            });
            let cs = self.client_secret.read().unwrap_or_else(|e| {
                tracing::warn!("{}", crate::log_fmt!("api.rwlock_recovering"));
                e.into_inner()
            });
            (cid.clone(), cs.clone())
        };
        let params = [
            ("client_id", cid.as_str()),
            ("client_secret", cs.as_str()),
            ("grant_type", "client_credentials"),
            ("scope", "public"),
        ];
        let resp = client
            .post("https://osu.ppy.sh/oauth/token")
            .form(&params)
            .send()
            .await?;

        if resp.status() != 200 {
            return Err(ApiError::OAuthError);
        }

        let token_data: OauthResponse = super::http::json_body(resp).await?;

        let raw_ttl = if token_data.expires_in == 0 {
            FALLBACK_EXPIRES_IN
        } else {
            Duration::from_secs(token_data.expires_in)
        };
        let effective_ttl = raw_ttl.saturating_sub(REFRESH_SAFETY_MARGIN);
        let ttl = if effective_ttl.is_zero() {
            Duration::from_secs(1)
        } else {
            effective_ttl
        };
        let expires_at = Instant::now() + ttl;

        {
            let mut guard = self.cache.lock().await;
            *guard = Some(CachedToken {
                access_token: token_data.access_token.clone(),
                expires_at,
            });
        }

        Ok(token_data.access_token)
    }
}

pub(crate) async fn retry_on_401<F, Fut, T>(
    oauth: &OauthTokenCache,
    max_retries: u32,
    operation: F,
) -> Result<T, ApiError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    debug_assert!(
        max_retries <= 30,
        "max_retries must be <= 30, got {max_retries}"
    );
    let max_retries = max_retries.min(30);
    let mut attempt = 0u32;
    loop {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(ApiError::OAuthError) if attempt < max_retries => {
                oauth.invalidate().await;
                let delay =
                    super::http::compute_backoff(attempt, &super::http::RetryConfig::api_default());
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_response_parses_expires_in() {
        let json = r#"{"access_token":"abc","expires_in":3600,"token_type":"Bearer"}"#;
        let parsed: OauthResponse = serde_json::from_str(json).expect("parses");
        assert_eq!(parsed.access_token, "abc");
        assert_eq!(parsed.expires_in, 3600);
    }

    #[test]
    fn oauth_response_missing_expires_in_defaults_to_zero() {
        let json = r#"{"access_token":"abc"}"#;
        let parsed: OauthResponse = serde_json::from_str(json).expect("parses");
        assert_eq!(parsed.access_token, "abc");
        assert_eq!(parsed.expires_in, 0);
    }
}
