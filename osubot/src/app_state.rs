//! 进程级共享状态（`AppState`）+ `BotContext` 派生。
//!
//! `AppState` 收纳 main() 当前持有的所有 Arc 句柄，构造一次后永久存活。
//! `BotContext` 字段定义仍保留在 main.rs 中；`for_dispatch` 从 `AppState` 派生。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{Mutex, RwLock};

use crate::config::Config;
use crate::scheduler::Scheduler;
use crate::WriteSink;
use osubot_core::{OauthTokenCache, RateLimiter, Storage, UpstreamChain};
use osubot_plugin::PluginManager;

/// WebSocket 写端：`Arc<Mutex<WriteSink>>`。
pub type WsWrite = Arc<Mutex<WriteSink>>;

/// 进程级共享状态。
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<Config>>,
    pub storage: Arc<Storage>,
    pub oauth: Arc<OauthTokenCache>,
    pub rate_limiter: Arc<RateLimiter>,
    pub sb_rate_limiter: Arc<RateLimiter>,
    pub upstream_chain: Arc<RwLock<UpstreamChain>>,
    pub onebot_api: Arc<crate::OneBotApi>,
    pub scheduler: Scheduler,
    pub current_write: Arc<Mutex<Option<WsWrite>>>,
    pub plugin_manager: Arc<Mutex<Option<PluginManager>>>,
    pub user_rate_limits: Arc<DashMap<i64, crate::UserRateLimit>>,
    pub shutdown: Arc<AtomicBool>,
    pub force_reconnect: Arc<AtomicBool>,
}
