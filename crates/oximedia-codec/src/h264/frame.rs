//! Frame buffer and neighbour-gathering helpers for H.264 intra
//! decode.
//!
//! The slice-level orchestrator iterates macroblocks in raster order
//! and, for each 4×4 (or 16×16 / chroma 8×8) sub-block, needs to
//! collect the neighbour samples that intra prediction consumes.
//! Those neighbours come from the *partially-reconstructed* output
//! frame — so the orchestrator needs a frame buffer it can read
//! samples out of while it's still writing samples in.
//!
//! This module provides:
//!
//! - [`Frame`] — Y/Cb/Cr planes with sample-level get/set methods.
//!   4:2:0 chroma subsampling is hard-coded for now (the common
//!   case); other chroma formats can land later.
//! - [`collect_intra4x4_neighbours`] — given a frame and the
//!   `(x, y)` of a 4×4 luma block's top-left corner, builds the
//!   [`Intra4x4Neighbours`] struct, threading frame-edge
//!   availability through to the optional fields.
//!
//! The MB-level orchestration that calls these primitives (header
//! parse + per-block decode + write back into the frame) is the next
//! step.

use crate::h264::intra_pred::Intra4x4Neighbours;

/// A 4:2:0 YUV frame with byte-per-sample precision.
///
/// Width and height count luma samples.  Chroma planes are half in
/// each dimension and are stored row-major like the luma plane.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Luma plane, row-major, length `width * height`.
    pub y: Vec<u8>,
    /// Cb (blue-difference chroma), row-major, length
    /// `(width / 2) * (height / 2)`.
    pub cb: Vec<u8>,
    /// Cr (red-difference chroma), row-major, length
    /// `(width / 2) * (height / 2)`.
    pub cr: Vec<u8>,
    /// Luma plane width in samples.
    pub width: usize,
    /// Luma plane height in samples.
    pub height: usize,
}

impl Frame {
    /// Allocates a zero-initialised 4:2:0 frame.
    ///
    /// `width` and `height` must each be at least 16 and multiples of
    /// 16 (one macroblock).  Smaller / odd sizes panic.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        assert!(width >= 16 && width % 16 == 0, "frame width must be a positive multiple of 16");
        assert!(height >= 16 && height % 16 == 0, "frame height must be a positive multiple of 16");
        let chroma = (width / 2) * (height / 2);
        Self {
            y: vec![0u8; width * height],
            cb: vec![0u8; chroma],
            cr: vec![0u8; chroma],
            width,
            height,
        }
    }

    /// Chroma plane width.
    #[must_use]
    pub fn chroma_width(&self) -> usize {
        self.width / 2
    }

    /// Chroma plane height.
    #[must_use]
    pub fn chroma_height(&self) -> usize {
        self.height / 2
    }

    /// Reads one luma sample.  Returns `None` for out-of-bounds
    /// coordinates so callers can use the same helper for both
    /// in-frame and edge-adjacent reads.
    #[must_use]
    pub fn get_luma(&self, x: usize, y: usize) -> Option<u8> {
        if x >= self.width || y >= self.height {
            return None;
        }
        Some(self.y[y * self.width + x])
    }

    /// Writes one luma sample.  Panics if the coordinates are out of
    /// range — the orchestrator should never produce out-of-range
    /// writes, so a panic here is a real bug.
    pub fn set_luma(&mut self, x: usize, y: usize, value: u8) {
        assert!(x < self.width && y < self.height, "luma write out of range");
        self.y[y * self.width + x] = value;
    }

    /// Reads one Cb sample (chroma-plane coordinates).
    #[must_use]
    pub fn get_cb(&self, cx: usize, cy: usize) -> Option<u8> {
        let cw = self.chroma_width();
        let ch = self.chroma_height();
        if cx >= cw || cy >= ch {
            return None;
        }
        Some(self.cb[cy * cw + cx])
    }

    /// Reads one Cr sample (chroma-plane coordinates).
    #[must_use]
    pub fn get_cr(&self, cx: usize, cy: usize) -> Option<u8> {
        let cw = self.chroma_width();
        let ch = self.chroma_height();
        if cx >= cw || cy >= ch {
            return None;
        }
        Some(self.cr[cy * cw + cx])
    }

    /// Writes one Cb sample.
    pub fn set_cb(&mut self, cx: usize, cy: usize, value: u8) {
        let cw = self.chroma_width();
        assert!(cx < cw && cy < self.chroma_height(), "Cb write out of range");
        self.cb[cy * cw + cx] = value;
    }

    /// Writes one Cr sample.
    pub fn set_cr(&mut self, cx: usize, cy: usize, value: u8) {
        let cw = self.chroma_width();
        assert!(cx < cw && cy < self.chroma_height(), "Cr write out of range");
        self.cr[cy * cw + cx] = value;
    }
}

