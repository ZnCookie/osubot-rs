use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use futures_util::SinkExt;
use serde::Deserialize;
use tokio::sync::{oneshot, Mutex};
use tokio_tungstenite::tungstenite::Message as WsMsg;

use osubot_core::strings::user_str;

/// Type alias for the WebSocket write half used per-connection.
pub(crate) type WriteSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMsg,
>;

#[derive(Debug, Clone)]
pub(crate) struct QQMessage {
    pub group_id: Option<i64>,
    pub user_id: i64,
    pub message: String,
    pub mentioned_user_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct OneBotResponse {
    pub status: Option<String>,
    pub data: Option<serde_json::Value>,
    pub echo: Option<String>,
    pub message: Option<String>,
    pub wording: Option<String>,
    pub retcode: Option<i64>,
}

pub(crate) struct PendingEntry {
    pub(crate) sender: oneshot::Sender<Result<serde_json::Value, String>>,
    pub(crate) created_at: std::time::Instant,
}

pub(crate) struct OneBotApi {
    pub(crate) pending: Mutex<HashMap<String, PendingEntry>>,
    pub(crate) timeout: Arc<AtomicU64>,
}

/// cleanup 任务保留 entry 的最小时长,避免在 `call_onebot_api` 还在等待响应时
/// sender 被 drop 触发 `request_cancelled`。
const MIN_CLEANUP_RETENTION_SECS: u64 = 30;
/// 超时配置之上的安全余量。
const CLEANUP_RETENTION_MARGIN_SECS: u64 = 5;

impl OneBotApi {
    pub(crate) fn new(timeout_secs: Arc<AtomicU64>) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            timeout: timeout_secs,
        }
    }

    pub(crate) fn cleanup_retention_secs(&self) -> u64 {
        let configured = self.timeout.load(Ordering::Relaxed);
        (configured + CLEANUP_RETENTION_MARGIN_SECS).max(MIN_CLEANUP_RETENTION_SECS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    fn api_with_timeout(secs: u64) -> OneBotApi {
        OneBotApi::new(Arc::new(AtomicU64::new(secs)))
    }

    #[test]
    fn cleanup_retention_floors_at_thirty_seconds() {
        let api = api_with_timeout(5);
        assert_eq!(api.cleanup_retention_secs(), 30);
    }

    #[test]
    fn cleanup_retention_exceeds_configured_timeout() {
        let api = api_with_timeout(120);
        assert_eq!(api.cleanup_retention_secs(), 125);
    }

    #[test]
    fn cleanup_retention_handles_large_timeout() {
        let api = api_with_timeout(300);
        assert_eq!(api.cleanup_retention_secs(), 305);
    }

    #[test]
    fn response_error_prefers_wording_then_message_then_retcode() {
        let resp = OneBotResponse {
            status: Some("failed".to_string()),
            data: None,
            echo: None,
            message: Some("message".to_string()),
            wording: Some("wording".to_string()),
            retcode: Some(100),
        };
        assert_eq!(onebot_response_error_message(&resp), "wording");

        let resp = OneBotResponse {
            status: Some("failed".to_string()),
            data: None,
            echo: None,
            message: Some("message".to_string()),
            wording: None,
            retcode: Some(100),
        };
        assert_eq!(onebot_response_error_message(&resp), "message");

        let resp = OneBotResponse {
            status: Some("failed".to_string()),
            data: None,
            echo: None,
            message: None,
            wording: None,
            retcode: Some(100),
        };
        assert_eq!(onebot_response_error_message(&resp), "retcode=100");
    }

    #[tokio::test]
    async fn call_onebot_api_cleans_pending_on_send_failure() {
        let api = api_with_timeout(5);

        let err = call_onebot_api_impl(&api, "send_group_msg", serde_json::json!({}), |_| async {
            Err("send failed".to_string())
        })
        .await
        .expect_err("send should fail");

        assert_eq!(err, "send failed");
        assert!(api.pending.lock().await.is_empty());
    }
}

static NEXT_ECHO: AtomicU64 = AtomicU64::new(0);

fn next_echo() -> String {
    NEXT_ECHO.fetch_add(1, Ordering::Relaxed).to_string()
}

pub(crate) fn onebot_response_error_message(resp: &OneBotResponse) -> String {
    resp.wording
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .or_else(|| {
            resp.message
                .as_ref()
                .filter(|s| !s.trim().is_empty())
                .cloned()
        })
        .or_else(|| resp.retcode.map(|code| format!("retcode={code}")))
        .unwrap_or_else(|| user_str("error.invalid_response").to_string())
}

#[derive(Debug, Deserialize)]
struct OneBotMessage {
    #[serde(rename = "post_type")]
    post_type: String,
    #[serde(rename = "message_type")]
    message_type: Option<String>,
    #[serde(rename = "group_id")]
    group_id: Option<i64>,
    #[serde(rename = "user_id")]
    user_id: Option<i64>,
    #[serde(rename = "message")]
    message: Option<serde_json::Value>,
}

/// Parse a OneBot JSON message into a `QQMessage`.
/// Returns `None` if the message is not a group/private message or lacks required fields.
pub(crate) fn parse_onebot_message(json: &str) -> Option<QQMessage> {
    let msg: OneBotMessage = serde_json::from_str(json).ok()?;

    if msg.post_type != "message" {
        return None;
    }

    let user_id = msg.user_id?;

    let (group_id, message_text, mentioned_user_id) = match msg.message_type.as_deref() {
        Some("group") => {
            let group_id = msg.group_id?;
            let (text, mention) = extract_message_and_mention(&msg.message?);
            (Some(group_id), text, mention)
        }
        Some("private") => {
            let (text, _) = extract_message_and_mention(&msg.message?);
            (None, text, None)
        }
        _ => return None,
    };

    Some(QQMessage {
        group_id,
        user_id,
        message: message_text,
        mentioned_user_id,
    })
}

/// Extract plain text and a single @mention user ID from a OneBot message array.
/// Returns `(text, mentioned_user_id)` — the mention is `Some` only if exactly one user is @mentioned.
fn extract_message_and_mention(message: &serde_json::Value) -> (String, Option<i64>) {
    let arr = match message.as_array() {
        Some(a) => a,
        None => {
            let text = message.as_str().unwrap_or("").to_string();
            return (text, None);
        }
    };

    let mut text = String::new();
    let mut at_qqs: Vec<i64> = Vec::new();

    for segment in arr {
        match segment.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                if let Some(t) = segment
                    .get("data")
                    .and_then(|d| d.get("text"))
                    .and_then(|v| v.as_str())
                {
                    text.push_str(t);
                }
            }
            Some("at") => {
                if let Some(qq_val) = segment.get("data").and_then(|d| d.get("qq")) {
                    if let Some(qq) = qq_val.as_i64() {
                        at_qqs.push(qq);
                    } else if let Some(qq_str) = qq_val.as_str() {
                        if let Ok(qq) = qq_str.parse::<i64>() {
                            at_qqs.push(qq);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let mentioned_user_id = if at_qqs.len() == 1 {
        Some(at_qqs[0])
    } else {
        None
    };

    (text, mentioned_user_id)
}

pub(crate) async fn call_onebot_api(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    call_onebot_api_impl(api, action, params, |payload| async move {
        let mut sink = write.lock().await;
        sink.send(WsMsg::Text(payload.into()))
            .await
            .map_err(|e| e.to_string())
    })
    .await
}

async fn call_onebot_api_impl<F, Fut>(
    api: &OneBotApi,
    action: &str,
    params: serde_json::Value,
    send_payload: F,
) -> Result<serde_json::Value, String>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let echo = next_echo();
    let (tx, rx) = oneshot::channel();

    api.pending.lock().await.insert(
        echo.clone(),
        PendingEntry {
            sender: tx,
            created_at: std::time::Instant::now(),
        },
    );

    let json = serde_json::json!({
        "action": action,
        "params": params,
        "echo": echo,
    });

    if let Err(e) = send_payload(json.to_string()).await {
        api.pending.lock().await.remove(&echo);
        return Err(e);
    }

    let timeout_dur = Duration::from_secs(api.timeout.load(Ordering::Relaxed));
    let result = tokio::time::timeout(timeout_dur, rx).await;
    api.pending.lock().await.remove(&echo);

    match result {
        Ok(Ok(Ok(data))) => Ok(data),
        Ok(Ok(Err(msg))) => Err(msg),
        Ok(Err(_)) => Err(user_str("error.request_cancelled").to_string()),
        Err(_) => Err(user_str("error.request_timeout").to_string()),
    }
}

pub(crate) async fn get_group_member_list(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    group_id: i64,
) -> Result<HashSet<i64>, String> {
    let value = call_onebot_api(
        write,
        api,
        "get_group_member_list",
        serde_json::json!({"group_id": group_id}),
    )
    .await?;

    let data = value.as_array().ok_or(user_str("error.invalid_response"))?;

    let mut members = HashSet::new();
    for member in data {
        if let Some(user_id) = member.get("user_id").and_then(|v| v.as_i64()) {
            members.insert(user_id);
        }
    }
    Ok(members)
}

/// Send a text message to a QQ group via the OneBot WebSocket connection.
pub(crate) async fn send_group_msg(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    group_id: i64,
    message: &str,
) -> Result<(), String> {
    call_onebot_api(
        write,
        api,
        "send_group_msg",
        serde_json::json!({
            "group_id": group_id,
            "message": message
        }),
    )
    .await
    .map(|_| ())
}

/// Send a text message to a QQ user (private chat) via the OneBot WebSocket connection.
pub(crate) async fn send_private_msg(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    user_id: i64,
    message: &str,
) -> Result<(), String> {
    call_onebot_api(
        write,
        api,
        "send_private_msg",
        serde_json::json!({
            "user_id": user_id,
            "message": message
        }),
    )
    .await
    .map(|_| ())
}

/// Send a message with a base64-encoded image to a QQ user (private chat)
/// via the OneBot WebSocket connection.
pub(crate) async fn send_private_msg_with_image(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    user_id: i64,
    image_data: &[u8],
) -> Result<(), String> {
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(image_data);
    let segments = serde_json::json!([
        {
            "type": "image",
            "data": {
                "file": format!("base64://{}", b64)
            }
        }
    ]);
    call_onebot_api(
        write,
        api,
        "send_private_msg",
        serde_json::json!({
            "user_id": user_id,
            "message": segments
        }),
    )
    .await
    .map(|_| ())
}

/// Send a message with a voice record (base64-encoded MP3 bytes) to a QQ user
/// (private chat) via the OneBot WebSocket connection.
pub(crate) async fn send_private_msg_with_record(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    user_id: i64,
    mp3: &[u8],
) -> Result<(), String> {
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(mp3);
    let segments = serde_json::json!([
        {
            "type": "record",
            "data": {
                "file": format!("base64://{}", b64)
            }
        }
    ]);
    call_onebot_api(
        write,
        api,
        "send_private_msg",
        serde_json::json!({
            "user_id": user_id,
            "message": segments
        }),
    )
    .await
    .map(|_| ())
}

/// Send a message with a base64-encoded image to a QQ group via the OneBot WebSocket connection.
pub(crate) async fn send_group_msg_with_image(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    group_id: i64,
    image_data: &[u8],
) -> Result<(), String> {
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(image_data);
    let segments = serde_json::json!([
        {
            "type": "image",
            "data": {
                "file": format!("base64://{}", b64)
            }
        }
    ]);
    call_onebot_api(
        write,
        api,
        "send_group_msg",
        serde_json::json!({
            "group_id": group_id,
            "message": segments
        }),
    )
    .await
    .map(|_| ())
}

/// Send a message with a voice record (base64-encoded MP3 bytes) to a QQ group
/// via the OneBot WebSocket connection. Mirrors `send_group_msg_with_image`.
pub(crate) async fn send_group_msg_with_record(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    group_id: i64,
    mp3: &[u8],
) -> Result<(), String> {
    use base64::prelude::*;
    let b64 = BASE64_STANDARD.encode(mp3);
    let segments = serde_json::json!([
        {
            "type": "record",
            "data": {
                "file": format!("base64://{}", b64)
            }
        }
    ]);
    call_onebot_api(
        write,
        api,
        "send_group_msg",
        serde_json::json!({
            "group_id": group_id,
            "message": segments
        }),
    )
    .await
    .map(|_| ())
}
