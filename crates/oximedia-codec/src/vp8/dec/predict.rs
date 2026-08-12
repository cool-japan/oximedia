//! VP8 intra prediction (RFC 6386 §12).
//!
//! All VP8 WebP frames are key frames, so prediction is purely intra: each
//! block is predicted from already-reconstructed pixels immediately above and
//! to the left. This module implements:
//! - whole-block 16x16 luma and 8x8 chroma prediction (DC/V/H/TM),
//! - per-4x4 luma sub-block prediction (the ten `B_*` modes).
//!
//! Predictions are written directly into the reconstruction plane; the residual
//! (if any) is added afterwards by the caller. Edge availability (whether
//! above/left neighbours exist) is passed in by the macroblock loop.

//!
//! Ported from the production-verified `oximedia-image` `webp/vp8` decoder
//! (same workspace; a WebP lossy frame is a VP8 key frame).

use super::tables::{
    B_DC_PRED, B_HD_PRED, B_HE_PRED, B_HU_PRED, B_LD_PRED, B_RD_PRED, B_TM_PRED, B_VE_PRED,
    B_VL_PRED, B_VR_PRED, DC_PRED, H_PRED, TM_PRED, V_PRED,
};

/// Clamps an `i32` into a valid `u8` pixel value.
#[inline]
fn clamp_u8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// Average rounding helper for two values.
#[inline]
fn avg2(a: i32, b: i32) -> i32 {
    (a + b + 1) >> 1
}

/// Average rounding helper for three values (the diagonal-filter kernel).
#[inline]
fn avg3(a: i32, b: i32, c: i32) -> i32 {
    (a + 2 * b + c + 2) >> 2
}

/// Predicts a whole `size`x`size` block (16 for luma, 8 for chroma).
///
/// `plane` is the reconstruction buffer, `off` the offset of the block's
/// top-left pixel, `stride` the row stride. `have_up` / `have_left` indicate
/// neighbour availability at the frame edge.
pub fn predict_block(
    plane: &mut [u8],
    off: usize,
    stride: usize,
    size: usize,
    mode: usize,
    have_up: bool,
    have_left: bool,
) {
    match mode {
        V_PRED => predict_v(plane, off, stride, size, have_up),
        H_PRED => predict_h(plane, off, stride, size, have_left),
        TM_PRED => predict_tm(plane, off, stride, size, have_up, have_left),
        // DC_PRED and anything else fall back to DC.
        _ if mode == DC_PRED => predict_dc(plane, off, stride, size, have_up, have_left),
        _ => predict_dc(plane, off, stride, size, have_up, have_left),
    }
}

/// Vertical prediction: copy the row above into every row.
fn predict_v(plane: &mut [u8], off: usize, stride: usize, size: usize, have_up: bool) {
    let mut top = vec![127u8; size];
    if have_up {
        for (c, t) in top.iter_mut().enumerate() {
            *t = plane[off - stride + c];
        }
    }
    for r in 0..size {
        let row = off + r * stride;
        for c in 0..size {
            plane[row + c] = top[c];
        }
    }
}

/// Horizontal prediction: copy the left column into every column.
fn predict_h(plane: &mut [u8], off: usize, stride: usize, size: usize, have_left: bool) {
    for r in 0..size {
        let row = off + r * stride;
        let left = if have_left {
            i32::from(plane[row - 1])
        } else {
            129
        };
        let v = clamp_u8(left);
        for c in 0..size {
            plane[row + c] = v;
        }
    }
}

/// DC prediction: fill with the average of the available top + left edges.
fn predict_dc(
    plane: &mut [u8],
    off: usize,
    stride: usize,
    size: usize,
    have_up: bool,
    have_left: bool,
) {
    let mut sum = 0i32;
    let mut count = 0i32;
    if have_up {
        for c in 0..size {
            sum += i32::from(plane[off - stride + c]);
        }
        count += size as i32;
    }
    if have_left {
        for r in 0..size {
            sum += i32::from(plane[off + r * stride - 1]);
        }
        count += size as i32;
    }
    let dc = if count == 0 {
        128
    } else {
        // Round-to-nearest: (sum + count/2) / count, count is a power of two.
        let shift = count.trailing_zeros();
        (sum + (count >> 1)) >> shift
    };
    let v = clamp_u8(dc);
    for r in 0..size {
        let row = off + r * stride;
        for c in 0..size {
            plane[row + c] = v;
        }
    }
}

