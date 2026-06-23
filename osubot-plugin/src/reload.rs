use crate::config::PluginConfig as PluginConfigInput;
use crate::config::PluginInstanceConfig;
use crate::instance::{PluginInstance, PluginInstanceParams};
use crate::path::resolve_wasm_path;
use crate::types::OldPluginEntry;
use crate::PluginManager;
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
        let consecutive = self.reload_failures.get(idx).copied().unwrap_or(0);
        if consecutive >= self.reload_failures_threshold {
            let name = self
                .instance_params
                .get(idx)
                .map_or("unknown", |p| p.name.as_str());
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

        let module = self
            .modules
            .get(idx)
            .ok_or_else(|| format!("module not found for idx {idx}"))?;
        let params = self
            .instance_params
            .get(idx)
            .ok_or_else(|| format!("params not found for idx {idx}"))?;

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
                if self.instances[idx].is_some() {
                    let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                        e.into_inner()
                    });
                    registry.extend(old_ticks);
                }
                self.reload_failures[idx] = self.reload_failures[idx].saturating_add(1);
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
            self.reload_failures[idx] = self.reload_failures[idx].saturating_add(1);
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
                std::cmp::Reverse(self.instance_params.get(i).map(|p| p.priority).unwrap_or(0))
            });
        }

        self.instances[idx] = Some(instance);
        self.lost_instances[idx] = 0;
        self.reload_failures[idx] = 0;
        Ok(())
    }

    fn diff_plugins<'a>(&self, new_config: &'a PluginConfigInput) -> PluginDiff<'a> {
        let mut old_map: HashMap<String, OldPluginEntry> = HashMap::new();
        for (idx, params) in self.instance_params.iter().enumerate() {
            if self.instances[idx].is_some() {
                old_map.insert(
                    params.name.clone(),
                    OldPluginEntry {
                        idx,
                        wasm_path: self.wasm_paths.get(idx).cloned().unwrap_or_default(),
                        priority: params.priority,
                        plugin_config: params.plugin_config.clone(),
                        wasm_mtime: self.wasm_mtimes.get(idx).copied().flatten(),
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

            self.instances[*idx] = None;
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

            let sorted_idx = self.instances.len();
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

            self.instances.push(Some(instance));
            self.modules.push(module);
            self.instance_params.push(params);
            self.lost_instances.push(0);
            self.reload_failures.push(0);
            let wasm_mtime = std::fs::metadata(&wasm_path)
                .ok()
                .and_then(|m| m.modified().ok());
            self.wasm_mtimes.push(wasm_mtime);
            self.wasm_paths.push(wasm_path);

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
                if self.reload_failures[idx] >= self.reload_failures_threshold {
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
                        self.reload_failures[idx] = self.reload_failures[idx].saturating_add(1);
                        continue;
                    }
                };

                // 不让 on_unload 失败嵌套触发 reload_instance：
                // apply_reloads 会自己 build+on_load 完整 instance，nested trigger
                // 会浪费一次 build+on_load 并重复 increment reload_failures。
                self.unload_single_instance(idx, "reload", false).await;

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

                {
                    let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                        e.into_inner()
                    });
                    registry.retain(|t| t.plugin_idx != idx);
                }

                if let Err(e) = instance.on_load() {
                    warn!("{}", log_fmt!("plugin.on_load_failed", error = e));
                    {
                        let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                            tracing::warn!("{}", log_fmt!("plugin.tick_registry_poisoned"));
                            e.into_inner()
                        });
                        registry.retain(|t| t.plugin_idx != idx);
                        registry.extend(old_ticks);
                    }
                    self.reload_failures[idx] = self.reload_failures[idx].saturating_add(1);
                    continue;
                }

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

                self.instances[idx] = Some(instance);
                self.modules[idx] = module;
                self.instance_params[idx] = params;
                self.lost_instances[idx] = 0;
                self.reload_failures[idx] = 0;
                let wasm_mtime = std::fs::metadata(&wasm_path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                self.wasm_mtimes[idx] = wasm_mtime;
                self.wasm_paths[idx] = wasm_path;

                info!("{}", log_fmt!("plugin.reloaded", name = &pcfg.name));
            }
        }
    }

    fn reserialize_command_map(&mut self) {
        for indices in self.command_map.values_mut() {
            indices.sort_by_key(|&i| {
                std::cmp::Reverse(self.instance_params.get(i).map(|p| p.priority).unwrap_or(0))
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
        let old_len = self.instances.len();
        if old_len == 0 {
            return;
        }

        let mut old_to_new: Vec<Option<usize>> = vec![None; old_len];
        let mut new_len = 0usize;
        for (old, inst) in self.instances.iter().enumerate() {
            if inst.is_some() {
                old_to_new[old] = Some(new_len);
                new_len += 1;
            }
        }

        if new_len == old_len {
            return;
        }

        // 按 old_to_new 掩码同步裁剪 7 个并行 Vec。
        fn filter_by_mask<T>(v: &mut Vec<T>, mask: &[Option<usize>]) {
            let mut keep = mask.iter().map(Option::is_some);
            v.retain(|_| keep.next().unwrap_or(false));
        }
        filter_by_mask(&mut self.instances, &old_to_new);
        filter_by_mask(&mut self.modules, &old_to_new);
        filter_by_mask(&mut self.instance_params, &old_to_new);
        filter_by_mask(&mut self.wasm_paths, &old_to_new);
        filter_by_mask(&mut self.wasm_mtimes, &old_to_new);
        filter_by_mask(&mut self.lost_instances, &old_to_new);
        filter_by_mask(&mut self.reload_failures, &old_to_new);

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
                std::cmp::Reverse(self.instance_params.get(i).map(|p| p.priority).unwrap_or(0))
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

        for (new_idx, inst_opt) in self.instances.iter_mut().enumerate() {
            if let Some(inst) = inst_opt {
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
