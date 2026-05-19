//! Macroblock-level intra decode orchestrator.
//!
//! Given a frame buffer (partially reconstructed) and the parsed
//! syntax / residuals for one macroblock, the functions here run the
//! per-block predict → inverse-transform → add → clip → write
//! pipeline and update the frame in place.  They sit at the level
//! above the per-stage modules (`intra_pred`, `transform`, `frame`)
//! and below the future slice-level loop that walks macroblocks in
//! raster order.
//!
//! Scope:
//!
//! - `I_16x16` macroblocks: single 16×16 luma prediction, 16 4×4
//!   residual blocks, chroma 8×8 prediction with 4 chroma 4×4
//!   residuals.
//! - Chroma 8×8 prediction for both components.
//! - The `I_NxN` macroblock path (16 separate 4×4 intra predictions,
//!   each driven by the spec's "most probable mode" derivation) is
//!   not yet wired up; it lands separately together with the MPM
//!   logic.
//!
//! Residuals are passed in by the caller as an optional slice of
//! 16-element coefficient arrays per 4×4 block.  Passing `None` for
//! a block means "all zeros" — the orchestrator skips the dequant /
//! inverse-transform path entirely.  This lets the orchestrator be
//! tested end-to-end before the CAVLC `coeff_token` lookup tables
//! land.

use crate::h264::frame::{
    collect_chroma_8x8_neighbours, collect_intra16x16_neighbours, collect_intra4x4_neighbours,
    Frame,
};
use crate::h264::intra_pred::{predict_16x16, predict_4x4, predict_chroma_8x8, Intra4x4Mode};
use crate::h264::macroblock::{Intra16x16PredMode, IntraChromaPredMode};
use crate::h264::motion::{fetch_chroma_4x4_subpel, fetch_luma_4x4_integer};
use crate::h264::transform::dequant_and_inverse_transform_4x4;
use crate::CodecError;

/// Per-4×4-block residual coefficient slot.
///
/// `None` means the block has no residual (its CBP bit was 0).
pub type Residual4x4Scan = Option<[i32; 16]>;

/// Decodes one `I_16x16` luma macroblock and writes the reconstructed
/// 16×16 luma samples into the frame.
///
/// `mb_x` / `mb_y` are macroblock-unit coordinates.
/// `pred_mode` picks one of the four I_16x16 intra prediction modes.
/// `luma_4x4_residuals` is a 4×4 array (in raster order) of optional
/// 4×4 residual coefficient blocks already decoded by the caller; a
/// `None` entry means "no residual" and the corresponding 4×4 region
/// in the macroblock keeps the pure prediction.
///
/// Note: the canonical I_16x16 path also has a separate 4×4 luma DC
/// Hadamard step that re-distributes DC coefficients across the 16
/// sub-blocks.  When the caller has already folded the dequantized
/// DC values into each 4×4 block's `[0]` position, that step is
/// done — and this function does no additional Hadamard work.  The
/// caller is responsible for that fold; the Hadamard primitives live
/// in [`crate::h264::transform`].
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when `mb_x` / `mb_y` would
/// place the macroblock outside the frame.
pub fn decode_intra_16x16_mb(
    frame: &mut Frame,
    mb_x: usize,
    mb_y: usize,
    pred_mode: Intra16x16PredMode,
    luma_4x4_residuals: &[Residual4x4Scan; 16],
    qp_y: u8,
) -> Result<(), CodecError> {
    let px = mb_x.checked_mul(16).ok_or_else(|| CodecError::InvalidData(
        "h264 decoder: macroblock x position overflows".into(),
    ))?;
    let py = mb_y.checked_mul(16).ok_or_else(|| CodecError::InvalidData(
        "h264 decoder: macroblock y position overflows".into(),
    ))?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 decoder: macroblock at ({mb_x}, {mb_y}) extends past frame ({}x{})",
            frame.width, frame.height,
        )));
    }

    let neighbours = collect_intra16x16_neighbours(frame, mb_x, mb_y);
    let prediction = predict_16x16(pred_mode, &neighbours);

    // Walk the 16 4×4 sub-blocks of this macroblock in raster order.
    for sub in 0..16 {
        let sub_x = (sub % 4) * 4;
        let sub_y = (sub / 4) * 4;

        let residual_block = match luma_4x4_residuals[sub] {
            None => [[0i32; 4]; 4],
            Some(scan) => dequant_and_inverse_transform_4x4(&scan, qp_y),
        };

        for j in 0..4 {
            for i in 0..4 {
                let y = sub_y + j;
                let x = sub_x + i;
                let pred = i32::from(prediction[y][x]);
                let res = residual_block[j][i];
                let sample = (pred + res).clamp(0, 255) as u8;
                frame.set_luma(px + x, py + y, sample);
            }
        }
    }

    Ok(())
}

