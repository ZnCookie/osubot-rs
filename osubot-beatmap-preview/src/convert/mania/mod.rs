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

//! standard → mania conversion (mode 3).
//! Ported 1:1 from the Python beatmap_preview mania converter, which itself
//! ports osu!lazer's ManiaBeatmapConverter.
//! RNG call order and float32 round-trip points must match Python exactly.

use crate::errors::{PreviewError, Result};
use crate::legacy_random::LegacyRandom;
use crate::mania::SOURCE_MODE_KEY;
use crate::models::{
    Beatmap, HitObjects, KvSection, ManiaHitObject, StandardHitObject, TimingPoint,
};
use crate::mods::ModSettings;
use crate::parser::round_half_even;

use super::{kiai_at, std_objects};

mod circle;
mod slider;
mod spinner;

use circle::circle_generate;
use slider::slider_generate;
use spinner::spinner_generate;

// pattern type bit flags
const P_FORCE_NOT_STACK: u32 = 1 << 0;
const P_KEEP_SINGLE: u32 = 1 << 1;
const P_MIRROR: u32 = 1 << 2;
const P_GATHERED: u32 = 1 << 3;
const P_STAIR: u32 = 1 << 4;
const P_REVERSE: u32 = 1 << 5;
const P_CYCLE: u32 = 1 << 6;
const P_LOW_PROBABILITY: u32 = 1 << 7;
const P_FORCE_STACK: u32 = 1 << 8;
// extra bit for stair direction tracking (not part of pattern logic)
const P_REVERSE_STAIR: u32 = 1 << 9;

const HIT_WHISTLE: i32 = 1 << 1;
const HIT_FINISH: i32 = 1 << 2;
const HIT_CLAP: i32 = 1 << 3;

// ── pattern ──

#[derive(Clone)]
pub(super) struct Pattern {
    pub(super) columns: Vec<i32>,
    pub(super) objects: Vec<ManiaHitObject>,
}

impl Pattern {
    fn new() -> Self {
        Pattern {
            columns: Vec::new(),
            objects: Vec::new(),
        }
    }

    fn has_column(&self, c: i32) -> bool {
        self.columns.contains(&c)
    }

    fn add(&mut self, col: i32, start_time: i64, end_time: i64) {
        if !self.columns.contains(&col) {
            self.columns.push(col);
        }
        self.objects.push(ManiaHitObject {
            lane: col,
            start_time,
            end_time,
            is_long_note: end_time > start_time,
        });
    }

    fn column_count(&self) -> i32 {
        self.columns.len() as i32
    }

    // Only meaningful when column_count == 1 (matching Python's set iteration use).
    fn any_column(&self) -> i32 {
        self.columns.first().copied().unwrap_or(0)
    }
}

// ── conversion state ──

pub(super) struct ConversionState<'a> {
    pub(super) rng: LegacyRandom,
    pub(super) total_columns: i32,
    pub(super) conv_diff: f64,
    pub(super) timing_points: &'a [TimingPoint],
    pub(super) slider_multiplier: f64,
    pub(super) random_start: i32,
    pub(super) stair_type: u32,
    pub(super) prev_pattern: Pattern,
    pub(super) prev_note_times: Vec<i64>,
    pub(super) last_time: i64,
    pub(super) last_x: i32,
    pub(super) last_y: i32,
}

