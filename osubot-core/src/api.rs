use crate::cache::beatmap_cache_dir;
use crate::rate_limiter::RateLimiter;
use crate::types::{GameMode, Score, ScoreStatistics, ScoreUser, UserStats};
use futures::stream::{self, StreamExt};
use osubot_types::{to_rosu_game_mode, PpBreakdown, PpIfAcc};
use reqwest::Client;
use rosu_mods::GameMods;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
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
    pub beatmap_star_rating: Option<f64>,
}

pub(crate) const API_VERSION: &str = "20260408";

pub(crate) fn http_client() -> &'static Client {
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
        let _ = std::fs::create_dir_all(write_path.parent().unwrap());
        let _ = std::fs::write(&write_path, &bytes);
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
        Some(
            rosu_pp::Performance::new(diff_attrs.unwrap())
                .mods(mods)
                .lazer(is_lazer),
        )
    }
}

/// Apply mode-appropriate stats to a `Performance` calculator and compute the result.
///
/// For Mania with statistics available, uses hit-count fields (`n_geki`, `n_katu`, etc.).
/// For all other modes, uses combo + accuracy + misses.
fn apply_stats_and_calculate(
    perf: rosu_pp::Performance<'_>,
    mode: GameMode,
    statistics: Option<&ScoreStatistics>,
    accuracy: f64,
    max_combo: u32,
    miss_count: u32,
) -> rosu_pp::any::PerformanceAttributes {
    let perf = if let (GameMode::Mania, Some(s)) = (mode, statistics) {
        perf.n_geki(s.count_geki as u32)
            .n300(s.count_300 as u32)
            .n_katu(s.count_katu as u32)
            .n100(s.count_100 as u32)
            .n50(s.count_50 as u32)
            .misses(miss_count)
    } else {
        perf.combo(max_combo)
            .accuracy(accuracy * 100.0)
            .misses(miss_count)
    };
    perf.calculate()
}

/// 判断 mod 是否仅包含不影响难度的 mod（NF、CL）
fn has_only_non_difficulty_mods(mods: &GameMods) -> bool {
    use rosu_mods::GameModIntermode;
    if mods.is_empty() {
        return true;
    }
    mods.iter().all(|m| {
        let intermode = m.intermode();
        intermode == GameModIntermode::NoFail || intermode == GameModIntermode::Classic
    })
}

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

    // 非转谱 + 仅 NF/CL：跳过难度计算，使用传入的谱面原始星级
    // NF 和 CL 不影响难度，rosu-pp 的 Difficulty::calculate() 会忽略它们
    // total_pp 和 accuracy 设为 0.0：score.pp 已来自 API，此处仅用于提取 star_rating
    // 必须在 pp_mods 消费 params.mods 之前检查
    if !needs_convert
        && params.beatmap_star_rating.is_some()
        && has_only_non_difficulty_mods(&params.mods)
    {
        return Some(PpBreakdown {
            aim: None,
            speed: None,
            accuracy: 0.0,
            flashlight: None,
            difficulty: None,
            total_pp: 0.0,
            star_rating: params.beatmap_star_rating,
        });
    }

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

    let calc_pp = |acc: f64, combo: u32, misses: u32| -> f64 {
        let perf = match create_performance(
            &map,
            diff_attrs.clone(),
            pp_mods.clone(),
            params.is_lazer,
            needs_convert,
            map_mode,
        ) {
            Some(p) => p,
            None => return 0.0,
        };
        perf.combo(combo)
            .accuracy(acc * 100.0)
            .misses(misses)
            .calculate()
            .pp()
    };

    let calc_mania_fc = |counts: &ScoreStatistics, miss_override: u32| -> f64 {
        let perf = match create_performance(
            &map,
            diff_attrs.clone(),
            pp_mods.clone(),
            params.is_lazer,
            needs_convert,
            map_mode,
        ) {
            Some(p) => p,
            None => return 0.0,
        };
        perf.n_geki(counts.count_geki as u32)
            .n300(counts.count_300 as u32)
            .n_katu(counts.count_katu as u32)
            .n100(counts.count_100 as u32)
            .n50(counts.count_50 as u32)
            .misses(miss_override)
            .calculate()
            .pp()
    };

    let if_fc = if let (GameMode::Mania, Some(s)) = (params.mode, params.statistics) {
        let fc_counts = ScoreStatistics {
            count_geki: s.count_geki,
            count_300: s.count_300 + s.count_miss,
            count_katu: s.count_katu,
            count_100: s.count_100,
            count_50: s.count_50,
            count_miss: 0,
        };
        calc_mania_fc(&fc_counts, 0)
    } else {
        calc_pp(params.accuracy, bm_combo, 0)
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
    })
}

