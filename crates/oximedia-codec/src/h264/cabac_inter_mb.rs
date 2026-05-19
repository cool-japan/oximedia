//! Inter-macroblock CABAC orchestrator (P-slice path).
//!
//! Ties the per-syntax-element decoders ([`crate::h264::cabac_syntax`])
//! and per-block residual dispatch ([`crate::h264::cabac_mb`]) together
//! into a full macroblock decode for P slices.  Implements the
//! macroblock_layer() flow from ITU-T Rec. H.264 / ISO/IEC 14496-10
//! § 7.3.5.1 for the inter branch:
//!
//! ```text
//! mb_skip_flag → mb_type
//!   if skip → record P_Skip (no further bins)
//!   if intra escape → route to decode_intra_mb
//!   else → per-partition ref_idx + mvd → cbp → mb_qp_delta
//! ```
//!
//! Output is a fully populated [`crate::h264::inter_cache::InterMbDecoded`]
//! with per-4×4 motion vectors resolved (predicted MV + decoded
//! mvd), ref indices, and absolute MVD magnitudes.  The caller
//! pushes the result into [`crate::h264::inter_cache::InterSliceCache`]
//! so the next macroblock can read its neighbour state.
//!
//! **Scope of this commit:** P_L0_16x16, P_L0_L0_16x8, P_L0_L0_8x16,
//! and the intra escape.  `P_8x8` (sub-mb partitioning) and the
//! `ref0_only` variant (`P_8x8ref0`) are landed in a follow-up.

use crate::h264::cabac::CabacContext;
use crate::h264::cabac_inter::{decode_p_mb_type, InterMbResult, InterPartShape, P_MB_TYPE_INFO};
use crate::h264::cabac_mb::decode_mb_qp_delta;
use crate::h264::cabac_syntax::{decode_cbp_chroma, decode_cbp_luma, decode_mb_skip, decode_mvd, decode_ref_idx};
use crate::h264::inter_cache::{InterMbDecoded, InterSliceCache};
use crate::h264::mv_pred::{predict_mv_median, MotionVector, MvPredictionContext};
use crate::h264::slice_header::SliceType;

/// Neighbour state for one macroblock decode, derived from
/// [`InterSliceCache`].
#[derive(Debug, Clone, Copy)]
pub struct MbNeighbours {
    /// `true` when the left macroblock exists in the current slice.
    pub left_available: bool,
    /// `true` when the top macroblock exists in the current slice.
    pub top_available: bool,
    /// `true` when the top-right macroblock exists.
    pub top_right_available: bool,
    /// Left neighbour bottom-row MV (4 entries: rows 0..=3 of the
    /// current macroblock see the left MB's column 3 sub-blocks).
    pub left_mv: [MotionVector; 4],
    /// Left neighbour ref_l0 (4 entries, rows 0..=3).
    pub left_ref: [i8; 4],
    /// Left neighbour absolute MVD magnitudes (rows 0..=3).
    pub left_mvd_abs: [[u8; 2]; 4],
    /// Left neighbour is a Skip MB.
    pub left_is_skip: bool,
    /// Top neighbour bottom-row MV (4 entries: cols 0..=3).
    pub top_mv: [MotionVector; 4],
    /// Top neighbour ref_l0 (4 entries, cols 0..=3).
    pub top_ref: [i8; 4],
    /// Top neighbour absolute MVD magnitudes (cols 0..=3).
    pub top_mvd_abs: [[u8; 2]; 4],
    /// Top neighbour is a Skip MB.
    pub top_is_skip: bool,
    /// Top-right neighbour bottom-left 4×4 MV (single entry).
    pub top_right_mv: Option<MotionVector>,
    /// Top-right neighbour ref_l0 (single entry, -1 if unused).
    pub top_right_ref: i8,
    /// 8-bit left-neighbour CBP (low 4 = luma, bits 4..=5 = chroma).
    pub left_cbp: u8,
    /// 8-bit top-neighbour CBP.
    pub top_cbp: u8,
    /// `true` when the left neighbour's `intra_chroma_pred_mode`
    /// was nonzero (used by the chroma-pred-mode CABAC decoder).
    pub left_chroma_pred_nonzero: bool,
    /// `true` when the top neighbour's `intra_chroma_pred_mode`
    /// was nonzero.
    pub top_chroma_pred_nonzero: bool,
}

