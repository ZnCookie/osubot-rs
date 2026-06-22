//! 后台任务：scheduler、IRC、文件 watcher、shutdown 信号、OneBot cleanup、用户 ID backfill。
//!
//! 每个 `spawn_*` 函数立即 spawn 一个 tokio 任务并返回。
//! spawn 顺序由调用方（main.rs）控制，函数本身不保证顺序。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::reload::ReloadCoordinator;
use crate::runtime::RuntimeHandles;
use osubot_core::api;
use osubot_core::irc::{IrcClient, IrcConfig as CoreIrcConfig};
use osubot_core::log_fmt;

/// 启动时一次性执行的用户 ID backfill（提取自 main.rs:3822-3858）。
pub(super) async fn backfill_user_ids(handles: &RuntimeHandles) {
    match handles.app_state.storage.get_users_without_ids().await {
        Ok(users) if !users.is_empty() => {
            info!("{}", log_fmt!("main.backfilling", count = users.len()));
            for username in &users {
                match api::get_user_info(
                    &handles.app_state.rate_limiter,
                    &handles.app_state.oauth,
                    username,
                )
                .await
                {
                    Ok(Some(info)) => {
                        if let Err(e) = handles
                            .app_state
                            .storage
                            .set_user_id(username, info.id)
                            .await
                        {
                            warn!(error = %e, "{}", log_fmt!("main.cache_user_id_failed"));
                        } else {
                            info!(
                                "{}",
                                log_fmt!("main.backfilled", username = username, user_id = info.id)
                            );
                        }
                    }
                    Ok(None) => {
                        warn!(
                            "{}",
                            log_fmt!("main.backfill_not_found", username = username)
                        );
                    }
                    Err(e) => {
                        warn!(
                            "{}",
                            log_fmt!(
                                "main.backfill_fetch_failed",
                                username = username,
                                error = &e
                            )
                        );
                    }
                }
            }
        }
        Ok(_) => info!("{}", log_fmt!("main.all_users_cached")),
        Err(e) => error!("{}", log_fmt!("main.backfill_query_failed", error = &e)),
    }
}

/// 启动 scheduler 任务（提取自 main.rs:3860-3870）。
pub(super) fn spawn_scheduler(handles: &RuntimeHandles) -> JoinHandle<()> {
    let scheduler = handles.app_state.scheduler.clone();
    tokio::spawn(async move {
        scheduler.run().await;
    })
}

/// 启动 IRC 客户端（提取自 main.rs:3895-3910）。
/// 凭据校验已在 runtime.rs::build_runtime_handles 中完成。
/// 返回外层 setup 任务的 JoinHandle；内部 IRC 客户端 JoinHandle 存入 `RuntimeHandles::irc_handle`。
pub(super) fn spawn_irc(handles: &RuntimeHandles) -> JoinHandle<()> {
    let irc_handle = handles.irc_handle.clone();
    let irc_tx = handles
        .irc_tx
        .clone()
        .expect("irc_tx set in runtime::build_runtime_handles");
    let config = handles.app_state.config.clone();

    tokio::spawn(async move {
        let cfg = config.read().await;
        if !cfg.irc.enabled {
            return;
        }

        let irc_config = CoreIrcConfig::new(
            cfg.irc.enabled,
            &cfg.irc.server,
            cfg.irc.port,
            &cfg.irc.nickname,
            &cfg.irc.password,
        );
        let irc_client = IrcClient::new(irc_config, irc_tx);
        *irc_handle.lock().await = Some(tokio::spawn(async move {
            if let Err(e) = irc_client.run().await {
                error!(error = %e, "{}", log_fmt!("main.irc_client_error"));
            }
        }));
    })
}

/// 启动 OneBot API pending 清理任务（提取自 main.rs:3920-3935）。
pub(super) fn spawn_onebot_cleanup(handles: &RuntimeHandles) -> JoinHandle<()> {
    let onebot_cleanup = handles.app_state.onebot_api.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            let mut pending = onebot_cleanup.pending.lock().await;
            let before = pending.len();
            pending.retain(|_, entry| entry.created_at.elapsed() < Duration::from_secs(30));
            let removed = before.saturating_sub(pending.len());
            if removed > 0 {
                tracing::warn!(removed, "{}", log_fmt!("main.cleanup_stale_pending"));
            }
        }
    })
}

/// 启动文件 watcher coordinator（从 runtime.rs 移来，原 main.rs:3988-3999）。
/// 同时确保插件目录存在。
pub(super) fn spawn_watcher(handles: &RuntimeHandles) -> JoinHandle<()> {
    let config = handles.app_state.config.clone();
    let reload_handle = handles.reload_handle.clone();
    tokio::spawn(async move {
        let cfg = config.read().await;
        let plugin_dir = PathBuf::from(&cfg.plugin.dir);
        drop(cfg);
        std::fs::create_dir_all(&plugin_dir).ok();

        let coordinator =
            ReloadCoordinator::new(reload_handle, PathBuf::from("osubot.toml"), plugin_dir);
        let watcher_join = coordinator.start();
        if let Err(e) = watcher_join.await {
            warn!("{}", log_fmt!("main.file_watcher_join_error", error = &e));
        }
        warn!("{}", log_fmt!("main.file_watcher_exited"));
    })
}

/// 启动 shutdown 信号监听（提取自 main.rs:4029-4035）。
pub(super) fn spawn_shutdown_signal(shutdown: Arc<AtomicBool>) {
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        let ctrl_c = async {
            tokio::signal::ctrl_c().await.ok();
        };
        #[cfg(unix)]
        let term = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(e) => {
                    error!("failed to install SIGTERM handler: {e}");
                }
            }
        };
        #[cfg(unix)]
        let hup = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
                Ok(mut s) => {
                    s.recv().await;
                }
                Err(e) => {
                    error!("failed to install SIGHUP handler: {e}");
                }
            }
        };
        #[cfg(unix)]
        tokio::select! {
            _ = ctrl_c => {}
            _ = term => {}
            _ = hup => {}
        }
        #[cfg(not(unix))]
        ctrl_c.await;
        info!("{}", log_fmt!("main.shutdown_signal"));
        shutdown_clone.store(true, Ordering::Release);
    });
}

/// 启动 IRC-to-WebSocket 桥接任务（提取自 main.rs:4008-4026）。
/// 消费 `RuntimeHandles` 中的 `irc_rx`。
pub(super) fn spawn_irc_bridge(handles: RuntimeHandles) -> JoinHandle<()> {
    let mut irc_rx = handles.irc_rx;
    let cw_for_irc = handles.app_state.current_write.clone();
    let storage = handles.app_state.storage.clone();
    let rate_limiter = handles.app_state.rate_limiter.clone();
    let oauth = handles.app_state.oauth.clone();

    tokio::spawn(async move {
        while let Some(irc_msg) = irc_rx.recv().await {
            let write_opt = { cw_for_irc.lock().await.clone() };
            if let Some(write) = write_opt {
                let storage = storage.clone();
                let rate_limiter = rate_limiter.clone();
                let oauth = oauth.clone();
                tokio::spawn(async move {
                    crate::handle_irc_message(storage, irc_msg, write, rate_limiter, oauth).await;
                });
            } else {
                warn!("{}", log_fmt!("main.no_ws_dropping_irc"));
            }
        }
    })
}
