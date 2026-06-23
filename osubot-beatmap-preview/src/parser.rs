// Copyright (c) 2026 xuan_yuan (from osu-beatmap-preview, MIT licensed)
// Copyright (c) 2026 ZnCookie
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use crate::errors::{PreviewError, Result};
use crate::models::*;
use std::collections::HashMap;

/// 单文件最大 50 MB，防止恶意大文件触发 OOM。
const MAX_BEATMAP_BYTES: usize = 50 * 1024 * 1024;
/// 最多 100k hit object / timing point，覆盖真实谱面 5–10 倍。
const MAX_HIT_OBJECTS: usize = 100_000;
const MAX_TIMING_POINTS: usize = 100_000;

pub fn parse_beatmap_from_bytes(bytes: &[u8]) -> Result<Beatmap> {
    if bytes.len() > MAX_BEATMAP_BYTES {
        return Err(PreviewError::new(format!(
            "beatmap: input size {} exceeds maximum {} bytes",
            bytes.len(),
            MAX_BEATMAP_BYTES
        )));
    }
    let content = String::from_utf8_lossy(bytes);
    let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
    parse_beatmap_str(content)
}

fn parse_beatmap_str(content: &str) -> Result<Beatmap> {
    let sections = split_sections(content);

    if let Some(tp_lines) = sections.get("TimingPoints") {
        if tp_lines.len() > MAX_TIMING_POINTS {
            return Err(PreviewError::new(format!(
                "beatmap: too many timing points ({} > {})",
                tp_lines.len(),
                MAX_TIMING_POINTS
            )));
        }
    }
    if let Some(ho_lines) = sections.get("HitObjects") {
        if ho_lines.len() > MAX_HIT_OBJECTS {
            return Err(PreviewError::new(format!(
                "beatmap: too many hit objects ({} > {})",
                ho_lines.len(),
                MAX_HIT_OBJECTS
            )));
        }
    }

    let metadata = match sections.get("Metadata") {
        Some(lines) => parse_key_value(lines),
        None => return Err(PreviewError::new("beatmap: missing [Metadata] section")),
    };
    let difficulty = parse_key_value(
        sections
            .get("Difficulty")
            .ok_or_else(|| PreviewError::new("beatmap: missing [Difficulty] section"))?,
    );
    let mut general = match sections.get("General") {
        Some(lines) => parse_key_value(lines),
        None => {
            let mut kv = KvSection::default();
            kv.insert("Mode", "0".to_string());
            kv
        }
    };
    general.insert("FormatVersion", parse_format_version(content).to_string());
    let timing_points = parse_timing_points(
        sections
            .get("TimingPoints")
            .ok_or_else(|| PreviewError::new("beatmap: missing [TimingPoints] section"))?,
    )
    .ok_or_else(|| PreviewError::new("beatmap: no valid timing points"))?;
    let break_periods = parse_break_periods(sections.get("Events"));
    let mode: i32 = general
        .get("Mode")
        .unwrap_or("0")
        .parse()
        .map_err(|_| PreviewError::new("beatmap: invalid mode value"))?;

    let combo_colors = parse_combo_colors(sections.get("Colours"));

    let hit_lines = sections
        .get("HitObjects")
        .ok_or_else(|| PreviewError::new("beatmap: missing [HitObjects] section"))?;
    let hit_objects = match mode {
        0 => HitObjects::Standard(
            parse_standard(hit_lines, &difficulty, &timing_points).ok_or_else(|| {
                PreviewError::new("beatmap: failed to parse standard hit objects")
            })?,
        ),
        1 => HitObjects::Taiko(
            parse_taiko(hit_lines, &difficulty, &timing_points)
                .ok_or_else(|| PreviewError::new("beatmap: failed to parse taiko hit objects"))?,
        ),
        2 => HitObjects::Catch(
            parse_catch(hit_lines, &difficulty, &timing_points)
                .ok_or_else(|| PreviewError::new("beatmap: failed to parse catch hit objects"))?,
        ),
        3 => HitObjects::Mania(
            parse_mania(hit_lines, &difficulty)
                .ok_or_else(|| PreviewError::new("beatmap: failed to parse mania hit objects"))?,
        ),
        _ => return Err(PreviewError::new("beatmap: unsupported mode")),
    };

    Ok(Beatmap {
        metadata,
        difficulty,
        general,
        timing_points,
        hit_objects,
        break_periods,
        combo_colors,
    })
}

