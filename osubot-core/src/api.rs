use crate::cache::beatmap_cache_dir;
use crate::rate_limiter::RateLimiter;
use crate::types::{GameMode, Score, ScoreStatistics, ScoreUser, UserStats};
use futures::stream::{self, StreamExt};
use osubot_types::{to_rosu_game_mode, PpBreakdown, PpIfAcc};
use reqwest::Client;
use rosu_mods::GameMods;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::sync::Mutex;

/// Convert osubot GameMode to rosu_pp GameMode (for DifficultyAttributes comparison / try_mode)
fn to_rosu_pp_game_mode(mode: GameMode) -> rosu_pp::model::mode::GameMode {
    match mode {
        GameMode::Osu => rosu_pp::model::mode::GameMode::Osu,
        GameMode::Taiko => rosu_pp::model::mode::GameMode::Taiko,
        GameMode::Catch => rosu_pp::model::mode::GameMode::Catch,
        GameMode::Mania => rosu_pp::model::mode::GameMode::Mania,
    }
}

/// Parameters for PP calculation functions.
/// `accuracy` is raw (0.0~1.0) — functions internally convert to percentage for rosu_pp.
pub struct PpCalcParams<'a> {
    pub osu_path: &'a std::path::Path,
    pub mode: GameMode,
    pub mods: GameMods,
    pub accuracy: f64,
    pub max_combo: i64,
    pub miss_count: i64,
    pub is_lazer: bool,
    pub statistics: Option<&'a ScoreStatistics>,
    /// Beatmap's original star rating from API, used by NF/CL fast path.
    /// When non-convert + NF/CL-only, skips difficulty calculation and passes this through.
    pub beatmap_star_rating: Option<f64>,
    /// Whether the score was passed. Used to apply `passed_objects` for
    /// failed/in-progress plays (rosu-pp 4.0.1 explicit API for partial
    /// play, conceptually similar to yumu-bot's combined
    /// `state.n300=0` + `setHitResultPriority(true)` approach).
    pub passed: bool,
}

pub(crate) const API_VERSION: &str = "20260408";

pub fn http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build reqwest client")
    })
}

/// 下载 osu! 谱面文件（.osu），带 7 天文件缓存。
///
/// 使用 `retry_on_transient` 处理瞬态错误，但不使用 `retry_on_401`，
/// 因为这是公共端点（`/osu/{id}`），无需 OAuth 认证。
pub async fn download_beatmap_osu(beatmap_id: i64) -> Result<PathBuf, ApiError> {
    let cache_path = beatmap_cache_dir().join(format!("{}.osu", beatmap_id));

    // 检查缓存（同步 I/O 移到 blocking 线程）
    let cache_valid = tokio::task::spawn_blocking({
        let cache_path = cache_path.clone();
        move || -> bool {
            if !cache_path.exists() {
                return false;
            }
            match std::fs::metadata(&cache_path) {
                Ok(meta) if meta.len() > 0 => {
                    if let Ok(modified) = meta.modified() {
                        modified.elapsed().unwrap_or(std::time::Duration::MAX)
                            < std::time::Duration::from_secs(7 * 86400)
                    } else {
                        false
                    }
                }
                _ => false,
            }
        }
    })
    .await
    .unwrap_or(false);

    if cache_valid {
        return Ok(cache_path);
    }

    let client = http_client();
    let url = format!("https://osu.ppy.sh/osu/{}", beatmap_id);

    let bytes = retry_on_transient(2, || async {
        let resp = client.get(&url).send().await?;

        classify_http_error(&resp)?;

        resp.bytes().await.map_err(ApiError::Http)
    })
    .await?;

    // 写入缓存（best-effort，与 replay 路径一致）
    let write_path = cache_path.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = std::fs::create_dir_all(
            write_path
                .parent()
                .expect("cache path always has parent dirs (beatmap_cache_dir()/id.osu)"),
        ) {
            tracing::warn!("failed to create cache dir: {e}");
        }
        if let Err(e) = std::fs::write(&write_path, &bytes) {
            tracing::warn!(
                "failed to write beatmap cache file {}: {e}",
                write_path.display()
            );
        }
    })
    .await
    .ok();

    Ok(cache_path)
}

/// Create a `Performance` calculator, converting the map mode if needed.
///
/// Returns `None` if mode conversion fails via `try_mode`.
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

/// Apply mode-appropriate stats to a `Performance` calculator and compute the result.
///
/// For all modes with `statistics` available, uses
/// `.n_geki(), .n300(), .n_katu(), .n100(), .n50(), .misses()`,
/// `.large_tick_hits(), .small_tick_hits(), .slider_end_hits()`.
/// For Osu/Taiko, geki/katu are typically 0 from the API, so calling
/// these is harmless. For Catch, katu carries the small droplet miss count
/// (`tiny_droplet_misses`). For Mania, geki/katu carry the 320/200 judgment
/// counts.
///
/// For failed scores (`passed=false`) with `statistics` available,
/// zeros n300 (matching yumu-bot's `getScoreRosuPerformance`).
/// `hitresult_priority` defaults to `BestCase` in rosu-pp 4.0.1.
///
/// When `statistics` is `None`, falls back to `combo + accuracy + misses`
/// (used by mode converts and call sites that don't pass hit counts).
///
/// **Edge case:** if all `count_*` fields are zero, `total_hits = 0` and
/// `passed_objects(0)` yields a zero-PP result. `calculate_pp_breakdown` still
/// returns `Some(PpBreakdown { total_pp: 0.0, .. })`; the actual `None`
/// materializes one frame up in `enrich_score_with_pp` (the
/// `if bd.total_pp > 0.0` guard at the bottom of that function), which leaves
/// `score.pp` unset. This is the correct behavior for a score with no hits,
/// not a bug — a future reviewer who hits this should accept the `None` rather
/// than try to "fix" it.
fn apply_stats_and_calculate(
    perf: rosu_pp::Performance<'_>,
    _mode: GameMode,
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

/// Calculate PP breakdown (aim/speed/acc/flashlight/difficulty) and star rating.
///
/// For converts, extracts star rating from `PerformanceAttributes::stars()`.
/// Catch mode returns a minimal `PpBreakdown` with only `star_rating` and `total_pp`.
pub fn calculate_pp_breakdown(params: PpCalcParams<'_>) -> Option<PpBreakdown> {
    use rosu_pp::any::PerformanceAttributes;
    use rosu_pp::{Beatmap, Difficulty, GameMods as PpMods};

    let map = match Beatmap::from_path(params.osu_path) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "Failed to parse .osu file");
            return None;
        }
    };

    let map_mode = to_rosu_pp_game_mode(params.mode);
    let needs_convert = map.mode != map_mode;

    let pp_mods = PpMods::from(params.mods);

    let diff_attrs = if !needs_convert {
        Some(
            Difficulty::new()
                .mods(pp_mods.clone())
                .lazer(params.is_lazer)
                .calculate(&map),
        )
    } else {
        None
    };

    let perf = match create_performance(
        &map,
        diff_attrs,
        pp_mods,
        params.is_lazer,
        needs_convert,
        map_mode,
    ) {
        Some(p) => p,
        None => {
            tracing::warn!(?params.mode, "try_mode failed; cannot convert");
            return None;
        }
    };
    let perf_attrs = apply_stats_and_calculate(
        perf,
        params.mode,
        params.statistics,
        params.accuracy,
        params.max_combo as u32,
        params.miss_count as u32,
        params.passed,
    );

    let total_pp = perf_attrs.pp();

    // 从已计算的 PerformanceAttributes 中提取星级
    // 对转谱和非转谱场景均有效，包含 mod 对难度的影响
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
        // Catch 模式：rosu-pp 的 CatchPerformanceAttributes 不含 PP 拆解字段，
        // 但仍可通过 stars() 获取星级。返回最小化 PpBreakdown。
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

