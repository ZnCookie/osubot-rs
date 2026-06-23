use osubot_core::log_fmt;
use osubot_plugin_sdk::PROTOCOL_VERSION;

use crate::bridge::HostServices;
use crate::types::{PluginAction, PluginMetadata};
use std::time::Duration;
use wasmtime::{Instance, Memory, Module, Store};

/// 10 秒 epoch deadline（与 dispatch() tokio::timeout 一致）。
/// epoch 每 500μs 递增一次，20_000 个 tick = 10 秒。
const EPOCH_DEADLINE_TICKS: u64 = 20_000;

fn read_json_from_memory(
    memory: &Memory,
    store: &Store<HostServices>,
    ptr: u32,
) -> Result<String, String> {
    let mut len_buf = [0u8; 4];
    memory
        .read(store, ptr as usize, &mut len_buf)
        .map_err(|e| format!("memory read failed: {e}"))?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mem_size = (memory.size(store) * 65536) as usize;
    if len > 10 * 1024 * 1024 || 4usize.checked_add(len).is_none_or(|total| total > mem_size) {
        return Err(format!("invalid data length: {len} exceeds memory bounds"));
    }
    let mut data = vec![0u8; len];
    memory
        .read(store, (ptr + 4) as usize, &mut data)
        .map_err(|e| format!("memory read failed: {e}"))?;
    String::from_utf8(data).map_err(|e| format!("invalid UTF-8: {e}"))
}

pub struct PluginInstance {
    pub name: String,
    instance: Instance,
    store: Store<HostServices>,
    metadata: PluginMetadata,
    memory: Memory,
    pub timeout: Duration,
}

#[derive(Clone)]
pub struct PluginInstanceParams {
    pub name: String,
    pub priority: u32,
    pub plugin_config: Option<serde_json::Value>,
    pub timeout: Duration,
}

impl PluginInstance {
    pub fn new(
        linker: &wasmtime::Linker<HostServices>,
        module: &Module,
        params: PluginInstanceParams,
        mut store: Store<HostServices>,
    ) -> Result<Self, String> {
        // 10 秒 epoch deadline（与 dispatch() 中的 tokio::timeout 一致）。
        store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
        let instance = linker
            .instantiate(&mut store, module)
            .map_err(|e| format!("instantiate {}: {e}", params.name))?;

        let memory = instance
            .get_export(&mut store, "memory")
            .and_then(|e| e.into_memory())
            .ok_or("plugin has no memory export")?;

        let metadata_func = instance
            .get_export(&mut store, "plugin_metadata")
            .and_then(|e| e.into_func())
            .ok_or("plugin missing plugin_metadata export")?;

        let metadata_ptr: u32 = metadata_func
            .typed::<(), u32>(&store)
            .map_err(|e| format!("metadata type error: {e}"))?
            .call(&mut store, ())
            .map_err(|e| format!("metadata call failed: {e}"))?;

        if metadata_ptr == 0 {
            return Err("plugin returned null pointer (OOM?)".into());
        }

        let metadata_json = read_json_from_memory(&memory, &store, metadata_ptr)?;
        let metadata: PluginMetadata =
            serde_json::from_str(&metadata_json).map_err(|e| format!("invalid metadata: {e}"))?;

        if metadata.protocol_version > PROTOCOL_VERSION {
            return Err(format!(
                "{}",
                log_fmt!(
                    "instance.plugin_protocol_too_new",
                    plugin_version = metadata.protocol_version,
                    host_version = PROTOCOL_VERSION
                )
            ));
        }

        // Dealloc metadata result
        if let Some(dealloc_func) = instance
            .get_export(&mut store, "dealloc")
            .and_then(|e| e.into_func())
        {
            let mut len_buf = [0u8; 4];
            if memory
                .read(&store, metadata_ptr as usize, &mut len_buf)
                .is_ok()
            {
                let data_len = u32::from_le_bytes(len_buf);
                let _ = dealloc_func
                    .typed::<(u32, u32), ()>(&store)
                    .and_then(|f| f.call(&mut store, (metadata_ptr, 4 + data_len)));
            }
        }

        tracing::info!(name = %params.name, version = %metadata.version, commands = ?metadata.commands, "{}", log_fmt!("instance.plugin_loaded"));

        Ok(Self {
            name: params.name,
            instance,
            store,
            metadata,
            memory,
            timeout: params.timeout,
        })
    }

    pub fn set_instance_idx(&mut self, idx: usize) {
        self.store.data_mut().instance_idx = idx;
    }

    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    pub fn has_export(&mut self, name: &str) -> bool {
        self.instance
            .get_export(&mut self.store, name)
            .and_then(|e| e.into_func())
            .is_some()
    }

    pub fn on_load(&mut self) -> Result<(), String> {
        if !self.has_export("on_load") {
            return Ok(());
        }
        let func = self
            .instance
            .get_export(&mut self.store, "on_load")
            .and_then(|e| e.into_func())
            .ok_or("missing on_load export")?;
        // 10 秒 epoch deadline（与 dispatch() tokio::timeout 一致）
        self.store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
        let ptr: u32 = func
            .typed::<(), u32>(&self.store)
            .map_err(|e| e.to_string())?
            .call(&mut self.store, ())
            .map_err(|e| format!("on_load failed: {e}"))?;
        if ptr != 0 {
            self.dealloc_result(ptr);
        }
        Ok(())
    }

