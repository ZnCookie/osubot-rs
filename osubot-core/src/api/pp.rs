use crate::log_fmt;
use crate::types::{GameMode, Score, ScoreStatistics};

use osubot_types::{PpBreakdown, PpIfAcc};

use super::download_beatmap_osu;

pub struct PpCalcParams<'a> {
    pub osu_path: &'a std::path::Path,
    pub mode: GameMode,
    pub mods: rosu_mods::GameMods,
    pub accuracy: f64,
    pub max_combo: i64,
    pub miss_count: i64,
    pub is_lazer: bool,
    pub statistics: Option<&'a ScoreStatistics>,
    pub passed: bool,
}

fn create_performance<'a>(
    map: &'a rosu_pp::Beatmap,
    diff_attrs: Option<rosu_pp::any::DifficultyAttributes>,
    mods: rosu_pp::GameMods,
    is_lazer: bool,
    needs_convert: bool,
    target_mode: rosu_pp::model::mode::GameMode,
) -> Option<rosu_pp::Performance<'a>> {
    if needs_convert {
        let perf = rosu_pp::Performance::new(map).mods(mods).lazer(is_lazer);
        perf.try_mode(target_mode).ok()
    } else {
        let attrs = diff_attrs?;
        Some(rosu_pp::Performance::new(attrs).mods(mods).lazer(is_lazer))
    }
}

fn apply_stats_and_calculate(
    perf: rosu_pp::Performance<'_>,
    statistics: Option<&ScoreStatistics>,
    accuracy: f64,
    max_combo: u32,
    miss_count: u32,
    passed: bool,
) -> rosu_pp::any::PerformanceAttributes {
    let perf = match statistics {
        Some(s) => {
            let n300 = if passed { s.count_300 as u32 } else { 0 };
            perf.combo(max_combo)
                .n300(n300)
                .n100(s.count_100 as u32)
                .n50(s.count_50 as u32)
                .n_geki(s.count_geki as u32)
                .n_katu(s.count_katu as u32)
                .large_tick_hits(s.osu_large_tick_hits as u32)
                .small_tick_hits(s.osu_small_tick_hits as u32)
                .slider_end_hits(s.osu_slider_tail_hits as u32)
                .misses(miss_count)
        }
        None => perf
            .combo(max_combo)
            .accuracy(accuracy * 100.0)
            .misses(miss_count),
    };
    perf.calculate()
}

fn build_pp_performance<'a>(
    map: &'a rosu_pp::Beatmap,
    params: &PpCalcParams<'_>,
    needs_convert: bool,
    map_mode: rosu_pp::model::mode::GameMode,
) -> Option<rosu_pp::Performance<'a>> {
    use rosu_pp::{Difficulty, GameMods as PpMods};

    let pp_mods = PpMods::from(params.mods.clone());
    let diff_attrs = if !needs_convert {
        Some(
            Difficulty::new()
                .mods(pp_mods.clone())
                .lazer(params.is_lazer)
                .calculate(map),
        )
    } else {
        None
    };
    create_performance(
        map,
        diff_attrs,
        pp_mods,
        params.is_lazer,
        needs_convert,
        map_mode,
    )
}

pub fn calculate_pp_breakdown(params: PpCalcParams<'_>) -> Option<PpBreakdown> {
    use rosu_pp::any::PerformanceAttributes;
    use rosu_pp::Beatmap;

    let map = match Beatmap::from_path(params.osu_path) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "{}", log_fmt!("api.parse_osu_failed"));
            return None;
        }
    };

    let map_mode: rosu_pp::model::mode::GameMode = params.mode.into();
    let needs_convert = map.mode != map_mode;

    let perf = build_pp_performance(&map, &params, needs_convert, map_mode).or_else(|| {
        tracing::warn!(?params.mode, "{}", log_fmt!("api.try_mode_failed"));
        None
    })?;
    let perf_attrs = apply_stats_and_calculate(
        perf,
        params.statistics,
        params.accuracy,
        params.max_combo.max(0).try_into().unwrap_or(0),
        params.miss_count.max(0).try_into().unwrap_or(0),
        params.passed,
    );

    let total_pp = perf_attrs.pp();
    let star_rating = Some(perf_attrs.stars());

    match perf_attrs {
        PerformanceAttributes::Osu(attrs) => Some(PpBreakdown {
            aim: Some(attrs.pp_aim),
            speed: Some(attrs.pp_speed),
            accuracy: attrs.pp_acc,
            flashlight: Some(attrs.pp_flashlight),
            difficulty: None,
            total_pp,
            star_rating,
        }),
        PerformanceAttributes::Taiko(attrs) => Some(PpBreakdown {
            aim: None,
            speed: None,
            accuracy: attrs.pp_acc,
            flashlight: None,
            difficulty: Some(attrs.pp_difficulty),
            total_pp,
            star_rating,
        }),
        PerformanceAttributes::Mania(attrs) => Some(PpBreakdown {
            aim: None,
            speed: None,
            accuracy: 0.0,
            flashlight: None,
            difficulty: Some(attrs.pp_difficulty),
            total_pp,
            star_rating,
        }),
        PerformanceAttributes::Catch(_) => Some(PpBreakdown {
            aim: None,
            speed: None,
            accuracy: 0.0,
            flashlight: None,
            difficulty: None,
            total_pp,
            star_rating,
        }),
    }
}