/// Calculate PP for various accuracy levels (95%~100%) and "if FC" scenario.
///
/// Pre-builds a base `Performance` once (doing `try_mode` for converts only once),
/// then clones it for each accuracy/combo/miss combination.
///
/// **Note:** the `params.passed` field is **intentionally ignored** here. The
/// "if FC" and "if acc N%" projections are always computed as if the play
/// were full-length, because the whole point of the projection is to answer
/// "what would my PP be at a higher accuracy / without misses?". Applying
/// `passed_objects` would contradict that intent. The only signal taken from
/// the actual play is `params.miss_count` (used as-is for the `acc N%` row)
/// and `params.statistics` (used for the Mania "if FC" recompute). Callers
/// that want failed-score-corrected if-acc must compute PP themselves.
pub fn calculate_pp_if_acc(params: PpCalcParams<'_>, beatmap_max_combo: i64) -> Option<PpIfAcc> {
    use rosu_pp::{Beatmap, Difficulty, GameMods as PpMods};

    let pp_mods = PpMods::from(params.mods);
    let map = match Beatmap::from_path(params.osu_path) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = ?e, "Failed to parse .osu file for if-acc");
            return None;
        }
    };

    let map_mode = to_rosu_pp_game_mode(params.mode);
    let needs_convert = map.mode != map_mode;

    let combo = params.max_combo as u32;
    let bm_combo = beatmap_max_combo as u32;
    let misses = params.miss_count as u32;

    let diff_attrs = if !needs_convert {
        Some(
            Difficulty::new()
                .mods(pp_mods.clone())
                .lazer(params.is_lazer)
                .calculate(&map),
        )
    } else {
        None
    };

    // Pre-build base Performance once; closures clone it and set per-call params.
    // For converts this avoids repeated try_mode() calls (map re-conversion).
    let base_perf = create_performance(
        &map,
        diff_attrs.clone(),
        pp_mods.clone(),
        params.is_lazer,
        needs_convert,
        map_mode,
    );

    let perfect_pp = if let Some(perf) = base_perf.clone() {
        perf.calculate().pp()
    } else {
        0.0
    };

    let calc_pp = |acc: f64, combo: u32, misses: u32| -> f64 {
        let perf = match base_perf.clone() {
            Some(p) => p,
            None => return 0.0,
        };
        perf.combo(combo)
            .accuracy(acc * 100.0)
            .misses(misses)
            .calculate()
            .pp()
    };

    let if_fc = 'fc: {
        let Some(s) = params.statistics else {
            break 'fc calc_pp(params.accuracy, bm_combo, 0);
        };
        let Some(perf) = base_perf.clone() else {
            break 'fc 0.0;
        };
        let fc_counts = ScoreStatistics {
            count_geki: s.count_geki,
            count_300: s.count_300 + s.count_miss,
            count_katu: s.count_katu,
            count_100: s.count_100,
            count_50: s.count_50,
            count_miss: 0,
            osu_large_tick_hits: s.osu_large_tick_hits,
            osu_small_tick_hits: s.osu_small_tick_hits,
            osu_slider_tail_hits: s.osu_slider_tail_hits,
            osu_large_tick_misses: 0,
            osu_small_tick_misses: 0,
        };
        match params.mode {
            GameMode::Mania => perf
                .n_geki(fc_counts.count_geki as u32)
                .n300(fc_counts.count_300 as u32)
                .n_katu(fc_counts.count_katu as u32)
                .n100(fc_counts.count_100 as u32)
                .n50(fc_counts.count_50 as u32)
                .large_tick_hits(fc_counts.osu_large_tick_hits as u32)
                .small_tick_hits(fc_counts.osu_small_tick_hits as u32)
                .slider_end_hits(fc_counts.osu_slider_tail_hits as u32)
                .misses(fc_counts.count_miss as u32)
                .calculate()
                .pp(),
            _ => perf
                .n_geki(fc_counts.count_geki as u32)
                .n300(fc_counts.count_300 as u32)
                .n_katu(fc_counts.count_katu as u32)
                .n100(fc_counts.count_100 as u32)
                .n50(fc_counts.count_50 as u32)
                .large_tick_hits(fc_counts.osu_large_tick_hits as u32)
                .small_tick_hits(fc_counts.osu_small_tick_hits as u32)
                .slider_end_hits(fc_counts.osu_slider_tail_hits as u32)
                .misses(fc_counts.count_miss as u32)
                .combo(bm_combo)
                .calculate()
                .pp(),
        }
    };

    let calc_acc = |acc: f64| -> f64 {
        if matches!(params.mode, GameMode::Mania) {
            calc_pp(acc, combo, 0)
        } else {
            calc_pp(acc, combo, misses)
        }
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

/// Enrich a score with PP breakdown and if-acc values.
/// Downloads the .osu file if needed, calculates PP decomposition,
/// and sets the `pp_breakdown` and `pp_if_acc` fields on the score.
pub async fn enrich_score_with_pp(score: &mut Score, mode: GameMode, compute_if_acc: bool) {
    if score.beatmap_id <= 0 {
        return;
    }

    let osu_path = match download_beatmap_osu(score.beatmap_id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = ?e,
                beatmap_id = score.beatmap_id,
                "Failed to download .osu for PP calculation"
            );
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
    let beatmap_star_rating = Some(score.star_rating);
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
            beatmap_star_rating,
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
                    beatmap_star_rating: None,
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
        tracing::warn!(error = ?e, "PP calculation task panicked");
        (None, None)
    });

    score.pp_breakdown = pp_breakdown;
    score.pp_if_acc = pp_if_acc;
    if let Some(ref if_acc) = score.pp_if_acc {
        if if_acc.perfect_pp > 0.0 {
            score.perfect_pp = Some(if_acc.perfect_pp);
        }
    }

    // 从 pp_breakdown 中提取星级（所有模式，含 Catch）
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

/// osu! API v2 basic user info (for activity detection)
#[derive(Debug, serde::Deserialize)]
pub struct OsuUserInfo {
    pub id: i64,
    pub username: String,
    pub is_active: bool,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct OsuApiBeatmap {
    id: i64,
    #[serde(rename = "beatmapset_id")]
    beatmapset_id: i64,
    #[serde(default)]
    version: String,
    #[serde(default)]
    ar: f64,
    #[serde(default, alias = "accuracy")]
    od: f64,
    #[serde(default)]
    cs: f64,
    #[serde(default, alias = "drain")]
    hp: f64,
    #[serde(default)]
    bpm: f64,
    #[serde(default)]
    total_length: i64,
    #[serde(default)]
    difficulty_rating: f64,
    #[serde(default)]
    max_combo: i64,
    #[serde(default)]
    passcount: i64,
    #[serde(default)]
    playcount: i64,
    #[serde(default)]
    status: String,
}

/// osu! API v2 beatmap info from recent plays
#[derive(Debug, serde::Deserialize)]
struct OsuApiBeatmapset {
    #[serde(default)]
    artist: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    creator: String,
    #[serde(default)]
    covers: Option<serde_json::Value>,
    #[serde(default)]
    favourite_count: i64,
    #[serde(default)]
    play_count: i64,
}

/// /beatmaps/{bid}/scores/users/{uid} 返回的包装结构
#[derive(Debug, serde::Deserialize)]
struct BeatmapUserScore {
    score: Option<OsuApiScore>,
}

/// /beatmaps/{bid}/scores/users/{uid}/all 返回的包装结构
#[derive(Debug, serde::Deserialize)]
struct BeatmapScoresResponse {
    scores: Vec<OsuApiScore>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(untagged)]
enum OsuApiMod {
    Object {
        acronym: String,
        #[serde(default)]
        settings: Option<serde_json::Value>,
    },
    String(String),
}

fn default_true() -> bool {
    true
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScore {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    total_score: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)] // is_lazer now determined from build_id > 0 (aligned with yumu-bot)
    is_lazer: bool,
    #[serde(default)]
    build_id: Option<i64>,
    #[serde(default)]
    legacy_total_score: Option<i64>,
    #[serde(default)]
    accuracy: f64,
    #[serde(default)]
    max_combo: i64,
    #[serde(default)]
    pp: Option<f64>,
    #[serde(default)]
    rank: String,
    #[serde(default = "default_true")]
    passed: bool,
    #[serde(default)]
    perfect: bool,
    #[serde(default, alias = "created_at")]
    ended_at: String,
    #[serde(default)]
    has_replay: bool,
    #[serde(default)]
    legacy_score_id: Option<i64>,
    #[serde(default)]
    beatmap_id: i64,
    #[serde(default)]
    beatmapset_id: i64,
    #[serde(default)]
    beatmap: Option<OsuApiBeatmap>,
    #[serde(default)]
    beatmapset: Option<OsuApiBeatmapset>,
    #[serde(default)]
    mods: Vec<OsuApiMod>,
    #[serde(default)]
    statistics: OsuApiScoreStatistics,
    #[serde(default)]
    user: Option<serde_json::Value>,
    #[serde(default)]
    ruleset_id: i64, // 0=osu, 1=taiko, 2=catch, 3=mania
}

/// lazer: perfect/great/ok/meh/miss, legacy: count_geki/count_300/count_katu/count_100/count_50/count_miss
#[derive(Debug, serde::Deserialize, Default)]
struct OsuApiScoreStatistics {
    #[serde(default, alias = "perfect")]
    count_geki: i64,
    #[serde(default, alias = "great")]
    count_300: i64,
    #[serde(default, alias = "good")]
    count_katu: i64,
    #[serde(default)]
    count_100: i64,
    #[serde(default, alias = "meh")]
    count_50: i64,
    #[serde(default, alias = "miss")]
    count_miss: i64,
    /// Lazer `ok` field — maps to count_100 (standard) or count_katu (mania)
    #[serde(default)]
    ok: i64,
    /// Lazer `large_tick_miss` — missed large droplets (Catch)
    #[serde(default, alias = "large_tick_miss")]
    osu_large_tick_misses: i64,
    /// Lazer `small_tick_miss` — missed small droplets (Catch)
    #[serde(default, alias = "small_tick_miss")]
    osu_small_tick_misses: i64,
    /// Lazer `large_tick_hit` — slider ticks (Osu), large droplets (Catch)
    #[serde(default, alias = "large_tick_hit")]
    osu_large_tick_hits: i64,
    /// Lazer `small_tick_hit` — small slider ticks (Osu), small droplets (Catch)
    #[serde(default, alias = "small_tick_hit")]
    osu_small_tick_hits: i64,
    /// Lazer `slider_tail_hit` — slider ends (Osu)
    #[serde(default, alias = "slider_tail_hit")]
    osu_slider_tail_hits: i64,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScoreUser {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    username: Option<String>,
    avatar_url: Option<String>,
    country_code: Option<String>,
    statistics: Option<OsuApiScoreUserStatistics>,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScoreUserStatistics {
    global_rank: Option<i64>,
    country_rank: Option<i64>,
    pp: Option<f64>,
}

impl OsuApiScore {
    fn extra_mode(&self) -> GameMode {
        match self.ruleset_id {
            1 => GameMode::Taiko,
            2 => GameMode::Catch,
            3 => GameMode::Mania,
            _ => GameMode::Osu,
        }
    }
}

/// Convert osu! API v2 mod objects into rosu_mods::GameMods with full settings.
fn api_mods_to_game_mods(api_mods: &[OsuApiMod], mode: GameMode) -> GameMods {
    use rosu_mods::serde::GameModSeed;
    use serde::de::DeserializeSeed;
    let ros_mode = to_rosu_game_mode(mode);
    let seed = GameModSeed::Mode {
        mode: ros_mode,
        deny_unknown_fields: false,
    };
    let mut mods = GameMods::new();
    for m in api_mods {
        let gamemod = match m {
            OsuApiMod::String(s) => {
                let json_str = format!("\"{s}\"");
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|e| {
                        tracing::warn!(mod_str = %s, error = %e, "Failed to deserialize mod, falling back to basic");
                        rosu_mods::GameMod::new(s, ros_mode)
                    })
            }
            OsuApiMod::Object {
                acronym,
                settings: Some(settings),
            } => {
                let json = serde_json::json!({"acronym": acronym, "settings": settings});
                let json_str = json.to_string();
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|e| {
                        tracing::warn!(acronym = %acronym, error = %e, "Failed to deserialize mod with settings, falling back to basic");
                        rosu_mods::GameMod::new(acronym, ros_mode)
                    })
            }
            OsuApiMod::Object {
                acronym,
                settings: None,
            } => {
                let json_str = serde_json::json!({"acronym": acronym}).to_string();
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|e| {
                        tracing::warn!(acronym = %acronym, error = %e, "Failed to deserialize mod, falling back to basic");
                        rosu_mods::GameMod::new(acronym, ros_mode)
                    })
            }
        };
        mods.insert(gamemod);
    }
    mods
}