/// Enrich a score with PP breakdown and if-acc values.
/// Downloads the .osu file if needed, calculates PP decomposition,
/// and sets the `pp_breakdown` and `pp_if_acc` fields on the score.
pub async fn enrich_score_with_pp(score: &mut Score, mode: GameMode) {
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
        });
        let if_acc = calculate_pp_if_acc(
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
            },
            beatmap_max_combo,
        );
        (breakdown, if_acc)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(error = ?e, "PP calculation task panicked");
        (None, None)
    });

    score.pp_breakdown = pp_breakdown;
    score.pp_if_acc = pp_if_acc;

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

#[derive(Debug, serde::Deserialize)]
struct OsuApiScore {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    total_score: Option<i64>,
    #[serde(default)]
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
    #[serde(default)]
    perfect: bool,
    #[serde(default, alias = "created_at")]
    ended_at: String,
    #[serde(default)]
    has_replay: bool,
    #[serde(default)]
    legacy_score_id: Option<i64>,
    beatmap: OsuApiBeatmap,
    beatmapset: OsuApiBeatmapset,
    #[serde(default)]
    mods: Vec<OsuApiMod>,
    #[serde(default)]
    statistics: OsuApiScoreStatistics,
    #[serde(default)]
    user: Option<serde_json::Value>,
}

/// lazer: perfect/great/ok/meh/miss, legacy: count_geki/count_300/count_katu/count_100/count_50/count_miss
#[derive(Debug, serde::Deserialize, Default)]
struct OsuApiScoreStatistics {
    #[serde(default, alias = "perfect")]
    count_geki: i64,
    #[serde(default, alias = "great")]
    count_300: i64,
    #[serde(default)]
    count_katu: i64,
    #[serde(default, alias = "meh")]
    count_100: i64,
    #[serde(default)]
    count_50: i64,
    #[serde(default, alias = "miss")]
    count_miss: i64,
    /// Lazer `ok` field — maps to count_100 (standard) or count_katu (mania)
    #[serde(default)]
    ok: i64,
}

#[derive(Debug, serde::Deserialize)]
struct OsuApiScoreUser {
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

fn api_score_to_score(api: OsuApiScore, mode: GameMode) -> Score {
    let beatmap = api.beatmap;
    let beatmapset = api.beatmapset;

    let cover_url = beatmapset
        .covers
        .as_ref()
        .and_then(|v| v.get("cover")?.as_str().map(|s| s.to_string()))
        .unwrap_or_default();

    let score_value = if api.score > 0 {
        api.score
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
                global_rank: u.statistics.as_ref().and_then(|s| s.global_rank),
                country_rank: u.statistics.as_ref().and_then(|s| s.country_rank),
                pp: u.statistics.as_ref().and_then(|s| s.pp).unwrap_or(0.0),
            })
        })
        .unwrap_or(ScoreUser {
            avatar_url: String::new(),
            country_code: String::new(),
            global_rank: None,
            country_rank: None,
            pp: 0.0,
        });

    Score {
        score_id: api.id,
        beatmap_id: beatmap.id,
        beatmapset_id: beatmap.beatmapset_id,
        artist: beatmapset.artist,
        title: beatmapset.title,
        version: beatmap.version,
        creator: beatmapset.creator,
        star_rating: beatmap.difficulty_rating,
        bpm: beatmap.bpm,
        ar: beatmap.ar,
        od: beatmap.od,
        cs: beatmap.cs,
        hp: beatmap.hp,
        length_seconds: beatmap.total_length,
        score_value,
        accuracy: api.accuracy,
        max_combo: api.max_combo,
        beatmap_max_combo: beatmap.max_combo,
        pp: api.pp,
        pp_breakdown: None,
        pp_if_acc: None,
        rank: api.rank,
        mods: api_mods_to_game_mods(&api.mods, mode),
        is_perfect: api.perfect,
        created_at: api.ended_at,
        // 三种方式检测 lazer 模式：
        // 1. API 显式标记 is_lazer = true
        // 2. build_id > 0 表示使用了 lazer 客户端
        // 3. legacy_total_score == 0 表示该分数在 lazer 中创建，无旧版分数
        is_lazer: api.is_lazer
            || api.build_id.is_some_and(|id| id > 0)
            || api.legacy_total_score.is_some_and(|v| v == 0),
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
                0
            },
            count_100: if mode == GameMode::Mania {
                api.statistics.count_100
            } else if api.statistics.ok != 0 {
                api.statistics.ok
            } else {
                api.statistics.count_100
            },
            count_50: api.statistics.count_50,
            count_miss: api.statistics.count_miss,
        },
        cover_url,
        user,
        fav_count: Some(beatmapset.favourite_count).filter(|&v| v > 0),
        play_count: Some(beatmapset.play_count).filter(|&v| v > 0),
        status: beatmap.status,
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

