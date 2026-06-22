use std::fmt;
use std::io::Read;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use wasmtime::Result as WasmResult;
use wasmtime::{Caller, Linker, StoreLimits};

use crate::types::TickRegistration;
use osubot_core::strings::user_str;

// 信任模型：所有宿主函数默认信任 WASM 插件的调用意图。
// 插件可以发送消息到任意群、发起任意 HTTP 请求、查询任意绑定——
// 这些能力是功能而非漏洞。部署者需自行审查插件行为，对插件的操作后果负责。
// 宿主仅提供进程级保护（wasmtime 沙箱 + 限流 + 超时），不做应用层权限控制。

/// HTTP 响应体最大大小限制（10MB），防止恶意或意外的大响应耗尽进程内存。
const MAX_HTTP_RESPONSE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug)]
pub enum BridgeError {
    JsonParse(serde_json::Error),
    MissingField(String),
    InvalidMode(String),
    HttpRequest(String),
    Database(String),
    SendMsg(String),
    UnknownHostCall(String),
    Validation(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use osubot_core::log_fmt;
        match self {
            BridgeError::JsonParse(e) => {
                write!(f, "{}", log_fmt!("bridge.err_json_parse", detail = e))
            }
            BridgeError::MissingField(field) => {
                write!(f, "{}", log_fmt!("bridge.err_missing_field", field = field))
            }
            BridgeError::InvalidMode(mode) => {
                write!(f, "{}", log_fmt!("bridge.err_invalid_mode", mode = mode))
            }
            BridgeError::HttpRequest(detail) => {
                write!(
                    f,
                    "{}",
                    log_fmt!("bridge.err_http_request", detail = detail)
                )
            }
            BridgeError::Database(detail) => {
                write!(f, "{}", log_fmt!("bridge.err_database", detail = detail))
            }
            BridgeError::SendMsg(detail) => {
                write!(f, "{}", log_fmt!("bridge.err_send_msg", detail = detail))
            }
            BridgeError::UnknownHostCall(detail) => {
                write!(
                    f,
                    "{}",
                    log_fmt!("bridge.err_unknown_host", detail = detail)
                )
            }
            BridgeError::Validation(detail) => {
                write!(f, "{}", log_fmt!("bridge.err_validation", detail = detail))
            }
        }
    }
}

impl std::error::Error for BridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BridgeError::JsonParse(e) => Some(e),
            _ => None,
        }
    }
}

impl From<BridgeError> for String {
    fn from(e: BridgeError) -> String {
        e.to_string()
    }
}

#[derive(Clone)]
pub struct HostServices {
    pub http_client: reqwest::Client,
    pub blocking_http_client: reqwest::blocking::Client,
    pub rate_limiter: Arc<osubot_core::RateLimiter>,
    pub oauth: Arc<osubot_core::OauthTokenCache>,
    pub storage: Arc<osubot_core::Storage>,
    pub send_msg_fn: Arc<dyn Fn(i64, serde_json::Value) -> Result<(), String> + Send + Sync>,
    pub runtime_handle: tokio::runtime::Handle,
    pub instance_idx: usize,
    pub tick_registry: Arc<std::sync::Mutex<Vec<TickRegistration>>>,
    pub tick_id_counter: Arc<AtomicU32>,
    pub instance_config: Option<serde_json::Value>,
    pub limiter: StoreLimits,
}

/// 测试用单线程 runtime（`OnceLock` 共享），用于构造 `Storage` 等需要 tokio 上下文的字段。
/// 仅在 `cfg(test)` 下使用。
#[cfg(test)]
fn test_runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build test tokio runtime")
    })
}