/// Apply mod adjustments to base AR/OD/CS/HP values.
/// Returns the effective in-game values after mods (DT, HT, HR, EZ, etc).
/// No beatmap download needed — uses BeatmapAttributesBuilder with base stats.
pub fn apply_mod_adjustment_to_stats(
    mode: GameMode,
    ar: f64,
    od: f64,
    cs: f64,
    hp: f64,
    mods: &GameMods,
) -> (f64, f64, f64, f64) {
    if mods.is_empty() {
        return (ar, od, cs, hp);
    }
    use rosu_pp::model::beatmap::BeatmapAttributesBuilder;
    use rosu_pp::model::mode::GameMode as RosuMode;
    let rosu_mode = match mode {
        GameMode::Osu => RosuMode::Osu,
        GameMode::Taiko => RosuMode::Taiko,
        GameMode::Catch => RosuMode::Catch,
        GameMode::Mania => RosuMode::Mania,
    };
    let adjusted = BeatmapAttributesBuilder::new()
        .mode(rosu_mode, false)
        .ar(ar as f32, false)
        .od(od as f32, false)
        .cs(cs as f32, false)
        .hp(hp as f32, false)
        .mods(mods.clone())
        .build()
        .apply_clock_rate();
    (
        adjusted.ar,
        adjusted.od,
        adjusted.cs as f64,
        adjusted.hp as f64,
    )
}

/// 从 covers JSON 提取 fullsize 背景图 URL（仿 yumu-bot: list → fullsize）
fn fullsize_cover_url(covers: Option<&serde_json::Value>) -> Option<String> {
    let covers = covers?;
    if let Some(list_url) = covers.get("list").and_then(|v| v.as_str()) {
        return Some(list_url.replace("@2x", "").replace("list", "fullsize"));
    }
    covers
        .get("cover")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// 对齐 yumu-bot: 用 hit statistics 重新计算 Stable 规则的 Grade（而非信任 API 的 `rank` 字段）。
/// 仅在 `passed` 为 true 时调用；若 `passed` 为 false，返回 "F"。
/// `has_hidden` 用于给 S/X 追加 "H" 后缀（HD/FL/PF）。
fn get_stable_rank(
    stats: &OsuApiScoreStatistics,
    mode: GameMode,
    passed: bool,
    has_hidden: bool,
) -> String {
    if !passed {
        return "F".to_string();
    }

    let great = stats.count_300;
    let count_100 = stats.count_100;
    let meh = stats.count_50;
    let miss = stats.count_miss;

    let total = match mode {
        GameMode::Taiko => great + count_100 + miss,
        GameMode::Catch => {
            great
                + stats.osu_large_tick_hits
                + stats.osu_small_tick_hits
                + stats.osu_large_tick_misses
                + stats.osu_small_tick_misses
                + miss
        }
        GameMode::Mania => stats.count_geki + great + stats.count_katu + count_100 + meh + miss,
        _ => great + count_100 + meh + miss, // osu!standard
    };

    let rank = match mode {
        GameMode::Taiko => {
            if great == total {
                "X"
            } else if great * 10 > total * 9 {
                if miss > 0 {
                    "A"
                } else {
                    "S"
                }
            } else if great * 10 > total * 8 {
                if miss > 0 {
                    "B"
                } else {
                    "A"
                }
            } else if great * 10 > total * 7 {
                if miss > 0 {
                    "C"
                } else {
                    "B"
                }
            } else if great * 10 > total * 6 {
                "C"
            } else {
                "D"
            }
        }
        GameMode::Catch => {
            let hit = great + stats.osu_large_tick_hits + stats.osu_small_tick_hits;
            if hit == total {
                "X"
            } else if hit * 100 > total * 98 {
                "S"
            } else if hit * 100 > total * 94 {
                "A"
            } else if hit * 100 > total * 90 {
                "B"
            } else if hit * 100 > total * 85 {
                "C"
            } else {
                "D"
            }
        }
        GameMode::Mania => {
            let perfect = stats.count_geki;
            let good = stats.count_katu;
            let judgement = perfect * 300 + great * 300 + good * 200 + count_100 * 100 + meh * 50;
            if judgement == total * 300 {
                "X"
            } else if judgement * 100 > total * 300 * 95 {
                "S"
            } else if judgement * 100 > total * 300 * 90 {
                "A"
            } else if judgement * 100 > total * 300 * 80 {
                "B"
            } else if judgement * 100 > total * 300 * 70 {
                "C"
            } else {
                "D"
            }
        }
        _ => {
            // osu!standard
            let is50_over_1p = meh * 100 > total;
            if great == total {
                "X"
            } else if great * 10 > total * 9 {
                if miss > 0 || is50_over_1p {
                    "A"
                } else {
                    "S"
                }
            } else if great * 10 > total * 8 {
                if miss > 0 {
                    "B"
                } else {
                    "A"
                }
            } else if great * 10 > total * 7 {
                if miss > 0 {
                    "C"
                } else {
                    "B"
                }
            } else if great * 10 > total * 6 {
                "C"
            } else {
                "D"
            }
        }
    };

    if has_hidden && (rank == "S" || rank == "X") {
        format!("{}H", rank)
    } else {
        rank.to_string()
    }
}

/// 对齐 yumu-bot: 用 hit statistics 重新计算 Stable 规则的 Accuracy。
/// 返回 0.0 表示无法计算（例如 fail 成绩无 max stats），此时应回退到 API 的 accuracy。
fn get_stable_accuracy(stats: &OsuApiScoreStatistics, mode: GameMode, passed: bool) -> f64 {
    let great = stats.count_300 as f64;
    let count_100 = stats.count_100 as f64;
    let meh = stats.count_50 as f64;
    let miss = stats.count_miss as f64;

    let total = if passed {
        match mode {
            GameMode::Taiko => great + count_100 + miss,
            GameMode::Catch => {
                great
                    + stats.osu_large_tick_hits as f64
                    + stats.osu_small_tick_hits as f64
                    + stats.osu_large_tick_misses as f64
                    + stats.osu_small_tick_misses as f64
                    + miss
            }
            GameMode::Mania => {
                stats.count_geki as f64 + great + stats.count_katu as f64 + count_100 + meh + miss
            }
            _ => great + count_100 + meh + miss, // osu!standard
        }
    } else {
        // 对于未通过的谱面没有 max statistics，返回 0.0 表示 fallback
        return 0.0;
    };

    if total == 0.0 {
        return 0.0;
    }

    let hit = match mode {
        GameMode::Taiko => great + 1.0 / 2.0 * count_100,
        GameMode::Catch => {
            (great + stats.osu_large_tick_hits as f64 + stats.osu_small_tick_hits as f64) * 1.0
        }
        GameMode::Mania => {
            let perfect = stats.count_geki as f64;
            let good = stats.count_katu as f64;
            perfect + great + 2.0 / 3.0 * good + 1.0 / 3.0 * count_100 + 1.0 / 6.0 * meh
        }
        _ => great + 1.0 / 3.0 * count_100 + 1.0 / 6.0 * meh, // osu!standard
    };

    (hit / total).clamp(0.0, 1.0)
}

fn api_score_to_score(api: OsuApiScore, mode: GameMode) -> Score {
    let bmap = api.beatmap.as_ref();
    let is_lazer = api.build_id.is_some_and(|id| id > 0);
    let has_hidden = api.mods.iter().any(|m| {
        let acronym = match m {
            OsuApiMod::String(s) => s.as_str(),
            OsuApiMod::Object { acronym, .. } => acronym.as_str(),
        };
        acronym == "HD" || acronym == "FL" || acronym == "PF"
    });

    let cover_url = api
        .beatmapset
        .as_ref()
        .and_then(|bs| fullsize_cover_url(bs.covers.as_ref()))
        .unwrap_or_default();
    let artist = api
        .beatmapset
        .as_ref()
        .map(|bs| bs.artist.clone())
        .unwrap_or_default();
    let title = api
        .beatmapset
        .as_ref()
        .map(|bs| bs.title.clone())
        .unwrap_or_default();
    let creator = api
        .beatmapset
        .as_ref()
        .map(|bs| bs.creator.clone())
        .unwrap_or_default();
    let fav_count = api
        .beatmapset
        .as_ref()
        .and_then(|bs| Some(bs.favourite_count).filter(|&v| v > 0));
    let play_count = api
        .beatmapset
        .as_ref()
        .and_then(|bs| Some(bs.play_count).filter(|&v| v > 0));

    let score_value = if api.score > 0 {
        api.score
    } else if !is_lazer {
        api.legacy_total_score.or(api.total_score).unwrap_or(0)
    } else {
        api.total_score.or(api.legacy_total_score).unwrap_or(0)
    };

    let user = api
        .user
        .and_then(|v| {
            let u: OsuApiScoreUser = match serde_json::from_value(v.clone()) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(error = %e, user_json = %v, "Failed to parse user from score response");
                    return None;
                }
            };
            Some(ScoreUser {
                avatar_url: u.avatar_url.unwrap_or_default(),
                country_code: u.country_code.unwrap_or_default(),
                user_id: u.id,
                username: u.username,
                global_rank: u.statistics.as_ref().and_then(|s| s.global_rank),
                country_rank: u.statistics.as_ref().and_then(|s| s.country_rank),
                pp: u.statistics.as_ref().and_then(|s| s.pp).unwrap_or(0.0),
            })
        })
        .unwrap_or(ScoreUser {
            avatar_url: String::new(),
            country_code: String::new(),
            user_id: None,
            username: None,
            global_rank: None,
            country_rank: None,
            pp: 0.0,
        });

    Score {
        score_id: api.id,
        beatmap_id: bmap.map_or(api.beatmap_id, |b| b.id),
        beatmapset_id: bmap.map_or(api.beatmapset_id, |b| b.beatmapset_id),
        artist,
        title,
        version: bmap.map_or(String::new(), |b| b.version.clone()),
        creator,
        star_rating: bmap.map_or(0.0, |b| b.difficulty_rating),
        bpm: bmap.map_or(0.0, |b| b.bpm),
        ar: bmap.map_or(0.0, |b| b.ar),
        od: bmap.map_or(0.0, |b| b.od),
        cs: bmap.map_or(0.0, |b| b.cs),
        hp: bmap.map_or(0.0, |b| b.hp),
        length_seconds: bmap.map_or(0, |b| b.total_length),
        score_value,
        accuracy: if is_lazer {
            api.accuracy
        } else {
            let stable_acc = get_stable_accuracy(&api.statistics, mode, api.passed);
            if stable_acc > 0.0 {
                stable_acc
            } else {
                api.accuracy
            }
        },
        max_combo: api.max_combo,
        beatmap_max_combo: bmap.map_or(0, |b| b.max_combo),
        pp: api.pp,
        pp_breakdown: None,
        pp_if_acc: None,
        perfect_pp: None,
        rank: if is_lazer {
            if api.passed {
                api.rank
            } else {
                "F".to_string()
            }
        } else {
            get_stable_rank(&api.statistics, mode, api.passed, has_hidden)
        },
        passed: api.passed,
        mods: if is_lazer {
            api_mods_to_game_mods(&api.mods, mode)
        } else {
            let filtered_mods: Vec<OsuApiMod> = api
                .mods
                .into_iter()
                .filter(|m| {
                    let acr = match m {
                        OsuApiMod::String(s) => s.as_str(),
                        OsuApiMod::Object { acronym, .. } => acronym.as_str(),
                    };
                    acr != "CL"
                })
                .collect();
            api_mods_to_game_mods(&filtered_mods, mode)
        },
        is_perfect: api.perfect,
        created_at: api.ended_at,
        is_lazer,
        has_replay: api.has_replay,
        legacy_score_id: api.legacy_score_id,
        statistics: ScoreStatistics {
            count_geki: api.statistics.count_geki,
            count_300: api.statistics.count_300,
            count_katu: if mode == GameMode::Mania {
                if api.statistics.count_katu != 0 {
                    api.statistics.count_katu
                } else {
                    api.statistics.ok
                }
            } else {
                api.statistics.count_katu
            },
            count_100: if mode == GameMode::Catch {
                if api.statistics.osu_large_tick_hits != 0 {
                    api.statistics.osu_large_tick_hits
                } else {
                    api.statistics.count_100
                }
            } else if mode == GameMode::Mania {
                if api.statistics.ok != 0 {
                    api.statistics.ok
                } else {
                    api.statistics.count_100
                }
            } else if api.statistics.ok != 0 {
                api.statistics.ok
            } else {
                api.statistics.count_100
            },
            count_50: if mode == GameMode::Catch {
                if api.statistics.osu_small_tick_hits != 0 {
                    api.statistics.osu_small_tick_hits
                } else {
                    api.statistics.count_50
                }
            } else {
                api.statistics.count_50
            },
            count_miss: api.statistics.count_miss,
            osu_large_tick_hits: api.statistics.osu_large_tick_hits,
            osu_small_tick_hits: api.statistics.osu_small_tick_hits,
            osu_slider_tail_hits: api.statistics.osu_slider_tail_hits,
            osu_large_tick_misses: api.statistics.osu_large_tick_misses,
            osu_small_tick_misses: api.statistics.osu_small_tick_misses,
        },
        cover_url,
        user,
        fav_count,
        play_count,
        status: bmap.map_or(String::new(), |b| b.status.clone()),
    }
}

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("User not found")]
    NotFound,
    #[error("Invalid API response")]
    InvalidResponse,
    #[error("Server error ({0})")]
    ServerError(u16),
    #[error("Response deserialization failed: {0}")]
    Deserialization(String),
    #[error("API key missing")]
    MissingApiKey,
    #[error("OAuth token error")]
    OAuthError,
    #[error("Rate limited - retry after {0:?} seconds")]
    RateLimitedWithRetryAfter(Option<u64>),
    #[error("Client rate limited - local token bucket exhausted")]
    ClientRateLimited,
}

