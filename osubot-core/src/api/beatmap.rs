//! Beatmap analysis pipeline: download → parse → compute star rating.
//!
//! Consolidates the shared "download .osu + parse with rosu_pp" logic that was
//! previously duplicated between `pp.rs` and `osu_api.rs`.

use crate::types::GameMode;

/// Download the .osu file and compute mod-adjusted star rating locally via rosu_pp.
/// Returns `None` if the download or parse fails.
pub async fn calc_star_rating_local(
    beatmap_id: i64,
    status: &str,
    mods: &rosu_mods::GameMods,
    mode: GameMode,
) -> Option<f64> {
    let osu_path = super::download_beatmap_osu(beatmap_id, status).await.ok()?;
    let map = rosu_pp::Beatmap::from_path(&osu_path).ok()?;

    let pp_mods = rosu_pp::GameMods::from(mods.clone());
    let mode_convert = match mode {
        GameMode::Osu => rosu_pp::model::mode::GameMode::Osu,
        GameMode::Taiko => rosu_pp::model::mode::GameMode::Taiko,
        GameMode::Catch => rosu_pp::model::mode::GameMode::Catch,
        GameMode::Mania => rosu_pp::model::mode::GameMode::Mania,
    };

    let perf = rosu_pp::Performance::new(&map)
        .mods(pp_mods)
        .try_mode(mode_convert)
        .ok()?;
    Some(perf.calculate().stars())
}

/// Get mod-adjusted star rating with a local-first, API-fallback strategy.
///
/// 1. Try local rosu_pp calculation (most accurate, zero API cost if cached)
/// 2. Fall back to the lightweight `/beatmaps/{id}/attributes` API
/// 3. Return 0.0 if both fail
pub async fn get_star_rating(
    rate_limiter: &crate::rate_limiter::RateLimiter,
    oauth: &super::oauth::OauthTokenCache,
    beatmap_id: i64,
    status: &str,
    mods: &rosu_mods::GameMods,
    mode: GameMode,
) -> f64 {
    if let Some(sr) = calc_star_rating_local(beatmap_id, status, mods, mode).await {
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
