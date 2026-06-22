//! 进程启动：tracing 初始化 + 运行时句柄构建。
//!
//! 提取自原 main.rs:3758-3952 (tracing + config + storage + oauth + rate_limiter +
//! onebot_api_timeout + upstream_chain + ReloadHandle 构建)。
//! 文件 watcher 启动已移至 `background::spawn_watcher`（Task 3）。

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use tracing_subscriber::fmt::time::LocalTime;
use tracing_subscriber::{fmt, EnvFilter};

use crate::app_state::AppState;
use crate::config::Config;
use crate::reload::{ReloadHandle, ReloadHandleParams};
use crate::scheduler::Scheduler;
use osubot_core::irc::IrcPrivateMessage;
use osubot_core::{OauthTokenCache, RateLimiter, Storage};
use osubot_plugin::PluginManager;

/// Build 期中间结构：构造 `AppState` + ReloadHandle + 协作句柄。
/// `background::*` 任务从这里取出所需句柄。
/// 文件 watcher 由 `background::spawn_watcher` 启动（Task 3 移出）。
pub(super) struct RuntimeHandles {
    pub app_state: AppState,
    pub reload_handle: ReloadHandle,
    pub irc_handle: Arc<std::sync::Mutex<Option<JoinHandle<()>>>>,
    pub irc_tx: Option<mpsc::Sender<IrcPrivateMessage>>,
    pub irc_rx: mpsc::Receiver<IrcPrivateMessage>,
}

/// 初始化 tracing subscriber。提取自原 main.rs:3758-3774。
pub(super) fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("osubot=info,osubot_core=info,info"));

    let subscriber = fmt::Subscriber::builder()
        .with_env_filter(env_filter)
        .with_target(true)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .with_timer(LocalTime::new(
            time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
                .expect("valid time format"),
        ))
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");
}

/// 构建运行时所有句柄。
/// 严格按原 main.rs 顺序执行（除 backfill/watcher 启动）：
/// 1. config load
/// 2. storage init
/// 3. oauth + rate_limiter
/// 4. HTTP client warmup
/// 5. onebot_api_timeout
/// 6. upstream_chain
/// 7. onebot_api
/// 8. scheduler（构造在 ReloadHandle 之前，不需要双构造）
/// 9. OAuth 缺凭据警告
/// 10. IRC channel
/// 11. pm slot
/// 12. ReloadHandle
/// 13. AppState
///
/// 文件 watcher 与插件目录创建已移至 `background::spawn_watcher`（Task 3）。
pub(super) async fn build_runtime_handles() -> RuntimeHandles {
    let config = Config::from_path("osubot.toml").expect("Failed to load config");
    config.validate().expect("配置校验失败");
    let config = Arc::new(RwLock::new(config));

    info!(
        "{}",
        osubot_core::log_fmt!("main.onebot_url", url = &config.read().await.bot.onebot_url)
    );

    let db_path = config.read().await.database.path.clone();
    let storage = Arc::new(
        Storage::new(&db_path)
            .await
            .expect("Failed to open database"),
    );

    let (client_id, client_secret) = {
        let cfg = config.read().await;
        (cfg.osu.client_id.clone(), cfg.osu.client_secret.clone())
    };
    let oauth = Arc::new(OauthTokenCache::new(client_id, client_secret));

    let rate_limiter = Arc::new(RateLimiter::new());

    // Trigger lazy initialization of the shared reqwest HTTP client early, so any
    // build failure (e.g. missing TLS backend) is surfaced at startup rather than
    // crashing the process mid-flight on the first API call.
    let _ = osubot_core::api::http_client();

    let onebot_api_timeout = {
        let cfg = config.read().await;
        Arc::new(AtomicU64::new(cfg.bot.onebot_api_timeout_secs))
    };

    let upstream_chain = {
        let cfg = config.read().await;
        Arc::new(RwLock::new(crate::reload::build_upstream_chain(
            &cfg.upstream,
            &oauth,
            &rate_limiter,
        )))
    };

    let onebot_api = Arc::new(crate::OneBotApi::new(onebot_api_timeout.clone()));

    // Scheduler 构造在 ReloadHandle 之前（按自然顺序）
    let scheduler = Scheduler::new(
        storage.clone(),
        oauth.clone(),
        rate_limiter.clone(),
        config.clone(),
    );

    // OAuth 缺凭据警告（提取自 main.rs:3907-3912）
    {
        let cfg = config.read().await;
        if cfg.osu.client_secret.is_empty() || cfg.osu.client_secret == "your-client-secret-here" {
            warn!("{}", osubot_core::log_fmt!("main.oauth_not_configured"));
        }
    }

    // IRC 启用但未配置凭据则立即退出（提取自 main.rs:3884-3887）
    {
        let cfg = config.read().await;
        let irc_enabled = cfg.irc.enabled;
        if irc_enabled && (cfg.irc.nickname.is_empty() || cfg.irc.password.is_empty()) {
            eprintln!("IRC is enabled but nickname or password is not set in osubot.toml");
            std::process::exit(1);
        }
    }

    // IRC channel（提取自 main.rs:3872-3878）
    let (irc_tx, irc_rx) = mpsc::channel::<IrcPrivateMessage>(100);
    let irc_handle: Arc<std::sync::Mutex<Option<JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(None));

    // ReloadHandle + pm slot（提取自 main.rs:3939-3952）
    let pm: Arc<tokio::sync::Mutex<Option<PluginManager>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    let reload_handle = ReloadHandle::new(ReloadHandleParams {
        config: config.clone(),
        pm: pm.clone(),
        onebot_api_timeout: onebot_api_timeout.clone(),
        upstream_chain: upstream_chain.clone(),
        oauth: oauth.clone(),
        rate_limiter: rate_limiter.clone(),
        scheduler: scheduler.clone(),
        irc_handle: irc_handle.clone(),
        irc_tx: Some(irc_tx.clone()),
    });

    let app_state = AppState {
        config: config.clone(),
        storage: storage.clone(),
        oauth: oauth.clone(),
        rate_limiter: rate_limiter.clone(),
        upstream_chain: upstream_chain.clone(),
        onebot_api: onebot_api.clone(),
        scheduler: scheduler.clone(),
        current_write: Arc::new(Mutex::new(None)),
        plugin_manager: pm.clone(),
        user_rate_limits: Arc::new(dashmap::DashMap::new()),
        shutdown: Arc::new(AtomicBool::new(false)),
        force_reconnect: reload_handle.network.force_reconnect.clone(),
    };

    RuntimeHandles {
        app_state,
        reload_handle,
        irc_handle,
        irc_tx: Some(irc_tx),
        irc_rx,
    }
}