impl ApiError {
    /// 检查错误是否为瞬态错误（可重试）。
    ///
    /// **重要：** 添加新的变体时必须更新此处的匹配（无通配符），编译器会在遗漏时提醒。
    fn is_transient(&self) -> bool {
        match self {
            ApiError::Http(_)
            | ApiError::ServerError(_)
            | ApiError::RateLimitedWithRetryAfter(_) => true,
            ApiError::NotFound
            | ApiError::InvalidResponse
            | ApiError::Deserialization(_)
            | ApiError::MissingApiKey
            | ApiError::OAuthError
            | ApiError::ClientRateLimited => false,
        }
    }
}

/// 读取响应体文本并反序列化为目标类型。
/// 反序列化失败时将 body 全文写入 warn 日志。
async fn json_body<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T, ApiError> {
    let status = resp.status();
    let url = resp.url().to_string();
    let body = resp.text().await.map_err(ApiError::Http)?;
    serde_json::from_str::<T>(&body).map_err(|e| {
        tracing::warn!(
            %status,
            %url,
            body = %body,
            error = %e,
            "API 响应反序列化失败"
        );
        ApiError::Deserialization(e.to_string())
    })
}

/// osu! OAuth token response
#[derive(Debug, serde::Deserialize)]
struct OauthResponse {
    access_token: String,
}

/// osu! API v2 user response — top-level fields
#[derive(Debug, serde::Deserialize)]
struct OsuApiV2User {
    id: i64,
    username: String,
    country_code: Option<String>, // e.g., "CN", "US", "JP"
    statistics: Option<OsuStatistics>,
    cover: Option<OsuUserCover>,
}

/// osu! API v2 user cover sub-object
/// 实际响应字段名: `custom_url` 和 `url`(API 把 "封面图" 拆成"自定义"和"默认")
#[derive(Debug, serde::Deserialize)]
struct OsuUserCover {
    custom_url: Option<String>,
    url: Option<String>,
}

/// osu! API v2 statistics sub-object
#[derive(Debug, serde::Deserialize)]
struct OsuStatistics {
    pp: Option<f64>,
    #[serde(rename = "global_rank")]
    rank: Option<i64>,
    #[serde(rename = "country_rank")]
    country_rank: Option<i64>,
    #[serde(rename = "ranked_score")]
    ranked_score: Option<i64>,
    #[serde(rename = "hit_accuracy")]
    accuracy: Option<f64>,
    #[serde(rename = "play_count")]
    playcount: Option<i64>,
    #[serde(rename = "total_hits")]
    hits: Option<i64>,
    #[serde(rename = "play_time")]
    playtime: Option<i64>, // seconds in v2
}

pub struct OauthTokenCache {
    client_id: RwLock<String>,
    client_secret: RwLock<String>,
    cache: Mutex<Option<(String, Instant)>>,
    refresh_lock: Mutex<()>,
    refresh_interval: Duration,
}

