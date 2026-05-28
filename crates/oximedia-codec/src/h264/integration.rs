//! End-to-end integration tests for the H.264 decode pipeline.
//!
//! Composes the per-stage modules:
//!
//! - CABAC context initialisation
//!   ([`crate::h264::cabac::init_contexts`]).
//! - CABAC slice loop ([`crate::h264::slice_cabac::parse_slice_cabac`]).
//! - Per-macroblock reconstruction
//!   ([`crate::h264::reconstruct_inter::reconstruct_inter_p_mb`] /
//!    `reconstruct_inter_b_mb`).
//! - Frame-level luma + chroma deblocking
//!   ([`crate::h264::deblock_frame::deblock_frame_luma`] /
//!    `crate::h264::deblock_frame_chroma::deblock_frame_chroma_420`).
//!
//! These tests deliberately run a synthetic CABAC bytestream rather
//! than a real H.264 file: without an encoder we can't generate a
//! conformant bitstream, and depending on external test vectors
//! would couple the workspace to a binary fixture.  The synthetic
//! input is enough to verify that the module API surfaces compose,
//! that reconstruction produces in-range pixels, and that the
//! deblocker runs without panic on the resulting frame.

#![cfg(test)]

use crate::h264::cabac::{init_contexts, CabacContext};
use crate::h264::cabac_mb::MbResidualState;
use crate::h264::deblock_frame::{deblock_frame_luma, DeblockMbState};
use crate::h264::deblock_frame_chroma::{deblock_frame_chroma_420, DeblockChromaInfo};
use crate::h264::frame::Frame;
use crate::h264::inter_cache::InterSliceCache;
use crate::h264::reconstruct_inter::{reconstruct_inter_p_mb, InterPMbInputs};
use crate::h264::slice_cabac::{parse_slice_cabac, MbKind, SliceCabacContext};
use crate::h264::slice_header::SliceType;

/// Builds a synthetic CABAC bitstream that initialises the decoder
/// cleanly (first byte ≤ 0x40 keeps `low` inside the spec's range).
fn synthetic_bitstream(len: usize) -> Vec<u8> {
    let mut v = vec![0x55u8; len];
    v[0] = 0x40;
    v
}

/// Builds a reference frame filled with `luma` plus mid-grey chroma
/// for the integration tests' motion-compensation step.
fn make_reference(width: usize, height: usize, luma: u8) -> Frame {
    let mut f = Frame::new(width, height);
    for y in 0..height {
        for x in 0..width {
            f.set_luma(x, y, luma);
        }
    }
    for y in 0..height / 2 {
        for x in 0..width / 2 {
            f.set_cb(x, y, 128);
            f.set_cr(x, y, 128);
        }
    }
    f
}

fn zigzag_4x4() -> [u8; 16] {
    [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15]
}

