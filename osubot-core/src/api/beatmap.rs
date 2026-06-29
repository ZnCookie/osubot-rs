use crate::types::GameMode;

pub async fn calc_star_rating_local(
    beatmap_id: i64,
    status: &str,
    mods: &rosu_mods::GameMods,
    mode: GameMode,
    is_lazer: bool,
) -> Option<f64> {
    let osu_path = super::download_beatmap_osu(beatmap_id, status).await.ok()?;
    let mods = mods.clone();

    tokio::task::spawn_blocking(move || {
        let map = rosu_pp::Beatmap::from_path(&osu_path).ok()?;
        let pp_mods = rosu_pp::GameMods::from(mods);
        let target_mode: rosu_pp::model::mode::GameMode = mode.into();

        if map.mode != target_mode {
            let perf = rosu_pp::Performance::new(&map)
                .mods(pp_mods)
                .lazer(is_lazer)
                .try_mode(target_mode)
                .ok()?;
            Some(perf.calculate().stars())
        } else {
            let attrs = rosu_pp::Difficulty::new()
                .mods(pp_mods)
                .lazer(is_lazer)
                .calculate(&map);
            Some(attrs.stars())
        }
    })
    .await
    .ok()
    .flatten()
}

/// Get mod-adjusted star rating with a local-first, API-fallback strategy.
///
/// 1. Try local rosu_pp calculation (most accurate, zero API cost if cached)
/// 2. Fall back to the lightweight `/beatmaps/{id}/attributes` API
/// 3. Return 0.0 if both fail
///
/// **Note:** `is_lazer` is respected in the local path (step 1) but
/// currently NOT passed to the API fallback (step 2) — the osu! API
/// `/beatmaps/{id}/attributes` endpoint does not yet support a lazer
/// parameter. This means that on the API fallback path, lazer-specific
/// score multiplier differences are not reflected.
pub async fn get_star_rating(
    rate_limiter: &crate::rate_limiter::RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    beatmap_id: i64,
    status: &str,
    mods: &rosu_mods::GameMods,
    mode: GameMode,
    is_lazer: bool,
) -> f64 {
    if let Some(sr) = calc_star_rating_local(beatmap_id, status, mods, mode, is_lazer).await {
        return sr;
    }

    if let Ok(sr) = super::osu_api::fetch_beatmap_difficulty_attributes(
        rate_limiter,
        oauth,
        beatmap_id,
        mods,
        mode,
    )
    .await
    {
        return sr;
    }

    0.0
}