pub fn calculate_pp_if_acc(params: PpCalcParams<'_>, beatmap_max_combo: i64) -> Option<PpIfAcc> {
    use rosu_pp::Beatmap;

    let map = match Beatmap::from_path(params.osu_path) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "{}", log_fmt!("api.ifacc_parse_failed"));
            return None;
        }
    };

    let map_mode: rosu_pp::model::mode::GameMode = params.mode.into();
    let needs_convert = map.mode != map_mode;

    let combo = params.max_combo.max(0).try_into().unwrap_or(0);
    let bm_combo = beatmap_max_combo.max(0).try_into().unwrap_or(0);
    let misses = params.miss_count.max(0).try_into().unwrap_or(0);

    let base_perf = build_pp_performance(&map, &params, needs_convert, map_mode)?;

    let perfect_pp = base_perf.clone().calculate().pp();

    let calc_pp = |acc: f64, combo: u32, misses: u32| -> f64 {
        base_perf
            .clone()
            .combo(combo)
            .accuracy(acc * 100.0)
            .misses(misses)
            .calculate()
            .pp()
    };

    let calc_acc = |acc: f64| -> f64 {
        if matches!(params.mode, GameMode::Osu) {
            if let Some(s) = params.statistics {
                base_perf
                    .clone()
                    .accuracy(acc * 100.0)
                    .combo(bm_combo)
                    .misses(0)
                    .large_tick_hits(s.osu_large_tick_hits as u32)
                    .small_tick_hits(s.osu_small_tick_hits as u32)
                    .slider_end_hits(s.osu_slider_tail_hits as u32)
                    .calculate()
                    .pp()
            } else {
                calc_pp(acc, combo, misses)
            }
        } else if matches!(params.mode, GameMode::Mania) {
            calc_pp(acc, combo, 0)
        } else {
            calc_pp(acc, combo, misses)
        }
    };

    let if_fc = 'fc: {
        let Some(s) = params.statistics else {
            break 'fc calc_pp(params.accuracy, bm_combo, 0);
        };
        let mut p = base_perf
            .clone()
            .accuracy(params.accuracy * 100.0)
            .misses(0)
            .large_tick_hits(s.osu_large_tick_hits as u32)
            .small_tick_hits(s.osu_small_tick_hits as u32)
            .slider_end_hits(s.osu_slider_tail_hits as u32);
        if !matches!(params.mode, GameMode::Mania) {
            p = p.combo(bm_combo);
        }
        p.calculate().pp()
    };

    Some(PpIfAcc {
        acc_95: calc_acc(0.95),
        acc_97: calc_acc(0.97),
        acc_98: calc_acc(0.98),
        acc_99: calc_acc(0.99),
        acc_100: calc_acc(1.0),
        if_fc,
        perfect_pp,
    })
}

pub async fn enrich_score_with_pp(score: &mut Score, mode: GameMode, compute_if_acc: bool) {
    if score.beatmap_id <= 0 {
        return;
    }

    let osu_path = match download_beatmap_osu(score.beatmap_id, &score.status).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = ?e, beatmap_id = score.beatmap_id, "{}", log_fmt!("api.pp_download_failed"));
            return;
        }
    };

    let mods_clone = score.mods.clone();
    let accuracy = score.accuracy;
    let max_combo = score.max_combo;
    let beatmap_max_combo = score.beatmap_max_combo;
    let count_miss = score.statistics.count_miss;
    let is_lazer = score.is_lazer;
    let statistics = score.statistics.clone();
    let passed = score.passed;

    let (pp_breakdown, pp_if_acc) = tokio::task::spawn_blocking(move || {
        let breakdown = calculate_pp_breakdown(PpCalcParams {
            osu_path: &osu_path,
            mode,
            mods: mods_clone.clone(),
            accuracy,
            max_combo,
            miss_count: count_miss,
            is_lazer,
            statistics: Some(&statistics),
            passed,
        });
        let if_acc = if compute_if_acc {
            calculate_pp_if_acc(
                PpCalcParams {
                    osu_path: &osu_path,
                    mode,
                    mods: mods_clone,
                    accuracy,
                    max_combo,
                    miss_count: count_miss,
                    is_lazer,
                    statistics: Some(&statistics),
                    passed,
                },
                beatmap_max_combo,
            )
        } else {
            None
        };
        (breakdown, if_acc)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = ?e, "{}", log_fmt!("api.pp_calc_panicked"));
        (None, None)
    });

    score.pp_breakdown = pp_breakdown;
    score.pp_if_acc = pp_if_acc;
    if let Some(ref if_acc) = score.pp_if_acc {
        if if_acc.perfect_pp > 0.0 {
            score.perfect_pp = Some(if_acc.perfect_pp);
        }
    }

    if let Some(ref bd) = score.pp_breakdown {
        if let Some(stars) = bd.star_rating {
            score.star_rating = stars;
        }
    }

    if score.pp.is_none() {
        if let Some(ref bd) = score.pp_breakdown {
            if bd.total_pp > 0.0 {
                score.pp = Some(bd.total_pp);
            }
        }
    }
}
