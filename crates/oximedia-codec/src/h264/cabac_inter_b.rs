//! B-slice CABAC macroblock orchestrator.
//!
//! Twin of [`crate::h264::cabac_inter_mb`] for bi-prediction
//! macroblocks: each partition can draw from list 0, list 1, both
//! (bi-prediction), or be `B_Direct` (motion inferred from
//! collocated / neighbouring blocks).  The bin tree at spec
//! § 9.3.3.1.1.3 selects from 23 codes (plus an intra escape) —
//! see [`crate::h264::cabac_inter::B_MB_TYPE_INFO`].
//!
//! ## Scope of this commit
//!
//! - B_Direct_16x16, B_Skip, intra escape.
//! - All 1- and 2-partition `B_L0_*` / `B_L1_*` / `B_Bi_*` codes
//!   (table entries 1..=21).
//! - **Not yet**: B_8x8 (code 22) sub-mb-type layer.  Direct mode
//!   currently fills L0 ref = 0, L1 ref = 0, both MVs = (0, 0)
//!   rather than running the spec's spatial / temporal direct
//!   inference — a follow-up replaces the placeholder.

use crate::h264::cabac::CabacContext;
use crate::h264::cabac_inter::{decode_b_mb_type, BMbInfo, InterMbResult, InterPartShape, RefListUse, B_MB_TYPE_INFO};
use crate::h264::cabac_inter_mb::MbNeighbours;
use crate::h264::cabac_mb::decode_mb_qp_delta;
use crate::h264::cabac_syntax::{
    decode_cbp_chroma, decode_cbp_luma, decode_mb_skip, decode_mvd, decode_ref_idx,
};
use crate::h264::inter_cache::InterMbDecoded;
use crate::h264::mv_pred::{predict_mv_median, MotionVector, MvPredictionContext};
use crate::h264::slice_header::SliceType;

/// Outcome of a B-slice macroblock decode.
#[derive(Debug, Clone, Copy)]
pub enum BMbOutcome {
    /// B_Skip: no syntax bins past the skip flag.  Caller infers
    /// motion via the direct prediction process.
    Skip,
    /// B_Direct_16x16 or any sub-partition using B_Direct.  Refs +
    /// MVs are inferred — this orchestrator currently fills the
    /// `decoded` struct with placeholders (refs = 0, MVs = (0, 0));
    /// callers running the direct-mode inference replace them.
    Direct {
        /// Decoded inter macroblock state (placeholder MVs + refs).
        decoded: InterMbDecoded,
        /// Signed mb_qp_delta to apply.
        mb_qp_delta: i32,
    },
    /// B inter-coded with explicit MVs.
    Inter {
        /// `0..=22` indexing [`B_MB_TYPE_INFO`].
        mb_type_code: u8,
        /// Decoded inter macroblock state (mv_l0 / mv_l1 / refs).
        decoded: InterMbDecoded,
        /// Signed mb_qp_delta to apply.
        mb_qp_delta: i32,
    },
    /// Macroblock escaped to the intra branch.
    Intra(crate::h264::cabac_syntax::IntraMbType),
}