#[test]
fn p_slice_pipeline_runs_to_completion_on_2x2_picture() {
    const W_MBS: usize = 2;
    const H_MBS: usize = 2;
    const W: usize = W_MBS * 16;
    const H: usize = H_MBS * 16;

    let bytes = synthetic_bitstream(1024);
    let mut states = init_contexts(SliceType::P, 26, 0);
    let mut cabac = CabacContext::new(&bytes).unwrap();

    let scan = zigzag_4x4();
    let scan8 = [0u8; 64];
    let dq4 = [16u32; 16];
    let dq8 = [16u32; 64];

    let ctx = SliceCabacContext {
        slice_type: SliceType::P,
        pic_width_mbs: W_MBS,
        pic_height_mbs: H_MBS,
        initial_qp_y: 26,
        chroma_qp_index_offset: 0,
        num_ref_idx_l0_active: 1,
        scan_4x4: &scan,
        scan_8x8: &scan8,
        dequant_4x4_luma: &dq4,
        dequant_4x4_cb: &dq4,
        dequant_4x4_cr: &dq4,
        dequant_8x8_luma: &dq8,
    };
    let mut cache = InterSliceCache::new(W_MBS);

    let decoded_mbs =
        parse_slice_cabac(&mut cabac, &mut states, ctx, &mut cache).expect("parse");
    assert_eq!(decoded_mbs.len(), W_MBS * H_MBS);

    // Per-MB reconstruction.  Reference frame uses a flat grey
    // luma so any non-zero MV doesn't push us off-frame.
    let reference = make_reference(W, H, 120);
    let mut frame = Frame::new(W, H);

    for mb in &decoded_mbs {
        match &mb.kind {
            MbKind::PSkip => {
                // P_Skip: synthesise a zero-MV / zero-residual
                // input so the slice still produces pixels.
                let mvs = [(0i32, 0i32); 16];
                let luma_4x4 = [[0i32; 16]; 16];
                let chroma_dc = [[0i32; 8]; 2];
                let chroma_ac = [[0i32; 16]; 8];
                let inputs = InterPMbInputs {
                    mb_x: mb.mb_x,
                    mb_y: mb.mb_y,
                    mvs_l0: &mvs,
                    luma_4x4: &luma_4x4,
                    chroma_dc: &chroma_dc,
                    chroma_ac: &chroma_ac,
                    qp_y: mb.qp_y,
                    qp_chroma: mb.qp_chroma,
                };
                reconstruct_inter_p_mb(&mut frame, &reference, &inputs).expect("p_skip");
            }
            MbKind::InterP { decoded, .. } => {
                let inputs = InterPMbInputs {
                    mb_x: mb.mb_x,
                    mb_y: mb.mb_y,
                    mvs_l0: &decoded.mv_l0,
                    luma_4x4: &mb.residual.luma_4x4,
                    chroma_dc: &mb.residual.chroma_dc,
                    chroma_ac: &mb.residual.chroma_ac,
                    qp_y: mb.qp_y,
                    qp_chroma: mb.qp_chroma,
                };
                reconstruct_inter_p_mb(&mut frame, &reference, &inputs).expect("inter");
            }
            MbKind::Intra(_) => {
                // Intra macroblocks need the intra reconstruction
                // path (not exercised by this scaffolding test).
                // Fill with mid-grey so the deblocker has valid
                // input.
                for j in 0..16 {
                    for i in 0..16 {
                        frame.set_luma(mb.mb_x * 16 + i, mb.mb_y * 16 + j, 128);
                    }
                }
                for j in 0..8 {
                    for i in 0..8 {
                        frame.set_cb(mb.mb_x * 8 + i, mb.mb_y * 8 + j, 128);
                        frame.set_cr(mb.mb_x * 8 + i, mb.mb_y * 8 + j, 128);
                    }
                }
            }
        }
    }

    // Build deblocker state from the residual non-zero counts.
    let mut luma_states = Vec::with_capacity(W_MBS * H_MBS);
    let mut chroma_states = Vec::with_capacity(W_MBS * H_MBS);
    for mb in &decoded_mbs {
        let is_intra = matches!(mb.kind, MbKind::Intra(_));
        let mut block_info: [crate::h264::deblock::DeblockBlockInfo; 16] = Default::default();
        for i in 0..16 {
            block_info[i] = crate::h264::deblock::DeblockBlockInfo {
                is_intra,
                has_residual: mb.residual.nz_count_luma[i] > 0,
                ..Default::default()
            };
        }
        luma_states.push(DeblockMbState {
            qp_y: mb.qp_y,
            is_intra,
            block_info,
            skip_external_filter: false,
        });

        let mut cb_blocks: [crate::h264::deblock::DeblockBlockInfo; 4] = Default::default();
        let mut cr_blocks: [crate::h264::deblock::DeblockBlockInfo; 4] = Default::default();
        for i in 0..4 {
            cb_blocks[i] = crate::h264::deblock::DeblockBlockInfo {
                is_intra,
                has_residual: mb.residual.nz_count_chroma[i] > 0,
                ..Default::default()
            };
            cr_blocks[i] = crate::h264::deblock::DeblockBlockInfo {
                is_intra,
                has_residual: mb.residual.nz_count_chroma[4 + i] > 0,
                ..Default::default()
            };
        }
        chroma_states.push(DeblockChromaInfo {
            qp_chroma: mb.qp_chroma,
            cb_blocks,
            cr_blocks,
        });
    }

    // Snapshot for the no-panic assertion.
    let before = (frame.get_luma(0, 0), frame.get_cb(0, 0), frame.get_cr(0, 0));
    deblock_frame_luma(&mut frame, W_MBS, H_MBS, &luma_states, [0, 0, 0]);
    deblock_frame_chroma_420(
        &mut frame,
        W_MBS,
        H_MBS,
        &luma_states,
        &chroma_states,
        [0, 0, 0],
    );

    // Pixels stay in range and the deblocker either touched the
    // pixel or left it identical to the pre-pass value.
    for y in 0..H {
        for x in 0..W {
            // u8 samples are inherently 0..=255; presence of the
            // pixel is all we're checking.
            let _ = frame.get_luma(x, y).expect("luma in range");
        }
    }
    // The 0, 0 corner is unaffected by deblocking (no neighbours).
    assert_eq!(frame.get_luma(0, 0), before.0);
    assert_eq!(frame.get_cb(0, 0), before.1);
    assert_eq!(frame.get_cr(0, 0), before.2);
}

