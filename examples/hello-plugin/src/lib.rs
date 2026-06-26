#![cfg_attr(not(test), no_main)]

use osubot_plugin_sdk::*;

static CTX: PluginContext = PluginContext;

/// 宿主传入的指针/长度上限，防止恶意或 buggy host 导致 OOB 读取
const MAX_HOST_BUF: u32 = 65536;

fn return_ptr<T: serde::Serialize>(val: &T) -> *const u8 {
    osubot_plugin_sdk::serialize_return(val)
        .map(|r| r.into_raw())
        .unwrap_or_else(|| {
            // 序列化失败是 SDK 内部错误，返回 null 会导致宿主 UB
            panic!("serialize_return failed")
        })
}

/// 从宿主传入的指针/长度安全构造 &str，校验通过返回 Some，否则返回 None
unsafe fn read_host_str(ptr: u32, len: u32) -> Option<&'static str> {
    if ptr == 0 || len == 0 || len > MAX_HOST_BUF {
        return None;
    }
    let slice = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    core::str::from_utf8(slice).ok()
}

#[no_mangle]
pub unsafe extern "C" fn alloc(size: u32) -> *mut u8 {
    osubot_plugin_sdk::alloc(size)
}

#[no_mangle]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, size: u32) {
    osubot_plugin_sdk::dealloc(ptr, size)
}

#[no_mangle]
pub extern "C" fn plugin_metadata() -> *const u8 {
    let meta = PluginMetadata {
        protocol_version: osubot_plugin_sdk::PROTOCOL_VERSION,
        name: "hello".to_string(),
        version: "0.1.0".to_string(),
        author: "osubot".to_string(),
        description: "A simple hello plugin for testing, demonstrating CTX and host calls".to_string(),
        commands: vec!["!hello".to_string(), "!ping".to_string()],
    };
    return_ptr(&meta)
}

#[no_mangle]
pub extern "C" fn on_load() -> *const u8 {
    // Register a tick every 3600 seconds (1 hour) as a demo
    if let Err(e) = CTX.register_tick("hello_tick", 3600) {
        return return_ptr(&e);
    }
    return_ptr(&"")
}

#[no_mangle]
pub extern "C" fn on_command(cmd_ptr: u32, cmd_len: u32) -> *const u8 {
    let cmd_json = match unsafe { read_host_str(cmd_ptr, cmd_len) } {
        Some(s) => s,
        None => return return_ptr(&PluginAction::Next),
    };

    let cmd: Command = match serde_json::from_str(cmd_json) {
        Ok(c) => c,
        Err(_) => return return_ptr(&PluginAction::Next),
    };

    match cmd.command_type.as_str() {
        "!hello" => {
            // 使用 Intercepted 表示插件已处理（通过 send_group_msg 主动发送），
            // 宿主无需再使用 Handled 的返回值重复发送响应。
            if let Some(gid) = cmd.group_id {
                if let Err(e) = CTX.send_group_msg(gid, "你好，这是来自 WASM 插件的消息！") {
                    return return_ptr(&PluginAction::Handled(format!("发送失败: {e}")));
                }
            }
            return_ptr(&PluginAction::Intercepted)
        }
        "!ping" => {
            // 使用 Handled 让宿主代为发送响应——插件无需主动调用 send_group_msg
            return_ptr(&PluginAction::Handled("pong from WASM plugin".to_string()))
        }
        _ => return_ptr(&PluginAction::Next),
    }
}

#[no_mangle]
pub extern "C" fn on_message(msg_ptr: u32, msg_len: u32) -> *const u8 {
    let msg_json = match unsafe { read_host_str(msg_ptr, msg_len) } {
        Some(s) => s,
        None => return return_ptr(&PluginAction::Next),
    };

    let msg: QQMessage = match serde_json::from_str(msg_json) {
        Ok(m) => m,
        Err(_) => return return_ptr(&PluginAction::Next),
    };

    if msg.message.contains("hello") || msg.message.contains("你好") {
        return_ptr(&PluginAction::Handled(format!(
            "WASM 插件收到消息: {}",
            msg.message
        )))
    } else {
        return_ptr(&PluginAction::Next)
    }
}

#[no_mangle]
pub extern "C" fn on_unload() -> *const u8 {
    return_ptr(&serde_json::json!({"ok": true}))
}

#[no_mangle]
pub extern "C" fn on_tick(tick_ptr: u32, tick_len: u32) -> *const u8 {
    let json = match unsafe { read_host_str(tick_ptr, tick_len) } {
        Some(s) => s,
        None => return return_ptr(&serde_json::json!({"ok": true})),
    };
    let tick_data: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return return_ptr(&serde_json::json!({"ok": true})),
    };
    let tick_id = tick_data
        .get("tick_id")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // on_tick 没有群号上下文。实际插件应当在 on_command 或 on_message 中
    // 通过 Command::group_id 或 QQMessage::group_id 获取并存储群号，
    // 然后在 on_tick 中使用存储的群号调用 send_group_msg。
    return_ptr(&serde_json::json!({"ok": true, "tick_id": tick_id}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_deserialize() {
        let json = r#"{"protocol_version":1,"name":"test","version":"1.0","author":"me","description":"desc","commands":["!ping"]}"#;
        let meta: PluginMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(meta.protocol_version, 1);
        assert_eq!(meta.name, "test");
        assert_eq!(meta.commands, vec!["!ping"]);
    }

    #[test]
    fn test_command_serde() {
        let cmd = Command {
            command_type: "!hello".into(),
            user_id: Some(123),
            group_id: Some(456),
            message: Some("hello".into()),
            mode: None,
            username: Some("test_user".into()),
            qq: Some(789),
            beatmap_id: None,
            score_id: None,
            filters: None,
            limit: None,
            limit_end: None,
            mentioned_user_id: None,
            explicit_position: false,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deserialized: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.command_type, "!hello");
        assert!(matches!(deserialized.group_id, Some(456)));
        assert!(matches!(deserialized.username, Some(ref n) if n == "test_user"));
    }

    #[test]
    fn test_plugin_action_serde() {
        let handled = PluginAction::Handled("response".into());
        let json = serde_json::to_string(&handled).unwrap();
        let deserialized: PluginAction = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, PluginAction::Handled(ref s) if s == "response"));

        let next = PluginAction::Next;
        let json = serde_json::to_string(&next).unwrap();
        let deserialized: PluginAction = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{:?}", deserialized), "Next");

        let intercepted = PluginAction::Intercepted;
        let json = serde_json::to_string(&intercepted).unwrap();
        let deserialized: PluginAction = serde_json::from_str(&json).unwrap();
        assert_eq!(format!("{:?}", deserialized), "Intercepted");
    }
}