/// Decodes one B-slice macroblock.
///
/// `num_ref_idx_l0_active` / `num_ref_idx_l1_active` come from the
/// slice header (post-`num_ref_idx_active_override_flag`).  When
/// either equals 1, the corresponding `ref_idx_lN` bin is **not**
/// in the bitstream and the implicit value is 0.
pub fn decode_b_mb_cabac(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    neighbours: &MbNeighbours,
    num_ref_idx_l0_active: u8,
    num_ref_idx_l1_active: u8,
    prev_qp_delta_nonzero: bool,
) -> BMbOutcome {
    // B_Skip flag.
    let skip = decode_mb_skip(
        cabac,
        states,
        SliceType::B,
        neighbours.left_available && !neighbours.left_is_skip,
        neighbours.top_available && !neighbours.top_is_skip,
    );
    if skip != 0 {
        return BMbOutcome::Skip;
    }

    // mb_type bin tree (contexts 27..=39 + intra escape into ctx 32+).
    let r = decode_b_mb_type(cabac, states, false, false);
    let mb_type_code = match r {
        InterMbResult::Inter(c) => c,
        InterMbResult::Intra(it) => return BMbOutcome::Intra(it),
    };
    let info = B_MB_TYPE_INFO[mb_type_code as usize];

    let mut decoded = InterMbDecoded {
        mb_type_code,
        is_intra: false,
        is_skip: false,
        ..InterMbDecoded::default()
    };

    if info.direct {
        // B_Direct_16x16 — spec § 8.4.1.2.2 spatial direct
        // prediction.  Per 8×8 sub-MB, derive ref + MV from the
        // spatial neighbours for each list.  Temporal direct mode
        // requires the collocated picture's MV cache (not yet
        // tracked) and is out of scope.
        apply_b_direct_spatial(neighbours, &mut decoded);
        let cbp_luma = decode_cbp_luma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
        let cbp_chroma = decode_cbp_chroma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
        decoded.cbp = (cbp_chroma << 4) | cbp_luma;
        let mb_qp_delta = if decoded.cbp != 0 {
            decode_mb_qp_delta(cabac, states, prev_qp_delta_nonzero)
        } else {
            0
        };
        return BMbOutcome::Direct { decoded, mb_qp_delta };
    }

    // B_8x8 (code 22) has a sub-mb layer that's out of scope for
    // this commit.  Fall back to a zero-fill so the slice loop
    // doesn't read stale memory; the orchestrator picks it up via
    // BMbInfo::partition_count = 4 + shape = P8x8 and the caller
    // can detect this is a partial decode.
    if mb_type_code == 22 {
        let cbp_luma = decode_cbp_luma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
        let cbp_chroma = decode_cbp_chroma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
        decoded.cbp = (cbp_chroma << 4) | cbp_luma;
        let mb_qp_delta = if decoded.cbp != 0 {
            decode_mb_qp_delta(cabac, states, prev_qp_delta_nonzero)
        } else {
            0
        };
        return BMbOutcome::Inter {
            mb_type_code,
            decoded,
            mb_qp_delta,
        };
    }

    let partitions = partitions_for_shape(info.shape);
    let need_ref_l0 = num_ref_idx_l0_active > 1;
    let need_ref_l1 = num_ref_idx_l1_active > 1;

    for (pi, blocks) in partitions.iter().enumerate() {
        let list_use = info.list_use[pi.min(1)];
        decode_partition_for_list_use(
            cabac,
            states,
            neighbours,
            blocks,
            list_use,
            need_ref_l0,
            need_ref_l1,
            pi,
            info.shape,
            &mut decoded,
        );
    }

    let cbp_luma = decode_cbp_luma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
    let cbp_chroma = decode_cbp_chroma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
    decoded.cbp = (cbp_chroma << 4) | cbp_luma;
    let mb_qp_delta = if decoded.cbp != 0 {
        decode_mb_qp_delta(cabac, states, prev_qp_delta_nonzero)
    } else {
        0
    };

    BMbOutcome::Inter {
        mb_type_code,
        decoded,
        mb_qp_delta,
    }
}