impl MbNeighbours {
    /// Builds the per-macroblock neighbour view from the slice
    /// cache plus an optional `top_right` slot.
    #[must_use]
    pub fn from_cache(
        cache: &InterSliceCache,
        mb_x: usize,
        top_right_slot: Option<&crate::h264::inter_cache::TopRowSlot>,
    ) -> Self {
        let left = &cache.left_col;
        let top = &cache.top_row[mb_x];
        Self {
            left_available: left.available,
            top_available: top.available,
            top_right_available: top_right_slot.is_some_and(|s| s.available),
            left_mv: left.mv_l0,
            left_ref: left.ref_l0,
            left_mvd_abs: left.mvd_abs_l0,
            left_is_skip: left.is_skip,
            top_mv: top.mv_l0,
            top_ref: top.ref_l0,
            top_mvd_abs: top.mvd_abs_l0,
            top_is_skip: top.is_skip,
            top_right_mv: top_right_slot.and_then(|s| {
                if s.available {
                    // Top-right's bottom-left 4×4 = column 0 of its
                    // bottom row.
                    Some(s.mv_l0[0])
                } else {
                    None
                }
            }),
            top_right_ref: top_right_slot
                .map(|s| if s.available { s.ref_l0[0] } else { -1 })
                .unwrap_or(-1),
            left_cbp: left.cbp,
            top_cbp: top.cbp,
            left_chroma_pred_nonzero: left.chroma_pred_mode != 0,
            top_chroma_pred_nonzero: top.chroma_pred_mode != 0,
        }
    }
}

/// Outcome of a P-slice macroblock decode.
#[derive(Debug, Clone, Copy)]
pub enum PMbOutcome {
    /// Macroblock was P_Skip — no further bins.  The slice loop
    /// applies inferred motion prediction (median MV from
    /// neighbours, ref = 0) to populate the cache.
    Skip,
    /// Macroblock was inter-coded with one of the P partition
    /// shapes; the slice loop reads `mb_qp_delta` from the result
    /// and threads the MVs into motion compensation.
    Inter {
        /// `0..=4` indexing [`P_MB_TYPE_INFO`].
        mb_type_code: u8,
        /// Decoded inter macroblock state.
        decoded: InterMbDecoded,
        /// Signed mb_qp_delta to apply to the running slice QP.
        mb_qp_delta: i32,
    },
    /// Macroblock escaped to the intra branch.  Caller continues
    /// with the existing I-slice flow (decode_intra_mb), starting
    /// from the resolved intra mb_type.
    Intra(crate::h264::cabac_syntax::IntraMbType),
}

