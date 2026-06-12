use crate::api::{ApiError, OauthTokenCache, API_VERSION};
use crate::rate_limiter::RateLimiter;
use crate::types::GameMode;
use rosu_mods::GameMods;
use rosu_pp::model::beatmap::BeatmapAttributesBuilder;
use rosu_pp::Beatmap;
use std::io::Read;
use std::sync::Arc;

use crate::cache::replay_cache_dir;

async fn download_replay(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    score_id: i64,
    legacy_score_id: Option<i64>,
    mode: GameMode,
) -> Result<bytes::Bytes, ApiError> {
    // 检查缓存（同步 I/O 移到 blocking 线程）
    let effective_id = if score_id == 0 {
        legacy_score_id.unwrap_or(0)
    } else {
        score_id
    };
    if effective_id == 0 {
        return Err(ApiError::InvalidResponse);
    }
    let cache_path = replay_cache_dir().join(format!("{}.osr", effective_id));
    let cached = tokio::task::spawn_blocking({
        let cache_path = cache_path.clone();
        move || -> Option<bytes::Bytes> {
            if !cache_path.exists() {
                return None;
            }
            let meta = std::fs::metadata(&cache_path).ok()?;
            if meta.len() == 0 {
                return None;
            }
            let fresh = if let Ok(modified) = meta.modified() {
                modified.elapsed().unwrap_or(std::time::Duration::MAX)
                    < std::time::Duration::from_secs(7 * 86400)
            } else {
                false
            };
            if fresh {
                std::fs::read(&cache_path).ok().map(bytes::Bytes::from)
            } else {
                None
            }
        }
    })
    .await
    .unwrap_or(None);

    if let Some(bytes) = cached {
        tracing::debug!(score_id, bytes = bytes.len(), "Replay loaded from cache");
        return Ok(bytes);
    }

    let client = crate::api::http_client();
    let url = match legacy_score_id {
        Some(legacy_id) => format!(
            "https://osu.ppy.sh/api/v2/scores/{}/{}/download",
            mode.api_value(),
            legacy_id
        ),
        None => format!("https://osu.ppy.sh/api/v2/scores/{}/download", score_id),
    };

    rate_limiter
        .acquire()
        .await
        .map_err(|_| ApiError::ClientRateLimited)?;
    let bytes = crate::api::retry_on_transient(2, || {
        crate::api::retry_on_401(oauth, 3, || async {
            let access_token = oauth.get_token().await?;

            let resp = client
                .get(&url)
                .header("Authorization", format!("Bearer {}", access_token))
                .header("x-api-version", API_VERSION)
                .send()
                .await?;

            if resp.status() == 404 {
                tracing::debug!(url = %url, status = %resp.status(), "Replay download 404");
                return Err(ApiError::NotFound);
            }
            crate::api::classify_http_error(&resp)?;

            tracing::debug!(url = %url, "Replay download request sent");

            let bytes = resp.bytes().await?;
            tracing::debug!(url = %url, bytes = bytes.len(), "Replay downloaded successfully");
            Ok(bytes)
        })
    })
    .await?;

    // 保存到缓存（同步 I/O 移到 blocking 线程）
    let write_path = cache_path.clone();
    let bytes_clone = bytes.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = std::fs::create_dir_all(write_path.parent().unwrap()) {
            tracing::warn!(error = %e, path = %write_path.parent().unwrap().display(), "failed to create replay cache dir");
        } else if let Err(e) = std::fs::write(&write_path, &bytes_clone) {
            tracing::warn!(error = %e, path = %write_path.display(), "failed to write replay cache");
        }
    })
    .await
    .ok();

    Ok(bytes)
}

/// 官方 osu! hit window 计算（两段线性插值）
/// 返回 (great_300, ok_100, meh_50) 的半窗口宽度（ms）
fn hit_windows(od: f64) -> (f64, f64, f64) {
    // 两段线性插值函数
    let interpolate = |difficulty: f64, min: f64, mid: f64, max: f64| -> f64 {
        if difficulty > 5.0 {
            mid + (max - mid) * (difficulty - 5.0) / 5.0
        } else if difficulty < 5.0 {
            mid + (mid - min) * (difficulty - 5.0) / 5.0
        } else {
            mid
        }
    };

    let great = interpolate(od, 80.0, 50.0, 20.0).floor() - 0.5;
    let ok = interpolate(od, 140.0, 100.0, 60.0).floor() - 0.5;
    let meh = interpolate(od, 200.0, 150.0, 100.0).floor() - 0.5;

    (great, ok, meh)
}