/// Decodes the ref + mvd for one partition under one list-use
/// configuration.  L0 first, then L1 (when applicable).
#[allow(clippy::too_many_arguments)]
fn decode_partition_for_list_use(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    neighbours: &MbNeighbours,
    blocks: &[usize],
    list_use: RefListUse,
    need_ref_l0: bool,
    need_ref_l1: bool,
    partition_index: usize,
    shape: InterPartShape,
    decoded: &mut InterMbDecoded,
) {
    let first_block = blocks[0];

    // L0 leg.
    if matches!(list_use, RefListUse::L0 | RefListUse::BiPred) {
        let ref_idx = if need_ref_l0 {
            let (ref_a, ref_b) = ref_neighbours_for_block(neighbours, first_block);
            decode_ref_idx(
                cabac,
                states,
                SliceType::B,
                ref_a as i32,
                ref_b as i32,
                false,
                false,
            )
            .max(0) as i8
        } else {
            0
        };
        let (mvd_x_a, mvd_x_b, mvd_y_a, mvd_y_b) =
            mvd_neighbours_for_block(neighbours, first_block);
        let mut mvd_x_abs = 0i32;
        let mvd_x = decode_mvd(
            cabac,
            states,
            40,
            (mvd_x_a + mvd_x_b) as i32,
            &mut mvd_x_abs,
        );
        let mut mvd_y_abs = 0i32;
        let mvd_y = decode_mvd(
            cabac,
            states,
            47,
            (mvd_y_a + mvd_y_b) as i32,
            &mut mvd_y_abs,
        );
        let mv_ctx = mv_pred_context(neighbours, shape, partition_index);
        let pred = predict_mv_median(&mv_ctx);
        let mv = (pred.0 + mvd_x, pred.1 + mvd_y);
        for &b in blocks {
            decoded.ref_l0[b] = ref_idx;
            decoded.mv_l0[b] = mv;
            decoded.mvd_abs_l0[b] = [mvd_x_abs.min(255) as u8, mvd_y_abs.min(255) as u8];
        }
    }

    // L1 leg.
    if matches!(list_use, RefListUse::L1 | RefListUse::BiPred) {
        let ref_idx = if need_ref_l1 {
            let (ref_a, ref_b) = ref_neighbours_for_block(neighbours, first_block);
            decode_ref_idx(
                cabac,
                states,
                SliceType::B,
                ref_a as i32,
                ref_b as i32,
                false,
                false,
            )
            .max(0) as i8
        } else {
            0
        };
        let (mvd_x_a, mvd_x_b, mvd_y_a, mvd_y_b) =
            mvd_neighbours_for_block(neighbours, first_block);
        let mut mvd_x_abs = 0i32;
        let mvd_x = decode_mvd(
            cabac,
            states,
            40,
            (mvd_x_a + mvd_x_b) as i32,
            &mut mvd_x_abs,
        );
        let mut mvd_y_abs = 0i32;
        let mvd_y = decode_mvd(
            cabac,
            states,
            47,
            (mvd_y_a + mvd_y_b) as i32,
            &mut mvd_y_abs,
        );
        let mv_ctx = mv_pred_context(neighbours, shape, partition_index);
        let pred = predict_mv_median(&mv_ctx);
        let mv = (pred.0 + mvd_x, pred.1 + mvd_y);
        for &b in blocks {
            decoded.ref_l1[b] = ref_idx;
            decoded.mv_l1[b] = mv;
            decoded.mvd_abs_l1[b] = [mvd_x_abs.min(255) as u8, mvd_y_abs.min(255) as u8];
        }
    }
}

