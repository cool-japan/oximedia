// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! In-place RGBA8 drawing surface.
//!
//! Unlike the upstream native `Canvas` (which owns a freshly allocated `Vec`
//! per call), [`CanvasMut`] borrows the caller-provided output slice, so
//! rendering a scope never allocates. It carries the same primitive drawing
//! operations the scope renderers need — pixel set / blend / accumulate, H/V
//! lines, Bresenham lines, rectangles, circles, and text via [`crate::font`].
//!
//! All coordinates are `u32`; out-of-bounds writes are silently clipped so no
//! drawing call can panic (important under `panic = "abort"`).

use crate::font;

/// A packed RGBA colour.
pub type Color = [u8; 4];

/// Opaque black — the scope background.
pub const BLACK: Color = [0, 0, 0, 255];
/// Semi-transparent white used for graticule lines.
pub const GRATICULE: Color = [255, 255, 255, 128];

/// A mutable RGBA8 canvas backed by a borrowed, tightly packed slice.
pub struct CanvasMut<'a> {
    data: &'a mut [u8],
    width: u32,
    height: u32,
}

impl<'a> CanvasMut<'a> {
    /// Wraps `data` as a `width x height` RGBA8 canvas.
    ///
    /// Returns [`None`] if `data` is not exactly `width * height * 4` bytes.
    #[must_use]
    pub fn new(data: &'a mut [u8], width: u32, height: u32) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|p| p.checked_mul(4))?;
        if data.len() != expected {
            return None;
        }
        Some(Self {
            data,
            width,
            height,
        })
    }

    /// Canvas width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Canvas height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Fills the whole canvas with opaque black.
    pub fn clear_black(&mut self) {
        for px in self.data.chunks_exact_mut(4) {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
            px[3] = 255;
        }
    }