/// Parse Combo1..ComboN from the [Colours] section, in numeric order.
fn parse_combo_colors(lines: Option<&Vec<&str>>) -> Vec<[u8; 3]> {
    let Some(lines) = lines else {
        return Vec::new();
    };
    let mut entries: Vec<(u32, [u8; 3])> = Vec::new();
    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let Some(num) = key.strip_prefix("Combo") else {
            continue;
        };
        let Ok(index) = num.parse::<u32>() else {
            continue;
        };
        let parts: Vec<&str> = value.split(',').map(|p| p.trim()).collect();
        if parts.len() < 3 {
            continue;
        }
        let (Ok(r), Ok(g), Ok(b)) = (
            parts[0].parse::<u8>(),
            parts[1].parse::<u8>(),
            parts[2].parse::<u8>(),
        ) else {
            continue;
        };
        entries.push((index, [r, g, b]));
    }
    entries.sort_by_key(|e| e.0);
    entries.into_iter().map(|e| e.1).collect()
}

fn split_sections(content: &str) -> HashMap<String, Vec<&str>> {
    let mut sections: HashMap<String, Vec<&str>> = HashMap::new();
    let mut current: Option<String> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].to_string();
            sections.entry(name.clone()).or_default();
            current = Some(name);
            continue;
        }
        if let Some(name) = &current {
            sections.get_mut(name).unwrap().push(line);
        }
    }
    sections
}

fn parse_format_version(content: &str) -> i32 {
    let first = content.lines().next().unwrap_or("").trim();
    if let Some(rest) = first.strip_prefix("osu file format v") {
        if let Ok(v) = rest.parse() {
            return v;
        }
        // 头存在但版本号 parse 失败：显式 warn，避免静默当 v14 处理
        // 走错 format_version 敏感路径（如 catch converter `if version < 8`）
        eprintln!(
            "[osubot-beatmap-preview] WARNING: unparseable osu file format version: {:?}, assuming v14",
            first
        );
        return 14;
    }
    // 没有 osu file format v... 头：v14+ 必有，缺失则非 .osu 文件或格式损坏
    eprintln!(
        "[osubot-beatmap-preview] WARNING: missing 'osu file format v...' header, assuming v14"
    );
    14
}

fn parse_key_value(lines: &[&str]) -> KvSection {
    let mut kv = KvSection::default();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            kv.insert(key.trim(), value.trim().to_string());
        }
    }
    kv
}

fn parse_timing_points(lines: &[&str]) -> Option<Vec<TimingPoint>> {
    let mut points: Vec<TimingPoint> = Vec::new();
    for line in lines {
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        if parts.len() < 2 {
            continue;
        }
        let time: f64 = match parts[0].parse::<f64>() {
            Ok(v) if v.is_finite() => v,
            _ => continue,
        };
        let beat_length: f64 = match parts[1].parse::<f64>() {
            Ok(v) if v.is_finite() && v > 0.0 => v,
            _ => continue,
        };
        let mut meter = if parts.len() > 2 && !parts[2].is_empty() {
            parts[2].parse::<i32>().unwrap_or(4)
        } else {
            4
        };
        if meter <= 0 {
            meter = 4;
        }
        let uninherited = parts.len() < 7 || parts[6] == "1";
        let effects = if parts.len() > 7 && !parts[7].is_empty() {
            parts[7].parse::<i32>().unwrap_or(0)
        } else {
            0
        };
        points.push(TimingPoint {
            time,
            beat_length,
            meter,
            uninherited,
            kiai_mode: effects & 1 != 0,
        });
    }
    points.sort_by(|a, b| a.time.total_cmp(&b.time));
    if points.is_empty() {
        return None;
    }
    Some(points)
}

