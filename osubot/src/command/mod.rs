use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use tokio::sync::mpsc;

use osubot_core::{
    api::{self, ApiError},
    highlight::{format_highlight, get_highlight, HighlightError},
    log_fmt, parse_command,
    response::format_stats_with_change,
    storage::Storage,
    strings::user_str,
    types::{Command, GameMode},
};
use osubot_plugin::{PluginActionResult, PluginManager};
use osubot_render::{render_profile_card, PROFILE_VIEWPORT_WIDTH};

use tracing::{debug, error, info, warn};

use crate::onebot::{get_group_member_list, send_group_msg_with_image, QQMessage};
use crate::score_filter::ScoreQueryParams;
use crate::score_query::{handle_beatmap_score_query, handle_best_score_query, handle_score_query};
use crate::{
    api_error_msg, beatmapset_dedup, profile_dedup, score_by_id_dedup, send_error, BotContext,
    UserRateLimit,
};

mod beatmap_audio;
mod beatmap_preview;
mod query;
mod settings;
mod utility;

use beatmap_audio::{handle_beatmap_audio, BeatmapAudioParams};
use beatmap_preview::{handle_beatmap_preview, BeatmapPreviewParams};
use query::handle_query_commands;
use settings::handle_settings_commands;
use utility::handle_utility_commands;

/// 解析本次命令的"目标 QQ"，用于在命令未显式指定模式时回退到该用户的 `default_mode`。
///
/// 设计语义：`default_mode` 是**被查询目标用户**的偏好（"我喜欢用 taiko 模式展示成绩"），
/// 而不是查询发起者的偏好。这意味着：
/// - `!p`（自己）→ target = msg.user_id → 用发起者自己的 default_mode
/// - `!p ZnCookie` / `where ZnCookie` / `!p @123456` → target = ZnCookie 的 QQ → 用 ZnCookie 的 default_mode
/// - `今日高光` → target = msg.user_id → 用发起者自己的 default_mode
///
/// 因此 `A !mode 1` 之后，不仅 A 自己的 `!p` 走 taiko，其他人对 A 用 `!p` / `where A` 也会走 taiko。
/// 这是 by design：让用户配置一次就适用于所有查询该用户名的场景，避免每次查询都要带 `:1`。
pub(crate) async fn resolve_cmd_target_qq(
    cmd: &Command,
    msg: &QQMessage,
    storage: &Storage,
) -> Option<i64> {
    match cmd {
        Command::QuerySelf { .. } => Some(msg.user_id),
        Command::QueryUser { username, .. } => match storage.find_qq_by_username(username).await {
            Ok(qq) => qq,
            Err(e) => {
                warn!(username = %username, error = %e, "{}", log_fmt!("main.find_qq_by_username_error", username = username, error = &e.to_string()));
                None
            }
        },
        Command::QueryMentionedUser { qq, .. } => Some(*qq),
        Command::Pass { qq: Some(qq), .. }
        | Command::Recent { qq: Some(qq), .. }
        | Command::Best { qq: Some(qq), .. }
        | Command::ScoreOnBeatmap { qq: Some(qq), .. }
        | Command::BeatmapAudio { qq: Some(qq), .. } => Some(*qq),
        Command::Pass {
            qq: None,
            username: Some(username),
            ..
        }
        | Command::Recent {
            qq: None,
            username: Some(username),
            ..
        }
        | Command::ScoreOnBeatmap {
            qq: None,
            username: Some(username),
            ..
        }
        | Command::Best {
            qq: None,
            username: Some(username),
            ..
        }
        | Command::BeatmapAudio {
            qq: None,
            username: Some(username),
            ..
        } => match storage.find_qq_by_username(username).await {
            Ok(qq) => qq,
            Err(e) => {
                warn!(username = %username, error = %e, "{}", log_fmt!("main.find_qq_by_username_error", username = username, error = &e.to_string()));
                None
            }
        },
        Command::Highlight { .. } => Some(msg.user_id),
        Command::Pass {
            qq: None,
            username: None,
            ..
        }
        | Command::Recent {
            qq: None,
            username: None,
            ..
        }
        | Command::Best {
            qq: None,
            username: None,
            ..
        }
        | Command::ScoreOnBeatmap {
            qq: None,
            username: None,
            ..
        }
        | Command::BeatmapAudio {
            qq: None,
            username: None,
            ..
        } => Some(msg.user_id),
        _ => None,
    }
}

/// 命令是否涉及模式（决定是否需要 `default_mode` 兜底）。
pub(crate) fn mode_sensitive(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::QuerySelf { .. }
            | Command::QueryUser { .. }
            | Command::QueryMentionedUser { .. }
            | Command::Pass { .. }
            | Command::Recent { .. }
            | Command::Best { .. }
            | Command::ScoreOnBeatmap { .. }
            | Command::Highlight { .. }
            | Command::BeatmapAudio { .. }
    )
}