/// Spatial direct prediction for B_Direct_16x16 (spec § 8.4.1.2.2).
///
/// Splits the macroblock into 4 8×8 sub-MBs.  For each sub-MB and
/// each reference list:
///
/// 1. Pick the minimum non-negative ref index from the spatial
///    neighbours (A = left, B = above, C = above-right).  If none
///    of the three has a valid ref for that list, the sub-MB does
///    not use that list (ref = -1).
/// 2. If neither list has a valid ref, fall back to (ref_l0 = 0,
///    ref_l1 = 0) so the partition becomes Bi with both refs at 0.
/// 3. Derive the MV via the standard median predictor on the
///    matching list's neighbour MVs.  This implementation omits
///    the `directZeroPredictionFlag` shortcut (which requires the
///    collocated block's MV from the temporal reference and isn't
///    tracked yet).
fn apply_b_direct_spatial(neighbours: &MbNeighbours, decoded: &mut InterMbDecoded) {
    for q in 0..4 {
        let blocks: [usize; 4] = match q {
            0 => [0, 1, 4, 5],
            1 => [2, 3, 6, 7],
            2 => [8, 9, 12, 13],
            _ => [10, 11, 14, 15],
        };
        let first = blocks[0];
        let row = first / 4;
        let col = first % 4;

        let (ref_a_l0, mv_a_l0) = if col == 0 && neighbours.left_available {
            (neighbours.left_ref[row], neighbours.left_mv[row])
        } else {
            (-1, (0, 0))
        };
        let (ref_b_l0, mv_b_l0) = if row == 0 && neighbours.top_available {
            (neighbours.top_ref[col], neighbours.top_mv[col])
        } else {
            (-1, (0, 0))
        };
        let (ref_c_l0, mv_c_l0) = if row == 0 {
            (
                neighbours.top_right_ref,
                neighbours.top_right_mv.unwrap_or((0, 0)),
            )
        } else {
            (-1, (0, 0))
        };
        let (ref_a_l1, mv_a_l1) = if col == 0 && neighbours.left_available {
            (neighbours.left_ref_l1[row], neighbours.left_mv_l1[row])
        } else {
            (-1, (0, 0))
        };
        let (ref_b_l1, mv_b_l1) = if row == 0 && neighbours.top_available {
            (neighbours.top_ref_l1[col], neighbours.top_mv_l1[col])
        } else {
            (-1, (0, 0))
        };
        let (ref_c_l1, mv_c_l1) = if row == 0 {
            (
                neighbours.top_right_ref_l1,
                neighbours.top_right_mv_l1.unwrap_or((0, 0)),
            )
        } else {
            (-1, (0, 0))
        };

        let mut ref_l0 = min_nonneg_ref(&[ref_a_l0, ref_b_l0, ref_c_l0]);
        let mut ref_l1 = min_nonneg_ref(&[ref_a_l1, ref_b_l1, ref_c_l1]);
        if ref_l0 < 0 && ref_l1 < 0 {
            ref_l0 = 0;
            ref_l1 = 0;
        }

        let mv_l0 = if ref_l0 < 0 {
            (0, 0)
        } else {
            median_mv(
                if ref_a_l0 == ref_l0 { Some(mv_a_l0) } else { None },
                if ref_b_l0 == ref_l0 { Some(mv_b_l0) } else { None },
                if ref_c_l0 == ref_l0 { Some(mv_c_l0) } else { None },
            )
        };
        let mv_l1 = if ref_l1 < 0 {
            (0, 0)
        } else {
            median_mv(
                if ref_a_l1 == ref_l1 { Some(mv_a_l1) } else { None },
                if ref_b_l1 == ref_l1 { Some(mv_b_l1) } else { None },
                if ref_c_l1 == ref_l1 { Some(mv_c_l1) } else { None },
            )
        };

        for &b in &blocks {
            decoded.ref_l0[b] = ref_l0;
            decoded.ref_l1[b] = ref_l1;
            decoded.mv_l0[b] = mv_l0;
            decoded.mv_l1[b] = mv_l1;
        }
    }
}

/// Returns the minimum non-negative ref index in `refs`, or `-1`
/// when every entry is negative.
fn min_nonneg_ref(refs: &[i8]) -> i8 {
    refs.iter().filter(|&&r| r >= 0).min().copied().unwrap_or(-1)
}

/// Median MV across up to three neighbour MVs, ignoring `None`
/// entries.  When only one is present that MV wins outright; when
/// none are present the predictor degrades to `(0, 0)`.
fn median_mv(
    a: Option<MotionVector>,
    b: Option<MotionVector>,
    c: Option<MotionVector>,
) -> MotionVector {
    let ctx = MvPredictionContext {
        left: a,
        above: b,
        above_right: c,
        above_left: None,
    };
    predict_mv_median(&ctx)
}

fn partitions_for_shape(shape: InterPartShape) -> &'static [&'static [usize]] {
    match shape {
        InterPartShape::P16x16 => &[&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]],
        InterPartShape::P16x8 => &[
            &[0, 1, 2, 3, 4, 5, 6, 7],
            &[8, 9, 10, 11, 12, 13, 14, 15],
        ],
        InterPartShape::P8x16 => &[
            &[0, 1, 4, 5, 8, 9, 12, 13],
            &[2, 3, 6, 7, 10, 11, 14, 15],
        ],
        InterPartShape::P8x8 => &[
            &[0, 1, 4, 5],
            &[2, 3, 6, 7],
            &[8, 9, 12, 13],
            &[10, 11, 14, 15],
        ],
    }
}