fn parse_break_periods(lines: Option<&Vec<&str>>) -> Vec<BreakPeriod> {
    let Some(lines) = lines else {
        return Vec::new();
    };
    let mut breaks = Vec::new();
    for line in lines {
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        if parts.len() < 3 || parts[0] != "2" {
            continue;
        }
        let (Ok(s), Ok(e)) = (parts[1].parse::<f64>(), parts[2].parse::<f64>()) else {
            continue;
        };
        // 过滤 NaN/inf：f64 as i64 静默转 0/i64::MAX 会产生巨大 break period，
        // 下游 subtract_periods 会把整段 playable 区间吃掉
        if !s.is_finite() || !e.is_finite() {
            eprintln!(
                "[osubot-beatmap-preview] WARNING: non-finite break period s={} e={}, skipping",
                s, e
            );
            continue;
        }
        let (start_time, end_time) = (s as i64, e as i64);
        if end_time > start_time {
            breaks.push(BreakPeriod {
                start_time,
                end_time,
            });
        }
    }
    breaks
}

struct SliderFields {
    slider_type: String,
    points: Vec<(i32, i32)>,
    repeats: i32,
    pixel_length: f64,
    edge_hitsounds: Vec<i32>,
}

fn parse_slider_fields(parts: &[&str]) -> Option<SliderFields> {
    if parts.len() < 6 {
        return None;
    }
    let mut slider_parts = parts[5].split('|');
    let slider_type = slider_parts.next()?.to_string();
    let mut points = Vec::new();
    for p in slider_parts {
        let (x, y) = p.split_once(':')?;
        points.push((x.parse().ok()?, y.parse().ok()?));
    }
    let raw_repeats: i32 = parts.get(6)?.parse().ok()?;
    let repeats = raw_repeats.clamp(1, 9000);
    let pixel_length: f64 = parts.get(7)?.parse().ok()?;
    if !pixel_length.is_finite() || !(0.0..=1e6).contains(&pixel_length) {
        return None;
    }
    let mut edge_hitsounds = Vec::new();
    if let Some(eh) = parts.get(8) {
        if !eh.is_empty() {
            for v in eh.split('|') {
                if !v.is_empty() {
                    if let Ok(val) = v.parse() {
                        edge_hitsounds.push(val);
                    }
                }
            }
        }
    }
    Some(SliderFields {
        slider_type,
        points,
        repeats,
        pixel_length,
        edge_hitsounds,
    })
}

fn parse_standard(
    lines: &[&str],
    difficulty: &KvSection,
    timing_points: &[TimingPoint],
) -> Option<Vec<StandardHitObject>> {
    let mut objects = Vec::with_capacity(lines.len());
    for line in lines {
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        if parts.len() < 5 {
            continue;
        }
        let x: i32 = match parts[0].parse::<f64>() {
            Ok(v) => v as i32,
            Err(_) => continue,
        };
        let y: i32 = match parts[1].parse::<f64>() {
            Ok(v) => v as i32,
            Err(_) => continue,
        };
        let start_time: i64 = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let hit_type: i32 = match parts[3].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let hitsound: i32 = match parts[4].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let end_time = match parse_end_time(&parts, start_time, hit_type, difficulty, timing_points)
        {
            Some(v) => v,
            None => continue,
        };

        let mut obj = StandardHitObject {
            x,
            y,
            start_time,
            end_time,
            hit_type,
            hitsound,
            new_combo: hit_type & 4 != 0,
            combo_offset: (hit_type & 112) >> 4,
            ..Default::default()
        };
        if hit_type & 2 != 0 {
            let sf = match parse_slider_fields(&parts) {
                Some(v) => v,
                None => continue,
            };
            obj.slider_type = Some(sf.slider_type);
            obj.slider_points = sf.points;
            obj.slider_repeats = sf.repeats;
            obj.slider_pixel_length = sf.pixel_length;
            obj.slider_edge_hitsounds = sf.edge_hitsounds;
        }
        objects.push(obj);
    }
    objects.sort_by_key(|o| (o.start_time, o.end_time));
    Some(objects)
}