// === osu! .osr binary header helpers ===

/// Skip an osu! binary string at the given position.
/// Returns the new position after the string, or `None` on malformed input.
fn skip_osu_string(data: &[u8], pos: usize) -> Option<usize> {
    if pos >= data.len() {
        return None;
    }
    match data[pos] {
        0x00 => Some(pos + 1),
        0x0b => {
            let mut new_pos = pos + 1;
            let mut length: usize = 0;
            let mut shift = 0;
            loop {
                if new_pos >= data.len() {
                    return None;
                }
                let byte = data[new_pos];
                new_pos += 1;
                length |= ((byte & 0x7f) as usize) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift >= 64 {
                    return None;
                }
            }
            if new_pos + length > data.len() {
                return None;
            }
            Some(new_pos + length)
        }
        _ => None,
    }
}

/// Decompress the LZMA-compressed replay data from a raw `.osr` file
/// and parse all frames into `(absolute_time_ms, x, y, keys)` tuples.
fn extract_raw_replay_frames(osr_bytes: &[u8]) -> Option<Vec<(i32, f32, f32, u32)>> {
    let mut pos = 1usize; // gameMode (1 byte)
    pos += 4; // gameVersion (4 bytes)

    // Skip 3 osu! strings: beatmap hash, player name, replay hash
    for _ in 0..3 {
        pos = skip_osu_string(osr_bytes, pos)?;
    }

    // Skip fixed fields: 6 shorts (hit counts, 12 bytes) + i32 (total score, 4) +
    // short (max combo, 2) + byte (perfect, 1) + i32 (mods, 4) = 23 bytes
    pos += 12 + 4 + 2 + 1 + 4;

    // Skip lifebar string
    pos = skip_osu_string(osr_bytes, pos)?;

    // Skip timestamp (8 bytes)
    pos += 8;

    // Read replay_length (i32 LE)
    if pos + 4 > osr_bytes.len() {
        return None;
    }
    let replay_len = i32::from_le_bytes(osr_bytes[pos..pos + 4].try_into().ok()?) as usize;
    pos += 4;

    // Extract compressed data
    if pos + replay_len > osr_bytes.len() {
        return None;
    }
    let compressed = &osr_bytes[pos..pos + replay_len];

    // LZMA decompress
    let mut buffer = Vec::new();
    liblzma::read::XzDecoder::new_multi_decoder(compressed)
        .read_to_end(&mut buffer)
        .ok()?;

    let data_str = String::from_utf8(buffer).ok()?;

    // Parse CSV frames: `time_delta|x|y|keys`
    let mut frames: Vec<(i32, f32, f32, u32)> = Vec::new();
    let mut cum_time: i32 = 0;

    for s in data_str.trim_end_matches(',').split(',') {
        let parts: Vec<&str> = s.split('|').collect();
        if parts.len() != 4 {
            continue;
        }
        if parts[0] == "-12345" {
            // RNG seed marker — signals end of real replay frames
            continue;
        }

        let delta: i32 = parts[0].parse().ok()?;
        let x: f32 = parts[1].parse().ok()?;
        let y: f32 = parts[2].parse().ok()?;
        let keys: u32 = parts[3].parse().ok()?;

        cum_time += delta;
        frames.push((cum_time, x, y, keys));
    }

    Some(frames)
}

/// Apply osu! lazer's time Correction B and strip lazer marker frames.
///
/// osu! lazer `LegacyScoreDecoder` applies Correction B *before* stripping
/// lazer markers `(256, -500)`, ensuring the first real frame's timing stays
/// consistent. The upstream replay decoder strips markers without this correction,
/// corrupting the first frame's delta. We re-implement both steps here.
///
/// Returns corrected `(delta, keys)` pairs ready for key-time extraction.
fn parse_corrected_replay_deltas(osr_bytes: &[u8]) -> Option<Vec<(i32, u32)>> {
    let mut frames = extract_raw_replay_frames(osr_bytes)?;

    if frames.len() < 3 {
        return None;
    }

    // Correction B (from osu! lazer `LegacyScoreDecoder`):
    //   if frame[0].time > frame[2].time:
    //       frame[0].time = frame[1].time = frame[2].time
    // This aligns lazer marker frames with the first real frame so that
    // stripping them does not break the delta chain.
    if frames[0].0 > frames[2].0 {
        let new_time = frames[2].0;
        frames[0].0 = new_time;
        frames[1].0 = new_time;
    }

    // Strip lazer marker frames at position (256, -500).
    // These are injected by the osu! API / lazer replay encoder and carry
    // no gameplay information.
    while !frames.is_empty() && frames[0].1 == 256.0 && frames[0].2 == -500.0 {
        frames.remove(0);
    }

    if frames.is_empty() {
        return None;
    }

    // Convert absolute times back to deltas for downstream processing
    let mut deltas: Vec<(i32, u32)> = Vec::with_capacity(frames.len());
    for i in 0..frames.len() {
        let delta = if i == 0 {
            frames[i].0 // first frame: delta = its corrected absolute time
        } else {
            frames[i].0 - frames[i - 1].0
        };
        deltas.push((delta, frames[i].3));
    }

    Some(deltas)
}

