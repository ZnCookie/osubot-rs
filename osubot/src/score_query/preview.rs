use super::*;
use crate::send_error;
use osubot_beatmap_preview::{
    self, convert_beatmap, parse_beatmap_from_bytes, parse_mods, validate_mods,
};

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_beatmap_preview_from_score(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    mods: &Option<Vec<String>>,
    gif: bool,
    times: &Option<Vec<i64>>,
    score: &Score,
    mode: GameMode,
) {
    let resolved_bid = score.beatmap_id as u32;
    ctx.last_beatmap
        .set(msg.group_id.unwrap_or(-msg.user_id), resolved_bid);
    render_beatmap_preview_by_id(ctx, msg, resp_tx, mods, gif, times, resolved_bid, mode).await;
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn render_beatmap_preview_by_id(
    ctx: &BotContext,
    msg: &QQMessage,
    resp_tx: &mpsc::Sender<String>,
    mods: &Option<Vec<String>>,
    gif: bool,
    times: &Option<Vec<i64>>,
    beatmap_id: u32,
    mode: GameMode,
) {
    let qq = msg.user_id;
    let resolved_bid = beatmap_id;

    let mods = mods.clone();

    let beatmap_path = match api::download_beatmap_osu(resolved_bid as i64, "ranked").await {
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
                .map_err(|e| osubot_beatmap_preview::PreviewError::new(format!("read beatmap metadata: {e}")))?;
            if meta.len() > 50 * 1024 * 1024 {
                return Err(osubot_beatmap_preview::PreviewError::new("beatmap file too large (>50MB)"));
            }
            let bytes = std::fs::read(&path)
                .map_err(|e| osubot_beatmap_preview::PreviewError::new(format!("read beatmap file: {e}")))?;
            parse_beatmap_from_bytes(&bytes)
        }
    }).await;

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

    let mod_settings = match &mods {
        Some(m) if !m.is_empty() => {
            let joined = m.join("+");
            match parse_mods(&joined) {
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

    let target_mode = mode as i32;
    if let Some(ref s) = mod_settings {
        let validation_errors = validate_mods(s, Some(target_mode));
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
            convert_beatmap(&beatmap, target_mode, mods_for_conv.as_ref())
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

    let use_gif = gif || target_mode == 0;
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

    let times_ms: Option<Vec<i64>> = match &times {
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

    let output_path_clone = output_path.clone();
    let render_join = tokio::task::spawn_blocking(move || {
        render_beatmap_preview(
            &beatmap,
            target_mode,
            mod_settings.as_ref(),
            &output_path_clone,
            use_gif,
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

    let image_bytes = match tokio::fs::read(&output_path).await {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_read_output_failed", error = &e.to_string()));
            send_error(resp_tx, qq, "error.render_failed").await;
            return;
        }
    };

    let write = ctx.write.clone();
    let send_result = match msg.group_id {
        Some(gid) => send_group_msg_with_image(&write, &ctx.onebot_api, gid, &image_bytes).await,
        None => {
            send_private_msg_with_image(&write, &ctx.onebot_api, msg.user_id, &image_bytes).await
        }
    };
    if let Err(e) = send_result {
        warn!(error = %e, "{}", log_fmt!("main.beatmap_preview_send_failed", error = &e.to_string()));
    }
}

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
        1 => {
            osubot_beatmap_preview::render_taiko_grid(beatmap, output_path, mods, None).map(|_| ())
        }
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

fn generate_linear_samples(start: i64, end: i64, n: usize) -> Vec<i64> {
    if n <= 1 || start >= end {
        return vec![start];
    }
    let step = (end - start) / (n - 1) as i64;
    (0..n).map(|i| start + step * i as i64).collect()
}
