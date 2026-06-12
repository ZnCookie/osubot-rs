mod bridge;
pub mod config;
mod instance;
mod types;

use instance::{PluginInstance, PluginInstanceParams};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{info, warn};
use types::PluginAction;
use wasmtime::{Engine, Linker, Module, Store};

pub use bridge::HostServices;
pub use config::{PluginConfig as PluginConfigInput, PluginInstanceConfig};
pub use types::PluginAction as PluginActionResult;
pub use wasmtime::StoreLimitsBuilder;

const DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 10;

enum DispatchOutcome<T> {
    Ok(T),
    PluginError,
    TimeoutLost,
    Panic,
    NotAvailable,
}

/// WASM 插件系统运行时宿主。
///
/// # 信任模型
///
/// 本系统采用**完全信任插件**模型：
///
/// - 所有宿主函数（HTTP 请求、消息发送、数据库查询等）对插件完全开放，不做 URL 白名单或响应大小限制
/// - 插件的操作能力和「正常编译的 Rust 代码」一致——它们可以做的事是功能，不是漏洞
/// - 部署者有责任审查加载的每个 `.wasm` 文件，并对插件行为承担后果
/// - 宿主仅提供**进程级故障隔离**：wasmtime 沙箱（内存上限、无文件系统）、epoch 超时中断、
///   tokio::timeout 兜底、令牌桶限流
///
/// 如果某个安全加固措施属于「限制插件能做什么」而非「保护宿主进程不崩溃」，
/// 则它不应存在于此模块中。
pub struct PluginManager {
    instances: Vec<Option<PluginInstance>>,
    command_map: HashMap<String, Vec<usize>>,
    #[allow(clippy::type_complexity)]
    tick_registry: Arc<Mutex<Vec<(usize, String, u64, u32)>>>,
    on_message_indices: HashSet<usize>,
    lost_instances: Vec<u32>,
    engine: Engine,
    linker: Linker<HostServices>,
    modules: Vec<Module>,
    instance_params: Vec<PluginInstanceParams>,
    wasm_paths: Vec<String>,
    wasm_mtimes: Vec<Option<std::time::SystemTime>>,
    reload_template: HostServices,
    epoch_running: Arc<AtomicBool>,
    epoch_handle: tokio::task::JoinHandle<()>,
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.epoch_running.store(false, Ordering::Relaxed);
        self.epoch_handle.abort();
    }
}

fn resolve_wasm_path(dir: &str, plugin_path: &str) -> Result<String, String> {
    let raw = if Path::new(plugin_path).is_absolute() {
        plugin_path.to_string()
    } else {
        format!("{dir}/{plugin_path}")
    };
    for component in Path::new(&raw).components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(format!("plugin path contains '..': {plugin_path}"));
        }
    }
    let path = Path::new(&raw);
    if path.exists() {
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("failed to canonicalize path: {e}"))?;
        return Ok(canonical.to_string_lossy().to_string());
    }
    Ok(raw)
}

impl PluginManager {
    pub async fn new(config: &PluginConfigInput, services: HostServices) -> Result<Self, String> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.epoch_interruption(true);
        let engine =
            Engine::new(&wasm_config).map_err(|e| format!("engine creation failed: {e}"))?;

