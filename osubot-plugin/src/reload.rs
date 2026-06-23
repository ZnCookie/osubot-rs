use crate::config::PluginConfig as PluginConfigInput;
use crate::config::PluginInstanceConfig;
use crate::instance::{PluginInstance, PluginInstanceParams};
use crate::path::resolve_wasm_path;
use crate::types::OldPluginEntry;
use crate::PluginManager;
use crate::PluginSlot;
use crate::{HostServices, DEFAULT_PLUGIN_TIMEOUT_SECS, WASM_MEMORY_LIMIT};
use osubot_core::log_fmt;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tracing::{info, warn};
use wasmtime::StoreLimitsBuilder;
use wasmtime::{Module, Store};

struct PluginDiff<'a> {
    to_remove: Vec<usize>,
    to_add: Vec<&'a PluginInstanceConfig>,
    to_reload: Vec<&'a PluginInstanceConfig>,
    old_map: HashMap<String, OldPluginEntry>,
}

impl PluginManager {
    fn create_reload_store(
        &self,
        instance_idx: usize,
        instance_config: Option<serde_json::Value>,
    ) -> Store<HostServices> {
        let mut host_services = self.reload_template.clone();
        host_services.instance_idx = instance_idx;
        host_services.tick_registry = self.tick_registry.clone();
        host_services.instance_config = instance_config;
        host_services.limiter = StoreLimitsBuilder::new()
            .memory_size(WASM_MEMORY_LIMIT)
            .build();
        let mut store = Store::new(&self.engine, host_services);
        store.limiter(|state| &mut state.limiter);
        store
    }

    /// 从配置构建 PluginInstance，减少 to_add/to_reload 中的重复代码。
    fn build_instance(
        &self,
        idx: usize,
        pcfg: &PluginInstanceConfig,
        module: &Module,
    ) -> Result<(PluginInstance, PluginInstanceParams), String> {
        let params = PluginInstanceParams {
            name: pcfg.name.clone(),
            priority: pcfg.priority,
            plugin_config: pcfg.config.clone(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECS),
        };
        let store = self.create_reload_store(idx, pcfg.config.clone());
        let mut instance = PluginInstance::new(&self.linker, module, params.clone(), store)?;
        instance.set_instance_idx(idx);
        Ok((instance, params))
    }

