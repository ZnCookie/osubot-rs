mod config;
mod scheduler;

use config::Config;
use scheduler::Scheduler;
use osubot_core::{
    parse_command,
    storage::Storage,
    response::{format_stats, format_stats_with_change},
    types::Command,
    api::{self, ApiError},
    OauthTokenCache, RateLimiter,
};
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use serde::Deserialize;
use tracing::{info, error, warn, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Debug, Clone)]
struct QQMessage {
    group_id: i64,
    user_id: i64,
    message: String,
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
    #[serde(rename = "sender")]
    sender: Option<OneBotSender>,
    #[serde(rename = "message")]
    message: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OneBotSender {
    #[serde(rename = "user_id")]
    user_id: Option<i64>,
}

/// Parse OneBot 11 JSON message, extract group message
fn parse_onebot_message(json: &str) -> Option<QQMessage> {
    let msg: OneBotMessage = serde_json::from_str(json).ok()?;

    if msg.post_type != "message" || msg.message_type.as_deref() != Some("group") {
        return None;
    }

    let group_id = msg.group_id?;
    let user_id = msg.user_id?;

    let message_text = extract_message_text(&msg.message?);

    Some(QQMessage {
        group_id,
        user_id,
        message: message_text,
    })
}

/// Extract plain text from OneBot message array
fn extract_message_text(message: &serde_json::Value) -> String {
    if let Some(arr) = message.as_array() {
        let mut text = String::new();
        for segment in arr {
            if let Some(text_seg) = segment.get("data").and_then(|d| d.get("text")) {
                if let Some(t) = text_seg.as_str() {
                    text.push_str(t);
                }
            }
        }
        text
    } else if let Some(s) = message.as_str() {
        s.to_string()
    } else {
        String::new()
    }
}

/// Handle command and send response
async fn handle_command(
    storage: Arc<Storage>,
    scheduler: Scheduler,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    msg: QQMessage,
    resp_tx: mpsc::Sender<String>,
) {
    let cmd = match parse_command(&msg.message) {
        Some(cmd) => cmd,
        None => return,
    };

    match cmd {
        Command::QuerySelf { mode } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "QuerySelf command");
            match storage.get_binding(msg.user_id) {
                Ok(Some(username)) => {
                    // Trigger update for all modes (bypassing cooldown)
                    scheduler.trigger_update(&username);
                    match api::fetch_user_stats(&rate_limiter, &oauth, &username, mode).await {
                        Ok(stats) => {
                            // Get change from storage (compares with 4h ago snapshot)
                            let change = storage.calculate_change(&username, mode, &stats).ok().flatten();
                            info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QuerySelf success");
                            let response = format_stats_with_change(&stats, &change, mode);
                            let _ = resp_tx.send(response).await;
                        }
                        Err(e) => {
                            warn!(username = %username, mode = ?mode, error = ?e, "QuerySelf failed");
                            let err_msg = match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            };
                            let _ = resp_tx.send(err_msg).await;
                        }
                    }
                }
                Ok(None) => {
                    info!(user_id = msg.user_id, "QuerySelf but no binding");
                    let _ = resp_tx.send("请先绑定 osu! 用户名，使用 绑定 <用户名>".to_string()).await;
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "QuerySelf database error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::QueryUser { username, mode } => {
            info!(group_id = msg.group_id, username = %username, mode = ?mode, "QueryUser command");
            match api::fetch_user_stats(&rate_limiter, &oauth, &username, mode).await {
                Ok(stats) => {
                    info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, "QueryUser success");
                    let response = format_stats(&stats, mode);
                    let _ = resp_tx.send(response).await;
                }
                Err(e) => {
                    warn!(username = %username, mode = ?mode, error = ?e, "QueryUser failed");
                    let err_msg = match e {
                        ApiError::NotFound => "未找到该用户".to_string(),
                        ApiError::MissingApiKey => "API Key 未配置".to_string(),
                        ApiError::OAuthError => "OAuth 认证失败".to_string(),
                        ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    };
                    let _ = resp_tx.send(err_msg).await;
                }
            }
        }
        Command::Bind { username } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, username = %username, "Bind command");
            // First check if this QQ already bound
            match storage.get_binding(msg.user_id) {
                Ok(Some(existing)) => {
                    info!(user_id = msg.user_id, existing = %existing, "Bind but already bound");
                    let _ = resp_tx.send(format!("你已经绑定为{},如需修改请先解绑", existing)).await;
                }
                Ok(None) => {
                    // Check if osu user exists
                    match api::get_user_info(&rate_limiter, &oauth, &username).await {
                        Ok(Some(_)) => {
                            // User exists, try to bind (checks if username already bound to another QQ)
                            match storage.bind(msg.user_id, &username) {
                                Ok(Ok(())) => {
                                    info!(user_id = msg.user_id, username = %username, "Bind success");
                                    let _ = resp_tx.send(format!("成功绑定为{}", username)).await;
                                }
                                Ok(Err(bound_qq)) => {
                                    info!(user_id = msg.user_id, username = %username, bound_qq = bound_qq, "Bind failed - username already bound");
                                    let _ = resp_tx.send("该 osu! 用户已绑定其他QQ".to_string()).await;
                                }
                                Err(_) => {
                                    error!(user_id = msg.user_id, username = %username, "Bind failed");
                                    let _ = resp_tx.send("绑定失败，请稍后重试".to_string()).await;
                                }
                            }
                        }
                        Ok(None) => {
                            info!(username = %username, "Bind but user not found");
                            let _ = resp_tx.send("未找到该 osu! 用户".to_string()).await;
                        }
                        Err(e) => {
                            warn!(username = %username, error = ?e, "Bind - user info check failed");
                            let err_msg = match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后再试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            };
                            let _ = resp_tx.send(err_msg).await;
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "Bind database error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::Unbind => {
            info!(user_id = msg.user_id, group_id = msg.group_id, "Unbind command");
            // Check if user has pending unbind confirmation (within 5 minutes)
            match storage.get_pending_unbind(msg.user_id) {
                Ok(Some(_)) => {
                    // Execute unbind and clear pending
                    match storage.unbind(msg.user_id) {
                        Ok(_) => {
                            storage.remove_pending_unbind(msg.user_id).ok();
                            info!(user_id = msg.user_id, "Unbind success");
                            let _ = resp_tx.send("解绑成功".to_string()).await;
                        }
                        Err(_) => {
                            error!(user_id = msg.user_id, "Unbind failed");
                            let _ = resp_tx.send("解绑失败，请稍后重试".to_string()).await;
                        }
                    }
                }
                Ok(None) => {
                    // Ask for confirmation and set pending
                    match storage.get_binding(msg.user_id) {
                        Ok(Some(username)) => {
                            storage.set_pending_unbind(msg.user_id).ok();
                            info!(user_id = msg.user_id, username = %username, "Unbind confirmation requested");
                            let _ = resp_tx.send(format!("确定要解除绑定 {} 吗？回复\"解绑\"确认", username)).await;
                        }
                        Ok(None) => {
                            info!(user_id = msg.user_id, "Unbind but no binding");
                            let _ = resp_tx.send("你还没有绑定任何 osu! 用户".to_string()).await;
                        }
                        Err(_) => {
                            error!(user_id = msg.user_id, "Unbind database error");
                            let _ = resp_tx.send("数据库错误".to_string()).await;
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "Unbind pending check error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::Help => {
            info!(user_id = msg.user_id, group_id = msg.group_id, "Help command");
            let help = "osu! 查分机器人命令：\n\
                        ~ 查询自己的 osu! 数据\n\
                        ~1/2/3 查询对应模式\n\
                        where <用户名> 查询他人\n\
                        绑定 <osu用户名>\n\
                        解绑";
            let _ = resp_tx.send(help.to_string()).await;
        }
    }
}

// ============================================================================
// spawn_blocking 示例 - CPU 密集型任务（图片生成）
// ============================================================================
// 所有耗时同步操作（图片生成、图像处理、文件压缩）必须通过
// tokio::task::spawn_blocking 执行，不要直接在异步任务里跑同步重代码。
// spawn_blocking 将任务交给独立阻塞线程池（默认 512 线程），
// 即使主运行时是单线程，多个用户请求也可以并行执行。
//
// 跨线程访问共享状态必须使用 Arc（Arc<Mutex<…>> 或 Arc<Atomic…>），
// 不能在 spawn_blocking 闭包中使用 Rc/RefCell。

#[derive(Debug)]
#[allow(dead_code)]
enum ImageTask {
    GenerateAvatar { username: String },
    GenerateCard { username: String },
}

#[allow(dead_code)]
fn generate_image_sync(_task: &ImageTask) -> Result<Vec<u8>, String> {
    std::thread::sleep(std::time::Duration::from_millis(100));
    Ok(Vec::from([0u8; 1024]))
}

#[allow(dead_code)]
async fn handle_generate_image(
    task: ImageTask,
    write: Arc<Mutex<WriteSink>>,
    group_id: i64,
    semaphore: Arc<tokio::sync::Semaphore>,
) {
    // 快速回复"正在处理"
    send_group_msg(&write, group_id, "正在生成图片，请稍候...").await;

    let _permit = semaphore.acquire().await.unwrap();

    let result = tokio::task::spawn_blocking(move || {
        generate_image_sync(&task)
    }).await;

    match result {
        Ok(Ok(image_data)) => {
            let msg = format!("图片生成完成: {} bytes", image_data.len());
            send_group_msg(&write, group_id, &msg).await;
        }
        Ok(Err(e)) => {
            let msg = format!("图片生成失败: {}", e);
            send_group_msg(&write, group_id, &msg).await;
        }
        Err(_) => {
            send_group_msg(&write, group_id, "图片生成失败: task panicked").await;
        }
    }
}

use tokio_tungstenite::tungstenite::Message as WsMsg;
type WriteSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    WsMsg,
>;

async fn send_group_msg(write: &Arc<Mutex<WriteSink>>, group_id: i64, message: &str) {
    let json = serde_json::json!({
        "action": "send_group_msg",
        "params": {
            "group_id": group_id,
            "message": message
        }
    });
    let mut sink = write.lock().await;
    let _ = sink.send(WsMsg::Text(json.to_string().into())).await;
}

#[tokio::main]
async fn main() {
    // Initialize logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set tracing subscriber");

    info!("Starting osubot...");

    let config = Config::from_path("osubot.toml")
        .expect("Failed to load config");

    info!("osubot starting...");
    info!("OneBot URL: {}", config.bot.onebot_url);

    let storage = Arc::new(Storage::new(&config.database.path)
        .expect("Failed to open database"));

    let oauth = Arc::new(OauthTokenCache::new(
        config.osu.client_id.clone(),
        config.osu.api_key.clone(),
    ));

    let rate_limiter = Arc::new(RateLimiter::new());

    // Create and spawn scheduler
    let scheduler = Scheduler::new(
        storage.clone(),
        oauth.clone(),
        rate_limiter.clone(),
        config.scheduler.clone(),
    );

    let scheduler_clone = scheduler.clone();
    let scheduler_handle = tokio::spawn(async move {
        scheduler_clone.run().await;
    });

    if config.osu.api_key.is_empty() || config.osu.api_key == "your-api-key-here" {
        warn!("API Key not configured. Please set osu.api_key in osubot.toml");
    }

    let (ws_stream, _) = connect_async(&config.bot.onebot_url)
        .await
        .expect("Failed to connect to OneBot 11");

    info!("Connected to OneBot 11");
    info!("WebSocket connection established");

    let (write, mut read) = ws_stream.split();
    let write = Arc::new(Mutex::new(write));

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                if let Some(qq_msg) = parse_onebot_message(&text) {
                    let (resp_tx, mut resp_rx) = mpsc::channel::<String>(1);

                    let write_clone = write.clone();
                    let group_id = qq_msg.group_id;
                    tokio::spawn(async move {
                        if let Some(response) = resp_rx.recv().await {
                            send_group_msg(&write_clone, group_id, &response).await;
                        }
                    });

                    let storage = storage.clone();
                    let oauth = oauth.clone();
                    let scheduler = scheduler.clone();
                    let rate_limiter = rate_limiter.clone();
                    tokio::spawn(async move {
                        handle_command(storage, scheduler, oauth, rate_limiter, qq_msg, resp_tx).await;
                    });
                }
            }
            Ok(Message::Close(_)) => {
                info!("Connection closed");
                break;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }
}