/// 从命令中提取显式指定的 mode（未指定返回 None）。
pub(crate) fn extract_explicit_mode(cmd: &Command) -> Option<GameMode> {
    match cmd {
        Command::QuerySelf { mode }
        | Command::QueryUser { mode, .. }
        | Command::QueryMentionedUser { mode, .. }
        | Command::Pass { mode, .. }
        | Command::Recent { mode, .. }
        | Command::Best { mode, .. }
        | Command::Highlight { mode, .. }
        | Command::ScoreOnBeatmap { mode, .. }
        | Command::BeatmapAudio { mode, .. } => *mode,
        _ => None,
    }
}

/// 解析本次命令最终使用的模式。
///
/// 优先级：命令中显式指定（`!p :1`） > 目标用户的 `default_mode` > `Osu` 回退。
/// "目标用户"由 [`resolve_cmd_target_qq`] 决定——通常是**被查询者**的 QQ，
/// 因此 `A !p B` 在 A 未指定模式时使用 B 的 default_mode，而非 A 的。
pub(crate) async fn resolve_mode(
    storage: &Storage,
    target_qq: Option<i64>,
    explicit_mode: Option<GameMode>,
) -> GameMode {
    match explicit_mode {
        Some(mode) => mode,
        None => match target_qq {
            Some(qq) => match storage.get_default_mode(qq).await {
                Ok(Some(mode)) => mode,
                Ok(None) => GameMode::Osu,
                Err(e) => {
                    warn!(user_id = qq, error = %e, "{}", log_fmt!("main.get_default_mode_error"));
                    GameMode::Osu
                }
            },
            None => GameMode::Osu,
        },
    }
}

/// Build a JSON payload for the plugin command dispatch.
pub(crate) fn build_cmd_payload(
    cmd: &Command,
    cmd_name: &str,
    msg: &QQMessage,
    resolved_mode: Option<GameMode>,
) -> serde_json::Value {
    let username = match cmd {
        Command::QueryUser { username, .. } => Some(username.as_str()),
        Command::Bind { username, .. } => Some(username.as_str()),
        Command::ScoreOnBeatmap { username, .. }
        | Command::Pass { username, .. }
        | Command::Recent { username, .. }
        | Command::Best { username, .. }
        | Command::ProfileCard { username, .. }
        | Command::BeatmapAudio { username, .. } => username.as_deref(),
        Command::BeatmapPreview { .. } => None,
        _ => None,
    };
    serde_json::json!({
        "command_type": cmd_name,
        "group_id": msg.group_id,
        "user_id": msg.user_id,
        "message": msg.message,
        "mentioned_user_id": msg.mentioned_user_id,
        "mode": resolved_mode,
        "username": username,
        "qq": match cmd {
            Command::QueryMentionedUser { qq, .. } => Some(*qq),
            Command::Pass { qq, .. }
            | Command::Recent { qq, .. }
            | Command::Best { qq, .. }
            | Command::ScoreOnBeatmap { qq, .. }
            | Command::ProfileCard { qq, .. }
            | Command::BeatmapAudio { qq, .. } => *qq,
            Command::BeatmapPreview { .. } => None,
            _ => None,
        },
        "beatmap_id": match cmd {
            Command::ScoreOnBeatmap { beatmap_id, .. }
            | Command::Pass { beatmap_id, .. }
            | Command::Recent { beatmap_id, .. }
            | Command::BeatmapPreview { beatmap_id, .. }
            | Command::BeatmapAudio { beatmap_id, .. } => *beatmap_id,
            _ => None,
        },
        "score_id": match cmd {
            Command::ScoreOnBeatmap { score_id, .. }
            | Command::Pass { score_id, .. }
            | Command::Recent { score_id, .. }
            | Command::BeatmapPreview { score_id, .. }
            | Command::BeatmapAudio { score_id, .. } => *score_id,
            _ => None,
        },
        "limit": match cmd {
            Command::ScoreOnBeatmap { limit, .. }
            | Command::Pass { limit, .. }
            | Command::Recent { limit, .. }
            | Command::Best { limit, .. }
            | Command::BeatmapAudio { limit, .. } => Some(*limit),
            _ => None,
        },
        "filters": match cmd {
            Command::ScoreOnBeatmap { filters, .. }
            | Command::Pass { filters, .. }
            | Command::Recent { filters, .. }
            | Command::Best { filters, .. }
            | Command::BeatmapAudio { filters, .. } => filters.clone(),
            _ => None,
        },
        "limit_end": match cmd {
            Command::ScoreOnBeatmap { limit_end, .. }
            | Command::Pass { limit_end, .. }
            | Command::Recent { limit_end, .. }
            | Command::Best { limit_end, .. } => *limit_end,
            _ => None,
        },
        "explicit_position": match cmd {
            Command::BeatmapAudio { explicit_position, .. } => *explicit_position,
            _ => false,
        },
    })
}

