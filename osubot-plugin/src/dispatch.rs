use crate::instance::PluginInstance;
use crate::types::{PluginAction, PluginError};
use crate::PluginManager;
use osubot_core::log_fmt;
use std::sync::Arc;

/// Result of plugin execution (no timeout/panic wrapping).
pub enum PluginDispatchResult<T> {
    Ok(T),
    PluginError(PluginError),
}

/// Wrapping error for spawn_blocking panic or timeout.
pub enum PluginDispatchPanic {
    Panic(tokio::task::JoinError),
    Timeout,
}

impl PluginManager {
    /// Process a completed plugin execution. Puts the instance back and handles error counting.
    /// Brief `&mut self`, no `.await`.
    /// Returns the plugin action for the caller to evaluate.
    pub fn complete_exec(
        &mut self,
        idx: usize,
        name: &str,
        instance: Option<PluginInstance>,
        kind: &'static str,
        result: Result<PluginDispatchResult<PluginAction>, PluginDispatchPanic>,
    ) -> PluginAction {
        if let Some(inst) = instance {
            self.put_instance(idx, inst);
        }
        match result {
            Ok(PluginDispatchResult::Ok(PluginAction::Handled(msg))) => PluginAction::Handled(msg),
            Ok(PluginDispatchResult::Ok(PluginAction::Intercepted)) => PluginAction::Intercepted,
            Ok(PluginDispatchResult::Ok(PluginAction::Next)) => PluginAction::Next,
            Ok(PluginDispatchResult::PluginError(e)) => {
                self.lost_instances[idx] = self.lost_instances[idx].saturating_add(1);
                if self.lost_instances[idx] >= self.lost_instances_threshold {
                    tracing::warn!(
                        "{}",
                        log_fmt!("plugin.consecutive_error_reload", kind = kind, name = name)
                    );
                    if let Err(re) = self.reload_instance(idx) {
                        tracing::error!("{}", log_fmt!("plugin.reload_failed", error = re));
                    }
                }
                tracing::warn!(
                    "{}",
                    log_fmt!("plugin.error", kind = kind, name = name, error = e)
                );
                PluginAction::Next
            }
            Err(PluginDispatchPanic::Panic(join_err)) => {
                tracing::error!(
                    "{}",
                    log_fmt!(
                        "plugin.panicked",
                        kind = kind,
                        name = name,
                        error = join_err
                    )
                );
                self.lost_instances[idx] = self.lost_instances[idx].saturating_add(1);
                if self.lost_instances[idx] >= self.lost_instances_threshold {
                    tracing::warn!(
                        "{}",
                        log_fmt!("plugin.consecutive_panic_reload", kind = kind, name = name)
                    );
                    if let Err(e) = self.reload_instance(idx) {
                        tracing::error!("{}", log_fmt!("plugin.reload_failed", error = e));
                    }
                }
                PluginAction::Next
            }
            Err(PluginDispatchPanic::Timeout) => {
                tracing::warn!(
                    "{}",
                    log_fmt!(
                        "plugin.timeout",
                        kind = kind,
                        name = name,
                        error = "timeout"
                    )
                );
                self.engine.increment_epoch();
                self.lost_instances[idx] = self.lost_instances[idx].saturating_add(1);
                if self.lost_instances[idx] >= self.lost_instances_threshold {
                    tracing::warn!(
                        "{}",
                        log_fmt!(
                            "plugin.consecutive_timeout_reload",
                            kind = kind,
                            name = name
                        )
                    );
                    if let Err(re) = self.reload_instance(idx) {
                        tracing::error!("{}", log_fmt!("plugin.reload_failed", error = re));
                    }
                } else {
                    tracing::warn!(
                        "{}",
                        log_fmt!("plugin.timeout_skip_reload", kind = kind, name = name)
                    );
                }
                PluginAction::Next
            }
        }
    }

