//! Intra macroblock reconstruction from CABAC-decoded data.
//!
//! Companion to [`crate::h264::reconstruct_inter`] for the intra
//! macroblock paths that come out of the CABAC slice loop.  The
//! existing `decoder.rs` intra reconstructors take residual
//! coefficients in **scan order** (`Residual4x4Scan = Option<[i32; 16]>`);
//! the CABAC residual decoder produces them in **position order**
//! (row-major flat).  Rather than convert back-and-forth at every
//! call site, this module owns position-order intra reconstruction
//! plus the correct dequantisation sequence for `I_16x16` luma DC
//! and chroma DC blocks — both of which the spec dequantises via
//! a separate Hadamard pass that the per-4×4 dequant must not
//! repeat.
//!
//! Spec references:
//! - § 8.5.2  `I_PCM` (handled elsewhere — see [`crate::h264::pcm`]).
//! - § 8.5.6 / 8.5.10  Inverse Hadamard for I_16x16 luma DC + 4:2:0
//!   chroma DC.
//! - § 8.5.12  4×4 inverse integer transform.
//! - § 8.3   Intra prediction.

use crate::h264::frame::{
    collect_chroma_8x8_neighbours, collect_intra16x16_neighbours, collect_intra4x4_neighbours,
    Frame,
};
use crate::h264::intra_pred::{
    predict_16x16, predict_4x4, predict_chroma_8x8, Intra16x16Neighbours, Intra4x4Mode,
};
use crate::h264::macroblock::{Intra16x16PredMode, IntraChromaPredMode};
use crate::h264::transform::{
    dequantize_4x4, inverse_hadamard_2x2_chroma_dc, inverse_hadamard_4x4_luma_dc,
    inverse_transform_4x4,
};
use crate::CodecError;

/// Reconstructs one `I_NxN` macroblock from CABAC-decoded data.
///
/// `modes` is the resolved per-4×4 intra prediction mode (post-MPM
/// derivation).  `luma_4x4` is the per-block residual in row-major
/// position order.  `qp_y` is the effective luma QP at this
/// macroblock.
pub fn reconstruct_intra_4x4_mb_cabac(
    frame: &mut Frame,
    mb_x: usize,
    mb_y: usize,
    modes: &[Intra4x4Mode; 16],
    luma_4x4: &[[i32; 16]; 16],
    qp_y: u8,
) -> Result<(), CodecError> {
    let px = mb_x.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 intra_cabac: mb_x overflow".into())
    })?;
    let py = mb_y.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 intra_cabac: mb_y overflow".into())
    })?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 intra_cabac: mb ({mb_x}, {mb_y}) extends past frame"
        )));
    }

    for sub in 0..16 {
        let sub_x_in_mb = (sub % 4) * 4;
        let sub_y_in_mb = (sub / 4) * 4;
        let block_x = px + sub_x_in_mb;
        let block_y = py + sub_y_in_mb;
        let neighbours = collect_intra4x4_neighbours(frame, block_x, block_y);
        let prediction = predict_4x4(modes[sub], &neighbours);
        let residual_block = dequant_and_idct_position(&luma_4x4[sub], qp_y);
        for j in 0..4 {
            for i in 0..4 {
                let v = (i32::from(prediction[j][i]) + residual_block[j][i]).clamp(0, 255) as u8;
                frame.set_luma(block_x + i, block_y + j, v);
            }
        }
    }
    Ok(())
}