/// Decodes one 8×8 chroma block for a given component (Cb or Cr) and
/// writes the reconstructed chroma samples into the frame.
///
/// `chroma_4x4_residuals` is a 2×2 array (in raster order) of
/// optional 4×4 residual coefficient blocks already decoded by the
/// caller.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the chroma block extends
/// past the frame.
pub fn decode_intra_chroma_8x8(
    frame: &mut Frame,
    mb_x: usize,
    mb_y: usize,
    pred_mode: IntraChromaPredMode,
    chroma_4x4_residuals: &[Residual4x4Scan; 4],
    qp_chroma: u8,
    is_cb: bool,
) -> Result<(), CodecError> {
    let cx = mb_x.checked_mul(8).ok_or_else(|| CodecError::InvalidData(
        "h264 decoder: chroma x position overflows".into(),
    ))?;
    let cy = mb_y.checked_mul(8).ok_or_else(|| CodecError::InvalidData(
        "h264 decoder: chroma y position overflows".into(),
    ))?;
    let cw = frame.chroma_width();
    let ch = frame.chroma_height();
    if cx + 8 > cw || cy + 8 > ch {
        return Err(CodecError::InvalidData(format!(
            "h264 decoder: chroma block at ({mb_x}, {mb_y}) extends past chroma plane ({cw}x{ch})"
        )));
    }

    let neighbours = collect_chroma_8x8_neighbours(frame, mb_x, mb_y, is_cb);
    let prediction = predict_chroma_8x8(pred_mode, &neighbours);

    for sub in 0..4 {
        let sub_x = (sub % 2) * 4;
        let sub_y = (sub / 2) * 4;

        let residual_block = match chroma_4x4_residuals[sub] {
            None => [[0i32; 4]; 4],
            Some(scan) => dequant_and_inverse_transform_4x4(&scan, qp_chroma),
        };

        for j in 0..4 {
            for i in 0..4 {
                let y = sub_y + j;
                let x = sub_x + i;
                let pred = i32::from(prediction[y][x]);
                let res = residual_block[j][i];
                let sample = (pred + res).clamp(0, 255) as u8;
                if is_cb {
                    frame.set_cb(cx + x, cy + y, sample);
                } else {
                    frame.set_cr(cx + x, cy + y, sample);
                }
            }
        }
    }

    Ok(())
}

