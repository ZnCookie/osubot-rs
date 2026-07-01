use super::*;
use futures_util::stream::{self, StreamExt};
use std::borrow::Cow;

pub(super) struct SingleScoreRenderParams<'a> {
    pub(super) ctx: &'a BotContext,
    pub(super) msg: &'a QQMessage,
    pub(super) resp_tx: &'a mpsc::Sender<String>,
    pub(super) score: &'a Score,
    pub(super) mode: GameMode,
    pub(super) user_stats: &'a UserStats,
    /// 在所属列表中的 0-索引位置。`Some(n)` 时渲染前缀 `"#(n+1) <label>"`；
    /// `None` 时仅渲染 `<label>`，适用于"该玩家在此谱面上的最佳成绩"等无明确排名的场景。
    pub(super) position: Option<usize>,
    pub(super) label: &'static str,
}

pub(super) async fn render_and_send_single_score(params: SingleScoreRenderParams<'_>) {
    let SingleScoreRenderParams {
        ctx,
        msg,
        resp_tx,
        score,
        mode,
        user_stats,
        position,
        label,
    } = params;
    let mut score = score.clone();
    if score.pp_breakdown.is_none() {
        enrich_score_with_pp(&mut score, mode, true).await;
    }

    let ur_value = if mode == GameMode::Osu && score.score_id > 0 && score.has_replay {
        tracing::trace!(score_id = score.score_id, mode = ?mode, is_lazer = score.is_lazer, length = score.length_seconds, "{}", log_fmt!("main.ur_calculation_start"));
        let ur_params = osubot_core::ur::ScoreUrParams {
            score_id: score.score_id,
            legacy_score_id: score.legacy_score_id,
            beatmap_id: score.beatmap_id,
            mode,
            mods: score.mods.clone(),
            status: score.status.clone(),
        };
        let ur_timeout = Duration::from_secs(ctx.config.read().await.bot.ur_timeout_secs);
        match tokio::time::timeout(
            ur_timeout,
            osubot_core::ur::calculate_score_ur(&ctx.rate_limiter, &ctx.oauth, ur_params),
        )
        .await
        {
            Ok(Some(ur_val)) => {
                tracing::debug!(
                    score_id = score.score_id,
                    total_ur = ur_val,
                    "{}",
                    log_fmt!("main.ur_calculation_succeeded")
                );
                Some(ur_val)
            }
            Ok(None) => {
                tracing::warn!(
                    score_id = score.score_id,
                    "{}",
                    log_fmt!("main.ur_calculation_none")
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    score_id = score.score_id,
                    "{}",
                    log_fmt!("main.ur_calculation_timeout")
                );
                None
            }
        }
    } else {
        tracing::trace!(
            score_id = score.score_id,
            mode = ?mode,
            is_lazer = score.is_lazer,
            has_replay = score.has_replay,
            "{}",
            log_fmt!("main.ur_calculation_skipped")
        );
        None
    };

    let (ar_eff, od_eff, cs_eff, hp_eff) = {
        let (a, o, c, h) = apply_mod_adjustment_to_stats(
            mode,
            score.ar,
            score.od,
            score.cs,
            score.hp,
            &score.mods,
        );
        let same = (a - score.ar).abs() < 0.01
            && (o - score.od).abs() < 0.01
            && (c - score.cs).abs() < 0.01
            && (h - score.hp).abs() < 0.01;
        if same {
            (None, None, None, None)
        } else {
            (Some(a), Some(o), Some(c), Some(h))
        }
    };

    let cover_image: Option<image::DynamicImage> = if !score.cover_url.is_empty() {
        match render_cache::fetch_and_cache(&score.cover_url, render_cache::http_client(), false)
            .await
        {
            Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
            Err(_) => None,
        }
    } else {
        None
    };

    let play_time = format_play_datetime(&score.created_at);
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel_flag.clone();

    let change = ctx
        .storage
        .calculate_change(user_stats.user_id, mode, user_stats)
        .await
        .ok()
        .flatten();
    let pp_change = change.as_ref().and_then(|c| c.pp_change);
    let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
    let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);

    let render_timeout = Duration::from_secs(ctx.config.read().await.bot.render_timeout_secs);
    let render_result = tokio::time::timeout(
        render_timeout,
        render_score_card(osubot_render::ScoreCardParams {
            user: osubot_render::UserContext {
                username: &user_stats.username,
                mode,
                user_pp: user_stats.pp,
                user_global_rank: if user_stats.rank > 0 {
                    Some(user_stats.rank)
                } else {
                    None
                },
                user_country_rank: if user_stats.country_rank > 0 {
                    Some(user_stats.country_rank)
                } else {
                    None
                },
                country_code: &user_stats.country_code,
                avatar_url: &format!("https://a.ppy.sh/{}", user_stats.user_id),
                pp_change,
                global_rank_change,
                country_rank_change,
            },
            score: &score,
            play_time: &play_time,
            fav_count: score.fav_count,
            play_count: score.play_count,
            ranked_status: &score.status,
            ur_value,
            ar_eff,
            od_eff,
            cs_eff,
            hp_eff,
            cover_image,
            cancel_flag: Some(cancel_clone),
        }),
    )
    .await;

    let qq = msg.user_id;

    match render_result {
        Ok(Ok(jpeg_bytes)) => {
            let write = ctx.write.clone();
            let onebot_api = ctx.onebot_api.clone();
            let group_id = msg.group_id.unwrap_or(0);
            let resp_tx_img = resp_tx.clone();

            tokio::spawn(async move {
                if send_group_msg_with_image(&write, &onebot_api, group_id, &jpeg_bytes)
                    .await
                    .is_err()
                {
                    let _ = resp_tx_img
                        .send(user_str("error.image_send_failed").replace("{qq}", &qq.to_string()))
                        .await;
                }
            });
        }
        Ok(Err(e)) => {
            warn!(error = %e, "{}", log_fmt!("main.render_score_card_failed_text"));
            let text = format_score(&score, &user_stats.username, mode, position, label);
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            cancel_flag.store(true, Ordering::Relaxed);
            warn!("{}", log_fmt!("main.render_score_card_timeout_text"));
            let text = format_score(&score, &user_stats.username, mode, position, label);
            let _ = resp_tx.send(text).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_and_send_score_list(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    scores: &[Score],
    user_stats: &UserStats,
    username: &str,
    mode: GameMode,
    label_key: &'static str,
    original_indices: &[usize],
) {
    let cover_images: Vec<Option<image::DynamicImage>> = join_all(scores.iter().map(|s| {
        let url = s.cover_url.clone();
        async move {
            if url.is_empty() {
                return None;
            }
            match render_cache::fetch_and_cache(&url, render_cache::http_client(), false).await {
                Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
                Err(_) => None,
            }
        }
    }))
    .await;

    let pp_enrich_indices: Vec<usize> = scores
        .iter()
        .enumerate()
        .filter(|(_, s)| s.pp.is_none() && s.beatmap_id > 0)
        .map(|(i, _)| i)
        .collect();
    let scores: Cow<'_, [Score]> = if pp_enrich_indices.is_empty() {
        Cow::Borrowed(scores)
    } else {
        let mut owned: Vec<Score> = scores.to_vec();
        let mut slots: Vec<Option<Score>> = owned.drain(..).map(Some).collect();
        let futs = pp_enrich_indices.into_iter().filter_map(|i| {
            let score = slots[i].take()?;
            Some(async move {
                let mut s = score;
                enrich_score_with_pp(&mut s, mode, false).await;
                (i, s)
            })
        });
        let enriched: Vec<(usize, Score)> = stream::iter(futs)
            .buffer_unordered(ENRICH_CONCURRENCY)
            .collect()
            .await;
        for (i, s) in enriched {
            slots[i] = Some(s);
        }
        Cow::Owned(slots.into_iter().flatten().collect())
    };

    let avatar_url = format!("https://a.ppy.sh/{}", user_stats.user_id);
    let hero_cover_url = user_stats.cover_url.clone().unwrap_or_default();
    let user_global_rank = if user_stats.rank > 0 {
        Some(user_stats.rank)
    } else {
        None
    };
    let user_country_rank = if user_stats.country_rank > 0 {
        Some(user_stats.country_rank)
    } else {
        None
    };

    let change = ctx
        .storage
        .calculate_change(user_stats.user_id, mode, user_stats)
        .await
        .inspect_err(|e| {
            tracing::warn!(
                user_id = user_stats.user_id,
                mode = ?mode,
                error = %e,
                "{}",
                log_fmt!("main.calculate_change_failed")
            )
        })
        .ok()
        .flatten();
    let pp_change = change.as_ref().and_then(|c| c.pp_change);
    let global_rank_change = change.as_ref().and_then(|c| c.rank_change);
    let country_rank_change = change.as_ref().and_then(|c| c.country_rank_change);

    let score_label = user_str(label_key);
    let score_count_text = user_str("fmt.score_count");
    let render_result = tokio::time::timeout(
        Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
        render_score_list_card(osubot_render::ScoreListCardParams {
            user: osubot_render::UserContext {
                username,
                mode,
                user_pp: user_stats.pp,
                user_global_rank,
                user_country_rank,
                country_code: &user_stats.country_code,
                avatar_url: &avatar_url,
                pp_change,
                global_rank_change,
                country_rank_change,
            },
            scores: &scores,
            label: score_label,
            count_text: score_count_text,
            cover_images,
            hero_cover_url: &hero_cover_url,
            original_indices,
        }),
    )
    .await;

    let qq = msg.user_id;

    match render_result {
        Ok(Ok(jpeg_bytes)) => {
            let write = ctx.write.clone();
            let onebot_api = ctx.onebot_api.clone();
            let group_id = msg.group_id.unwrap_or(0);
            let resp_tx_img = resp_tx.clone();

            tokio::spawn(async move {
                if send_group_msg_with_image(&write, &onebot_api, group_id, &jpeg_bytes)
                    .await
                    .is_err()
                {
                    let _ = resp_tx_img
                        .send(user_str("error.image_send_failed").replace("{qq}", &qq.to_string()))
                        .await;
                }
            });
        }
        Ok(Err(e)) => {
            warn!(error = %e, "{}", log_fmt!("main.render_score_list_failed_text"));
            let text = format_scores(
                &scores,
                username,
                mode,
                user_str(label_key),
                original_indices,
            );
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            warn!("{}", log_fmt!("main.render_score_list_timeout_text"));
            let text = format_scores(
                &scores,
                username,
                mode,
                user_str(label_key),
                original_indices,
            );
            let _ = resp_tx.send(text).await;
        }
    }
}