    pub fn on_command(&mut self, cmd_json: &str) -> Result<PluginAction, String> {
        let result_ptr = self.call_with_json("on_command", cmd_json)?;
        if result_ptr == 0 {
            return Err("plugin returned null pointer (OOM?)".into());
        }
        let result_json = read_json_from_memory(&self.memory, &self.store, result_ptr);
        self.dealloc_result(result_ptr);
        serde_json::from_str(&result_json?).map_err(|e| format!("invalid PluginAction: {e}"))
    }

    pub fn on_message(&mut self, msg_json: &str) -> Result<PluginAction, String> {
        let result_ptr = self.call_with_json("on_message", msg_json)?;
        if result_ptr == 0 {
            return Err("plugin returned null pointer (OOM?)".into());
        }
        let result_json = read_json_from_memory(&self.memory, &self.store, result_ptr);
        self.dealloc_result(result_ptr);
        serde_json::from_str(&result_json?).map_err(|e| format!("invalid PluginAction: {e}"))
    }

    pub fn on_tick(&mut self, tick_id: u32) -> Result<(), String> {
        if !self.has_export("on_tick") {
            return Ok(());
        }
        let payload = serde_json::json!({"tick_id": tick_id});
        let ptr = self.call_with_json("on_tick", &payload.to_string())?;
        if ptr != 0 {
            self.dealloc_result(ptr);
        }
        Ok(())
    }

    pub fn on_unload(&mut self) -> Result<(), String> {
        if !self.has_export("on_unload") {
            return Ok(());
        }
        let func = self
            .instance
            .get_export(&mut self.store, "on_unload")
            .and_then(|e| e.into_func())
            .ok_or("missing on_unload export")?;
        // 10 秒 epoch deadline（与 dispatch() tokio::timeout 一致）
        self.store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);
        let ptr: u32 = func
            .typed::<(), u32>(&self.store)
            .map_err(|e| e.to_string())?
            .call(&mut self.store, ())
            .map_err(|e| format!("on_unload failed: {e}"))?;
        if ptr != 0 {
            self.dealloc_result(ptr);
        }
        Ok(())
    }

    fn call_with_json(&mut self, export_name: &str, json: &str) -> Result<u32, String> {
        // 10 秒 epoch deadline（与 dispatch() tokio::timeout 一致）
        self.store.set_epoch_deadline(EPOCH_DEADLINE_TICKS);

        let func = self
            .instance
            .get_export(&mut self.store, export_name)
            .and_then(|e| e.into_func())
            .ok_or_else(|| format!("plugin missing {export_name} export"))?;

        let alloc_fn = self
            .instance
            .get_export(&mut self.store, "alloc")
            .and_then(|e| e.into_func())
            .ok_or("plugin missing alloc export")?;

        let bytes = json.as_bytes();
        let ptr: u32 = alloc_fn
            .typed::<(u32,), u32>(&self.store)
            .map_err(|e| e.to_string())?
            .call(&mut self.store, (bytes.len() as u32,))
            .map_err(|e| format!("alloc failed: {e}"))?;
        // plugin alloc OOM 时返回 null；如果直接写 wasm 内存 offset 0
        // 会破坏 runtime 状态（函数指针 / 长度前缀）。拒绝而不是覆盖。
        if ptr == 0 {
            return Err("plugin alloc returned null pointer (OOM?)".into());
        }

        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .map_err(|e| format!("memory write failed: {e}"))?;

        let call_result: Result<u32, String> = func
            .typed::<(u32, u32), u32>(&self.store)
            .map_err(|e| e.to_string())?
            .call(&mut self.store, (ptr, bytes.len() as u32))
            .map_err(|e| format!("{export_name} call failed: {e}"));

        // Free the input buffer — always, even on call failure
        self.dealloc(ptr, bytes.len() as u32);

        call_result
    }

    fn dealloc_result(&mut self, ptr: u32) {
        let mut len_buf = [0u8; 4];
        if self
            .memory
            .read(&self.store, ptr as usize, &mut len_buf)
            .is_ok()
        {
            let data_len = u32::from_le_bytes(len_buf);
            let total = 4u32.saturating_add(data_len);
            self.dealloc(ptr, total);
        } else {
            tracing::warn!(ptr, "{}", log_fmt!("instance.dealloc_length_prefix_failed"));
        }
    }

    fn dealloc(&mut self, ptr: u32, size: u32) {
        if let Some(func) = self
            .instance
            .get_export(&mut self.store, "dealloc")
            .and_then(|e| e.into_func())
        {
            if let Err(e) = func
                .typed::<(u32, u32), ()>(&self.store)
                .and_then(|f| f.call(&mut self.store, (ptr, size)))
            {
                tracing::warn!(plugin = %self.name, error = %e, "{}", log_fmt!("plugin.dealloc_failed"));
            }
        }
    }
}