fn parse_taiko(
    lines: &[&str],
    difficulty: &KvSection,
    timing_points: &[TimingPoint],
) -> Option<Vec<TaikoHitObject>> {
    let mut objects = Vec::with_capacity(lines.len());
    for line in lines {
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        if parts.len() < 5 {
            continue;
        }
        let start_time: i64 = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let hit_type: i32 = match parts[3].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let hitsound: i32 = match parts[4].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let end_time = match parse_end_time(&parts, start_time, hit_type, difficulty, timing_points)
        {
            Some(v) => v,
            None => continue,
        };
        objects.push(TaikoHitObject {
            start_time,
            end_time,
            hit_type,
            hitsound,
        });
    }
    objects.sort_by_key(|o| (o.start_time, o.end_time));
    Some(objects)
}

fn parse_catch(
    lines: &[&str],
    difficulty: &KvSection,
    timing_points: &[TimingPoint],
) -> Option<Vec<CatchHitObject>> {
    let mut objects = Vec::with_capacity(lines.len());
    for line in lines {
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        if parts.len() < 5 {
            continue;
        }
        let x: i32 = match parts[0].parse::<f64>() {
            Ok(v) => v as i32,
            Err(_) => continue,
        };
        let y: i32 = match parts[1].parse::<f64>() {
            Ok(v) => v as i32,
            Err(_) => continue,
        };
        let start_time: i64 = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let hit_type: i32 = match parts[3].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let end_time = match parse_end_time(&parts, start_time, hit_type, difficulty, timing_points)
        {
            Some(v) => v,
            None => continue,
        };

        let mut obj = CatchHitObject {
            x,
            y,
            start_time,
            end_time,
            hit_type,
            new_combo: hit_type & 4 != 0,
            combo_offset: (hit_type & 112) >> 4,
            slider_type: None,
            slider_points: Vec::new(),
            slider_repeats: 1,
            slider_pixel_length: 0.0,
        };
        if hit_type & 2 != 0 {
            let sf = match parse_slider_fields(&parts) {
                Some(v) => v,
                None => continue,
            };
            obj.slider_type = Some(sf.slider_type);
            obj.slider_points = sf.points;
            obj.slider_repeats = sf.repeats;
            obj.slider_pixel_length = sf.pixel_length;
        }
        objects.push(obj);
    }
    objects.sort_by_key(|o| (o.start_time, o.end_time));
    Some(objects)
}

fn parse_mania(lines: &[&str], difficulty: &KvSection) -> Option<Vec<ManiaHitObject>> {
    let raw_key_count = difficulty.get_f64("CircleSize")?;
    // 显式拒绝 < 1.0 与 NaN：clamp(1.0, 18.0) 对 < 1.0 会静默修复为 1.0（错误地接受
    // 无效谱面），对 NaN 透明返回 NaN。无效 CircleSize 必须报错而非自动修复。
    if !raw_key_count.is_finite() || raw_key_count < 1.0 {
        return None;
    }
    let key_count = raw_key_count.clamp(1.0, 18.0) as i64;
    let mut objects = Vec::with_capacity(lines.len());
    for line in lines {
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        if parts.len() < 5 {
            continue;
        }
        let x: i64 = match parts[0].parse::<f64>() {
            Ok(v) => v as i64,
            Err(_) => continue,
        };
        let start_time: i64 = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let hit_type: i32 = match parts[3].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let lane = (x * key_count).div_euclid(512).clamp(0, key_count - 1) as i32;
        let is_long_note = hit_type & 128 != 0;
        let mut end_time = start_time;
        if is_long_note {
            let head = match parts.get(5).and_then(|s| s.split(':').next()) {
                Some(v) => v,
                None => continue,
            };
            end_time = match head.parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
        }
        objects.push(ManiaHitObject {
            lane,
            start_time,
            end_time,
            is_long_note,
        });
    }
    objects.sort_by_key(|o| (o.start_time, o.end_time));
    Some(objects)
}

