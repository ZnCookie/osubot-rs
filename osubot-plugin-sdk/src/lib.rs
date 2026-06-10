mod types;
pub use types::*;

use core::ptr;
use core::slice;
use core::str;

fn host_call(name: &str, payload: &str) -> Result<String, String> {
    let name_bytes = name.as_bytes();
    let payload_bytes = payload.as_bytes();
    let result_ptr = unsafe {
        host_call_impl(
            name_bytes.as_ptr(),
            name_bytes.len(),
            payload_bytes.as_ptr(),
            payload_bytes.len(),
        )
    };
    if result_ptr.is_null() {
        return Err("host call returned null".into());
    }
    unsafe {
        let len = ptr::read_unaligned(result_ptr as *const u32) as usize;
        let total_size = (4 + len) as u32;
        let data = slice::from_raw_parts(result_ptr.add(4), len);
        let s = match str::from_utf8(data) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                dealloc(result_ptr as *mut u8, total_size);
                return Err("host call: invalid UTF-8".to_string());
            }
        };
        dealloc(result_ptr as *mut u8, total_size);
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&s) {
            if let Some(ok_val) = val.get("ok") {
                return Ok(match ok_val {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                });
            }
            if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
                return Err(err.to_string());
            }
        }
        Ok(s)
    }
}

#[link(wasm_import_module = "osubot")]
extern "C" {
    fn host_call_impl(
        name_ptr: *const u8,
        name_len: usize,
        payload_ptr: *const u8,
        payload_len: usize,
    ) -> *const u8;
}

impl PluginContext {
    pub fn send_group_msg(&self, group_id: i64, text: &str) -> Result<(), String> {
        let payload = serde_json::json!({"group_id": group_id, "text": text});
        host_call("send_group_msg", &payload.to_string()).map(|_| ())
    }

    pub fn send_image(&self, group_id: i64, jpeg_b64: &str) -> Result<(), String> {
        let payload = serde_json::json!({"group_id": group_id, "jpeg_base64": jpeg_b64});
        host_call("send_image", &payload.to_string()).map(|_| ())
    }

    pub fn http_request(&self, url: &str) -> Result<String, String> {
        let payload = serde_json::json!({"url": url});
        host_call("http_request", &payload.to_string())
    }

    pub fn db_get_binding(&self, qq: i64) -> Result<Option<(i64, String)>, String> {
        let payload = serde_json::json!({"qq": qq});
        let json = host_call("db_get_binding", &payload.to_string())?;
        if json == "null" {
            return Ok(None);
        }
        let v: serde_json::Value = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let user_id = v["user_id"].as_i64().ok_or("missing user_id")?;
        let username = v["username"].as_str().ok_or("missing username")?;
        Ok(Some((user_id, username.to_string())))
    }

    pub fn osu_api_fetch_user(&self, username: &str, mode: u8) -> Result<String, String> {
        let payload = serde_json::json!({"username": username, "mode": mode});
        host_call("osu_api_fetch_user", &payload.to_string())
    }

    pub fn register_tick(&self, name: &str, interval_secs: u64) -> Result<u32, String> {
        let payload = serde_json::json!({"name": name, "interval_secs": interval_secs});
        let json = host_call("register_tick", &payload.to_string())?;
        let result: u32 = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        Ok(result)
    }

    pub fn get_plugin_config(&self) -> Result<serde_json::Value, String> {
        let json = host_call("get_plugin_config", "{}")?;
        serde_json::from_str(&json).map_err(|e| e.to_string())
    }
}

/// Allocate a buffer of `size` bytes and return a raw pointer.
///
/// # Safety
///
/// The caller must ensure the returned pointer is later freed via [`dealloc`]
/// with the same size. The memory is uninitialized.
pub unsafe fn alloc(size: u32) -> *mut u8 {
    let mut buf = Vec::with_capacity(size as usize);
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf);
    ptr
}

/// Deallocate a buffer previously allocated by [`alloc`].
///
/// # Safety
///
/// `ptr` must have been returned by a previous call to [`alloc`] and `size`
/// must match the size passed to that call.
pub unsafe fn dealloc(ptr: *mut u8, size: u32) {
    let _ = Vec::from_raw_parts(ptr, 0, size as usize);
}

pub fn serialize_return<T: serde::Serialize>(val: &T) -> *const u8 {
    let json = serde_json::to_string(val)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialization failed: {e}\"}}"));
    let bytes = json.into_bytes();
    let len = bytes.len();
    let mut buf = Vec::with_capacity(4 + len);
    buf.extend_from_slice(&(len as u32).to_le_bytes());
    buf.extend_from_slice(&bytes);
    let ptr = buf.as_ptr();
    core::mem::forget(buf);
    ptr
}
