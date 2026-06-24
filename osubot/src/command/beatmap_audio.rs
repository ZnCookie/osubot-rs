use super::*;
use crate::onebot::send_group_msg_with_record;
use crate::score_filter::score_matches_filters;
use crate::SCORE_API_FETCH_LIMIT;

pub(super) struct BeatmapAudioParams {
    pub(super) score_id: Option<u64>,
    pub(super) beatmap_id: Option<u32>,
    pub(super) username: Option<String>,
    pub(super) qq: Option<i64>,
    pub(super) mode: GameMode,
    pub(super) mode_specified: bool,
    pub(super) filters: Option<Vec<String>>,
    pub(super) limit: u32,
    pub(super) explicit_position: bool,
}

pub(super) async fn handle_beatmap_audio(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    params: BeatmapAudioParams,
) {
    let qq = msg.user_id;
    let group_id = msg.group_id;

    let beatmapset_id: i64 = match (&params.score_id, &params.beatmap_id) {
        (Some(sid), None) => {
            let dedup_rate_limiter = ctx.rate_limiter.clone();
            let dedup_oauth = ctx.oauth.clone();
            let sid_owned = *sid;
            let result = score_by_id_dedup()
                .run_or_wait(sid_owned as i64, move || {
                    let rl = dedup_rate_limiter.clone();
                    let oauth = dedup_oauth.clone();
                    async move {
                        let result = api::get_score_by_id(&rl, &oauth, sid_owned).await;
                        if let Err(ref e) = result {
                            if !matches!(e, ApiError::NotFound) {
                                warn!(error = ?e, "{}", log_fmt!("main.get_score_by_id_failed"));
                            }
                        }
                        result.map_err(|e| DedupApiError::from_api_error(&e))
                    }
                })
                .await;
            match result {
                Ok(score) => {
                    ctx.last_beatmap.set(group_id, score.beatmap_id as u32);
                    score.beatmapset_id
                }
                Err(e) => {
                    let _ = resp_tx.send(score_by_id_err_msg(qq, &e)).await;
                    return;
                }
            }
        }
        (None, Some(bid)) => match fetch_beatmapset_id_dedup(ctx, *bid as i64).await {
            Ok(set_id) => {
                ctx.last_beatmap.set(group_id, *bid);
                set_id
            }
            Err(e) => {
                let _ = resp_tx.send(e.to_user_msg(qq)).await;
                return;
            }
        },
        (None, None) => {
            match resolve_beatmapset_id_fallback(ctx, &params, qq, group_id, resp_tx).await {
                Some(id) => id,
                None => return,
            }
        }
        (Some(_), Some(_)) => {
            send_error(resp_tx, qq, "error.data_fetch_failed").await;
            return;
        }
    };

    if beatmapset_id <= 0 {
        send_error(resp_tx, qq, "error.data_fetch_failed").await;
        return;
    }

    let mp3 = match api::download_beatmap_preview_mp3(beatmapset_id).await {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = resp_tx.send(api_error_msg(qq, &e)).await;
            return;
        }
    };

    let write = ctx.write.clone();
    if let Err(e) = send_group_msg_with_record(&write, group_id, &mp3).await {
        warn!(error = %e, "{}", log_fmt!("main.beatmap_audio_send_failed", error = &e.to_string()));
        let _ = resp_tx
            .send(user_str("error.audio_send_failed").replace("{qq}", &qq.to_string()))
            .await;
    }
}

async fn resolve_beatmapset_id_fallback(
    ctx: &BotContext,
    params: &BeatmapAudioParams,
    qq: i64,
    group_id: i64,
    resp_tx: &mpsc::Sender<String>,
) -> Option<i64> {
    let has_target = params.username.is_some()
        || params.qq.is_some()
        || params.filters.as_ref().is_some_and(|f| !f.is_empty())
        || params.explicit_position
        || params.mode_specified;
    if !has_target {
        if let Some(bid) = ctx.last_beatmap.get(group_id) {
            return match fetch_beatmapset_id_dedup(ctx, bid as i64).await {
                Ok(set_id) => Some(set_id),
                Err(e) => {
                    let _ = resp_tx.send(e.to_user_msg(qq)).await;
                    None
                }
            };
        }
    }

    let user_id = if let Some(ref name) = params.username {
        match api::fetch_user_stats_by_username(&ctx.rate_limiter, &ctx.oauth, name, params.mode)
            .await
        {
            Ok(stats) => stats.user_id,
            Err(ApiError::NotFound) => {
                let _ = resp_tx
                    .send(
                        user_str("error.not_found_named")
                            .replace("{qq}", &qq.to_string())
                            .replace("{name}", name),
                    )
                    .await;
                return None;
            }
            Err(e) => {
                let _ = resp_tx.send(api_error_msg(qq, &e)).await;
                return None;
            }
        }
    } else {
        let target_qq = params.qq.unwrap_or(qq);
        match ctx.resolve_binding(target_qq).await {
            Some((uid, _name)) => uid,
            None => {
                let key = if params.qq.is_some() {
                    "bind.user_not_bound"
                } else {
                    "bind.not_bound"
                };
                let _ = resp_tx
                    .send(user_str(key).replace("{qq}", &qq.to_string()))
                    .await;
                return None;
            }
        }
    };

    let has_filters = params.filters.as_ref().is_some_and(|f| !f.is_empty());
    let api_limit = if has_filters {
        SCORE_API_FETCH_LIMIT
    } else {
        params.limit
    };

    match api::get_user_recent(
        &ctx.rate_limiter,
        &ctx.oauth,
        user_id,
        params.mode,
        true,
        api_limit,
    )
    .await
    {
        Ok(scores) => {
            let mut matching: Vec<_> = if let Some(ref filters) = params.filters {
                scores
                    .into_iter()
                    .filter(|s| score_matches_filters(s, filters))
                    .collect()
            } else {
                scores
            };
            let index = (params.limit.saturating_sub(1)) as usize;
            if matching.is_empty() {
                let _ = resp_tx
                    .send(
                        user_str("query.no_match")
                            .replace("{qq}", &qq.to_string())
                            .replace("{name}", user_str("query.noun_replay")),
                    )
                    .await;
                return None;
            }
            if index >= matching.len() {
                let _ = resp_tx
                    .send(
                        user_str("query.index_out_of_range")
                            .replace("{qq}", &qq.to_string())
                            .replace("{pos}", &params.limit.to_string())
                            .replace("{name}", user_str("query.noun_replay"))
                            .replace("{total}", &matching.len().to_string()),
                    )
                    .await;
                return None;
            }
            let score = matching.swap_remove(index);
            ctx.last_beatmap.set(group_id, score.beatmap_id as u32);
            Some(score.beatmapset_id)
        }
        Err(e) => {
            let _ = resp_tx.send(api_error_msg(qq, &e)).await;
            None
        }
    }
}

/// 通过 beatmap_id 解析 beatmapset_id，按 beatmap_id 去重并发请求。
/// 返回结构化错误（不含 qq），由调用方各自格式化。
async fn fetch_beatmapset_id_dedup(
    ctx: &BotContext,
    beatmap_id: i64,
) -> Result<i64, DedupApiError> {
    let rl = ctx.rate_limiter.clone();
    let oauth = ctx.oauth.clone();
    beatmapset_dedup()
        .run_or_wait(beatmap_id, move || {
            let rl = rl.clone();
            let oauth = oauth.clone();
            async move {
                api::get_beatmapset_id(&rl, &oauth, beatmap_id)
                    .await
                    .map_err(|e| DedupApiError::from_api_error(&e))
            }
        })
        .await
}
