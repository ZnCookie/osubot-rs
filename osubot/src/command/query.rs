use super::*;

/// Handle query commands: QuerySelf, QueryUser, QueryMentionedUser, ScoreOnBeatmap, Pass, Recent, Best, BeatmapAudio.
pub(super) async fn handle_query_commands(
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
        Command::Best { .. } => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                mode = ?mode,
                "{}",
                log_fmt!("main.best_score_command")
            );
            handle_best_score_query(ctx, msg, resp_tx, cmd, mode).await;
        }
        Command::BeatmapAudio {
            score_id,
            beatmap_id,
            username,
            qq,
            filters,
            limit,
            explicit_position,
            mode: cmd_mode,
            ..
        } => {
            handle_beatmap_audio(
                ctx,
                msg,
                resp_tx,
                BeatmapAudioParams {
                    score_id: *score_id,
                    beatmap_id: *beatmap_id,
                    username: username.clone(),
                    qq: *qq,
                    mode,
                    mode_specified: cmd_mode.is_some(),
                    filters: filters.clone(),
                    limit: *limit,
                    explicit_position: *explicit_position,
                },
            )
            .await;
        }
        _ => unreachable!("handle_query_commands called with non-query command"),
    }
}
