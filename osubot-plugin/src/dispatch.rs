use crate::instance::PluginInstance;
use crate::types::{PluginAction, PluginError};
use crate::PluginManager;
use osubot_core::log_fmt;
use std::sync::Arc;
use tokio::sync::Mutex;

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

impl PluginManager {}

/// Generic dispatch loop shared by `dispatch_command` and `dispatch_message`.
///
/// Iterates over indices returned by `select_indices` (called with a brief read
/// lock), takes the instance under a brief write lock, runs `exec_fn` inside
/// `spawn_blocking` with a timeout, then completes the execution under another
/// brief write lock. Returns the first non-`Next` action; falls through to
/// `PluginAction::Next` if no plugin handles the event.
///
/// `select_indices` is called synchronously under a read lock, so it does not
/// need to be `Send`. `exec_fn` runs on a blocking thread, so it must be
/// `Send + Sync + 'static`. The `payload` is owned by the helper and moved
/// into the blocking task.
async fn dispatch_loop<S, E>(
    pm: &Arc<Mutex<Option<PluginManager>>>,
    kind: &'static str,
    payload: String,
    select_indices: S,
    exec_fn: E,
) -> PluginAction
where
    S: Fn(&PluginManager) -> Vec<usize>,
    E: Fn(&mut PluginInstance, &str) -> Result<PluginAction, String>
        + Send
        + Sync
        + Clone
        + 'static,
{
    let indices = {
        let pm_guard = pm.lock().await;
        match pm_guard.as_ref() {
            Some(manager) => select_indices(manager),
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
        let payload_owned = payload.clone();
        let exec_fn = exec_fn.clone();
        let exec_result = tokio::time::timeout(
            timeout,
            tokio::task::spawn_blocking(move || {
                let r = exec_fn(&mut instance, &payload_owned);
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
                Some(manager) => manager.complete_exec(idx, &name, instance_opt, kind, wrapped),
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

impl PluginManager {
    /// 累加插件实例的连续错误计数；达到阈值时记录日志并根据 `allow_reload` 决定是否重载。
    /// `allow_reload` 为 true 时，达到阈值会尝试重载实例；为 false 时仅记录日志。
    /// `consecutive_key` 为达到阈值（触发重载）时的日志 key；
    /// `skip_reload_key` 若提供，则在未达到阈值时记录（用于超时场景）。
    pub fn record_exec_error(
        &mut self,
        idx: usize,
        name: &str,
        kind: &'static str,
        allow_reload: bool,
        consecutive_key: &'static str,
        skip_reload_key: Option<&'static str>,
    ) {
        let slot = match self.slots.get_mut(idx) {
            Some(s) => s,
            None => return,
        };
        slot.lost_instances = slot.lost_instances.saturating_add(1);
        let lost = slot.lost_instances;
        if lost >= self.lost_instances_threshold {
            tracing::warn!("{}", log_fmt!(consecutive_key, kind = kind, name = name));
            if allow_reload {
                if let Err(re) = self.reload_instance(idx) {
                    tracing::error!("{}", log_fmt!("plugin.reload_failed", error = re));
                }
            }
        } else if let Some(skip_key) = skip_reload_key {
            tracing::warn!("{}", log_fmt!(skip_key, kind = kind, name = name));
        }
    }

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
                self.record_exec_error(
                    idx,
                    name,
                    kind,
                    true,
                    "plugin.consecutive_error_reload",
                    None,
                );
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
                self.record_exec_error(
                    idx,
                    name,
                    kind,
                    true,
                    "plugin.consecutive_panic_reload",
                    None,
                );
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
                self.record_exec_error(
                    idx,
                    name,
                    kind,
                    true,
                    "plugin.consecutive_timeout_reload",
                    Some("plugin.timeout_skip_reload"),
                );
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
        dispatch_loop(
            pm,
            "command",
            cmd_json.to_owned(),
            |manager| manager.command_indices(cmd_name),
            |inst, payload| inst.on_command(payload),
        )
        .await
    }

    /// 调度消息到所有 on_message 插件，返回首个非 Next 动作。
    /// 走完整 `complete_exec` 路径（错误计数/阈值/重载）。
    /// 遍历顺序按 priority 降序（通过 `sorted_message_indices`）。
    /// 内部使用 brief locks：不在 `.await` 点持有 `plugin_manager` 锁。
    pub async fn dispatch_message(
        pm: &Arc<tokio::sync::Mutex<Option<PluginManager>>>,
        msg_json: &str,
    ) -> PluginAction {
        dispatch_loop(
            pm,
            "on_message",
            msg_json.to_owned(),
            |manager| manager.sorted_message_indices(),
            |inst, payload| inst.on_message(payload),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 编译期签名锁定：`record_exec_error` 的公开签名与本测试里的 `fn` 指针类型
    /// 必须完全一致。`osubot` crate 内的 tick 循环依赖此签名去累加错误计数。
    #[test]
    fn record_exec_error_signature_exists() {
        let _: fn(
            &mut PluginManager,
            usize,
            &str,
            &'static str,
            bool,
            &'static str,
            Option<&'static str>,
        ) = PluginManager::record_exec_error;
    }
}
