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

//! Minimal bitmap text rendering (8x8 base font, nearest-neighbour scaled).
//! Glyphs are trimmed to their real width so digit spacing stays tight,
//! mirroring the role of PIL's default proportional font.
//!
//! Uses a thread-local lazy cache keyed on (char, size, color) so repeated
//! glyphs (digits, punctuation) are rendered once then alpha-composited.

use crate::canvas::{Img, Rgba};
use font8x8::legacy::BASIC_LEGACY;
use std::cell::RefCell;
use std::collections::HashMap;

fn glyph(c: char) -> [u8; 8] {
    let idx = c as usize;
    if idx < BASIC_LEGACY.len() {
        BASIC_LEGACY[idx]
    } else {
        BASIC_LEGACY[b'?' as usize]
    }
}

/// Leftmost set column and width of the glyph's used columns.
fn glyph_extent(g: &[u8; 8]) -> (u32, u32) {
    let mut min_col = 8u32;
    let mut max_col = 0u32;
    let mut any = false;
    for bits in g.iter() {
        for col in 0..8u32 {
            if bits >> col & 1 != 0 {
                any = true;
                min_col = min_col.min(col);
                max_col = max_col.max(col);
            }
        }
    }
    if any {
        (min_col, max_col - min_col + 1)
    } else {
        (0, 3) // space advance
    }
}

fn scale_for(size: u32) -> u32 {
    (size.max(8) / 8).max(1)
}

/// Approximate PIL load_default(size=N): glyph cell height ~= size.
pub fn text_size(text: &str, size: u32) -> (u32, u32) {
    let scale = scale_for(size);
    let mut w = 0u32;
    for ch in text.chars() {
        let (_, gw) = glyph_extent(&glyph(ch));
        w += (gw + 1) * scale;
    }
    (w.saturating_sub(scale), 8 * scale)
}

// ─── lazy glyph cache ───

type CacheKey = (char, u32, [u8; 4]);

thread_local! {
    static GLYPH_CACHE: RefCell<HashMap<CacheKey, Img>> = RefCell::new(HashMap::new());
}

fn build_glyph_sprite(ch: char, size: u32, color: Rgba) -> Img {
    let g = glyph(ch);
    let (min_col, gw) = glyph_extent(&g);
    let scale = scale_for(size) as i64;
    let sprite_w = (gw as i64 * scale).max(1) as u32;
    let sprite_h = (8 * scale).max(1) as u32;
    let mut sprite = Img::new(sprite_w, sprite_h, [0, 0, 0, 0]);
    for (row, bits) in g.iter().enumerate() {
        for col in 0..8u32 {
            if bits >> col & 1 != 0 {
                let px = (col - min_col) as i64 * scale;
                let py = row as i64 * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        sprite.blend_px(px + dx, py + dy, color);
                    }
                }
            }
        }
    }
    sprite
}

pub fn draw_text(img: &mut Img, x: i64, y: i64, text: &str, size: u32, color: Rgba) {
    let scale = scale_for(size) as i64;
    let mut cx = x;
    for ch in text.chars() {
        let g = glyph(ch);
        let (min_col, gw) = glyph_extent(&g);
        let sprite = GLYPH_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            let key = (ch, size, color);
            if let Some(s) = cache.get(&key) {
                return s.clone();
            }
            let s = build_glyph_sprite(ch, size, color);
            cache.insert(key, s.clone());
            s
        });
        let offset_x = cx - (min_col as i64 * scale);
        img.alpha_composite(&sprite, offset_x, y);
        cx += (gw as i64 + 1) * scale;
    }
}

#[must_use]
pub fn format_mmssmmm(ms: i64) -> String {
    let ms = ms.max(0);
    let minutes = ms / 60000;
    let seconds = (ms % 60000) / 1000;
    let millis = ms % 1000;
    format!("{minutes:02}:{seconds:02}:{millis:03}")
}
