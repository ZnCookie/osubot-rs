use std::collections::HashMap;

use futures_util::stream::{self, StreamExt};

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
        Command::Help => {
            handle_help_command(ctx, msg, resp_tx).await;
        }
        Command::Highlight { .. } => {
            handle_highlight_command(ctx, msg, resp_tx, mode).await;
        }
        Command::ProfileCard { username, qq, .. } => {
            handle_profile_card(ctx, msg, resp_tx, username, qq, mode).await;
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
            let onebot_api = ctx.onebot_api.clone();
            let group_id = msg.group_id;
            let resp_tx = resp_tx.clone();

            tokio::spawn(async move {
                if send_group_msg_with_image(&write, &onebot_api, group_id, &jpeg_bytes)
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

pub(super) async fn handle_sb_highlight(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
) {
    info!(
        user_id = msg.user_id,
        group_id = msg.group_id,
        "{}",
        log_fmt!("main.sb_highlight_command")
    );

    let bindings = match ctx.storage.sb_get_all_bindings().await {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "{}", log_fmt!("main.sb_get_all_bindings_error"));
            let _ = resp_tx
                .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }
    };

    let snapshots = match ctx.storage.sb_get_all_snapshots().await {
        Ok(s) => s,
        Err(e) => {
            error!(error = %e, "{}", log_fmt!("main.sb_get_all_snapshots_error"));
            let _ = resp_tx
                .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }
    };

    let mut entries: Vec<(i64, String, f64)> = Vec::new();
    let mut fetched: HashMap<
        i64,
        (
            api::sb_api::SbPlayerInfo,
            HashMap<String, api::sb_api::SbPlayerStats>,
        ),
    > = HashMap::new();

    let fetch_targets: Vec<_> = bindings
        .iter()
        .map(|b| (b.sb_user_id, b.qq, b.sb_username.clone(), b.default_mode))
        .collect();

    let rate_limiter = ctx.sb_rate_limiter.clone();
    let results: Vec<_> = stream::iter(fetch_targets.into_iter().map(
        |(sb_user_id, qq, username, default_mode)| {
            let rate_limiter = rate_limiter.clone();
            let snapshots = snapshots.clone();
            async move {
                let result = api::sb_api::get_player_info(sb_user_id, &rate_limiter).await;
                (sb_user_id, qq, username, default_mode, result, snapshots)
            }
        },
    ))
    .buffer_unordered(10)
    .collect()
    .await;

    for (sb_user_id, qq, username, default_mode, result, snapshots) in results {
        if let Ok(result) = result {
            let mode_key = default_mode.to_string();
            let current_pp = result
                .1
                .get(&mode_key)
                .or_else(|| result.1.get("0"))
                .map(|s| s.pp)
                .unwrap_or(0.0);

            let snapshot_pp = snapshots
                .iter()
                .find(|(sqq, mode, _, _, _)| *sqq == qq && *mode == default_mode)
                .map(|(_, _, pp, _, _)| *pp)
                .unwrap_or(0.0);

            let delta = current_pp - snapshot_pp;
            if delta > 0.0 {
                entries.push((qq, username, delta));
            }

            fetched.insert(sb_user_id, result);
        }
    }

    entries.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

    if entries.is_empty() {
        let _ = resp_tx
            .send(user_str("sb.highlight.empty").replace("{qq}", &msg.user_id.to_string()))
            .await;
        return;
    }

    let mut reply = format!(
        "[CQ:at,qq={qq}] ═══ ppy.sb 今日高光 ═══\n",
        qq = msg.user_id,
    );
    for (i, (_, name, delta)) in entries.iter().take(10).enumerate() {
        reply.push_str(&format!("\n{}. {}  +{:.0} PP", i + 1, name, delta));
    }

    let _ = resp_tx.send(reply).await;

    for (sb_user_id, (_, stats)) in &fetched {
        if let Some(binding) = bindings.iter().find(|b| b.sb_user_id == *sb_user_id) {
            let mode_key = binding.default_mode.to_string();
            if let Some(s) = stats.get(&mode_key).or_else(|| stats.get("0")) {
                let _ = ctx
                    .storage
                    .sb_save_snapshot(
                        binding.qq,
                        binding.default_mode,
                        Some(s.pp),
                        Some(s.global_rank),
                        Some(s.country_rank),
                    )
                    .await;
            }
        }
    }
}
