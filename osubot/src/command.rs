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
use crate::score_query::{handle_beatmap_score_query, handle_score_query};
use crate::{
    api_error_msg, profile_dedup, score_by_id_dedup, send_error, BotContext, UserRateLimit,
};

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
        | Command::ScoreOnBeatmap { qq: Some(qq), .. } => Some(*qq),
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
        | Command::ScoreOnBeatmap {
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
            | Command::ScoreOnBeatmap { .. }
            | Command::Highlight { .. }
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
        | Command::Highlight { mode, .. }
        | Command::ScoreOnBeatmap { mode, .. } => *mode,
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
        | Command::ProfileCard { username, .. } => username.as_deref(),
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
            | Command::ScoreOnBeatmap { qq, .. }
            | Command::ProfileCard { qq, .. } => *qq,
            Command::BeatmapPreview { .. } => None,
            _ => None,
        },
        "beatmap_id": match cmd {
            Command::ScoreOnBeatmap { beatmap_id, .. }
            | Command::Pass { beatmap_id, .. }
            | Command::Recent { beatmap_id, .. }
            | Command::BeatmapPreview { beatmap_id, .. } => *beatmap_id,
            _ => None,
        },
        "score_id": match cmd {
            Command::ScoreOnBeatmap { score_id, .. }
            | Command::Pass { score_id, .. }
            | Command::Recent { score_id, .. }
            | Command::BeatmapPreview { score_id, .. } => *score_id,
            _ => None,
        },
        "limit": match cmd {
            Command::ScoreOnBeatmap { limit, .. } | Command::Pass { limit, .. } | Command::Recent { limit, .. } => Some(*limit),
            _ => None,
        },
        "filters": match cmd {
            Command::ScoreOnBeatmap { filters, .. }
            | Command::Pass { filters, .. }
            | Command::Recent { filters, .. } => filters.clone(),
            _ => None,
        },
        "limit_end": match cmd {
            Command::ScoreOnBeatmap { limit_end, .. }
            | Command::Pass { limit_end, .. }
            | Command::Recent { limit_end, .. } => *limit_end,
            _ => None,
        },
    })
}

