use crate::score_filter::{score_matches_filters, ScoreQueryParams};
use crate::BotContext;
use futures_util::future::join_all;
use osubot_core::apply_mod_adjustment_to_stats;
use osubot_core::enrich_score_with_pp;
use osubot_core::{
    api, log_fmt,
    response::{format_score, format_scores},
    strings::user_str,
    types::{format_play_datetime, Command, GameMode, Score, UserStats},
};
use osubot_render::cache as render_cache;
use osubot_render::SCORE_LIST_RENDER_TIMEOUT_SECS;
use osubot_render::{render_score_card, render_score_list_card};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::api_error_msg;
use crate::onebot::{send_group_msg_with_image, QQMessage};
use crate::{
    beatmap_score_dedup, beatmap_scores_dedup, score_by_id_dedup, score_dedup,
    SCORE_API_FETCH_LIMIT,
};

async fn resolve_score_user(
    ctx: &BotContext,
    msg: &QQMessage,
    username: &Option<String>,
    qq: &Option<i64>,
    mode: GameMode,
    resp_tx: &mpsc::Sender<String>,
) -> Option<(i64, String, UserStats)> {
    tracing::trace!("{}", log_fmt!("main.resolve_score_user_start"));

    if let Some(ref name) = username {
        tracing::trace!(
            "{}",
            log_fmt!("main.resolve_score_user_lookup", username = name)
        );
        match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, name, mode).await {
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
                Some((stats.user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, username = %name, "{}", log_fmt!("main.resolve_score_user_api_failed"));
                let err_msg = match e {
                    api::ApiError::NotFound => user_str("error.not_found_named")
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{name}", name),
                    other => api_error_msg(msg.user_id, &other),
                };
                let _ = resp_tx.send(err_msg).await;
                None
            }
        }
    } else {
        let (user_id, _stored_name, error_msg) = if let Some(mentioned_qq) = qq {
            match ctx.resolve_binding(*mentioned_qq).await {
                Some((user_id, name)) => (user_id, name, None),
                None => (
                    0,
                    String::new(),
                    Some(user_str("bind.user_not_bound").replace("{qq}", &msg.user_id.to_string())),
                ),
            }
        } else {
            match ctx.resolve_binding(msg.user_id).await {
                Some((user_id, name)) => (user_id, name, None),
                None => (
                    0,
                    String::new(),
                    Some(user_str("bind.not_bound").replace("{qq}", &msg.user_id.to_string())),
                ),
            }
        };
        if let Some(err) = error_msg {
            let _ = resp_tx.send(err).await;
            return None;
        }
        tracing::info!(
            "{}",
            log_fmt!("main.resolve_score_user_fetch_stats", user_id = user_id)
        );
        match api::fetch_user_stats_by_user_id(&ctx.rate_limiter, &ctx.oauth, user_id, mode).await {
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
                Some((user_id, stats.username.clone(), stats))
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id, "{}", log_fmt!("main.resolve_score_user_lookup_bound_failed"));
                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                None
            }
        }
    }
}

