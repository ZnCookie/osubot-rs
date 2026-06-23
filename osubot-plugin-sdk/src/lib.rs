mod types;
pub use types::*;

use osubot_game_mode::GameMode;

extern crate alloc;

use core::ptr;
use core::slice;
use core::str;

/// Maximum response size from host calls (64 MB).
const MAX_HOST_CALL_RESPONSE: usize = 64 * 1024 * 1024;

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
    // SAFETY: wasmtime 线性内存模型保证 host_call_impl 返回的指针
    // 指向至少 4 字节的已分配内存（长度前缀），且在当前调用期间有效。
    unsafe {
        let len = ptr::read_unaligned(result_ptr as *const u32) as usize;
        if len > MAX_HOST_CALL_RESPONSE {
            let total_size = 4u32.saturating_add(len as u32);
            dealloc(result_ptr, total_size);
            return Err(format!(
                "host call response exceeds {}MB limit (len: {})",
                MAX_HOST_CALL_RESPONSE / (1024 * 1024),
                len
            ));
        }
        let total_size = 4u32
            .checked_add(len as u32)
            .ok_or_else(|| "host call response size overflow".to_string())?;
        let data = slice::from_raw_parts(result_ptr.add(4), len);
        let s = match str::from_utf8(data) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                dealloc(result_ptr, total_size);
                return Err("host call: invalid UTF-8".to_string());
            }
        };
        dealloc(result_ptr, total_size);
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
    ) -> *mut u8;
}

impl PluginContext {
    pub fn send_group_msg(&self, group_id: i64, text: &str) -> Result<(), String> {
        let payload = serde_json::json!({"group_id": group_id, "text": text});
        host_call("send_group_msg", &payload.to_string()).map(|_| ())
    }

    pub fn send_group_msg_segments(
        &self,
        group_id: i64,
        segments: serde_json::Value,
    ) -> Result<(), String> {
        let payload = serde_json::json!({"group_id": group_id, "segments": segments});
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

    pub fn http_request_with_method(
        &self,
        url: &str,
        method: &str,
        body: Option<&str>,
    ) -> Result<String, String> {
        let payload = match body {
            Some(b) => serde_json::json!({"url": url, "method": method, "body": b}),
            None => serde_json::json!({"url": url, "method": method}),
        };
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

    pub fn osu_api_fetch_user(&self, username: &str, mode: GameMode) -> Result<String, String> {
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
/// Uses the global allocator directly to ensure layout consistency with
/// [`dealloc`]. Unlike `Vec::with_capacity`, no internal over-allocation
/// can occur that would produce a layout mismatch on deallocation.
///
/// # OOM behavior
///
/// Returns [`core::ptr::null_mut`] on allocation failure or when `size` is
/// `0` (a 1-byte layout is used to keep the call safe). **Callers MUST
/// check the returned pointer for null and propagate as a recoverable
/// error** — writing to wasm linear memory at offset 0 will corrupt the
/// runtime state.
///
/// # Safety
///
/// The caller must ensure the returned pointer is later freed via [`dealloc`]
/// with the same size. The memory is uninitialized.
pub unsafe fn alloc(size: u32) -> *mut u8 {
    let n = size.max(1) as usize;
    let layout = match alloc::alloc::Layout::from_size_align(n, 4)
        .or_else(|_| alloc::alloc::Layout::array::<u8>(n))
    {
        Ok(l) => l,
        Err(_) => return core::ptr::null_mut(),
    };
    let ptr = alloc::alloc::alloc(layout);
    if ptr.is_null() {
        return core::ptr::null_mut();
    }
    ptr
}

/// Deallocate a buffer previously allocated by [`alloc`].
///
/// # Safety
///
/// `ptr` must have been returned by a previous call to [`alloc`] and `size`
/// must match the size passed to that call.
pub unsafe fn dealloc(ptr: *mut u8, size: u32) {
    let n = size.max(1) as usize;
    let layout = match alloc::alloc::Layout::from_size_align(n, 4)
        .or_else(|_| alloc::alloc::Layout::array::<u8>(n))
    {
        Ok(l) => l,
        Err(_) => return,
    };
    alloc::alloc::dealloc(ptr, layout);
}

/// RAII 包装：Drop 时自动释放线性内存。指针非空保证。
///
/// 用于 [`serialize_return`] 的返回值。FFI 导出函数应调用 [`PluginReturn::into_raw`]
/// 获取裸指针返回给宿主——宿主读取后通过 `dealloc` 释放。
pub struct PluginReturn {
    ptr: *mut u8,
    layout: alloc::alloc::Layout,
}

impl PluginReturn {
    /// 返回供 FFI 使用的裸指针。
    ///
    /// 调用后所有权转移给宿主，宿主必须通过 `dealloc(ptr, 4 + len)` 释放。
    /// 若不调用此方法，[`PluginReturn`] 在 Drop 时自动释放。
    ///
    /// 与 `Box::into_raw` 类似，此方法是 safe 的：内存泄漏在 Rust 中是 safe 的。
    pub fn into_raw(self) -> *const u8 {
        let ptr = self.ptr;
        core::mem::forget(self);
        ptr
    }
}

impl Drop for PluginReturn {
    fn drop(&mut self) {
        // SAFETY: ptr 由 alloc::alloc::alloc 返回，layout 与分配时一致。
        unsafe { alloc::alloc::dealloc(self.ptr, self.layout) }
    }
}

/// 将 T 序列化为 JSON，在堆上分配长度为 4 + json.len() 的缓冲区，
/// 前 4 字节为小端长度前缀，后续为 JSON 字节。
///
/// 返回 [`PluginReturn`] RAII 类型，Drop 时自动释放。若需返回裸指针给宿主，
/// 调用 [`PluginReturn::into_raw`]。
///
/// # 用途
///
/// 此函数设计为从 `extern "C"` 导出函数中使用：
/// ```ignore
/// pub extern "C" fn plugin_metadata() -> *const u8 {
///     serialize_return(&meta).map(|r| r.into_raw()).unwrap_or(core::ptr::null())
/// }
/// ```
pub fn serialize_return<T: serde::Serialize>(val: &T) -> Option<PluginReturn> {
    let json = serde_json::to_string(val)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialization failed: {e}\"}}"));
    let bytes = json.into_bytes();
    let len = bytes.len();
    if len > u32::MAX as usize {
        return None;
    }
    let total = 4usize.checked_add(len)?;
    let layout = alloc::alloc::Layout::from_size_align(total, 4)
        .or_else(|_| alloc::alloc::Layout::array::<u8>(total))
        .ok()?;
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: layout 由 from_size_align(total, 4) 创建，ptr 由 alloc(layout) 返回。
    // copy_nonoverlapping 写入 len_le (4 bytes) + bytes (len bytes) = total bytes，
    // 不超出分配范围。bytes.as_ptr() 和 ptr.add(4) 不重叠。
    unsafe {
        let len_le = (len as u32).to_le_bytes();
        core::ptr::copy_nonoverlapping(len_le.as_ptr(), ptr, 4);
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.add(4), len);
    }
    Some(PluginReturn { ptr, layout })
}
