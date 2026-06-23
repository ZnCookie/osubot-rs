use super::*;

/// Handle settings commands: SetDefaultMode, Bind, Unbind.
pub(super) async fn handle_settings_commands(
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