/// TrueMotion prediction: `pred = left + above - above_left`, clamped.
fn predict_tm(
    plane: &mut [u8],
    off: usize,
    stride: usize,
    size: usize,
    have_up: bool,
    have_left: bool,
) {
    // Above-left corner pixel.
    let corner = if have_up && have_left {
        i32::from(plane[off - stride - 1])
    } else if have_up {
        129
    } else {
        127
    };
    let mut top = vec![127i32; size];
    if have_up {
        for (c, t) in top.iter_mut().enumerate() {
            *t = i32::from(plane[off - stride + c]);
        }
    }
    for r in 0..size {
        let row = off + r * stride;
        let left = if have_left {
            i32::from(plane[row - 1])
        } else {
            129
        };
        for c in 0..size {
            plane[row + c] = clamp_u8(left + top[c] - corner);
        }
    }
}

/// Edge samples for a 4x4 sub-block prediction.
///
/// Layout follows RFC 6386 §12.3: `above[0..8]` are the 8 pixels above the
/// sub-block (4 directly above, 4 above-right), `left[0..4]` the 4 to the left,
/// and `corner` the above-left pixel.
pub struct SubBlockEdge {
    /// Eight samples above the 4x4 block (4 above + 4 above-right).
    pub above: [i32; 8],
    /// Four samples to the left of the 4x4 block.
    pub left: [i32; 4],
    /// The above-left corner sample.
    pub corner: i32,
}

