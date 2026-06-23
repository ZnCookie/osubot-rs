use super::*;

pub(super) struct BeatmapPreviewParams {
    pub(super) score_id: Option<u64>,
    pub(super) beatmap_id: Option<u32>,
    pub(super) mode: Option<GameMode>,
    pub(super) mods: Option<Vec<String>>,
    pub(super) gif: bool,
    pub(super) times: Option<Vec<i64>>,
}

pub(super) async fn handle_beatmap_preview(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    params: BeatmapPreviewParams,
) {
    let qq = msg.user_id;
    let group_id = msg.group_id;

    let resolved_bid_i64: i64 = match (&params.score_id, &params.beatmap_id) {
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

    let mod_settings = match &params.mods {
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

    let target_mode = params.mode.map(i32::from).unwrap_or_else(|| beatmap.mode());
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

    let use_gif = params.gif || target_mode == 0;
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
        .unwrap_or_else(|_| {
            warn!("{}", log_fmt!("main.system_time_before_epoch"));
            std::time::Duration::from_nanos(0)
        })
        .as_nanos();
    let filename = if mod_suffix.is_empty() {
        format!("{}_{:x}.{}", resolved_bid, nanos, fmt)
    } else {
        format!("{}_{}_{:x}.{}", resolved_bid, mod_suffix, nanos, fmt)
    };
    let output_path = osubot_core::cache::preview_cache_dir().join(&filename);

    let times_ms: Option<Vec<i64>> = match &params.times {
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
