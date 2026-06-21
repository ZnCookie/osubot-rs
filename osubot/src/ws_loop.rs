//! WebSocket 重连循环：每次连接创建 plugin 运行时、ping、tick、消息循环。
//! 提取自原 main.rs:4008-4440。

#![allow(clippy::too_many_lines)]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tracing::{debug, error, info, warn};

use crate::app_state::AppState;
use crate::command::handle_command;
use crate::constants;
use crate::last_beatmap_cache::LastBeatmapCache;
use crate::plugin_runtime::PluginRuntime;
use crate::BotContext;
use crate::InFlightGuard;
use crate::{parse_onebot_message, send_group_msg, OneBotResponse, WriteSink};
use osubot_core::log_fmt;
use osubot_core::strings::user_str;

/// WS read half type alias（与 main.rs 一致）。
type ReadHalf = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;

/// WS write half type alias（与 main.rs WriteSink 一致）。
type WsSplitSink =
    futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

/// 单条 WS 文本帧大小上限。OneBot 实际单条消息远小于 1 MiB；
/// tungstenite 默认 16 MiB 太宽松，恶意帧会直接 OOM 路径分配。
const MAX_WS_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

/// Per-connection token bucket 上限。每秒最多处理 100 条 incoming message，
/// 超出丢弃并 warn。防御 OneBot 上游 bug/恶意洪泛。
const MAX_MESSAGES_PER_SECOND: u32 = 100;

/// 同步 token bucket：每次 `try_acquire` 同步加锁、短持锁、不跨 await。
/// 用 `std::sync::Mutex` 而非 `tokio::sync::Mutex`，避免在同步路径上 spawn。
pub(crate) struct RateLimiter {
    last_refill: StdMutex<Instant>,
    tokens: StdMutex<u32>,
}

impl RateLimiter {
    pub(crate) fn new() -> Self {
        Self {
            last_refill: StdMutex::new(Instant::now()),
            tokens: StdMutex::new(MAX_MESSAGES_PER_SECOND),
        }
    }
    pub(crate) fn try_acquire(&self) -> bool {
        let now = Instant::now();
        let mut last = self.last_refill.lock().unwrap();
        let mut tokens = self.tokens.lock().unwrap();
        let elapsed = now.duration_since(*last).as_secs_f64();
        let refill = (elapsed * MAX_MESSAGES_PER_SECOND as f64) as u32;
        if refill > 0 {
            *tokens = tokens.saturating_add(refill).min(MAX_MESSAGES_PER_SECOND);
            *last = now;
        }
        if *tokens == 0 {
            false
        } else {
            *tokens -= 1;
            true
        }
    }
}

/// WebSocket 重连 + 消息循环主函数。
/// 提取自原 main.rs:4011-4440。
pub(super) async fn run_ws_reconnect_loop(
    state: AppState,
    drain: Arc<std::sync::atomic::AtomicBool>,
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
) {
    // http_client / blocking_http_client 每次连接创建（与原 main.rs 一致）
    let http_client = reqwest::Client::new();
    let blocking_http_client = reqwest::blocking::Client::new();

    let mut reconnect_delay = 1u64;
    loop {
        if state.shutdown.load(std::sync::atomic::Ordering::Acquire) {
            tracing::info!("{}", log_fmt!("main.shutdown_no_reconnect"));
            break;
        }
        let onebot_url = state.config.read().await.bot.onebot_url.clone();
        info!(url = %onebot_url, "{}", log_fmt!("main.connecting_ws"));

        let (write, mut read) =
            match connect_ws(&onebot_url, &mut reconnect_delay, &state.shutdown).await {
                Ok(ws) => ws,
                Err(()) => {
                    // connect_ws 内部 sleep 已被 shutdown 打断，退出整个循环
                    if state.shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    continue;
                }
            };
        let write = Arc::new(Mutex::new(write));

        // 更新 current_write（提取自 main.rs:4037-4047）
        {
            let mut cw = state.current_write.lock().await;
            let old = cw.replace(write.clone());
            if let Some(old) = old {
                let mut sink = old.lock().await;
                if let Err(e) = sink.close().await {
                    tracing::debug!(error = %e, "{}", log_fmt!("main.ws_sink_close_failed"));
                }
            }
        }

        let connection_alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

        // 启动 ping 任务（提取自 main.rs:4051-4071）
        let ping_write = write.clone();
        let ping_shutdown = state.shutdown.clone();
        let ping_connection_alive = connection_alive.clone();
        let ping_handle = spawn_ping_task(ping_write, ping_shutdown, ping_connection_alive);

        let last_beatmap = LastBeatmapCache::new();

        // 初始化 plugin 运行时
        let plugin_rt = PluginRuntime::new(
            &state,
            write.clone(),
            drain.clone(),
            in_flight.clone(),
            http_client.clone(),
            blocking_http_client.clone(),
        )
        .await;

        // 启动 tick 循环（同步调用，返回 JoinHandle）
        let mut tick_handle = plugin_rt.spawn_tick_loop();

        // 消息循环
        run_message_loop(
            &state,
            write.clone(),
            &mut read,
            last_beatmap,
            drain.clone(),
            in_flight.clone(),
            &mut reconnect_delay,
        )
        .await;

        // 断开清理
        connection_alive.store(false, std::sync::atomic::Ordering::Relaxed);
        state.force_reconnect.store(false, Ordering::SeqCst);
        ping_handle.abort();

        // 等待 tick 完成，超时后强制 abort
        if tokio::time::timeout(
            Duration::from_secs(constants::TICK_HANDLE_SHUTDOWN_SECS),
            &mut tick_handle,
        )
        .await
        .is_err()
        {
            tick_handle.abort();
        }

        // 清理 current_write（提取自 main.rs:4424-4428）
        clear_current_write(&state).await;

        // 关闭 plugin（提取自 main.rs:4430-4436）
        plugin_rt.shutdown_for_reconnect().await;

        // 修复：sleep 期间 SIGINT 触发的 shutdown 不能立即生效（最长阻塞 60s）。
        // 用 select 同时等 sleep 和 shutdown 信号。
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(reconnect_delay)) => {}
            _ = wait_shutdown(&state.shutdown) => {
                tracing::info!("{}", log_fmt!("main.shutdown_during_reconnect_sleep"));
                break;
            }
        }
        reconnect_delay = (reconnect_delay * 2).min(60);
    }
}

