pub mod config;
mod scheduler;

use config::Config;
use futures_util::{SinkExt, StreamExt};
use osubot_core::{
    api,
    handler::{self, CommandResult, HandlerContext},
    storage::Storage,
    OauthTokenCache, RateLimiter,
};
use osubot_render::{
    render_profile_card, render_score_card, ScoreCardData, PROFILE_VIEWPORT_WIDTH,
};
use scheduler::Scheduler;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{error, info, warn};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Debug, Deserialize)]
struct OneBotResponse {
    status: Option<String>,
    data: Option<serde_json::Value>,
    echo: Option<String>,
}

struct OneBotApi {
    pending: Mutex<HashMap<String, oneshot::Sender<serde_json::Value>>>,
}

impl OneBotApi {
    fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }
}

static NEXT_ECHO: AtomicU64 = AtomicU64::new(0);

fn next_echo() -> String {
    NEXT_ECHO.fetch_add(1, Ordering::Relaxed).to_string()
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
fn parse_onebot_message(json: &str) -> Option<osubot_core::types::QQMessage> {
    let msg: OneBotMessage = serde_json::from_str(json).ok()?;

    if msg.post_type != "message" || msg.message_type.as_deref() != Some("group") {
        return None;
    }

    let group_id = msg.group_id?;
    let user_id = msg.user_id?;

    let (message_text, mentioned_user_id) = extract_message_and_mention(&msg.message?);

    Some(osubot_core::types::QQMessage {
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

/// Shared bot state passed through command handlers
#[derive(Clone)]
struct BotContext {
    storage: Arc<Storage>,
    scheduler: Scheduler,
    oauth: Arc<OauthTokenCache>,
    rate_limiter: Arc<RateLimiter>,
    write: Arc<Mutex<WriteSink>>,
    #[allow(dead_code)]
    onebot_api: Arc<OneBotApi>,
    fetch_group_members: osubot_core::handler::GroupMemberFetcher,
}

/// Handle command and send response
async fn handle_command(
    ctx: BotContext,
    msg: osubot_core::types::QQMessage,
    resp_tx: mpsc::Sender<String>,
    irc_nickname: Option<String>,
) {
    let hctx = HandlerContext {
        storage: ctx.storage.clone(),
        oauth: ctx.oauth.clone(),
        rate_limiter: ctx.rate_limiter.clone(),
        trigger_update: {
            let scheduler = ctx.scheduler.clone();
            Some(std::sync::Arc::new(move |user_id: i64| {
                scheduler.trigger_update(user_id);
            }) as std::sync::Arc<dyn Fn(i64) + Send + Sync>)
        },
        fetch_group_members: Some(ctx.fetch_group_members.clone()),
    };

    let result = handler::handle_command(hctx, msg.clone(), irc_nickname).await;

    match result {
        CommandResult::Text(text) => {
            let _ = resp_tx.send(text).await;
        }
        CommandResult::ProfileCard(data) => {
            let write = ctx.write.clone();
            let group_id = msg.group_id;
            let resp_tx = resp_tx.clone();
            tokio::spawn(async move {
                match render_profile_card(
                    &data.html,
                    data.profile_hue,
                    &data.avatar_url,
                    &data.username,
                    PROFILE_VIEWPORT_WIDTH,
                    1200,
                )
                .await
                {
                    Ok(jpeg) => {
                        if send_group_msg_with_image(&write, group_id, &jpeg)
                            .await
                            .is_err()
                        {
                            let _ = resp_tx.send("图片发送失败".to_string()).await;
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "ProfileCard render failed");
                        let _ = resp_tx.send("渲染失败，请稍后重试".to_string()).await;
                    }
                }
            });
        }
        CommandResult::ScoreCard(sc) => {
            let write = ctx.write.clone();
            let group_id = msg.group_id;
            let resp_tx = resp_tx.clone();
            tokio::spawn(async move {
                match render_score_card(ScoreCardData::from(&sc)).await {
                    Ok(jpeg) => {
                        if send_group_msg_with_image(&write, group_id, &jpeg)
                            .await
                            .is_err()
                        {
                            let _ = resp_tx.send("图片发送失败".to_string()).await;
                        }
                    }
                    Err(e) => {
                        error!(error = ?e, "ScoreCard render failed");
                        let _ = resp_tx.send("渲染成绩卡失败".to_string()).await;
                    }
                }
            });
        }
        CommandResult::None => {}
    }
}

use tokio_tungstenite::tungstenite::Message as WsMsg;
use tracing_subscriber::fmt::time::LocalTime;
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
    if let Err(e) = sink.send(WsMsg::Text(json.to_string().into())).await {
        warn!(error = %e, group_id = group_id, "Failed to send group message (WebSocket may be disconnected)");
    }
}

async fn send_group_msg_with_image(
    write: &Arc<Mutex<WriteSink>>,
    group_id: i64,
    image_data: &[u8],
) -> Result<(), ()> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(image_data);
    let segments = serde_json::json!([
        {
            "type": "image",
            "data": {
                "file": format!("base64://{}", b64)
            }
        }
    ]);
    let json = serde_json::json!({
        "action": "send_group_msg",
        "params": {
            "group_id": group_id,
            "message": segments
        }
    });
    let mut sink = write.lock().await;
    sink.send(WsMsg::Text(json.to_string().into()))
        .await
        .map_err(|e| {
            warn!(error = %e, group_id = group_id, "Failed to send group image message");
        })
}

async fn call_onebot_api(
    write: &Arc<Mutex<WriteSink>>,
    api: &OneBotApi,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let echo = next_echo();
    let (tx, rx) = oneshot::channel();

    api.pending.lock().await.insert(echo.clone(), tx);

    let json = serde_json::json!({
        "action": action,
        "params": params,
        "echo": echo,
    });

    {
        let mut sink = write.lock().await;
        if let Err(e) = sink.send(WsMsg::Text(json.to_string().into())).await {
            warn!(error = %e, action = action, "Failed to send OneBot API call (WebSocket may be disconnected)");
            return Err(e.to_string());
        }
    }

    let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
    api.pending.lock().await.remove(&echo);

    match result {
        Ok(Ok(data)) => Ok(data),
        Ok(Err(_)) => Err("请求已取消".to_string()),
        Err(_) => Err("请求超时".to_string()),
    }
}

async fn get_group_member_list(
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

    let data = value.as_array().ok_or("无效的响应数据")?;

    let mut members = HashSet::new();
    for member in data {
        if let Some(user_id) = member.get("user_id").and_then(|v| v.as_i64()) {
            members.insert(user_id);
        }
    }
    Ok(members)
}

async fn handle_irc_message(
    storage: Arc<Storage>,
    irc_msg: osubot_core::irc::IrcPrivateMessage,
    write: Arc<Mutex<WriteSink>>,
    rate_limiter: Arc<RateLimiter>,
    oauth: Arc<OauthTokenCache>,
) {
    let code = irc_msg.message.trim();

    let pending = match storage.get_pending_bind(code) {
        Ok(Some(p)) => p,
        Ok(None) => {
            warn!(code = code, sender = %irc_msg.sender, "No matching pending bind for IRC code");
            return;
        }
        Err(_) => {
            error!("Database error looking up pending bind");
            return;
        }
    };

    // Check if the sender matches the target username
    // osu! replaces spaces with underscores in IRC nicks
    if irc_msg.sender.to_lowercase() != pending.target_username.replace(' ', "_").to_lowercase() {
        storage.remove_pending_bind(code).ok();
        let msg = "绑定失败（绑定的不是本人）";
        send_group_msg(&write, pending.group_id, msg).await;
        return;
    }

    // Get user info first to obtain user_id
    match api::get_user_info(&rate_limiter, &oauth, &pending.target_username).await {
        Ok(Some(info)) => {
            // Cache the numeric user ID
            if let Err(e) = storage.set_user_id(&pending.target_username, info.id) {
                warn!(
                    "Failed to cache user_id for {}: {e}",
                    pending.target_username
                );
            }
            // Perform the bind
            match storage.bind(pending.qq_user_id, info.id, &info.username) {
                Ok(Ok(())) => {
                    storage.remove_pending_bind(code).ok();
                    info!(qq = pending.qq_user_id, username = %info.username, "Bind verified and completed");
                    let msg = format!("成功绑定为{}", info.username);
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
        Ok(None) => {
            storage.remove_pending_bind(code).ok();
            warn!("User {} not found during IRC bind", pending.target_username);
            let msg = "绑定失败（用户不存在）";
            send_group_msg(&write, pending.group_id, msg).await;
        }
        Err(e) => {
            storage.remove_pending_bind(code).ok();
            warn!(
                "Failed to fetch user info for {} during IRC bind: {e}",
                pending.target_username
            );
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
        .with_timer(LocalTime::rfc_3339())
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("Starting osubot...");

    osubot_render::ensure_cache_dir().await;

    let config = Config::from_path("osubot.toml").expect("Failed to load config");

    info!("osubot starting...");
    info!("OneBot URL: {}", config.bot.onebot_url);

    let storage = Arc::new(Storage::new(&config.database.path).expect("Failed to open database"));

    let oauth = Arc::new(OauthTokenCache::new(
        config.osu.client_id.clone(),
        config.osu.api_key.clone(),
    ));

    let rate_limiter = Arc::new(RateLimiter::new());

    // Backfill osu! user IDs for existing bindings that don't have cached IDs
    match storage.get_users_without_ids() {
        Ok(users) if !users.is_empty() => {
            info!("Backfilling user IDs for {} users...", users.len());
            for username in &users {
                match api::get_user_info(&rate_limiter, &oauth, username).await {
                    Ok(Some(info)) => {
                        if let Err(e) = storage.set_user_id(username, info.id) {
                            warn!("Failed to cache user_id for {username}: {e}");
                        } else {
                            info!("Cached user_id for {username}: {}", info.id);
                        }
                    }
                    Ok(None) => {
                        warn!("User {username} not found during backfill");
                    }
                    Err(e) => {
                        warn!("Failed to fetch user info for {username} during backfill: {e}");
                    }
                }
            }
        }
        Ok(_) => info!("All bound users already have cached user IDs"),
        Err(e) => error!("Failed to query users without IDs: {e}"),
    }

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

    let irc_enabled = config.irc.enabled;

    if irc_enabled && (config.irc.nickname.is_empty() || config.irc.password.is_empty()) {
        panic!("IRC is enabled but nickname or password is not set in osubot.toml");
    }

    let irc_nickname = if irc_enabled {
        Some(config.irc.nickname.clone())
    } else {
        None
    };

    // Set up IRC client
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

    let irc_write: Arc<Mutex<Option<Arc<Mutex<WriteSink>>>>> =
        Arc::new(Mutex::new(None));

    tokio::spawn({
        let irc_w = irc_write.clone();
        let storage = storage.clone();
        let rate_limiter = rate_limiter.clone();
        let oauth = oauth.clone();
        async move {
            while let Some(irc_msg) = irc_rx.recv().await {
                let write = {
                    let guard = irc_w.lock().await;
                    guard.as_ref().cloned()
                };
                let Some(write) = write else {
                    warn!("IRC message dropped: bot not connected to OneBot");
                    continue;
                };
                let storage = storage.clone();
                let rate_limiter = rate_limiter.clone();
                let oauth = oauth.clone();
                tokio::spawn(async move {
                    handle_irc_message(storage, irc_msg, write, rate_limiter, oauth).await;
                });
            }
        }
    });

    loop {
        let ws_stream = loop {
            match connect_async(&config.bot.onebot_url).await {
                Ok((stream, _)) => break stream,
                Err(e) => {
                    warn!(error = %e, url = %config.bot.onebot_url, "Failed to connect to OneBot, retrying in 5s...");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        };

        info!("Connected to OneBot 11");
        info!("WebSocket connection established");

        let (write, mut read) = ws_stream.split();
        let write = Arc::new(Mutex::new(write));
        *irc_write.lock().await = Some(write.clone());

        let onebot_api = Arc::new(OneBotApi::new());

        let fetch_group_members: osubot_core::handler::GroupMemberFetcher = {
            let write = write.clone();
            let onebot_api = onebot_api.clone();
            Arc::new(move |group_id| {
                let write = write.clone();
                let onebot_api = onebot_api.clone();
                Box::pin(async move {
                    match get_group_member_list(&write, &onebot_api, group_id).await {
                        Ok(m) => Ok(m.into_iter().collect()),
                        Err(e) => Err(e),
                    }
                })
            })
        };

        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(resp) = serde_json::from_str::<OneBotResponse>(&text) {
                        if resp.status.is_some() {
                            if let Some(echo) = resp.echo {
                                let mut pending = onebot_api.pending.lock().await;
                                if let Some(tx) = pending.remove(&echo) {
                                    let _ = tx
                                        .send(resp.data.unwrap_or(serde_json::Value::Null));
                                }
                                continue;
                            }
                        }
                    }

                    if let Some(qq_msg) = parse_onebot_message(&text) {
                        let (resp_tx, mut resp_rx) = mpsc::channel::<String>(1);

                        let write_clone = write.clone();
                        let group_id = qq_msg.group_id;
                        tokio::spawn(async move {
                            if let Some(response) = resp_rx.recv().await {
                                send_group_msg(&write_clone, group_id, &response).await;
                            }
                        });

                        let ctx = BotContext {
                            storage: storage.clone(),
                            scheduler: scheduler.clone(),
                            oauth: oauth.clone(),
                            rate_limiter: rate_limiter.clone(),
                            write: write.clone(),
                            onebot_api: onebot_api.clone(),
                            fetch_group_members: fetch_group_members.clone(),
                        };
                        let irc_nickname = irc_nickname.clone();
                        tokio::spawn(async move {
                            handle_command(ctx, qq_msg, resp_tx, irc_nickname).await;
                        });
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("WebSocket connection closed");
                    break;
                }
                Err(e) => {
                    error!("WebSocket error: {}", e);
                    break;
                }
                _ => {}
            }
        }

        *irc_write.lock().await = None;
        info!("Disconnected from OneBot, reconnecting in 3s...");
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