/// Predicts one 4x4 luma sub-block using mode `mode` and the supplied edges.
///
/// The result is written into `plane` at `off` with the given `stride`.
pub fn predict_subblock(
    plane: &mut [u8],
    off: usize,
    stride: usize,
    mode: usize,
    e: &SubBlockEdge,
) {
    // Convenience aliases matching RFC 6386 §12.3 naming.
    let a = e.above;
    let l = e.left;
    let p = e.corner;

    // 4x4 working buffer, row-major.
    let mut out = [[0i32; 4]; 4];

    match mode {
        B_DC_PRED => {
            // Average of the 4 above + 4 left samples, +4 >> 3.
            let mut sum = 4i32;
            for k in 0..4 {
                sum += a[k] + l[k];
            }
            let dc = sum >> 3;
            for row in &mut out {
                for v in row {
                    *v = dc;
                }
            }
        }
        B_TM_PRED => {
            for (r, row) in out.iter_mut().enumerate() {
                for (c, v) in row.iter_mut().enumerate() {
                    *v = l[r] + a[c] - p;
                }
            }
        }
        B_VE_PRED => {
            // Smoothed vertical: each column `c` uses the 3-tap kernel on
            // (above[c-1], above[c], above[c+1]), where above[-1] is the
            // corner and above[4] is the above-right sample (RFC 6386 §12.3).
            let ext = [p, a[0], a[1], a[2], a[3], a[4]];
            let mut col = [0i32; 4];
            for c in 0..4 {
                col[c] = avg3(ext[c], ext[c + 1], ext[c + 2]);
            }
            for row in &mut out {
                row[..4].copy_from_slice(&col[..4]);
            }
        }
        B_HE_PRED => {
            // Smoothed horizontal: each row `r` uses the 3-tap kernel on
            // (left[r-1], left[r], left[r+1]), where left[-1] is the corner
            // and left[4] repeats left[3] (RFC 6386 §12.3).
            let ext = [p, l[0], l[1], l[2], l[3], l[3]];
            let mut rowv = [0i32; 4];
            for r in 0..4 {
                rowv[r] = avg3(ext[r], ext[r + 1], ext[r + 2]);
            }
            for (r, row) in out.iter_mut().enumerate() {
                for v in row.iter_mut() {
                    *v = rowv[r];
                }
            }
        }
        B_LD_PRED => {
            // Left-down diagonal using the 8 above samples.
            out[0][0] = avg3(a[0], a[1], a[2]);
            let v01 = avg3(a[1], a[2], a[3]);
            out[0][1] = v01;
            out[1][0] = v01;
            let v02 = avg3(a[2], a[3], a[4]);
            out[0][2] = v02;
            out[1][1] = v02;
            out[2][0] = v02;
            let v03 = avg3(a[3], a[4], a[5]);
            out[0][3] = v03;
            out[1][2] = v03;
            out[2][1] = v03;
            out[3][0] = v03;
            let v13 = avg3(a[4], a[5], a[6]);
            out[1][3] = v13;
            out[2][2] = v13;
            out[3][1] = v13;
            let v23 = avg3(a[5], a[6], a[7]);
            out[2][3] = v23;
            out[3][2] = v23;
            out[3][3] = avg3(a[6], a[7], a[7]);
        }
        B_RD_PRED => {
            // Right-down diagonal using corner + left + above.
            let e3 = l[3];
            let e2 = l[2];
            let e1 = l[1];
            let e0 = l[0];
            let e4 = p;
            let e5 = a[0];
            let e6 = a[1];
            let e7 = a[2];
            let e8 = a[3];
            out[3][0] = avg3(e3, e2, e1);
            let v = avg3(e2, e1, e0);
            out[3][1] = v;
            out[2][0] = v;
            let v = avg3(e1, e0, e4);
            out[3][2] = v;
            out[2][1] = v;
            out[1][0] = v;
            let v = avg3(e0, e4, e5);
            out[3][3] = v;
            out[2][2] = v;
            out[1][1] = v;
            out[0][0] = v;
            let v = avg3(e4, e5, e6);
            out[2][3] = v;
            out[1][2] = v;
            out[0][1] = v;
            let v = avg3(e5, e6, e7);
            out[1][3] = v;
            out[0][2] = v;
            out[0][3] = avg3(e6, e7, e8);
        }
        B_VR_PRED => {
            // VR prediction does not reach the bottom-left sample, so l[3]
            // (the deepest left sample) is intentionally not referenced.
            let e2 = l[2];
            let e1 = l[1];
            let e0 = l[0];
            let e4 = p;
            let e5 = a[0];
            let e6 = a[1];
            let e7 = a[2];
            let e8 = a[3];
            out[3][0] = avg3(e2, e1, e0);
            out[2][0] = avg3(e1, e0, e4);
            let v = avg3(e0, e4, e5);
            out[3][1] = v;
            out[1][0] = v;
            let v = avg2(e4, e5);
            out[2][1] = v;
            out[0][0] = v;
            let v = avg3(e4, e5, e6);
            out[3][2] = v;
            out[1][1] = v;
            let v = avg2(e5, e6);
            out[2][2] = v;
            out[0][1] = v;
            let v = avg3(e5, e6, e7);
            out[3][3] = v;
            out[1][2] = v;
            let v = avg2(e6, e7);
            out[2][3] = v;
            out[0][2] = v;
            out[1][3] = avg3(e6, e7, e8);
            out[0][3] = avg2(e7, e8);
        }
        B_VL_PRED => {
            out[0][0] = avg2(a[0], a[1]);
            out[1][0] = avg3(a[0], a[1], a[2]);
            let v = avg2(a[1], a[2]);
            out[2][0] = v;
            out[0][1] = v;
            let v = avg3(a[1], a[2], a[3]);
            out[3][0] = v;
            out[1][1] = v;
            let v = avg2(a[2], a[3]);
            out[2][1] = v;
            out[0][2] = v;
            let v = avg3(a[2], a[3], a[4]);
            out[3][1] = v;
            out[1][2] = v;
            let v = avg2(a[3], a[4]);
            out[2][2] = v;
            out[0][3] = v;
            let v = avg3(a[3], a[4], a[5]);
            out[3][2] = v;
            out[1][3] = v;
            out[2][3] = avg3(a[4], a[5], a[6]);
            out[3][3] = avg3(a[5], a[6], a[7]);
        }
        B_HD_PRED => {
            let e3 = l[3];
            let e2 = l[2];
            let e1 = l[1];
            let e0 = l[0];
            let e4 = p;
            let e5 = a[0];
            let e6 = a[1];
            let e7 = a[2];
            out[3][0] = avg2(e3, e2);
            out[3][1] = avg3(e3, e2, e1);
            let v = avg2(e2, e1);
            out[2][0] = v;
            out[3][2] = v;
            let v = avg3(e2, e1, e0);
            out[2][1] = v;
            out[3][3] = v;
            let v = avg2(e1, e0);
            out[2][2] = v;
            out[1][0] = v;
            let v = avg3(e1, e0, e4);
            out[2][3] = v;
            out[1][1] = v;
            let v = avg2(e0, e4);
            out[1][2] = v;
            out[0][0] = v;
            let v = avg3(e0, e4, e5);
            out[1][3] = v;
            out[0][1] = v;
            out[0][2] = avg3(e4, e5, e6);
            out[0][3] = avg3(e5, e6, e7);
        }
        B_HU_PRED => {
            out[0][0] = avg2(l[0], l[1]);
            out[0][1] = avg3(l[0], l[1], l[2]);
            let v = avg2(l[1], l[2]);
            out[0][2] = v;
            out[1][0] = v;
            let v = avg3(l[1], l[2], l[3]);
            out[0][3] = v;
            out[1][1] = v;
            let v = avg2(l[2], l[3]);
            out[1][2] = v;
            out[2][0] = v;
            let v = avg3(l[2], l[3], l[3]);
            out[1][3] = v;
            out[2][1] = v;
            // Remaining positions clamp onto the last left sample.
            out[2][2] = l[3];
            out[2][3] = l[3];
            out[3][0] = l[3];
            out[3][1] = l[3];
            out[3][2] = l[3];
            out[3][3] = l[3];
        }
        _ => {
            // Unknown mode: behave as DC.
            let mut sum = 4i32;
            for k in 0..4 {
                sum += a[k] + l[k];
            }
            let dc = sum >> 3;
            for row in &mut out {
                for v in row {
                    *v = dc;
                }
            }
        }
    }

    // Write the 4x4 prediction into the plane, clamping each sample.
    for (r, row) in out.iter().enumerate() {
        let dst_row = off + r * stride;
        for (c, &v) in row.iter().enumerate() {
            plane[dst_row + c] = clamp_u8(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dc_no_neighbours_is_128() {
        let mut plane = vec![0u8; 16 * 16];
        predict_block(&mut plane, 0, 16, 16, DC_PRED, false, false);
        for &v in &plane[0..16] {
            assert_eq!(v, 128);
        }
    }

    #[test]
    fn test_v_pred_copies_above() {
        let stride = 8;
        // 9 rows: 1 above row + 8 rows of 8x8 block placed at offset = stride.
        let mut plane = vec![0u8; stride * 9];
        for c in 0..8 {
            plane[c] = (c * 10) as u8;
        }
        predict_block(&mut plane, stride, stride, 8, V_PRED, true, false);
        for r in 0..8 {
            for c in 0..8 {
                assert_eq!(plane[(r + 1) * stride + c], (c * 10) as u8);
            }
        }
    }

    #[test]
    fn test_h_pred_copies_left() {
        let stride = 16;
        let mut plane = vec![0u8; stride * 16];
        for r in 0..8 {
            plane[r * stride] = (r * 5) as u8;
        }
        predict_block(&mut plane, 1, stride, 8, H_PRED, false, true);
        for r in 0..8 {
            for c in 0..8 {
                assert_eq!(plane[r * stride + 1 + c], (r * 5) as u8);
            }
        }
    }

    #[test]
    fn test_subblock_dc_average() {
        let mut plane = vec![100u8; 64];
        let e = SubBlockEdge {
            above: [10; 8],
            left: [30; 4],
            corner: 0,
        };
        predict_subblock(&mut plane, 0, 8, B_DC_PRED, &e);
        // DC = (4*10 + 4*30 + 4) >> 3 = (160 + 4) >> 3 = 20.
        for r in 0..4 {
            for c in 0..4 {
                assert_eq!(plane[r * 8 + c], 20);
            }
        }
    }

    #[test]
    fn test_subblock_tm() {
        let mut plane = vec![0u8; 64];
        let e = SubBlockEdge {
            above: [50; 8],
            left: [60; 4],
            corner: 40,
        };
        predict_subblock(&mut plane, 0, 8, B_TM_PRED, &e);
        // pred = left + above - corner = 60 + 50 - 40 = 70.
        for r in 0..4 {
            for c in 0..4 {
                assert_eq!(plane[r * 8 + c], 70);
            }
        }
    }

    #[test]
    fn test_tm_pred_block() {
        let stride = 16;
        let mut plane = vec![0u8; stride * 16];
        plane[0] = 100; // corner at (0,0) — block at (1,1)
        for c in 0..8 {
            plane[1 + c] = 120;
        }
        for r in 0..8 {
            plane[(r + 1) * stride] = 110;
        }
        predict_block(&mut plane, stride + 1, stride, 8, TM_PRED, true, true);
        // pred = 110 + 120 - 100 = 130.
        for r in 0..8 {
            for c in 0..8 {
                assert_eq!(plane[(r + 1) * stride + 1 + c], 130);
            }
        }
    }
}