/// 等待 shutdown 信号置位。
async fn wait_shutdown(flag: &std::sync::Arc<std::sync::atomic::AtomicBool>) {
    loop {
        if flag.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// 连接 WebSocket。失败时 backoff 并返回 Err(())。
/// 提取自原 main.rs:4017-4030。
async fn connect_ws(
    onebot_url: &str,
    reconnect_delay: &mut u64,
    shutdown: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<(WsSplitSink, ReadHalf), ()> {
    match connect_async(onebot_url).await {
        Ok((stream, _)) => {
            *reconnect_delay = 1;
            info!("{}", log_fmt!("main.ws_connected"));
            let (write, read) = stream.split();
            Ok((write, read))
        }
        Err(e) => {
            error!(
                error = %e,
                delay = *reconnect_delay,
                "{}",
                log_fmt!("main.ws_connect_failed", secs = *reconnect_delay)
            );
            // sleep 期间允许 shutdown 打断，避免 SIGINT 后阻塞 60s
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(*reconnect_delay)) => {}
                _ = wait_shutdown(shutdown) => {
                    tracing::info!("{}", log_fmt!("main.shutdown_during_connect_sleep"));
                    return Err(());
                }
            }
            *reconnect_delay = (*reconnect_delay * 2).min(60);
            Err(())
        }
    }
}

/// 启动 ping 任务（提取自 main.rs:4051-4071）。
fn spawn_ping_task(
    ping_write: Arc<Mutex<WriteSink>>,
    ping_shutdown: Arc<std::sync::atomic::AtomicBool>,
    ping_connection_alive: Arc<std::sync::atomic::AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(Duration::from_secs(constants::PING_INTERVAL_SECS));
        loop {
            interval.tick().await;
            if ping_shutdown.load(std::sync::atomic::Ordering::Acquire)
                || !ping_connection_alive.load(std::sync::atomic::Ordering::Relaxed)
            {
                break;
            }
            let mut sink = ping_write.lock().await;
            if let Err(e) = sink.send(Message::Ping(vec![].into())).await {
                tracing::debug!(error = %e, "{}", log_fmt!("main.ws_ping_failed"));
                break;
            }
        }
    })
}