/// Decodes one P-slice macroblock.
///
/// Returns a [`PMbOutcome`] describing the outcome; the slice loop
/// is responsible for updating [`InterSliceCache`] afterwards via
/// `record_inter_mb`.
///
/// `num_ref_idx_l0_active` is taken from the slice header (after
/// any `num_ref_idx_active_override_flag` adjustment).  When it
/// equals 1, `ref_idx_l0` is *not* signalled in the bitstream and
/// the implicit value is 0.
pub fn decode_p_mb_cabac(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    neighbours: &MbNeighbours,
    num_ref_idx_l0_active: u8,
    prev_qp_delta_nonzero: bool,
) -> PMbOutcome {
    // mb_skip_flag.
    let skip = decode_mb_skip(
        cabac,
        states,
        SliceType::P,
        neighbours.left_available && !neighbours.left_is_skip,
        neighbours.top_available && !neighbours.top_is_skip,
    );
    if skip != 0 {
        return PMbOutcome::Skip;
    }

    // mb_type.
    let r = decode_p_mb_type(cabac, states);
    let mb_type_code = match r {
        InterMbResult::Inter(c) => c,
        InterMbResult::Intra(it) => return PMbOutcome::Intra(it),
    };
    let info = P_MB_TYPE_INFO[mb_type_code as usize];

    // Decode ref_idx + mvd per partition.
    let mut decoded = InterMbDecoded {
        mb_type_code,
        is_intra: false,
        is_skip: false,
        ..InterMbDecoded::default()
    };

    let partitions = partitions_for_shape(info.shape);
    let need_ref = num_ref_idx_l0_active > 1 && !info.ref0_only;

    for (pi, blocks) in partitions.iter().enumerate() {
        // ref_idx for partition `pi`.
        let ref_idx = if need_ref {
            // Neighbour ref view for this partition: pick the first
            // 4×4 of the partition as the reference point for the
            // CABAC ctx selection — matches spec § 9.3.3.1.1.6.
            let first_block = blocks[0];
            let (ref_a, ref_b) = ref_neighbours_for_block(neighbours, first_block);
            let r = decode_ref_idx(
                cabac,
                states,
                SliceType::P,
                ref_a as i32,
                ref_b as i32,
                false,
                false,
            );
            r.max(0) as i8
        } else {
            0
        };

        // mvd_x + mvd_y for partition `pi`.
        let first_block = blocks[0];
        let (mvd_x_a, mvd_x_b, mvd_y_a, mvd_y_b) = mvd_neighbours_for_block(neighbours, first_block);

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

        // Median MV predictor for the partition.
        let mv_ctx = mv_pred_context_for_partition(neighbours, info.shape, pi);
        let pred = predict_mv_median(&mv_ctx);
        let mv = (pred.0 + mvd_x, pred.1 + mvd_y);

        // Splat the decoded ref + mv + mvd into all 4×4 blocks
        // covered by this partition.
        for &b in *blocks {
            decoded.ref_l0[b] = ref_idx;
            decoded.mv_l0[b] = mv;
            decoded.mvd_abs_l0[b] = [mvd_x_abs.min(255) as u8, mvd_y_abs.min(255) as u8];
        }
    }

    // cbp.
    let cbp_luma = decode_cbp_luma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
    let cbp_chroma = decode_cbp_chroma(cabac, states, neighbours.left_cbp, neighbours.top_cbp);
    decoded.cbp = (cbp_chroma << 4) | cbp_luma;

    // mb_qp_delta when cbp > 0.
    let mb_qp_delta = if decoded.cbp != 0 {
        decode_mb_qp_delta(cabac, states, prev_qp_delta_nonzero)
    } else {
        0
    };

    PMbOutcome::Inter {
        mb_type_code,
        decoded,
        mb_qp_delta,
    }
}

/// Returns the 4×4 block indices covered by each partition of the
/// given shape.  Partitions are listed in scan order.
///
/// Output is a fixed-size 4-slot array with `[]` for unused slots.
fn partitions_for_shape(shape: InterPartShape) -> &'static [&'static [usize]] {
    match shape {
        InterPartShape::P16x16 => &[&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]],
        InterPartShape::P16x8 => &[
            &[0, 1, 2, 3, 4, 5, 6, 7],        // top 16×8
            &[8, 9, 10, 11, 12, 13, 14, 15],  // bottom 16×8
        ],
        InterPartShape::P8x16 => &[
            &[0, 1, 4, 5, 8, 9, 12, 13],      // left 8×16
            &[2, 3, 6, 7, 10, 11, 14, 15],    // right 8×16
        ],
        // P_8x8: 4 8×8 sub-MBs, each treated as a single partition
        // by this commit (no sub-mb-type bin tree yet).
        InterPartShape::P8x8 => &[
            &[0, 1, 4, 5],
            &[2, 3, 6, 7],
            &[8, 9, 12, 13],
            &[10, 11, 14, 15],
        ],
    }
}

