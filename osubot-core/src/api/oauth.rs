use std::sync::RwLock;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::{http_client, ApiError, OauthResponse};

pub struct OauthTokenCache {
    client_id: RwLock<String>,
    client_secret: RwLock<String>,
    cache: Mutex<Option<(String, Instant)>>,
    refresh_lock: Mutex<()>,
    refresh_interval: Duration,
}

impl OauthTokenCache {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id: RwLock::new(client_id),
            client_secret: RwLock::new(client_secret),
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
            refresh_interval: Duration::from_secs(20 * 3600),
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
            if let Some((ref token, fetched_at)) = *guard {
                if fetched_at.elapsed() < self.refresh_interval {
                    return Ok(token.clone());
                }
            }
        }

        let _refresh_guard = self.refresh_lock.lock().await;

        {
            let guard = self.cache.lock().await;
            if let Some((ref token, fetched_at)) = *guard {
                if fetched_at.elapsed() < self.refresh_interval {
                    return Ok(token.clone());
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

        {
            let mut guard = self.cache.lock().await;
            *guard = Some((token_data.access_token.clone(), Instant::now()));
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
                let delay = super::http::backoff_with_jitter(attempt);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}
