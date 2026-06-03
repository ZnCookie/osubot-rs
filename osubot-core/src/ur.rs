use crate::api::{ApiError, OauthTokenCache, API_VERSION};
use crate::rate_limiter::RateLimiter;
use crate::types::GameMode;
use rosu_mods::GameMods;
use rosu_pp::model::beatmap::BeatmapAttributesBuilder;
use rosu_pp::Beatmap;
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

/// 解析 replay 获取按键时间点
fn parse_replay_key_times(osr_bytes: &[u8]) -> Option<(Vec<f64>, f64)> {
    use rosu_replay::Replay;

    let replay = Replay::from_bytes(osr_bytes).ok()?;

    tracing::debug!(
        replay_data_len = replay.replay_data.len(),
        mode = ?replay.mode,
        mods = replay.mods.value(),
        replay_300 = replay.count_300,
        replay_100 = replay.count_100,
        replay_50 = replay.count_50,
        replay_miss = replay.count_miss,
        "Replay parsed"
    );

    let result = extract_key_times_and_offset(&replay.replay_data);

    if let Some((ref key_times, time_offset)) = result {
        tracing::debug!(
            key_times_count = key_times.len(),
            time_offset,
            "Key times extracted from replay"
        );
    }

    result
}

/// 从 replay 事件流提取按键时间点与 replay→beatmap 时序偏移。
///
/// 第一帧若 delta 异常（不在 0..=10000），视为 replay 与谱面的时间偏移并跳过累加。
/// 按键按下通过状态从 0 变为非 0 检测，按住不重复计数。
/// key_times 为空时返回 None。
fn extract_key_times_and_offset(events: &[rosu_replay::ReplayEvent]) -> Option<(Vec<f64>, f64)> {
    use rosu_replay::ReplayEvent;

    let mut cumulative_time = 0.0f64;
    let mut key_times = Vec::new();
    let mut prev_keys = 0u32;
    let mut first_frame = true;
    let mut time_offset = 0.0f64;

    for event in events.iter() {
        let delta = event.time_delta();

        if first_frame {
            first_frame = false;
            if !(0..=10000).contains(&delta) {
                // abnormal first delta represents replay→beatmap time offset
                time_offset = (delta as f64).abs();
                continue;
            }
        }

        cumulative_time += delta as f64;

        // 只处理 osu! standard 的 events
        if let ReplayEvent::Osu(e) = event {
            let current_keys = e.keys.value();
            // 检测按键按下（状态从 0 变为非 0）
            let just_pressed = current_keys & !prev_keys;
            if just_pressed != 0 {
                key_times.push(cumulative_time);
            }
            prev_keys = current_keys;
        }
    }

    if key_times.is_empty() {
        return None;
    }

    Some((key_times, time_offset))
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
        let start = match key_times.binary_search_by(|&k| k.partial_cmp(&lo).unwrap()) {
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

    tracing::debug!(
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
        tracing::debug!(mode = ?mode, "Skipping UR/PP calculation: not osu! mode");
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

    fn osu_event(time_delta: i32, keys: u32) -> rosu_replay::ReplayEvent {
        rosu_replay::ReplayEvent::Osu(rosu_replay::ReplayEventOsu {
            time_delta,
            x: 0.0,
            y: 0.0,
            keys: rosu_replay::Key(keys),
        })
    }

    const K1: u32 = 1 << 2; // rosu_replay::Key::K1

    #[test]
    fn extract_normal_no_offset() {
        // first delta 50 is in range → no offset; presses on frames 1 and 3
        let events = [
            osu_event(50, 0),
            osu_event(50, K1),
            osu_event(50, 0),
            osu_event(50, K1),
        ];
        let (key_times, offset) = extract_key_times_and_offset(&events).unwrap();
        assert_eq!(offset, 0.0);
        assert_eq!(key_times, vec![100.0, 200.0]);
    }

    #[test]
    fn extract_abnormal_first_delta_is_offset() {
        // first delta -5000 is out of range → offset 5000, frame skipped (no accumulate)
        let events = [
            osu_event(-5000, 0),
            osu_event(50, K1),
            osu_event(50, 0),
            osu_event(50, K1),
        ];
        let (key_times, offset) = extract_key_times_and_offset(&events).unwrap();
        assert_eq!(offset, 5000.0);
        assert_eq!(key_times, vec![50.0, 150.0]);
    }

    #[test]
    fn extract_held_key_counts_once() {
        // K1 held across two frames → a single press at the first frame
        let events = [osu_event(50, K1), osu_event(50, K1), osu_event(50, 0)];
        let (key_times, offset) = extract_key_times_and_offset(&events).unwrap();
        assert_eq!(offset, 0.0);
        assert_eq!(key_times, vec![50.0]);
    }

    #[test]
    fn extract_no_presses_returns_none() {
        let events = [osu_event(50, 0), osu_event(50, 0)];
        assert!(extract_key_times_and_offset(&events).is_none());
    }

    #[test]
    fn extract_large_first_delta_over_threshold_is_offset() {
        // boundary: delta 10001 > 10000 → treated as offset
        let events = [osu_event(10001, 0), osu_event(50, K1)];
        let (key_times, offset) = extract_key_times_and_offset(&events).unwrap();
        assert_eq!(offset, 10001.0);
        assert_eq!(key_times, vec![50.0]);
    }
}
