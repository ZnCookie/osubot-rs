use std::sync::atomic::AtomicU32;
use std::sync::Arc;
use wasmtime::Result as WasmResult;
use wasmtime::{Caller, Linker, StoreLimits};

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("JSON 解析失败: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("缺少字段 '{0}'")]
    MissingField(String),
    #[error("无效游戏模式: {0}")]
    InvalidMode(u8),
    #[error("HTTP 请求失败: {0}")]
    HttpRequest(String),
    #[error("数据库查询失败: {0}")]
    Database(String),
    #[error("宿主函数调用失败: {0}")]
    SendMsg(String),
    #[error("未知宿主函数: {0}")]
    UnknownHostCall(String),
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
    #[allow(clippy::type_complexity)]
    pub tick_registry: Arc<std::sync::Mutex<Vec<(usize, String, u64, u32)>>>,
    pub tick_id_counter: Arc<AtomicU32>,
    pub instance_config: Option<serde_json::Value>,
    pub limiter: StoreLimits,
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

            memory
                .write(&mut caller, result_ptr as usize, &result_len.to_le_bytes())
                .map_err(|e| wasmtime::format_err!("write length: {e}"))?;
            memory
                .write(&mut caller, (result_ptr + 4) as usize, &result_bytes)
                .map_err(|e| wasmtime::format_err!("write data: {e}"))?;

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
            let text = get_field(&v, "text")?;
            let rate_limiter = services.rate_limiter.clone();
            tokio::task::block_in_place(|| {
                services.runtime_handle.block_on(async {
                    if rate_limiter.acquire().await.is_err() {
                        return Err(BridgeError::SendMsg("消息发送过于频繁，请稍后再试".into()));
                    }
                    Ok(())
                })
            })?;
            (services.send_msg_fn)(group_id, serde_json::Value::String(text))
                .map_err(BridgeError::SendMsg)?;
            Ok("{}".to_string())
        }
        "send_image" => {
            let v = parse_payload(payload)?;
            let group_id = v["group_id"]
                .as_i64()
                .ok_or_else(|| BridgeError::MissingField("group_id".into()))?;
            let jpeg_b64 = get_field(&v, "jpeg_base64")?;
            let rate_limiter = services.rate_limiter.clone();
            tokio::task::block_in_place(|| {
                services.runtime_handle.block_on(async {
                    if rate_limiter.acquire().await.is_err() {
                        return Err(BridgeError::SendMsg("消息发送过于频繁，请稍后再试".into()));
                    }
                    Ok(())
                })
            })?;
            let image_segment = serde_json::json!([{
                "type": "image",
                "data": {
                    "file": format!("base64://{}", jpeg_b64)
                }
            }]);
            (services.send_msg_fn)(group_id, image_segment).map_err(BridgeError::SendMsg)?;
            Ok("{}".to_string())
        }
        "http_request" => {
            let v = parse_payload(payload)?;
            let url = get_field(&v, "url")?;
            let rate_limiter = services.rate_limiter.clone();
            tokio::task::block_in_place(|| {
                services.runtime_handle.block_on(async {
                    if rate_limiter.acquire().await.is_err() {
                        return Err(BridgeError::HttpRequest(
                            "请求过于频繁，请稍后再试".to_string(),
                        ));
                    }
                    Ok::<_, BridgeError>(())
                })
            })?;
            let body = services
                .blocking_http_client
                .get(&url)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .map_err(|e| BridgeError::HttpRequest(format!("HTTP 请求失败 (30s 超时): {e}")))?
                .text()
                .map_err(|e| BridgeError::HttpRequest(format!("读取响应体失败: {e}")))?;
            Ok(body)
        }
        "db_get_binding" => {
            let v = parse_payload(payload)?;
            let qq = v["qq"]
                .as_i64()
                .ok_or_else(|| BridgeError::MissingField("qq".into()))?;
            let binding = services
                .storage
                .get_binding(qq)
                .map_err(|e| BridgeError::Database(e.to_string()))?;
            let result = match binding {
                Some((uid, uname)) => serde_json::json!({"user_id": uid, "username": uname}),
                None => serde_json::Value::Null,
            };
            Ok(result.to_string())
        }
        "osu_api_fetch_user" => {
            let v = parse_payload(payload)?;
            let username = get_field(&v, "username")?;
            let mode_num = v["mode"].as_u64().unwrap_or(0) as u8;
            let mode = match mode_num {
                0 => osubot_types::GameMode::Osu,
                1 => osubot_types::GameMode::Taiko,
                2 => osubot_types::GameMode::Catch,
                3 => osubot_types::GameMode::Mania,
                _ => return Err(BridgeError::InvalidMode(mode_num)),
            };
            let stats = tokio::task::block_in_place(|| {
                services.runtime_handle.block_on(async {
                    if services.rate_limiter.acquire().await.is_err() {
                        return Err(BridgeError::HttpRequest(
                            "请求过于频繁，请稍后再试".to_string(),
                        ));
                    }
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
            serde_json::to_string(&stats).map_err(|e| BridgeError::HttpRequest(e.to_string()))
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
                return Err(BridgeError::Database(format!(
                    "tick 间隔不能小于 {MIN_INTERVAL} 秒"
                )));
            }
            let mut registry = services
                .tick_registry
                .lock()
                .map_err(|e| BridgeError::Database(e.to_string()))?;
            let plugin_tick_count = registry
                .iter()
                .filter(|(idx, _, _, _)| *idx == services.instance_idx)
                .count();
            if plugin_tick_count >= MAX_TICKS_PER_PLUGIN
                && !registry
                    .iter()
                    .any(|(idx, name, _, _)| *idx == services.instance_idx && name == &tick_name)
            {
                return Err(BridgeError::Database(format!(
                    "每个插件最多注册 {MAX_TICKS_PER_PLUGIN} 个 tick"
                )));
            }
            if let Some((existing_pos, _)) = registry
                .iter()
                .enumerate()
                .find(|(_, (idx, name, _, _))| *idx == services.instance_idx && name == &tick_name)
            {
                registry[existing_pos].2 = interval_secs;
                return Ok(registry[existing_pos].3.to_string());
            }
            let tick_id = services
                .tick_id_counter
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            registry.push((services.instance_idx, tick_name, interval_secs, tick_id));
            Ok(tick_id.to_string())
        }
        _ => Err(BridgeError::UnknownHostCall(name.to_string())),
    }
}
