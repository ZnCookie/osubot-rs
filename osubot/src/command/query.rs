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
        Command::ScoreOnBeatmap { .. }
        | Command::Pass { .. }
        | Command::Recent { .. }
        | Command::Best { .. }
        | Command::TodayBest { .. }
        | Command::BeatmapAudio { .. }
        | Command::BeatmapPreview { .. } => {
            info!(
                user_id = msg.user_id,
                group_id = msg.group_id,
                mode = ?mode,
                "{}",
                log_fmt!("main.score_query_command")
            );
            handle_score_query(ctx, msg, resp_tx, cmd, mode).await;
        }
        _ => unreachable!("handle_query_commands called with non-query command"),
    }
}

pub(super) async fn handle_sb_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
) {
    info!(
        user_id = msg.user_id,
        group_id = msg.group_id,
        "{}",
        log_fmt!("main.sb_query_command")
    );

    let target_qq = resolve_cmd_target_qq(cmd, msg, &ctx.storage).await;
    let qq = target_qq.unwrap_or(msg.user_id);

    let binding = match ctx.storage.sb_get_binding(qq).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            let _ = resp_tx
                .send(user_str("sb.not_bound").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }
        Err(e) => {
            error!(user_id = qq, error = %e, "{}", log_fmt!("main.sb_get_binding_error"));
            let _ = resp_tx
                .send(user_str("error.db_error").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }
    };

    let explicit_mode = extract_explicit_mode(cmd);
    let mode = resolve_sb_mode_to_u8(&ctx.storage, qq, explicit_mode).await;

    let (info, stats) =
        match api::sb_api::get_player_info(binding.sb_user_id, &ctx.sb_rate_limiter).await {
            Ok(result) => result,
            Err(e) => {
                warn!(user_id = qq, error = ?e, "{}", log_fmt!("main.sb_api_fetch_failed"));
                let _ = resp_tx
                    .send(user_str("error.query_failed").replace("{qq}", &msg.user_id.to_string()))
                    .await;
                return;
            }
        };

    let mode_key = mode.to_string();
    let s = stats.get(&mode_key).or_else(|| stats.get("0"));

    let mut reply = format!(
        "[CQ:at,qq={qq}] ppy.sb 玩家：{name} (ID: {id})\n",
        qq = msg.user_id,
        name = info.name,
        id = info.id,
    );

    if let Some(s) = s {
        let mode_name = GameMode::try_from(mode)
            .map(|m| m.name().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        reply.push_str(&format!(
            "模式：{mode} | PP：{pp:.0} | 准确率：{acc:.2}%\n\
             排名：#{rank_global} (全球) / #{rank_country} (地区)\n\
             游戏次数：{plays} | 总分：{tscore}",
            mode = mode_name,
            pp = s.pp,
            acc = s.accuracy,
            rank_global = s.global_rank,
            rank_country = s.country_rank,
            plays = s.play_count,
            tscore = s.total_score,
        ));
    } else {
        reply.push_str("该模式无统计数据");
    }

    if let Some(s) = stats.get(&mode_key).or_else(|| stats.get("0")) {
        let _ = ctx
            .storage
            .sb_save_snapshot(
                qq,
                mode,
                Some(s.pp),
                Some(s.global_rank),
                Some(s.country_rank),
            )
            .await;
    }

    let _ = resp_tx.send(reply).await;
}

pub(super) async fn resolve_sb_mode_to_u8(
    storage: &Storage,
    qq: i64,
    explicit: Option<GameMode>,
) -> u8 {
    if let Some(m) = explicit {
        return m as u8;
    }
    match storage.sb_get_default_mode(qq).await {
        Ok(Some(m)) => m,
        Ok(None) => 0,
        Err(e) => {
            warn!(user_id = qq, error = %e, "{}", log_fmt!("main.sb_get_default_mode_error"));
            0
        }
    }
}