impl ConversionState<'_> {
    fn compute_density(&mut self, time: i64) {
        self.prev_note_times.push(time);
        if self.prev_note_times.len() > 7 {
            self.prev_note_times.remove(0);
        }
    }

    fn record_note(&mut self, time: i64, x: i32, y: i32) {
        self.last_time = time;
        self.last_x = x;
        self.last_y = y;
    }

    pub(super) fn density(&self) -> f64 {
        if self.prev_note_times.len() < 2 {
            return 2147483647.0;
        }
        let first = self.prev_note_times[0];
        let last = self.prev_note_times[self.prev_note_times.len() - 1];
        (last - first) as f64 / self.prev_note_times.len() as f64
    }

    pub(super) fn slider_segment_duration(&self, ho: &StandardHitObject) -> i64 {
        let span_count = i32::max(1, ho.slider_repeats) as i64;
        let (beat_length, slider_velocity) =
            mania_resolve_slider_timing(ho.start_time, self.timing_points);
        let adjusted_beat_length =
            beat_length * (100.0 / slider_velocity).clamp(10.0, 10000.0) / 100.0;
        let duration = (ho.start_time as f64
            + ho.slider_pixel_length * adjusted_beat_length * span_count as f64 * 0.01
                / self.slider_multiplier)
            .floor() as i64
            - ho.start_time;
        // 防御：slider_pixel_length<0 或 adjusted_beat_length 异常时 duration 可能为负
        if duration < 0 {
            return ho.start_time;
        }
        // span_count 上面已 i32::max(1, ...)，保证 ≥ 1
        duration.div_euclid(span_count)
    }

    pub(super) fn get_column(&self, x: i32, allow_special: bool) -> i32 {
        if allow_special && self.total_columns == 8 {
            return i32::min(6, (x * 7).div_euclid(512)) + 1;
        }
        i32::min(
            self.total_columns - 1,
            (x * self.total_columns).div_euclid(512),
        )
    }

    pub(super) fn get_random_column(&mut self, lo: Option<i32>, hi: Option<i32>) -> i32 {
        self.rng.next_range(
            lo.unwrap_or(self.random_start),
            hi.unwrap_or(self.total_columns),
        )
    }

    pub(super) fn find_available_column(
        &mut self,
        start: i32,
        patterns: &[&Pattern],
        lo: Option<i32>,
        hi: Option<i32>,
        gathered: bool,
        not_equal: Option<i32>,
    ) -> Result<i32> {
        let lo = lo.unwrap_or(self.random_start);
        let hi = hi.unwrap_or(self.total_columns);

        let ok = |c: i32| -> bool {
            if let Some(ne) = not_equal {
                if c == ne {
                    return false;
                }
            }
            patterns.iter().all(|p| !p.has_column(c))
        };

        if lo <= start && start < hi && ok(start) {
            return Ok(start);
        }
        if !(lo..hi).any(&ok) {
            return Err(PreviewError::new(
                "not enough columns to complete mania conversion",
            ));
        }

        let mut col = start;
        const MAX_ITERATIONS: usize = 10_000;
        for _ in 0..MAX_ITERATIONS {
            col = if gathered {
                let mut c = col + 1;
                if c == self.total_columns {
                    c = self.random_start;
                }
                c
            } else {
                self.get_random_column(Some(lo), Some(hi))
            };
            if ok(col) {
                return Ok(col);
            }
        }
        Err(PreviewError::new(
            "find_available_column exceeded max iterations",
        ))
    }

    // Inverse cumulative probability: val >= 1-pN → N (highest match wins).
    pub(super) fn get_random_note_count(&mut self, p2: f64, p3: f64, p4: f64, p5: f64) -> i32 {
        let val = self.rng.next_double();
        if p5 != 0.0 && val >= 1.0 - p5 {
            return 5;
        }
        if p4 != 0.0 && val >= 1.0 - p4 {
            return 4;
        }
        if p3 != 0.0 && val >= 1.0 - p3 {
            return 3;
        }
        if p2 != 0.0 && val >= 1.0 - p2 {
            return 2;
        }
        1
    }
}

// ── public API ──

pub(crate) fn mania_convert(
    beatmap: &Beatmap,
    target_mode: i32,
    mods: Option<&ModSettings>,
) -> Result<Beatmap> {
    if beatmap.mode() != 0 {
        return Err(PreviewError::new(
            "source beatmap must be osu!standard (mode=0)",
        ));
    }
    if target_mode != 3 {
        return Err(PreviewError::new(
            "only mania (mode=3) conversion is currently supported",
        ));
    }

    let objects = std_objects(beatmap);
    if objects.is_empty() {
        return Err(PreviewError::new(
            "standard beatmap has no hit objects to convert",
        ));
    }

    let diff = &beatmap.difficulty;
    let total_columns = mania_resolve_total_columns(objects, diff, mods);
    let seed = mania_build_seed(diff);
    let rng = LegacyRandom::new(seed as u32);
    let conv_diff = mania_compute_conversion_difficulty(objects, beatmap, diff);

    let mut state = ConversionState {
        rng,
        total_columns,
        conv_diff,
        timing_points: &beatmap.timing_points,
        slider_multiplier: diff.get_f64_or("SliderMultiplier", 1.4),
        random_start: if total_columns == 8 { 1 } else { 0 },
        stair_type: P_STAIR,
        prev_pattern: Pattern::new(),
        prev_note_times: Vec::new(),
        last_time: 0,
        last_x: 0,
        last_y: 0,
    };
    let mania_objects = mania_convert_all(&mut state, objects)?;

    let mut new_diff = diff.clone();
    new_diff.insert("CircleSize", total_columns.to_string());
    let mut new_general = beatmap.general.clone();
    new_general.insert(SOURCE_MODE_KEY, beatmap.mode().to_string());
    new_general.insert("Mode", "3".to_string());

    Ok(Beatmap {
        metadata: beatmap.metadata.clone(),
        difficulty: new_diff,
        general: new_general,
        timing_points: beatmap.timing_points.clone(),
        hit_objects: HitObjects::Mania(mania_objects),
        break_periods: beatmap.break_periods.clone(),
        combo_colors: beatmap.combo_colors.clone(),
        beat_divisor: beatmap.beat_divisor,
    })
}