#[cfg(test)]
impl Default for HostServices {
    fn default() -> Self {
        let runtime = test_runtime();
        // `RateLimiter::new()` 内部调用 `tokio::spawn`，需要当前线程在 runtime 上下文里。
        let _guard = runtime.enter();
        let storage = runtime
            .block_on(async { osubot_core::Storage::new(":memory:").await })
            .expect("failed to create in-memory storage");
        Self {
            http_client: reqwest::Client::new(),
            blocking_http_client: reqwest::blocking::Client::new(),
            rate_limiter: Arc::new(osubot_core::RateLimiter::new()),
            oauth: Arc::new(osubot_core::OauthTokenCache::new(
                String::new(),
                String::new(),
            )),
            storage: Arc::new(storage),
            send_msg_fn: Arc::new(|_group_id, _text| Ok(())),
            runtime_handle: runtime.handle().clone(),
            instance_idx: 0,
            tick_registry: Arc::new(std::sync::Mutex::new(Vec::new())),
            tick_id_counter: Arc::new(AtomicU32::new(0)),
            instance_config: None,
            limiter: StoreLimits::default(),
        }
    }
}

#[cfg(test)]
impl HostServices {
    /// 与 `Self::default()` 等价；保留该名称以明确测试意图。
    #[must_use]
    pub fn default_for_test() -> Self {
        Self::default()
    }
}

pub fn register_host_functions(linker: &mut Linker<HostServices>) -> Result<(), wasmtime::Error> {
    linker.func_wrap(
        "osubot",
        "host_call_impl",
        |mut caller: Caller<'_, HostServices>,
         name_ptr: u32,
         name_len: u32,
         payload_ptr: u32,
         payload_len: u32|
         -> WasmResult<u32> {
            let memory = caller
                .get_export("memory")
                .and_then(|e| e.into_memory())
                .ok_or_else(|| wasmtime::format_err!("no memory export"))?;

            if name_len > 1024 * 1024 {
                return Err(wasmtime::format_err!("name length too large: {name_len}"));
            }
            let mut buf = vec![0u8; name_len as usize];
            memory
                .read(&mut caller, name_ptr as usize, &mut buf)
                .map_err(|e| wasmtime::format_err!("read name: {e}"))?;
            let name = String::from_utf8(buf)
                .map_err(|_| wasmtime::format_err!("invalid UTF-8 in name"))?;

            if payload_len > 1024 * 1024 {
                return Err(wasmtime::format_err!(
                    "payload length too large: {payload_len}"
                ));
            }
            let mut buf = vec![0u8; payload_len as usize];
            memory
                .read(&mut caller, payload_ptr as usize, &mut buf)
                .map_err(|e| wasmtime::format_err!("read payload: {e}"))?;
            let payload_str = String::from_utf8(buf)
                .map_err(|_| wasmtime::format_err!("invalid UTF-8 in payload"))?;

            let result_json = match dispatch_host_call(caller.data(), &name, &payload_str) {
                Ok(val) => serde_json::json!({"ok": val}).to_string(),
                Err(e) => serde_json::json!({"error": e.to_string()}).to_string(),
            };

            let result_bytes = result_json.into_bytes();
            let result_len = result_bytes.len() as u32;

            let alloc_fn = caller
                .get_export("alloc")
                .and_then(|e| e.into_func())
                .ok_or_else(|| wasmtime::format_err!("no alloc export"))?;

            let alloc_size = result_bytes.len() as u32;
            let alloc_total = alloc_size
                .checked_add(4)
                .ok_or_else(|| wasmtime::format_err!("alloc size overflow"))?;
            let result_ptr: u32 = alloc_fn
                .typed::<(u32,), u32>(&caller)
                .map_err(|_| wasmtime::format_err!("alloc type mismatch"))?
                .call(&mut caller, (alloc_total,))
                .map_err(|e| wasmtime::format_err!("alloc call: {e}"))?;

            // 与 osubot-plugin/src/instance.rs:231-233 对齐：plugin alloc OOM 时
            // 返回 0；直接写 wasm 内存 offset 0 会破坏 runtime 状态。拒绝而不是覆盖。
            if result_ptr == 0 {
                return Err(wasmtime::format_err!(
                    "plugin alloc returned null pointer (OOM?)"
                ));
            }

            if let Err(e) =
                memory.write(&mut caller, result_ptr as usize, &result_len.to_le_bytes())
            {
                if let Some(dealloc_fn) = caller.get_export("dealloc").and_then(|e| e.into_func()) {
                    let _ = dealloc_fn
                        .typed::<(u32, u32), ()>(&caller)
                        .and_then(|f| f.call(&mut caller, (result_ptr, alloc_total)));
                }
                return Err(wasmtime::format_err!("write length: {e}"));
            }
            if let Err(e) = memory.write(&mut caller, (result_ptr + 4) as usize, &result_bytes) {
                if let Some(dealloc_fn) = caller.get_export("dealloc").and_then(|e| e.into_func()) {
                    let _ = dealloc_fn
                        .typed::<(u32, u32), ()>(&caller)
                        .and_then(|f| f.call(&mut caller, (result_ptr, alloc_total)));
                }
                return Err(wasmtime::format_err!("write data: {e}"));
            }

            Ok(result_ptr)
        },
    )?;

    Ok(())
}

