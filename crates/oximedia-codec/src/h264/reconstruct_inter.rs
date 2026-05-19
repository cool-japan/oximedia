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
use crate::h264::transform::{
    dequant_and_inverse_transform_4x4_pos, inverse_hadamard_2x2_chroma_dc,
};
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
    /// Per-4×4 luma residual coefficients in row-major position
    /// order (matching CABAC's `decode_residual_nondc` output via
    /// `block[scantable[idx]] = coeff`).  An entry of all zeros
    /// is treated as "no residual" and skips IDCT.
    pub luma_4x4: &'a [[i32; 16]; 16],
    /// Chroma DC blocks for the macroblock — `[0]` = Cb 2×2 DC,
    /// `[1]` = Cr 2×2 DC.  Only the first 4 entries of each
    /// sub-array are meaningful for 4:2:0; they store the 2×2 DC
    /// block in row-major (`[0]` = (0, 0), `[1]` = (0, 1),
    /// `[2]` = (1, 0), `[3]` = (1, 1)).
    pub chroma_dc: &'a [[i32; 8]; 2],
    /// Per-chroma-4×4 residual coefficients in row-major position
    /// order (15 AC slots; DC slot at index 0 is overwritten by
    /// the inverse-Hadamard chroma DC values inside this
    /// reconstruction function).  Indices: 0..=3 = Cb, 4..=7 = Cr.
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
            dequant_and_inverse_transform_4x4_pos(&inputs.luma_4x4[sub], inputs.qp_y)
        };
        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[j][i]);
                let v = (pred + residual[j][i]).clamp(0, 255) as u8;
                frame.set_luma(px + sub_x + i, py + sub_y + j, v);
            }
        }
    }

    // Inverse-Hadamard the 2×2 chroma DC blocks before walking the
    // chroma 4×4 sub-blocks — each DC value is folded into the
    // corresponding 4×4 AC block's DC slot.
    let chroma_dc_2x2: [[[i32; 2]; 2]; 2] = [
        [
            [inputs.chroma_dc[0][0], inputs.chroma_dc[0][1]],
            [inputs.chroma_dc[0][2], inputs.chroma_dc[0][3]],
        ],
        [
            [inputs.chroma_dc[1][0], inputs.chroma_dc[1][1]],
            [inputs.chroma_dc[1][2], inputs.chroma_dc[1][3]],
        ],
    ];
    let chroma_dc_dequant: [[[i32; 2]; 2]; 2] = [
        inverse_hadamard_2x2_chroma_dc(chroma_dc_2x2[0], inputs.qp_chroma),
        inverse_hadamard_2x2_chroma_dc(chroma_dc_2x2[1], inputs.qp_chroma),
    ];

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
            // Inject the inverse-Hadamard chroma DC value into the
            // 4×4 block's DC slot before running IDCT.  Chroma
            // sub-block ordering matches the 2×2 DC layout: sub=0
            // ↔ (0, 0), sub=1 ↔ (0, 1), sub=2 ↔ (1, 0), sub=3 ↔
            // (1, 1).
            let dc_row = sub / 2;
            let dc_col = sub % 2;
            let dc = chroma_dc_dequant[plane][dc_row][dc_col];
            let mut ac_with_dc = inputs.chroma_ac[chroma_idx];
            ac_with_dc[0] = dc;
            let residual = if is_all_zero(&ac_with_dc) {
                [[0i32; 4]; 4]
            } else {
                dequant_and_inverse_transform_4x4_pos(&ac_with_dc, inputs.qp_chroma)
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

/// Per-macroblock inputs for a bi-predicted (B-slice) inter
/// reconstruction.  Mirrors [`InterPMbInputs`] but carries both
/// L0 and L1 motion data.
#[derive(Debug, Clone)]
pub struct InterBMbInputs<'a> {
    /// Macroblock column.
    pub mb_x: usize,
    /// Macroblock row.
    pub mb_y: usize,
    /// Per-4×4 L0 motion vectors in luma quarter-pel units.
    pub mvs_l0: &'a [(i32, i32); 16],
    /// Per-4×4 L1 motion vectors.
    pub mvs_l1: &'a [(i32, i32); 16],
    /// Per-4×4 L0 ref indices; `-1` when this sub-block does not
    /// use list 0 (B_L1 partitions, B_Direct with `iCbCr` flag, …).
    pub refs_l0: &'a [i8; 16],
    /// Per-4×4 L1 ref indices; `-1` when unused.
    pub refs_l1: &'a [i8; 16],
    /// Luma residual blocks in row-major position order.
    pub luma_4x4: &'a [[i32; 16]; 16],
    /// Chroma DC (Cb at `[0]`, Cr at `[1]`) — first 4 entries used.
    pub chroma_dc: &'a [[i32; 8]; 2],
    /// Chroma AC residuals (0..=3 = Cb, 4..=7 = Cr).
    pub chroma_ac: &'a [[i32; 16]; 8],
    /// Effective luma QP.
    pub qp_y: u8,
    /// Effective chroma QP.
    pub qp_chroma: u8,
}