fn ref_neighbours_for_block(neighbours: &MbNeighbours, block: usize) -> (i8, i8) {
    let row = block / 4;
    let col = block % 4;
    let ref_a = if col == 0 && neighbours.left_available {
        neighbours.left_ref[row]
    } else {
        -1
    };
    let ref_b = if row == 0 && neighbours.top_available {
        neighbours.top_ref[col]
    } else {
        -1
    };
    (ref_a, ref_b)
}

fn mvd_neighbours_for_block(neighbours: &MbNeighbours, block: usize) -> (u8, u8, u8, u8) {
    let row = block / 4;
    let col = block % 4;
    let (mvd_x_a, mvd_y_a) = if col == 0 && neighbours.left_available {
        (neighbours.left_mvd_abs[row][0], neighbours.left_mvd_abs[row][1])
    } else {
        (0, 0)
    };
    let (mvd_x_b, mvd_y_b) = if row == 0 && neighbours.top_available {
        (neighbours.top_mvd_abs[col][0], neighbours.top_mvd_abs[col][1])
    } else {
        (0, 0)
    };
    (mvd_x_a, mvd_x_b, mvd_y_a, mvd_y_b)
}

fn mv_pred_context(
    neighbours: &MbNeighbours,
    shape: InterPartShape,
    partition_index: usize,
) -> MvPredictionContext {
    let first_block = match (shape, partition_index) {
        (InterPartShape::P16x16, _) => 0,
        (InterPartShape::P16x8, 0) => 0,
        (InterPartShape::P16x8, _) => 8,
        (InterPartShape::P8x16, 0) => 0,
        (InterPartShape::P8x16, _) => 2,
        (InterPartShape::P8x8, 0) => 0,
        (InterPartShape::P8x8, 1) => 2,
        (InterPartShape::P8x8, 2) => 8,
        (InterPartShape::P8x8, _) => 10,
    };
    let row = first_block / 4;
    let col = first_block % 4;
    let left = if col == 0 && neighbours.left_available {
        Some(neighbours.left_mv[row])
    } else {
        None
    };
    let above = if row == 0 && neighbours.top_available {
        Some(neighbours.top_mv[col])
    } else {
        None
    };
    let above_right = if row == 0 {
        neighbours.top_right_mv
    } else {
        None
    };
    MvPredictionContext {
        left,
        above,
        above_right,
        above_left: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::cabac::init_contexts;

    fn buf() -> Vec<u8> {
        let mut v = vec![0x55u8; 256];
        v[0] = 0x40;
        v
    }

    fn empty_neighbours() -> MbNeighbours {
        MbNeighbours {
            left_available: false,
            top_available: false,
            top_right_available: false,
            left_mv: [(0, 0); 4],
            left_ref: [-1; 4],
            left_mvd_abs: [[0; 2]; 4],
            left_mv_l1: [(0, 0); 4],
            left_ref_l1: [-1; 4],
            left_mvd_abs_l1: [[0; 2]; 4],
            left_is_skip: false,
            top_mv: [(0, 0); 4],
            top_ref: [-1; 4],
            top_mvd_abs: [[0; 2]; 4],
            top_mv_l1: [(0, 0); 4],
            top_ref_l1: [-1; 4],
            top_mvd_abs_l1: [[0; 2]; 4],
            top_is_skip: false,
            top_right_mv: None,
            top_right_ref: -1,
            top_right_mv_l1: None,
            top_right_ref_l1: -1,
            left_cbp: 0,
            top_cbp: 0,
            left_chroma_pred_nonzero: false,
            top_chroma_pred_nonzero: false,
        }
    }

    #[test]
    fn b_mb_decode_returns_some_outcome() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::B, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let nb = empty_neighbours();
        let outcome = decode_b_mb_cabac(&mut cabac, &mut states, &nb, 1, 1, false);
        match outcome {
            BMbOutcome::Skip => {}
            BMbOutcome::Direct { .. } => {}
            BMbOutcome::Inter { mb_type_code, .. } => assert!(mb_type_code <= 22),
            BMbOutcome::Intra(_) => {}
        }
    }
}
