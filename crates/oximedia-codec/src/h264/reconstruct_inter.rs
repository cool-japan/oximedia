//! Per-macroblock reconstruction for inter (P) macroblocks.
//!
//! Takes the CABAC-decoded macroblock state — per-4×4 motion
//! vectors plus residual blocks in scan order — fetches predicted
//! samples from the L0 reference frame, inverse-transforms each
//! residual block, and writes the summed-and-clipped pixels into
//! the current frame.
//!
//! All partition shapes inside one macroblock are handled
//! uniformly: the caller has already splatted partition-level MVs
//! across the per-4×4 entries in [`crate::h264::inter_cache::InterMbDecoded`],
//! so this module just iterates 4×4 sub-blocks and dispatches on
//! their MV.
//!
//! ## Scope
//!
//! - **Luma**: 16 × 4×4 sub-blocks via
//!   [`crate::h264::motion::fetch_luma_4x4_subpel`] (proper 6-tap
//!   half-pel + bilinear quarter-pel).
//! - **Chroma (4:2:0)**: 8 × 4×4 sub-blocks via
//!   [`crate::h264::motion::fetch_chroma_4x4_subpel`] (bilinear).
//! - 4×4 inverse transform via
//!   [`crate::h264::transform::dequant_and_inverse_transform_4x4`].
//! - Chroma DC is **not** yet processed via the 2×2 inverse
//!   Hadamard — for now we assume the caller has already merged DC
//!   into each chroma 4×4 block's residual (or that chroma_dc is
//!   all-zero).  The full chroma DC dispatch lands in the
//!   slice-level reconstruction step.

use crate::h264::frame::Frame;
use crate::h264::motion::{fetch_chroma_4x4_subpel, fetch_luma_4x4_subpel};
use crate::h264::transform::dequant_and_inverse_transform_4x4;
use crate::CodecError;

/// Per-macroblock inputs the reconstruction needs.  Fields here are
/// the subset of [`crate::h264::inter_cache::InterMbDecoded`] +
/// [`crate::h264::cabac_mb::MbResidualState`] that actually drives
/// reconstruction.
#[derive(Debug, Clone)]
pub struct InterPMbInputs<'a> {
    /// Macroblock column.
    pub mb_x: usize,
    /// Macroblock row.
    pub mb_y: usize,
    /// Per-4×4 motion vectors in luma quarter-pel units.
    pub mvs_l0: &'a [(i32, i32); 16],
    /// Per-4×4 luma residual coefficients in scan order.  An entry
    /// of all zeros is treated as "no residual" and skips IDCT.
    pub luma_4x4: &'a [[i32; 16]; 16],
    /// Per-chroma-4×4 residual coefficients in scan order: indices
    /// 0..=3 = Cb, 4..=7 = Cr.
    pub chroma_ac: &'a [[i32; 16]; 8],
    /// Effective luma QP for this macroblock.
    pub qp_y: u8,
    /// Effective chroma QP for this macroblock.
    pub qp_chroma: u8,
}