/// 区分 JSON 反序列化错误（非瞬态）与 body 读取网络错误（瞬态）
fn json_to_api_error(e: reqwest::Error) -> ApiError {
    if e.is_decode() {
        ApiError::Deserialization(e.to_string())
    } else {
        ApiError::Http(e)
    }
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
    client_id: String,
    client_secret: String,
    cache: Mutex<Option<(String, Instant)>>,
    refresh_lock: Mutex<()>,
    refresh_interval: Duration,
}

impl OauthTokenCache {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
            refresh_interval: Duration::from_secs(20 * 3600),
        }
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
        let params = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
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

        let token_data: OauthResponse = resp.json().await.map_err(json_to_api_error)?;

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
    use rand::Rng;
    let base_delay = Duration::from_secs(1);
    let exp = base_delay * 2u32.pow(attempt);
    let exp_ms = exp.as_millis() as u64;
    // 75%~125% 范围：先算 75% 基准，再加 0~50% 随机偏移
    let min_ms = exp_ms * 3 / 4;
    let range_ms = exp_ms / 2;
    let jitter_ms = if range_ms > 0 {
        rand::thread_rng().gen_range(0..=range_ms)
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
    assert!(
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
    assert!(
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

            let data: OsuApiV2User = resp.json().await.map_err(json_to_api_error)?;

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
    // 回填谱面 od/hp/max_combo
    if ((score.od == 0.0 && score.hp == 0.0) || score.beatmap_max_combo == 0)
        && score.beatmap_id > 0
    {
        match fetch_beatmap(rate_limiter, oauth, score.beatmap_id).await {
            Ok(bm) => {
                if score.od == 0.0 && score.hp == 0.0 {
                    score.od = bm.od;
                    score.hp = bm.hp;
                }
                if score.beatmap_max_combo == 0 {
                    score.beatmap_max_combo = bm.max_combo;
                }
                tracing::debug!(
                    beatmap_id = score.beatmap_id,
                    od = bm.od,
                    hp = bm.hp,
                    max_combo = bm.max_combo,
                    "Backfilled beatmap stats"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = ?e,
                    beatmap_id = score.beatmap_id,
                    "Failed to backfill beatmap stats"
                );
            }
        }
    }

    // 回填 lazer 分数值
    if score.score_value == 0 && score.score_id > 0 {
        match fetch_score_detail(rate_limiter, oauth, mode_str, score.score_id).await {
            Ok(Some(val)) => {
                score.score_value = val;
                tracing::debug!(
                    score_id = score.score_id,
                    score_value = val,
                    "Backfilled score value from detail endpoint"
                );
            }
            Ok(None) => {
                tracing::debug!(
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

            let raw_json: serde_json::Value = resp.json().await.map_err(json_to_api_error)?;

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

/// 通过 score ID 获取单条成绩详情（用于 lazer 分数值回填）
async fn fetch_score_detail(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    ruleset: &str,
    score_id: i64,
) -> Result<Option<i64>, ApiError> {
    let client = http_client();
    let url = format!("https://osu.ppy.sh/api/v2/scores/{}/{}", ruleset, score_id);
    tracing::debug!(url = %url, "Fetching score detail");

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

            let data: serde_json::Value = resp.json().await.map_err(json_to_api_error)?;
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

            resp.json().await.map_err(json_to_api_error)
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

            let user: OsuUserInfo = resp.json().await.map_err(json_to_api_error)?;
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

            let data: OsuProfileResponse = resp.json().await.map_err(json_to_api_error)?;

            Ok(UserProfile {
                html: data.page.html,
                profile_hue: data.profile_hue.unwrap_or(333),
                username: data.username,
                avatar_url: data.avatar_url,
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
            perfect: false,
            ended_at: "2024-01-01T00:00:00Z".to_string(),
            is_lazer: false,
            build_id: None,
            has_replay: true,
            legacy_score_id: None,
            beatmap: OsuApiBeatmap {
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
            },
            beatmapset: OsuApiBeatmapset {
                artist: "TestArtist".to_string(),
                title: "TestTitle".to_string(),
                creator: "Mapper".to_string(),
                covers: None,
                favourite_count: 100,
                play_count: 5000,
            },
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
                ok: 0,
            },
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
        assert!((score.accuracy - 0.9876).abs() < 0.0001);
        assert_eq!(score.max_combo, 543);
        assert_eq!(score.beatmap_max_combo, 800);
        assert_eq!(score.pp, Some(300.5));
        assert_eq!(score.rank, "S");
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
        api.beatmapset.covers = Some(serde_json::json!({
            "cover": "https://example.com/cover.jpg"
        }));
        api.user = Some(serde_json::json!({
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
        assert!(score.is_lazer);
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
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let oauth = OauthTokenCache::new("test".to_string(), "test".to_string());
            let _ = retry_on_401(&oauth, 31, || async { Ok(42) }).await;
        });
    }

    #[test]
    #[should_panic(expected = "max_retries must be <= 30")]
    fn test_retry_on_transient_rejects_large_max_retries() {
        let rt = tokio::runtime::Runtime::new().unwrap();
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