impl OauthTokenCache {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id: RwLock::new(client_id),
            client_secret: RwLock::new(client_secret),
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
            refresh_interval: Duration::from_secs(20 * 3600),
        }
    }

    pub fn is_configured(&self) -> bool {
        let cid = self.client_id.read().unwrap_or_else(|e| {
            tracing::warn!("RwLock poisoned, recovering");
            e.into_inner()
        });
        let cs = self.client_secret.read().unwrap_or_else(|e| {
            tracing::warn!("RwLock poisoned, recovering");
            e.into_inner()
        });
        !cid.is_empty() && !cs.is_empty()
    }

    /// 热重载时更新 API 凭据，同时清空已缓存的 token（旧凭据的 token 已失效）。
    pub async fn update_credentials(&self, client_id: String, client_secret: String) {
        let _guard = self.refresh_lock.lock().await;
        *self.client_id.write().unwrap_or_else(|e| {
            tracing::warn!("RwLock poisoned, recovering");
            e.into_inner()
        }) = client_id;
        *self.client_secret.write().unwrap_or_else(|e| {
            tracing::warn!("RwLock poisoned, recovering");
            e.into_inner()
        }) = client_secret;
        let mut cache = self.cache.lock().await;
        *cache = None;
    }

    pub async fn invalidate(&self) {
        let _guard = self.refresh_lock.lock().await;
        let mut guard = self.cache.lock().await;
        *guard = None;
    }

    pub async fn get_token(&self) -> Result<String, ApiError> {
        // 第一次检查：缓存有效则直接返回
        {
            let guard = self.cache.lock().await;
            if let Some((ref token, fetched_at)) = *guard {
                if fetched_at.elapsed() < self.refresh_interval {
                    return Ok(token.clone());
                }
            }
        } // guard 在此释放

        // 获取刷新锁，防止并发刷新（thundering herd）
        let _refresh_guard = self.refresh_lock.lock().await;

        // 第二次检查：可能已被其他并发请求刷新
        {
            let guard = self.cache.lock().await;
            if let Some((ref token, fetched_at)) = *guard {
                if fetched_at.elapsed() < self.refresh_interval {
                    return Ok(token.clone());
                }
            }
        }

        // 缓存过期，发 HTTP 请求（不持有锁）
        let client = http_client();
        let (cid, cs) = {
            let cid = self.client_id.read().unwrap_or_else(|e| {
                tracing::warn!("RwLock poisoned, recovering");
                e.into_inner()
            });
            let cs = self.client_secret.read().unwrap_or_else(|e| {
                tracing::warn!("RwLock poisoned, recovering");
                e.into_inner()
            });
            (cid.clone(), cs.clone())
        };
        let params = [
            ("client_id", cid.as_str()),
            ("client_secret", cs.as_str()),
            ("grant_type", "client_credentials"),
            ("scope", "public"),
        ];
        let resp = client
            .post("https://osu.ppy.sh/oauth/token")
            .form(&params)
            .send()
            .await?;

        if resp.status() != 200 {
            return Err(ApiError::OAuthError);
        }

        let token_data: OauthResponse = json_body(resp).await?;

        // 重新获取锁，写入缓存
        {
            let mut guard = self.cache.lock().await;
            *guard = Some((token_data.access_token.clone(), Instant::now()));
        }

        Ok(token_data.access_token)
    }
}

/// 将 HTTP 响应状态码分类为 `ApiError`。
///
/// - 429 → `RateLimitedWithRetryAfter`（钳位至 300s，由 `retry_on_transient` 处理重试）
/// - 5xx → `ServerError`（由 `retry_on_transient` 处理重试）
/// - 其他非成功 → `InvalidResponse`（不重试）
/// - 成功 → `Ok(())`
///
/// 404 由调用方单独处理（返回 `NotFound`）。
const MAX_RETRY_AFTER_SECS: u64 = 300;

pub(crate) fn classify_http_error(resp: &reqwest::Response) -> Result<(), ApiError> {
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        // 尝试读取 Retry-After 头
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.min(MAX_RETRY_AFTER_SECS));

        return Err(ApiError::RateLimitedWithRetryAfter(retry_after));
    }
    if status.is_server_error() {
        return Err(ApiError::ServerError(status.as_u16()));
    }
    if !status.is_success() {
        return Err(ApiError::InvalidResponse);
    }
    Ok(())
}

/// 计算指数退避 + jitter 延迟。
/// jitter 在基准值的 75%~125% 之间波动，双向分散避免 thundering herd。
/// `attempt` 由调用方保证 <= 30（`max_retries <= 30`），无需溢出防护。
fn backoff_with_jitter(attempt: u32) -> Duration {
    use rand::RngExt;
    let base_delay = Duration::from_secs(1);
    let exp = base_delay * 2u32.pow(attempt.min(31));
    let exp_ms = exp.as_millis() as u64;
    // 75%~125% 范围：先算 75% 基准，再加 0~50% 随机偏移
    let min_ms = exp_ms * 3 / 4;
    let range_ms = exp_ms / 2;
    let jitter_ms = if range_ms > 0 {
        rand::rng().random_range(0..=range_ms)
    } else {
        0
    };
    Duration::from_millis(min_ms + jitter_ms)
}

/// Execute an async operation with retry on 401 (OAuth invalidation + exponential backoff).
///
/// Max retries capped at 30 to prevent overflow.
///
/// Note: `RateLimitedWithRetryAfter` (429) is handled exclusively by `retry_on_transient`.
/// When nested, this function only handles OAuth errors.
pub(crate) async fn retry_on_401<F, Fut, T>(
    oauth: &OauthTokenCache,
    max_retries: u32,
    operation: F,
) -> Result<T, ApiError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    debug_assert!(
        max_retries <= 30,
        "max_retries must be <= 30, got {max_retries}"
    );
    let mut attempt = 0u32;
    loop {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(ApiError::OAuthError) if attempt < max_retries => {
                oauth.invalidate().await;
                let delay = backoff_with_jitter(attempt);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Execute an async operation with retry on transient errors (network errors, 5xx, 429).
///
/// Uses exponential backoff with jitter. Max retries capped at 30 to prevent overflow.
///
/// This is the **only** function that retries `RateLimitedWithRetryAfter` (429). When nested with
/// `retry_on_401`, the inner function handles OAuth errors only, avoiding double-retry
/// on 429.
///
/// Note: When nested with `retry_on_401`, transient errors may produce log entries at
/// both levels — the inner level logs OAuth context, the outer logs retry decisions.
/// This is intentional for debugging but may be verbose in production logs.
pub(crate) async fn retry_on_transient<F, Fut, T>(
    max_retries: u32,
    operation: F,
) -> Result<T, ApiError>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, ApiError>>,
{
    debug_assert!(
        max_retries <= 30,
        "max_retries must be <= 30, got {max_retries}"
    );
    let mut attempt = 0u32;
    loop {
        match operation().await {
            Ok(val) => return Ok(val),
            Err(ApiError::RateLimitedWithRetryAfter(Some(retry_after)))
                if attempt < max_retries =>
            {
                // 使用服务器提供的 Retry-After 延迟
                let delay = Duration::from_secs(retry_after);
                tracing::warn!(
                    attempt = attempt + 1,
                    max_retries,
                    delay_secs = retry_after,
                    "Retrying after Retry-After header"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) if e.is_transient() && attempt < max_retries => {
                let delay = backoff_with_jitter(attempt);
                tracing::warn!(
                    error = %e,
                    attempt = attempt + 1,
                    max_retries,
                    delay_ms = delay.as_millis(),
                    "Retrying transient API error"
                );
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn fetch_user_stats_internal(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    url: &str,
) -> Result<UserStats, ApiError> {
    let client = http_client();

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                return Err(ApiError::NotFound);
            }
            classify_http_error(&resp)?;

            let data: OsuApiV2User = json_body(resp).await?;

            let stats = match data.statistics {
                Some(s) => s,
                None => return Err(ApiError::NotFound),
            };

            Ok(UserStats {
                user_id: data.id,
                username: data.username,
                pp: stats.pp.unwrap_or(0.0),
                rank: stats.rank.unwrap_or(0),
                country_rank: stats.country_rank.unwrap_or(0),
                country_code: data.country_code.unwrap_or_else(|| "XX".to_string()),
                ranked_score: stats.ranked_score.unwrap_or(0),
                accuracy: stats.accuracy.unwrap_or(0.0),
                playcount: stats.playcount.unwrap_or(0),
                hits: stats.hits.unwrap_or(0),
                playtime: stats.playtime.unwrap_or(0),
                rank_change: None,
                country_rank_change: None,
                cover_url: data.cover.and_then(|c| c.custom_url.or(c.url)),
            })
        })
    })
    .await
}

/// 纯数字用户名加 @ 前缀，避免被当作 user_id 查询
fn url_encode_username(username: &str) -> String {
    if username.chars().all(|c| c.is_ascii_digit()) {
        format!("@{}", username)
    } else {
        username.to_string()
    }
}

/// 通过用户名获取用户详细统计（pp、排名、准确率等）
pub async fn fetch_user_stats_by_username(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    username: &str,
    mode: GameMode,
) -> Result<UserStats, ApiError> {
    let mode_param = mode.api_value();
    let url_username = url_encode_username(username);
    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/{}",
        url_username, mode_param
    );
    fetch_user_stats_internal(rate_limiter, oauth, &url).await
}

/// 通过用户 ID 获取用户详细统计
pub async fn fetch_user_stats_by_user_id(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<UserStats, ApiError> {
    let mode_param = mode.api_value();
    let url = format!("https://osu.ppy.sh/api/v2/users/{}/{}", user_id, mode_param);
    fetch_user_stats_internal(rate_limiter, oauth, &url).await
}

/// 回填缺失的 score 详情（beatmap od/hp/max_combo、lazer 分数值）
async fn backfill_score_details(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    score: &mut Score,
    mode_str: &str,
) {
    // 回填谱面数据（SoloScore 格式中可能不包含嵌套 beatmap）
    if (score.ar == 0.0
        || score.od == 0.0
        || score.star_rating == 0.0
        || score.beatmap_max_combo == 0
        || score.status.is_empty())
        && score.beatmap_id > 0
    {
        if let Ok(bm) = fetch_beatmap(rate_limiter, oauth, score.beatmap_id).await {
            score.ar = bm.ar;
            score.od = bm.od;
            score.cs = bm.cs;
            score.hp = bm.hp;
            score.star_rating = bm.difficulty_rating;
            score.bpm = bm.bpm;
            score.length_seconds = bm.total_length;
            score.beatmap_max_combo = bm.max_combo;
            if score.version.is_empty() {
                score.version = bm.version;
            }
            if score.status.is_empty() {
                score.status = bm.status;
            }
            if score.beatmapset_id == 0 {
                score.beatmapset_id = bm.beatmapset_id;
            }
        }
    }

    // 回填 beatmapset 元数据（SoloScore 格式中不包含）
    if (score.artist.is_empty() || score.title.is_empty() || score.cover_url.is_empty())
        && score.beatmapset_id > 0
    {
        match fetch_beatmapset(rate_limiter, oauth, score.beatmapset_id).await {
            Ok(bs) => {
                score.artist = bs.artist;
                score.title = bs.title;
                score.creator = bs.creator;
                if score.cover_url.is_empty() {
                    score.cover_url = fullsize_cover_url(bs.covers.as_ref()).unwrap_or_default();
                }
                if score.fav_count.is_none() {
                    score.fav_count = Some(bs.favourite_count).filter(|&v| v > 0);
                }
                if score.play_count.is_none() {
                    score.play_count = Some(bs.play_count).filter(|&v| v > 0);
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    beatmapset_id = score.beatmapset_id,
                    "Failed to backfill beatmapset metadata"
                );
            }
        }
    }

    // 回填 lazer 分数值
    if score.score_value == 0 && score.score_id > 0 {
        match fetch_score_detail(rate_limiter, oauth, mode_str, score.score_id).await {
            Ok(Some(val)) => {
                score.score_value = val;
                tracing::trace!(
                    score_id = score.score_id,
                    score_value = val,
                    "Backfilled score value from detail endpoint"
                );
            }
            Ok(None) => {
                tracing::trace!(
                    score_id = score.score_id,
                    "Score detail endpoint returned no value"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    score_id = score.score_id,
                    "Failed to backfill score value"
                );
            }
        }
    }
}

/// 获取用户最近游玩成绩列表（pass/recent），自动回填缺失数据
pub async fn get_user_recent(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    user_id: i64,
    mode: GameMode,
    include_fails: bool,
    limit: u32,
) -> Result<Vec<Score>, ApiError> {
    let client = http_client();

    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/scores/recent?mode={}&include_fails={}&limit={}&legacy_only=0",
        user_id,
        mode.api_value(),
        if include_fails { 1 } else { 0 },
        limit
    );

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                return Err(ApiError::NotFound);
            }
            classify_http_error(&resp)?;

            let raw_json: serde_json::Value = json_body(resp).await?;

            let plays: Vec<OsuApiScore> = serde_json::from_value(raw_json).map_err(|e| {
                tracing::error!(error = %e, "Failed to parse score JSON");
                ApiError::InvalidResponse
            })?;
            let mut scores_raw: Vec<Score> = plays
                .into_iter()
                .map(|p| api_score_to_score(p, mode))
                .collect();
            scores_raw.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            let mode_str = mode.api_value().to_string();

            let scores: Vec<Score> = stream::iter(scores_raw)
                .map(|mut score| {
                    let rl = rate_limiter.clone();
                    let oa = oauth.clone();
                    let ruleset = mode_str.clone();
                    async move {
                        backfill_score_details(&rl, &oa, &mut score, &ruleset).await;
                        score
                    }
                })
                .buffered(5)
                .collect()
                .await;

            Ok(scores)
        })
    })
    .await
}