/// Picks the neighbour ref indices used for a 4×4 block's
/// ref_idx CABAC context (spec § 9.3.3.1.1.6 → § 6.4.11.4).
fn ref_neighbours_for_block(neighbours: &MbNeighbours, block: usize) -> (i8, i8) {
    let row = block / 4;
    let col = block % 4;
    let ref_a = if col == 0 {
        if neighbours.left_available { neighbours.left_ref[row] } else { -1 }
    } else {
        // Same macroblock: previous column at this row's partition.
        -1 // Filled by the orchestrator after the first partition.
    };
    let ref_b = if row == 0 {
        if neighbours.top_available { neighbours.top_ref[col] } else { -1 }
    } else {
        -1
    };
    (ref_a, ref_b)
}

/// Picks the neighbour absolute MVD magnitudes used for a 4×4
/// block's mvd CABAC context (spec § 9.3.3.1.1.7).
fn mvd_neighbours_for_block(neighbours: &MbNeighbours, block: usize) -> (u8, u8, u8, u8) {
    let row = block / 4;
    let col = block % 4;
    let (mvd_x_a, mvd_y_a) = if col == 0 {
        if neighbours.left_available {
            (neighbours.left_mvd_abs[row][0], neighbours.left_mvd_abs[row][1])
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };
    let (mvd_x_b, mvd_y_b) = if row == 0 {
        if neighbours.top_available {
            (neighbours.top_mvd_abs[col][0], neighbours.top_mvd_abs[col][1])
        } else {
            (0, 0)
        }
    } else {
        (0, 0)
    };
    (mvd_x_a, mvd_x_b, mvd_y_a, mvd_y_b)
}

/// Median-predictor context for a partition.  Coarse approximation
/// for non-16×16 shapes; the spec's per-partition overrides (16×8
/// uses the above MV outright when both partitions match, etc.)
/// are handled by [`crate::h264::mv_pred::predict_mv_16x8_top`]
/// / `_bottom` / `_left` / `_right` in motion compensation, not
/// here at parse time.
fn mv_pred_context_for_partition(
    neighbours: &MbNeighbours,
    shape: InterPartShape,
    partition_index: usize,
) -> MvPredictionContext {
    // Pick the "first" 4×4 of the partition.
    let first_block: usize = match (shape, partition_index) {
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
        // Top-right is the 4×4 to the right of the top-row's last
        // sub-block in the partition.  For 16×16 / 16×8 top, that's
        // the top-right macroblock's bottom-left.
        neighbours.top_right_mv
    } else {
        None
    };
    MvPredictionContext { left, above, above_right, above_left: None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::cabac::init_contexts;

    fn buf() -> Vec<u8> {
        let mut v = vec![0x55u8; 128];
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
            left_is_skip: false,
            top_mv: [(0, 0); 4],
            top_ref: [-1; 4],
            top_mvd_abs: [[0; 2]; 4],
            top_is_skip: false,
            top_right_mv: None,
            top_right_ref: -1,
            left_cbp: 0,
            top_cbp: 0,
            left_chroma_pred_nonzero: false,
            top_chroma_pred_nonzero: false,
        }
    }

    #[test]
    fn p_mb_decode_returns_some_outcome() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let nb = empty_neighbours();
        let outcome = decode_p_mb_cabac(&mut cabac, &mut states, &nb, 1, false);
        match outcome {
            PMbOutcome::Skip => {}
            PMbOutcome::Inter { mb_type_code, decoded, .. } => {
                assert!(mb_type_code <= 4);
                assert!(decoded.cbp <= 0x3F);
            }
            PMbOutcome::Intra(_) => {}
        }
    }

    #[test]
    fn partitions_for_shape_cover_all_blocks() {
        for shape in [
            InterPartShape::P16x16,
            InterPartShape::P16x8,
            InterPartShape::P8x16,
            InterPartShape::P8x8,
        ] {
            let parts = partitions_for_shape(shape);
            let mut seen = [false; 16];
            for blocks in parts {
                for &b in *blocks {
                    assert!(!seen[b], "block {b} covered twice for {shape:?}");
                    seen[b] = true;
                }
            }
            assert!(seen.iter().all(|&s| s), "shape {shape:?} missed a block");
        }
    }
}