    /// 调度命令到所有注册了 `cmd_name` 的插件，返回首个非 Next 动作。
    /// 走完整 `complete_exec` 路径（错误计数/阈值/重载）。
    /// 内部使用 brief locks：不在 `.await` 点持有 `plugin_manager` 锁。
    pub async fn dispatch_command(
        pm: &Arc<tokio::sync::Mutex<Option<PluginManager>>>,
        cmd_name: &str,
        cmd_json: &str,
    ) -> PluginAction {
        let indices = {
            let pm_guard = pm.lock().await;
            match pm_guard.as_ref() {
                Some(manager) => manager.command_indices(cmd_name),
                None => return PluginAction::Next,
            }
        };

        for idx in indices {
            // Phase 1: brief lock to take instance
            let (mut instance, name, timeout) = {
                let mut pm_guard = pm.lock().await;
                match pm_guard.as_mut().and_then(|manager| {
                    let inst = manager.take_instance(idx)?;
                    let params = manager.instance_params(idx)?;
                    Some((inst, params.name.clone(), params.timeout))
                }) {
                    Some(v) => v,
                    None => continue,
                }
            }; // lock dropped

            // Phase 2: no lock held during spawn_blocking + timeout
            let payload = cmd_json.to_owned();
            let exec_result = tokio::time::timeout(
                timeout,
                tokio::task::spawn_blocking(move || {
                    let r = instance.on_command(&payload);
                    (r, instance)
                }),
            )
            .await;
            let (wrapped, instance_opt) = match exec_result {
                Ok(Ok((Ok(a), inst))) => (Ok(PluginDispatchResult::Ok(a)), Some(inst)),
                Ok(Ok((Err(e), inst))) => (
                    Ok(PluginDispatchResult::PluginError(PluginError::Dispatch(e))),
                    Some(inst),
                ),
                Ok(Err(join_err)) => (Err(PluginDispatchPanic::Panic(join_err)), None),
                Err(_) => (Err(PluginDispatchPanic::Timeout), None),
            };

            // Phase 3: brief lock to complete
            let action = {
                let mut pm_guard = pm.lock().await;
                match pm_guard.as_mut() {
                    Some(manager) => {
                        manager.complete_exec(idx, &name, instance_opt, "command", wrapped)
                    }
                    None => PluginAction::Next,
                }
            }; // lock dropped

            match action {
                PluginAction::Handled(_) | PluginAction::Intercepted => return action,
                PluginAction::Next => continue,
            }
        }
        PluginAction::Next
    }

    /// 调度消息到所有 on_message 插件，返回首个非 Next 动作。
    /// 走完整 `complete_exec` 路径（错误计数/阈值/重载）。
    /// 遍历顺序按 priority 降序（通过 `sorted_message_indices`）。
    /// 内部使用 brief locks：不在 `.await` 点持有 `plugin_manager` 锁。
    pub async fn dispatch_message(
        pm: &Arc<tokio::sync::Mutex<Option<PluginManager>>>,
        msg_json: &str,
    ) -> PluginAction {
        let indices = {
            let pm_guard = pm.lock().await;
            match pm_guard.as_ref() {
                Some(manager) => manager.sorted_message_indices(),
                None => return PluginAction::Next,
            }
        };

        for idx in indices {
            let (mut instance, name, timeout) = {
                let mut pm_guard = pm.lock().await;
                match pm_guard.as_mut().and_then(|manager| {
                    let inst = manager.take_instance(idx)?;
                    let params = manager.instance_params(idx)?;
                    Some((inst, params.name.clone(), params.timeout))
                }) {
                    Some(v) => v,
                    None => continue,
                }
            };

            let payload = msg_json.to_owned();
            let exec_result = tokio::time::timeout(
                timeout,
                tokio::task::spawn_blocking(move || {
                    let r = instance.on_message(&payload);
                    (r, instance)
                }),
            )
            .await;
            let (wrapped, instance_opt) = match exec_result {
                Ok(Ok((Ok(a), inst))) => (Ok(PluginDispatchResult::Ok(a)), Some(inst)),
                Ok(Ok((Err(e), inst))) => (
                    Ok(PluginDispatchResult::PluginError(PluginError::Dispatch(e))),
                    Some(inst),
                ),
                Ok(Err(join_err)) => (Err(PluginDispatchPanic::Panic(join_err)), None),
                Err(_) => (Err(PluginDispatchPanic::Timeout), None),
            };

            let action = {
                let mut pm_guard = pm.lock().await;
                match pm_guard.as_mut() {
                    Some(manager) => {
                        manager.complete_exec(idx, &name, instance_opt, "on_message", wrapped)
                    }
                    None => PluginAction::Next,
                }
            };

            match action {
                PluginAction::Handled(_) | PluginAction::Intercepted => return action,
                PluginAction::Next => continue,
            }
        }
        PluginAction::Next
    }
}
