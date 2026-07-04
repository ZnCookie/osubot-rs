use super::*;

/// Handle utility commands: Help, Highlight, ProfileCard, BeatmapPreview.
pub(super) async fn handle_utility_commands(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    match cmd {
        Command::Help { server } => {
            handle_help_command(ctx, msg, resp_tx, *server).await;
        }
        Command::Highlight { server, .. } => {
            handle_highlight_command(ctx, msg, resp_tx, mode, *server).await;
        }
        Command::ProfileCard { username, qq } => {
            handle_profile_card(ctx, msg, resp_tx, username, qq, mode).await;
        }
        _ => unreachable!("handle_utility_commands called with non-utility command"),
    }
}

async fn handle_help_command(
    _ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    server: Server,
) {
    info!(
        user_id = msg.user_id,
        group_id = ?msg.group_id,
        "{}",
        log_fmt!("main.help_command")
    );
    let help_text = match server {
        Server::Official => user_str("sys.help"),
        Server::PpySb => user_str("sys.help_ppy_sb"),
    };
    let _ = resp_tx
        .send(help_text.replace("{qq}", &msg.user_id.to_string()))
        .await;
}

async fn handle_highlight_command(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    mode: GameMode,
    server: Server,
) {
    info!(user_id = msg.user_id, group_id = ?msg.group_id, mode = ?mode, "{}", log_fmt!("main.highlight_command"));

    let group_members = match msg.group_id {
        Some(gid) => match get_group_member_list(&ctx.write, &ctx.onebot_api, gid).await {
            Ok(m) => m,
            Err(_) => {
                let _ = resp_tx
                    .send(
                        user_str("error.get_group_members")
                            .replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
        },
        None => {
            let _ = resp_tx
                .send(
                    user_str("highlight.not_supported_in_private")
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
        server,
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
            Some(mentioned_qq) => match ctx
                .storage
                .get_binding(*mentioned_qq, Server::Official)
                .await
            {
                Ok(Some((user_id, current_username))) => {
                    info!(qq = mentioned_qq, osu_id = user_id, username = %current_username, "{}", log_fmt!("main.profile_card_mention"));
                    user_id
                }
                Ok(None) => {
                    if let Some((uid, uname)) =
                        ctx.resolve_binding(*mentioned_qq, Server::Official).await
                    {
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
            None => match ctx.storage.get_binding(msg.user_id, Server::Official).await {
                Ok(Some((user_id, current_username))) => {
                    info!(user_id = msg.user_id, osu_id = user_id, username = %current_username, "{}", log_fmt!("main.profile_card_self"));
                    user_id
                }
                Ok(None) => {
                    if let Some((uid, uname)) =
                        ctx.resolve_binding(msg.user_id, Server::Official).await
                    {
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
            let onebot_api = ctx.onebot_api.clone();
            let resp_tx = resp_tx.clone();
            let msg_group_id = msg.group_id;
            let msg_user_id = msg.user_id;

            tokio::spawn(async move {
                let send_result = match msg_group_id {
                    Some(gid) => {
                        send_group_msg_with_image(&write, &onebot_api, gid, &jpeg_bytes).await
                    }
                    None => {
                        send_private_msg_with_image(&write, &onebot_api, msg_user_id, &jpeg_bytes)
                            .await
                    }
                };
                if send_result.is_err() {
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
