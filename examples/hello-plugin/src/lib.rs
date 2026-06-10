#![no_main]

use osubot_plugin_sdk::*;

static CTX: PluginContext = PluginContext;

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
        name: "hello",
        version: "0.1.0",
        author: "osubot",
        description: "A simple hello plugin for testing, demonstrating CTX and host calls",
        commands: vec!["!hello", "!ping"],
    };
    serialize_return(&meta)
}

#[no_mangle]
pub extern "C" fn on_load() -> *const u8 {
    // Register a tick every 3600 seconds (1 hour) as a demo
    let _ = CTX.register_tick("hello_tick", 3600);
    serialize_return(&serde_json::json!({"ok": true}))
}

#[no_mangle]
pub extern "C" fn on_command(cmd_ptr: u32, cmd_len: u32) -> *const u8 {
    let cmd_json = unsafe {
        let slice = core::slice::from_raw_parts(cmd_ptr as *const u8, cmd_len as usize);
        core::str::from_utf8(slice).unwrap_or("")
    };

    let cmd: Command = match serde_json::from_str(cmd_json) {
        Ok(c) => c,
        Err(_) => return serialize_return(&PluginAction::Next),
    };

    match cmd.command_type.as_str() {
        "!hello" => {
            if let Some(gid) = cmd.group_id {
                let _ = CTX.send_group_msg(gid, "你好，这是来自 WASM 插件的消息！");
            }
            serialize_return(&PluginAction::Handled("Hello from WASM plugin!".to_string()))
        }
        "!ping" => {
            serialize_return(&PluginAction::Handled("pong from WASM plugin".to_string()))
        }
        _ => serialize_return(&PluginAction::Next),
    }
}

#[no_mangle]
pub extern "C" fn on_message(msg_ptr: u32, msg_len: u32) -> *const u8 {
    let msg_json = unsafe {
        let slice = core::slice::from_raw_parts(msg_ptr as *const u8, msg_len as usize);
        core::str::from_utf8(slice).unwrap_or("")
    };

    let msg: QQMessage = match serde_json::from_str(msg_json) {
        Ok(m) => m,
        Err(_) => return serialize_return(&PluginAction::Next),
    };

    if msg.message.contains("hello") || msg.message.contains("你好") {
        serialize_return(&PluginAction::Handled(format!(
            "WASM 插件收到消息: {}",
            msg.message
        )))
    } else {
        serialize_return(&PluginAction::Next)
    }
}

#[no_mangle]
pub extern "C" fn on_unload() -> *const u8 {
    serialize_return(&serde_json::json!({"ok": true}))
}

#[no_mangle]
pub extern "C" fn on_tick(tick_ptr: u32, tick_len: u32) -> *const u8 {
    let json = unsafe {
        let slice = core::slice::from_raw_parts(tick_ptr as *const u8, tick_len as usize);
        core::str::from_utf8(slice).unwrap_or("")
    };
    let tick_data: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return serialize_return(&serde_json::json!({"ok": true})),
    };
    let tick_id = tick_data
        .get("tick_id")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // 实际插件中应存储并引用收到消息时的 group_id
    // send_group_msg(0, ...) 会静默失败
    serialize_return(&serde_json::json!({"ok": true, "tick_id": tick_id}))
}