/// Reconstructs one B-slice macroblock with optional bi-prediction.
///
/// For each 4×4 sub-block:
///
/// - If only `refs_l0[sub] >= 0`: motion-compensate from
///   `ref_l0_frame` (P-style fetch).
/// - If only `refs_l1[sub] >= 0`: from `ref_l1_frame`.
/// - If both: fetch from each, average per sample.
/// - Otherwise (rare — only B_Direct with neither list used): use
///   zero prediction.
///
/// Residual addition + clipping + write-back is identical to the P
/// path.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the macroblock or its
/// chroma counterpart extends past the frame.
pub fn reconstruct_inter_b_mb(
    frame: &mut Frame,
    ref_l0_frame: &Frame,
    ref_l1_frame: &Frame,
    inputs: &InterBMbInputs<'_>,
) -> Result<(), CodecError> {
    let px = inputs.mb_x.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 reconstruct_inter_b: mb_x overflow".into())
    })?;
    let py = inputs.mb_y.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 reconstruct_inter_b: mb_y overflow".into())
    })?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 reconstruct_inter_b: mb ({}, {}) extends past frame {}x{}",
            inputs.mb_x, inputs.mb_y, frame.width, frame.height
        )));
    }

    for sub in 0..16 {
        let sub_x = (sub % 4) * 4;
        let sub_y = (sub / 4) * 4;
        let uses_l0 = inputs.refs_l0[sub] >= 0;
        let uses_l1 = inputs.refs_l1[sub] >= 0;
        let pred_l0 = if uses_l0 {
            let (mv_x, mv_y) = inputs.mvs_l0[sub];
            fetch_luma_4x4_subpel(
                ref_l0_frame,
                (px + sub_x) as i32,
                (py + sub_y) as i32,
                mv_x,
                mv_y,
            )
        } else {
            [[0u8; 4]; 4]
        };
        let pred_l1 = if uses_l1 {
            let (mv_x, mv_y) = inputs.mvs_l1[sub];
            fetch_luma_4x4_subpel(
                ref_l1_frame,
                (px + sub_x) as i32,
                (py + sub_y) as i32,
                mv_x,
                mv_y,
            )
        } else {
            [[0u8; 4]; 4]
        };
        let prediction: [[u8; 4]; 4] = match (uses_l0, uses_l1) {
            (true, true) => bipred_average_4x4(&pred_l0, &pred_l1),
            (true, false) => pred_l0,
            (false, true) => pred_l1,
            (false, false) => [[0u8; 4]; 4],
        };
        let residual = if is_all_zero(&inputs.luma_4x4[sub]) {
            [[0i32; 4]; 4]
        } else {
            dequant_and_inverse_transform_4x4_pos(&inputs.luma_4x4[sub], inputs.qp_y)
        };
        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[j][i]);
                let v = (pred + residual[j][i]).clamp(0, 255) as u8;
                frame.set_luma(px + sub_x + i, py + sub_y + j, v);
            }
        }
    }

    // Chroma 4:2:0 — analogous bi-pred fetch + residual addition.
    let chroma_dc_2x2: [[[i32; 2]; 2]; 2] = [
        [
            [inputs.chroma_dc[0][0], inputs.chroma_dc[0][1]],
            [inputs.chroma_dc[0][2], inputs.chroma_dc[0][3]],
        ],
        [
            [inputs.chroma_dc[1][0], inputs.chroma_dc[1][1]],
            [inputs.chroma_dc[1][2], inputs.chroma_dc[1][3]],
        ],
    ];
    let chroma_dc_dequant: [[[i32; 2]; 2]; 2] = [
        inverse_hadamard_2x2_chroma_dc(chroma_dc_2x2[0], inputs.qp_chroma),
        inverse_hadamard_2x2_chroma_dc(chroma_dc_2x2[1], inputs.qp_chroma),
    ];

    let cx = inputs.mb_x * 8;
    let cy = inputs.mb_y * 8;
    let cw = frame.chroma_width();
    let ch = frame.chroma_height();
    if cx + 8 > cw || cy + 8 > ch {
        return Err(CodecError::InvalidData(format!(
            "h264 reconstruct_inter_b: chroma at ({}, {}) extends past {}x{}",
            inputs.mb_x, inputs.mb_y, cw, ch
        )));
    }

    for plane in 0..2 {
        let is_cb = plane == 0;
        for sub in 0..4 {
            let sub_x = (sub % 2) * 4;
            let sub_y = (sub / 2) * 4;
            let luma_sub = (sub_y / 2) * 4 + (sub_x / 2);
            let uses_l0 = inputs.refs_l0[luma_sub] >= 0;
            let uses_l1 = inputs.refs_l1[luma_sub] >= 0;
            let pred_l0 = if uses_l0 {
                let (mv_x, mv_y) = inputs.mvs_l0[luma_sub];
                fetch_chroma_4x4_subpel(
                    ref_l0_frame,
                    (cx + sub_x) as i32,
                    (cy + sub_y) as i32,
                    mv_x,
                    mv_y,
                    is_cb,
                )
            } else {
                [[0u8; 4]; 4]
            };
            let pred_l1 = if uses_l1 {
                let (mv_x, mv_y) = inputs.mvs_l1[luma_sub];
                fetch_chroma_4x4_subpel(
                    ref_l1_frame,
                    (cx + sub_x) as i32,
                    (cy + sub_y) as i32,
                    mv_x,
                    mv_y,
                    is_cb,
                )
            } else {
                [[0u8; 4]; 4]
            };
            let prediction: [[u8; 4]; 4] = match (uses_l0, uses_l1) {
                (true, true) => bipred_average_4x4(&pred_l0, &pred_l1),
                (true, false) => pred_l0,
                (false, true) => pred_l1,
                (false, false) => [[0u8; 4]; 4],
            };
            let chroma_idx = 4 * plane + sub;
            let dc_row = sub / 2;
            let dc_col = sub % 2;
            let dc = chroma_dc_dequant[plane][dc_row][dc_col];
            let mut ac_with_dc = inputs.chroma_ac[chroma_idx];
            ac_with_dc[0] = dc;
            let residual = if is_all_zero(&ac_with_dc) {
                [[0i32; 4]; 4]
            } else {
                dequant_and_inverse_transform_4x4_pos(&ac_with_dc, inputs.qp_chroma)
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

/// Per-sample average of two 4×4 prediction patches with the
/// standard rounding `(a + b + 1) / 2` per spec § 8.4.2.3.
fn bipred_average_4x4(a: &[[u8; 4]; 4], b: &[[u8; 4]; 4]) -> [[u8; 4]; 4] {
    let mut out = [[0u8; 4]; 4];
    for j in 0..4 {
        for i in 0..4 {
            out[j][i] = ((u16::from(a[j][i]) + u16::from(b[j][i]) + 1) >> 1) as u8;
        }
    }
    out
}

/// Reconstructs one inter P macroblock with a multi-reference
/// frame list.
///
/// Behaves identically to [`reconstruct_inter_p_mb`] when
/// `ref_list_l0` has length 1.  Per 4×4 sub-block, picks the
/// reference frame at `decoded.ref_l0[sub]` from `ref_list_l0`;
/// negative or out-of-range indices fall back to zero prediction.
///
/// `decoded.ref_l0` is a `&[i8; 16]` — one entry per 4×4
/// sub-block, mirroring [`crate::h264::inter_cache::InterMbDecoded::ref_l0`].
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the macroblock extends
/// past the frame.
pub fn reconstruct_inter_p_mb_multiref(
    frame: &mut Frame,
    ref_list_l0: &[&Frame],
    inputs: &InterPMbInputs<'_>,
    ref_idx_per_block: &[i8; 16],
) -> Result<(), CodecError> {
    let px = inputs.mb_x.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 reconstruct_inter_p_multiref: mb_x overflow".into())
    })?;
    let py = inputs.mb_y.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 reconstruct_inter_p_multiref: mb_y overflow".into())
    })?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 reconstruct_inter_p_multiref: mb ({}, {}) extends past frame {}x{}",
            inputs.mb_x, inputs.mb_y, frame.width, frame.height
        )));
    }

    for sub in 0..16 {
        let sub_x = (sub % 4) * 4;
        let sub_y = (sub / 4) * 4;
        let (mv_x, mv_y) = inputs.mvs_l0[sub];
        let ref_idx = ref_idx_per_block[sub];
        let ref_frame = pick_ref_frame(ref_list_l0, ref_idx);
        let prediction = if let Some(rf) = ref_frame {
            fetch_luma_4x4_subpel(rf, (px + sub_x) as i32, (py + sub_y) as i32, mv_x, mv_y)
        } else {
            [[0u8; 4]; 4]
        };
        let residual = if is_all_zero(&inputs.luma_4x4[sub]) {
            [[0i32; 4]; 4]
        } else {
            dequant_and_inverse_transform_4x4_pos(&inputs.luma_4x4[sub], inputs.qp_y)
        };
        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[j][i]);
                let v = (pred + residual[j][i]).clamp(0, 255) as u8;
                frame.set_luma(px + sub_x + i, py + sub_y + j, v);
            }
        }
    }

    let chroma_dc_2x2: [[[i32; 2]; 2]; 2] = [
        [
            [inputs.chroma_dc[0][0], inputs.chroma_dc[0][1]],
            [inputs.chroma_dc[0][2], inputs.chroma_dc[0][3]],
        ],
        [
            [inputs.chroma_dc[1][0], inputs.chroma_dc[1][1]],
            [inputs.chroma_dc[1][2], inputs.chroma_dc[1][3]],
        ],
    ];
    let chroma_dc_dequant: [[[i32; 2]; 2]; 2] = [
        inverse_hadamard_2x2_chroma_dc(chroma_dc_2x2[0], inputs.qp_chroma),
        inverse_hadamard_2x2_chroma_dc(chroma_dc_2x2[1], inputs.qp_chroma),
    ];

    let cx = inputs.mb_x * 8;
    let cy = inputs.mb_y * 8;
    let cw = frame.chroma_width();
    let ch = frame.chroma_height();
    if cx + 8 > cw || cy + 8 > ch {
        return Err(CodecError::InvalidData(format!(
            "h264 reconstruct_inter_p_multiref: chroma at ({}, {}) extends past {}x{}",
            inputs.mb_x, inputs.mb_y, cw, ch
        )));
    }

    for plane in 0..2 {
        let is_cb = plane == 0;
        for sub in 0..4 {
            let sub_x = (sub % 2) * 4;
            let sub_y = (sub / 2) * 4;
            let luma_sub = (sub_y / 2) * 4 + (sub_x / 2);
            let (mv_x, mv_y) = inputs.mvs_l0[luma_sub];
            let ref_idx = ref_idx_per_block[luma_sub];
            let ref_frame = pick_ref_frame(ref_list_l0, ref_idx);
            let prediction = if let Some(rf) = ref_frame {
                fetch_chroma_4x4_subpel(
                    rf,
                    (cx + sub_x) as i32,
                    (cy + sub_y) as i32,
                    mv_x,
                    mv_y,
                    is_cb,
                )
            } else {
                [[0u8; 4]; 4]
            };
            let chroma_idx = 4 * plane + sub;
            let dc_row = sub / 2;
            let dc_col = sub % 2;
            let dc = chroma_dc_dequant[plane][dc_row][dc_col];
            let mut ac_with_dc = inputs.chroma_ac[chroma_idx];
            ac_with_dc[0] = dc;
            let residual = if is_all_zero(&ac_with_dc) {
                [[0i32; 4]; 4]
            } else {
                dequant_and_inverse_transform_4x4_pos(&ac_with_dc, inputs.qp_chroma)
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

fn pick_ref_frame<'a>(list: &'a [&'a Frame], idx: i8) -> Option<&'a Frame> {
    if idx < 0 {
        return None;
    }
    list.get(idx as usize).copied()
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
        let chroma_dc = [[0i32; 8]; 2];
        let chroma_ac = [[0i32; 16]; 8];
        let inputs = InterPMbInputs {
            mb_x: 0,
            mb_y: 0,
            mvs_l0: &mvs,
            luma_4x4: &luma_4x4,
            chroma_dc: &chroma_dc,
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
        let chroma_dc = [[0i32; 8]; 2];
        let chroma_ac = [[0i32; 16]; 8];
        let inputs = InterPMbInputs {
            mb_x: 2,
            mb_y: 0,
            mvs_l0: &mvs,
            luma_4x4: &luma_4x4,
            chroma_dc: &chroma_dc,
            chroma_ac: &chroma_ac,
            qp_y: 26,
            qp_chroma: 26,
        };
        assert!(reconstruct_inter_p_mb(&mut frame, &ref_frame, &inputs).is_err());
    }

    #[test]
    fn nonzero_chroma_dc_perturbs_chroma_samples() {
        let mut frame = Frame::new(16, 16);
        let ref_frame = make_frame(0);
        let mvs = [(0i32, 0i32); 16];
        let luma_4x4 = [[0i32; 16]; 16];
        // One nonzero chroma DC for Cb sub-block 0 — the inverse
        // Hadamard distributes it across all four Cb sub-blocks.
        let mut chroma_dc = [[0i32; 8]; 2];
        chroma_dc[0][0] = 4;
        let chroma_ac = [[0i32; 16]; 8];
        let inputs = InterPMbInputs {
            mb_x: 0,
            mb_y: 0,
            mvs_l0: &mvs,
            luma_4x4: &luma_4x4,
            chroma_dc: &chroma_dc,
            chroma_ac: &chroma_ac,
            qp_y: 26,
            qp_chroma: 26,
        };
        reconstruct_inter_p_mb(&mut frame, &ref_frame, &inputs).unwrap();
        // The Cb samples should now differ from the reference's
        // initial 0; we don't pin the exact value but at least one
        // sample should be > 0.
        let mut any_nonzero = false;
        for y in 0..8 {
            for x in 0..8 {
                if frame.get_cb(x, y).unwrap() > 0 {
                    any_nonzero = true;
                }
            }
        }
        assert!(any_nonzero, "chroma DC injection did not perturb any Cb sample");
    }
}