/// Extract key press times from corrected `(delta, keys)` pairs.
///
/// Uses the same offset detection and rising-edge press detection logic
/// as the original `extract_key_times_and_offset`, but operates on
/// pre-corrected (delta, keys) tuples instead of `ReplayEvent` objects.
fn extract_key_times_from_deltas(deltas_and_keys: &[(i32, u32)]) -> Option<(Vec<f64>, f64)> {
    let mut cumulative_time = 0.0f64;
    let mut key_times = Vec::new();
    let mut prev_keys = 0u32;
    let mut first_frame = true;
    let mut time_offset = 0.0f64;

    for &(delta, keys) in deltas_and_keys {
        if first_frame {
            first_frame = false;
            if !(0..=10000).contains(&delta) {
                // abnormal first delta represents replay→beatmap time offset
                time_offset = (delta as f64).abs();
                continue;
            }
        }

        cumulative_time += delta as f64;

        // Detect rising edge of key presses (M1|M2|K1|K2)
        let just_pressed = keys & !prev_keys;
        if just_pressed != 0 {
            key_times.push(cumulative_time);
        }
        prev_keys = keys;
    }

    if key_times.is_empty() {
        return None;
    }

    Some((key_times, time_offset))
}

/// 解析 replay 获取按键时间点
fn parse_replay_key_times(osr_bytes: &[u8]) -> Option<(Vec<f64>, f64)> {
    let deltas = parse_corrected_replay_deltas(osr_bytes)?;

    tracing::trace!(
        replay_data_len = deltas.len(),
        "Replay parsed with lazer time correction"
    );

    let result = extract_key_times_from_deltas(&deltas);

    if let Some((ref key_times, time_offset)) = result {
        tracing::trace!(
            key_times_count = key_times.len(),
            time_offset,
            "Key times extracted from replay"
        );
    }

    result
}

/// Calculate preempt time (ms) from AR — how long before a note appears
fn preempt_from_ar(ar: f64) -> f64 {
    if ar < 5.0 {
        1200.0 + 600.0 * (5.0 - ar) / 5.0
    } else if ar > 5.0 {
        1200.0 - 750.0 * (ar - 5.0) / 5.0
    } else {
        1200.0
    }
}

/// 匹配按键和 note，返回 (note_time, hit_result, key_time) 对
/// hit_result: 0=miss, 1=50, 2=100, 3=300
///
/// 误差排序贪心：生成所有有效 (note, key) 配对，按误差升序分配，
/// 每个 note 和 key 最多用一次。消除了按键视角贪心中早期不精确按键
/// 抢走 note 导致后期精确按键落空的问题。
fn match_hits(
    key_times: &[f64],
    hit_times: &[f64],
    od: f64,
    ar: f64,
) -> Vec<(f64, i32, Option<f64>)> {
    let (w300, w100, w50) = hit_windows(od);
    let preempt = preempt_from_ar(ar);

    let mut results: Vec<(f64, i32, Option<f64>)> =
        hit_times.iter().map(|&t| (t, 0, None)).collect();

    // 1. 生成所有有效配对 (error, note_idx, key_idx)
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (ni, &nt) in hit_times.iter().enumerate() {
        let lo = nt - w50;
        let hi = nt + w50;
        // 滑动窗口：找到 key_times 中第一个 >= lo 的索引
        let start = match key_times
            .binary_search_by(|&k| k.partial_cmp(&lo).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) | Err(i) => i,
        };
        for (ki, &kt) in key_times.iter().enumerate().skip(start) {
            if kt > hi {
                break;
            }
            if kt < nt - preempt {
                continue;
            }
            let err = (kt - nt).abs();
            pairs.push((err, ni, ki));
        }
    }

    // 2. 按误差升序
    let total_pairs = pairs.len();
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    // 3. 贪心分配
    let mut note_used = vec![false; hit_times.len()];
    let mut key_used = vec![false; key_times.len()];
    let mut assigned = 0usize;
    for (err, ni, ki) in &pairs {
        let err = *err;
        let ni = *ni;
        let ki = *ki;
        if note_used[ni] || key_used[ki] {
            continue;
        }
        note_used[ni] = true;
        key_used[ki] = true;
        assigned += 1;
        let hit_result = if err <= w300 {
            3
        } else if err <= w100 {
            2
        } else {
            1
        };
        results[ni] = (hit_times[ni], hit_result, Some(key_times[ki]));
    }

    tracing::trace!(
        notes = hit_times.len(),
        keys = key_times.len(),
        pairs = total_pairs,
        assigned,
        unmatched_notes = hit_times.len() - assigned,
        unmatched_keys = key_times.len() - assigned,
        "match_hits complete"
    );

    results
}