fn mania_resolve_total_columns(
    hit_objects: &[StandardHitObject],
    difficulty: &KvSection,
    mods: Option<&ModSettings>,
) -> i32 {
    if let Some(m) = mods {
        if let Some(keys) = m.mania_keys {
            let mut cols = keys;
            if m.dual_stage {
                cols = i32::min(18, cols * 2);
            }
            return cols;
        }
    }

    let cs = difficulty.get_f64_or("CircleSize", 4.0);
    let od = difficulty.get_f64_or("OverallDifficulty", 8.0);
    let rounded_cs = round_half_even(cs);
    let rounded_od = round_half_even(od);
    let total = hit_objects.len();
    let end_time_obj = hit_objects
        .iter()
        .filter(|ho| ho.end_time > ho.start_time)
        .count();
    let ratio = if total > 0 {
        end_time_obj as f64 / total as f64
    } else {
        0.0
    };

    let mut cols: i32 = if ratio < 0.2 {
        7
    } else if ratio < 0.3 || rounded_cs >= 5 {
        if rounded_od > 5 {
            7
        } else {
            6
        }
    } else if ratio > 0.6 {
        if rounded_od > 4 {
            5
        } else {
            4
        }
    } else {
        i64::max(4, i64::min(7, rounded_od + 1)) as i32
    };

    if let Some(m) = mods {
        if m.dual_stage {
            cols = i32::min(18, cols * 2);
        }
    }
    cols
}

fn mania_build_seed(difficulty: &KvSection) -> i64 {
    let cs = difficulty.get_f64_or("CircleSize", 4.0);
    let od = difficulty.get_f64_or("OverallDifficulty", 8.0);
    let ar = difficulty.get_f64_or("ApproachRate", 5.0);
    let dr = difficulty.get_f64_or("HPDrainRate", 5.0);
    round_half_even(dr + cs) * 20 + (od * 41.2) as i64 + round_half_even(ar)
}

fn mania_compute_conversion_difficulty(
    hit_objects: &[StandardHitObject],
    beatmap: &Beatmap,
    difficulty: &KvSection,
) -> f64 {
    let total_break: i64 = beatmap
        .break_periods
        .iter()
        .map(|b| b.end_time - b.start_time)
        .sum();
    let first_time = hit_objects[0].start_time;
    let last_time = hit_objects[hit_objects.len() - 1].start_time;
    let drain_time_int = ((last_time - first_time - total_break) as f64 / 1000.0) as i64;
    let drain_time = if drain_time_int == 0 {
        10000.0
    } else {
        drain_time_int as f64
    };

    let dr = difficulty.get_f64_or("HPDrainRate", 5.0);
    let ar = difficulty.get_f64_or("ApproachRate", 5.0);
    let clamped_ar = ar.clamp(4.0, 7.0);
    let obj_density = hit_objects.len() as f64 / drain_time;

    let cd = ((dr + clamped_ar) / 1.5 + obj_density * 9.0) / 38.0 * 5.0 / 1.15;
    f64::min(cd, 12.0)
}

fn mania_resolve_slider_timing(start_time: i64, timing_points: &[TimingPoint]) -> (f64, f64) {
    let mut beat_length = if !timing_points.is_empty() {
        timing_points[0].beat_length
    } else {
        500.0
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

// ── main conversion loop ──

fn mania_convert_all(
    s: &mut ConversionState,
    hit_objects: &[StandardHitObject],
) -> Result<Vec<ManiaHitObject>> {
    let mut result: Vec<ManiaHitObject> = Vec::new();
    for ho in hit_objects {
        if ho.end_time > ho.start_time && ho.slider_type.is_some() {
            let seg_dur = s.slider_segment_duration(ho);
            for i in 0..(ho.slider_repeats + 1) {
                let time = ho.start_time + seg_dur * i as i64;
                s.record_note(time, ho.x, ho.y);
                s.compute_density(time);
            }
            result.extend(slider_generate(s, ho)?);
        } else if ho.end_time > ho.start_time {
            s.record_note(ho.end_time, 256, 192);
            s.compute_density(ho.end_time);
            result.extend(spinner_generate(s, ho)?);
        } else {
            let time_gap = ho.start_time - s.last_time;
            let pos_gap = ((ho.x - s.last_x) as f64).hypot((ho.y - s.last_y) as f64);
            s.compute_density(ho.start_time);
            result.extend(circle_generate(s, ho, time_gap, pos_gap)?);
            s.record_note(ho.start_time, ho.x, ho.y);
        }
    }
    Ok(result)
}
