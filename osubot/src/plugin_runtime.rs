//! 插件运行时：PluginManager 初始化、tick 循环、断连/退出时清理。
//!
//! 每个 WS 连接创建一次 `PluginRuntime`：初始化 PluginManager（如启用）+ 启动 tick loop。
//! 断连时 `shutdown_for_reconnect` 关闭 plugin；SIGINT 时 `shutdown_all` 关闭。

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::app_state::AppState;
use crate::WriteSink;
use futures_util::SinkExt;
use osubot_plugin::{HostServices, PluginManager};

/// 插件运行时封装：持有 pm slot + drain/in_flight 协调句柄。
pub(super) struct PluginRuntime {
    pm: Arc<Mutex<Option<PluginManager>>>,
    drain: Arc<std::sync::atomic::AtomicBool>,
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

impl PluginRuntime {
    /// 初始化 PluginManager（如配置启用）并启动 plugin 消息出口。
    /// 提取自原 main.rs:4073-4145。
    pub(super) async fn new(
        state: &AppState,
        write: Arc<Mutex<WriteSink>>,
        drain: Arc<std::sync::atomic::AtomicBool>,
        in_flight: Arc<std::sync::atomic::AtomicUsize>,
        http_client: reqwest::Client,
        blocking_http_client: reqwest::blocking::Client,
    ) -> Self {
        let plugin_cfg = {
            let cfg = state.config.read().await;
            cfg.plugin.clone()
        };

        let new_pm = if plugin_cfg.instances.iter().any(|p| p.enabled) {
            let (plugin_tx, mut plugin_rx) = mpsc::channel::<(i64, serde_json::Value)>(256);

            let write_consumer = write.clone();
            tokio::spawn(async move {
                while let Some((group_id, message)) = plugin_rx.recv().await {
                    let json = serde_json::json!({
                        "action": "send_group_msg",
                        "params": {
                            "group_id": group_id,
                            "message": message
                        }
                    });
                    let mut sink = write_consumer.lock().await;
                    if let Err(e) = sink
                        .send(tokio_tungstenite::tungstenite::Message::Text(
                            json.to_string().into(),
                        ))
                        .await
                    {
                        tracing::debug!(
                            error = %e,
                            "{}",
                            osubot_core::log_fmt!("main.plugin_channel_closed")
                        );
                    }
                }
            });

            let msg_fn: Arc<dyn Fn(i64, serde_json::Value) -> Result<(), String> + Send + Sync> =
                Arc::new(move |group_id, message| {
                    plugin_tx.try_send((group_id, message)).map_err(|e| {
                        osubot_core::log_fmt!("main.plugin_msg_channel_busy", error = &e)
                            .to_string()
                    })
                });

            // 构造 HostServices（提取自 main.rs:4107-4122）
            let services = HostServices {
                http_client: http_client.clone(),
                blocking_http_client: blocking_http_client.clone(),
                rate_limiter: state.rate_limiter.clone(),
                oauth: state.oauth.clone(),
                storage: state.storage.clone(),
                send_msg_fn: msg_fn,
                runtime_handle: tokio::runtime::Handle::current(),
                instance_idx: 0,
                tick_registry: Arc::new(std::sync::Mutex::new(Vec::new())),
                tick_id_counter: Arc::new(std::sync::atomic::AtomicU32::new(0)),
                instance_config: None,
                limiter: osubot_plugin::StoreLimitsBuilder::new()
                    .memory_size(100 * 1024 * 1024)
                    .build(),
            };

            match PluginManager::new(&plugin_cfg, services).await {
                Ok(mgr) => {
                    info!(
                        "{}",
                        osubot_core::log_fmt!("main.plugin_manager_init", count = mgr.len())
                    );
                    Some(mgr)
                }
                Err(e) => {
                    warn!(
                        "{}",
                        osubot_core::log_fmt!("main.plugin_init_failed", error = &e)
                    );
                    None
                }
            }
        } else {
            None
        };

        // 更新共享 pm（与 coordinator 同 Arc，提取自 main.rs:4141-4145）
        {
            let mut guard = state.plugin_manager.lock().await;
            *guard = new_pm;
        }

        Self {
            pm: state.plugin_manager.clone(),
            drain,
            in_flight,
        }
    }