fn get_field(v: &serde_json::Value, name: &str) -> Result<String, BridgeError> {
    v.get(name)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| BridgeError::MissingField(name.to_string()))
}

fn parse_payload(payload: &str) -> Result<serde_json::Value, BridgeError> {
    serde_json::from_str(payload).map_err(BridgeError::JsonParse)
}

fn acquire_rate_limiter(services: &HostServices) -> bool {
    let rl = services.rate_limiter.clone();
    tokio::task::block_in_place(|| {
        services.runtime_handle.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(5), rl.acquire())
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false)
        })
    })
}

fn send_msg_sync(
    services: &HostServices,
    group_id: i64,
    message: serde_json::Value,
) -> Result<(), BridgeError> {
    let send_fn = services.send_msg_fn.clone();
    tokio::task::block_in_place(|| send_fn(group_id, message).map_err(BridgeError::SendMsg))
}

fn parse_fetch_user_mode(payload: &str) -> Result<(String, osubot_types::GameMode), BridgeError> {
    let v = parse_payload(payload)?;
    let username = get_field(&v, "username")?;
    let mode = serde_json::from_value(v["mode"].clone())
        .map_err(|e| BridgeError::InvalidMode(e.to_string()))?;
    Ok((username, mode))
}

fn dispatch_host_call(
    services: &HostServices,
    name: &str,
    payload: &str,
) -> Result<String, BridgeError> {
    match name {
        "send_group_msg" => {
            let v = parse_payload(payload)?;
            let group_id = v["group_id"]
                .as_i64()
                .ok_or_else(|| BridgeError::MissingField("group_id".into()))?;
            // 支持两种格式:
            // 1. 纯文本: {"group_id": 123, "text": "hello"}
            // 2. 富文本(segments): {"group_id": 123, "segments": [{"type": "text", "data": {"text": "hello"}}, ...]}
            let message = if let Some(segments) = v.get("segments").and_then(|s| s.as_array()) {
                serde_json::Value::Array(segments.clone())
            } else {
                let text = get_field(&v, "text")?;
                serde_json::Value::String(text)
            };
            if !acquire_rate_limiter(services) {
                return Err(BridgeError::SendMsg(
                    user_str("bridge.rate_limit_send_msg").into(),
                ));
            }
            send_msg_sync(services, group_id, message)?;
            Ok("{}".to_string())
        }
        "send_image" => {
            let v = parse_payload(payload)?;
            let group_id = v["group_id"]
                .as_i64()
                .ok_or_else(|| BridgeError::MissingField("group_id".into()))?;
            let jpeg_b64 = get_field(&v, "jpeg_base64")?;
            if !acquire_rate_limiter(services) {
                return Err(BridgeError::SendMsg(
                    user_str("bridge.rate_limit_send_msg").into(),
                ));
            }
            let image_segment = serde_json::json!([{
                "type": "image",
                "data": {
                    "file": format!("base64://{}", jpeg_b64)
                }
            }]);
            send_msg_sync(services, group_id, image_segment)?;
            Ok("{}".to_string())
        }
        "http_request" => {
            let v = parse_payload(payload)?;
            let url = get_field(&v, "url")?;
            let method_str = v["method"].as_str().unwrap_or("GET");
            let method = reqwest::Method::from_bytes(method_str.as_bytes())
                .map_err(|e| BridgeError::HttpRequest(format!("invalid HTTP method: {e}")))?;
            if !acquire_rate_limiter(services) {
                return Err(BridgeError::HttpRequest(
                    user_str("bridge.rate_limit_http").into(),
                ));
            }
            let mut req = services
                .blocking_http_client
                .request(method, &url)
                .timeout(std::time::Duration::from_secs(30));
            if let Some(body) = v.get("body").and_then(|b| b.as_str()) {
                req = req.body(body.to_string());
            }
            let mut response = req
                .send()
                .map_err(|e| BridgeError::HttpRequest(format!("HTTP request failed: {e}")))?;

            // 检查 Content-Length 头，提前拒绝超大响应
            if let Some(len) = response.content_length() {
                if len > MAX_HTTP_RESPONSE_BYTES {
                    return Err(BridgeError::HttpRequest(format!(
                        "HTTP response exceeds {}MB limit (Content-Length: {} bytes)",
                        MAX_HTTP_RESPONSE_BYTES / (1024 * 1024),
                        len
                    )));
                }
            }

            // 流式读取响应体，限制最大 10MB
            let mut body = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                let n = response
                    .read(&mut buf)
                    .map_err(|e| BridgeError::HttpRequest(format!("read response: {e}")))?;
                if n == 0 {
                    break;
                }
                if body.len() as u64 + n as u64 > MAX_HTTP_RESPONSE_BYTES {
                    return Err(BridgeError::HttpRequest(format!(
                        "HTTP response exceeds {}MB limit",
                        MAX_HTTP_RESPONSE_BYTES / (1024 * 1024)
                    )));
                }
                body.extend_from_slice(&buf[..n]);
            }
            let body = String::from_utf8(body)
                .map_err(|e| BridgeError::HttpRequest(format!("invalid UTF-8 in response: {e}")))?;
            Ok(body)
        }
        "db_get_binding" => {
            let v = parse_payload(payload)?;
            let qq = v["qq"]
                .as_i64()
                .ok_or_else(|| BridgeError::MissingField("qq".into()))?;
            let binding = tokio::task::block_in_place(|| {
                services.runtime_handle.block_on(async {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(5),
                        services.storage.get_binding(qq),
                    )
                    .await
                })
            })
            .map_err(|e| BridgeError::Database(e.to_string()))?;
            let binding = match binding {
                Ok(result) => result,
                Err(_) => return Err(BridgeError::Database("database query timed out".into())),
            };
            let result = match binding {
                Some((uid, uname)) => serde_json::json!({"user_id": uid, "username": uname}),
                None => serde_json::Value::Null,
            };
            Ok(result.to_string())
        }
        "osu_api_fetch_user" => {
            let (username, mode) = parse_fetch_user_mode(payload)?;
            if !acquire_rate_limiter(services) {
                return Err(BridgeError::HttpRequest(
                    user_str("bridge.rate_limit_http").into(),
                ));
            }
            let stats = tokio::task::block_in_place(|| {
                services.runtime_handle.block_on(async {
                    osubot_core::api::fetch_user_stats_by_username(
                        &services.rate_limiter,
                        &services.oauth,
                        &username,
                        mode,
                    )
                    .await
                    .map_err(|e| BridgeError::HttpRequest(e.to_string()))
                })
            })?;
            serde_json::to_string(&stats).map_err(BridgeError::JsonParse)
        }
        "get_plugin_config" => {
            let config = services
                .instance_config
                .as_ref()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "{}".to_string());
            Ok(config)
        }
        "register_tick" => {
            let v = parse_payload(payload)?;
            let tick_name = get_field(&v, "name")?;
            let interval_secs = v["interval_secs"]
                .as_u64()
                .ok_or_else(|| BridgeError::MissingField("interval_secs".into()))?;
            const MIN_INTERVAL: u64 = 5;
            const MAX_TICKS_PER_PLUGIN: usize = 8;
            if interval_secs < MIN_INTERVAL {
                return Err(BridgeError::Validation(
                    user_str("bridge.tick_interval_too_short")
                        .replace("{secs}", &MIN_INTERVAL.to_string()),
                ));
            }
            let mut registry = services
                .tick_registry
                .lock()
                .map_err(|e| BridgeError::Database(e.to_string()))?;
            let plugin_tick_count = registry
                .iter()
                .filter(|t| t.plugin_idx == services.instance_idx)
                .count();
            if plugin_tick_count >= MAX_TICKS_PER_PLUGIN
                && !registry
                    .iter()
                    .any(|t| t.plugin_idx == services.instance_idx && t.name == tick_name)
            {
                return Err(BridgeError::Validation(
                    user_str("bridge.tick_limit_exceeded")
                        .replace("{limit}", &MAX_TICKS_PER_PLUGIN.to_string()),
                ));
            }
            if let Some(existing_pos) = registry
                .iter()
                .enumerate()
                .find(|(_, t)| t.plugin_idx == services.instance_idx && t.name == tick_name)
                .map(|(pos, _)| pos)
            {
                registry[existing_pos].interval_secs = interval_secs;
                return Ok(registry[existing_pos].tick_id.to_string());
            }
            let tick_id = services
                .tick_id_counter
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            registry.push(TickRegistration {
                plugin_idx: services.instance_idx,
                name: tick_name,
                interval_secs,
                tick_id,
            });
            Ok(tick_id.to_string())
        }
        _ => Err(BridgeError::UnknownHostCall(name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osubot_types::GameMode;

    fn call(payload: &str) -> Result<(String, GameMode), BridgeError> {
        parse_fetch_user_mode(payload)
    }

    #[test]
    fn fetch_user_mode_accepts_string_osu() {
        let (u, m) = call(r#"{"username":"cookie","mode":"osu"}"#).unwrap();
        assert_eq!(u, "cookie");
        assert_eq!(m, GameMode::Osu);
    }

    #[test]
    fn parse_fetch_user_mode_returns_osubot_types_game_mode() {
        let (_, m) = call(r#"{"username":"a","mode":"osu"}"#).unwrap();
        let _: osubot_types::GameMode = m;
    }

    #[test]
    fn fetch_user_mode_accepts_taiko_catch_mania() {
        let (_, m) = call(r#"{"username":"a","mode":"taiko"}"#).unwrap();
        assert_eq!(m, GameMode::Taiko);
        let (_, m) = call(r#"{"username":"a","mode":"catch"}"#).unwrap();
        assert_eq!(m, GameMode::Catch);
        let (_, m) = call(r#"{"username":"a","mode":"mania"}"#).unwrap();
        assert_eq!(m, GameMode::Mania);
    }

    #[test]
    fn fetch_user_mode_rejects_integer() {
        let err = call(r#"{"username":"a","mode":0}"#).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidMode(_)), "got {err:?}");
    }

    #[test]
    fn fetch_user_mode_rejects_unknown_string() {
        let err = call(r#"{"username":"a","mode":"OSU"}"#).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidMode(_)), "got {err:?}");
    }

    #[test]
    fn fetch_user_mode_rejects_missing() {
        let err = call(r#"{"username":"a"}"#).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidMode(_)), "got {err:?}");
    }
}