/// Reconstructs one inter P macroblock into the current frame.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the macroblock or its
/// chroma counterpart extends past the frame.
pub fn reconstruct_inter_p_mb(
    frame: &mut Frame,
    ref_frame: &Frame,
    inputs: &InterPMbInputs<'_>,
) -> Result<(), CodecError> {
    let px = inputs.mb_x.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 reconstruct_inter: mb_x overflow".into())
    })?;
    let py = inputs.mb_y.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 reconstruct_inter: mb_y overflow".into())
    })?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 reconstruct_inter: mb ({}, {}) extends past frame {}x{}",
            inputs.mb_x, inputs.mb_y, frame.width, frame.height
        )));
    }

    // Luma: 16 × 4×4 with per-block motion compensation.
    for sub in 0..16 {
        let sub_x = (sub % 4) * 4;
        let sub_y = (sub / 4) * 4;
        let (mv_x, mv_y) = inputs.mvs_l0[sub];
        let prediction = fetch_luma_4x4_subpel(
            ref_frame,
            (px + sub_x) as i32,
            (py + sub_y) as i32,
            mv_x,
            mv_y,
        );
        let residual = if is_all_zero(&inputs.luma_4x4[sub]) {
            [[0i32; 4]; 4]
        } else {
            dequant_and_inverse_transform_4x4(&inputs.luma_4x4[sub], inputs.qp_y)
        };
        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[j][i]);
                let v = (pred + residual[j][i]).clamp(0, 255) as u8;
                frame.set_luma(px + sub_x + i, py + sub_y + j, v);
            }
        }
    }

    // Chroma 4:2:0 — chroma plane is half-resolution per axis.
    // Each macroblock covers an 8×8 chroma block (4 × 4×4 sub-blocks)
    // per plane.  Chroma MV = luma MV (numerically — see
    // motion::fetch_chroma_4x4_subpel for why the 4:2:0 derivation
    // works out to using the luma MV directly).
    let cx = inputs.mb_x * 8;
    let cy = inputs.mb_y * 8;
    let cw = frame.chroma_width();
    let ch = frame.chroma_height();
    if cx + 8 > cw || cy + 8 > ch {
        return Err(CodecError::InvalidData(format!(
            "h264 reconstruct_inter: chroma block at ({}, {}) extends past {}x{}",
            inputs.mb_x, inputs.mb_y, cw, ch
        )));
    }

    for plane in 0..2 {
        let is_cb = plane == 0;
        for sub in 0..4 {
            let sub_x = (sub % 2) * 4;
            let sub_y = (sub / 2) * 4;
            // Pick the MV from the top-left of the matching luma area
            // (chroma 4×4 covers an 8×8 luma area at half scale).
            // For 4:2:0 the chroma 4×4 at chroma offset (sx, sy) in
            // {(0,0), (4,0), (0,4), (4,4)} maps to the luma 8×8
            // starting at (sx*2, sy*2); the luma block raster index
            // of its top-left 4×4 is (sy/2)*4 + (sx/2) → one of
            // {0, 2, 8, 10}.
            let luma_sub = (sub_y / 2) * 4 + (sub_x / 2);
            let (mv_x, mv_y) = inputs.mvs_l0[luma_sub];

            let prediction = fetch_chroma_4x4_subpel(
                ref_frame,
                (cx + sub_x) as i32,
                (cy + sub_y) as i32,
                mv_x,
                mv_y,
                is_cb,
            );
            let chroma_idx = 4 * plane + sub;
            let residual = if is_all_zero(&inputs.chroma_ac[chroma_idx]) {
                [[0i32; 4]; 4]
            } else {
                dequant_and_inverse_transform_4x4(&inputs.chroma_ac[chroma_idx], inputs.qp_chroma)
            };

            for j in 0..4 {
                for i in 0..4 {
                    let pred = i32::from(prediction[j][i]);
                    let v = (pred + residual[j][i]).clamp(0, 255) as u8;
                    if is_cb {
                        frame.set_cb(cx + sub_x + i, cy + sub_y + j, v);
                    } else {
                        frame.set_cr(cx + sub_x + i, cy + sub_y + j, v);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Returns `true` when every coefficient in the scan-order block
/// is zero — a cheap fast path that lets the caller skip the
/// dequant + IDCT pipeline entirely.
fn is_all_zero(block: &[i32; 16]) -> bool {
    block.iter().all(|&c| c == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::frame::Frame;

    fn make_frame(luma: u8) -> Frame {
        let mut f = Frame::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                f.set_luma(x, y, luma);
            }
        }
        for y in 0..8 {
            for x in 0..8 {
                f.set_cb(x, y, 128);
                f.set_cr(x, y, 128);
            }
        }
        f
    }

    #[test]
    fn zero_mv_zero_residual_copies_reference() {
        let mut frame = make_frame(0);
        let ref_frame = make_frame(120);
        let mvs = [(0i32, 0i32); 16];
        let luma_4x4 = [[0i32; 16]; 16];
        let chroma_ac = [[0i32; 16]; 8];
        let inputs = InterPMbInputs {
            mb_x: 0,
            mb_y: 0,
            mvs_l0: &mvs,
            luma_4x4: &luma_4x4,
            chroma_ac: &chroma_ac,
            qp_y: 26,
            qp_chroma: 26,
        };
        reconstruct_inter_p_mb(&mut frame, &ref_frame, &inputs).unwrap();
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(frame.get_luma(x, y), Some(120));
            }
        }
    }

    #[test]
    fn rejects_out_of_bounds_macroblock() {
        let mut frame = Frame::new(16, 16);
        let ref_frame = Frame::new(16, 16);
        let mvs = [(0i32, 0i32); 16];
        let luma_4x4 = [[0i32; 16]; 16];
        let chroma_ac = [[0i32; 16]; 8];
        let inputs = InterPMbInputs {
            mb_x: 2,
            mb_y: 0,
            mvs_l0: &mvs,
            luma_4x4: &luma_4x4,
            chroma_ac: &chroma_ac,
            qp_y: 26,
            qp_chroma: 26,
        };
        assert!(reconstruct_inter_p_mb(&mut frame, &ref_frame, &inputs).is_err());
    }
}