fn parse_end_time(
    parts: &[&str],
    start_time: i64,
    hit_type: i32,
    difficulty: &KvSection,
    timing_points: &[TimingPoint],
) -> Option<i64> {
    if hit_type & 8 != 0 {
        return parts.get(5)?.parse::<f64>().ok().map(|v| v as i64);
    }
    if hit_type & 2 != 0 {
        return parse_slider_end_time(parts, start_time, difficulty, timing_points);
    }
    Some(start_time)
}

fn parse_slider_end_time(
    parts: &[&str],
    start_time: i64,
    difficulty: &KvSection,
    timing_points: &[TimingPoint],
) -> Option<i64> {
    let slides: f64 = parts.get(6)?.parse::<i32>().ok()? as f64;
    let pixel_length: f64 = parts.get(7)?.parse().ok()?;
    let slider_multiplier = match difficulty.get_f64("SliderMultiplier") {
        Some(v) if v > 0.0 => v,
        Some(v) => {
            eprintln!(
                "[osubot-beatmap-preview] WARNING: invalid SliderMultiplier={}, skipping slider",
                v
            );
            return Some(start_time);
        }
        None => {
            eprintln!(
                "[osubot-beatmap-preview] WARNING: missing [Difficulty] SliderMultiplier, skipping slider"
            );
            return Some(start_time);
        }
    };
    // 上面 match 已保证 slider_multiplier > 0
    let (beat_length, slider_velocity) = resolve_slider_timing(start_time, timing_points);
    let denominator = slider_multiplier * 100.0 * slider_velocity;
    if denominator <= 0.0 {
        return Some(start_time);
    }
    let duration = pixel_length / denominator * beat_length * slides;
    if !duration.is_finite() {
        return Some(start_time);
    }
    let end = start_time.saturating_add(round_half_even(duration));
    Some(if end < start_time { start_time } else { end })
}

pub fn resolve_slider_timing(start_time: i64, timing_points: &[TimingPoint]) -> (f64, f64) {
    let mut beat_length = if let Some(first) = timing_points.first() {
        first.beat_length
    } else {
        return (500.0, 1.0);
    };
    let mut slider_velocity = 1.0;
    for point in timing_points {
        if point.time > start_time as f64 {
            break;
        }
        if point.uninherited {
            beat_length = point.beat_length;
            slider_velocity = 1.0;
        } else if point.beat_length < 0.0 {
            slider_velocity = -100.0 / point.beat_length;
        }
    }
    (beat_length, slider_velocity)
}