/// Gathers the four optional neighbour samples for a 4×4 luma intra
/// prediction at the given block coordinates.
///
/// `block_x` and `block_y` are the *luma-sample* coordinates of the
/// 4×4 block's top-left corner.  They must be multiples of 4.
///
/// Availability rules implemented here:
///
/// - The top row is unavailable when `block_y == 0`.
/// - The left column is unavailable when `block_x == 0`.
/// - The top-left corner is unavailable when either of the above is
///   true.
/// - The top-right extension is unavailable when `block_y == 0`, when
///   `block_x + 7 >= frame.width`, or when the 4×4 block's row is the
///   bottom row of its containing macroblock — at that depth the
///   above-right samples haven't been written yet by raster-order
///   decoding.  The conservative "above-right unavailable for
///   bottom-row 4×4s within a macroblock" rule is omitted here; the
///   MB-level orchestrator can mask the field to `None` when it
///   knows the position-within-MB.
#[must_use]
pub fn collect_intra4x4_neighbours(
    frame: &Frame,
    block_x: usize,
    block_y: usize,
) -> Intra4x4Neighbours {
    let top = if block_y == 0 {
        None
    } else {
        let mut t = [0u8; 4];
        for (i, slot) in t.iter_mut().enumerate() {
            *slot = frame.get_luma(block_x + i, block_y - 1).unwrap_or(0);
        }
        Some(t)
    };

    let left = if block_x == 0 {
        None
    } else {
        let mut l = [0u8; 4];
        for (j, slot) in l.iter_mut().enumerate() {
            *slot = frame.get_luma(block_x - 1, block_y + j).unwrap_or(0);
        }
        Some(l)
    };

    let top_left = if block_x == 0 || block_y == 0 {
        None
    } else {
        frame.get_luma(block_x - 1, block_y - 1)
    };

    let top_right = if block_y == 0 || block_x + 7 >= frame.width {
        None
    } else {
        let mut tr = [0u8; 4];
        for (i, slot) in tr.iter_mut().enumerate() {
            *slot = frame.get_luma(block_x + 4 + i, block_y - 1).unwrap_or(0);
        }
        Some(tr)
    };

    Intra4x4Neighbours {
        top_left,
        top,
        top_right,
        left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_frame_has_correct_plane_sizes() {
        let f = Frame::new(32, 16);
        assert_eq!(f.y.len(), 32 * 16);
        assert_eq!(f.cb.len(), 16 * 8);
        assert_eq!(f.cr.len(), 16 * 8);
        assert_eq!(f.chroma_width(), 16);
        assert_eq!(f.chroma_height(), 8);
    }

    #[test]
    #[should_panic(expected = "multiple of 16")]
    fn new_frame_panics_on_non_multiple_of_16_width() {
        let _ = Frame::new(33, 16);
    }

    #[test]
    fn luma_round_trip() {
        let mut f = Frame::new(16, 16);
        f.set_luma(5, 7, 200);
        assert_eq!(f.get_luma(5, 7), Some(200));
        assert_eq!(f.get_luma(0, 0), Some(0));
    }

    #[test]
    fn out_of_range_get_returns_none() {
        let f = Frame::new(16, 16);
        assert_eq!(f.get_luma(16, 0), None);
        assert_eq!(f.get_luma(0, 16), None);
    }

    #[test]
    fn chroma_round_trip() {
        let mut f = Frame::new(32, 16);
        f.set_cb(3, 4, 100);
        f.set_cr(3, 4, 150);
        assert_eq!(f.get_cb(3, 4), Some(100));
        assert_eq!(f.get_cr(3, 4), Some(150));
    }

    #[test]
    fn top_left_block_has_no_neighbours() {
        let mut f = Frame::new(16, 16);
        // Fill row 0 and column 0 with non-zero so we'd notice if
        // they leaked into the neighbour reads.
        for x in 0..16 {
            f.set_luma(x, 0, 1);
        }
        for y in 0..16 {
            f.set_luma(0, y, 2);
        }
        let n = collect_intra4x4_neighbours(&f, 0, 0);
        assert!(n.top.is_none());
        assert!(n.left.is_none());
        assert!(n.top_left.is_none());
        assert!(n.top_right.is_none());
    }

    #[test]
    fn mid_frame_block_has_all_neighbours() {
        let mut f = Frame::new(32, 32);
        // Write a recognisable pattern in the row above and column
        // left of block (4, 4).
        for x in 0..32 {
            f.set_luma(x, 3, 100 + x as u8);
        }
        for y in 0..32 {
            f.set_luma(3, y, 50 + y as u8);
        }
        let n = collect_intra4x4_neighbours(&f, 4, 4);
        assert_eq!(n.top, Some([104, 105, 106, 107]));
        assert_eq!(n.left, Some([54, 55, 56, 57]));
        assert_eq!(n.top_left, Some(53));
        assert_eq!(n.top_right, Some([108, 109, 110, 111]));
    }

    #[test]
    fn top_right_unavailable_near_right_edge() {
        let mut f = Frame::new(16, 16);
        for x in 0..16 {
            f.set_luma(x, 3, 200);
        }
        // Block (12, 4): block_x + 7 = 19 > width=16 -> top-right None.
        let n = collect_intra4x4_neighbours(&f, 12, 4);
        assert!(n.top_right.is_none());
        // But the regular top row is still available.
        assert_eq!(n.top, Some([200; 4]));
    }
}