/// Handle query commands: QuerySelf, QueryUser, QueryMentionedUser, ScoreOnBeatmap, Pass, Recent.
async fn handle_query_commands(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    match cmd {
        Command::QuerySelf { .. } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "{}", log_fmt!("main.query_self"));
            match ctx.storage.get_binding(msg.user_id).await {
                Ok(Some((user_id, username))) => {
                    ctx.fetch_stats_and_reply(
                        msg.user_id,
                        user_id,
                        &username,
                        mode,
                        resp_tx,
                        "QuerySelf",
                    )
                    .await;
                }
                Ok(None) => {
                    if let Some((user_id, username)) = ctx.resolve_binding(msg.user_id).await {
                        info!(user_id = msg.user_id, osu_id = user_id, username = %username, "{}", log_fmt!("main.query_self_auto_bound"));
                        ctx.fetch_stats_and_reply(
                            msg.user_id,
                            user_id,
                            &username,
                            mode,
                            resp_tx,
                            "QuerySelf (auto-bound)",
                        )
                        .await;
                    } else {
                        info!(
                            user_id = msg.user_id,
                            "{}",
                            log_fmt!("main.query_self_no_binding")
                        );
                        let _ = resp_tx
                            .send(
                                user_str("bind.not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
                Err(_) => {
                    error!(
                        user_id = msg.user_id,
                        "{}",
                        log_fmt!("main.query_self_db_error")
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        Command::QueryUser { username, .. } => {
            info!(group_id = msg.group_id, username = %username, mode = ?mode, "{}", log_fmt!("main.query_user"));
            match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, username, mode)
                .await
            {
                Ok(stats) => {
                    if let Err(e) = ctx
                        .storage
                        .set_user_id(&stats.username, stats.user_id)
                        .await
                    {
                        tracing::warn!(
                            username = %stats.username,
                            user_id = stats.user_id,
                            error = %e,
                            "{}",
                            log_fmt!("main.cache_user_id_failed")
                        );
                    }
                    if stats.username != *username {
                        if let Err(e) = ctx.storage.set_user_id(username, stats.user_id).await {
                            tracing::warn!(
                                username = %username,
                                user_id = stats.user_id,
                                error = %e,
                                "{}",
                                log_fmt!("main.cache_user_id_failed")
                            );
                        }
                    }
                    ctx.scheduler.trigger_update(stats.user_id, mode).await;
                    let change = ctx
                        .storage
                        .calculate_change(stats.user_id, mode, &stats)
                        .await
                        .inspect_err(|e| {
                            tracing::warn!(
                                user_id = stats.user_id,
                                mode = ?mode,
                                error = %e,
                                "{}",
                                log_fmt!("main.calculate_change_failed")
                            )
                        })
                        .ok()
                        .flatten();
                    let has_change = change.is_some();
                    info!(username = %username, mode = ?mode, pp = stats.pp, rank = stats.rank, change = ?change, "{}", log_fmt!("main.query_user_success"));
                    let response = format_stats_with_change(&stats, &change, mode);
                    let _ = resp_tx.send(response).await;
                    if !has_change {
                        info!(username = %username, "{}", log_fmt!("main.query_user_no_change"));
                    }
                }
                Err(e) => {
                    warn!(username = %username, mode = ?mode, error = ?e, "{}", log_fmt!("main.query_user_failed"));
                    let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                }
            }
        }
        Command::QueryMentionedUser { qq, .. } => {
            info!(qq = qq, group_id = msg.group_id, mode = ?mode, "{}", log_fmt!("main.query_mentioned_user"));
            match ctx.storage.get_binding(*qq).await {
                Ok(Some((user_id, username))) => {
                    ctx.fetch_stats_and_reply(
                        *qq,
                        user_id,
                        &username,
                        mode,
                        resp_tx,
                        "QueryMentionedUser",
                    )
                    .await;
                }
                Ok(None) => {
                    if let Some((user_id, username)) = ctx.resolve_binding(*qq).await {
                        info!(qq = qq, osu_id = user_id, username = %username, "{}", log_fmt!("main.query_mentioned_auto_bound"));
                        ctx.fetch_stats_and_reply(
                            *qq,
                            user_id,
                            &username,
                            mode,
                            resp_tx,
                            "QueryMentionedUser (auto-bound)",
                        )
                        .await;
                    } else {
                        info!(qq = qq, "{}", log_fmt!("main.query_mentioned_no_binding"));
                        let _ = resp_tx
                            .send(
                                user_str("bind.mentioned_not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
                Err(_) => {
                    error!(qq = qq, "{}", log_fmt!("main.query_mentioned_db_error"));
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        Command::ScoreOnBeatmap { .. } => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "{}",
                log_fmt!("main.score_on_beatmap_cmd")
            );
            handle_beatmap_score_query(ctx, msg, resp_tx, cmd, mode).await;
        }
        Command::Pass {
            mode: _,
            username,
            qq,
            beatmap_id,
            score_id,
            limit,
            limit_end,
            is_summary,
            filters,
        } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, limit = *limit, "{}", log_fmt!("main.pass_command"));
            handle_score_query(
                ctx,
                msg,
                resp_tx,
                ScoreQueryParams {
                    username,
                    qq,
                    is_pass: true,
                    beatmap_id: *beatmap_id,
                    score_id: *score_id,
                    limit: *limit,
                    is_single: !*is_summary,
                    limit_end: *limit_end,
                    filters: filters.as_deref(),
                },
                mode,
            )
            .await;
        }
        Command::Recent {
            mode: _,
            username,
            qq,
            beatmap_id,
            score_id,
            limit,
            limit_end,
            is_summary,
            filters,
        } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, limit = *limit, "{}", log_fmt!("main.recent_command"));
            handle_score_query(
                ctx,
                msg,
                resp_tx,
                ScoreQueryParams {
                    username,
                    qq,
                    is_pass: false,
                    beatmap_id: *beatmap_id,
                    score_id: *score_id,
                    limit: *limit,
                    is_single: !*is_summary,
                    limit_end: *limit_end,
                    filters: filters.as_deref(),
                },
                mode,
            )
            .await;
        }
        _ => unreachable!("handle_query_commands called with non-query command"),
    }
}

/// Handle settings commands: SetDefaultMode, Bind, Unbind.
async fn handle_settings_commands(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
) {
    match cmd {
        Command::SetDefaultMode { mode } => match mode {
            Some(mode) => {
                info!(
                    user_id = msg.user_id,
                    ?mode,
                    "{}",
                    log_fmt!(
                        "main.set_default_mode",
                        user_id = &msg.user_id.to_string(),
                        mode = &format!("{:?}", mode)
                    )
                );
                match ctx.storage.set_default_mode(msg.user_id, *mode).await {
                    Ok(true) => {
                        let _ = resp_tx
                            .send(
                                user_str("mode.set_success")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{mode}", mode.display_name()),
                            )
                            .await;
                    }
                    Ok(false) => {
                        let _ = resp_tx
                            .send(
                                user_str("mode.not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                    Err(e) => {
                        error!(user_id = msg.user_id, error = %e, "{}", log_fmt!("main.set_default_mode_error", user_id = &msg.user_id.to_string(), error = &e.to_string()));
                        let _ = resp_tx
                            .send(
                                user_str("error.db_error")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
            }
            None => {
                info!(
                    user_id = msg.user_id,
                    "{}",
                    log_fmt!("main.get_default_mode", user_id = &msg.user_id.to_string())
                );
                match ctx.storage.get_binding(msg.user_id).await {
                    Ok(Some(_)) => match ctx.storage.get_default_mode(msg.user_id).await {
                        Ok(Some(mode)) => {
                            let _ = resp_tx
                                .send(
                                    user_str("mode.get_success")
                                        .replace("{qq}", &msg.user_id.to_string())
                                        .replace("{mode}", mode.display_name()),
                                )
                                .await;
                        }
                        Ok(None) => {
                            let _ = resp_tx
                                .send(
                                    user_str("mode.get_success")
                                        .replace("{qq}", &msg.user_id.to_string())
                                        .replace("{mode}", GameMode::Osu.display_name()),
                                )
                                .await;
                        }
                        Err(e) => {
                            error!(
                                user_id = msg.user_id,
                                error = %e,
                                "{}",
                                log_fmt!("main.get_default_mode_error", user_id = &msg.user_id.to_string(), error = &e.to_string())
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("error.db_error")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        }
                    },
                    Ok(None) => {
                        let _ = resp_tx
                            .send(
                                user_str("bind.not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                    Err(e) => {
                        error!(
                            user_id = msg.user_id,
                            error = %e,
                            "{}",
                            log_fmt!("main.get_default_mode_error", user_id = &msg.user_id.to_string(), error = &e.to_string())
                        );
                        let _ = resp_tx
                            .send(
                                user_str("error.db_error")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                }
            }
        },
        Command::Bind { username } => {
            info!(user_id = msg.user_id, group_id = msg.group_id, username = %username, "{}", log_fmt!("main.bind_command"));
            match ctx.storage.get_binding(msg.user_id).await {
                Ok(Some((_, existing_username))) => {
                    info!(user_id = msg.user_id, existing = %existing_username, "{}", log_fmt!("main.bind_already_bound"));
                    let _ = resp_tx
                        .send(
                            user_str("bind.already_bound")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", &existing_username),
                        )
                        .await;
                }
                Ok(None) => {
                    let irc_nickname = {
                        let cfg = ctx.config.read().await;
                        if cfg.irc.enabled {
                            Some(cfg.irc.nickname.clone())
                        } else {
                            None
                        }
                    };
                    if let Some(nickname) = irc_nickname {
                        match ctx.storage.has_pending_bind(msg.user_id).await {
                            Ok(true) => {
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.pending_exists")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                                return;
                            }
                            Err(_) => {
                                error!(
                                    user_id = msg.user_id,
                                    "{}",
                                    log_fmt!("main.bind_check_pending_failed")
                                );
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.failed_retry")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                                return;
                            }
                            _ => {}
                        }
                        match ctx
                            .storage
                            .add_pending_bind(msg.user_id, msg.group_id, username)
                            .await
                        {
                            Ok(code) => {
                                info!(user_id = msg.user_id, username = %username, code = %code, "{}", log_fmt!("main.bind_pending_created"));
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.code_sent")
                                            .replace("{qq}", &msg.user_id.to_string())
                                            .replace("{code}", &code)
                                            .replace("{target}", &nickname),
                                    )
                                    .await;
                            }
                            Err(_) => {
                                error!(
                                    user_id = msg.user_id,
                                    "{}",
                                    log_fmt!("main.bind_create_pending_failed")
                                );
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.failed_retry")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                            }
                        }
                    } else {
                        match api::get_user_info(&ctx.rate_limiter, &ctx.oauth, username).await {
                            Ok(Some(user_info)) => {
                                if let Err(e) =
                                    ctx.storage.set_user_id(username, user_info.id).await
                                {
                                    warn!(error = %e, "{}", log_fmt!("main.cache_user_id_failed"));
                                }
                                match ctx
                                    .storage
                                    .bind(msg.user_id, user_info.id, &user_info.username)
                                    .await
                                {
                                    Ok(Ok(())) => {
                                        info!(user_id = msg.user_id, username = %user_info.username, "{}", log_fmt!("main.bind_success"));
                                        let _ = resp_tx
                                            .send(
                                                user_str("bind.success")
                                                    .replace("{qq}", &msg.user_id.to_string())
                                                    .replace("{name}", &user_info.username),
                                            )
                                            .await;
                                    }
                                    Ok(Err(bound_qq)) => {
                                        info!(user_id = msg.user_id, username = %username, bound_qq = bound_qq, "{}", log_fmt!("main.bind_failed_already_bound"));
                                        let _ = resp_tx
                                            .send(
                                                user_str("bind.already_bound_other")
                                                    .replace("{qq}", &msg.user_id.to_string()),
                                            )
                                            .await;
                                    }
                                    Err(_) => {
                                        error!(user_id = msg.user_id, username = %username, "{}", log_fmt!("main.bind_failed"));
                                        let _ = resp_tx
                                            .send(
                                                user_str("bind.failed_retry")
                                                    .replace("{qq}", &msg.user_id.to_string()),
                                            )
                                            .await;
                                    }
                                }
                            }
                            Ok(None) => {
                                info!(username = %username, "{}", log_fmt!("main.bind_user_not_found"));
                                let _ = resp_tx
                                    .send(
                                        user_str("bind.user_not_found")
                                            .replace("{qq}", &msg.user_id.to_string()),
                                    )
                                    .await;
                            }
                            Err(e) => {
                                warn!(username = %username, error = ?e, "{}", log_fmt!("main.bind_user_info_failed"));
                                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                            }
                        }
                    }
                }
                Err(_) => {
                    error!(user_id = msg.user_id, "{}", log_fmt!("main.bind_db_error"));
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        Command::Unbind => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                "{}",
                log_fmt!("main.unbind_command")
            );
            match ctx.storage.get_pending_unbind(msg.user_id).await {
                Ok(Some(_)) => match ctx.storage.unbind(msg.user_id).await {
                    Ok(_) => {
                        if let Err(e) = ctx.storage.remove_pending_unbind(msg.user_id).await {
                            tracing::warn!(
                                user_id = msg.user_id,
                                error = %e,
                                "{}",
                                log_fmt!("main.unbind_remove_pending_failed")
                            );
                        }
                        info!(user_id = msg.user_id, "{}", log_fmt!("main.unbind_success"));
                        let _ = resp_tx
                            .send(
                                user_str("bind.unbind_success")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                    Err(_) => {
                        error!(user_id = msg.user_id, "{}", log_fmt!("main.unbind_failed"));
                        let _ = resp_tx
                            .send(
                                user_str("bind.unbind_failed")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                },
                Ok(None) => match ctx.storage.get_binding(msg.user_id).await {
                    Ok(Some((_, current_username))) => {
                        if let Err(e) = ctx.storage.set_pending_unbind(msg.user_id).await {
                            tracing::warn!(
                                user_id = msg.user_id,
                                error = %e,
                                "{}",
                                log_fmt!("main.unbind_set_pending_failed")
                            );
                        }
                        info!(user_id = msg.user_id, username = %current_username, "{}", log_fmt!("main.unbind_confirmation"));
                        let _ = resp_tx
                            .send(
                                user_str("bind.confirm_unbind")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{name}", &current_username),
                            )
                            .await;
                    }
                    Ok(None) => {
                        info!(
                            user_id = msg.user_id,
                            "{}",
                            log_fmt!("main.unbind_no_binding")
                        );
                        let _ = resp_tx
                            .send(
                                user_str("bind.not_bound_any")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                    Err(_) => {
                        error!(
                            user_id = msg.user_id,
                            "{}",
                            log_fmt!("main.unbind_db_error")
                        );
                        let _ = resp_tx
                            .send(
                                user_str("error.db_error")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                    }
                },
                Err(_) => {
                    error!(
                        user_id = msg.user_id,
                        "{}",
                        log_fmt!("main.unbind_pending_check_error")
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        _ => unreachable!("handle_settings_commands called with non-settings command"),
    }
}

/// Handle utility commands: Help, Highlight, ProfileCard, BeatmapPreview.
async fn handle_utility_commands(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    match cmd {
        Command::Help => {
            handle_help_command(ctx, msg, resp_tx).await;
        }
        Command::Highlight { .. } => {
            handle_highlight_command(ctx, msg, resp_tx, mode).await;
        }
        Command::ProfileCard { username, qq } => {
            handle_profile_card(ctx, msg, resp_tx, username, qq, mode).await;
        }
        Command::BeatmapPreview {
            score_id,
            beatmap_id,
            mode: preview_mode,
            mods,
            gif,
            times,
        } => {
            handle_beatmap_preview(
                ctx,
                msg,
                resp_tx,
                score_id,
                beatmap_id,
                preview_mode,
                mods,
                gif,
                times,
            )
            .await;
        }
        _ => unreachable!("handle_utility_commands called with non-utility command"),
    }
}

async fn handle_help_command(_ctx: &BotContext, msg: &QQMessage, resp_tx: &mpsc::Sender<String>) {
    info!(
        user_id = msg.user_id,
        group_id = msg.group_id,
        "{}",
        log_fmt!("main.help_command")
    );
    let _ = resp_tx
        .send(user_str("sys.help").replace("{qq}", &msg.user_id.to_string()))
        .await;
}

async fn handle_highlight_command(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    mode: GameMode,
) {
    info!(user_id = msg.user_id, group_id = msg.group_id, mode = ?mode, "{}", log_fmt!("main.highlight_command"));

    let group_members = match get_group_member_list(&ctx.write, &ctx.onebot_api, msg.group_id).await
    {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "{}", log_fmt!("main.highlight_group_member_failed"));
            let _ = resp_tx
                .send(
                    user_str("error.get_group_member_failed")
                        .replace("{qq}", &msg.user_id.to_string()),
                )
                .await;
            return;
        }
    };

    let all_bindings = match ctx.storage.get_all_user_bindings().await {
        Ok(bindings) => bindings,
        Err(_) => {
            error!("{}", log_fmt!("main.highlight_fetch_bindings_failed"));
            let _ = resp_tx
                .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }
    };

    let group_bindings: Vec<(i64, i64, String)> = all_bindings
        .into_iter()
        .filter(|(qq, _, _)| group_members.contains(qq))
        .collect();

    if group_bindings.is_empty() {
        let _ = resp_tx
            .send(user_str("query.no_bound_users").replace("{qq}", &msg.user_id.to_string()))
            .await;
        return;
    }

    match get_highlight(
        &ctx.storage,
        &ctx.rate_limiter,
        &ctx.oauth,
        &group_bindings,
        mode,
    )
    .await
    {
        Ok(result) => {
            let response = format_highlight(&result);
            let _ = resp_tx.send(response).await;
        }
        Err(e) => {
            warn!(error = ?e, "{}", log_fmt!("main.highlight_fetch_failed"));
            let err_msg = match e {
                HighlightError::NoData => user_str("highlight.no_data").to_string(),
                _ => user_str("error.query_failed").replace("{qq}", &msg.user_id.to_string()),
            };
            let _ = resp_tx.send(err_msg).await;
        }
    }
}

async fn handle_profile_card(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    username: &Option<String>,
    qq: &Option<i64>,
    _mode: GameMode,
) {
    let target_user_id = match username {
        Some(ref name) => {
            if let Ok(Some(cached_id)) = ctx.storage.get_user_id(name).await {
                info!(username = %name, user_id = cached_id, "{}", log_fmt!("main.profile_card_cached"));
                cached_id
            } else {
                match api::fetch_user_stats_by_username(
                    &ctx.rate_limiter,
                    &ctx.oauth,
                    name,
                    GameMode::Osu,
                )
                .await
                {
                    Ok(stats) => {
                        info!(username = %name, user_id = stats.user_id, "{}", log_fmt!("main.profile_card_by_username"));
                        if let Err(e) = ctx
                            .storage
                            .set_user_id(&stats.username, stats.user_id)
                            .await
                        {
                            tracing::warn!(
                                username = %stats.username,
                                user_id = stats.user_id,
                                error = %e,
                                "{}",
                                log_fmt!("main.cache_user_id_failed")
                            );
                        }
                        stats.user_id
                    }
                    Err(e) => {
                        warn!(username = %name, error = ?e, "{}", log_fmt!("main.profile_card_resolution_failed"));
                        let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                        return;
                    }
                }
            }
        }
        None => match qq {
            Some(mentioned_qq) => match ctx.storage.get_binding(*mentioned_qq).await {
                Ok(Some((user_id, current_username))) => {
                    info!(qq = mentioned_qq, osu_id = user_id, username = %current_username, "{}", log_fmt!("main.profile_card_mention"));
                    user_id
                }
                Ok(None) => {
                    if let Some((uid, uname)) = ctx.resolve_binding(*mentioned_qq).await {
                        info!(qq = mentioned_qq, osu_id = uid, username = %uname, "{}", log_fmt!("main.profile_card_mention_bound"));
                        uid
                    } else {
                        info!(
                            qq = mentioned_qq,
                            "{}",
                            log_fmt!("main.profile_card_mention_no_binding")
                        );
                        let _ = resp_tx
                            .send(
                                user_str("bind.mentioned_not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                        return;
                    }
                }
                Err(_) => {
                    error!(
                        qq = mentioned_qq,
                        "{}",
                        log_fmt!("main.profile_card_mention_db_error")
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                    return;
                }
            },
            None => match ctx.storage.get_binding(msg.user_id).await {
                Ok(Some((user_id, current_username))) => {
                    info!(user_id = msg.user_id, osu_id = user_id, username = %current_username, "{}", log_fmt!("main.profile_card_self"));
                    user_id
                }
                Ok(None) => {
                    if let Some((uid, uname)) = ctx.resolve_binding(msg.user_id).await {
                        info!(user_id = msg.user_id, osu_id = uid, username = %uname, "{}", log_fmt!("main.profile_card_self_bound"));
                        uid
                    } else {
                        let _ = resp_tx
                            .send(
                                user_str("query.profile_not_bound")
                                    .replace("{qq}", &msg.user_id.to_string()),
                            )
                            .await;
                        return;
                    }
                }
                Err(_) => {
                    error!(
                        user_id = msg.user_id,
                        "{}",
                        log_fmt!("main.profile_card_db_error")
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                    return;
                }
            },
        },
    };

    info!(user_id = target_user_id, qq = ?qq, "{}", log_fmt!("main.profile_card_command"));
    let qq = msg.user_id;

    let dedup_rate_limiter = ctx.rate_limiter.clone();
    let dedup_oauth = ctx.oauth.clone();
    let dedup_config = ctx.config.clone();
    let dedup_target_id = target_user_id;
    let render_result = profile_dedup()
        .run_or_wait((target_user_id, GameMode::Osu), move || async move {
            let profile = api::fetch_user_profile(
                &dedup_rate_limiter,
                &dedup_oauth,
                dedup_target_id,
                GameMode::Osu,
            )
            .await
            .map_err(|e| api_error_msg(qq, &e))?;
            info!(
                user_id = dedup_target_id,
                html_len = profile.html.len(),
                hue = profile.profile_hue,
                "{}",
                log_fmt!("main.profile_card_html_fetched")
            );
            let profile_render = render_profile_card(
                &profile.html,
                profile.profile_hue,
                &profile.avatar_url,
                &profile.username,
                PROFILE_VIEWPORT_WIDTH,
                1200,
            );
            let render_timeout =
                Duration::from_secs(dedup_config.read().await.bot.render_timeout_secs);
            tokio::time::timeout(render_timeout, profile_render)
                .await
                .map_err(|_| {
                    warn!(user_id = target_user_id, "{}", log_fmt!("main.profile_card_render_timeout"));
                    user_str("error.render_timeout").replace("{qq}", &qq.to_string())
                })?
                .map(Arc::new)
                .map_err(|e| {
                    warn!(user_id = target_user_id, error = %e, "{}", log_fmt!("main.profile_card_render_failed"));
                    user_str("error.render_failed").replace("{qq}", &qq.to_string())
                })
        })
        .await;

    match render_result {
        Ok(jpeg_bytes) => {
            info!(
                user_id = target_user_id,
                jpeg_len = jpeg_bytes.len(),
                "{}",
                log_fmt!("main.profile_card_rendered")
            );
            let write = ctx.write.clone();
            let group_id = msg.group_id;
            let resp_tx = resp_tx.clone();

            tokio::spawn(async move {
                if send_group_msg_with_image(&write, group_id, &jpeg_bytes)
                    .await
                    .is_err()
                {
                    let _ = resp_tx
                        .send(user_str("error.image_send_failed").replace("{qq}", &qq.to_string()))
                        .await;
                }
            });
        }
        Err(msg) => {
            warn!(user_id = target_user_id, msg = %msg, "{}", log_fmt!("main.profile_card_failed"));
            let _ = resp_tx.send(msg).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_beatmap_preview(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    score_id: &Option<u64>,
    beatmap_id: &Option<u32>,
    mode: &Option<GameMode>,
    mods: &Option<Vec<String>>,
    gif: &bool,
    times: &Option<Vec<i64>>,
) {
    let qq = msg.user_id;
    let group_id = msg.group_id;

    let resolved_bid_i64: i64 = match (score_id, beatmap_id) {
        (None, Some(bid)) => *bid as i64,
        (Some(sid), None) => {
            let dedup_rate_limiter = ctx.rate_limiter.clone();
            let dedup_oauth = ctx.oauth.clone();
            let qq_for_dedup = qq;
            let sid_owned = *sid;
            let result = score_by_id_dedup()
                .run_or_wait((sid_owned as i64, GameMode::Osu), move || {
                    let rl = dedup_rate_limiter.clone();
                    let oauth = dedup_oauth.clone();
                    let qq_inner = qq_for_dedup;
                    async move {
                        api::get_score_by_id(&rl, &oauth, sid_owned)
                            .await
                            .map_err(|e| match e {
                                ApiError::NotFound => user_str("query.score_not_found")
                                    .replace("{qq}", &qq_inner.to_string()),
                                other => api_error_msg(qq_inner, &other),
                            })
                    }
                })
                .await;
            match result {
                Ok(score) => score.beatmap_id,
                Err(err_msg) => {
                    let _ = resp_tx.send(err_msg).await;
                    return;
                }
            }
        }
        (None, None) => match ctx.last_beatmap.get(group_id) {
            Some(bid) => bid as i64,
            None => {
                send_error(resp_tx, qq, "query.need_beatmap_or_cache").await;
                return;
            }
        },
        (Some(_), Some(_)) => {
            send_error(resp_tx, qq, "error.data_fetch_failed").await;
            return;
        }
    };
    let resolved_bid = match u32::try_from(resolved_bid_i64) {
        Ok(b) => b,
        Err(_) => {
            send_error(resp_tx, qq, "error.data_fetch_failed").await;
            return;
        }
    };
    ctx.last_beatmap.set(group_id, resolved_bid);

    let beatmap_path = match api::download_beatmap_osu(resolved_bid_i64).await {
        Ok(p) => p,
        Err(e) => {
            let _ = resp_tx.send(api_error_msg(qq, &e)).await;
            return;
        }
    };

    let parse_result = tokio::task::spawn_blocking({
        let path = beatmap_path.clone();
        move || -> std::result::Result<osubot_beatmap_preview::Beatmap, osubot_beatmap_preview::PreviewError> {
            let meta = std::fs::metadata(&path)
                .map_err(|e| osubot_beatmap_preview::PreviewError::new(
                    format!("read beatmap metadata: {e}")))?;
            if meta.len() > 50 * 1024 * 1024 {
                return Err(osubot_beatmap_preview::PreviewError::new(
                    "beatmap file too large (>50MB)"));
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| osubot_beatmap_preview::PreviewError::new(
                    format!("read beatmap file: {e}")))?;
            osubot_beatmap_preview::parse_beatmap_from_bytes(&bytes)
        }
    })
    .await;

    let mut beatmap = match parse_result {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_parse_failed", error = &e.to_string()));
            send_error(resp_tx, qq, "error.data_fetch_failed").await;
            return;
        }
        Err(_) => {
            send_error(resp_tx, qq, "error.render_failed").await;
            return;
        }
    };

    let mod_settings = match mods {
        Some(m) if !m.is_empty() => {
            let joined = m.join("+");
            match osubot_beatmap_preview::parse_mods(&joined) {
                Ok(s) if s.has_any_mod() => Some(s),
                Ok(_) => None,
                Err(e) => {
                    warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_mods_parse_failed", error = &e.to_string()));
                    send_error(resp_tx, qq, "error.data_fetch_failed").await;
                    return;
                }
            }
        }
        _ => None,
    };

    let target_mode = mode.map(|m| m as i32).unwrap_or_else(|| beatmap.mode());
    if let Some(ref s) = mod_settings {
        let validation_errors = osubot_beatmap_preview::validate_mods(s, Some(target_mode));
        if let Some(first) = validation_errors.first() {
            warn!(error = %first, "{}", log_fmt!("main.beatmap_preview_mods_invalid", error = &first));
            let msg = user_str("error.beatmap_preview_mods_invalid").replace("{error}", first);
            let _ = resp_tx.send(msg).await;
            return;
        }
    }

    if target_mode != beatmap.mode() {
        if beatmap.mode() != 0 {
            warn!(
                source_mode = beatmap.mode(),
                target_mode = target_mode,
                "{}",
                log_fmt!(
                    "main.beatmap_preview_convert_unsupported",
                    source_mode = beatmap.mode(),
                    target_mode = target_mode
                )
            );
            send_error(resp_tx, qq, "error.beatmap_preview_convert_unsupported").await;
            return;
        }
        let mods_for_conv = mod_settings.clone();
        let convert_result = tokio::task::spawn_blocking(move || {
            osubot_beatmap_preview::convert_beatmap(&beatmap, target_mode, mods_for_conv.as_ref())
        })
        .await;
        beatmap = match convert_result {
            Ok(Ok(b)) => b,
            Ok(Err(e)) => {
                warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_convert_failed", error = &e.to_string()));
                send_error(resp_tx, qq, "error.data_fetch_failed").await;
                return;
            }
            Err(_) => {
                send_error(resp_tx, qq, "error.render_failed").await;
                return;
            }
        };
    }

    let use_gif = *gif || target_mode == 0;
    let fmt = if use_gif { "gif" } else { "png" };
    let mod_suffix = match &mod_settings {
        Some(s) if s.has_any_mod() => s
            .tokens
            .iter()
            .map(|t| t.to_lowercase())
            .collect::<Vec<_>>()
            .join("+"),
        _ => String::new(),
    };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let filename = if mod_suffix.is_empty() {
        format!("{}_{:x}.{}", resolved_bid, nanos, fmt)
    } else {
        format!("{}_{}_{:x}.{}", resolved_bid, mod_suffix, nanos, fmt)
    };
    let output_path = osubot_core::cache::preview_cache_dir().join(&filename);

    let times_ms: Option<Vec<i64>> = match times {
        None => None,
        Some(t) if t.len() == 1 => {
            let anchor = t[0];
            let half_window = 30_000_i64;
            let window_start = (anchor - half_window).max(0);
            let window_end = (anchor + half_window).min(beatmap.end_time());
            let window_end = window_end.max(window_start);
            Some(generate_linear_samples(window_start, window_end, 4))
        }
        Some(t) if t.len() == 2 => {
            let start = t[0].min(t[1]);
            let end = t[0].max(t[1]).min(beatmap.end_time());
            let end = end.max(start);
            Some(generate_linear_samples(start, end, 4))
        }
        _ => None,
    };

    let mode_for_render = target_mode;
    let output_path_for_render = output_path.clone();
    let mods_for_render = mod_settings.clone();
    let use_gif_for_render = use_gif;
    let render_join = tokio::task::spawn_blocking(move || {
        render_beatmap_preview(
            &beatmap,
            mode_for_render,
            mods_for_render.as_ref(),
            &output_path_for_render,
            use_gif_for_render,
            times_ms,
        )
    });
    let render_timeout = Duration::from_secs(ctx.config.read().await.bot.render_timeout_secs);
    let timed = tokio::time::timeout(render_timeout, render_join).await;

    match timed {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(e))) => {
            warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_render_failed", error = &e.to_string()));
            send_error(resp_tx, qq, "error.render_failed").await;
            return;
        }
        Ok(Err(_)) => {
            send_error(resp_tx, qq, "error.render_failed").await;
            return;
        }
        Err(_) => {
            warn!("{}", log_fmt!("main.beatmap_preview_render_timeout"));
            send_error(resp_tx, qq, "error.render_timeout").await;
            return;
        }
    }

    let image_data = match tokio::fs::read(&output_path).await {
        Ok(d) => d,
        Err(e) => {
            warn!(error = %e, path = ?output_path, "{}", log_fmt!("main.beatmap_preview_read_failed", error = &e.to_string()));
            send_error(resp_tx, qq, "error.render_failed").await;
            return;
        }
    };

    let write = ctx.write.clone();
    if let Err(e) = send_group_msg_with_image(&write, group_id, &image_data).await {
        warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_send_failed", error = &e.to_string()));
    }
}

/// Main command dispatcher. Parses the command text, resolves the target user,
/// executes the appropriate query, and sends the response via `resp_tx`.
pub(crate) async fn handle_command(ctx: BotContext, msg: QQMessage, resp_tx: mpsc::Sender<String>) {
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
    if cmd_opt.is_none() {
        return;
    }
    let cmd = cmd_opt.expect("guarded by cmd_opt.is_none() early-return");

    // 命令开关检查
    let group_cfg = {
        let cfg = ctx.config.read().await;
        cfg.groups.get_group_config(msg.group_id)
    };
    if !group_cfg.is_enabled(cmd.group_name()) {
        debug!(group_id = msg.group_id, command = ?cmd.group_name(), "{}", log_fmt!("main.command_disabled"));
        return;
    }

    // 用户命令频率限制（滑动窗口：3秒内最多5次）
    let rate_limited = {
        let mut entry = ctx
            .command_rate_limits
            .entry(msg.user_id)
            .or_insert(UserRateLimit {
                last_command: std::time::Instant::now(),
                command_timestamps: Vec::new(),
            });

        let now = std::time::Instant::now();
        // 清理超过3秒的记录
        entry
            .command_timestamps
            .retain(|t| now.duration_since(*t) < Duration::from_secs(3));
        entry.command_timestamps.push(now);
        entry.last_command = now;

        // 检查是否超过限制
        entry.command_timestamps.len() > 5
    };
    if rate_limited {
        let _ = resp_tx
            .send(user_str("error.rate_limit_generic").replace("{qq}", &msg.user_id.to_string()))
            .await;
        return;
    }

    // 定期清理不活跃的用户（每60秒清理30秒内无命令的用户）
    static LAST_CLEANUP: OnceLock<std::sync::Mutex<std::time::Instant>> = OnceLock::new();
    let last = LAST_CLEANUP.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now()));
    if let Ok(mut last_time) = last.try_lock() {
        if last_time.elapsed() >= Duration::from_secs(60) {
            ctx.command_rate_limits
                .retain(|_, v| v.last_command.elapsed() < Duration::from_secs(30));
            *last_time = std::time::Instant::now();
        }
    }

    // Handle command and send response
    let mode = resolved_mode.unwrap_or(GameMode::Osu);
    match &cmd {
        Command::QuerySelf { .. }
        | Command::QueryUser { .. }
        | Command::QueryMentionedUser { .. }
        | Command::ScoreOnBeatmap { .. }
        | Command::Pass { .. }
        | Command::Recent { .. } => {
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

/// Render beatmap preview to file. Returns Ok(()) on success.
fn render_beatmap_preview(
    beatmap: &osubot_beatmap_preview::Beatmap,
    target_mode: i32,
    mods: Option<&osubot_beatmap_preview::ModSettings>,
    output_path: &std::path::Path,
    use_gif: bool,
    times_ms: Option<Vec<i64>>,
) -> std::result::Result<(), osubot_beatmap_preview::PreviewError> {
    let fmt = if use_gif { "gif" } else { "png" };

    std::fs::create_dir_all(
        output_path
            .parent()
            .expect("preview output path must have a parent dir"),
    )
    .map_err(|e| {
        osubot_beatmap_preview::PreviewError::new(format!("[{fmt}] create output dir: {e}"))
    })?;

    let result = match target_mode {
        0 => osubot_beatmap_preview::render_standard_gif(beatmap, mods, times_ms, output_path),
        1 if use_gif => {
            osubot_beatmap_preview::render_taiko_gif(beatmap, mods, times_ms.clone(), output_path)
        }
        1 => osubot_beatmap_preview::render_taiko_grid(beatmap, output_path, mods).map(|_| ()),
        2 if use_gif => {
            osubot_beatmap_preview::render_catch_gif(beatmap, mods, times_ms.clone(), output_path)
        }
        2 => osubot_beatmap_preview::render_catch_grid(beatmap, output_path, mods).map(|_| ()),
        3 if use_gif => {
            osubot_beatmap_preview::render_mania_gif(beatmap, mods, times_ms.clone(), output_path)
        }
        3 => osubot_beatmap_preview::render_mania_grid(beatmap, output_path, mods).map(|_| ()),
        _ => Err(osubot_beatmap_preview::PreviewError::new(format!(
            "unsupported mode: {target_mode}"
        ))),
    };
    result.map_err(|e| osubot_beatmap_preview::PreviewError::new(format!("[{fmt}] {e}")))
}

/// Generate `n` linearly-spaced sampling points in `[start, end]`.
fn generate_linear_samples(start: i64, end: i64, n: usize) -> Vec<i64> {
    if n <= 1 || start >= end {
        return vec![start];
    }
    let step = (end - start) / (n - 1) as i64;
    (0..n).map(|i| start + step * i as i64).collect()
}