/// 获取用户在指定谱面的最佳成绩（支持 mod 过滤）
/// 仿 yumu-bot retryOn404: 先用 legacy_only=0 带 mode 请求，
/// 404 则用 legacy_only=1 不带 mode 重试。
pub async fn get_user_beatmap_score(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    beatmap_id: i64,
    user_id: i64,
    mode: GameMode,
    mods: &Option<Vec<String>>,
) -> Result<Score, ApiError> {
    let client = http_client();
    let (url_primary, url_retry) = {
        let mut primary = format!(
            "https://osu.ppy.sh/api/v2/beatmaps/{}/scores/users/{}?legacy_only=0",
            beatmap_id, user_id,
        );
        if mode != GameMode::Osu {
            primary.push_str(&format!("&mode={}", mode.api_value()));
        }
        let mut retry = format!(
            "https://osu.ppy.sh/api/v2/beatmaps/{}/scores/users/{}?legacy_only=1",
            beatmap_id, user_id,
        );
        if let Some(mod_list) = mods {
            for m in mod_list {
                primary.push_str("&mods[]=");
                primary.push_str(m);
                retry.push_str("&mods[]=");
                retry.push_str(m);
            }
        }
        (primary, retry)
    };

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;
            let resp = client
                .get(&url_primary)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                tracing::debug!(
                    beatmap_id,
                    user_id,
                    ?mode,
                    "Beatmap score 404, retrying with legacy_only=1"
                );
                let access_token = oauth.get_token().await?;
                let retry_resp = client
                    .get(&url_retry)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .header("x-api-version", API_VERSION)
                    .send()
                    .await?;

                if retry_resp.status() == 404 {
                    return Err(ApiError::NotFound);
                }
                classify_http_error(&retry_resp)?;

                let body = retry_resp.text().await.map_err(ApiError::Http)?;
                let raw: BeatmapUserScore = serde_json::from_str(&body).map_err(|e| {
                    tracing::error!(error = %e, body, "BeatmapScore retry parse failed");
                    ApiError::InvalidResponse
                })?;
                let api_score = raw.score.ok_or(ApiError::NotFound)?;
                let mut score = api_score_to_score(api_score, mode);
                backfill_score_details(rate_limiter, oauth, &mut score, mode.api_value()).await;
                return Ok(score);
            }

            classify_http_error(&resp)?;

            let body = resp.text().await.map_err(ApiError::Http)?;
            let raw: BeatmapUserScore = serde_json::from_str(&body).map_err(|e| {
                tracing::error!(error = %e, body, "BeatmapScore parse failed");
                ApiError::InvalidResponse
            })?;
            let api_score = raw.score.ok_or(ApiError::NotFound)?;
            let mut score = api_score_to_score(api_score, mode);
            backfill_score_details(rate_limiter, oauth, &mut score, mode.api_value()).await;
            Ok(score)
        })
    })
    .await
}

/// 获取用户在指定谱面的所有成绩（!ss 使用）
/// `limit`：若 Some(N)，URL 追加 `&limit=N`，由 API 端截断；客户端仍以
/// API 返回的顺序取前 N（API 失败时仍能按 N 截断作为兜底）。
/// 仿 yumu-bot retryOn404: 先用 legacy_only=0 带 mode 请求，
/// 404 则用 legacy_only=1 不带 mode 重试。
pub async fn get_user_beatmap_scores_all(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    beatmap_id: i64,
    user_id: i64,
    mode: GameMode,
    limit: Option<u32>,
) -> Result<Vec<Score>, ApiError> {
    let client = http_client();
    let mut url_primary = format!(
        "https://osu.ppy.sh/api/v2/beatmaps/{}/scores/users/{}/all?legacy_only=0",
        beatmap_id, user_id,
    );
    if mode != GameMode::Osu {
        url_primary.push_str(&format!("&mode={}", mode.api_value()));
    }
    if let Some(n) = limit {
        url_primary.push_str(&format!("&limit={}", n));
    }
    let url_retry = format!(
        "https://osu.ppy.sh/api/v2/beatmaps/{}/scores/users/{}/all?legacy_only=1",
        beatmap_id, user_id,
    );

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;
            let resp = client
                .get(&url_primary)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                tracing::debug!(
                    beatmap_id,
                    user_id,
                    ?mode,
                    "Beatmap scores all 404, retrying with legacy_only=1"
                );
                let access_token = oauth.get_token().await?;
                let retry_resp = client
                    .get(&url_retry)
                    .header("Authorization", format!("Bearer {}", access_token))
                    .header("x-api-version", API_VERSION)
                    .send()
                    .await?;

                if retry_resp.status() == 404 {
                    return Err(ApiError::NotFound);
                }
                classify_http_error(&retry_resp)?;

                let body = retry_resp.text().await.map_err(ApiError::Http)?;
                let raw: BeatmapScoresResponse = serde_json::from_str(&body).map_err(|e| {
                    tracing::error!(error = %e, body, "BeatmapScoresAll retry parse failed");
                    ApiError::InvalidResponse
                })?;
                let scores_raw: Vec<Score> = raw
                    .scores
                    .into_iter()
                    .map(|s| api_score_to_score(s, mode))
                    .collect();
                let mode_str = mode.api_value().to_string();

                let scores: Vec<Score> = stream::iter(scores_raw)
                    .map(|mut score| {
                        let rl = rate_limiter.clone();
                        let oa = oauth.clone();
                        let ruleset = mode_str.clone();
                        async move {
                            backfill_score_details(&rl, &oa, &mut score, &ruleset).await;
                            score
                        }
                    })
                    .buffered(5)
                    .collect()
                    .await;

                if let Some(n) = limit {
                    let mut limited = scores;
                    limited.truncate(n as usize);
                    return Ok(limited);
                }
                return Ok(scores);
            }

            classify_http_error(&resp)?;

            let body = resp.text().await.map_err(ApiError::Http)?;
            let raw: BeatmapScoresResponse = serde_json::from_str(&body).map_err(|e| {
                tracing::error!(error = %e, body, "BeatmapScoresAll parse failed");
                ApiError::InvalidResponse
            })?;
            let scores_raw: Vec<Score> = raw
                .scores
                .into_iter()
                .map(|s| api_score_to_score(s, mode))
                .collect();
            let mode_str = mode.api_value().to_string();

            let scores: Vec<Score> = stream::iter(scores_raw)
                .map(|mut score| {
                    let rl = rate_limiter.clone();
                    let oa = oauth.clone();
                    let ruleset = mode_str.clone();
                    async move {
                        backfill_score_details(&rl, &oa, &mut score, &ruleset).await;
                        score
                    }
                })
                .buffered(5)
                .collect()
                .await;

            if let Some(n) = limit {
                let mut limited = scores;
                limited.truncate(n as usize);
                Ok(limited)
            } else {
                Ok(scores)
            }
        })
    })
    .await
}