pub(crate) async fn handle_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    params: ScoreQueryParams<'_>,
    mode: GameMode,
) {
    tracing::trace!("{}", log_fmt!("main.handle_score_query_start"));

    let is_self = params.username.is_none() && params.qq.is_none();
    let include_fails = !params.is_pass;
    let raw_limit = params.limit_end.unwrap_or(params.limit);
    let has_client_filter = params.filters.is_some_and(|f| !f.is_empty())
        || params.beatmap_id.is_some()
        || params.score_id.is_some();
    let api_limit = if has_client_filter {
        raw_limit.max(SCORE_API_FETCH_LIMIT)
    } else {
        raw_limit
    };
    let (user_id, resolved_username, user_stats, score_result) = if is_self {
        let (uid, name) = match ctx.resolve_binding(msg.user_id).await {
            Some(binding) => binding,
            None => {
                let _ = resp_tx
                    .send(user_str("bind.not_bound").replace("{qq}", &msg.user_id.to_string()))
                    .await;
                return;
            }
        };

        tracing::trace!(
            "{}",
            log_fmt!(
                "main.handle_score_query_bound",
                user_id = uid,
                username = name
            )
        );
        ctx.scheduler.trigger_update(uid, mode).await;

        let is_pass = params.is_pass;
        let qq = msg.user_id;
        let rate_limiter = ctx.rate_limiter.clone();
        let oauth = ctx.oauth.clone();
        let rl2 = rate_limiter.clone();
        let oa2 = oauth.clone();

        let (stats_result, scores) = tokio::join!(
            api::fetch_user_stats_by_user_id(&rate_limiter, &oauth, uid, mode),
            score_dedup().run_or_wait((uid, is_pass, api_limit, mode), move || {
                let rate_limiter = rl2.clone();
                let oauth = oa2.clone();

                async move {
                    api::get_user_recent(&rate_limiter, &oauth, uid, mode, include_fails, api_limit)
                        .await
                        .map(Arc::new)
                        .map_err(|e| {
                            warn!(user_id = uid, mode = ?mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                            if !matches!(e, api::ApiError::NotFound | api::ApiError::RateLimitedWithRetryAfter(_) | api::ApiError::ClientRateLimited) {
                                tracing::error!(user_id = uid, error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                            }
                            api_error_msg(qq, &e)
                        })
                }
            }),
        );

        let user_stats = match stats_result {
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
                stats
            }
            Err(e) => {
                tracing::error!(error = ?e, user_id = uid, "{}", log_fmt!("main.resolve_bound_user_failed"));
                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                return;
            }
        };

        (uid, name, user_stats, scores)
    } else {
        let qq = msg.user_id;
        let (uid, name, user_stats) =
            match resolve_score_user(ctx, msg, params.username, params.qq, mode, resp_tx).await {
                Some(u) => {
                    tracing::trace!(
                        "{}",
                        log_fmt!(
                            "main.resolve_score_user_resolved",
                            user_id = u.0,
                            username = &u.1
                        )
                    );
                    u
                }
                None => {
                    tracing::warn!("{}", log_fmt!("main.resolve_score_user_none"));
                    return;
                }
            };

        ctx.scheduler.trigger_update(uid, mode).await;
        let dedup_key = (uid, params.is_pass, api_limit, mode);
        let dedup_rate_limiter = ctx.rate_limiter.clone();
        let dedup_oauth = ctx.oauth.clone();
        let dedup_mode = mode;

        tracing::trace!(
            "{}",
            log_fmt!(
                "main.fetch_scores",
                user_id = uid,
                mode = &format!("{:?}", mode),
                limit = api_limit
            )
        );
        let scores: Result<Arc<Vec<Score>>, String> = score_dedup()
            .run_or_wait(dedup_key, move || {
                let dedup_rate_limiter = dedup_rate_limiter.clone();
                let dedup_oauth = dedup_oauth.clone();

                async move {
                    api::get_user_recent(
                        &dedup_rate_limiter,
                        &dedup_oauth,
                        uid,
                        dedup_mode,
                        include_fails,
                        api_limit,
                    )
                    .await
                    .map(Arc::new)
                    .map_err(|e| {
                        warn!(user_id = uid, mode = ?dedup_mode, error = ?e, "{}", log_fmt!("main.score_query_failed"));
                        if !matches!(e, api::ApiError::NotFound | api::ApiError::RateLimitedWithRetryAfter(_) | api::ApiError::ClientRateLimited) {
                            tracing::error!(user_id = uid, error = ?e, "{}", log_fmt!("main.score_query_error_details"));
                        }
                        api_error_msg(qq, &e)
                    })
                }
            })
            .await;

        (uid, name, user_stats, scores)
    };

    let dedup_username = resolved_username.clone();
    let qq = msg.user_id;

    match score_result {
        Ok(mut scores) => {
            if scores.is_empty() {
                let empty_msg = if include_fails {
                    user_str("query.no_records").replace("{qq}", &msg.user_id.to_string())
                } else {
                    user_str("query.no_records_pass").replace("{qq}", &msg.user_id.to_string())
                };
                let _ = resp_tx.send(empty_msg).await;
                return;
            }
            ctx.last_beatmap
                .set(msg.group_id, scores[0].beatmap_id as u32);

            if let Some(bid) = params.beatmap_id {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| s.beatmap_id == bid as i64);
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if let Some(sid) = params.score_id {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| s.score_id == sid as i64);
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if let Some(filters) = params.filters {
                let scores_arc = Arc::make_mut(&mut scores);
                scores_arc.retain(|s| score_matches_filters(s, filters));
                if scores_arc.is_empty() {
                    let _ = resp_tx
                        .send(
                            user_str("query.no_match")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{name}", user_str("query.noun_replay")),
                        )
                        .await;
                    return;
                }
            }

            if params.is_single {
                let index = (params.limit - 1) as usize;
                if index >= scores.len() {
                    let _ = resp_tx
                        .send(
                            user_str("query.index_out_of_range")
                                .replace("{qq}", &msg.user_id.to_string())
                                .replace("{pos}", &params.limit.to_string())
                                .replace("{name}", user_str("query.noun_replay"))
                                .replace("{total}", &scores.len().to_string()),
                        )
                        .await;
                    return;
                }
                let score = &scores[index];
                render_and_send_single_score(SingleScoreRenderParams {
                    ctx,
                    msg,
                    resp_tx,
                    score,
                    mode,
                    user_stats: &user_stats,
                    position: Some(index),
                    is_pass: params.is_pass,
                })
                .await;
            } else {
                if let Some(end) = params.limit_end {
                    let start = (params.limit - 1) as usize;
                    let end = end as usize;
                    if start >= scores.len() {
                        let _ = resp_tx
                            .send(
                                user_str("query.index_out_of_range")
                                    .replace("{qq}", &msg.user_id.to_string())
                                    .replace("{pos}", &params.limit.to_string())
                                    .replace("{name}", user_str("query.noun_replay"))
                                    .replace("{total}", &scores.len().to_string()),
                            )
                            .await;
                        return;
                    }
                    let end = end.min(scores.len());
                    let scores_arc = Arc::make_mut(&mut scores);
                    let _ = scores_arc.drain(..start);
                    scores_arc.truncate(end - start);
                }

                let results =
                    futures_util::future::join_all(scores.iter().enumerate().map(|(i, s)| {
                        let cover_url = s.cover_url.clone();
                        let needs_enrich = s.pp.is_none() && s.beatmap_id > 0;
                        let score_clone = if needs_enrich { Some(s.clone()) } else { None };
                        async move {
                            let enriched = if let Some(mut sc) = score_clone {
                                osubot_core::enrich_score_with_pp(&mut sc, mode, false).await;
                                Some(sc)
                            } else {
                                None
                            };
                            let cover = if !cover_url.is_empty() {
                                match osubot_render::cache::fetch_and_cache(
                                    &cover_url,
                                    osubot_render::cache::http_client(),
                                )
                                .await
                                {
                                    Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
                                    Err(_) => None,
                                }
                            } else {
                                None
                            };
                            (i, enriched, cover)
                        }
                    }))
                    .await;

                let scores_mut = Arc::make_mut(&mut scores);
                let mut cover_images: Vec<Option<image::DynamicImage>> =
                    vec![None; scores_mut.len()];
                for (i, enriched, cover) in results {
                    if let Some(new_s) = enriched {
                        scores_mut[i] = new_s;
                    }
                    cover_images[i] = cover;
                }

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
                    .calculate_change(user_id, mode, &user_stats)
                    .await
                    .inspect_err(|e| {
                        tracing::warn!(
                            user_id = user_id,
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
                let score_label = if params.is_pass {
                    user_str("fmt.recent_pass")
                } else {
                    user_str("fmt.recent_play")
                };
                let score_count_text = user_str("fmt.score_count");
                let render_result = tokio::time::timeout(
                    std::time::Duration::from_secs(SCORE_LIST_RENDER_TIMEOUT_SECS),
                    osubot_render::render_score_list_card(osubot_render::ScoreListCardParams {
                        user: osubot_render::UserContext {
                            username: &dedup_username,
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
                    }),
                )
                .await;

                match render_result {
                    Ok(Ok(jpeg_bytes)) => {
                        tracing::info!(
                            "{}",
                            log_fmt!("main.score_list_card_rendered", bytes = jpeg_bytes.len())
                        );
                        let jpeg = Arc::new(jpeg_bytes);
                        let write = ctx.write.clone();
                        let group_id = msg.group_id;
                        let resp_tx_img = resp_tx.clone();
                        tokio::spawn(async move {
                            if send_group_msg_with_image(&write, group_id, &jpeg)
                                .await
                                .is_err()
                            {
                                let _ = resp_tx_img
                                    .send(
                                        user_str("error.image_send_failed")
                                            .replace("{qq}", &qq.to_string()),
                                    )
                                    .await;
                            }
                        });
                    }
                    Ok(Err(e)) => {
                        warn!(error = %e, "{}", log_fmt!("main.render_score_list_failed_text"));
                        let response =
                            format_scores(&scores, &dedup_username, mode, params.is_pass);
                        let _ = resp_tx.send(response).await;
                    }
                    Err(_) => {
                        warn!("{}", log_fmt!("main.render_score_list_timeout_text"));
                        let response =
                            format_scores(&scores, &dedup_username, mode, params.is_pass);
                        let _ = resp_tx.send(response).await;
                    }
                }
            }
        }
        Err(err_msg) => {
            let _ = resp_tx.send(err_msg).await;
        }
    }
}

pub(crate) async fn handle_beatmap_score_query(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    cmd: &Command,
    mode: GameMode,
) {
    let (username, qq, beatmap_id, score_id, filters, limit, limit_end, is_all) = match cmd {
        Command::ScoreOnBeatmap {
            username,
            qq,
            beatmap_id,
            score_id,
            filters,
            limit,
            limit_end,
            is_all,
            ..
        } => (
            username.as_deref(),
            *qq,
            *beatmap_id,
            *score_id,
            filters.as_deref(),
            *limit,
            *limit_end,
            *is_all,
        ),
        _ => return,
    };

    if let Some(sid) = score_id {
        info!(score_id = sid, "{}", log_fmt!("main.score_by_id"));
        let qq = msg.user_id;
        let dedup_rate_limiter = ctx.rate_limiter.clone();
        let dedup_oauth = ctx.oauth.clone();
        let sid_key = sid as i64;
        let score_result = score_by_id_dedup()
            .run_or_wait((sid_key, mode), move || {
                let rate_limiter = dedup_rate_limiter.clone();
                let oauth = dedup_oauth.clone();

                async move {
                    api::get_score_by_id(&rate_limiter, &oauth, sid)
                        .await
                        .map_err(|e| {
                            if !matches!(e, api::ApiError::NotFound) {
                                warn!(error = ?e, "{}", log_fmt!("main.get_score_by_id_failed"));
                            }
                            match e {
                                api::ApiError::NotFound => user_str("query.score_not_found")
                                    .replace("{qq}", &qq.to_string()),
                                other => api_error_msg(qq, &other),
                            }
                        })
                }
            })
            .await;
        let score = match score_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };
        ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);

        let user_id = score.user.user_id.unwrap_or(0);
        if user_id == 0 {
            let _ = resp_tx
                .send(user_str("query.user_info_failed").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }
        let user_stats = match api::fetch_user_stats_by_user_id(
            &ctx.rate_limiter,
            &ctx.oauth,
            user_id,
            mode,
        )
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
                ctx.scheduler.trigger_update(user_id, mode).await;
                stats
            }
            Err(e) => {
                if !matches!(e, api::ApiError::NotFound) {
                    warn!(user_id = user_id, error = ?e, "{}", log_fmt!("main.fetch_stats_score_id_failed"));
                }
                let _ = resp_tx.send(api_error_msg(msg.user_id, &e)).await;
                return;
            }
        };
        render_and_send_single_score(SingleScoreRenderParams {
            ctx,
            msg,
            resp_tx,
            score: &score,
            mode,
            user_stats: &user_stats,
            position: None,
            is_pass: true,
        })
        .await;
        return;
    }

    let resolved_bid = match beatmap_id {
        Some(bid) => bid,
        None => match ctx.last_beatmap.get(msg.group_id) {
            Some(bid) => bid,
            None => {
                let _ = resp_tx
                    .send(
                        user_str("query.need_beatmap_or_cache")
                            .replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
        },
    };

    info!(
        beatmap_id = resolved_bid,
        mode = ?mode,
        filters = ?filters,
        limit,
        is_all,
        "{}",
        log_fmt!("main.score_on_beatmap")
    );
    ctx.last_beatmap.set(msg.group_id, resolved_bid);

    let (_user_id, username_str, user_stats) = match resolve_score_user(
        ctx,
        msg,
        &username.map(|s| s.to_string()),
        &qq,
        mode,
        resp_tx,
    )
    .await
    {
        Some(result) => result,
        None => return,
    };

    ctx.scheduler.trigger_update(_user_id, mode).await;
    let qq = msg.user_id;

    if is_all {
        let raw_api_limit = limit_end.or(if limit > 1 { Some(limit) } else { None });
        let api_limit = match (raw_api_limit, filters.is_some_and(|f| !f.is_empty())) {
            (Some(n), true) => Some(n.max(SCORE_API_FETCH_LIMIT)),
            (other, _) => other,
        };
        let key = (_user_id, resolved_bid as i64, mode, api_limit);
        let dedup_rall = ctx.rate_limiter.clone();
        let dedup_oall = ctx.oauth.clone();
        let scores_result = beatmap_scores_dedup()
            .run_or_wait(key, move || {
                let rate_limiter = dedup_rall.clone();
                let oauth = dedup_oall.clone();

                async move {
                    api::get_user_beatmap_scores_all(
                        &rate_limiter,
                        &oauth,
                        resolved_bid as i64,
                        _user_id,
                        mode,
                        api_limit,
                    )
                    .await
                    .map_err(|e| {
                        if !matches!(e, api::ApiError::NotFound) {
                            warn!(error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                        }
                        match e {
                            api::ApiError::NotFound => {
                                user_str("query.no_score_on_map").replace("{qq}", &qq.to_string())
                            }
                            other => api_error_msg(qq, &other),
                        }
                    })
                }
            })
            .await;

        let scores = match scores_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };
        if scores.is_empty() {
            let _ = resp_tx
                .send(user_str("query.no_score_on_map").replace("{qq}", &msg.user_id.to_string()))
                .await;
            return;
        }

        let total_scores = scores.len();
        let scores = match process_scores(scores, filters, limit, limit_end) {
            Ok(s) => s,
            Err(key) => {
                let msg_text = user_str(key)
                    .replace("{qq}", &msg.user_id.to_string())
                    .replace("{pos}", &limit.to_string())
                    .replace("{name}", user_str("query.noun_score"))
                    .replace("{total}", &total_scores.to_string());
                let _ = resp_tx.send(msg_text).await;
                return;
            }
        };

        render_scores(ctx, msg, resp_tx, &scores, &user_stats, &username_str, mode).await;
    } else if limit == 1 && limit_end.is_none() {
        let active_filters = filters.filter(|f| !f.is_empty());
        if let Some(filters) = active_filters {
            let api_limit = SCORE_API_FETCH_LIMIT;
            let key = (_user_id, resolved_bid as i64, mode, Some(api_limit));
            let dedup_rscores = ctx.rate_limiter.clone();
            let dedup_oscores = ctx.oauth.clone();
            let scores_result =
                beatmap_scores_dedup()
                    .run_or_wait(key, move || {
                        let rate_limiter = dedup_rscores.clone();
                        let oauth = dedup_oscores.clone();

                        async move {
                            api::get_user_beatmap_scores_all(
                                &rate_limiter,
                                &oauth,
                                resolved_bid as i64,
                                _user_id,
                                mode,
                                Some(api_limit),
                            )
                            .await
                            .map_err(|e| {
                                if !matches!(e, api::ApiError::NotFound) {
                                    warn!(error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                                }
                                match e {
                                    api::ApiError::NotFound => user_str("query.no_score_on_map")
                                        .replace("{qq}", &qq.to_string()),
                                    other => api_error_msg(qq, &other),
                                }
                            })
                        }
                    })
                    .await;
            let scores = match scores_result {
                Ok(s) => s,
                Err(err_msg) => {
                    let _ = resp_tx.send(err_msg).await;
                    return;
                }
            };
            if scores.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_score_on_map").replace("{qq}", &msg.user_id.to_string()),
                    )
                    .await;
                return;
            }
            let total_scores = scores.len();
            let scores = match process_scores(scores, Some(filters), limit, None) {
                Ok(s) => s,
                Err(key) => {
                    let msg_text = user_str(key)
                        .replace("{qq}", &msg.user_id.to_string())
                        .replace("{pos}", &limit.to_string())
                        .replace("{name}", user_str("query.noun_score"))
                        .replace("{total}", &total_scores.to_string());
                    let _ = resp_tx.send(msg_text).await;
                    return;
                }
            };
            let score = &scores[0];
            ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
            render_and_send_single_score(SingleScoreRenderParams {
                ctx,
                msg,
                resp_tx,
                score,
                mode,
                user_stats: &user_stats,
                position: None,
                is_pass: true,
            })
            .await;
        } else {
            let key = (_user_id, resolved_bid as i64, mode);
            let dedup_rscore = ctx.rate_limiter.clone();
            let dedup_oscore = ctx.oauth.clone();
            let score_result = beatmap_score_dedup()
                .run_or_wait(key, move || {
                    let rate_limiter = dedup_rscore.clone();
                    let oauth = dedup_oscore.clone();

                    async move {
                        api::get_user_beatmap_score(
                            &rate_limiter,
                            &oauth,
                            resolved_bid as i64,
                            _user_id,
                            mode,
                            &None,
                        )
                        .await
                        .map_err(|e| {
                            if !matches!(e, api::ApiError::NotFound) {
                                warn!(
                                    error = ?e,
                                    beatmap_id = resolved_bid,
                                    "{}",
                                    log_fmt!("main.get_user_beatmap_score_failed")
                                );
                            }
                            match e {
                                api::ApiError::NotFound => user_str("query.no_score_on_map")
                                    .replace("{qq}", &qq.to_string()),
                                other => api_error_msg(qq, &other),
                            }
                        })
                    }
                })
                .await;
            let score = match score_result {
                Ok(s) => s,
                Err(err_msg) => {
                    let _ = resp_tx.send(err_msg).await;
                    return;
                }
            };
            ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
            render_and_send_single_score(SingleScoreRenderParams {
                ctx,
                msg,
                resp_tx,
                score: &score,
                mode,
                user_stats: &user_stats,
                position: None,
                is_pass: true,
            })
            .await;
        }
    } else {
        let raw_limit = limit_end.unwrap_or(limit);
        let api_limit = if filters.is_some_and(|f| !f.is_empty()) {
            raw_limit.max(SCORE_API_FETCH_LIMIT)
        } else {
            raw_limit
        };
        let n = limit as usize;
        let key = (_user_id, resolved_bid as i64, mode, Some(api_limit));
        let dedup_rscores = ctx.rate_limiter.clone();
        let dedup_oscores = ctx.oauth.clone();
        let scores_result = beatmap_scores_dedup()
            .run_or_wait(key, move || {
                let rate_limiter = dedup_rscores.clone();
                let oauth = dedup_oscores.clone();

                async move {
                    api::get_user_beatmap_scores_all(
                        &rate_limiter,
                        &oauth,
                        resolved_bid as i64,
                        _user_id,
                        mode,
                        Some(api_limit),
                    )
                    .await
                    .map_err(|e| {
                        if !matches!(e, api::ApiError::NotFound) {
                            warn!(error = ?e, "{}", log_fmt!("main.get_user_beatmap_scores_failed"));
                        }
                        match e {
                            api::ApiError::NotFound => {
                                user_str("query.no_score_on_map").replace("{qq}", &qq.to_string())
                            }
                            other => api_error_msg(qq, &other),
                        }
                    })
                }
            })
            .await;
        let scores = match scores_result {
            Ok(s) => s,
            Err(err_msg) => {
                let _ = resp_tx.send(err_msg).await;
                return;
            }
        };

        let total_scores = scores.len();
        let scores = match process_scores(scores, filters, limit, limit_end) {
            Ok(s) => s,
            Err(key) => {
                let msg_text = user_str(key)
                    .replace("{qq}", &msg.user_id.to_string())
                    .replace("{pos}", &limit.to_string())
                    .replace("{name}", user_str("query.noun_score"))
                    .replace("{total}", &total_scores.to_string());
                let _ = resp_tx.send(msg_text).await;
                return;
            }
        };

        if limit_end.is_some() {
            render_scores(ctx, msg, resp_tx, &scores, &user_stats, &username_str, mode).await;
        } else {
            if scores.len() < n {
                let _ = resp_tx
                    .send(
                        user_str("query.index_out_of_range")
                            .replace("{qq}", &msg.user_id.to_string())
                            .replace("{pos}", &n.to_string())
                            .replace("{name}", user_str("query.noun_score"))
                            .replace("{total}", &scores.len().to_string()),
                    )
                    .await;
                return;
            }
            let score = scores.into_iter().nth(n - 1).expect("len checked above");
            ctx.last_beatmap.set(msg.group_id, score.beatmap_id as u32);
            render_and_send_single_score(SingleScoreRenderParams {
                ctx,
                msg,
                resp_tx,
                score: &score,
                mode,
                user_stats: &user_stats,
                position: Some(n - 1),
                is_pass: true,
            })
            .await;
        }
    }
}

fn filter_scores(scores: Vec<Score>, filters: Option<&[String]>) -> Vec<Score> {
    if let Some(filters) = filters {
        scores
            .into_iter()
            .filter(|s| score_matches_filters(s, filters))
            .collect()
    } else {
        scores
    }
}

fn process_scores(
    scores: Vec<Score>,
    filters: Option<&[String]>,
    limit: u32,
    limit_end: Option<u32>,
) -> Result<Vec<Score>, &'static str> {
    let mut scores = filter_scores(scores, filters);
    if scores.is_empty() {
        return Err("query.no_match");
    }

    if let Some(end) = limit_end {
        let start = (limit - 1) as usize;
        let end = end as usize;
        if start >= scores.len() {
            return Err("query.index_out_of_range");
        }
        let end = end.min(scores.len());
        let _ = scores.drain(..start);
        scores.truncate(end - start);
        if scores.is_empty() {
            return Err("query.index_out_of_range");
        }
    }

    Ok(scores)
}

async fn render_scores(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    scores: &[Score],
    user_stats: &UserStats,
    username: &str,
    mode: GameMode,
) {
    if scores.len() == 1 {
        render_and_send_single_score(SingleScoreRenderParams {
            ctx,
            msg,
            resp_tx,
            score: &scores[0],
            mode,
            user_stats,
            position: None,
            is_pass: true,
        })
        .await;
    } else {
        render_and_send_score_list(ctx, msg, resp_tx, scores, user_stats, username, mode).await;
    }
}

struct SingleScoreRenderParams<'a> {
    ctx: &'a BotContext,
    msg: &'a QQMessage,
    resp_tx: &'a mpsc::Sender<String>,
    score: &'a Score,
    mode: GameMode,
    user_stats: &'a UserStats,
    position: Option<usize>,
    is_pass: bool,
}

async fn render_and_send_single_score(params: SingleScoreRenderParams<'_>) {
    let SingleScoreRenderParams {
        ctx,
        msg,
        resp_tx,
        score,
        mode,
        user_stats,
        position,
        is_pass,
    } = params;
    let mut score = score.clone();
    enrich_score_with_pp(&mut score, mode, true).await;

    let ur_value = if mode == GameMode::Osu && score.score_id > 0 && score.has_replay {
        tracing::trace!(score_id = score.score_id, mode = ?mode, is_lazer = score.is_lazer, length = score.length_seconds, "{}", log_fmt!("main.ur_calculation_start"));
        let rl = ctx.rate_limiter.clone();
        let oa = ctx.oauth.clone();
        let ur_params = osubot_core::ur::ScoreUrParams {
            score_id: score.score_id,
            legacy_score_id: score.legacy_score_id,
            beatmap_id: score.beatmap_id,
            mode,
            mods: score.mods.clone(),
        };
        let ur_timeout = Duration::from_secs(ctx.config.read().await.bot.ur_timeout_secs);
        match tokio::time::timeout(
            ur_timeout,
            osubot_core::ur::calculate_score_ur(&rl, &oa, ur_params),
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
        match render_cache::fetch_and_cache(&score.cover_url, render_cache::http_client()).await {
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
            let group_id = msg.group_id;
            let resp_tx_img = resp_tx.clone();

            tokio::spawn(async move {
                if send_group_msg_with_image(&write, group_id, &jpeg_bytes)
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
            let text = format_score(&score, &user_stats.username, mode, position, is_pass);
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            cancel_flag.store(true, Ordering::Relaxed);
            warn!("{}", log_fmt!("main.render_score_card_timeout_text"));
            let text = format_score(&score, &user_stats.username, mode, position, is_pass);
            let _ = resp_tx.send(text).await;
        }
    }
}

async fn render_and_send_score_list(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    scores: &[Score],
    user_stats: &UserStats,
    username: &str,
    mode: GameMode,
) {
    let results = join_all(scores.iter().enumerate().map(|(i, s)| {
        let cover_url = s.cover_url.clone();
        let needs_enrich = s.pp.is_none() && s.beatmap_id > 0;
        let score_clone = if needs_enrich { Some(s.clone()) } else { None };
        async move {
            let enriched = if let Some(mut sc) = score_clone {
                enrich_score_with_pp(&mut sc, mode, false).await;
                Some(sc)
            } else {
                None
            };
            let cover = if !cover_url.is_empty() {
                match render_cache::fetch_and_cache(&cover_url, render_cache::http_client()).await {
                    Ok((bytes, _, _)) => image::load_from_memory(&bytes).ok(),
                    Err(_) => None,
                }
            } else {
                None
            };
            (i, enriched, cover)
        }
    }))
    .await;

    let scores_vec: Vec<Score> = scores.to_vec();
    let mut scores_mut = scores_vec;
    let mut cover_images: Vec<Option<image::DynamicImage>> = vec![None; scores_mut.len()];
    for (i, enriched, cover) in results {
        if let Some(new_s) = enriched {
            scores_mut[i] = new_s;
        }
        cover_images[i] = cover;
    }

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

    let score_label = user_str("fmt.beatmap_score");
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
            scores: &scores_mut,
            label: score_label,
            count_text: score_count_text,
            cover_images,
            hero_cover_url: &hero_cover_url,
        }),
    )
    .await;

    let qq = msg.user_id;

    match render_result {
        Ok(Ok(jpeg_bytes)) => {
            let write = ctx.write.clone();
            let group_id = msg.group_id;
            let resp_tx_img = resp_tx.clone();

            tokio::spawn(async move {
                if send_group_msg_with_image(&write, group_id, &jpeg_bytes)
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
            let text = format_scores(&scores_mut, username, mode, true);
            let _ = resp_tx.send(text).await;
        }
        Err(_) => {
            warn!("{}", log_fmt!("main.render_score_list_timeout_text"));
            let text = format_scores(&scores_mut, username, mode, true);
            let _ = resp_tx.send(text).await;
        }
    }
}
