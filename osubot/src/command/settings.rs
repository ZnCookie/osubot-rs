use super::*;
use osubot_core::types::Server;

/// Handle settings commands: SetDefaultMode, Bind, Unbind.
/// Dispatches to SB-specific handlers when the server is PpsySb.
pub(super) async fn handle_settings_commands(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
) {
    match cmd.server() {
        Server::PpsySb => {
            match cmd {
                Command::Bind { username, .. } => {
                    handle_sb_bind(ctx, msg, resp_tx, username).await;
                }
                Command::Unbind { .. } => {
                    handle_sb_unbind(ctx, msg, resp_tx).await;
                }
                Command::SetDefaultMode { mode, .. } => {
                    handle_sb_set_mode(ctx, msg, resp_tx, *mode).await;
                }
                _ => {
                    unreachable!("handle_settings_commands called with non-settings command for SB")
                }
            }
            return;
        }
        Server::Official => {}
    }

    match cmd {
        Command::SetDefaultMode { mode, .. } => {
            if let Some(m) = mode {
                if m.is_sb_specific() {
                    let _ = resp_tx
                        .send(
                            user_str("mode.sb_mode_on_official")
                                .replace("{qq}", &msg.user_id.to_string()),
                        )
                        .await;
                    return;
                }
            }
            match mode {
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
            }
        }
        Command::Bind { username, .. } => {
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
        Command::Unbind { .. } => {
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

async fn handle_sb_bind(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    username: &str,
) {
    info!(
        user_id = msg.user_id,
        username = %username,
        "{}",
        log_fmt!("main.sb_bind_command")
    );

    let config = ctx.config.read().await;
    if config.irc.enabled && config.sb.sb_bind_require_official_bind {
        match ctx.storage.get_binding(msg.user_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                let _ = resp_tx
                    .send(
                        user_str("sb.bind.require_official")
                            .replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
            Err(e) => {
                error!(
                    user_id = msg.user_id,
                    error = %e,
                    "{}",
                    log_fmt!(
                        "main.sb_bind_check_binding_error",
                        user_id = &msg.user_id.to_string(),
                        error = &e.to_string()
                    )
                );
                let _ = resp_tx
                    .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                    .await;
                return;
            }
        }
    }
    drop(config);

    match api::sb_api::search_player(username, &ctx.rate_limiter).await {
        Ok(players) => match players.into_iter().next() {
            Some(player) => {
                match ctx
                    .storage
                    .sb_bind(msg.user_id, player.id, &player.name)
                    .await
                {
                    Ok(Ok(())) => {
                        info!(
                            user_id = msg.user_id,
                            sb_user_id = player.id,
                            sb_username = %player.name,
                            "{}",
                            log_fmt!(
                                "main.sb_bind_success",
                                user_id = &msg.user_id.to_string(),
                                sb_user_id = &player.id.to_string(),
                                sb_username = &player.name
                            )
                        );
                        let _ = resp_tx
                            .send(
                                user_str("sb.bind.success")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{username}", &player.name)
                                    .replace("{id}", &player.id.to_string()),
                            )
                            .await;
                    }
                    Ok(Err(existing_qq)) => {
                        if existing_qq == msg.user_id {
                            info!(
                                user_id = msg.user_id,
                                "{}",
                                log_fmt!(
                                    "main.sb_bind_already_self",
                                    user_id = &msg.user_id.to_string()
                                )
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("sb.bind.already_self")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        } else {
                            info!(
                                user_id = msg.user_id,
                                existing_qq = existing_qq,
                                "{}",
                                log_fmt!(
                                    "main.sb_bind_already_other",
                                    user_id = &msg.user_id.to_string(),
                                    existing_qq = &existing_qq.to_string()
                                )
                            );
                            let _ = resp_tx
                                .send(
                                    user_str("sb.bind.already_other")
                                        .replace("{qq}", &msg.user_id.to_string()),
                                )
                                .await;
                        }
                    }
                    Err(e) => {
                        error!(
                            user_id = msg.user_id,
                            error = %e,
                            "{}",
                            log_fmt!(
                                "main.sb_bind_db_error",
                                user_id = &msg.user_id.to_string(),
                                error = &e.to_string()
                            )
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
            None => {
                info!(
                    username = %username,
                    "{}",
                    log_fmt!("main.sb_bind_not_found", username = username)
                );
                let _ = resp_tx
                    .send(
                        user_str("sb.bind.not_found")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{username}", username),
                    )
                    .await;
            }
        },
        Err(e) => {
            error!(
                user_id = msg.user_id,
                error = ?e,
                "{}",
                log_fmt!(
                    "main.sb_bind_search_error",
                    user_id = &msg.user_id.to_string()
                )
            );
            let _ = resp_tx
                .send(user_str("error.internal").replace("{qq}", &msg.user_id.to_string()))
                .await;
        }
    }
}

async fn handle_sb_unbind(ctx: &BotContext, msg: &QQMessage, resp_tx: &mpsc::Sender<String>) {
    info!(
        user_id = msg.user_id,
        group_id = msg.group_id,
        "{}",
        log_fmt!("main.sb_unbind_command")
    );
    match ctx.storage.get_pending_unbind(msg.user_id).await {
        Ok(Some(_)) => match ctx.storage.sb_unbind(msg.user_id).await {
            Ok(_) => {
                if let Err(e) = ctx.storage.remove_pending_unbind(msg.user_id).await {
                    tracing::warn!(
                        user_id = msg.user_id,
                        error = %e,
                        "{}",
                        log_fmt!("main.sb_unbind_remove_pending_failed")
                    );
                }
                info!(
                    user_id = msg.user_id,
                    "{}",
                    log_fmt!("main.sb_unbind_success")
                );
                let _ = resp_tx
                    .send(user_str("sb.unbind.success").replace("{qq}", &msg.user_id.to_string()))
                    .await;
            }
            Err(e) => {
                error!(
                    user_id = msg.user_id,
                    error = %e,
                    "{}",
                    log_fmt!(
                        "main.sb_unbind_error",
                        user_id = &msg.user_id.to_string(),
                        error = &e.to_string()
                    )
                );
                let _ = resp_tx
                    .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                    .await;
            }
        },
        Ok(None) => match ctx.storage.sb_get_binding(msg.user_id).await {
            Ok(Some(binding)) => {
                if let Err(e) = ctx.storage.set_pending_unbind(msg.user_id).await {
                    tracing::warn!(
                        user_id = msg.user_id,
                        error = %e,
                        "{}",
                        log_fmt!("main.sb_unbind_set_pending_failed")
                    );
                }
                info!(
                    user_id = msg.user_id,
                    sb_username = %binding.sb_username,
                    "{}",
                    log_fmt!("main.sb_unbind_confirmation")
                );
                let _ = resp_tx
                    .send(user_str("sb.unbind.prompt").replace("{qq}", &msg.user_id.to_string()))
                    .await;
            }
            Ok(None) => {
                info!(
                    user_id = msg.user_id,
                    "{}",
                    log_fmt!("main.sb_unbind_not_bound")
                );
                let _ = resp_tx
                    .send(user_str("sb.unbind.not_bound").replace("{qq}", &msg.user_id.to_string()))
                    .await;
            }
            Err(e) => {
                error!(
                    user_id = msg.user_id,
                    error = %e,
                    "{}",
                    log_fmt!(
                        "main.sb_unbind_check_error",
                        user_id = &msg.user_id.to_string(),
                        error = &e.to_string()
                    )
                );
                let _ = resp_tx
                    .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                    .await;
            }
        },
        Err(e) => {
            error!(
                user_id = msg.user_id,
                error = %e,
                "{}",
                log_fmt!(
                    "main.sb_unbind_pending_check_error",
                    user_id = &msg.user_id.to_string(),
                    error = &e.to_string()
                )
            );
            let _ = resp_tx
                .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                .await;
        }
    }
}

async fn handle_sb_set_mode(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    mode: Option<GameMode>,
) {
    match mode {
        Some(m) => {
            info!(
                user_id = msg.user_id,
                ?mode,
                "{}",
                log_fmt!(
                    "main.sb_set_mode",
                    user_id = &msg.user_id.to_string(),
                    mode = &format!("{:?}", m)
                )
            );
            match ctx.storage.sb_set_default_mode(msg.user_id, m as u8).await {
                Ok(true) => {
                    let _ = resp_tx
                        .send(
                            user_str("sb.mode.set")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{mode_name}", m.name()),
                        )
                        .await;
                }
                Ok(false) => {
                    let _ = resp_tx
                        .send(
                            user_str("sb.mode.not_bound").replace("{qq}", &msg.user_id.to_string()),
                        )
                        .await;
                }
                Err(e) => {
                    error!(
                        user_id = msg.user_id,
                        error = %e,
                        "{}",
                        log_fmt!(
                            "main.sb_set_mode_error",
                            user_id = &msg.user_id.to_string(),
                            error = &e.to_string()
                        )
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
        None => {
            info!(
                user_id = msg.user_id,
                "{}",
                log_fmt!("main.sb_get_mode", user_id = &msg.user_id.to_string())
            );
            match ctx.storage.sb_get_binding(msg.user_id).await {
                Ok(Some(_)) => match ctx.storage.sb_get_default_mode(msg.user_id).await {
                    Ok(Some(current)) => {
                        let mode_name = GameMode::try_from(current)
                            .map(|m| m.name().to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        let _ = resp_tx
                            .send(
                                user_str("sb.mode.get")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{mode}", &current.to_string())
                                    .replace("{mode_name}", &mode_name),
                            )
                            .await;
                    }
                    Ok(None) => {
                        let _ = resp_tx
                            .send(
                                user_str("sb.mode.get")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{mode}", "0")
                                    .replace("{mode_name}", GameMode::Osu.name()),
                            )
                            .await;
                    }
                    Err(e) => {
                        error!(
                            user_id = msg.user_id,
                            error = %e,
                            "{}",
                            log_fmt!(
                                "main.sb_get_mode_error",
                                user_id = &msg.user_id.to_string(),
                                error = &e.to_string()
                            )
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
                            user_str("sb.mode.not_bound").replace("{qq}", &msg.user_id.to_string()),
                        )
                        .await;
                }
                Err(e) => {
                    error!(
                        user_id = msg.user_id,
                        error = %e,
                        "{}",
                        log_fmt!(
                            "main.sb_get_mode_error",
                            user_id = &msg.user_id.to_string(),
                            error = &e.to_string()
                        )
                    );
                    let _ = resp_tx
                        .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                        .await;
                }
            }
        }
    }
}