    /// 启动 tick loop（提取自 main.rs:4147-4273）。
    /// 两阶段 take → execute → put 模式，不持锁 await。
    pub(super) fn spawn_tick_loop(&self) -> JoinHandle<()> {
        let pm = self.pm.clone();
        let drain = self.drain.clone();
        let in_flight = self.in_flight.clone();

        tokio::spawn(async move {
            let mut last_fired: HashMap<(usize, u32), std::time::Instant> = HashMap::new();
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                // 热重载 drain 期间暂停 tick 分派，避免 phase 1 收集的索引
                // 在 phase 2 使用前被 reload_all()→compact() 重映射
                if drain.load(Ordering::SeqCst) {
                    continue;
                }
                let now = std::time::Instant::now();
                // 第一阶段：收集到期 tick（短暂持锁，读取后立即释放）
                let due_ticks: Vec<(usize, u32)> = {
                    let mut guard = pm.lock().await;
                    guard
                        .as_mut()
                        .map(|pm| {
                            let all_ticks = pm.get_ticks();
                            let valid_keys: std::collections::HashSet<_> =
                                all_ticks.iter().map(|(idx, _, tid)| (*idx, *tid)).collect();
                            last_fired.retain(|k, _| valid_keys.contains(k));
                            all_ticks
                                .into_iter()
                                .filter(|(idx, interval_secs, tid)| {
                                    let key = (*idx, *tid);
                                    last_fired.get(&key).is_none_or(|last| {
                                        now.duration_since(*last)
                                            >= Duration::from_secs(*interval_secs)
                                    })
                                })
                                .map(|(idx, _, tid)| (idx, tid))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                // 第二阶段：逐个触发 tick
                // 采用 take → execute（无锁）→ put 模式：
                // dispatch 内部有 spawn_blocking + timeout 的 .await 点，
                // 若在此期间持有 pm 锁，会阻塞消息分发和热重载。
                for (plugin_idx, tick_id) in due_ticks {
                    if drain.load(Ordering::SeqCst) {
                        break;
                    }

                    // 取出实例（短暂持锁），同时注册 in_flight 防止 reload_all()→compact()
                    // 在 spawn_blocking 执行期间重排索引导致 put_instance 用旧 idx 污染其他槽位。
                    // InFlightGuard 确保 wait_drain() 等待 tick 完成后再执行 compact()。
                    let (instance, _tick_guard) = {
                        let mut guard = pm.lock().await;
                        let tick_valid = guard.as_ref().is_some_and(|pm| {
                            pm.has_instance(plugin_idx) && pm.has_tick(plugin_idx, tick_id)
                        });
                        if !tick_valid {
                            continue;
                        }
                        let inst = guard.as_mut().and_then(|pm| pm.take_instance(plugin_idx));
                        if drain.load(Ordering::SeqCst) {
                            if let Some(inst) = inst {
                                if let Some(ref mut pm) = *guard {
                                    pm.put_instance(plugin_idx, inst);
                                }
                            }
                            break;
                        }
                        in_flight.fetch_add(1, Ordering::SeqCst);
                        let tick_guard = crate::InFlightGuard(in_flight.clone());
                        (inst, tick_guard)
                    };

                    let Some(mut inst) = instance else {
                        continue;
                    };

                    // 检查导出（不持锁）
                    if !inst.has_export("on_tick") {
                        let mut guard = pm.lock().await;
                        if let Some(ref mut pm) = *guard {
                            pm.put_instance(plugin_idx, inst);
                        }
                        last_fired.insert((plugin_idx, tick_id), now);
                        continue;
                    }

                    // 执行 tick（不持锁）
                    let timeout_dur = inst.timeout;
                    let result = tokio::time::timeout(
                        timeout_dur,
                        tokio::task::spawn_blocking(move || {
                            let res = inst.on_tick(tick_id);
                            (res, inst)
                        }),
                    )
                    .await;

                    // 处理结果并放回实例
                    {
                        let mut guard = pm.lock().await;
                        if let Some(ref mut pm) = *guard {
                            match result {
                                Ok(Ok((Ok(()), inst))) | Ok(Ok((Err(_), inst))) => {
                                    pm.put_instance(plugin_idx, inst);
                                }
                                Ok(Err(_)) | Err(_) => {
                                    let _ = pm.reload_instance(plugin_idx);
                                }
                            }
                        }
                    }

                    last_fired.insert((plugin_idx, tick_id), now);
                }
            }
        })
    }

    /// 断连时关闭 plugin（提取自 main.rs:4430-4436）。
    /// 关键修复：用 take() 把 Option 置 None，避免 shutdown 后的窗口期
    /// （reconnect_delay sleep 期间）state.plugin_manager 仍指向已 shutdown
    /// 的 manager，IRC 桥触发 send_msg_fn 走失效的 plugin_tx。
    pub(super) async fn shutdown_for_reconnect(&self) {
        let mut guard = self.pm.lock().await;
        if let Some(mut mgr) = guard.take() {
            mgr.shutdown().await;
        }
    }
}

/// SIGINT 时关闭所有 plugin（提取自 main.rs:4442-4448）。
pub(super) async fn shutdown_all(pm: &Arc<Mutex<Option<PluginManager>>>) {
    let mut guard = pm.lock().await;
    if let Some(ref mut mgr) = *guard {
        mgr.shutdown().await;
    }
}
