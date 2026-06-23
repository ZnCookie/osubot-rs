mod bridge;
pub mod config;
pub mod instance;
mod types;

mod dispatch;
mod lifecycle;
mod manager;
mod path;
mod reload;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use wasmtime::{Engine, Linker, Module};

pub use bridge::HostServices;
pub use config::{PluginConfig as PluginConfigInput, PluginInstanceConfig};
pub use dispatch::{PluginDispatchPanic, PluginDispatchResult};
use instance::{PluginInstance, PluginInstanceParams};
pub use types::{PluginAction as PluginActionResult, PluginError, TickRegistration};
pub use wasmtime::StoreLimitsBuilder;

const DEFAULT_PLUGIN_TIMEOUT_SECS: u64 = 10;

/// WASM Store 内存上限（字节）
const WASM_MEMORY_LIMIT: usize = 100 * 1024 * 1024;

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
/// Single per-plugin state record, replacing the 7 parallel `Vec`s that
/// previously had to be kept in sync by index across modules.
pub struct PluginSlot {
    /// Currently-loaded instance. `None` after `take_instance` until `put_instance`
    /// is called. A persistent `None` (after a failed reload) marks a slot
    /// as "lost"; the corresponding plugin is considered unavailable.
    pub instance: Option<PluginInstance>,
    /// Compiled WASM module — kept so we can rebuild a `Store` on reload.
    pub module: Module,
    /// Static per-instance parameters (name, priority, timeout, plugin config).
    pub params: PluginInstanceParams,
    /// Absolute path of the `.wasm` file on disk; used for hot-reload mtime
    /// checking and re-loading after a rebuild.
    pub wasm_path: String,
    /// Last-observed mtime of `wasm_path` at the time the slot was loaded /
    /// last reloaded.
    pub wasm_mtime: Option<std::time::SystemTime>,
    /// Consecutive error count (timeouts, panics, dispatch errors). Reset to 0
    /// on a successful reload.
    pub lost_instances: u32,
    /// Consecutive reload-failure count. Reset to 0 on a successful reload.
    pub reload_failures: u32,
}

pub struct PluginManager {
    /// Per-plugin state, indexed by slot position. Length matches the number
    /// of enabled plugin instances configured at startup (and stays
    /// `compact`ed after hot-reload).
    slots: Vec<PluginSlot>,
    command_map: HashMap<String, Vec<usize>>,
    tick_registry: Arc<Mutex<Vec<TickRegistration>>>,
    on_message_indices: HashSet<usize>,
    lost_instances_threshold: u32,
    reload_failures_threshold: u32,
    engine: Engine,
    linker: Linker<HostServices>,
    reload_template: HostServices,
    epoch_running: Arc<AtomicBool>,
    epoch_handle: tokio::task::JoinHandle<()>,
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.epoch_running.store(false, Ordering::SeqCst);
        self.epoch_handle.abort();
    }
}

impl PluginManager {
    /// 暴露给 `osubot` crate 调用以在 tick 超时后提升 epoch deadline。
    /// 注意：仅用于 `record_exec_error` 的 on_tick 超时分支。其它用途请通过更
    /// 窄的封装方法暴露。
    pub fn engine_mut(&mut self) -> &mut wasmtime::Engine {
        &mut self.engine
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{PluginAction, PluginMetadata};
    use std::sync::atomic::AtomicU32;

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
                rt.block_on(osubot_core::Storage::new(":memory:"))
                    .expect("failed to create in-memory storage"),
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

    fn setup_plugin_manager(
        wasm_path: String,
    ) -> (
        tokio::runtime::Runtime,
        Arc<tokio::sync::Mutex<Option<PluginManager>>>,
    ) {
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
            ..Default::default()
        };
        let pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");
        let pm_arc = Arc::new(tokio::sync::Mutex::new(Some(pm)));
        (rt, pm_arc)
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

        let (rt, pm) = setup_plugin_manager(wasm_path);

        assert!(!rt.block_on(async { pm.lock().await.as_ref().unwrap().is_empty() }));
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1
        );

        // Test !ping command (returns Handled)
        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        match action {
            PluginAction::Handled(ref msg) => {
                assert!(msg.contains("pong from WASM plugin"));
            }
            other => panic!("expected Handled, got {other:?}"),
        }

        // Test unknown command passes through
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!unknown", cmd_json));
        assert!(matches!(action, PluginAction::Next));

        // Test ticks were registered (hello-plugin registers a tick in on_load)
        let ticks = rt.block_on(async { pm.lock().await.as_ref().unwrap().get_ticks() });
        assert!(!ticks.is_empty(), "expected at least one tick registration");

