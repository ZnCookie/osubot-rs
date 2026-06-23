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

//! standard → catch conversion (mode 2).

use crate::errors::{PreviewError, Result};
use crate::models::{Beatmap, CatchHitObject, HitObjects};
use crate::mods::ModSettings;

use super::std_objects;

pub(crate) fn catch_convert(
    beatmap: &Beatmap,
    target_mode: i32,
    _mods: Option<&ModSettings>,
) -> Result<Beatmap> {
    if beatmap.mode() != 0 {
        return Err(PreviewError::new(
            "source beatmap must be osu!standard (mode=0)",
        ));
    }
    if target_mode != 2 {
        return Err(PreviewError::new(
            "only catch (mode=2) conversion is supported here",
        ));
    }

    let objects = std_objects(beatmap);
    if objects.is_empty() {
        return Err(PreviewError::new(
            "standard beatmap has no hit objects to convert",
        ));
    }

    // CatchBeatmapConverter top-level mapping:
    // circle -> Fruit, slider -> JuiceStream, spinner -> BananaShower.
    let mut catch_objects: Vec<CatchHitObject> = objects
        .iter()
        .map(|ho| CatchHitObject {
            x: ho.x,
            y: ho.y,
            start_time: ho.start_time,
            end_time: ho.end_time,
            hit_type: ho.hit_type,
            new_combo: ho.new_combo,
            combo_offset: ho.combo_offset,
            slider_type: ho.slider_type.clone(),
            slider_points: ho.slider_points.clone(),
            slider_repeats: ho.slider_repeats,
            slider_pixel_length: ho.slider_pixel_length,
        })
        .collect();
    catch_objects.sort_by_key(|ho| (ho.start_time, ho.end_time));

    let mut new_general = beatmap.general.clone();
    new_general.insert("Mode", "2".to_string());

    Ok(Beatmap {
        metadata: beatmap.metadata.clone(),
        difficulty: beatmap.difficulty.clone(),
        general: new_general,
        timing_points: beatmap.timing_points.clone(),
        hit_objects: HitObjects::Catch(catch_objects),
        break_periods: beatmap.break_periods.clone(),
        combo_colors: beatmap.combo_colors.clone(),
        beat_divisor: beatmap.beat_divisor,
    })
}