pub(crate) async fn handle_command(ctx: BotContext, msg: QQMessage, resp_tx: mpsc::Sender<String>) {
    // ==== 用户命令频率限制（滑动窗口：3秒内最多5次） ====
    // 限流检查放在最前面，防止插件命令绕过限流
    {
        let rate_limited = {
            let mut entry = ctx
                .command_rate_limits
                .entry(msg.user_id)
                .or_insert(UserRateLimit {
                    last_command: std::time::Instant::now(),
                    command_timestamps: Vec::new(),
                });

            let now = std::time::Instant::now();
            entry
                .command_timestamps
                .retain(|t| now.duration_since(*t) < Duration::from_secs(3));
            entry.command_timestamps.push(now);
            entry.last_command = now;

            entry.command_timestamps.len() > 5
        };
        if rate_limited {
            let _ = resp_tx
                .send(
                    user_str("error.rate_limit_generic").replace("{qq}", &msg.user_id.to_string()),
                )
                .await;
            return;
        }
    }

    // 定期清理不活跃的用户（每60秒清理30秒内无命令的用户）
    // 用 unwrap_or_else 容忍锁中毒（panic 后 Mutex 进入 poisoned 状态，
    // 但本临界区仅做 elapsed 检查 + retain 调用，不跨 await，安全）。
    static LAST_CLEANUP: OnceLock<std::sync::Mutex<std::time::Instant>> = OnceLock::new();
    let last = LAST_CLEANUP.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
    {
        let mut last_time = last.lock().unwrap_or_else(|e| e.into_inner());
        if last_time.elapsed() >= Duration::from_secs(60) {
            ctx.command_rate_limits
                .retain(|_, v| v.last_command.elapsed() < Duration::from_secs(30));
            *last_time = std::time::Instant::now();
        }
    }

    // ==== Plugin on_message dispatch ====
    {
        let msg_payload = serde_json::json!({
            "group_id": msg.group_id,
            "user_id": msg.user_id,
            "message": msg.message,
            "mentioned_user_id": msg.mentioned_user_id,
        });
        let msg_payload_str = msg_payload.to_string();
        let action = PluginManager::dispatch_message(&ctx.plugin_manager, &msg_payload_str).await;
        match action {
            PluginActionResult::Handled(response) => {
                let _ = resp_tx.send(response).await;
                return;
            }
            PluginActionResult::Intercepted => return,
            PluginActionResult::Next => {}
        }
    }

    // ==== Plugin on_command dispatch (brief locks managed inside dispatch_command) ====
    let cmd_opt = parse_command(&msg.message, msg.mentioned_user_id);

    // Pre-resolve mode once for both plugin dispatch and native handlers.
    // 仅对 mode-sensitive 命令触发 DB 查询；SetDefaultMode / Bind / Unbind / Help / ProfileCard
    // 不涉及模式，跳过 resolve_cmd_target_qq 和 get_default_mode。
    let resolved_mode = match cmd_opt.as_ref() {
        Some(cmd) if mode_sensitive(cmd) => {
            let target_qq = resolve_cmd_target_qq(cmd, &msg, &ctx.storage).await;
            let explicit_mode = extract_explicit_mode(cmd);
            Some(resolve_mode(&ctx.storage, target_qq, explicit_mode).await)
        }
        _ => None,
    };

    if let Some(ref cmd) = cmd_opt {
        let cmd_name = cmd.command_name();
        let cmd_payload = build_cmd_payload(cmd, cmd_name, &msg, resolved_mode);
        let cmd_payload_str = cmd_payload.to_string();
        let action =
            PluginManager::dispatch_command(&ctx.plugin_manager, cmd_name, &cmd_payload_str).await;
        match action {
            PluginActionResult::Handled(response) => {
                let _ = resp_tx.send(response).await;
                return;
            }
            PluginActionResult::Intercepted => return,
            PluginActionResult::Next => {}
        }
    }

    // ==== Fallback: old text dispatch (native commands) ====

    // 未识别命令 — 插件已拒绝，直接结束
    let Some(cmd) = cmd_opt else {
        return;
    };

    // 命令开关检查
    let group_cfg = {
        let cfg = ctx.config.read().await;
        cfg.groups.get_group_config(msg.group_id)
    };
    if !group_cfg.is_enabled(cmd.group_name()) {
        debug!(group_id = msg.group_id, command = ?cmd.group_name(), "{}", log_fmt!("main.command_disabled"));
        return;
    }

    // Handle command and send response
    let mode = resolved_mode.unwrap_or(GameMode::Osu);
    match &cmd {
        Command::QuerySelf { .. }
        | Command::QueryUser { .. }
        | Command::QueryMentionedUser { .. }
        | Command::ScoreOnBeatmap { .. }
        | Command::Pass { .. }
        | Command::Recent { .. }
        | Command::Best { .. }
        | Command::BeatmapAudio { .. } => {
            handle_query_commands(&ctx, &msg, &resp_tx, &cmd, mode).await;
        }
        Command::SetDefaultMode { .. } | Command::Bind { .. } | Command::Unbind => {
            handle_settings_commands(&ctx, &msg, &resp_tx, &cmd).await;
        }
        Command::Help
        | Command::Highlight { .. }
        | Command::ProfileCard { .. }
        | Command::BeatmapPreview { .. } => {
            handle_utility_commands(&ctx, &msg, &resp_tx, &cmd, mode).await;
        }
    }
}