/// Reconstructs one `I_16x16` macroblock from CABAC-decoded data.
///
/// `pred_mode` is the macroblock-level intra16x16 prediction mode.
/// `luma_dc` is the 16-entry DC block in row-major position order
/// (the 16 DC coefficients of the 4×4 sub-blocks arranged into a
/// 4×4 grid; the Hadamard transform is applied here).
/// `luma_4x4` is the per-block AC residual in position order (15
/// AC coeffs + 0 at the DC slot).  The DC values from the Hadamard
/// pass are injected into each AC block before IDCT.
pub fn reconstruct_intra_16x16_mb_cabac(
    frame: &mut Frame,
    mb_x: usize,
    mb_y: usize,
    pred_mode: Intra16x16PredMode,
    luma_dc: &[i32; 16],
    luma_4x4: &[[i32; 16]; 16],
    qp_y: u8,
) -> Result<(), CodecError> {
    let px = mb_x.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 intra_cabac: mb_x overflow".into())
    })?;
    let py = mb_y.checked_mul(16).ok_or_else(|| {
        CodecError::InvalidData("h264 intra_cabac: mb_y overflow".into())
    })?;
    if px + 16 > frame.width || py + 16 > frame.height {
        return Err(CodecError::InvalidData(format!(
            "h264 intra_cabac: mb ({mb_x}, {mb_y}) extends past frame"
        )));
    }

    // Inverse Hadamard 4×4 on the luma DC block (dequantises in
    // the process — the per-sub-block dequant below must NOT touch
    // position 0 again, which is why we use the dequant-then-inject
    // sequence).
    let dc_grid = position_flat_to_4x4(luma_dc);
    let dc_dequant = inverse_hadamard_4x4_luma_dc(&dc_grid, qp_y);

    // Predict using the macroblock-level 16×16 mode.
    let neighbours = collect_intra16x16_neighbours(frame, mb_x, mb_y);
    let prediction = predict_16x16(pred_mode, &neighbours_into_intra16(&neighbours));

    for sub in 0..16 {
        let sub_x_in_mb = (sub % 4) * 4;
        let sub_y_in_mb = (sub / 4) * 4;
        let dc_row = sub / 4;
        let dc_col = sub % 4;
        let residual_block =
            dequant_ac_then_inject_dc(&luma_4x4[sub], dc_dequant[dc_row][dc_col], qp_y);
        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[sub_y_in_mb + j][sub_x_in_mb + i]);
                let v = (pred + residual_block[j][i]).clamp(0, 255) as u8;
                frame.set_luma(px + sub_x_in_mb + i, py + sub_y_in_mb + j, v);
            }
        }
    }
    Ok(())
}

/// Reconstructs the chroma 8×8 plane of an intra macroblock from
/// CABAC-decoded data.  Runs the inverse Hadamard 2×2 on the
/// chroma DC, injects each DC into the corresponding 4×4 AC block,
/// dequants the AC only, IDCTs, sums with the chroma intra
/// prediction, and writes the samples.
///
/// `chroma_dc` holds the 4 chroma DC coefficients in raster order
/// (`[0]` = (0,0), `[1]` = (0,1), `[2]` = (1,0), `[3]` = (1,1)).
/// `chroma_ac` holds 4 chroma 4×4 AC blocks in row-major position
/// order (DC slot zero).
pub fn reconstruct_intra_chroma_8x8_cabac(
    frame: &mut Frame,
    mb_x: usize,
    mb_y: usize,
    pred_mode: IntraChromaPredMode,
    chroma_dc: &[i32; 4],
    chroma_ac: &[[i32; 16]; 4],
    qp_chroma: u8,
    is_cb: bool,
) -> Result<(), CodecError> {
    let cx = mb_x.checked_mul(8).ok_or_else(|| {
        CodecError::InvalidData("h264 intra_cabac: chroma x overflow".into())
    })?;
    let cy = mb_y.checked_mul(8).ok_or_else(|| {
        CodecError::InvalidData("h264 intra_cabac: chroma y overflow".into())
    })?;
    if cx + 8 > frame.chroma_width() || cy + 8 > frame.chroma_height() {
        return Err(CodecError::InvalidData(format!(
            "h264 intra_cabac: chroma ({mb_x}, {mb_y}) extends past plane"
        )));
    }

    // Inverse Hadamard 2×2 — dequants the 4 DC values.
    let dc_2x2 = [
        [chroma_dc[0], chroma_dc[1]],
        [chroma_dc[2], chroma_dc[3]],
    ];
    let dc_dequant = inverse_hadamard_2x2_chroma_dc(dc_2x2, qp_chroma);

    let neighbours = collect_chroma_8x8_neighbours(frame, mb_x, mb_y, is_cb);
    let prediction = predict_chroma_8x8(pred_mode, &neighbours);

    for sub in 0..4 {
        let sub_x = (sub % 2) * 4;
        let sub_y = (sub / 2) * 4;
        let dc_row = sub / 2;
        let dc_col = sub % 2;
        let residual_block = dequant_ac_then_inject_dc(
            &chroma_ac[sub],
            dc_dequant[dc_row][dc_col],
            qp_chroma,
        );
        for j in 0..4 {
            for i in 0..4 {
                let pred = i32::from(prediction[sub_y + j][sub_x + i]);
                let v = (pred + residual_block[j][i]).clamp(0, 255) as u8;
                if is_cb {
                    frame.set_cb(cx + sub_x + i, cy + sub_y + j, v);
                } else {
                    frame.set_cr(cx + sub_x + i, cy + sub_y + j, v);
                }
            }
        }
    }
    Ok(())
}

