use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::Deserialize;
use serde_json::json;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMsg;
use tracing::{debug, warn};

use osubot_core::api;
use osubot_core::rate_limiter::RateLimiter;
use osubot_core::types::GameMode;
use osubot_core::OauthTokenCache;
use osubot_core::UpstreamBindingProvider;

use crate::config::ProviderConfig;

pub struct XfsUpstream {
    url: String,
    access_token: String,
    self_id: i64,
    timeout: Duration,
    rate_limiter: RateLimiter,
    oauth: Arc<OauthTokenCache>,
    api_rate_limiter: Arc<RateLimiter>,
}

impl XfsUpstream {
    pub fn from_config(
        cfg: &ProviderConfig,
        oauth: Arc<OauthTokenCache>,
        api_rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        let self_id = cfg.self_id.unwrap_or_else(|| {
            let mut rng = rand::thread_rng();
            rng.gen_range(100000000..999999999i64)
        });

        Self {
            url: cfg.url.clone(),
            access_token: cfg.access_token.clone(),
            self_id,
            timeout: Duration::from_secs(cfg.timeout_secs),
            rate_limiter: RateLimiter::with_config(cfg.burst, cfg.rate_per_minute),
            oauth,
            api_rate_limiter,
        }
    }
}

#[async_trait]
impl UpstreamBindingProvider for XfsUpstream {
    async fn query_binding(&self, qq: i64) -> Result<Option<(i64, String)>, String> {
        let _permit = self
            .rate_limiter
            .acquire()
            .await
            .map_err(|_| "rate limited")?;

        let group_id: i64 = {
            let mut rng = rand::thread_rng();
            rng.gen_range(1000000000..9999999999i64)
        };
        let message_id: i32 = {
            let mut rng = rand::thread_rng();
            rng.gen_range(1..i32::MAX)
        };

        let mut request = match self.url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                warn!("xfs: failed to build WS request: {e}");
                return Ok(None);
            }
        };
        request
            .headers_mut()
            .insert("X-Self-ID", self.self_id.to_string().parse().unwrap());
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {}", self.access_token).parse().unwrap(),
        );

        let ws_stream = match timeout(self.timeout, connect_async(request)).await {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(e)) => {
                warn!("xfs: WS connect failed: {e}");
                return Ok(None);
            }
            Err(_) => {
                warn!("xfs: WS connect timed out");
                return Ok(None);
            }
        };

        let (mut write, mut read) = ws_stream.split();

        let event = json!({
            "post_type": "message",
            "message_type": "group",
            "sub_type": "normal",
            "group_id": group_id,
            "user_id": qq,
            "message": format!("where qq={qq}"),
            "self_id": self.self_id,
            "time": Utc::now().timestamp(),
            "message_id": message_id,
        });

        debug!(target: "xfs_upstream", %event, "计划请求");

        let event_str = event.to_string();
        debug!(target: "xfs_upstream", text = %event_str, "实际发送");
        let deadline = Instant::now() + self.timeout;

        let send_timeout = deadline.saturating_duration_since(Instant::now());
        if timeout(send_timeout, write.send(WsMsg::Text(event_str.into())))
            .await
            .is_err()
        {
            warn!("xfs: failed to send event");
            return Ok(None);
        }

        while let Ok(Some(msg)) = timeout(
            deadline.saturating_duration_since(Instant::now()),
            read.next(),
        )
        .await
        {
            let msg = match msg {
                Ok(m) => m,
                Err(_) => {
                    warn!("xfs: WS read error");
                    return Ok(None);
                }
            };

            let text = match msg.to_text() {
                Ok(t) => t,
                Err(_) => continue,
            };

            debug!(target: "xfs_upstream", %text, "服务器返回");

            #[derive(Deserialize)]
            struct SendGroupMsgAction {
                action: String,
                params: SendGroupMsgParams,
            }

            #[derive(Deserialize)]
            struct SendGroupMsgParams {
                #[allow(dead_code)]
                group_id: i64,
                message: String,
            }

            let action: SendGroupMsgAction = match serde_json::from_str(text) {
                Ok(a) => a,
                Err(_) => continue,
            };

            if action.action != "send_group_msg" {
                continue;
            }

            let resp_text = action.params.message;

            if resp_text.contains("未绑定 osu! 账号") {
                return Ok(None);
            }

            let first_line = resp_text.lines().next().unwrap_or("");
            if let Some(pos) = first_line.find("的个人信息") {
                let username = first_line[..pos].trim();
                if username.is_empty() {
                    return Ok(None);
                }

                debug!(username, "xfs: resolved username from upstream");
                let user_id = match api::fetch_user_stats_by_username(
                    &self.api_rate_limiter,
                    &self.oauth,
                    username,
                    GameMode::Osu,
                )
                .await
                {
                    Ok(stats) => stats.user_id,
                    Err(_) => {
                        warn!(username, "xfs: failed to resolve username to user_id");
                        return Ok(None);
                    }
                };

                debug!(target: "xfs_upstream", %username, user_id, "解析结果");
                return Ok(Some((user_id, username.to_string())));
            }

            return Ok(None);
        }

        warn!("xfs: timed out waiting for response");
        Ok(None)
    }
}

#[cfg(all(test, feature = "integration-tests"))]
mod integration_tests {
    use super::*;
    use crate::config::ProviderConfig;
    use osubot_core::OauthTokenCache;
    use osubot_core::RateLimiter;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_xfs_query_known_user() {
        let _ = tracing_subscriber::fmt()
            .without_time()
            .with_writer(std::io::stdout)
            .try_init();

        let cfg = ProviderConfig {
            provider_type: "xfs".into(),
            rate_per_minute: 10,
            burst: 20,
            url: "wss://public-service.b11p.com/".into(),
            access_token: "bleatingsheep.org".into(),
            self_id: None,
            timeout_secs: 10,
        };

        let oauth = Arc::new(OauthTokenCache::new(String::new(), String::new()));
        let rate_limiter = Arc::new(RateLimiter::new());
        let provider = XfsUpstream::from_config(&cfg, oauth, rate_limiter);

        let result = provider.query_binding(3628905173).await;

        assert!(
            result.is_ok(),
            "xfs query should not error, got {:?}",
            result
        );
    }
}
