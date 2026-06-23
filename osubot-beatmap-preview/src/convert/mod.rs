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

//! standard → taiko/catch/mania conversion, ported 1:1 from the Python
//! beatmap_preview convert modules (which themselves port osu!lazer converters).
//! RNG call order and float32 round-trip points must match Python exactly.

mod catch_conv;
mod mania;
mod taiko_conv;

use crate::errors::{PreviewError, Result};
use crate::models::{Beatmap, HitObjects, StandardHitObject, TimingPoint};
use crate::mods::ModSettings;

pub(crate) use catch_conv::catch_convert;
pub(crate) use mania::mania_convert;
pub(crate) use taiko_conv::taiko_convert;

// ── dispatch ─────────────────────────────────────────────────────────────────

pub fn convert_beatmap(
    beatmap: &Beatmap,
    target_mode: i32,
    mods: Option<&ModSettings>,
) -> Result<Beatmap> {
    if beatmap.mode() != 0 {
        return Err(PreviewError::new(
            "source beatmap must be osu!standard (mode=0)",
        ));
    }

    match target_mode {
        3 => mania_convert(beatmap, target_mode, mods),
        1 => taiko_convert(beatmap, target_mode, mods),
        2 => catch_convert(beatmap, target_mode, mods),
        _ => Err(PreviewError::new(format!(
            "conversion to mode {target_mode} is not yet implemented"
        ))),
    }
}

pub(crate) fn std_objects(beatmap: &Beatmap) -> &[StandardHitObject] {
    match &beatmap.hit_objects {
        HitObjects::Standard(v) => v,
        _ => &[],
    }
}

// math.isclose(a, b, abs_tol=1e-7) keeps the default rel_tol=1e-9.
pub(crate) fn almost_equals(a: f64, b: f64) -> bool {
    (a - b).abs() <= f64::max(1e-9 * f64::max(a.abs(), b.abs()), 1e-7)
}

pub(crate) fn kiai_at(time: i64, timing_points: &[TimingPoint]) -> bool {
    let mut kiai = false;
    for point in timing_points {
        if point.time > time as f64 {
            break;
        }
        kiai = point.kiai_mode;
    }
    kiai
}
