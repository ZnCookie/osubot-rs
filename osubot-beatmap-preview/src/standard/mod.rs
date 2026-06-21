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

//! osu!standard renderer: per-frame 512×384 gameplay snapshots composed into a
//! PNG grid (5×8) or animated GIF (2×2 segments). Port of the Python renderer
//! with identical constants, alpha curves and layout.

mod alpha;
mod constants;
pub(crate) mod context;
mod gif;
mod objects;
pub(crate) mod slider;

pub use gif::render_standard_gif;
