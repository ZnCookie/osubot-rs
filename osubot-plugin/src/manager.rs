use crate::bridge;
use crate::config::{PluginConfig as PluginConfigInput, PluginInstanceConfig};
use crate::instance::{PluginInstance, PluginInstanceParams};
use crate::path::resolve_wasm_path;
use crate::types::PluginError;
use crate::PluginManager;
use crate::{HostServices, DEFAULT_PLUGIN_TIMEOUT_SECS, WASM_MEMORY_LIMIT};
use osubot_core::log_fmt;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wasmtime::StoreLimitsBuilder;
use wasmtime::{Engine, Linker, Module, Store};

impl PluginManager {
    pub async fn new(
        config: &PluginConfigInput,
        services: HostServices,
    ) -> Result<Self, PluginError> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.epoch_interruption(true);
        let engine = Engine::new(&wasm_config).map_err(|e| PluginError::Load(e.to_string()))?;

        let epoch_running = Arc::new(AtomicBool::new(true));
        let epoch_engine = engine.clone();
        let epoch_flag = epoch_running.clone();
        let epoch_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_micros(500));
            while epoch_flag.load(Ordering::SeqCst) {
                interval.tick().await;
                epoch_engine.increment_epoch();
            }
        });
        let mut linker = Linker::<HostServices>::new(&engine);
        bridge::register_host_functions(&mut linker)
            .map_err(|e| PluginError::Load(e.to_string()))?;

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
                tracing::info!("{}", log_fmt!("plugin.disabled", name = &pcfg.name));
                continue;
            }
            if !seen_names.insert(&pcfg.name) {
                return Err(PluginError::Load(format!(
                    "duplicate plugin name: {}",
                    pcfg.name
                )));
            }

            let wasm_path = resolve_wasm_path(&config.dir, &pcfg.path)
                .map_err(|e| PluginError::Load(format!("bad plugin path: {e}")))?;

            let module = Module::from_file(&engine, &wasm_path)
                .map_err(|e| PluginError::Load(format!("load module {}: {e}", pcfg.name)))?;

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
            tracing::info!("{}", log_fmt!("plugin.module_loaded", name = &pcfg.name));
        }

        blueprints.sort_by_key(|b| std::cmp::Reverse(b.params.priority));

        let mut instances: Vec<Option<PluginInstance>> = Vec::new();
        let mut modules: Vec<Module> = Vec::new();
        let mut instance_params: Vec<PluginInstanceParams> = Vec::new();
        let mut wasm_paths_vec: Vec<String> = Vec::new();
        let mut wasm_mtimes: Vec<Option<std::time::SystemTime>> = Vec::new();
        let mut command_map: HashMap<String, Vec<usize>> = HashMap::new();
        let mut on_message_indices: HashSet<usize> = HashSet::new();
        let tick_registry: Arc<Mutex<Vec<crate::types::TickRegistration>>> =
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
                        .memory_size(WASM_MEMORY_LIMIT)
                        .build(),
                },
            );
            store.limiter(|state| &mut state.limiter);

            let mut instance =
                PluginInstance::new(&linker, &blueprint.module, blueprint.params.clone(), store)
                    .map_err(|e| {
                        PluginError::Load(format!("load module {}: {e}", blueprint.params.name))
                    })?;

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

            let name = instance_params
                .last()
                .map_or("unknown", |p| p.name.as_str());
            tracing::info!("{}", log_fmt!("plugin.instantiated", name = name));
        }

        for (sorted_idx, instance) in instances.iter_mut().enumerate() {
            if let Some(inst) = instance {
                inst.set_instance_idx(sorted_idx);
                inst.on_load().map_err(PluginError::Dispatch)?;
                tracing::info!(
                    "{}",
                    log_fmt!("plugin.on_load_completed", name = &inst.name)
                );
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
                .memory_size(WASM_MEMORY_LIMIT)
                .build(),
        };

        let lost_instances = vec![0u32; instances.len()];
        let reload_failures = vec![0u32; instances.len()];

        for indices in command_map.values_mut() {
            indices.sort_by_key(|&i| {
                std::cmp::Reverse(instance_params.get(i).map(|p| p.priority).unwrap_or(0))
            });
        }

        Ok(Self {
            instances,
            command_map,
            tick_registry,
            on_message_indices,
            lost_instances,
            reload_failures,
            lost_instances_threshold: config.lost_instances_threshold,
            reload_failures_threshold: config.reload_failures_threshold,
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
}