    #[inline]
    fn index(&self, x: u32, y: u32) -> Option<usize> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(((y * self.width + x) * 4) as usize)
    }

    /// Writes `color` at `(x, y)`, ignoring out-of-bounds coordinates.
    pub fn set_pixel(&mut self, x: u32, y: u32, color: Color) {
        if let Some(idx) = self.index(x, y) {
            self.data[idx..idx + 4].copy_from_slice(&color);
        }
    }

    /// Mutable RGBA8 slice of row `y` (`width * 4` bytes), or [`None`] out
    /// of bounds. The bulk-write primitive for the scope trace normalise
    /// passes: writing whole rows through one slice lets those loops
    /// auto-vectorise, where per-pixel [`CanvasMut::set_pixel`] calls
    /// (with their per-call bounds checks) cannot.
    #[inline]
    #[must_use]
    pub fn row_mut(&mut self, y: u32) -> Option<&mut [u8]> {
        if y >= self.height {
            return None;
        }
        let start = (y * self.width * 4) as usize;
        let len = (self.width * 4) as usize;
        Some(&mut self.data[start..start + len])
    }

    /// Reads the pixel at `(x, y)`, returning opaque black if out of bounds.
    #[must_use]
    pub fn get_pixel(&self, x: u32, y: u32) -> Color {
        match self.index(x, y) {
            Some(idx) => [
                self.data[idx],
                self.data[idx + 1],
                self.data[idx + 2],
                self.data[idx + 3],
            ],
            None => BLACK,
        }
    }

    /// Source-over alpha blend of `color` onto the existing pixel (result is
    /// opaque).
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    pub fn blend_pixel(&mut self, x: u32, y: u32, color: Color) {
        let Some(idx) = self.index(x, y) else {
            return;
        };
        let existing = self.get_pixel(x, y);
        let alpha = f32::from(color[3]) / 255.0;
        let inv = 1.0 - alpha;
        for i in 0..3 {
            self.data[idx + i] =
                (f32::from(color[i]) * alpha + f32::from(existing[i]) * inv) as u8;
        }
        self.data[idx + 3] = 255;
    }

    /// Draws a horizontal line spanning `[x1, x2]` at row `y`.
    pub fn draw_hline(&mut self, x1: u32, x2: u32, y: u32, color: Color) {
        let (start, end) = (x1.min(x2), x1.max(x2));
        for x in start..=end {
            self.plot(x, y, color);
        }
    }

    /// Draws a vertical line spanning `[y1, y2]` at column `x`.
    pub fn draw_vline(&mut self, x: u32, y1: u32, y2: u32, color: Color) {
        let (start, end) = (y1.min(y2), y1.max(y2));
        for y in start..=end {
            self.plot(x, y, color);
        }
    }

    #[inline]
    fn plot(&mut self, x: u32, y: u32, color: Color) {
        if color[3] < 255 {
            self.blend_pixel(x, y, color);
        } else {
            self.set_pixel(x, y, color);
        }
    }

    /// Draws a line between two points with Bresenham's algorithm.
    #[allow(clippy::cast_possible_wrap)]
    #[allow(clippy::cast_sign_loss)]
    pub fn draw_line(&mut self, x1: u32, y1: u32, x2: u32, y2: u32, color: Color) {
        let (mut x, mut y) = (x1 as i64, y1 as i64);
        let (x2, y2) = (x2 as i64, y2 as i64);
        let dx = (x2 - x).abs();
        let dy = -(y2 - y).abs();
        let sx = if x < x2 { 1 } else { -1 };
        let sy = if y < y2 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            if x >= 0 && y >= 0 {
                self.plot(x as u32, y as u32, color);
            }
            if x == x2 && y == y2 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draws a rectangle outline with top-left `(x, y)`.
    pub fn draw_rect(&mut self, x: u32, y: u32, width: u32, height: u32, color: Color) {
        if width == 0 || height == 0 {
            return;
        }
        let x2 = x + width - 1;
        let y2 = y + height - 1;
        self.draw_hline(x, x2, y, color);
        self.draw_hline(x, x2, y2, color);
        self.draw_vline(x, y, y2, color);
        self.draw_vline(x2, y, y2, color);
    }

    /// Draws a circle outline (midpoint algorithm).
    #[allow(clippy::cast_possible_wrap)]
    #[allow(clippy::cast_sign_loss)]
    pub fn draw_circle(&mut self, cx: u32, cy: u32, radius: u32, color: Color) {
        if radius == 0 {
            return;
        }
        let (cx, cy, r) = (cx as i64, cy as i64, radius as i64);
        let mut x = 0i64;
        let mut y = r;
        let mut d = 3 - 2 * r;
        while x <= y {
            for (px, py) in [
                (cx + x, cy + y),
                (cx - x, cy + y),
                (cx + x, cy - y),
                (cx - x, cy - y),
                (cx + y, cy + x),
                (cx - y, cy + x),
                (cx + y, cy - x),
                (cx - y, cy - x),
            ] {
                if px >= 0 && py >= 0 {
                    self.plot(px as u32, py as u32, color);
                }
            }
            if d <= 0 {
                d += 4 * x + 6;
            } else {
                d += 4 * (x - y) + 10;
                y -= 1;
            }
            x += 1;
        }
    }

    /// Draws `text` at `(x, y)` using the 5x7 [`crate::font`]. Unsupported
    /// characters advance the cursor but draw nothing.
    pub fn draw_text(&mut self, x: u32, y: u32, text: &str, color: Color) {
        let mut ox = 0u32;
        for c in text.chars() {
            if let Some(rows) = font::glyph(c) {
                for (row, bits) in rows.iter().enumerate() {
                    for col in 0..font::GLYPH_WIDTH {
                        if (bits >> (font::GLYPH_WIDTH - 1 - col)) & 1 != 0 {
                            self.set_pixel(x + ox + col, y + row as u32, color);
                        }
                    }
                }
            }
            ox += font::GLYPH_ADVANCE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_rejects_wrong_length() {
        let mut buf = vec![0u8; 10];
        assert!(CanvasMut::new(&mut buf, 2, 2).is_none());
        let mut ok = vec![0u8; 16];
        assert!(CanvasMut::new(&mut ok, 2, 2).is_some());
    }

    #[test]
    fn clear_black_sets_opaque() {
        let mut buf = vec![7u8; 16];
        let mut c = CanvasMut::new(&mut buf, 2, 2).expect("canvas");
        c.clear_black();
        assert_eq!(buf, [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn out_of_bounds_is_noop() {
        let mut buf = vec![0u8; 16];
        let mut c = CanvasMut::new(&mut buf, 2, 2).expect("canvas");
        c.set_pixel(99, 99, [1, 2, 3, 4]);
        c.blend_pixel(99, 99, [1, 2, 3, 128]);
        assert!(c.row_mut(99).is_none(), "row 99 must be out of bounds");
        assert!(buf.iter().all(|&b| b == 0));
    }

    #[test]
    fn row_mut_spans_exactly_one_row() {
        let mut buf = vec![0u8; 2 * 2 * 4];
        let mut c = CanvasMut::new(&mut buf, 2, 2).expect("canvas");
        let row = c.row_mut(1).expect("row 1");
        assert_eq!(row.len(), 8);
        row.fill(7);
        assert!(buf[..8].iter().all(|&b| b == 0), "row 0 untouched");
        assert!(buf[8..].iter().all(|&b| b == 7), "row 1 filled");
    }

    #[test]
    fn draw_text_marks_pixels() {
        let mut buf = vec![0u8; 32 * 16 * 4];
        let mut c = CanvasMut::new(&mut buf, 32, 16).expect("canvas");
        c.draw_text(0, 0, "R", [255, 255, 255, 255]);
        let lit = buf.chunks_exact(4).filter(|p| p[0] == 255).count();
        assert!(lit > 0, "R glyph drew no pixels");
    }
}