// Python's round() = banker's rounding.
pub fn round_half_even(v: f64) -> i64 {
    if !v.is_finite() {
        return 0;
    }
    let floor = v.floor();
    let diff = v - floor;
    if diff > 0.5 {
        floor as i64 + 1
    } else if diff < 0.5 {
        floor as i64
    } else {
        let f = floor as i64;
        if f % 2 == 0 {
            f
        } else {
            f + 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_content() {
        let result = parse_beatmap_str("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_missing_metadata() {
        let content =
            "[Difficulty]\n\n[TimingPoints]\n0,500,4,0,0,100,1,0\n\n[HitObjects]\n256,192,1000,1,0";
        let result = parse_beatmap_str(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("[Metadata]"));
    }

    #[test]
    fn test_parse_missing_difficulty() {
        let content =
            "[Metadata]\nTitle:Test\n\n[TimingPoints]\n0,500,4,0,0,100,1,0\n\n[HitObjects]\n256,192,1000,1,0";
        let result = parse_beatmap_str(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().message().contains("[Difficulty]"));
    }

    #[test]
    fn test_parse_bom_header() {
        let content = "\u{feff}osu file format v14\n\n[Metadata]\nTitle:Test\n\n[Difficulty]\n\n[TimingPoints]\n0,500,4,0,0,100,1,0\n\n[HitObjects]\n256,192,1000,1,0";
        let result = parse_beatmap_from_bytes(content.as_bytes());
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_valid_content() {
        let content = "osu file format v14\n\n[Metadata]\nTitle:Test\n\n[Difficulty]\n\n[TimingPoints]\n0,500,4,0,0,100,1,0\n\n[HitObjects]\n256,192,1000,1,0";
        let result = parse_beatmap_str(content);
        assert!(result.is_ok());
        let beatmap = result.unwrap();
        assert_eq!(beatmap.metadata.get("Title"), Some("Test"));
    }

    /// 恶意谱面：slider_pixel_length = "NaN" / "inf" / "-1" / "1e30" / 正常值。
    /// 期望：非有限 / 负数 / 超过 1e6 的 pixel_length 不能传播到 `obj.slider_pixel_length`。
    /// 字段位置：parts[7] 是 pixelLength，所以 raw 放在第 7 段。
    fn parse_one_slider_pixel_length(raw: &str) -> Option<f64> {
        let line = format!("256,192,1000,2,0,B|0:0|0:1,1,{},0:0:0:0:", raw);
        let kvs = KvSection::default();
        let timing = vec![TimingPoint {
            time: 0.0,
            beat_length: 500.0,
            meter: 4,
            uninherited: true,
            kiai_mode: false,
        }];
        let lines = vec![line.as_str()];
        let objs = parse_standard(&lines, &kvs, &timing)?;
        let obj = objs.into_iter().next()?;
        if obj.hit_type & 2 == 0 {
            return None;
        }
        Some(obj.slider_pixel_length)
    }

    #[test]
    fn slider_pixel_length_rejects_nan() {
        assert_eq!(parse_one_slider_pixel_length("NaN"), None);
    }

    #[test]
    fn slider_pixel_length_rejects_inf() {
        assert_eq!(parse_one_slider_pixel_length("inf"), None);
    }

    #[test]
    fn slider_pixel_length_rejects_negative() {
        assert_eq!(parse_one_slider_pixel_length("-1"), None);
    }

    #[test]
    fn slider_pixel_length_rejects_huge() {
        // 1e30 -> saturate 或 reject；本测试断言"未传播到 obj"
        let out = parse_one_slider_pixel_length("1e30");
        assert!(out.is_none() || out.unwrap() <= 1e6, "got {out:?}");
    }

    #[test]
    fn slider_pixel_length_accepts_normal() {
        let out = parse_one_slider_pixel_length("100").expect("normal");
        assert!((out - 100.0).abs() < 1e-9);
    }

    #[test]
    fn timing_point_rejects_nan_beat_length() {
        let lines = vec!["0,NaN,4,1,0,100,1,0"];
        let points = parse_timing_points(&lines);
        if let Some(pts) = points {
            for p in &pts {
                assert!(p.beat_length.is_finite(), "got {:?}", p);
            }
        }
    }

    #[test]
    fn timing_point_rejects_inf_time() {
        let lines = vec!["inf,500,4,1,0,100,1,0"];
        let points = parse_timing_points(&lines);
        if let Some(pts) = points {
            for p in &pts {
                assert!(p.time.is_finite());
            }
        }
    }

    #[test]
    fn parse_end_time_saturates_on_huge_duration() {
        // 构造使 duration 远超 i64::MAX 的极值场景：
        // beat_length=1e15, slides=1, pixel_length=1e6, SliderMultiplier=0.4
        // duration = 1e6 / (0.4 * 100 * 1) * 1e15 * 1 = 2.5e19 ms  ->  i64 overflow.
        let mut kvs = KvSection::default();
        kvs.insert("SliderMultiplier", "0.4".to_string());
        let timing = vec![TimingPoint {
            time: 0.0,
            beat_length: 1.0e15,
            meter: 4,
            uninherited: true,
            kiai_mode: false,
        }];
        let line = "256,192,1000,2,0,B|0:0|0:1,1,1000000,0:0:0:0:";
        let lines = vec![line];
        let objs = parse_standard(&lines, &kvs, &timing).expect("parse");
        let obj = &objs[0];
        assert!(
            obj.end_time >= obj.start_time,
            "end {} < start {}",
            obj.end_time,
            obj.start_time
        );
        assert!(
            obj.end_time - obj.start_time > 0,
            "duration must be positive, got {}",
            obj.end_time - obj.start_time
        );
    }

    #[test]
    fn parse_slider_fields_clamps_huge_repeats() {
        let line = "256,192,1000,2,0,B|0:0|0:1,2147483647,100,0:0:0:0:";
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        let sf = parse_slider_fields(&parts).expect("valid parts");
        assert!(sf.repeats >= 1 && sf.repeats <= 9000, "got {}", sf.repeats);
    }

    #[test]
    fn parse_slider_fields_rejects_negative_repeats() {
        let line = "256,192,1000,2,0,B|0:0|0:1,-5,100,0:0:0:0:";
        let parts: Vec<&str> = line.split(',').map(|p| p.trim()).collect();
        let sf = parse_slider_fields(&parts).expect("valid parts");
        assert_eq!(sf.repeats, 1);
    }

    fn mania_lane_for(circle_size: &str, x: i64) -> Option<i32> {
        let content = format!(
            "osu file format v14\n\n[Metadata]\nTitle:t\n\n[General]\nMode:3\n\n\
             [Difficulty]\nCircleSize:{circle_size}\n\n[TimingPoints]\n0,500,4,0,0,100,1,0\n\n\
             [HitObjects]\n{x},192,1000,1,0"
        );
        let bm = parse_beatmap_str(&content).ok()?;
        if let HitObjects::Mania(v) = &bm.hit_objects {
            v.first().map(|o| o.lane)
        } else {
            None
        }
    }

    #[test]
    fn parse_mania_clamps_huge_circle_size() {
        let lane = mania_lane_for("1000000000", 200).expect("parse ok");
        assert!((0..=17).contains(&lane), "lane out of [0,17]: {lane}");
    }

    #[test]
    fn parse_mania_rejects_zero_circle_size() {
        assert!(mania_lane_for("0", 200).is_none());
    }

    #[test]
    fn parse_mania_rejects_nan_circle_size() {
        // NaN 必须被显式拒绝：clamp 对 NaN 透明，下游 as i64 = 0 会触发 OOB。
        assert!(mania_lane_for("NaN", 200).is_none());
    }

    #[test]
    fn parse_beatmap_rejects_oversize_input() {
        let mut content = String::from(
            "osu file format v14\n\n[Metadata]\nTitle:t\n\n[Difficulty]\n\n\
             [TimingPoints]\n0,500,4,0,0,100,1,0\n\n[HitObjects]\n",
        );
        content.push_str(&"0,0,0,1,0\n".repeat(60 * 1024 * 1024 / 8));
        let result = parse_beatmap_from_bytes(content.as_bytes());
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.message();
        assert!(
            msg.contains("exceeds") || msg.contains("too large"),
            "got: {msg}"
        );
    }

    fn header() -> &'static str {
        "osu file format v14\n\n[Metadata]\nTitle:t\n\n[Difficulty]\n\n[TimingPoints]\n"
    }

    #[test]
    fn parse_beatmap_rejects_too_many_hit_objects() {
        let mut content = String::from(header());
        content.push_str("0,500,4,0,0,100,1,0\n\n[HitObjects]\n");
        for i in 0..200_000 {
            content.push_str(&format!("0,0,{i},1,0\n"));
        }
        let result = parse_beatmap_from_bytes(content.as_bytes());
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = err.message();
        assert!(
            msg.contains("hit objects") || msg.contains("too many"),
            "got: {msg}"
        );
    }
}
