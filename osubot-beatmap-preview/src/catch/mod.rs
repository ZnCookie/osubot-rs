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

//! osu!catch renderers: render-object expansion (fruits, juice streams,
//! banana showers, HR offsets, hyperdash) plus PNG grid and GIF previews.
//! RNG call order mirrors the Python/stable implementations exactly.

mod constants;
mod drawing;
mod gif;
pub(crate) mod objects;
mod png;

pub use gif::render_catch_gif;
pub use png::render_catch_grid;