    pub fn reload_instance(&mut self, idx: usize) -> Result<(), String> {
        // 连续重载失败保护：超过阈值则拒绝重载，需手动干预
        let slot = self
            .slots
            .get(idx)
            .ok_or_else(|| format!("slot not found for idx {idx}"))?;
        let consecutive = slot.reload_failures;
        if consecutive >= self.reload_failures_threshold {
            let name = &slot.params.name;
            warn!(
                "{}",
                log_fmt!(
                    "plugin.reload_too_many_times",
                    name = name,
                    consecutive = consecutive
                )
            );
            return Err(format!(
                "plugin reload failed too many times ({consecutive}), manual reload required"
            ));
        }
        let module = &slot.module;
        let params = &slot.params;

        // 原子操作：保存旧 tick 注册快照并清除旧注册（防止 TOCTOU 竞争）
        let old_ticks: Vec<_> = {
            let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                e.into_inner()
            });
            let snapshot: Vec<_> = registry
                .iter()
                .filter(|t| t.plugin_idx == idx)
                .cloned()
                .collect();
            registry.retain(|t| t.plugin_idx != idx);
            snapshot
        };

        let store = self.create_reload_store(idx, params.plugin_config.clone());
        let mut instance = match PluginInstance::new(&self.linker, module, params.clone(), store) {
            Ok(inst) => inst,
            Err(e) => {
                // new() 失败：仅在旧实例仍存在时恢复旧 tick 注册
                if self.slots[idx].instance.is_some() {
                    let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                        e.into_inner()
                    });
                    registry.extend(old_ticks);
                }
                self.slots[idx].reload_failures = self.slots[idx].reload_failures.saturating_add(1);
                return Err(e);
            }
        };
        instance.set_instance_idx(idx);

        if let Err(e) = instance.on_load() {
            // on_load 中可能已注册 tick，失败后原子清理残留并恢复旧 tick
            {
                let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                    tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                    e.into_inner()
                });
                registry.retain(|t| t.plugin_idx != idx);
                registry.extend(old_ticks);
            }
            // 旧实例仍然有效，不清理 command_map / on_message_indices
            self.slots[idx].reload_failures = self.slots[idx].reload_failures.saturating_add(1);
            return Err(e);
        }

        // 成功路径：更新 on_message 索引并重建 command_map
        if instance.has_export("on_message") {
            self.on_message_indices.insert(idx);
        } else {
            self.on_message_indices.remove(&idx);
        }

        for indices in self.command_map.values_mut() {
            indices.retain(|i| *i != idx);
        }
        let metadata = instance.metadata();
        for cmd in metadata.commands.iter() {
            self.command_map.entry(cmd.clone()).or_default().push(idx);
        }
        for indices in self.command_map.values_mut() {
            indices.sort_by_key(|&i| {
                std::cmp::Reverse(self.slots.get(i).map(|s| s.params.priority).unwrap_or(0))
            });
        }

        self.slots[idx].instance = Some(instance);
        self.slots[idx].lost_instances = 0;
        self.slots[idx].reload_failures = 0;
        Ok(())
    }

    fn diff_plugins<'a>(&self, new_config: &'a PluginConfigInput) -> PluginDiff<'a> {
        let mut old_map: HashMap<String, OldPluginEntry> = HashMap::new();
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.instance.is_some() {
                old_map.insert(
                    slot.params.name.clone(),
                    OldPluginEntry {
                        idx,
                        wasm_path: slot.wasm_path.clone(),
                        priority: slot.params.priority,
                        plugin_config: slot.params.plugin_config.clone(),
                        wasm_mtime: slot.wasm_mtime,
                    },
                );
            }
        }

        let new_enabled: Vec<&PluginInstanceConfig> =
            new_config.instances.iter().filter(|p| p.enabled).collect();

        let new_names: HashSet<&str> = new_enabled.iter().map(|p| p.name.as_str()).collect();
        let old_names: HashSet<&str> = old_map.keys().map(|s| s.as_str()).collect();

        let mut to_remove: Vec<usize> = Vec::new();
        for (name, entry) in &old_map {
            if !new_names.contains(name.as_str()) {
                to_remove.push(entry.idx);
            }
        }
        to_remove.sort_unstable();
        to_remove.dedup();

        let to_add: Vec<&PluginInstanceConfig> = new_enabled
            .iter()
            .filter(|p| !old_names.contains(p.name.as_str()))
            .copied()
            .collect();

        let to_reload: Vec<&PluginInstanceConfig> = new_enabled
            .iter()
            .filter(|p| {
                if let Some(entry) = old_map.get(&p.name) {
                    let new_path = match resolve_wasm_path(&new_config.dir, &p.path) {
                        Ok(p) => p,
                        Err(_) => return false,
                    };
                    let current_mtime = std::fs::metadata(&new_path)
                        .ok()
                        .and_then(|m| m.modified().ok());
                    p.priority != entry.priority
                        || new_path != entry.wasm_path
                        || p.config != entry.plugin_config
                        || entry.wasm_mtime != current_mtime
                } else {
                    false
                }
            })
            .copied()
            .collect();

        PluginDiff {
            to_remove,
            to_add,
            to_reload,
            old_map,
        }
    }

    async fn apply_removals(
        &mut self,
        to_remove: &[usize],
        old_map: &HashMap<String, OldPluginEntry>,
    ) {
        for idx in to_remove.iter().rev() {
            // 不让 on_unload 失败嵌套触发 reload_instance：
            // apply_removals 负责完整移除流程，on_unload 仅做清理。
            self.unload_single_instance(*idx, "remove", false).await;

            for indices in self.command_map.values_mut() {
                indices.retain(|i| *i != *idx);
            }
            self.on_message_indices.remove(idx);

            {
                let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                    tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                    e.into_inner()
                });
                registry.retain(|t| t.plugin_idx != *idx);
            }

            if let Some(slot) = self.slots.get_mut(*idx) {
                slot.instance = None;
            }
            let removed_name = old_map
                .iter()
                .find(|(_, entry)| entry.idx == *idx)
                .map(|(n, _)| n.as_str())
                .unwrap_or("unknown");
            info!("{}", log_fmt!("plugin.removed", name = removed_name));
        }
    }

    async fn apply_adds(&mut self, to_add: &[&PluginInstanceConfig], dir: &str) {
        for pcfg in to_add {
            let wasm_path = match resolve_wasm_path(dir, &pcfg.path) {
                Ok(p) => p,
                Err(e) => {
                    warn!("{}", log_fmt!("plugin.invalid_path", error = e));
                    continue;
                }
            };

            let module = match Module::from_file(&self.engine, &wasm_path) {
                Ok(m) => m,
                Err(e) => {
                    warn!("{}", log_fmt!("plugin.compile_failed", error = e));
                    continue;
                }
            };

            let sorted_idx = self.slots.len();
            let (mut instance, params) = match self.build_instance(sorted_idx, pcfg, &module) {
                Ok(v) => v,
                Err(e) => {
                    warn!("{}", log_fmt!("plugin.instance_build_failed", error = e));
                    continue;
                }
            };

            if let Err(e) = instance.on_load() {
                warn!("{}", log_fmt!("plugin.on_load_failed", error = e));
                {
                    let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                        e.into_inner()
                    });
                    registry.retain(|t| t.plugin_idx != sorted_idx);
                }
                continue;
            }

            let metadata = instance.metadata();
            for cmd in metadata.commands.iter() {
                self.command_map
                    .entry(cmd.clone())
                    .or_default()
                    .push(sorted_idx);
            }
            if instance.has_export("on_message") {
                self.on_message_indices.insert(sorted_idx);
            }

            let wasm_mtime = std::fs::metadata(&wasm_path)
                .ok()
                .and_then(|m| m.modified().ok());
            self.slots.push(PluginSlot {
                instance: Some(instance),
                module,
                params,
                wasm_path,
                wasm_mtime,
                lost_instances: 0,
                reload_failures: 0,
            });

            info!("{}", log_fmt!("plugin.added", name = &pcfg.name));
        }
    }

    async fn apply_reloads(
        &mut self,
        to_reload: &[&PluginInstanceConfig],
        dir: &str,
        old_map: &HashMap<String, OldPluginEntry>,
    ) {
        for pcfg in to_reload {
            if let Some(entry) = old_map.get(&pcfg.name) {
                let idx = entry.idx;
                // 阈值检查前移：超限直接跳过整个 reload 流程，避免新 instance
                // 被 build 又被 drop、ticks 被清空但 command_map 残留的死锁状态。
                if self.slots[idx].reload_failures >= self.reload_failures_threshold {
                    warn!("{}", log_fmt!("plugin.reload_too_many_times_all"));
                    continue;
                }

                let wasm_path = match resolve_wasm_path(dir, &pcfg.path) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("{}", log_fmt!("plugin.reload_invalid_path", error = e));
                        continue;
                    }
                };

                let module = match Module::from_file(&self.engine, &wasm_path) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("{}", log_fmt!("plugin.reload_compile_failed", error = e));
                        continue;
                    }
                };

                let (mut instance, params) = match self.build_instance(idx, pcfg, &module) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("{}", log_fmt!("plugin.reload_instance_failed", error = e));
                        self.slots[idx].reload_failures =
                            self.slots[idx].reload_failures.saturating_add(1);
                        continue;
                    }
                };

                // 反转顺序：先 snapshot + 清空旧 ticks，再 on_load 新实例。
                // 这样若新实例 on_load 失败，旧实例从未 on_unload，
                // slot 中仍然保留旧实例（僵尸防护）。若先 unload 再 load，
                // 一旦 load 失败则旧实例资源已被释放，slot 仍指向它形成僵尸。

                // 1. 先 snapshot 旧 ticks（不删）。
                let old_ticks: Vec<_> = {
                    let registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                        e.into_inner()
                    });
                    registry
                        .iter()
                        .filter(|t| t.plugin_idx == idx)
                        .cloned()
                        .collect()
                };

                // 2. 清除旧 tick 注册（防止新 on_load 期间旧 tick 重复触发）。
                {
                    let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                        e.into_inner()
                    });
                    registry.retain(|t| t.plugin_idx != idx);
                }

                // 3. on_load 新实例：若失败，恢复旧 ticks，新实例直接 drop，
                //    旧实例保留在 slot 中（未 on_unload）。
                if let Err(e) = instance.on_load() {
                    warn!("{}", log_fmt!("plugin.on_load_failed", error = e));
                    {
                        let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                            tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                            e.into_inner()
                        });
                        // 清理 on_load 失败时可能已注册的部分 tick，避免与 old_ticks
                        // 合并后超过 MAX_TICKS_PER_PLUGIN。
                        registry.retain(|t| t.plugin_idx != idx);
                        registry.extend(old_ticks);
                    }
                    self.slots[idx].reload_failures =
                        self.slots[idx].reload_failures.saturating_add(1);
                    continue;
                }

                // 4. on_load 成功后才 on_unload 旧实例（allow_reload=false 避免嵌套触发 reload_instance）。
                self.unload_single_instance(idx, "reload", false).await;

                for indices in self.command_map.values_mut() {
                    indices.retain(|i| *i != idx);
                }
                self.on_message_indices.remove(&idx);

                let metadata = instance.metadata();
                for cmd in metadata.commands.iter() {
                    self.command_map.entry(cmd.clone()).or_default().push(idx);
                }
                if instance.has_export("on_message") {
                    self.on_message_indices.insert(idx);
                }

                let wasm_mtime = std::fs::metadata(&wasm_path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                let slot = &mut self.slots[idx];
                slot.instance = Some(instance);
                slot.module = module;
                slot.params = params;
                slot.lost_instances = 0;
                slot.reload_failures = 0;
                slot.wasm_mtime = wasm_mtime;
                slot.wasm_path = wasm_path;

                info!("{}", log_fmt!("plugin.reloaded", name = &pcfg.name));
            }
        }
    }

    fn reserialize_command_map(&mut self) {
        for indices in self.command_map.values_mut() {
            indices.sort_by_key(|&i| {
                std::cmp::Reverse(self.slots.get(i).map(|s| s.params.priority).unwrap_or(0))
            });
        }
    }

    pub async fn reload_all(&mut self, new_config: &PluginConfigInput) -> Result<(), String> {
        let PluginDiff {
            to_remove,
            to_add,
            to_reload,
            old_map,
        } = self.diff_plugins(new_config);

        info!(
            "{}",
            log_fmt!(
                "plugin.diff_complete",
                to_add = to_add.len(),
                to_remove = to_remove.len(),
                to_reload = to_reload.len()
            )
        );

        self.apply_removals(&to_remove, &old_map).await;
        self.apply_adds(&to_add, &new_config.dir).await;
        self.apply_reloads(&to_reload, &new_config.dir, &old_map)
            .await;

        self.reserialize_command_map();
        self.compact();
        Ok(())
    }

    pub(crate) fn compact(&mut self) {
        let old_len = self.slots.len();
        if old_len == 0 {
            return;
        }

        let mut old_to_new: Vec<Option<usize>> = vec![None; old_len];
        let mut new_len = 0usize;
        for (old, slot) in self.slots.iter().enumerate() {
            if slot.instance.is_some() {
                old_to_new[old] = Some(new_len);
                new_len += 1;
            }
        }

        if new_len == old_len {
            return;
        }

        // 单次 retain 即可：slot 自带全部 7 个并行字段，无需跨 Vec 同步。
        let mut keep = old_to_new.iter().map(Option::is_some);
        self.slots.retain(|_| keep.next().unwrap_or(false));

        for indices in self.command_map.values_mut() {
            let mut retained = Vec::with_capacity(indices.len());
            for &old_idx in indices.iter() {
                if let Some(Some(new_idx)) = old_to_new.get(old_idx) {
                    retained.push(*new_idx);
                }
            }
            *indices = retained;
        }

        for indices in self.command_map.values_mut() {
            indices.sort_by_key(|&i| {
                std::cmp::Reverse(self.slots.get(i).map(|s| s.params.priority).unwrap_or(0))
            });
        }

        // Remove empty entries from command_map
        self.command_map.retain(|_, indices| !indices.is_empty());

        let old_msg = std::mem::take(&mut self.on_message_indices);
        self.on_message_indices = old_msg
            .into_iter()
            .filter_map(|old| old_to_new.get(old).copied().flatten())
            .collect();

        {
            let mut reg = self.tick_registry.lock().unwrap_or_else(|e| {
                tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                e.into_inner()
            });
            for entry in reg.iter_mut() {
                if let Some(new_idx) = old_to_new.get(entry.plugin_idx).and_then(|x| *x) {
                    entry.plugin_idx = new_idx;
                }
            }
        }

        for (new_idx, slot) in self.slots.iter_mut().enumerate() {
            if let Some(inst) = slot.instance.as_mut() {
                inst.set_instance_idx(new_idx);
            }
        }

        info!(
            "{}",
            log_fmt!(
                "plugin.manager_compacted",
                old_len = old_len,
                new_len = new_len,
                removed = old_len - new_len
            )
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::config::PluginInstanceConfig;
    use crate::instance::{PluginInstance, PluginInstanceParams};
    use crate::PluginManager;
    use wasmtime::Module;

    /// Pin 公共 API：apply_reloads 顺序在 osubot-plugin/src/reload.rs 注释中固化（先
    /// on_load 新实例再 on_unload 旧实例）。集成层走 ReloadCoordinator 端到端验证。
    /// 这里用函数指针断言锁定 build_instance 签名，防止未来重构改其行为。
    #[test]
    fn build_instance_signature_locked() {
        let _: fn(
            &PluginManager,
            usize,
            &PluginInstanceConfig,
            &Module,
        ) -> Result<(PluginInstance, PluginInstanceParams), String> = PluginManager::build_instance;
    }
}
