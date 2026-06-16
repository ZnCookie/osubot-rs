use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use rand::RngExt;
use serde_json::json;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMsg;
use tracing::{debug, warn};

use osubot_core::log_fmt;
use osubot_core::rate_limiter::RateLimiter;
use osubot_core::upstream::{extract_text_from_message, SendAction};
use osubot_core::UpstreamBindingProvider;

use crate::config::ProviderConfig;

const YUMU_DEFAULT_URL: &str = "ws://121.41.63.60:11735/pub/onebotSocket";

fn parse_bind_response_text(resp_text: &str) -> Option<(i64, String)> {
    if let Some(pos) = resp_text.find("您已绑定") {
        let after = &resp_text[pos..];
        if let Some(paren_open) = after.find('(') {
            let after_open = &after[paren_open + 1..];
            if let Some(paren_close) = after_open.find(')') {
                let osu_id: i64 = after_open[..paren_close].trim().parse().ok()?;
                let after_close = &after_open[paren_close + 1..];
                let username = after_close
                    .split([',', '，'])
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if !username.is_empty() {
                    return Some((osu_id, username));
                }
            }
        }
    }
    None
}

fn parse_send_msg_action(action_text: &str) -> Option<String> {
    let action: SendAction = serde_json::from_str(action_text).ok()?;
    if action.action != "send_group_msg" && action.action != "send_msg" {
        return None;
    }
    let text = extract_text_from_message(&action.params["message"]);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

pub struct YumuUpstream {
    url: String,
    timeout: Duration,
    rate_limiter: RateLimiter,
}

impl YumuUpstream {
    pub fn from_config(cfg: &ProviderConfig) -> Self {
        let url = cfg.url.clone().unwrap_or_else(|| YUMU_DEFAULT_URL.into());

        Self {
            url,
            timeout: Duration::from_secs(cfg.timeout_secs),
            rate_limiter: RateLimiter::with_config(cfg.burst, cfg.rate_per_minute),
        }
    }
}

#[async_trait]
impl UpstreamBindingProvider for YumuUpstream {
    async fn query_binding(&self, qq: i64) -> Result<Option<(i64, String)>, String> {
        let _permit = self
            .rate_limiter
            .acquire()
            .await
            .map_err(|_| "rate limited")?;

        let group_id: i64 = {
            let mut rng = rand::rng();
            rng.random_range(1000000000..9999999999i64)
        };
        let message_id: i32 = {
            let mut rng = rand::rng();
            rng.random_range(1..i32::MAX)
        };

        let mut request = match self.url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                warn!("{}", log_fmt!("yumu.build_request_failed", error = &e));
                return Ok(None);
            }
        };

        if let Ok(val) = "Universal".parse() {
            request.headers_mut().insert("X-Client-Role", val);
        } else {
            warn!("{}", log_fmt!("yumu.invalid_client_role"));
            return Ok(None);
        }
        if let Ok(val) = qq.to_string().parse() {
            request.headers_mut().insert("X-Self-ID", val);
        } else {
            warn!("{}", log_fmt!("yumu.invalid_self_id"));
            return Ok(None);
        }

        let ws_stream = match timeout(self.timeout, connect_async(request)).await {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(e)) => {
                warn!("{}", log_fmt!("yumu.connect_failed", error = &e));
                return Ok(None);
            }
            Err(_) => {
                warn!("{}", log_fmt!("yumu.connect_timeout"));
                return Ok(None);
            }
        };

        let (mut write, mut read) = ws_stream.split();

        // Step 1: Send lifecycle event
        let lifecycle = json!({
            "time": Utc::now().timestamp(),
            "self_id": qq,
            "post_type": "meta_event",
            "meta_event_type": "lifecycle",
            "sub_type": "connect",
            "status": {"online": true, "good": true},
        });

        let lifecycle_str = lifecycle.to_string();
        if timeout(self.timeout, write.send(lifecycle_str.into()))
            .await
            .is_err()
        {
            warn!("{}", log_fmt!("yumu.send_lifecycle_failed"));
            return Ok(None);
        }

        // Step 2: Send GROUP message event with !bi command
        // Brief pause for server to process lifecycle
        tokio::time::sleep(Duration::from_millis(100)).await;
        let event = json!({
            "time": Utc::now().timestamp(),
            "self_id": qq,
            "post_type": "message",
            "message_type": "group",
            "sub_type": "normal",
            "message_id": message_id,
            "message_seq": 1,
            "group_id": group_id,
            "user_id": qq,
            "message": [
                {"type": "text", "data": {"text": "!bi"}}
            ],
            "raw_message": "!bi",
            "font": 0,
            "sender": {
                "user_id": qq,
                "nickname": "test",
                "card": "",
                "role": "member",
                "sex": "unknown",
                "age": 0,
            },
            "anonymous": null,
        });

        let event_str = event.to_string();
        if timeout(self.timeout, write.send(event_str.into()))
            .await
            .is_err()
        {
            warn!("{}", log_fmt!("yumu.send_event_failed"));
            return Ok(None);
        }

        // Step 3: Read responses (send_group_msg actions from bot)
        let deadline = Instant::now() + self.timeout;
        while let Ok(Some(_msg)) = timeout(
            deadline.saturating_duration_since(Instant::now()),
            read.next(),
        )
        .await
        {
            let msg = match _msg {
                Ok(m) => m,
                Err(_) => {
                    warn!("{}", log_fmt!("yumu.ws_read_error"));
                    return Ok(None);
                }
            };

            let text = match msg {
                WsMsg::Text(s) => s.to_string(),
                WsMsg::Binary(bin) => match String::from_utf8(bin.to_vec()) {
                    Ok(s) => s,
                    Err(_) => continue,
                },
                _ => continue,
            };

            debug!(target: "yumu_upstream", %text, "{}", log_fmt!("yumu.received_message"));

            if let Some(resp_text) = parse_send_msg_action(&text) {
                debug!(target: "yumu_upstream", resp = %resp_text, "{}", log_fmt!("yumu.parsed_send_action"));
                if let Some(binding) = parse_bind_response_text(&resp_text) {
                    debug!(target: "yumu_upstream", username = %binding.1, osu_id = binding.0, "{}", log_fmt!("yumu.parsed_binding"));
                    return Ok(Some(binding));
                }
                // Got a response but it's not a binding info (e.g., "已撤回绑定授权" etc.)
                // Continue reading in case there are more responses
                continue;
            }
        }

        warn!("{}", log_fmt!("yumu.response_timeout"));
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_bind_response() {
        let text = "您已绑定 (18230719) ZnCookie，但是令牌依旧有效。\n如果要改绑，请回复 OK。";
        let binding = parse_bind_response_text(text).unwrap();
        assert_eq!(binding.0, 18230719);
        assert_eq!(binding.1, "ZnCookie");
    }

    #[test]
    fn test_parse_bind_response_no_bind() {
        let text = "请在获取六位数的验证码后，回来发送 !bi 验证码 完成绑定。";
        assert!(parse_bind_response_text(text).is_none());
    }

    #[test]
    fn test_parse_bind_response_revoked() {
        let text = "您已撤回绑定授权";
        assert!(parse_bind_response_text(text).is_none());
    }

    #[test]
    fn test_parse_send_msg_action() {
        let action = r#"{"action":"send_group_msg","echo":1272,"params":{"group_id":9876543210,"message":[{"data":{"text":"您已绑定 (18230719) ZnCookie，但是令牌依旧有效。\n如果要改绑，请回复 OK。"},"type":"text"}],"auto_escape":false}}"#;
        let text = parse_send_msg_action(action).unwrap();
        assert_eq!(
            text,
            "您已绑定 (18230719) ZnCookie，但是令牌依旧有效。\n如果要改绑，请回复 OK。"
        );
    }

    #[test]
    fn test_parse_send_msg_action_non_match() {
        let action = r#"{"action":"get_version_info","echo":"1","params":{}}"#;
        assert!(parse_send_msg_action(action).is_none());
    }

    #[test]
    fn test_extract_text_from_message_array() {
        let msg = serde_json::json!([
            {"type": "text", "data": {"text": "您已绑定 (18230719) "}},
            {"type": "text", "data": {"text": "ZnCookie，但是令牌依旧有效。"}}
        ]);
        let text = extract_text_from_message(&msg);
        assert_eq!(text, "您已绑定 (18230719) ZnCookie，但是令牌依旧有效。");
    }
}

#[cfg(all(test, feature = "integration-tests"))]
mod integration_tests {
    use super::*;
    use crate::config::ProviderConfig;

    #[tokio::test]
    async fn test_yumu_query_bound_user() {
        let _ = tracing_subscriber::fmt()
            .without_time()
            .with_writer(std::io::stdout)
            .try_init();

        let cfg = ProviderConfig {
            provider_type: "yumu".into(),
            rate_per_minute: 10,
            burst: 20,
            url: Some(YUMU_DEFAULT_URL.into()),
            access_token: String::new(),
            self_id: None,
            timeout_secs: 10,
        };

        let provider = YumuUpstream::from_config(&cfg);

        let result = provider.query_binding(3628905173).await;

        assert!(
            result.is_ok(),
            "yumu query should not error, got {:?}",
            result
        );
        let binding = result.unwrap();
        if binding.is_none() {
            tracing::info!("{}", log_fmt!("yumu.no_binding_test"));
        } else {
            let (osu_id, username) = binding.unwrap();
            tracing::info!(
                "{}",
                log_fmt!("yumu.binding_found", osu_id = osu_id, username = username)
            );
        }
    }
}
