use crate::instance::PluginInstance;
use crate::PluginManager;
use osubot_core::log_fmt;
use std::sync::atomic::Ordering;

impl PluginManager {
    /// Returns sorted indices of on_message plugins (priority descending, no instance taken).
    /// Brief `&self`, no `.await`.
    #[must_use]
    pub fn sorted_message_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self.on_message_indices.iter().copied().collect();
        indices.sort_by_key(|&i| {
            std::cmp::Reverse(self.slots.get(i).map(|s| s.params.priority).unwrap_or(0))
        });
        indices
    }

    /// Returns command-map indices for a command name (no instance taken).
    /// Brief `&self`, no `.await`.
    #[must_use]
    pub fn command_indices(&self, cmd_name: &str) -> Vec<usize> {
        self.command_map.get(cmd_name).cloned().unwrap_or_default()
    }

    /// Returns the instance params for a given index.
    /// Brief `&self`, no `.await`.
    pub fn instance_params(&self, idx: usize) -> Option<&crate::instance::PluginInstanceParams> {
        self.slots.get(idx).map(|s| &s.params)
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.instance.is_none())
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.instance.is_some()).count()
    }

    #[must_use]
    pub fn get_ticks(&self) -> Vec<(usize, u64, u32)> {
        let registry = self.tick_registry.lock().unwrap_or_else(|e| {
            tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
            e.into_inner()
        });
        registry
            .iter()
            .map(|t| (t.plugin_idx, t.interval_secs, t.tick_id))
            .collect()
    }

    /// 检查指定索引处是否存在有效实例。
    pub fn has_instance(&self, idx: usize) -> bool {
        self.slots.get(idx).is_some_and(|s| s.instance.is_some())
    }

    /// 检查指定 (plugin_idx, tick_id) 是否仍在 tick_registry 中注册。
    /// 用于 tick loop phase 2 验证 stale index（compact 可能已重映射索引）。
    pub fn has_tick(&self, plugin_idx: usize, tick_id: u32) -> bool {
        let registry = self.tick_registry.lock().unwrap_or_else(|e| e.into_inner());
        registry
            .iter()
            .any(|t| t.plugin_idx == plugin_idx && t.tick_id == tick_id)
    }

    /// 从指定槽位取出实例（不持锁时调用方负责同步）。
    /// 返回 None 表示槽位越界或为空。
    pub fn take_instance(&mut self, idx: usize) -> Option<PluginInstance> {
        self.slots.get_mut(idx).and_then(|s| s.instance.take())
    }

    /// 将实例放回指定槽位。如果槽位越界则静默丢弃。
    pub fn put_instance(&mut self, idx: usize, instance: PluginInstance) {
        if let Some(slot) = self.slots.get_mut(idx) {
            slot.instance = Some(instance);
        }
    }

    pub async fn handle_tick(&mut self, plugin_idx: usize, tick_id: u32) {
        let has = self
            .slots
            .get_mut(plugin_idx)
            .and_then(|s| s.instance.as_mut())
            .is_some_and(|i| i.has_export("on_tick"));
        if !has {
            return;
        }

        let name = self
            .slots
            .get(plugin_idx)
            .map_or("unknown", |s| s.params.name.as_str())
            .to_owned();

        let mut instance = match self.take_instance(plugin_idx) {
            Some(inst) => inst,
            None => return,
        };
        let timeout_dur = instance.timeout;

        let result = tokio::time::timeout(
            timeout_dur,
            tokio::task::spawn_blocking(move || {
                let r = instance.on_tick(tick_id);
                (r, instance)
            }),
        )
        .await;

        match result {
            Ok(Ok((Ok(()), instance))) => {
                self.put_instance(plugin_idx, instance);
            }
            Ok(Ok((Err(e), instance))) => {
                self.put_instance(plugin_idx, instance);
                self.record_exec_error(
                    plugin_idx,
                    &name,
                    "on_tick",
                    true,
                    "plugin.on_tick_consecutive_error",
                    None,
                );
                tracing::warn!(
                    "{}",
                    log_fmt!(
                        "plugin.on_tick_error",
                        kind = "on_tick",
                        name = &name,
                        error = e
                    )
                );
            }
            Ok(Err(join_err)) => {
                tracing::error!(
                    "{}",
                    log_fmt!(
                        "plugin.on_tick_panicked",
                        kind = "on_tick",
                        name = &name,
                        error = join_err
                    )
                );
                self.record_exec_error(
                    plugin_idx,
                    &name,
                    "on_tick",
                    true,
                    "plugin.on_tick_consecutive_panic",
                    None,
                );
            }
            Err(_) => {
                tracing::warn!(
                    "{}",
                    log_fmt!("plugin.on_tick_timeout", kind = "on_tick", name = &name)
                );
                self.engine.increment_epoch();
                self.record_exec_error(
                    plugin_idx,
                    &name,
                    "on_tick",
                    true,
                    "plugin.on_tick_consecutive_timeout",
                    Some("plugin.timeout_skip_reload"),
                );
            }
        }
    }

    pub(crate) async fn unload_single_instance(
        &mut self,
        idx: usize,
        context: &str,
        allow_reload: bool,
    ) {
        let has = self
            .slots
            .get_mut(idx)
            .and_then(|s| s.instance.as_mut())
            .is_some_and(|i| i.has_export("on_unload"));
        if !has {
            return;
        }

        let name = self
            .slots
            .get(idx)
            .map_or("unknown", |s| s.params.name.as_str())
            .to_owned();

        let mut instance = match self.take_instance(idx) {
            Some(inst) => inst,
            None => return,
        };
        let timeout_dur = instance.timeout;

        let result = tokio::time::timeout(
            timeout_dur,
            tokio::task::spawn_blocking(move || {
                let r = instance.on_unload();
                (r, instance)
            }),
        )
        .await;

        match result {
            Ok(Ok((Ok(()), instance))) => {
                self.put_instance(idx, instance);
            }
            Ok(Ok((Err(e), instance))) => {
                self.put_instance(idx, instance);
                self.record_exec_error(
                    idx,
                    &name,
                    "on_unload",
                    allow_reload,
                    "plugin.on_unload_consecutive_error",
                    None,
                );
                tracing::warn!(
                    "{}",
                    log_fmt!(
                        "plugin.on_unload_error",
                        kind = "on_unload",
                        name = &name,
                        error = e
                    )
                );
            }
            Ok(Err(join_err)) => {
                tracing::error!(
                    "{}",
                    log_fmt!(
                        "plugin.on_unload_panicked",
                        kind = "on_unload",
                        name = &name,
                        error = join_err
                    )
                );
                self.record_exec_error(
                    idx,
                    &name,
                    "on_unload",
                    allow_reload,
                    "plugin.on_unload_consecutive_panic",
                    None,
                );
            }
            Err(_) => {
                tracing::warn!(
                    "{}",
                    log_fmt!(
                        "plugin.on_unload_timeout",
                        kind = "on_unload",
                        name = &name,
                        context = context
                    )
                );
                self.engine.increment_epoch();
                self.record_exec_error(
                    idx,
                    &name,
                    "on_unload",
                    allow_reload,
                    "plugin.on_unload_consecutive_timeout",
                    Some("plugin.timeout_skip_reload"),
                );
            }
        }
    }

    pub async fn shutdown(&mut self) {
        self.epoch_running.store(false, Ordering::Relaxed);
        self.epoch_handle.abort();

        // 收集所有需要 unload 的实例及其元数据
        let mut tasks: Vec<(String, Option<PluginInstance>)> = Vec::with_capacity(self.slots.len());
        for idx in 0..self.slots.len() {
            let name = self
                .slots
                .get(idx)
                .map_or("unknown".to_string(), |s| s.params.name.clone());
            let has_unload = self
                .slots
                .get_mut(idx)
                .and_then(|s| s.instance.as_mut())
                .is_some_and(|i| i.has_export("on_unload"));
            let instance = if has_unload {
                self.slots.get_mut(idx).and_then(|s| s.instance.take())
            } else {
                None
            };
            tasks.push((name, instance));
        }

        // 并行执行所有 on_unload
        let unload_futures: Vec<_> = tasks
            .into_iter()
            .filter_map(|(name, instance)| {
                instance.map(|mut inst| {
                    let timeout_dur = inst.timeout;
                    tokio::spawn(async move {
                        let result = tokio::time::timeout(
                            timeout_dur,
                            tokio::task::spawn_blocking(move || inst.on_unload()),
                        )
                        .await;
                        (name, result)
                    })
                })
            })
            .collect();

        if !unload_futures.is_empty() {
            let results = futures_util::future::join_all(unload_futures).await;
            for task_result in results {
                match task_result {
                    Ok((name, result)) => match result {
                        Ok(Ok(Ok(()))) => {
                            tracing::info!("{}", log_fmt!("plugin.unloaded", name = name));
                        }
                        Ok(Ok(Err(e))) => {
                            tracing::warn!(
                                "{}",
                                log_fmt!(
                                    "plugin.on_unload_error",
                                    kind = "on_unload",
                                    name = name,
                                    error = e
                                )
                            );
                        }
                        Ok(Err(join_err)) => {
                            tracing::error!(
                                "{}",
                                log_fmt!(
                                    "plugin.on_unload_panicked",
                                    kind = "on_unload",
                                    name = name,
                                    error = join_err
                                )
                            );
                        }
                        Err(_) => {
                            tracing::warn!(
                                "{}",
                                log_fmt!(
                                    "plugin.on_unload_timeout",
                                    kind = "on_unload",
                                    name = name,
                                    context = "shutdown"
                                )
                            );
                            self.engine.increment_epoch();
                        }
                    },
                    Err(join_err) => {
                        tracing::error!(
                            "{}",
                            log_fmt!(
                                "plugin.on_unload_panicked",
                                kind = "on_unload",
                                name = "unknown",
                                error = join_err
                            )
                        );
                    }
                }
            }
        }

        self.slots.clear();
        self.on_message_indices.clear();
        self.command_map.clear();
        if let Ok(mut reg) = self.tick_registry.lock() {
            reg.clear();
        }
    }
}