        // Test tick handler
        if let Some((plugin_idx, _interval_secs, tick_id)) = ticks.first().copied() {
            rt.block_on(async {
                pm.lock()
                    .await
                    .as_mut()
                    .unwrap()
                    .handle_tick(plugin_idx, tick_id)
                    .await
            });
        }

        // Test shutdown
        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
        assert!(rt.block_on(async { pm.lock().await.as_ref().unwrap().is_empty() }));
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

        let (rt, pm) = setup_plugin_manager(wasm_path);

        // hello-plugin now exports on_message — sends a handled response for "hello" / "你好"
        let msg_json = r#"{"group_id":12345,"user_id":67890,"message":"hello world","mentioned_user_id":null}"#;
        let action = rt.block_on(PluginManager::dispatch_message(&pm, msg_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "expected Handled since hello-plugin has on_message for 'hello', got {action:?}"
        );

        // Message without trigger words should return Next
        let msg_json2 = r#"{"group_id":12345,"user_id":67890,"message":"random text","mentioned_user_id":null}"#;
        let action2 = rt.block_on(PluginManager::dispatch_message(&pm, msg_json2));
        assert!(
            matches!(action2, PluginAction::Next),
            "expected Next for non-matching message, got {action2:?}"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
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

        let (rt, pm) = setup_plugin_manager(wasm_path);

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        match action {
            PluginAction::Handled(ref msg) => {
                assert!(msg.contains("pong from WASM plugin"));
            }
            other => panic!("expected Handled for !ping, got {other:?}"),
        }

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
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
            ..Default::default()
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
        pm.slots[0].instance = None;
        pm.slots[0].lost_instances = 5;

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
            ..Default::default()
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

        let (rt, pm) = setup_plugin_manager(wasm_path.clone());

        // Record the stored mtime
        let initial_mtime =
            rt.block_on(async { pm.lock().await.as_ref().unwrap().slots[0].wasm_mtime });

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
            ..Default::default()
        };

        rt.block_on(async {
            pm.lock()
                .await
                .as_mut()
                .unwrap()
                .reload_all(&same_config)
                .await
        })
        .expect("reload_all should succeed");

        // Verify mtime was updated
        let updated_mtime =
            rt.block_on(async { pm.lock().await.as_ref().unwrap().slots[0].wasm_mtime });
        assert_ne!(
            initial_mtime, updated_mtime,
            "mtime should be updated after reload"
        );

        // Verify the plugin still works after reload
        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        match action {
            PluginAction::Handled(ref msg) => {
                assert!(msg.contains("pong from WASM plugin"));
            }
            other => panic!("expected Handled for !ping after reload, got {other:?}"),
        }

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
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

        let (rt, pm) = setup_plugin_manager(wasm_path.clone());
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1
        );

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
            ..Default::default()
        };

        rt.block_on(async { pm.lock().await.as_mut().unwrap().reload_all(&config2).await })
            .expect("reload_all should succeed");
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            2,
            "should have 2 instances after adding"
        );

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "!ping should still work after reload_all add, got {action:?}"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
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
            ..Default::default()
        };

        let pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");
        let pm = Arc::new(tokio::sync::Mutex::new(Some(pm)));
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            2
        );

        let config2 = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![PluginInstanceConfig {
                name: "hello2".to_string(),
                path: wasm_path.clone(),
                enabled: true,
                priority: 10,
                config: None,
            }],
            ..Default::default()
        };

        rt.block_on(async { pm.lock().await.as_mut().unwrap().reload_all(&config2).await })
            .expect("reload_all should succeed");
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1,
            "should have 1 instance after removal"
        );

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "remaining plugin should still handle !ping after reload_all remove, got {action:?}"
        );

        let action2 = rt.block_on(PluginManager::dispatch_command(&pm, "!hello", cmd_json));
        assert!(
            matches!(action2, PluginAction::Handled(_)),
            "!hello should also still work after reload_all remove, got {action2:?}"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
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

        let (rt, pm) = setup_plugin_manager(wasm_path.clone());
        assert!(!rt.block_on(async { pm.lock().await.as_ref().unwrap().is_empty() }));

        let config_empty = PluginConfigInput {
            dir: ".".to_string(),
            instances: vec![],
            ..Default::default()
        };

        rt.block_on(async {
            pm.lock()
                .await
                .as_mut()
                .unwrap()
                .reload_all(&config_empty)
                .await
        })
        .expect("reload_all should succeed");
        assert!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().is_empty() }),
            "PM should be empty after removing all plugins"
        );
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            0
        );

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Next),
            "no plugins left, should pass through, got {action:?}"
        );

        // shutdown on empty PM should not panic
        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
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

        let (rt, pm) = setup_plugin_manager(wasm_path.clone());
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1
        );

        let ticks_before = rt.block_on(async { pm.lock().await.as_ref().unwrap().get_ticks() });
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
            ..Default::default()
        };

        rt.block_on(async { pm.lock().await.as_mut().unwrap().reload_all(&config2).await })
            .expect("reload_all on priority change should succeed");
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1,
            "should still have 1 instance after priority change"
        );

        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "plugin should still respond after priority change reload, got {action:?}"
        );

        let ticks_after = rt.block_on(async { pm.lock().await.as_ref().unwrap().get_ticks() });
        assert!(
            !ticks_after.is_empty(),
            "tick should be re-registered after priority change reload"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
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

        let (rt, pm) = setup_plugin_manager(wasm_path.clone());
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1
        );

        let cmd_json = r#"{"command_type":"!ping","group_id":1,"user_id":1,"mode":"osu"}"#;

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
            ..Default::default()
        };
        rt.block_on(async {
            pm.lock()
                .await
                .as_mut()
                .unwrap()
                .reload_all(&config_add)
                .await
        })
        .expect("reload_all add should succeed");
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            2,
            "after add: 2 instances"
        );

        assert!(
            matches!(
                rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json)),
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
            ..Default::default()
        };
        rt.block_on(async {
            pm.lock()
                .await
                .as_mut()
                .unwrap()
                .reload_all(&config_remove)
                .await
        })
        .expect("reload_all remove should succeed");
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1,
            "after remove: 1 instance"
        );

        assert!(
            matches!(
                rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json)),
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
            ..Default::default()
        };
        rt.block_on(async {
            pm.lock()
                .await
                .as_mut()
                .unwrap()
                .reload_all(&config_reload)
                .await
        })
        .expect("reload_all reload should succeed");
        assert_eq!(
            rt.block_on(async { pm.lock().await.as_ref().unwrap().len() }),
            1,
            "after priority change: 1 instance"
        );

        assert!(
            matches!(
                rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json)),
                PluginAction::Handled(_)
            ),
            "plugin should still work after priority reload"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
    }

    #[test]
    fn test_reload_instance_rebuilds_command_map() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let (rt, pm) = setup_plugin_manager(wasm_path);

        // Verify initial command_map state
        assert!(
            rt.block_on(async {
                pm.lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .command_map
                    .contains_key("!ping")
            }),
            "!ping should be in command_map after load"
        );
        assert!(
            rt.block_on(async {
                pm.lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .command_map
                    .contains_key("!hello")
            }),
            "!hello should be in command_map after load"
        );

        // Reload the instance
        rt.block_on(async { pm.lock().await.as_mut().unwrap().reload_instance(0) })
            .expect("reload_instance should succeed");

        // Verify command_map still contains the plugin's commands
        assert!(
            rt.block_on(async {
                pm.lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .command_map
                    .contains_key("!ping")
            }),
            "!ping should still be in command_map after reload_instance"
        );
        assert!(
            rt.block_on(async {
                pm.lock()
                    .await
                    .as_ref()
                    .unwrap()
                    .command_map
                    .contains_key("!hello")
            }),
            "!hello should still be in command_map after reload_instance"
        );

        let ping_indices = rt.block_on(async {
            pm.lock()
                .await
                .as_ref()
                .unwrap()
                .command_map
                .get("!ping")
                .cloned()
                .unwrap()
        });
        assert_eq!(ping_indices.len(), 1, "!ping should have exactly 1 entry");
        assert_eq!(ping_indices[0], 0, "!ping entry should point to index 0");

        // Verify the plugin still works
        let cmd_json = r#"{"command_type":"!ping","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(&pm, "!ping", cmd_json));
        assert!(
            matches!(action, PluginAction::Handled(_)),
            "!ping should still work after reload_instance, got {action:?}"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
    }

    #[test]
    fn test_reload_instance_failure_cap() {
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
            ..Default::default()
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        // Set reload_failures to 3 (at cap)
        pm.slots[0].reload_failures = 3;

        // reload_instance should return Err due to too many failures
        let result = pm.reload_instance(0);
        assert!(
            result.is_err(),
            "reload_instance should return Err when reload_failures >= 3"
        );
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("too many times"),
            "error message should mention 'too many times', got: {err_msg}"
        );

        // Verify the instance is still present (not replaced)
        assert!(
            pm.slots[0].instance.is_some(),
            "instance should still be present after capped reload attempt"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_dispatch_command_unknown_returns_next() {
        let wasm_path = match find_hello_plugin_wasm() {
            Some(p) => p,
            None => {
                eprintln!("wasm32-unknown-unknown target not available, skipping");
                return;
            }
        };

        let (rt, pm) = setup_plugin_manager(wasm_path);

        let cmd_json =
            r#"{"command_type":"!nonexistent","group_id":12345,"user_id":67890,"mode":"osu"}"#;
        let action = rt.block_on(PluginManager::dispatch_command(
            &pm,
            "!nonexistent",
            cmd_json,
        ));
        assert!(
            matches!(action, PluginAction::Next),
            "unknown command should return Next, got {action:?}"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
    }

    #[test]
    fn test_dispatch_message_empty_returns_next() {
        let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
        let _guard = rt.enter();
        let services = make_services(&rt);
        let config = PluginConfigInput::default();

        let pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");
        let pm = Arc::new(tokio::sync::Mutex::new(Some(pm)));

        let msg_json =
            r#"{"group_id":12345,"user_id":67890,"message":"hello","mentioned_user_id":null}"#;
        let action = rt.block_on(PluginManager::dispatch_message(&pm, msg_json));
        assert!(
            matches!(action, PluginAction::Next),
            "dispatch_message with no on_message plugins should return Next, got {action:?}"
        );

        rt.block_on(async { pm.lock().await.as_mut().unwrap().shutdown().await });
        drop(_guard);
    }

    #[test]
    fn test_complete_exec_timeout_increments_lost_instances() {
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
            ..Default::default()
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        pm.lost_instances_threshold = 100;

        let _instance = pm.take_instance(0).expect("instance should exist");
        let action = pm.complete_exec(
            0,
            "hello",
            None,
            "command",
            Err(PluginDispatchPanic::Timeout),
        );
        assert!(
            matches!(action, PluginAction::Next),
            "complete_exec with Timeout should return Next, got {action:?}"
        );
        assert_eq!(
            pm.slots[0].lost_instances, 1,
            "slots[0].lost_instances should be 1 after one timeout"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn test_complete_exec_threshold_triggers_reload() {
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
            ..Default::default()
        };

        let mut pm = rt
            .block_on(PluginManager::new(&config, services))
            .expect("PluginManager::new failed");

        pm.lost_instances_threshold = 1;

        let _instance = pm.take_instance(0).expect("instance should exist");
        let action = pm.complete_exec(
            0,
            "hello",
            None,
            "command",
            Err(PluginDispatchPanic::Timeout),
        );
        assert!(
            matches!(action, PluginAction::Next),
            "complete_exec with Timeout should return Next, got {action:?}"
        );
        assert_eq!(
            pm.slots[0].lost_instances, 0,
            "slots[0].lost_instances should be reset to 0 after successful reload"
        );
        assert_eq!(
            pm.slots[0].reload_failures, 0,
            "slots[0].reload_failures should be 0 after successful reload"
        );
        assert!(
            pm.slots[0].instance.is_some(),
            "instance should be restored after successful reload"
        );

        rt.block_on(pm.shutdown());
        drop(_guard);
    }

    #[test]
    fn host_call_impl_rejects_null_alloc() {
        use crate::bridge::register_host_functions;
        use wasmtime::{Config, Engine, Linker, Module, Store};

        // Wasm 模拟一个 plugin：alloc 始终返回 0（模拟 OOM），
        // host_call 透传 4 个 i32 参数到宿主函数 `osubot.host_call_impl`。
        let wat = r#"
            (module
                (import "osubot" "host_call_impl" (func $host_call_impl (param i32 i32 i32 i32) (result i32)))
                (memory (export "memory") 1)
                (func (export "alloc") (param $sz i32) (result i32)
                    i32.const 0)
                (func (export "host_call")
                    (param $name_ptr i32) (param $name_len i32)
                    (param $payload_ptr i32) (param $payload_len i32)
                    (result i32)
                    local.get $name_ptr
                    local.get $name_len
                    local.get $payload_ptr
                    local.get $payload_len
                    call $host_call_impl))
        "#;
        let wasm = wat::parse_str(wat).expect("parse wat");
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).unwrap();
        let module = Module::from_binary(&engine, &wasm).unwrap();

        let services = HostServices::default_for_test();
        let mut store: Store<HostServices> = Store::new(&engine, services);
        store.set_fuel(u64::MAX).unwrap();
        let mut linker = Linker::<HostServices>::new(&engine);
        register_host_functions(&mut linker).unwrap();
        let pre = linker.instantiate_pre(&module).unwrap();
        let inst = pre.instantiate(&mut store).unwrap();
        let f = inst
            .get_typed_func::<(u32, u32, u32, u32), u32>(&mut store, "host_call")
            .unwrap();
        let result = f.call(&mut store, (0, 0, 0, 0));
        assert!(result.is_err(), "must reject null alloc: {result:?}");
    }
}