/// 消息循环（提取自原 main.rs:4275-4408）。
async fn run_message_loop(
    state: &AppState,
    write: Arc<Mutex<WriteSink>>,
    read: &mut ReadHalf,
    last_beatmap: LastBeatmapCache,
    drain: Arc<std::sync::atomic::AtomicBool>,
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
    reconnect_delay: &mut u64,
) {
    const SPAWN_COUNT: usize = 2; // 必须与下方两个 tokio::spawn 中各持有的 InFlightGuard 数量一致
                                  // 编译期断言：若编译失败，请更新 SPAWN_COUNT
    const _: [(); SPAWN_COUNT] = [(); 2];

    let limiter = RateLimiter::new();

    loop {
        if state.shutdown.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        if state.force_reconnect.load(Ordering::SeqCst) {
            info!("{}", log_fmt!("main.force_reconnect_url_changed"));
            break;
        }
        match read.next().await {
            Some(Ok(Message::Text(text))) => {
                if !limiter.try_acquire() {
                    warn!(
                        "{}",
                        log_fmt!(
                            "main.ws_message_rate_limited",
                            limit = MAX_MESSAGES_PER_SECOND
                        )
                    );
                    continue;
                }
                // 防御：恶意/故障 OneBot 框架发大帧（tungstenite 默认 16 MiB），
                // 解析前先丢。OneBot 正常单条消息远小于 1 MiB。
                if text.len() > MAX_WS_MESSAGE_SIZE {
                    tracing::warn!(
                        size = text.len(),
                        limit = MAX_WS_MESSAGE_SIZE,
                        "{}",
                        log_fmt!("main.ws_message_too_large_drop")
                    );
                    continue;
                }
                if let Ok(resp) = serde_json::from_str::<OneBotResponse>(&text) {
                    if resp.status.is_some() {
                        if let Some(echo) = resp.echo {
                            let mut pending = state.onebot_api.pending.lock().await;
                            if let Some(entry) = pending.remove(&echo) {
                                let _ = entry
                                    .sender
                                    .send(resp.data.unwrap_or(serde_json::Value::Null));
                            }
                            continue;
                        }
                    }
                }

                if let Some(qq_msg) = parse_onebot_message(&text) {
                    // 群黑白名单
                    {
                        let cfg = state.config.read().await;
                        if !cfg.group_filter.is_group_allowed(qq_msg.group_id) {
                            debug!(
                                group_id = qq_msg.group_id,
                                mode = ?cfg.group_filter.mode,
                                "{}",
                                log_fmt!("main.group_filtered")
                            );
                            continue;
                        }
                    }

                    let (resp_tx, mut resp_rx) = tokio::sync::mpsc::channel::<String>(1);

                    let increment_result =
                        in_flight.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                            if drain.load(Ordering::SeqCst) {
                                None
                            } else {
                                Some(current + SPAWN_COUNT)
                            }
                        });
                    if increment_result.is_err() {
                        info!(
                            group_id = qq_msg.group_id,
                            "{}",
                            log_fmt!("main.hot_reload_skip")
                        );
                        continue;
                    }

                    let write_clone = write.clone();
                    let group_id = qq_msg.group_id;
                    let in_flight1 = in_flight.clone();
                    tokio::spawn(async move {
                        let _guard = InFlightGuard(in_flight1);
                        if let Some(response) = resp_rx.recv().await {
                            send_group_msg(&write_clone, group_id, &response).await;
                        }
                    });

                    let ctx = BotContext::for_dispatch(state, write.clone(), last_beatmap.clone());
                    let in_flight2 = in_flight.clone();
                    tokio::spawn(async move {
                        let _guard = InFlightGuard(in_flight2);
                        let command_timeout =
                            Duration::from_secs(ctx.config.read().await.bot.command_timeout_secs);
                        let qq = qq_msg.user_id;
                        if tokio::time::timeout(
                            command_timeout,
                            handle_command(ctx, qq_msg, resp_tx.clone()),
                        )
                        .await
                        .is_err()
                        {
                            tracing::warn!(
                                "{}",
                                log_fmt!("main.command_timeout", secs = command_timeout.as_secs())
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("error.command_timeout")
                                        .replace("{qq}", &qq.to_string()),
                                )
                                .await;
                        }
                    });
                }
            }
            Some(Ok(Message::Close(_))) => {
                // 服务端计划内重启（go-cqhttp / Lagrange 重启）走 Close 帧，
                // 不应触发指数退避。连续 Close 会把 delay 推到 60s 永久滞留。
                *reconnect_delay = 1;
                warn!(
                    "{}",
                    log_fmt!("main.ws_closed_reconnect", secs = *reconnect_delay)
                );
                break;
            }
            Some(Err(e)) => {
                error!(
                    error = %e,
                    "{}",
                    log_fmt!("main.ws_error_reconnect", secs = *reconnect_delay)
                );
                break;
            }
            None => {
                warn!(
                    "{}",
                    log_fmt!("main.ws_stream_ended", secs = *reconnect_delay)
                );
                break;
            }
            _ => {}
        }
    }
}

/// 清理 current_write（提取自 main.rs:4424-4428）。
async fn clear_current_write(state: &AppState) {
    let mut cw = state.current_write.lock().await;
    *cw = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_burst_then_block() {
        let l = RateLimiter::new();
        for _ in 0..MAX_MESSAGES_PER_SECOND {
            assert!(l.try_acquire());
        }
        assert!(!l.try_acquire(), "should be exhausted");
    }

    #[test]
    fn rate_limiter_refills_over_time() {
        let l = RateLimiter::new();
        for _ in 0..MAX_MESSAGES_PER_SECOND {
            assert!(l.try_acquire());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(l.try_acquire(), "should refill after sleep");
    }
}