/// 通过 score ID 获取单条成绩详情
pub async fn get_score_by_id(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    score_id: u64,
) -> Result<Score, ApiError> {
    let client = http_client();
    let url = format!("https://osu.ppy.sh/api/v2/scores/{}", score_id,);

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;
            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                return Err(ApiError::NotFound);
            }
            classify_http_error(&resp)?;

            let raw: OsuApiScore = json_body(resp).await?;
            let mode = raw.extra_mode();
            let mut score = api_score_to_score(raw, mode);
            backfill_score_details(rate_limiter, oauth, &mut score, mode.api_value()).await;
            Ok(score)
        })
    })
    .await
}

/// 通过 score ID 获取单条成绩详情（用于 lazer 分数值回填）
async fn fetch_score_detail(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    ruleset: &str,
    score_id: i64,
) -> Result<Option<i64>, ApiError> {
    let client = http_client();
    let url = format!("https://osu.ppy.sh/api/v2/scores/{}/{}", ruleset, score_id);
    tracing::trace!(url = %url, "Fetching score detail");

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                tracing::debug!(url = %url, status = %resp.status(), "Score detail endpoint 404");
                return Ok(None);
            }
            classify_http_error(&resp)?;

            let data: serde_json::Value = json_body(resp).await?;
            tracing::trace!(
                keys = ?data.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                score = ?data.get("score"),
                total_score = ?data.get("total_score"),
                legacy_total_score = ?data.get("legacy_total_score"),
                classic_total_score = ?data.get("classic_total_score"),
                "Score detail endpoint response"
            );
            let score_val = data
                .get("total_score")
                .and_then(|v| v.as_i64())
                .or_else(|| data.get("score").and_then(|v| v.as_i64()))
                .or_else(|| data.get("legacy_total_score").and_then(|v| v.as_i64()))
                .filter(|&v| v > 0);
            Ok(score_val)
        })
    })
    .await
}

/// 通过 beatmap ID 获取谱面信息（用于 od/hp/max_combo 回填）
async fn fetch_beatmap(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    beatmap_id: i64,
) -> Result<OsuApiBeatmap, ApiError> {
    let client = http_client();
    let url = format!("https://osu.ppy.sh/api/v2/beatmaps/{}", beatmap_id);

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                return Err(ApiError::NotFound);
            }
            classify_http_error(&resp)?;

            json_body(resp).await
        })
    })
    .await
}

/// 通过 beatmapset ID 获取谱面集信息（用于 artist/title/creator 回填）
async fn fetch_beatmapset(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    beatmapset_id: i64,
) -> Result<OsuApiBeatmapset, ApiError> {
    let client = http_client();
    let url = format!("https://osu.ppy.sh/api/v2/beatmapsets/{}", beatmapset_id,);

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                return Err(ApiError::NotFound);
            }
            classify_http_error(&resp)?;

            json_body(resp).await
        })
    })
    .await
}

/// 通过用户名获取 osu! 用户基本信息
pub async fn get_user_info(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    username: &str,
) -> Result<Option<OsuUserInfo>, ApiError> {
    let client = http_client();
    let url_username = url_encode_username(username);
    let url = format!("https://osu.ppy.sh/api/v2/users/{}", url_username);

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                return Ok(None);
            }
            classify_http_error(&resp)?;

            let user: OsuUserInfo = json_body(resp).await?;
            Ok(Some(user))
        })
    })
    .await
}

#[derive(Debug, serde::Deserialize)]
struct OsuProfileResponse {
    page: ProfilePage,
    profile_hue: Option<u16>,
    username: String,
    avatar_url: String,
    cover: Option<Cover>,
}

#[derive(Debug, serde::Deserialize)]
struct Cover {
    url: Option<String>,
    custom_url: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ProfilePage {
    html: String,
}

pub struct UserProfile {
    pub html: String,
    pub profile_hue: u16,
    pub username: String,
    pub avatar_url: String,
    pub cover_url: Option<String>,
}

/// 获取用户主页数据（用于 !profile 卡片渲染）
pub async fn fetch_user_profile(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    user_id: i64,
    mode: GameMode,
) -> Result<UserProfile, ApiError> {
    let url = format!(
        "https://osu.ppy.sh/api/v2/users/{}/{}?key=id",
        user_id,
        mode.api_value()
    );

    let client = http_client();

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    retry_on_transient(2, || {
        retry_on_401(oauth, 5, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                return Err(ApiError::NotFound);
            }
            classify_http_error(&resp)?;

            let data: OsuProfileResponse = json_body(resp).await?;

            let cover_url = data.cover.and_then(|c| c.custom_url.or(c.url));

            Ok(UserProfile {
                html: data.page.html,
                profile_hue: data.profile_hue.unwrap_or(333),
                username: data.username,
                avatar_url: data.avatar_url,
                cover_url,
            })
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_full_score() -> OsuApiScore {
        OsuApiScore {
            id: 1001,
            score: 1234567,
            total_score: None,
            legacy_total_score: None,
            accuracy: 0.9876,
            max_combo: 543,
            pp: Some(300.5),
            rank: "S".to_string(),
            passed: true,
            perfect: false,
            ended_at: "2024-01-01T00:00:00Z".to_string(),
            is_lazer: false,
            build_id: None,
            has_replay: true,
            legacy_score_id: None,
            beatmap_id: 0,
            beatmapset_id: 0,
            beatmap: Some(OsuApiBeatmap {
                id: 2001,
                beatmapset_id: 3001,
                version: "Insane".to_string(),
                difficulty_rating: 5.5,
                bpm: 180.0,
                ar: 9.0,
                od: 8.0,
                cs: 4.0,
                hp: 5.0,
                total_length: 200,
                max_combo: 800,
                passcount: 100,
                playcount: 500,
                status: "ranked".to_string(),
            }),
            beatmapset: Some(OsuApiBeatmapset {
                artist: "TestArtist".to_string(),
                title: "TestTitle".to_string(),
                creator: "Mapper".to_string(),
                covers: None,
                favourite_count: 100,
                play_count: 5000,
            }),
            mods: vec![
                OsuApiMod::String("HD".to_string()),
                OsuApiMod::String("DT".to_string()),
            ],
            statistics: OsuApiScoreStatistics {
                count_geki: 0,
                count_300: 500,
                count_katu: 0,
                count_100: 10,
                count_50: 0,
                count_miss: 1,
                osu_large_tick_hits: 0,
                osu_small_tick_hits: 0,
                osu_slider_tail_hits: 0,
                osu_large_tick_misses: 0,
                osu_small_tick_misses: 0,
                ok: 0,
            },
            ruleset_id: 0,
            user: None,
        }
    }

    #[test]
    fn test_api_score_to_score_happy_path() {
        let api = make_full_score();
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.beatmap_id, 2001);
        assert_eq!(score.beatmapset_id, 3001);
        assert_eq!(score.artist, "TestArtist");
        assert_eq!(score.title, "TestTitle");
        assert_eq!(score.version, "Insane");
        assert_eq!(score.creator, "Mapper");
        assert!((score.star_rating - 5.5).abs() < 0.0001);
        assert!((score.bpm - 180.0).abs() < 0.0001);
        assert!((score.ar - 9.0).abs() < 0.0001);
        assert!((score.od - 8.0).abs() < 0.0001);
        assert!((score.cs - 4.0).abs() < 0.0001);
        assert!((score.hp - 5.0).abs() < 0.0001);
        assert_eq!(score.length_seconds, 200);
        assert_eq!(score.score_value, 1234567);
        assert!((score.accuracy - 0.9850).abs() < 0.0001);
        assert_eq!(score.max_combo, 543);
        assert_eq!(score.beatmap_max_combo, 800);
        assert_eq!(score.pp, Some(300.5));
        assert_eq!(score.rank, "A");
        let mod_acronyms: Vec<String> = score
            .mods
            .iter()
            .map(|m| m.acronym().as_str().to_string())
            .collect();
        assert!(mod_acronyms.iter().any(|m| m == "HD"));
        assert!(mod_acronyms.iter().any(|m| m == "DT"));
        assert_eq!(mod_acronyms.len(), 2);
        assert!(!score.is_perfect);
        assert_eq!(score.created_at, "2024-01-01T00:00:00Z");
        assert!(!score.is_lazer);
        assert_eq!(score.statistics.count_300, 500);
        assert_eq!(score.statistics.count_100, 10);
        assert_eq!(score.statistics.count_50, 0);
        assert_eq!(score.statistics.count_miss, 1);
        assert_eq!(score.cover_url, "");
        assert_eq!(score.user.avatar_url, "");
        assert_eq!(score.user.country_code, "");
        assert_eq!(score.user.global_rank, None);
        assert_eq!(score.user.country_rank, None);
        assert!((score.user.pp - 0.0).abs() < 0.0001);
        assert_eq!(score.status, "ranked");
    }

    #[test]
    fn test_api_score_to_score_pp_null() {
        let mut api = make_full_score();
        api.pp = None;
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.pp, None);
    }

    #[test]
    fn test_api_score_to_score_is_perfect() {
        let mut api = make_full_score();
        api.perfect = true;
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.is_perfect);
    }