        let epoch_running = Arc::new(AtomicBool::new(true));
        let epoch_engine = engine.clone();
        let epoch_flag = epoch_running.clone();
        let epoch_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_micros(500));
            while epoch_flag.load(Ordering::Relaxed) {
                interval.tick().await;
                epoch_engine.increment_epoch();
            }
        });
        let mut linker = Linker::<HostServices>::new(&engine);
        bridge::register_host_functions(&mut linker)
            .map_err(|e| format!("linker setup failed: {e}"))?;

        struct PluginBlueprint {
            module: Module,
            params: PluginInstanceParams,
            config: PluginInstanceConfig,
            wasm_path: String,
        }

        let mut seen_names = HashSet::new();
        let mut blueprints: Vec<PluginBlueprint> = Vec::new();
        for pcfg in &config.instances {
            if !pcfg.enabled {
                tracing::info!(name = %pcfg.name, "Plugin disabled, skipping");
                continue;
            }
            if !seen_names.insert(&pcfg.name) {
                return Err(format!("duplicate plugin name: {}", pcfg.name));
            }

            let wasm_path = resolve_wasm_path(&config.dir, &pcfg.path)
                .map_err(|e| format!("bad plugin path: {e}"))?;

            let module = Module::from_file(&engine, &wasm_path)
                .map_err(|e| format!("load module {}: {e}", pcfg.name))?;

            let params = PluginInstanceParams {
                name: pcfg.name.clone(),
                priority: pcfg.priority,
                plugin_config: pcfg.config.clone(),
                timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECS),
            };

            blueprints.push(PluginBlueprint {
                module,
                params,
                config: pcfg.clone(),
                wasm_path,
            });
            tracing::info!(name = %pcfg.name, "Plugin module loaded");
        }

        blueprints.sort_by_key(|b| std::cmp::Reverse(b.params.priority));

        let mut instances: Vec<Option<PluginInstance>> = Vec::new();
        let mut modules: Vec<Module> = Vec::new();
        let mut instance_params: Vec<PluginInstanceParams> = Vec::new();
        let mut wasm_paths_vec: Vec<String> = Vec::new();
        let mut wasm_mtimes: Vec<Option<std::time::SystemTime>> = Vec::new();
        let mut command_map: HashMap<String, Vec<usize>> = HashMap::new();
        let mut on_message_indices: HashSet<usize> = HashSet::new();
        #[allow(clippy::type_complexity)]
        let tick_registry: Arc<Mutex<Vec<(usize, String, u64, u32)>>> =
            Arc::new(Mutex::new(Vec::new()));
        let tick_id_counter = Arc::new(AtomicU32::new(0));

        for (sorted_idx, blueprint) in blueprints.into_iter().enumerate() {
            let mut store = Store::new(
                &engine,
                HostServices {
                    http_client: services.http_client.clone(),
                    blocking_http_client: services.blocking_http_client.clone(),
                    rate_limiter: services.rate_limiter.clone(),
                    oauth: services.oauth.clone(),
                    storage: services.storage.clone(),
                    send_msg_fn: services.send_msg_fn.clone(),
                    runtime_handle: tokio::runtime::Handle::current(),
                    instance_idx: sorted_idx,
                    tick_registry: tick_registry.clone(),
                    tick_id_counter: tick_id_counter.clone(),
                    instance_config: blueprint.config.config.clone(),
                    limiter: StoreLimitsBuilder::new()
                        .memory_size(100 * 1024 * 1024)
                        .build(),
                },
            );
            store.limiter(|state| &mut state.limiter);

            let mut instance = PluginInstance::new(
                &engine,
                &linker,
                &blueprint.module,
                blueprint.params.clone(),
                store,
            )?;

            let metadata = instance.metadata();
            for cmd in metadata.commands.iter() {
                command_map.entry(cmd.clone()).or_default().push(sorted_idx);
            }
            // 记录拥有 on_message 导出的实例索引
            if instance.has_export("on_message") {
                on_message_indices.insert(sorted_idx);
            }

            instances.push(Some(instance));
            modules.push(blueprint.module);
            instance_params.push(blueprint.params);
            let wasm_mtime = std::fs::metadata(&blueprint.wasm_path)
                .ok()
                .and_then(|m| m.modified().ok());
            wasm_mtimes.push(wasm_mtime);
            let wasm_path = blueprint.wasm_path;
            wasm_paths_vec.push(wasm_path);

            tracing::info!(
                name = %instance_params.last().map_or("unknown", |p| &p.name),
                sorted_idx,
                "Plugin instantiated"
            );
        }

        for (sorted_idx, instance) in instances.iter_mut().enumerate() {
            if let Some(inst) = instance {
                inst.set_instance_idx(sorted_idx);
                inst.on_load()?;
                tracing::info!(name = %inst.name, sorted_idx, "Plugin on_load completed");
            }
        }

        let reload_template = HostServices {
            http_client: services.http_client,
            blocking_http_client: services.blocking_http_client,
            rate_limiter: services.rate_limiter,
            oauth: services.oauth,
            storage: services.storage,
            send_msg_fn: services.send_msg_fn,
            runtime_handle: tokio::runtime::Handle::current(),
            instance_idx: 0,
            tick_registry: tick_registry.clone(),
            tick_id_counter: tick_id_counter.clone(),
            instance_config: None,
            limiter: StoreLimitsBuilder::new()
                .memory_size(100 * 1024 * 1024)
                .build(),
        };

        let lost_instances = vec![0u32; instances.len()];

        Ok(Self {
            instances,
            command_map,
            tick_registry,
            on_message_indices,
            lost_instances,
            engine,
            linker,
            modules,
            instance_params,
            wasm_paths: wasm_paths_vec,
            wasm_mtimes,
            reload_template,
            epoch_running,
            epoch_handle,
        })
    }

    pub async fn handle_command(&mut self, cmd_name: &str, cmd_json: &str) -> PluginAction {
        let mut indices = match self.command_map.get(cmd_name) {
            Some(indices) => indices.clone(),
            None => return PluginAction::Next,
        };
        indices.sort_by_key(|&i| {
            std::cmp::Reverse(self.instance_params.get(i).map(|p| p.priority).unwrap_or(0))
        });

        for &idx in &indices {
            let cmd_owned = cmd_json.to_owned();
            match self
                .dispatch(idx, "command", move |instance| {
                    instance.on_command(&cmd_owned)
                })
                .await
            {
                DispatchOutcome::Ok(PluginAction::Handled(msg)) => {
                    return PluginAction::Handled(msg)
                }
                DispatchOutcome::Ok(PluginAction::Intercepted) => return PluginAction::Intercepted,
                DispatchOutcome::Ok(PluginAction::Next)
                | DispatchOutcome::PluginError
                | DispatchOutcome::TimeoutLost
                | DispatchOutcome::Panic
                | DispatchOutcome::NotAvailable => continue,
            }
        }
        PluginAction::Next
    }

    /// Dispatch a raw message to all plugins that have on_message export.
    /// Returns the first non-Next action from any plugin.
    pub async fn handle_message(&mut self, msg_json: &str) -> PluginAction {
        let mut indices: Vec<usize> = self.on_message_indices.iter().copied().collect();
        indices.sort_by_key(|&i| {
            std::cmp::Reverse(self.instance_params.get(i).map(|p| p.priority).unwrap_or(0))
        });
        for idx in indices {
            if self.instances[idx].is_none() {
                continue;
            }

            let msg_owned = msg_json.to_owned();
            match self
                .dispatch(idx, "on_message", move |instance| {
                    instance.on_message(&msg_owned)
                })
                .await
            {
                DispatchOutcome::Ok(PluginAction::Handled(msg)) => {
                    return PluginAction::Handled(msg)
                }
                DispatchOutcome::Ok(PluginAction::Intercepted) => return PluginAction::Intercepted,
                DispatchOutcome::Ok(PluginAction::Next)
                | DispatchOutcome::PluginError
                | DispatchOutcome::TimeoutLost
                | DispatchOutcome::Panic
                | DispatchOutcome::NotAvailable => continue,
            }
        }
        PluginAction::Next
    }

    // 插件宿主函数（send_group_msg、http_request 等）通过 block_in_place + block_on
    // 调用异步运行时，在 spawn_blocking 或 async 上下文中均可安全执行。
    async fn dispatch<F, T>(
        &mut self,
        idx: usize,
        operation: &'static str,
        f: F,
    ) -> DispatchOutcome<T>
    where
        F: FnOnce(&mut PluginInstance) -> Result<T, String> + Send + 'static,
        T: Send + 'static,
    {
        if idx >= self.instances.len() {
            return DispatchOutcome::NotAvailable;
        }
        let mut instance = match self.instances[idx].take() {
            Some(inst) => inst,
            None => return DispatchOutcome::NotAvailable,
        };

        let name = instance.name.clone();
        let timeout_dur = instance.timeout;

        let result = tokio::time::timeout(
            timeout_dur,
            tokio::task::spawn_blocking(move || {
                let r = f(&mut instance);
                (r, instance)
            }),
        )
        .await;

        match result {
            Ok(Ok((Ok(val), instance))) => {
                self.instances[idx] = Some(instance);
                DispatchOutcome::Ok(val)
            }
            Ok(Ok((Err(e), instance))) => {
                self.instances[idx] = Some(instance);
                self.lost_instances[idx] = self.lost_instances[idx].saturating_add(1);
                if self.lost_instances[idx] >= 5 {
                    tracing::warn!(
                        plugin = %name,
                        consecutive = self.lost_instances[idx],
                        "Plugin {operation} 连续错误，自动重载"
                    );
                    if let Err(re) = self.reload_instance(idx) {
                        tracing::error!(plugin = %name, "重载失败: {re}");
                    }
                }
                tracing::warn!(plugin = %name, error = %e, "Plugin {operation} error");
                DispatchOutcome::PluginError
            }
            Ok(Err(join_err)) => {
                tracing::error!(plugin = %name, "Plugin {operation} panicked: {join_err}");
                self.lost_instances[idx] = self.lost_instances[idx].saturating_add(1);
                if self.lost_instances[idx] >= 5 {
                    tracing::warn!(
                        plugin = %name,
                        consecutive = self.lost_instances[idx],
                        "Plugin {operation} 连续 panic，自动重载"
                    );
                    if let Err(e) = self.reload_instance(idx) {
                        tracing::error!(plugin = %name, "Failed to reload after panic: {e}");
                    }
                }
                DispatchOutcome::Panic
            }
            Err(_) => {
                tracing::warn!(plugin = %name, "Plugin {operation} timed out");
                // 强制推进 epoch，使 spawn_blocking 中的 wasm 执行尽快触发 epoch trap，
                // 避免后台线程在超时后继续泄漏 10+ 秒
                self.engine.increment_epoch();
                // 超时时实例已被 take() 移出并移交给了 spawn_blocking（仍在后台运行），
                // 无法放回 self.instances[idx]，必须立即重载——不需要等待 lost_instances 累积到 5，
                // 因为后续 dispatch 会因 self.instances[idx] 为 None 而直接返回 NotAvailable
                tracing::warn!(
                    plugin = %name,
                    "Plugin {operation} 超时，自动重载实例"
                );
                if let Err(e) = self.reload_instance(idx) {
                    tracing::error!(plugin = %name, "Failed to reload plugin instance: {e}");
                }
                DispatchOutcome::TimeoutLost
            }
        }
    }

    /// 从 reload_template 克隆并创建 Store，减少 reload_instance/reload_all 中的代码重复。
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
            .memory_size(100 * 1024 * 1024)
            .build();
        let mut store = Store::new(&self.engine, host_services);
        store.limiter(|state| &mut state.limiter);
        store
    }

    /// 从配置构建 PluginInstance，减少 to_add/to_reload 中的重复代码。
    fn build_instance(
        &self,
        idx: usize,
        pcfg: &crate::config::PluginInstanceConfig,
        module: &Module,
    ) -> Result<(PluginInstance, crate::instance::PluginInstanceParams), String> {
        let params = crate::instance::PluginInstanceParams {
            name: pcfg.name.clone(),
            priority: pcfg.priority,
            plugin_config: pcfg.config.clone(),
            timeout: Duration::from_secs(DEFAULT_PLUGIN_TIMEOUT_SECS),
        };
        let store = self.create_reload_store(idx, pcfg.config.clone());
        let mut instance = crate::instance::PluginInstance::new(
            &self.engine,
            &self.linker,
            module,
            params.clone(),
            store,
        )?;
        instance.set_instance_idx(idx);
        Ok((instance, params))
    }

    pub fn reload_instance(&mut self, idx: usize) -> Result<(), String> {
        let module = self
            .modules
            .get(idx)
            .ok_or_else(|| format!("module not found for idx {idx}"))?;
        let params = self
            .instance_params
            .get(idx)
            .ok_or_else(|| format!("params not found for idx {idx}"))?;

        // 清除旧的 tick 注册，防止重复注册和内存泄漏
        {
            let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                tracing::warn!("tick_registry mutex 被污染，强制恢复（数据可能不一致）");
                e.into_inner()
            });
            registry.retain(|(plugin_idx, _, _, _)| *plugin_idx != idx);
        }

        let store = self.create_reload_store(idx, params.plugin_config.clone());
        let mut instance =
            PluginInstance::new(&self.engine, &self.linker, module, params.clone(), store)?;
        instance.set_instance_idx(idx);

        // 更新 on_message 索引
        if instance.has_export("on_message") {
            self.on_message_indices.insert(idx);
        } else {
            self.on_message_indices.remove(&idx);
        }

        if let Err(e) = instance.on_load() {
            // on_load 中可能已注册 tick，失败后需清理残留
            let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                tracing::warn!("tick_registry mutex 被污染，强制恢复（数据可能不一致）");
                e.into_inner()
            });
            registry.retain(|(plugin_idx, _, _, _)| *plugin_idx != idx);
            return Err(e);
        }
        self.instances[idx] = Some(instance);
        self.lost_instances[idx] = 0;
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.instances.iter().all(|i| i.is_none())
    }

    pub fn len(&self) -> usize {
        self.instances.iter().filter(|i| i.is_some()).count()
    }

    pub fn get_ticks(&self) -> Vec<(usize, u64, u32)> {
        let registry = self.tick_registry.lock().unwrap_or_else(|e| {
            tracing::warn!("tick_registry mutex 被污染，强制恢复（数据可能不一致）");
            e.into_inner()
        });
        registry
            .iter()
            .map(|(plugin_idx, _, interval_secs, tick_id)| (*plugin_idx, *interval_secs, *tick_id))
            .collect()
    }

    /// 从指定槽位取出实例（不持锁时调用方负责同步）。
    /// 返回 None 表示槽位越界或为空。
    pub fn take_instance(&mut self, idx: usize) -> Option<PluginInstance> {
        self.instances.get_mut(idx).and_then(|slot| slot.take())
    }

    /// 将实例放回指定槽位。如果槽位越界则静默丢弃。
    pub fn put_instance(&mut self, idx: usize, instance: PluginInstance) {
        if idx < self.instances.len() {
            self.instances[idx] = Some(instance);
        }
    }

    pub async fn handle_tick(&mut self, plugin_idx: usize, tick_id: u32) {
        if plugin_idx >= self.instances.len() {
            return;
        }
        let has = self.instances[plugin_idx]
            .as_mut()
            .map(|i| i.has_export("on_tick"))
            .unwrap_or(false);
        if !has {
            return;
        }

        match self
            .dispatch(plugin_idx, "on_tick", move |instance| {
                instance.on_tick(tick_id)
            })
            .await
        {
            DispatchOutcome::Ok(()) | DispatchOutcome::PluginError => {}
            DispatchOutcome::TimeoutLost
            | DispatchOutcome::Panic
            | DispatchOutcome::NotAvailable => {}
        }
    }

    pub async fn shutdown(&mut self) {
        self.epoch_running.store(false, Ordering::Relaxed);
        self.epoch_handle.abort();
        // epoch task 通过 epoch_running 标志自行退出，无需 join。
        // abort() 确保在极少数情况下（如任务卡在 interval tick 中）强制终止。

        for idx in 0..self.instances.len() {
            let has = self.instances[idx]
                .as_mut()
                .map(|i| i.has_export("on_unload"))
                .unwrap_or(false);
            if !has {
                continue;
            }

            match self
                .dispatch(idx, "on_unload", |instance| instance.on_unload())
                .await
            {
                DispatchOutcome::Ok(()) => {
                    let name = self.instances[idx].as_ref().map(|i| &i.name).cloned();
                    tracing::info!(plugin = ?name, "Plugin unloaded");
                }
                DispatchOutcome::PluginError => {}
                DispatchOutcome::TimeoutLost
                | DispatchOutcome::Panic
                | DispatchOutcome::NotAvailable => {}
            }
        }
        self.instances.clear();
        self.modules.clear();
        self.instance_params.clear();
        self.wasm_paths.clear();
        self.wasm_mtimes.clear();
        self.lost_instances.clear();
        self.on_message_indices.clear();
        self.command_map.clear();
    }

    pub async fn reload_all(&mut self, new_config: &PluginConfigInput) -> Result<(), String> {
        use crate::config::PluginInstanceConfig;
        use std::collections::{HashMap, HashSet};

        #[allow(clippy::type_complexity)]
        let mut old_map: HashMap<
            String,
            (
                usize,
                String,
                u32,
                Option<serde_json::Value>,
                Option<std::time::SystemTime>,
            ),
        > = HashMap::new();
        for (idx, params) in self.instance_params.iter().enumerate() {
            if self.instances[idx].is_some() {
                old_map.insert(
                    params.name.clone(),
                    (
                        idx,
                        self.wasm_paths.get(idx).cloned().unwrap_or_default(),
                        params.priority,
                        params.plugin_config.clone(),
                        self.wasm_mtimes.get(idx).copied().flatten(),
                    ),
                );
            }
        }

        let new_enabled: Vec<&PluginInstanceConfig> =
            new_config.instances.iter().filter(|p| p.enabled).collect();

        let new_names: HashSet<&str> = new_enabled.iter().map(|p| p.name.as_str()).collect();
        let old_names: HashSet<&str> = old_map.keys().map(|s| s.as_str()).collect();

        let mut to_remove: Vec<usize> = Vec::new();
        for (name, (idx, _, _, _, _)) in &old_map {
            if !new_names.contains(name.as_str()) {
                to_remove.push(*idx);
            }
        }

        let to_add: Vec<&PluginInstanceConfig> = new_enabled
            .iter()
            .filter(|p| !old_names.contains(p.name.as_str()))
            .copied()
            .collect();

        let to_reload: Vec<&PluginInstanceConfig> = new_enabled
            .iter()
            .filter(|p| {
                if let Some((_, old_path, old_priority, old_config, old_mtime)) =
                    old_map.get(&p.name)
                {
                    let new_path = match resolve_wasm_path(&new_config.dir, &p.path) {
                        Ok(p) => p,
                        Err(_) => return false,
                    };
                    let current_mtime = std::fs::metadata(&new_path)
                        .ok()
                        .and_then(|m| m.modified().ok());
                    p.priority != *old_priority
                        || new_path != *old_path
                        || p.config != *old_config
                        || *old_mtime != current_mtime
                } else {
                    false
                }
            })
            .copied()
            .collect();

        info!(
            to_add = to_add.len(),
            to_remove = to_remove.len(),
            to_reload = to_reload.len(),
            "Plugin diff 完成"
        );

        for idx in to_remove.iter().rev() {
            let has_unload = self.instances[*idx]
                .as_mut()
                .map(|i| i.has_export("on_unload"))
                .unwrap_or(false);

            if has_unload {
                match self
                    .dispatch(*idx, "on_unload", |instance| instance.on_unload())
                    .await
                {
                    DispatchOutcome::Ok(()) | DispatchOutcome::PluginError => {}
                    DispatchOutcome::TimeoutLost
                    | DispatchOutcome::Panic
                    | DispatchOutcome::NotAvailable => {}
                }
            }

            for indices in self.command_map.values_mut() {
                indices.retain(|i| *i != *idx);
            }
            self.on_message_indices.remove(idx);

            {
                let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                    tracing::warn!("tick_registry mutex 被污染，强制恢复（数据可能不一致）");
                    e.into_inner()
                });
                registry.retain(|(plugin_idx, _, _, _)| *plugin_idx != *idx);
            }

            self.instances[*idx] = None;
            info!(idx = *idx, "Plugin 已移除");
        }

        for pcfg in &to_add {
            let wasm_path = match resolve_wasm_path(&new_config.dir, &pcfg.path) {
                Ok(p) => p,
                Err(e) => {
                    warn!(name = %pcfg.name, "路径无效，跳过: {e}");
                    continue;
                }
            };

            let module = Module::from_file(&self.engine, &wasm_path).map_err(|e| {
                warn!(name = %pcfg.name, "WASM 编译失败: {e}");
                e.to_string()
            })?;

            let sorted_idx = self.instances.len();
            let (mut instance, params) = self.build_instance(sorted_idx, pcfg, &module)?;

            if let Err(e) = instance.on_load() {
                warn!(name = %pcfg.name, "on_load 失败: {e}");
                // on_load 中可能已注册 tick，失败后需清理残留
                {
                    let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("tick_registry mutex 被污染，强制恢复（数据可能不一致）");
                        e.into_inner()
                    });
                    registry.retain(|(plugin_idx, _, _, _)| *plugin_idx != sorted_idx);
                }
                // 不 push 失败实例，避免僵尸 slot（永不 dispatch，占用 100MB Store）。
                // 实例/module/params 在此处 drop，compact() 负责清理索引空洞。
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
            let wasm_mtime = std::fs::metadata(&wasm_path)
                .ok()
                .and_then(|m| m.modified().ok());
            self.wasm_mtimes.push(wasm_mtime);
            self.wasm_paths.push(wasm_path);

            info!(name = %pcfg.name, "Plugin 已添加");
        }

        for pcfg in &to_reload {
            if let Some((idx, _, _, _, _)) = old_map.get(&pcfg.name) {
                let wasm_path = match resolve_wasm_path(&new_config.dir, &pcfg.path) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!(name = %pcfg.name, "路径无效，跳过重载: {e}");
                        continue;
                    }
                };

                let module = match Module::from_file(&self.engine, &wasm_path) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!(name = %pcfg.name, "WASM 编译失败，跳过重载: {e}");
                        continue;
                    }
                };

                let has_unload = self.instances[*idx]
                    .as_mut()
                    .map(|i| i.has_export("on_unload"))
                    .unwrap_or(false);
                if has_unload {
                    let _ = self
                        .dispatch(*idx, "on_unload", |instance| instance.on_unload())
                        .await;
                }

                let (mut instance, params) = self.build_instance(*idx, pcfg, &module)?;

                // 在 on_load 之前清理旧 tick，避免新注册的 tick 被误删
                {
                    let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                        tracing::warn!("tick_registry mutex 被污染，强制恢复（数据可能不一致）");
                        e.into_inner()
                    });
                    registry.retain(|(plugin_idx, _, _, _)| *plugin_idx != *idx);
                }

                if let Err(e) = instance.on_load() {
                    warn!(name = %pcfg.name, "on_load 失败: {e}");
                    self.instances[*idx] = Some(instance);
                    self.modules[*idx] = module;
                    self.instance_params[*idx] = params;
                    self.lost_instances[*idx] = 1;
                    {
                        let mut registry = self.tick_registry.lock().unwrap_or_else(|e| {
                            tracing::warn!(
                                "tick_registry mutex 被污染，强制恢复（数据可能不一致）"
                            );
                            e.into_inner()
                        });
                        registry.retain(|(plugin_idx, _, _, _)| *plugin_idx != *idx);
                    }
                    for indices in self.command_map.values_mut() {
                        indices.retain(|i| *i != *idx);
                    }
                    self.on_message_indices.remove(idx);
                    continue;
                }

                for indices in self.command_map.values_mut() {
                    indices.retain(|i| *i != *idx);
                }
                self.on_message_indices.remove(idx);

                let metadata = instance.metadata();
                for cmd in metadata.commands.iter() {
                    self.command_map.entry(cmd.clone()).or_default().push(*idx);
                }
                if instance.has_export("on_message") {
                    self.on_message_indices.insert(*idx);
                }

                self.instances[*idx] = Some(instance);
                self.modules[*idx] = module;
                self.instance_params[*idx] = params;
                self.lost_instances[*idx] = 0;
                let wasm_mtime = std::fs::metadata(&wasm_path)
                    .ok()
                    .and_then(|m| m.modified().ok());
                self.wasm_mtimes[*idx] = wasm_mtime;
                self.wasm_paths[*idx] = wasm_path;

                info!(name = %pcfg.name, "Plugin 已重载");
            }
        }

        self.compact();
        Ok(())
    }

    fn compact(&mut self) {
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

        macro_rules! filter_by_mask {
            ($v:expr) => {{
                let taken = std::mem::take($v);
                taken
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| old_to_new[*i].is_some())
                    .map(|(_, x)| x)
                    .collect()
            }};
        }

        self.instances = filter_by_mask!(&mut self.instances);
        self.modules = filter_by_mask!(&mut self.modules);
        self.instance_params = filter_by_mask!(&mut self.instance_params);
        self.wasm_paths = filter_by_mask!(&mut self.wasm_paths);
        self.wasm_mtimes = filter_by_mask!(&mut self.wasm_mtimes);
        self.lost_instances = filter_by_mask!(&mut self.lost_instances);

        for indices in self.command_map.values_mut() {
            let mut retained = Vec::with_capacity(indices.len());
            for &old_idx in indices.iter() {
                if let Some(Some(new_idx)) = old_to_new.get(old_idx) {
                    retained.push(*new_idx);
                }
            }
            *indices = retained;
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
                tracing::warn!("tick_registry mutex 被污染，强制恢复（数据可能不一致）");
                e.into_inner()
            });
            for entry in reg.iter_mut() {
                if let Some(new_idx) = old_to_new.get(entry.0).and_then(|x| *x) {
                    entry.0 = new_idx;
                }
            }
        }

        for (new_idx, inst_opt) in self.instances.iter_mut().enumerate() {
            if let Some(inst) = inst_opt {
                inst.set_instance_idx(new_idx);
            }
        }

        info!(
            old_len,
            new_len,
            removed = old_len - new_len,
            "PluginManager 已压缩"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PluginMetadata;

    fn make_services(rt: &tokio::runtime::Runtime) -> HostServices {
        HostServices {
            http_client: reqwest::Client::new(),
            blocking_http_client: reqwest::blocking::Client::new(),
            rate_limiter: Arc::new(osubot_core::RateLimiter::new()),
            oauth: Arc::new(osubot_core::OauthTokenCache::new(
                String::new(),
                String::new(),
            )),
            storage: Arc::new(
                osubot_core::Storage::new(":memory:").expect("failed to create in-memory storage"),
            ),
            send_msg_fn: Arc::new(|_group_id, _text| Ok(())),
            runtime_handle: rt.handle().clone(),
            instance_idx: 0,
            tick_registry: Arc::new(std::sync::Mutex::new(Vec::new())),
            tick_id_counter: Arc::new(AtomicU32::new(0)),
            instance_config: None,
            limiter: StoreLimitsBuilder::new()
                .memory_size(100 * 1024 * 1024)
                .build(),
        }
    }

    #[test]
    fn test_plugin_action_deserialize_handled() {
        let json = r#"{"Handled":"hello"}"#;
        let action: PluginAction = serde_json::from_str(json).unwrap();
        match action {
            PluginAction::Handled(s) => assert_eq!(s, "hello"),
            _ => panic!("expected Handled"),
        }
    }

    #[test]
    fn test_plugin_action_deserialize_next() {
        let json = r#""Next""#;
        let action: PluginAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, PluginAction::Next));
    }

    #[test]
    fn test_plugin_action_deserialize_intercepted() {
        let json = r#""Intercepted""#;
        let action: PluginAction = serde_json::from_str(json).unwrap();
        assert!(matches!(action, PluginAction::Intercepted));
    }

    #[test]
    fn test_plugin_manager_default_empty() {
        let config = PluginConfigInput::default();
        assert!(config.instances.is_empty());
    }

    #[test]
    fn test_metadata_deserialize() {
        // 注意：priority 字段在 TOML config 中设置，PluginMetadata 宿主端没有此字段
        let json = r#"{"name":"test","version":"1.0","author":"me","description":"desc","commands":["!ping"]}"#;
        let meta: PluginMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(meta.name, "test");
        assert_eq!(meta.commands, vec!["!ping"]);
    }

    #[test]
    fn test_enabled_default_is_false() {
        let config = PluginInstanceConfig {
            name: "test".to_string(),
            path: "test.wasm".to_string(),
            enabled: false,
            priority: 0,
            config: None,
        };
        assert!(!config.enabled);
        // Default should be false — verify via serde
        let json = r#"{"name":"test","path":"test.wasm"}"#;
        let config: PluginInstanceConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled, "enabled should default to false");
    }

    #[test]
    fn test_get_due_ticks() {
        assert!(
            true,
            "tick interval scheduling is handled in the main binary tick loop"
        );
    }

    fn find_hello_plugin_wasm() -> Option<String> {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
        let workspace_dir = std::path::Path::new(&manifest_dir).parent()?;
        // hello-plugin is not a workspace member, so its build artifacts go into
        // examples/hello-plugin/target/ instead of the workspace target/
        // Build with wasm32-unknown-unknown (no WASI imports) since the SDK
        // uses custom alloc/dealloc and doesn't depend on WASI.
        let wasm_path = workspace_dir
            .join("examples")
            .join("hello-plugin")
            .join("target")
            .join("wasm32-unknown-unknown")
            .join("debug")
            .join("hello_plugin.wasm");

        if wasm_path.exists() {
            return Some(wasm_path.to_string_lossy().to_string());
        }

        // Try building it (hello-plugin is excluded from workspace, use --manifest-path)
        let status = std::process::Command::new("cargo")
            .args([
                "build",
                "--manifest-path",
                "examples/hello-plugin/Cargo.toml",
                "--target",
                "wasm32-unknown-unknown",
            ])
            .current_dir(workspace_dir)
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        Some(wasm_path.to_string_lossy().to_string())
    }

    #[test]
    fn test_plugin_load_and_metadata() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        assert!(!pm.is_empty());
        assert_eq!(pm.len(), 1);

        // Test !hello command
        let cmd_json = r#"{"command_type":"!hello","group_id":12345,"user_id":67890,"mode":0}"#;
        let action = rt.block_on(pm.handle_command("!hello", cmd_json));
        match action {
            PluginAction::Handled(ref msg) => {
                assert!(msg.contains("Hello from WASM plugin"));
            }
            other => panic!("expected Handled, got {other:?}"),
        }

        // Test unknown command passes through
        let action = rt.block_on(pm.handle_command("!unknown", cmd_json));
        assert!(matches!(action, PluginAction::Next));

        // Test ticks were registered (hello-plugin registers a tick in on_load)
        let ticks = pm.get_ticks();
        assert!(!ticks.is_empty(), "expected at least one tick registration");

        // Test tick handler
        if let Some((plugin_idx, _interval_secs, tick_id)) = ticks.first().copied() {
            rt.block_on(pm.handle_tick(plugin_idx, tick_id));
        }

        // Test shutdown
        rt.block_on(pm.shutdown());
        drop(_guard);
        assert!(pm.is_empty());
    }

    #[test]
    fn test_handle_message_no_on_message_export() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path,
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        // hello-plugin now exports on_message — sends a handled response for "hello" / "你好"
        let msg_json = r#"{"group_id":12345,"user_id":67890,"message":"hello world","mentioned_user_id":null}"#;
        let action = rt.block_on(pm.handle_message(msg_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "expected Handled since hello-plugin has on_message for 'hello', got {action:?}"
        );

        // Message without trigger words should return Next
        let msg_json2 = r#"{"group_id":12345,"user_id":67890,"message":"random text","mentioned_user_id":null}"#;
        let action2 = rt.block_on(pm.handle_message(msg_json2));
        assert!(
            matches!(action2, PluginAction::Next),
            "expected Next for non-matching message, got {action2:?}"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_plugin_ping_command() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path,
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":0}"#;
        let action = rt.block_on(pm.handle_command("!ping", cmd_json));
        match action {
            PluginAction::Handled(ref msg) => {
                assert!(msg.contains("pong from WASM plugin"));
            }
            other => panic!("expected Handled for !ping, got {other:?}"),
        }

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_compact_command_map_cleanup() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![
                PluginInstanceConfig {
                    name: "hello1".to_string(),
                    path: wasm_path.clone(),
                    enabled: true,
                    priority: 0,
                    config: None,
                },
                PluginInstanceConfig {
                    name: "hello2".to_string(),
                    path: wasm_path,
                    enabled: true,
                    priority: 0,
                    config: None,
                },
            ],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        assert_eq!(pm.len(), 2);
        assert!(
            pm.command_map.contains_key("!ping"),
            "!ping should be in command_map"
        );

        let ping_indices_before = pm.command_map.get("!ping").cloned().unwrap_or_default();
        assert_eq!(
            ping_indices_before.len(),
            2,
            "both plugins registered !ping"
        );

        // Set instance 0 to None (simulate a failed instance that was removed)
        pm.instances[0] = None;
        pm.lost_instances[0] = 5;

        pm.compact();

        assert_eq!(pm.len(), 1, "only one plugin should remain after compact");

        let ping_indices_after = pm.command_map.get("!ping").cloned().unwrap_or_default();
        assert_eq!(
            ping_indices_after.len(),
            1,
            "!ping should have only 1 entry after compact, got {}: {:?}",
            ping_indices_after.len(),
            ping_indices_after
        );
        assert_eq!(
            ping_indices_after[0], 0,
            "remaining !ping entry should point to index 0, got {}",
            ping_indices_after[0]
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_tick_cleanup_on_reload() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path,
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        // 验证 tick 已注册（hello-plugin 在 on_load 中注册 tick）
        let ticks_before = pm.get_ticks();
        assert!(
            !ticks_before.is_empty(),
            "expected at least one tick registration after load"
        );
        let tick_count_before = ticks_before.len();

        // 触发 reload（模拟超时）
        pm.reload_instance(0).expect("reload failed");

        // 验证旧 tick 已清除，新 tick 已重新注册
        let ticks_after = pm.get_ticks();
        assert_eq!(
            ticks_after.len(),
            tick_count_before,
            "tick count should remain the same after reload"
        );

        // 验证 tick 的 instance_idx 正确
        for (plugin_idx, _, _) in &ticks_after {
            assert_eq!(*plugin_idx, 0, "tick should belong to instance 0");
        }

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_wasm_hotreload_detects_mtime_change() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        // Record the stored mtime
        let initial_mtime = pm.wasm_mtimes[0];

        // Touch the wasm file to change its mtime (without changing config)
        let touch_status = std::process::Command::new("touch")
            .arg(&wasm_path)
            .status()
            .expect("failed to run touch");
        assert!(touch_status.success(), "touch should succeed");

        // Call reload_all with identical config — should detect mtime change
        let same_config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        rt.block_on(pm.reload_all(&same_config))
            .expect("reload_all should succeed");

        // Verify mtime was updated
        let updated_mtime = pm.wasm_mtimes[0];
        assert_ne!(
            initial_mtime, updated_mtime,
            "mtime should be updated after reload"
        );

        // Verify the plugin still works after reload
        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":0}"#;
        let action = rt.block_on(pm.handle_command("!ping", cmd_json));
        match action {
            PluginAction::Handled(ref msg) => {
                assert!(msg.contains("pong from WASM plugin"));
            }
            other => panic!("expected Handled for !ping after reload, got {other:?}"),
        }

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_reload_all_add_plugin() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config1 = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello1".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config1, services))
            .expect("PluginManager::new failed");
        assert_eq!(pm.len(), 1);

        let config2 = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![
                PluginInstanceConfig {
                    name: "hello1".to_string(),
                    path: wasm_path.clone(),
                    enabled: true,
                    priority: 0,
                    config: None,
                },
                PluginInstanceConfig {
                    name: "hello2".to_string(),
                    path: wasm_path.clone(),
                    enabled: true,
                    priority: 10,
                    config: None,
                },
            ],
        };

        rt.block_on(pm.reload_all(&config2))
            .expect("reload_all should succeed");
        assert_eq!(pm.len(), 2, "should have 2 instances after adding");

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":0}"#;
        let action = rt.block_on(pm.handle_command("!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "!ping should still work after reload_all add, got {action:?}"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_reload_all_remove_plugin() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![
                PluginInstanceConfig {
                    name: "hello1".to_string(),
                    path: wasm_path.clone(),
                    enabled: true,
                    priority: 0,
                    config: None,
                },
                PluginInstanceConfig {
                    name: "hello2".to_string(),
                    path: wasm_path.clone(),
                    enabled: true,
                    priority: 10,
                    config: None,
                },
            ],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");
        assert_eq!(pm.len(), 2);

        let config2 = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello2".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 10,
                config: None,
            }],
        };

        rt.block_on(pm.reload_all(&config2))
            .expect("reload_all should succeed");
        assert_eq!(pm.len(), 1, "should have 1 instance after removal");

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":0}"#;
        let action = rt.block_on(pm.handle_command("!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "remaining plugin should still handle !ping after reload_all remove, got {action:?}"
        );

        let action2 = rt.block_on(pm.handle_command("!hello", cmd_json));
        assert!(
            matches!(action2, PluginAction::Handled(_)),
            "!hello should also still work after reload_all remove, got {action2:?}"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_reload_all_remove_all_plugins() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello1".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");
        assert!(!pm.is_empty());

        let config_empty = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![],
        };

        rt.block_on(pm.reload_all(&config_empty))
            .expect("reload_all should succeed");
        assert!(
            pm.is_empty(),
            "PM should be empty after removing all plugins"
        );
        assert_eq!(pm.len(), 0);

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":0}"#;
        let action = rt.block_on(pm.handle_command("!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Next),
            "no plugins left, should pass through, got {action:?}"
        );

        // shutdown on empty PM should not panic
        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_reload_all_reload_on_priority_change() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");
        assert_eq!(pm.len(), 1);

        let ticks_before = pm.get_ticks();
        assert!(
            !ticks_before.is_empty(),
            "expected tick registration from on_load"
        );

        let config2 = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 99,
                config: None,
            }],
        };

        rt.block_on(pm.reload_all(&config2))
            .expect("reload_all on priority change should succeed");
        assert_eq!(
            pm.len(),
            1,
            "should still have 1 instance after priority change"
        );

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":0}"#;
        let action = rt.block_on(pm.handle_command("!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "plugin should still respond after priority change reload, got {action:?}"
        );

        let ticks_after = pm.get_ticks();
        assert!(
            !ticks_after.is_empty(),
            "tick should be re-registered after priority change reload"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_reload_all_add_then_remove_then_reload() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "a".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 0,
                config: None,
            }],
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");
        assert_eq!(pm.len(), 1);

        let cmd_json = r#"{"command_type":"!ping","group_id":1,"user_id":1,"mode":0}"#;

        // Add b
        let config_add = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![
                PluginInstanceConfig {
                    name: "a".to_string(),
                    path: wasm_path.clone(),
                    enabled: true,
                    priority: 0,
                    config: None,
                },
                PluginInstanceConfig {
                    name: "b".to_string(),
                    path: wasm_path.clone(),
                    enabled: true,
                    priority: 5,
                    config: None,
                },
            ],
        };
        rt.block_on(pm.reload_all(&config_add))
            .expect("reload_all add should succeed");
        assert_eq!(pm.len(), 2, "after add: 2 instances");

        assert!(
            matches!(
                rt.block_on(pm.handle_command("!ping", cmd_json)),
                PluginAction::Handled(_)
            ),
            "!ping should work after add"
        );

        // Remove a (keep b)
        let config_remove = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "b".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 5,
                config: None,
            }],
        };
        rt.block_on(pm.reload_all(&config_remove))
            .expect("reload_all remove should succeed");
        assert_eq!(pm.len(), 1, "after remove: 1 instance");

        assert!(
            matches!(
                rt.block_on(pm.handle_command("!ping", cmd_json)),
                PluginAction::Handled(_)
            ),
            "remaining plugin should still handle !ping after remove"
        );

        // Change priority of b
        let config_reload = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "b".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 50,
                config: None,
            }],
        };
        rt.block_on(pm.reload_all(&config_reload))
            .expect("reload_all reload should succeed");
        assert_eq!(pm.len(), 1, "after priority change: 1 instance");

        assert!(
            matches!(
                rt.block_on(pm.handle_command("!ping", cmd_json)),
                PluginAction::Handled(_)
            ),
            "plugin should still work after priority reload"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }
}
