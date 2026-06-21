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

pub(crate) mod canvas;
pub(crate) mod catch;
pub(crate) mod composer;
pub(crate) mod convert;
pub(crate) mod digits;
pub mod errors;
pub(crate) mod gif_common;
pub(crate) mod legacy_random;
pub(crate) mod mania;
pub(crate) mod models;
pub mod mods;
pub(crate) mod parser;
pub(crate) mod skin;
pub(crate) mod slider_path;
pub(crate) mod standard;
pub(crate) mod taiko;
pub(crate) mod text;
pub(crate) mod time_selection;

pub use errors::{PreviewError, Result};
pub use models::Beatmap;
pub use mods::ModSettings;

pub use convert::convert_beatmap;
pub use mods::{parse_mods, validate_mods};
pub use parser::parse_beatmap_from_bytes;

pub use catch::{render_catch_gif, render_catch_grid};
pub use mania::{render_mania_gif, render_mania_grid};
pub use standard::render_standard_gif;
pub use taiko::{render_taiko_gif, render_taiko_grid};