/// Performs dequantisation + IDCT on a 4×4 block whose coefficients
/// are stored in row-major position order.  Used for I_NxN luma
/// blocks where the DC slot is part of the AC and must be
/// dequantised normally.
fn dequant_and_idct_position(block: &[i32; 16], qp: u8) -> [[i32; 4]; 4] {
    let mut grid = position_flat_to_4x4(block);
    dequantize_4x4(&mut grid, qp);
    inverse_transform_4x4(&grid)
}

/// Dequantises the AC coefficients of a 4×4 block (positions 1..15),
/// injects an *already-dequantised* DC value at position 0, then
/// runs the inverse 4×4 transform.  This is the sequence the spec
/// requires for I_16x16 luma blocks and 4:2:0 chroma blocks where
/// the DC has already been processed through its own Hadamard +
/// scaling step.
fn dequant_ac_then_inject_dc(block: &[i32; 16], dc: i32, qp: u8) -> [[i32; 4]; 4] {
    let mut grid = position_flat_to_4x4(block);
    // Position 0 is the DC slot — clear it before dequant so the
    // injected DC value doesn't get scaled twice.
    grid[0][0] = 0;
    dequantize_4x4(&mut grid, qp);
    grid[0][0] = dc;
    inverse_transform_4x4(&grid)
}

fn position_flat_to_4x4(flat: &[i32; 16]) -> [[i32; 4]; 4] {
    let mut grid = [[0i32; 4]; 4];
    for k in 0..16 {
        grid[k / 4][k % 4] = flat[k];
    }
    grid
}

/// Adapts the [`crate::h264::frame::Intra16x16Neighbours`] view to
/// the [`Intra16x16Neighbours`] used by
/// [`crate::h264::intra_pred::predict_16x16`].  Currently a no-op
/// pass-through since both modules share the same struct.
fn neighbours_into_intra16(n: &Intra16x16Neighbours) -> Intra16x16Neighbours {
    *n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intra_4x4_recon_runs_without_panic_on_empty_residual() {
        let mut frame = Frame::new(16, 16);
        let modes = [Intra4x4Mode::Dc; 16];
        let luma = [[0i32; 16]; 16];
        reconstruct_intra_4x4_mb_cabac(&mut frame, 0, 0, &modes, &luma, 26).unwrap();
    }

    #[test]
    fn intra_16x16_recon_runs_without_panic_on_empty_residual() {
        let mut frame = Frame::new(16, 16);
        let luma_dc = [0i32; 16];
        let luma = [[0i32; 16]; 16];
        reconstruct_intra_16x16_mb_cabac(
            &mut frame,
            0,
            0,
            Intra16x16PredMode::Dc,
            &luma_dc,
            &luma,
            26,
        )
        .unwrap();
    }

    #[test]
    fn intra_chroma_recon_runs_without_panic_on_empty_residual() {
        let mut frame = Frame::new(16, 16);
        let chroma_dc = [0i32; 4];
        let chroma_ac = [[0i32; 16]; 4];
        reconstruct_intra_chroma_8x8_cabac(
            &mut frame,
            0,
            0,
            IntraChromaPredMode::Dc,
            &chroma_dc,
            &chroma_ac,
            26,
            true,
        )
        .unwrap();
    }
}