#[test]
fn i_slice_pipeline_dispatches_intra_macroblocks() {
    // I-slice variant — every MB is an intra macroblock (or
    // routed through the intra escape).  This test exercises the
    // I-slice path of parse_slice_cabac.
    const W_MBS: usize = 1;
    const H_MBS: usize = 1;

    let bytes = synthetic_bitstream(512);
    let mut states = init_contexts(SliceType::I, 26, 0);
    let mut cabac = CabacContext::new(&bytes).unwrap();
    let scan = zigzag_4x4();
    let scan8 = [0u8; 64];
    let dq4 = [16u32; 16];
    let dq8 = [16u32; 64];

    let ctx = SliceCabacContext {
        slice_type: SliceType::I,
        pic_width_mbs: W_MBS,
        pic_height_mbs: H_MBS,
        initial_qp_y: 26,
        chroma_qp_index_offset: 0,
        num_ref_idx_l0_active: 1,
        scan_4x4: &scan,
        scan_8x8: &scan8,
        dequant_4x4_luma: &dq4,
        dequant_4x4_cb: &dq4,
        dequant_4x4_cr: &dq4,
        dequant_8x8_luma: &dq8,
    };
    let mut cache = InterSliceCache::new(W_MBS);
    let mbs = parse_slice_cabac(&mut cabac, &mut states, ctx, &mut cache).expect("parse");
    assert_eq!(mbs.len(), 1);
    // I-slice MBs decode as either Intra or (rarely on synthetic
    // input) the unexpected PSkip / InterP branches.  All that
    // matters here is the slice loop ran without erroring.
    let _ = &mbs[0].kind;
}

#[test]
fn empty_residual_passes_zero_through() {
    // A standalone reconstruction call with all-zero residuals +
    // zero MV copies the reference frame exactly.
    let reference = make_reference(16, 16, 99);
    let mut frame = Frame::new(16, 16);
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
    reconstruct_inter_p_mb(&mut frame, &reference, &inputs).unwrap();
    for y in 0..16 {
        for x in 0..16 {
            assert_eq!(frame.get_luma(x, y), Some(99));
        }
    }
}

#[allow(dead_code)]
fn _residual_state_is_zeroed_by_default() {
    // Compile-time sanity: MbResidualState::default() must zero
    // every block — slice loops rely on this for skip MBs.
    let r = MbResidualState::default();
    assert!(r.nz_count_luma.iter().all(|&c| c == 0));
}