/// Decodes one P-macroblock with a single 16×16 motion vector (the
/// `P_L0_16x16` partition shape) and writes the reconstructed
/// samples into the frame.
///
/// `ref_frame` is the L0 reference frame.  `mv_x` / `mv_y` are the
/// quarter-pel motion vector for luma; chroma uses the same numeric
/// MV interpreted as eighth-pel against the half-resolution chroma
/// plane (the H.264 4:2:0 derivation).
///
/// Only the integer-pel luma fetch path is implemented in this
/// commit; sub-pel luma offsets fall back to integer-pel fetch with
/// the offset truncated, which is non-conformant but lets the
/// orchestrator function be wired up and tested before the full
/// luma quarter-pel filter dispatch lands.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the macroblock extends
/// past the output frame.
#[allow(clippy::too_many_arguments)]
pub fn decode_inter_p_l0_16x16_mb(
    frame: &mut Frame,
    ref_frame: &Frame,
    mb_x: usize,
    mb_y: usize,
    mv_x: i32,
    mv_y: i32,
    luma_4x4_residuals: &[Residual4x4Scan; 16],
    chroma_cb_residuals: &[Residual4x4Scan; 4],
    chroma_cr_residuals: &[Residual4x4Scan; 4],
    qp_y: u8,
    qp_chroma: u8,
) -> Result<(), CodecError> {
    let px = mb_x
        .checked_mul(16)
        .ok_or_else(|| CodecError::InvalidData("mb_x overflow".into()))?;
    let py = mb_y
        .checked_mul(16)
        .ok_or_else(|| CodecError::InvalidData("mb_y overflow".into()))?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "macroblock at ({mb_x}, {mb_y}) extends past frame"
        )));
    }

    // Luma: integer-pel fetch (sub-pel offset truncated for now).
    let int_mv_x = mv_x >> 2;
    let int_mv_y = mv_y >> 2;
    for sub in 0..16 {
        let sub_x_in_mb = (sub % 4) * 4;
        let sub_y_in_mb = (sub / 4) * 4;
        let ref_x = (px as i32 + sub_x_in_mb as i32) + int_mv_x;
        let ref_y = (py as i32 + sub_y_in_mb as i32) + int_mv_y;
        let prediction = fetch_luma_4x4_integer(ref_frame, ref_x, ref_y);
        let residual = match luma_4x4_residuals[sub] {
            None => [[0i32; 4]; 4],
            Some(scan) => dequant_and_inverse_transform_4x4(&scan, qp_y),
        };
        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[j][i]);
                let v = (pred + residual[j][i]).clamp(0, 255) as u8;
                frame.set_luma(px + sub_x_in_mb + i, py + sub_y_in_mb + j, v);
            }
        }
    }

    // Chroma: half-resolution.  MV is interpreted as eighth-pel
    // against the chroma plane.  Both Cb and Cr use the same MV.
    let cx = mb_x * 8;
    let cy = mb_y * 8;
    let chroma_mv_x = mv_x; // already eighth-pel resolution for 4:2:0
    let chroma_mv_y = mv_y;
    for component_is_cb in [true, false] {
        let residuals = if component_is_cb {
            chroma_cb_residuals
        } else {
            chroma_cr_residuals
        };
        for sub in 0..4 {
            let sub_x_in_block = (sub % 2) * 4;
            let sub_y_in_block = (sub / 2) * 4;
            let block_cx = (cx + sub_x_in_block) as i32;
            let block_cy = (cy + sub_y_in_block) as i32;
            let prediction = fetch_chroma_4x4_subpel(
                ref_frame,
                block_cx,
                block_cy,
                chroma_mv_x,
                chroma_mv_y,
                component_is_cb,
            );
            let residual = match residuals[sub] {
                None => [[0i32; 4]; 4],
                Some(scan) => dequant_and_inverse_transform_4x4(&scan, qp_chroma),
            };
            for j in 0..4 {
                for i in 0..4 {
                    let pred = i32::from(prediction[j][i]);
                    let v = (pred + residual[j][i]).clamp(0, 255) as u8;
                    let dst_x = cx + sub_x_in_block + i;
                    let dst_y = cy + sub_y_in_block + j;
                    if component_is_cb {
                        frame.set_cb(dst_x, dst_y, v);
                    } else {
                        frame.set_cr(dst_x, dst_y, v);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Parsed-and-resolved info for one macroblock that an I-slice
/// decoder needs to reconstruct pixels.
#[derive(Debug, Clone)]
pub struct IntraMacroblock {
    /// Macroblock coordinates in macroblock units.
    pub mb_x: usize,
    /// Macroblock coordinates in macroblock units.
    pub mb_y: usize,
    /// Luma intra mode plus residual payload for this macroblock.
    pub luma: IntraLumaSpec,
    /// Chroma intra prediction mode (shared by both components).
    pub chroma_mode: IntraChromaPredMode,
    /// Per-block residual coefficients for Cb (4 blocks in raster).
    pub chroma_cb_residuals: [Residual4x4Scan; 4],
    /// Per-block residual coefficients for Cr (4 blocks in raster).
    pub chroma_cr_residuals: [Residual4x4Scan; 4],
    /// Effective luma QP for this macroblock.
    pub qp_y: u8,
    /// Effective chroma QP for this macroblock.
    pub qp_chroma: u8,
}

/// Luma side of an intra macroblock.
///
/// One enum variant per macroblock type the I-slice decoder
/// supports.  `I_PCM` and the inter macroblock types are not
/// included — those land in their own follow-ups.
#[derive(Debug, Clone)]
pub enum IntraLumaSpec {
    /// `I_16x16` macroblock: single 16×16 prediction plus 16 per-4×4
    /// residual blocks.
    Sixteen16 {
        /// Prediction direction.
        pred_mode: Intra16x16PredMode,
        /// 16 per-4×4-block residual coefficient arrays.
        residuals: [Residual4x4Scan; 16],
    },
    /// `I_NxN` macroblock: 16 separate 4×4 predictions, each with
    /// its own mode and residual.
    NxN {
        /// 16 per-sub-block intra modes in raster order.
        modes: [Intra4x4Mode; 16],
        /// 16 per-sub-block residual coefficient arrays.
        residuals: [Residual4x4Scan; 16],
    },
}

/// Decodes one I-slice from a list of already-parsed and resolved
/// intra macroblocks and writes the reconstructed samples into the
/// frame.
///
/// `macroblocks` must cover the slice in raster order; the function
/// applies each macroblock in turn.  Callers are responsible for
/// having already parsed the slice header, resolved each macroblock
/// type, applied the MPM logic to recover per-block intra-4×4 modes,
/// and decoded the residual coefficients (via the entropy decoder
/// once the CAVLC coefficient-decode tables land).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when any macroblock extends
/// past the frame.
pub fn decode_i_slice(
    frame: &mut Frame,
    macroblocks: &[IntraMacroblock],
) -> Result<(), CodecError> {
    for mb in macroblocks {
        match &mb.luma {
            IntraLumaSpec::Sixteen16 {
                pred_mode,
                residuals,
            } => {
                decode_intra_16x16_mb(
                    frame,
                    mb.mb_x,
                    mb.mb_y,
                    *pred_mode,
                    residuals,
                    mb.qp_y,
                )?;
            }
            IntraLumaSpec::NxN { modes, residuals } => {
                decode_intra_4x4_mb(
                    frame,
                    mb.mb_x,
                    mb.mb_y,
                    modes,
                    residuals,
                    mb.qp_y,
                )?;
            }
        }
        decode_intra_chroma_8x8(
            frame,
            mb.mb_x,
            mb.mb_y,
            mb.chroma_mode,
            &mb.chroma_cb_residuals,
            mb.qp_chroma,
            true,
        )?;
        decode_intra_chroma_8x8(
            frame,
            mb.mb_x,
            mb.mb_y,
            mb.chroma_mode,
            &mb.chroma_cr_residuals,
            mb.qp_chroma,
            false,
        )?;
    }
    Ok(())
}

/// Decodes one `I_NxN` luma macroblock (16 separate 4×4 intra
/// predictions) and writes the reconstructed luma samples into the
/// frame.
///
/// `intra4x4_modes` is the array of 16 already-resolved 4×4 intra
/// modes in raster order — see
/// [`crate::h264::intra_mode::resolve_intra4x4_mode`] for the
/// MPM-based resolution from the bitstream's
/// `(prev_intra4x4_pred_mode_flag, rem_intra4x4_pred_mode)` pair.
///
/// Each sub-block is predicted from its own neighbour samples
/// (gathered fresh from the frame after the previous sub-block's
/// reconstructed pixels have been written back), then optionally
/// summed with its residual.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the macroblock extends
/// past the frame.
pub fn decode_intra_4x4_mb(
    frame: &mut Frame,
    mb_x: usize,
    mb_y: usize,
    intra4x4_modes: &[Intra4x4Mode; 16],
    luma_4x4_residuals: &[Residual4x4Scan; 16],
    qp_y: u8,
) -> Result<(), CodecError> {
    let px = mb_x.checked_mul(16).ok_or_else(|| CodecError::InvalidData(
        "h264 decoder: macroblock x position overflows".into(),
    ))?;
    let py = mb_y.checked_mul(16).ok_or_else(|| CodecError::InvalidData(
        "h264 decoder: macroblock y position overflows".into(),
    ))?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 decoder: macroblock at ({mb_x}, {mb_y}) extends past frame ({}x{})",
            frame.width, frame.height,
        )));
    }

    for sub in 0..16 {
        let sub_x_in_mb = (sub % 4) * 4;
        let sub_y_in_mb = (sub / 4) * 4;
        let block_x = px + sub_x_in_mb;
        let block_y = py + sub_y_in_mb;

        // Re-gather neighbours after each sub-block — the previous
        // sub-block may have just written pixels that are this
        // sub-block's left or top neighbour.
        let neighbours = collect_intra4x4_neighbours(frame, block_x, block_y);
        let prediction = predict_4x4(intra4x4_modes[sub], &neighbours);

        let residual_block = match luma_4x4_residuals[sub] {
            None => [[0i32; 4]; 4],
            Some(scan) => dequant_and_inverse_transform_4x4(&scan, qp_y),
        };

        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[j][i]);
                let res = residual_block[j][i];
                let sample = (pred + res).clamp(0, 255) as u8;
                frame.set_luma(block_x + i, block_y + j, sample);
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_luma_residuals() -> [Residual4x4Scan; 16] {
        [None; 16]
    }

    fn empty_chroma_residuals() -> [Residual4x4Scan; 4] {
        [None; 4]
    }

    #[test]
    fn intra_16x16_dc_with_no_neighbours_writes_128_block() {
        let mut frame = Frame::new(16, 16);
        decode_intra_16x16_mb(
            &mut frame,
            0,
            0,
            Intra16x16PredMode::Dc,
            &empty_luma_residuals(),
            28,
        )
        .expect("should decode");
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(frame.get_luma(x, y), Some(128), "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn intra_16x16_vertical_replicates_top_row() {
        let mut frame = Frame::new(32, 32);
        // Pre-populate the row above macroblock (1, 1) with a pattern.
        for x in 0..32 {
            frame.set_luma(x, 15, x as u8 * 4);
        }
        // Also pre-populate left column so neighbours are all present.
        for y in 0..32 {
            frame.set_luma(15, y, 99);
        }
        decode_intra_16x16_mb(
            &mut frame,
            1,
            1,
            Intra16x16PredMode::Vertical,
            &empty_luma_residuals(),
            28,
        )
        .expect("should decode");
        // The block at (16, 16)..(31, 31) should mirror the top row
        // samples at positions (16, 15)..(31, 15).
        for y in 16..32 {
            for x in 16..32 {
                assert_eq!(
                    frame.get_luma(x, y),
                    frame.get_luma(x, 15),
                    "({x}, {y}) should mirror ({x}, 15)",
                );
            }
        }
    }

    #[test]
    fn intra_16x16_rejects_macroblock_past_frame_right_edge() {
        let mut frame = Frame::new(16, 16);
        let err = decode_intra_16x16_mb(
            &mut frame,
            1,
            0,
            Intra16x16PredMode::Dc,
            &empty_luma_residuals(),
            28,
        )
        .expect_err("MB at (1, 0) in a 16x16 frame must error");
        assert!(matches!(err, CodecError::InvalidData(_)));
    }

    #[test]
    fn chroma_dc_with_no_neighbours_writes_128_block() {
        let mut frame = Frame::new(16, 16);
        decode_intra_chroma_8x8(
            &mut frame,
            0,
            0,
            IntraChromaPredMode::Dc,
            &empty_chroma_residuals(),
            28,
            true, // Cb
        )
        .expect("should decode");
        decode_intra_chroma_8x8(
            &mut frame,
            0,
            0,
            IntraChromaPredMode::Dc,
            &empty_chroma_residuals(),
            28,
            false, // Cr
        )
        .expect("should decode");
        for cy in 0..8 {
            for cx in 0..8 {
                assert_eq!(frame.get_cb(cx, cy), Some(128));
                assert_eq!(frame.get_cr(cx, cy), Some(128));
            }
        }
    }

    #[test]
    fn chroma_horizontal_replicates_left_column() {
        let mut frame = Frame::new(32, 32);
        // Pre-populate the column left of chroma block (1, 1).
        for cy in 0..16 {
            frame.set_cb(7, cy, cy as u8 + 10);
        }
        // And the top row so all neighbours are present.
        for cx in 0..16 {
            frame.set_cb(cx, 7, 200);
        }
        decode_intra_chroma_8x8(
            &mut frame,
            1,
            1,
            IntraChromaPredMode::Horizontal,
            &empty_chroma_residuals(),
            28,
            true,
        )
        .expect("should decode");
        for cy in 8..16 {
            for cx in 8..16 {
                assert_eq!(
                    frame.get_cb(cx, cy),
                    frame.get_cb(7, cy),
                    "({cx}, {cy}) should mirror (7, {cy})",
                );
            }
        }
    }

    #[test]
    fn intra_4x4_mb_with_no_neighbours_writes_128_block() {
        let mut frame = Frame::new(16, 16);
        let modes = [Intra4x4Mode::Dc; 16];
        decode_intra_4x4_mb(
            &mut frame,
            0,
            0,
            &modes,
            &empty_luma_residuals(),
            28,
        )
        .expect("should decode");
        // All-DC with no neighbours = 128 fallback for the top-left
        // 4×4.  Subsequent 4×4 blocks within the MB pick up DC from
        // the previously-written 128 pixels and stay 128.
        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(frame.get_luma(x, y), Some(128), "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn intra_4x4_mb_propagates_neighbour_writes_between_sub_blocks() {
        let mut frame = Frame::new(16, 32);
        // Pre-populate the top row (above mb_y == 0 is out-of-frame,
        // so use mb (0, 1) and write the row above it).
        for x in 0..16 {
            frame.set_luma(x, 15, 60);
        }
        // Also fill column left of (0, 1) — but at mb_x == 0 there is
        // no left neighbour anyway, so this is a no-op.
        // Set all 16 4×4 blocks to Vertical mode.
        let modes = [Intra4x4Mode::Vertical; 16];
        decode_intra_4x4_mb(
            &mut frame,
            0,
            1,
            &modes,
            &empty_luma_residuals(),
            28,
        )
        .expect("should decode");
        // Every pixel of macroblock (0, 1) should equal 60: the first
        // row of 4×4 blocks copies the top row, then subsequent rows
        // of 4×4 blocks pick up 60 from the just-decoded rows above.
        for y in 16..32 {
            for x in 0..16 {
                assert_eq!(frame.get_luma(x, y), Some(60), "pixel ({x}, {y})");
            }
        }
    }

    #[test]
    fn inter_p_l0_16x16_with_zero_mv_copies_reference_frame() {
        // Reference frame filled with a recognisable pattern.
        let mut reference = Frame::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                reference.set_luma(x, y, (x ^ y) as u8 * 8);
            }
        }
        for cy in 0..8 {
            for cx in 0..8 {
                reference.set_cb(cx, cy, 64);
                reference.set_cr(cx, cy, 192);
            }
        }
        let mut frame = Frame::new(16, 16);
        decode_inter_p_l0_16x16_mb(
            &mut frame,
            &reference,
            0,
            0,
            0,
            0,
            &empty_luma_residuals(),
            &empty_chroma_residuals(),
            &empty_chroma_residuals(),
            28,
            28,
        )
        .expect("should decode");

        for y in 0..16 {
            for x in 0..16 {
                assert_eq!(frame.get_luma(x, y), reference.get_luma(x, y));
            }
        }
        for cy in 0..8 {
            for cx in 0..8 {
                assert_eq!(frame.get_cb(cx, cy), Some(64));
                assert_eq!(frame.get_cr(cx, cy), Some(192));
            }
        }
    }

    #[test]
    fn decode_i_slice_walks_macroblocks_in_order() {
        // 32x32 frame -> 2x2 macroblocks.  Build four I_16x16 DC
        // macroblocks with no residuals; expect every pixel = 128.
        let mut frame = Frame::new(32, 32);
        let mb = |mb_x: usize, mb_y: usize| IntraMacroblock {
            mb_x,
            mb_y,
            luma: IntraLumaSpec::Sixteen16 {
                pred_mode: Intra16x16PredMode::Dc,
                residuals: empty_luma_residuals(),
            },
            chroma_mode: IntraChromaPredMode::Dc,
            chroma_cb_residuals: empty_chroma_residuals(),
            chroma_cr_residuals: empty_chroma_residuals(),
            qp_y: 28,
            qp_chroma: 28,
        };
        let mbs = vec![mb(0, 0), mb(1, 0), mb(0, 1), mb(1, 1)];
        decode_i_slice(&mut frame, &mbs).expect("should decode slice");

        for y in 0..32 {
            for x in 0..32 {
                assert_eq!(frame.get_luma(x, y), Some(128), "luma ({x}, {y})");
            }
        }
        for cy in 0..16 {
            for cx in 0..16 {
                assert_eq!(frame.get_cb(cx, cy), Some(128), "Cb ({cx}, {cy})");
                assert_eq!(frame.get_cr(cx, cy), Some(128), "Cr ({cx}, {cy})");
            }
        }
    }

    #[test]
    fn nonzero_residual_adds_to_prediction() {
        let mut frame = Frame::new(16, 16);
        // For DC mode with no neighbours, prediction = 128 everywhere.
        // At QP=28 a DC coefficient of 8 dequantizes large enough that
        // the inverse transform's final round-shift leaves a small
        // non-zero residual in the spatial domain, which sums into
        // the corner sub-block but leaves other sub-blocks at 128.
        let mut residuals = empty_luma_residuals();
        let mut scan = [0i32; 16];
        scan[0] = 8;
        residuals[0] = Some(scan);
        decode_intra_16x16_mb(
            &mut frame,
            0,
            0,
            Intra16x16PredMode::Dc,
            &residuals,
            28,
        )
        .expect("should decode");
        let first_pixel = frame.get_luma(0, 0).unwrap();
        assert_ne!(first_pixel, 128, "DC residual should bump the corner");
        assert_eq!(frame.get_luma(8, 8), Some(128), "untouched sub-block");
    }
}
