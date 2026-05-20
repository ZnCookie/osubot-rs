mod config;
mod scheduler;

use config::Config;
use futures_util::{SinkExt, StreamExt};
use osubot_core::{
    api::{self, ApiError},
    highlight::{format_highlight, get_highlight, HighlightError},
    parse_command,
    response::format_stats_with_change,
    storage::Storage,
    types::Command,
    OauthTokenCache, RateLimiter,
};
use scheduler::Scheduler;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Debug, Clone)]
struct QQMessage {
    group_id: i64,
    user_id: i64,
    message: String,
    mentioned_user_id: Option<i64>,
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

/// Parse OneBot 11 JSON message, extract group message
fn parse_onebot_message(json: &str) -> Option<QQMessage> {
    let msg: OneBotMessage = serde_json::from_str(json).ok()?;

    if msg.post_type != "message" || msg.message_type.as_deref() != Some("group") {
        return None;
    }

    let group_id = msg.group_id?;
    let user_id = msg.user_id?;

    let (message_text, mentioned_user_id) = extract_message_and_mention(&msg.message?);

    Some(QQMessage {
        group_id,
        user_id,
        message: message_text,
        mentioned_user_id,
    })
}

/// Extract plain text and first valid @mention QQ ID from OneBot message array.
/// Returns None for mentioned_user_id when:
/// - No "at" segments
/// - More than one "at" segment
/// - The "at" segment's qq value is not a valid i64 (e.g. "all" for @全体成员)
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
                        // qq="all" falls through — not a valid i64, ignored
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

/// Handle command and send response
async fn handle_command(
    storage: Arc<Storage>,
    scheduler: Scheduler,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    msg: QQMessage,
    resp_tx: mpsc::Sender<String>,
    irc_enabled: bool,
) {
    let cmd = match parse_command(&msg.message, msg.mentioned_user_id) {
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
                            // Get change from storage (compares with 24h ago snapshot)
                            let change = storage
                                .calculate_change(&username, mode, &stats)
                                .ok()
                                .flatten();
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
                    let _ = resp_tx
                        .send("请先绑定 osu! 用户名，使用 绑定 <用户名>".to_string())
                        .await;
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
                    let change = storage
                        .calculate_change(&username, mode, &stats)
                        .ok()
                        .flatten();
                    info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QueryUser success");
                    let response = format_stats_with_change(&stats, &change, mode);
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
        Command::QueryMentionedUser { qq, mode } => {
            info!(qq = qq, group_id = msg.group_id, mode = ?mode, "QueryMentionedUser command");
            match storage.get_binding(qq) {
                Ok(Some(username)) => {
                    scheduler.trigger_update(&username);
                    match api::fetch_user_stats(&rate_limiter, &oauth, &username, mode).await {
                        Ok(stats) => {
                            let change = storage
                                .calculate_change(&username, mode, &stats)
                                .ok()
                                .flatten();
                            info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "QueryMentionedUser success");
                            let response = format_stats_with_change(&stats, &change, mode);
                            let _ = resp_tx.send(response).await;
                        }
                        Err(e) => {
                            warn!(username = %username, mode = ?mode, error = ?e, "QueryMentionedUser failed");
                            let err_msg = match e {
                                ApiError::NotFound => "未找到该用户".to_string(),
                                ApiError::MissingApiKey => "API Key 未配置".to_string(),
                                ApiError::OAuthError => "OAuth 认证失败".to_string(),
                                ApiError::RateLimited => "查询繁忙，请稍后重试".to_string(),
                                _ => "查询失败，请稍后重试".to_string(),
                            };
                            let _ = resp_tx.send(err_msg).await;
                        }
                    }
                }
                Ok(None) => {
                    info!(qq = qq, "QueryMentionedUser but no binding");
                    let _ = resp_tx
                        .send(
                            "该用户未绑定 osu! 账号，请使用 绑定 <osu用户名> 命令绑定".to_string(),
                        )
                        .await;
                }
                Err(_) => {
                    error!(qq = qq, "QueryMentionedUser database error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::Bind { username } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, username = %username, "Bind command");
            // First check if this QQ already bound
            match storage.get_binding(msg.user_id) {
                Ok(Some(existing)) => {
                    info!(user_id = msg.user_id, existing = %existing, "Bind but already bound");
                    let _ = resp_tx
                        .send(format!("你已经绑定为{},如需修改请先解绑", existing))
                        .await;
                }
                Ok(None) => {
                    if irc_enabled {
                        // IRC auth mode: check rate limit (one pending bind at a time)
                        match storage.has_pending_bind(msg.user_id) {
                            Ok(true) => {
                                let _ = resp_tx
                                    .send(
                                        "你已有进行中的绑定请求，请等待当前验证码过期后再试"
                                            .to_string(),
                                    )
                                    .await;
                                return;
                            }
                            Err(_) => {
                                error!(user_id = msg.user_id, "Failed to check pending bind");
                                let _ = resp_tx.send("绑定失败，请稍后重试".to_string()).await;
                                return;
                            }
                            _ => {}
                        }
                        // Generate code and wait for verification
                        match storage.add_pending_bind(msg.user_id, msg.group_id, &username) {
                            Ok(code) => {
                                info!(user_id = msg.user_id, username = %username, code = %code, "Pending bind created");
                                let _ = resp_tx
                                    .send(format!(
                                        "您的验证码是 {}，请在2分钟内通过 osu! IRC 私聊发送给我",
                                        code
                                    ))
                                    .await;
                            }
                            Err(_) => {
                                error!(user_id = msg.user_id, "Failed to create pending bind");
                                let _ = resp_tx.send("绑定失败，请稍后重试".to_string()).await;
                            }
                        }
                    } else {
                        // Direct bind (original logic)
                        match api::get_user_info(&rate_limiter, &oauth, &username).await {
                            Ok(Some(_)) => match storage.bind(msg.user_id, &username) {
                                Ok(Ok(())) => {
                                    info!(user_id = msg.user_id, username = %username, "Bind success");
                                    let _ = resp_tx.send(format!("成功绑定为{}", username)).await;
                                }
                                Ok(Err(bound_qq)) => {
                                    info!(user_id = msg.user_id, username = %username, bound_qq = bound_qq, "Bind failed - username already bound");
                                    let _ =
                                        resp_tx.send("该 osu! 用户已绑定其他QQ".to_string()).await;
                                }
                                Err(_) => {
                                    error!(user_id = msg.user_id, username = %username, "Bind failed");
                                    let _ = resp_tx.send("绑定失败，请稍后重试".to_string()).await;
                                }
                            },
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
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "Bind database error");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                }
            }
        }
        Command::Unbind => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "Unbind command"
            );
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
                            let _ = resp_tx
                                .send(format!("确定要解除绑定 {} 吗？回复\"解绑\"确认", username))
                                .await;
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
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "Help command"
            );
            let help = "osu! 查分机器人命令：\n\
                        ~ 查询自己的 osu! 数据\n\
                        ~1/2/3 查询对应模式\n\
                        where <用户名> 查询他人\n\
                        绑定 <osu用户名>\n\
                        解绑";
            let _ = resp_tx.send(help.to_string()).await;
        }
        Command::Highlight { mode } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "Highlight command");

            // Get all bindings (group member filtering requires proper OneBot API)
            let all_bindings = match storage.get_all_user_bindings() {
                Ok(bindings) => bindings,
                Err(_) => {
                    error!("Highlight failed to get bindings");
                    let _ = resp_tx.send("数据库错误".to_string()).await;
                    return;
                }
            };

            if all_bindings.is_empty() {
                let _ = resp_tx
                    .send("你群根本没有人绑定 osu! 账号".to_string())
                    .await;
                return;
            }

            // Fetch highlight data
            match get_highlight(&storage, &rate_limiter, &oauth, &all_bindings, mode).await {
                Ok(result) => {
                    let response = format_highlight(&result);
                    let _ = resp_tx.send(response).await;
                }
                Err(e) => {
                    warn!(error = ?e, "Highlight fetch failed");
                    let err_msg = match e {
                        HighlightError::NoData => "你群根本没有人屙屎。".to_string(),
                        _ => "查询失败，请稍后重试".to_string(),
                    };
                    let _ = resp_tx.send(err_msg).await;
                }
            }
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

async fn handle_irc_message(
    storage: Arc<Storage>,
    irc_msg: osubot_core::irc::IrcPrivateMessage,
    write: Arc<Mutex<WriteSink>>,
) {
    let code = irc_msg.message.trim();

    let pending = match storage.get_pending_bind(code) {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(_) => {
            error!("Database error looking up pending bind");
            return;
        }
    };

    // Check if the sender matches the target username
    // osu! replaces spaces with underscores in IRC nicks, so normalize both sides
    if irc_msg.sender.replace(' ', "_").to_lowercase() != pending.target_username.replace(' ', "_").to_lowercase() {
        storage.remove_pending_bind(code).ok();
        let msg = "绑定失败（绑定的不是本人）";
        send_group_msg(&write, pending.group_id, msg).await;
        return;
    }

    // Perform the bind
    match storage.bind(pending.qq_user_id, &pending.target_username) {
        Ok(Ok(())) => {
            storage.remove_pending_bind(code).ok();
            info!(qq = pending.qq_user_id, username = %pending.target_username, "Bind verified and completed");
            let msg = format!("成功绑定为{}", pending.target_username);
            send_group_msg(&write, pending.group_id, &msg).await;
        }
        Ok(Err(_)) => {
            storage.remove_pending_bind(code).ok();
            let msg = "绑定失败（该 osu! 用户已绑定其他 QQ）";
            send_group_msg(&write, pending.group_id, msg).await;
        }
        Err(_) => {
            storage.remove_pending_bind(code).ok();
            let msg = "绑定失败，请稍后重试";
            send_group_msg(&write, pending.group_id, msg).await;
        }
    }
}

#[tokio::main]
async fn main() {
    // Initialize logging
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("osubot=debug,osubot_core=debug,info"));

    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("Starting osubot...");

    let config = Config::from_path("osubot.toml").expect("Failed to load config");

    info!("osubot starting...");
    info!("OneBot URL: {}", config.bot.onebot_url);

    let storage = Arc::new(Storage::new(&config.database.path).expect("Failed to open database"));

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
    let _scheduler_handle = tokio::spawn(async move {
        scheduler_clone.run().await;
    });

    // Set up IRC client
    let irc_enabled = config.irc.enabled;
    let (irc_tx, mut irc_rx) = mpsc::channel::<osubot_core::irc::IrcPrivateMessage>(100);

    if irc_enabled {
        let irc_config = osubot_core::IrcConfig::new(
            config.irc.enabled,
            &config.irc.server,
            config.irc.port,
            &config.irc.nickname,
            &config.irc.password,
        );
        let irc_client = osubot_core::irc::IrcClient::new(irc_config, irc_tx);
        tokio::spawn(async move {
            if let Err(e) = irc_client.run().await {
                error!(error = %e, "IRC client error");
            }
        });
    }

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

    // Spawn IRC message handler
    let write_for_irc = write.clone();
    let storage_for_irc = storage.clone();
    tokio::spawn(async move {
        while let Some(irc_msg) = irc_rx.recv().await {
            let storage = storage_for_irc.clone();
            let write = write_for_irc.clone();
            tokio::spawn(async move {
                handle_irc_message(storage, irc_msg, write).await;
            });
        }
    });

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
                        handle_command(
                            storage,
                            scheduler,
                            oauth,
                            rate_limiter,
                            qq_msg,
                            resp_tx,
                            irc_enabled,
                        )
                        .await;
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
