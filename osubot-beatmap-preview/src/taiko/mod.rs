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

//! osu!taiko renderers: multi-row PNG scroll chart and 4-row animated GIF
//! (lazer Overlapping scroll algorithm). Port of beatmap_preview/taiko/*.
//!
//! Re-exports from submodules: [constants], [timing], [notes], [png], [gif].

mod constants;
mod gif;
mod notes;
mod png;
pub(crate) mod timing;

pub use gif::render_taiko_gif;
pub use png::render_taiko_grid;