    #[test]
    fn test_api_score_to_score_empty_mods() {
        let mut api = make_full_score();
        api.mods = vec![];
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.mods.is_empty());
    }

    #[test]
    fn test_api_score_to_score_nested_user_data() {
        let mut api = make_full_score();
        api.beatmapset.as_mut().unwrap().covers = Some(serde_json::json!({
            "cover": "https://example.com/cover.jpg"
        }));
        api.user = Some(serde_json::json!({
            "id": 1001,
            "username": "TestPlayer",
            "avatar_url": "https://example.com/avatar.png",
            "country_code": "CN",
            "statistics": {
                "global_rank": 1234,
                "country_rank": 56,
                "pp": 9876.5
            }
        }));
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.cover_url, "https://example.com/cover.jpg");
        assert_eq!(score.user.avatar_url, "https://example.com/avatar.png");
        assert_eq!(score.user.country_code, "CN");
        assert_eq!(score.user.user_id, Some(1001));
        assert_eq!(score.user.username.as_deref(), Some("TestPlayer"));
        assert_eq!(score.user.global_rank, Some(1234));
        assert_eq!(score.user.country_rank, Some(56));
        assert!((score.user.pp - 9876.5).abs() < 0.0001);
    }

    #[test]
    fn mod_adjust_osu_hr_scales_cs_and_ar() {
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::HardRockOsu(Default::default()));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 9.0, 8.0, 4.0, 5.0, &mods);
        assert!((ar - 10.0).abs() < 0.01, "ar={ar}");
        assert!((od - 10.0).abs() < 0.01, "od={od}");
        assert!((cs - 5.2).abs() < 0.01, "cs={cs}");
        assert!((hp - 7.0).abs() < 0.01, "hp={hp}");
    }

    #[test]
    fn mod_adjust_mania_hr_leaves_cs_and_ar_unchanged() {
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::HardRockMania(Default::default()));
        let (ar, _od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Mania, 9.0, 8.0, 4.0, 5.0, &mods);
        assert!(
            (ar - 9.0).abs() < 0.01,
            "mania ar should be unchanged, got {ar}"
        );
        assert!(
            (cs - 4.0).abs() < 0.01,
            "mania cs should be unchanged, got {cs}"
        );
        assert!((hp - 7.0).abs() < 0.01, "hp={hp}");
    }

    #[test]
    fn mod_adjust_no_mods_returns_base() {
        let mods = rosu_mods::GameMods::new();
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 9.0, 8.0, 4.0, 5.0, &mods);
        assert_eq!((ar, od, cs, hp), (9.0, 8.0, 4.0, 5.0));
    }

    #[test]
    fn test_api_score_to_score_lazer_by_build_id() {
        let mut api = make_full_score();
        api.is_lazer = false;
        api.build_id = Some(12345);
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_lazer_by_legacy_total_score_zero() {
        let mut api = make_full_score();
        api.is_lazer = false;
        api.build_id = None;
        api.legacy_total_score = Some(0);
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(
            !score.is_lazer,
            "legacy_total_score=0 should no longer trigger is_lazer"
        );
    }

    #[test]
    fn test_api_score_to_score_not_lazer_when_build_id_zero() {
        let mut api = make_full_score();
        api.is_lazer = false;
        api.build_id = Some(0);
        api.legacy_total_score = Some(5000);
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(!score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_not_lazer_all_conditions_false() {
        let mut api = make_full_score();
        api.is_lazer = false;
        api.build_id = None;
        api.legacy_total_score = None;
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(!score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_solo_score_no_beatmap() {
        let mut api = make_full_score();
        api.beatmap = None;
        api.beatmapset = None;
        api.beatmap_id = 9999;
        api.beatmapset_id = 8888;
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.beatmap_id, 9999);
        assert_eq!(score.beatmapset_id, 8888);
        assert!(score.artist.is_empty());
        assert!(score.title.is_empty());
        assert!(score.version.is_empty());
        assert!((score.ar - 0.0).abs() < 0.0001);
        assert!((score.od - 0.0).abs() < 0.0001);
        assert_eq!(score.beatmap_max_combo, 0);
        assert!(score.status.is_empty());
        assert!(score.cover_url.is_empty());
    }

    #[test]
    fn test_api_score_to_score_solo_score_beatmap_id_zero() {
        let mut api = make_full_score();
        api.beatmap = None;
        api.beatmapset = None;
        api.beatmap_id = 0;
        api.beatmapset_id = 0;
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.beatmap_id, 0);
        assert_eq!(score.beatmapset_id, 0);
    }

    #[test]
    fn test_api_score_to_score_solo_score_covers_fullsize() {
        let mut api = make_full_score();
        api.beatmap = None;
        api.beatmapset = Some(OsuApiBeatmapset {
            artist: "Artist".to_string(),
            title: "Title".to_string(),
            creator: "Creator".to_string(),
            covers: Some(serde_json::json!({
                "cover": "https://a.ppy.sh/thumb/1.jpg",
                "cover@2x": "https://a.ppy.sh/thumb@2x/1.jpg",
                "card": "https://a.ppy.sh/card/1.jpg",
                "card@2x": "https://a.ppy.sh/card@2x/1.jpg",
                "list": "https://assets.ppy.sh/beatmaps/1/covers/list.jpg",
                "list@2x": "https://assets.ppy.sh/beatmaps/1/covers/list@2x.jpg",
                "slimcover": "https://a.ppy.sh/slim/1.jpg",
                "slimcover@2x": "https://a.ppy.sh/slim@2x/1.jpg",
            })),
            favourite_count: 0,
            play_count: 0,
        });
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(
            score.cover_url,
            "https://assets.ppy.sh/beatmaps/1/covers/fullsize.jpg"
        );
    }

    #[test]
    fn test_is_transient_classification() {
        assert!(!ApiError::NotFound.is_transient());
        assert!(!ApiError::InvalidResponse.is_transient());
        assert!(!ApiError::MissingApiKey.is_transient());
        assert!(!ApiError::OAuthError.is_transient());
        assert!(ApiError::RateLimitedWithRetryAfter(Some(60)).is_transient());
        assert!(ApiError::RateLimitedWithRetryAfter(None).is_transient());
        assert!(!ApiError::ClientRateLimited.is_transient());
        assert!(ApiError::ServerError(500).is_transient());
        assert!(ApiError::ServerError(503).is_transient());
        assert!(!ApiError::Deserialization("bad json".into()).is_transient());
    }

    #[test]
    fn test_server_error_display() {
        let e = ApiError::ServerError(502);
        assert_eq!(format!("{}", e), "Server error (502)");
    }

    #[test]
    fn test_deserialization_display() {
        let e = ApiError::Deserialization("missing field `id`".into());
        assert!(format!("{}", e).contains("missing field `id`"));
    }

    #[tokio::test]
    async fn test_retry_on_transient_retries_server_error() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_on_transient(2, || {
            let attempts = attempts_clone.clone();
            async move {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    Err(ApiError::ServerError(500))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_on_transient_fails_after_max_retries() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<i32, _> = retry_on_transient(2, || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(ApiError::ServerError(503))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3); // 1 initial + 2 retries
    }

    #[tokio::test]
    async fn test_retry_on_transient_no_retry_on_non_transient() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result: Result<i32, _> = retry_on_transient(2, || {
            let attempts = attempts_clone.clone();
            async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(ApiError::NotFound)
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1); // no retry
    }

    #[tokio::test]
    async fn test_retry_on_transient_retries_rate_limited() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_on_transient(1, || {
            let attempts = attempts_clone.clone();
            async move {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    Err(ApiError::RateLimitedWithRetryAfter(None))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_backoff_with_jitter_increases() {
        let d0 = backoff_with_jitter(0);
        let d1 = backoff_with_jitter(1);
        let d2 = backoff_with_jitter(2);
        // 最小延迟应递增：750ms, 1500ms, 3000ms
        assert!(d0 >= Duration::from_millis(750), "d0={d0:?}");
        assert!(d1 >= Duration::from_millis(1500), "d1={d1:?}");
        assert!(d2 >= Duration::from_millis(3000), "d2={d2:?}");
    }

    #[test]
    fn test_backoff_with_jitter_has_upper_bound() {
        // jitter 不应超过基准值的 125%
        let d = backoff_with_jitter(3); // 基准 8s
        assert!(d <= Duration::from_millis(10000), "d={d:?}"); // 8s * 1.25 = 10s
    }

    #[test]
    #[should_panic(expected = "max_retries must be <= 30")]
    fn test_retry_on_401_rejects_large_max_retries() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let oauth = OauthTokenCache::new("test".to_string(), "test".to_string());
            let _ = retry_on_401(&oauth, 31, || async { Ok(42) }).await;
        });
    }

    #[test]
    #[should_panic(expected = "max_retries must be <= 30")]
    fn test_retry_on_transient_rejects_large_max_retries() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let _ = retry_on_transient(31, || async { Ok(42) }).await;
        });
    }

    #[tokio::test]
    async fn test_retry_on_transient_retries_http_error() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_on_transient(2, || {
            let attempts = attempts_clone.clone();
            async move {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count < 1 {
                    let client = reqwest::Client::builder()
                        .timeout(Duration::from_millis(500))
                        .build()
                        .unwrap();
                    Err(ApiError::Http(
                        client.get("http://127.0.0.1:1").send().await.unwrap_err(),
                    ))
                } else {
                    Ok("connected")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "connected");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_after_clamped() {
        // 启动本地 TCP 服务器返回 429 + Retry-After: 9999
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            use tokio::io::AsyncWriteExt;
            let response =
                b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 9999\r\ncontent-length: 0\r\n\r\n";
            stream.write_all(response).await.unwrap();
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .unwrap();
        let err = classify_http_error(&resp).unwrap_err();
        match err {
            ApiError::RateLimitedWithRetryAfter(Some(secs)) => assert_eq!(secs, 300),
            other => panic!("expected RateLimitedWithRetryAfter(Some(300)), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_retry_on_transient_respects_retry_after() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = retry_on_transient(2, || {
            let attempts = attempts_clone.clone();
            async move {
                let count = attempts.fetch_add(1, Ordering::SeqCst);
                if count == 0 {
                    Err(ApiError::RateLimitedWithRetryAfter(Some(1)))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