fn calculate_ur(errors: &[f64]) -> f64 {
    if errors.is_empty() {
        return 0.0;
    }
    let mean = errors.iter().sum::<f64>() / errors.len() as f64;
    // 使用总体方差（除以 N）而非样本方差（除以 N-1），
    // 与社区 UR 计算公式一致。对于大样本量差异可忽略。
    let variance = errors.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / errors.len() as f64;
    variance.sqrt() * 10.0
}

pub struct ScoreUrParams {
    pub score_id: i64,
    pub legacy_score_id: Option<i64>,
    pub beatmap_id: i64,
    pub mode: GameMode,
    pub mods: GameMods,
}

pub async fn calculate_score_ur(
    rate_limiter: &Arc<RateLimiter>,
    oauth: &Arc<OauthTokenCache>,
    params: ScoreUrParams,
) -> Option<f64> {
    let ScoreUrParams {
        score_id,
        legacy_score_id,
        beatmap_id,
        mode,
        mods,
    } = params;
    if mode != GameMode::Osu {
        tracing::trace!(mode = ?mode, "Skipping UR/PP calculation: not osu! mode");
        return None;
    }

    tracing::debug!(
        score_id,
        beatmap_id,
        "Downloading replay for UR/PP calculation"
    );

    // 并行下载 replay 和谱面
    let (osr_result, osu_path_result) = tokio::join!(
        download_replay(rate_limiter, oauth, score_id, legacy_score_id, mode),
        crate::api::download_beatmap_osu(beatmap_id)
    );

    let osr_bytes = match osr_result {
        Ok(bytes) => {
            tracing::debug!(score_id, bytes = bytes.len(), "Replay downloaded");
            bytes
        }
        Err(e) => {
            tracing::warn!(score_id, error = ?e, "Failed to download replay");
            return None;
        }
    };

    let osu_path = match osu_path_result {
        Ok(path) => {
            tracing::debug!(beatmap_id, path = %path.display(), "Beatmap downloaded");
            path
        }
        Err(e) => {
            tracing::warn!(beatmap_id, error = ?e, "Failed to download beatmap");
            return None;
        }
    };
    let game_mods = mods;

    let result = tokio::task::spawn_blocking(move || {
        let map = match Beatmap::from_path(&osu_path) {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!(beatmap_id, error = ?e, "Failed to parse beatmap");
                return None;
            }
        };

        let hit_times: Vec<f64> = map
            .hit_objects
            .iter()
            .filter(|h| h.is_circle() || h.is_slider())
            .map(|h| h.start_time)
            .collect();

        if hit_times.is_empty() {
            tracing::warn!(beatmap_id, "No hit objects in beatmap");
            return None;
        }

        let attrs = BeatmapAttributesBuilder::new()
            .map(&map)
            .mods(rosu_pp::GameMods::from(game_mods.clone()))
            .build();
        let od = attrs.od() as f64;
        let ar = attrs.ar() as f64;
        let gameplay_rate = attrs.clock_rate();

        tracing::trace!(
            beatmap_id,
            count = hit_times.len(),
            od,
            ar,
            gameplay_rate,
            "Parsed beatmap info"
        );

        let (key_times, time_offset) = match parse_replay_key_times(&osr_bytes) {
            Some((times, offset)) => {
                tracing::trace!(
                    score_id,
                    count = times.len(),
                    first_keys = ?times.iter().take(5).collect::<Vec<_>>(),
                    last_key = times.last(),
                    first_hits = ?hit_times.iter().take(5).collect::<Vec<_>>(),
                    last_hit = hit_times.last(),
                    time_offset = offset,
                    "Parsed replay key times"
                );
                (times, offset)
            }
            None => {
                tracing::warn!(score_id, "Failed to parse replay key times");
                return None;
            }
        };

        // replay 的时序可能与谱面有偏移（第一个 frame 的 delta 表示偏移量）
        let hit_times_aligned: Vec<f64> = if time_offset > 0.0 {
            hit_times.iter().map(|t| t + time_offset).collect()
        } else {
            hit_times
        };

        let hits = match_hits(&key_times, &hit_times_aligned, od, ar);
        let hit_counts =
            hits.iter()
                .fold((0, 0, 0, 0), |(c300, c100, c50, cm), &(_, r, _)| match r {
                    3 => (c300 + 1, c100, c50, cm),
                    2 => (c300, c100 + 1, c50, cm),
                    1 => (c300, c100, c50 + 1, cm),
                    _ => (c300, c100, c50, cm + 1),
                });
        tracing::trace!(
            score_id,
            h300 = hit_counts.0,
            h100 = hit_counts.1,
            h50 = hit_counts.2,
            miss = hit_counts.3,
            "Matched hits"
        );

        let timing_errors: Vec<(f64, f64)> = hits
            .iter()
            .filter(|&(_, r, _)| *r > 0)
            .filter_map(|&(hit_time, _, key_time)| {
                key_time.map(|kt| {
                    let raw_offset = kt - hit_time;
                    let normalized_offset = raw_offset / gameplay_rate;
                    (hit_time, normalized_offset)
                })
            })
            .collect();

        tracing::trace!(
            score_id,
            matched = timing_errors.len(),
            "Calculated timing errors"
        );

        if timing_errors.len() >= 5 {
            let first_5: Vec<f64> = timing_errors.iter().take(5).map(|&(_, e)| e).collect();
            tracing::trace!(score_id, first_errors = ?first_5, "First 5 timing errors");
            let mean: f64 =
                timing_errors.iter().map(|&(_, e)| e).sum::<f64>() / timing_errors.len() as f64;
            let variance: f64 = timing_errors
                .iter()
                .map(|&(_, e)| (e - mean).powi(2))
                .sum::<f64>()
                / timing_errors.len() as f64;
            let std_dev = variance.sqrt();
            tracing::trace!(score_id, mean, std_dev, "Timing error statistics");
        }

        if timing_errors.is_empty() {
            tracing::warn!(score_id, "No timing errors, skipping UR calculation");
            return None;
        }

        let all_errors: Vec<f64> = timing_errors.iter().map(|&(_, err)| err).collect();
        let total_ur = calculate_ur(&all_errors);
        tracing::info!(score_id, total_ur, "UR calculation complete");
        Some(total_ur)
    })
    .await
    .unwrap_or_else(|e| {
        tracing::warn!(score_id, error = ?e, "UR/PP calculation panicked");
        None
    });

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    // match_hits signature: (key_times, hit_times, od, ar)
    // od=8.0 → w300≈31.5ms, w100≈75.5ms, w50≈119.5ms
    // ar=9.3 → preempt≈400ms

    #[test]
    fn match_hits_basic_late_hit() {
        let key_times = vec![130.0];
        let hit_times = vec![100.0, 200.0];
        let results = match_hits(&key_times, &hit_times, 8.0, 9.3);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 100.0);
        assert_eq!(results[0].1, 3);
        assert_eq!(results[0].2, Some(130.0));
        assert_eq!(results[1].1, 0);
        assert_eq!(results[1].2, None);
    }

    #[test]
    fn match_hits_basic_early_hit() {
        let key_times = vec![170.0];
        let hit_times = vec![200.0];
        let results = match_hits(&key_times, &hit_times, 8.0, 9.3);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, 200.0);
        assert_eq!(results[0].1, 3);
        assert_eq!(results[0].2, Some(170.0));
    }

    #[test]
    fn match_hits_rejects_key_press_before_visibility() {
        // ar=12.5 (simulates DT on high AR) → preempt=75ms
        // Note at 200ms → visible at 125ms. Key at 120ms → before visibility.
        // Error=80ms < w50(119.5), so without the guard it would match.
        let key_times = vec![120.0];
        let hit_times = vec![200.0];
        let results = match_hits(&key_times, &hit_times, 8.0, 12.5);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, 0); // not hit
        assert_eq!(results[0].2, None);
    }

    #[test]
    fn match_hits_prefers_closer_note() {
        let key_times = vec![120.0];
        let hit_times = vec![100.0, 150.0];
        let results = match_hits(&key_times, &hit_times, 8.0, 9.3);

        assert_eq!(results[0].1, 3);
        assert_eq!(results[0].2, Some(120.0));
        assert_eq!(results[1].1, 0);
    }

    #[test]
    fn hit_windows_standard_od() {
        let (w300, w100, w50) = hit_windows(8.0);
        assert!((w300 - 31.5).abs() < 0.1, "w300={w300}, expected ~31.5");
        assert!((w100 - 75.5).abs() < 0.1, "w100={w100}, expected ~75.5");
        assert!((w50 - 119.5).abs() < 0.1, "w50={w50}, expected ~119.5");
    }

    #[test]
    fn hit_windows_symmetry_at_od5() {
        let (w300, w100, w50) = hit_windows(5.0);
        assert!((w300 - 49.5).abs() < 0.1);
        assert!((w100 - 99.5).abs() < 0.1);
        assert!((w50 - 149.5).abs() < 0.1);
    }

    #[test]
    fn hit_windows_extreme_od() {
        let (w300_low, _, _) = hit_windows(0.0);
        let (w300_high, _, _) = hit_windows(10.0);
        assert!(w300_low > w300_high, "Lower OD should have wider windows");
        assert!(w300_high > 0.0, "Even OD 10 should have positive window");
    }

    #[test]
    fn preempt_from_ar_at_5() {
        assert!((preempt_from_ar(5.0) - 1200.0).abs() < 0.01);
    }

    #[test]
    fn preempt_from_ar_negative_for_extreme_ar() {
        let p = preempt_from_ar(15.0);
        assert!(p < 0.0, "AR 15 preempt should be negative, got {p}");
    }

    #[test]
    fn preempt_from_ar_monotonic() {
        let p_low = preempt_from_ar(0.0);
        let p_mid = preempt_from_ar(5.0);
        let p_high = preempt_from_ar(11.0);
        assert!(p_low > p_mid);
        assert!(p_mid > p_high);
    }

    #[test]
    fn calculate_ur_perfect_returns_zero() {
        let errors = vec![0.0; 100];
        assert!(calculate_ur(&errors) < 0.01);
    }

    #[test]
    fn calculate_ur_with_constant_offset() {
        let errors = vec![10.0; 50];
        assert!(calculate_ur(&errors) < 0.01);
    }

    #[test]
    fn calculate_ur_empty_returns_zero() {
        assert_eq!(calculate_ur(&[]), 0.0);
    }

    #[test]
    fn calculate_ur_with_variance() {
        let errors = vec![-10.0, 0.0, 10.0];
        let ur = calculate_ur(&errors);
        assert!(ur > 0.0, "Non-zero UR expected, got {ur}");
    }

    const K1: u32 = 4; // bit 2 = K1 key press

    #[test]
    fn extract_normal_no_offset() {
        let deltas = [(50, 0), (50, K1), (50, 0), (50, K1)];
        let (key_times, offset) = extract_key_times_from_deltas(&deltas).unwrap();
        assert_eq!(offset, 0.0);
        assert_eq!(key_times, vec![100.0, 200.0]);
    }

    #[test]
    fn extract_abnormal_first_delta_is_offset() {
        let deltas = [(-5000, 0), (50, K1), (50, 0), (50, K1)];
        let (key_times, offset) = extract_key_times_from_deltas(&deltas).unwrap();
        assert_eq!(offset, 5000.0);
        assert_eq!(key_times, vec![50.0, 150.0]);
    }

    #[test]
    fn extract_held_key_counts_once() {
        let deltas = [(50, K1), (50, K1), (50, 0)];
        let (key_times, offset) = extract_key_times_from_deltas(&deltas).unwrap();
        assert_eq!(offset, 0.0);
        assert_eq!(key_times, vec![50.0]);
    }

    #[test]
    fn extract_no_presses_returns_none() {
        let deltas = [(50, 0), (50, 0)];
        assert!(extract_key_times_from_deltas(&deltas).is_none());
    }

    #[test]
    fn extract_large_first_delta_over_threshold_is_offset() {
        let deltas = [(10001, 0), (50, K1)];
        let (key_times, offset) = extract_key_times_from_deltas(&deltas).unwrap();
        assert_eq!(offset, 10001.0);
        assert_eq!(key_times, vec![50.0]);
    }
}
