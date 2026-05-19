//! Inter-macroblock neighbour caches.
//!
//! The H.264 inter parser reads per-4×4-subblock motion-vector
//! difference, ref-idx, and non-zero-count from one macroblock and
//! threads it forward into the *next* macroblock's CABAC context
//! selection.  This module owns the data structures that carry that
//! state across macroblock decodes.
//!
//! The CABAC inter decoders use three pieces of neighbour state:
//!
//! - **Reference indices** of neighbour partitions (spec
//!   § 9.3.3.1.1.6) — pick the `ref_idx` context.
//! - **Absolute MVD magnitudes** of neighbour partitions
//!   (spec § 9.3.3.1.1.7) — pick the `mvd_lN` context.
//! - **Motion vectors** of neighbour partitions (spec § 8.4.1.3) —
//!   form the median MV predictor that turns mvd into a real MV.
//!
//! We track these at 4×4 sub-block granularity inside a macroblock
//! and at "rightmost column" / "bottom row" granularity at the
//! neighbour boundary, mirroring the standard scan8 layout but
//! without the 40-entry padded scan structure.

use crate::h264::mv_pred::MotionVector;

/// All the data produced by one decoded inter macroblock that the
/// slice loop needs to thread forward.
///
/// The 16 entries correspond to 4×4 sub-blocks in raster order
/// inside the macroblock (row 0..=3, col 0..=3).
#[derive(Debug, Clone, Copy)]
pub struct InterMbDecoded {
    /// `mb_type` code: 0..=4 for P slices (per
    /// [`crate::h264::cabac_inter::P_MB_TYPE_INFO`]), 0..=22 for B.
    pub mb_type_code: u8,
    /// Per-4×4 motion vectors for list 0 (i16 half-pel units).
    pub mv_l0: [MotionVector; 16],
    /// Per-4×4 motion vectors for list 1 (B slices only — zeroed on
    /// P slices).
    pub mv_l1: [MotionVector; 16],
    /// Per-4×4 reference indices for list 0; `-1` for sub-blocks
    /// that don't use L0 (Intra / B_L1 / B_Direct partitions).
    pub ref_l0: [i8; 16],
    /// Per-4×4 reference indices for list 1; `-1` when L1 unused.
    pub ref_l1: [i8; 16],
    /// Absolute MVD magnitudes for list 0, packed `[mvd_x, mvd_y]`.
    pub mvd_abs_l0: [[u8; 2]; 16],
    /// Absolute MVD magnitudes for list 1.
    pub mvd_abs_l1: [[u8; 2]; 16],
    /// Per-4×4 luma non-zero coefficient count (0..=16).
    pub nz_count_luma: [u8; 16],
    /// Per-4×4 chroma non-zero coefficient count: 0..=3 = Cb,
    /// 4..=7 = Cr (4:2:0).
    pub nz_count_chroma: [u8; 8],
    /// Packed CBP byte (low 4 = luma, bits 4..=5 = chroma).
    pub cbp: u8,
    /// `true` when the macroblock was signalled as Intra (the
    /// inter decoder routed to the intra path).
    pub is_intra: bool,
    /// `true` when the macroblock was P_Skip / B_Skip.
    pub is_skip: bool,
}

impl Default for InterMbDecoded {
    fn default() -> Self {
        Self {
            mb_type_code: 0,
            mv_l0: [(0, 0); 16],
            mv_l1: [(0, 0); 16],
            ref_l0: [-1; 16],
            ref_l1: [-1; 16],
            mvd_abs_l0: [[0; 2]; 16],
            mvd_abs_l1: [[0; 2]; 16],
            nz_count_luma: [0; 16],
            nz_count_chroma: [0; 8],
            cbp: 0,
            is_intra: false,
            is_skip: false,
        }
    }
}

/// Slice-wide neighbour cache.  Stores the bottom row of every
/// macroblock in the previous MB row (length = picture width in
/// MBs) plus the right column of the previous macroblock in the
/// current row.  CABAC's context selection always sees these as
/// "A" (left) and "B" (top) neighbours.
#[derive(Debug, Clone)]
pub struct InterSliceCache {
    /// Picture width in macroblocks.
    pub pic_width_mbs: usize,
    /// Bottom row of the previous-row macroblock at column `mb_x`:
    /// 4 4×4 entries each.  Reset to "unavailable" at slice start.
    pub top_row: Vec<TopRowSlot>,
    /// Right column of the just-decoded macroblock at the current
    /// row: 4 4×4 entries.  Reset at the start of each row.
    pub left_col: LeftColSlot,
    /// `true` when the previous macroblock decode produced a
    /// nonzero `mb_qp_delta` — biases the next macroblock's
    /// `mb_qp_delta` first-bin context.
    pub prev_qp_delta_nonzero: bool,
}

/// Per-column slot describing the bottom 4×4 row of a top neighbour.
#[derive(Debug, Clone, Copy, Default)]
pub struct TopRowSlot {
    /// `true` if a macroblock was decoded for this column in the
    /// previous row.  `false` at slice start or at the first MB of
    /// each row when no previous row exists.
    pub available: bool,
    /// `true` when the top neighbour was Intra-coded (drives the
    /// `mb_type` context for the next macroblock down).
    pub is_intra: bool,
    /// `true` when the top neighbour was a `B_Direct` partition.
    pub is_b_direct: bool,
    /// `true` when the top neighbour was P_Skip / B_Skip.
    pub is_skip: bool,
    /// 4-entry strip of L0 ref indices (one per 4×4 column).
    pub ref_l0: [i8; 4],
    /// 4-entry strip of L0 motion vectors.
    pub mv_l0: [MotionVector; 4],
    /// 4-entry strip of L0 absolute MVD magnitudes.
    pub mvd_abs_l0: [[u8; 2]; 4],
    /// 4-entry strip of non-zero counts (luma).
    pub nz_count: [u8; 4],
    /// Packed CBP of the top neighbour (for chroma_pred_mode +
    /// CBP context selection).
    pub cbp: u8,
    /// Top neighbour's `intra_chroma_pred_mode` (0..=3).  Used by
    /// [`crate::h264::cabac_syntax::decode_intra_chroma_pred_mode`].
    pub chroma_pred_mode: u8,
}

/// Right-column slot describing the rightmost 4×4 column of the
/// just-decoded macroblock at the current row.  Reset to
/// "unavailable" at the first MB of each row.
#[derive(Debug, Clone, Copy, Default)]
pub struct LeftColSlot {
    /// `true` when a left neighbour exists (i.e. mb_x > 0 in the
    /// current row).
    pub available: bool,
    /// `true` when the left neighbour was Intra-coded.
    pub is_intra: bool,
    /// `true` when the left neighbour was a `B_Direct` partition.
    pub is_b_direct: bool,
    /// `true` when the left neighbour was P_Skip / B_Skip.
    pub is_skip: bool,
    /// 4-entry strip of L0 ref indices (one per 4×4 row).
    pub ref_l0: [i8; 4],
    /// 4-entry strip of L0 motion vectors.
    pub mv_l0: [MotionVector; 4],
    /// 4-entry strip of L0 absolute MVD magnitudes.
    pub mvd_abs_l0: [[u8; 2]; 4],
    /// 4-entry strip of non-zero counts (luma).
    pub nz_count: [u8; 4],
    /// Packed CBP of the left neighbour.
    pub cbp: u8,
    /// Left neighbour's `intra_chroma_pred_mode` (0..=3).
    pub chroma_pred_mode: u8,
}

impl InterSliceCache {
    /// Builds a fresh cache sized for a picture `pic_width_mbs`
    /// macroblocks wide.  Every slot is marked unavailable.
    #[must_use]
    pub fn new(pic_width_mbs: usize) -> Self {
        Self {
            pic_width_mbs,
            top_row: vec![TopRowSlot::default(); pic_width_mbs],
            left_col: LeftColSlot::default(),
            prev_qp_delta_nonzero: false,
        }
    }

    /// Resets the left-column slot — called at the start of each
    /// macroblock row.
    pub fn begin_row(&mut self) {
        self.left_col = LeftColSlot::default();
    }

    /// Pushes one decoded macroblock into both the top-row slot
    /// (for the macroblock below) and the left-col slot (for the
    /// macroblock to the right).
    pub fn record_inter_mb(
        &mut self,
        mb_x: usize,
        decoded: &InterMbDecoded,
        chroma_pred_mode: u8,
        mb_qp_delta: i32,
    ) {
        debug_assert!(mb_x < self.pic_width_mbs);

        // Bottom row of the decoded MB = indices 12..=15 of the
        // 4×4 raster (row 3 of the macroblock).
        let bottom_idx = [12usize, 13, 14, 15];
        let bottom_ref_l0 = [
            decoded.ref_l0[bottom_idx[0]],
            decoded.ref_l0[bottom_idx[1]],
            decoded.ref_l0[bottom_idx[2]],
            decoded.ref_l0[bottom_idx[3]],
        ];
        let bottom_mv_l0 = [
            decoded.mv_l0[bottom_idx[0]],
            decoded.mv_l0[bottom_idx[1]],
            decoded.mv_l0[bottom_idx[2]],
            decoded.mv_l0[bottom_idx[3]],
        ];
        let bottom_mvd_l0 = [
            decoded.mvd_abs_l0[bottom_idx[0]],
            decoded.mvd_abs_l0[bottom_idx[1]],
            decoded.mvd_abs_l0[bottom_idx[2]],
            decoded.mvd_abs_l0[bottom_idx[3]],
        ];
        let bottom_nz = [
            decoded.nz_count_luma[bottom_idx[0]],
            decoded.nz_count_luma[bottom_idx[1]],
            decoded.nz_count_luma[bottom_idx[2]],
            decoded.nz_count_luma[bottom_idx[3]],
        ];

        let is_b_direct =
            !decoded.is_intra && !decoded.is_skip && decoded.cbp == 0 && decoded.mb_type_code == 0;

        self.top_row[mb_x] = TopRowSlot {
            available: true,
            is_intra: decoded.is_intra,
            is_b_direct,
            is_skip: decoded.is_skip,
            ref_l0: bottom_ref_l0,
            mv_l0: bottom_mv_l0,
            mvd_abs_l0: bottom_mvd_l0,
            nz_count: bottom_nz,
            cbp: decoded.cbp,
            chroma_pred_mode,
        };

        // Right column of the decoded MB = indices 3, 7, 11, 15
        // (col 3 of the macroblock).
        let right_idx = [3usize, 7, 11, 15];
        let right_ref_l0 = [
            decoded.ref_l0[right_idx[0]],
            decoded.ref_l0[right_idx[1]],
            decoded.ref_l0[right_idx[2]],
            decoded.ref_l0[right_idx[3]],
        ];
        let right_mv_l0 = [
            decoded.mv_l0[right_idx[0]],
            decoded.mv_l0[right_idx[1]],
            decoded.mv_l0[right_idx[2]],
            decoded.mv_l0[right_idx[3]],
        ];
        let right_mvd_l0 = [
            decoded.mvd_abs_l0[right_idx[0]],
            decoded.mvd_abs_l0[right_idx[1]],
            decoded.mvd_abs_l0[right_idx[2]],
            decoded.mvd_abs_l0[right_idx[3]],
        ];
        let right_nz = [
            decoded.nz_count_luma[right_idx[0]],
            decoded.nz_count_luma[right_idx[1]],
            decoded.nz_count_luma[right_idx[2]],
            decoded.nz_count_luma[right_idx[3]],
        ];

        self.left_col = LeftColSlot {
            available: true,
            is_intra: decoded.is_intra,
            is_b_direct,
            is_skip: decoded.is_skip,
            ref_l0: right_ref_l0,
            mv_l0: right_mv_l0,
            mvd_abs_l0: right_mvd_l0,
            nz_count: right_nz,
            cbp: decoded.cbp,
            chroma_pred_mode,
        };

        self.prev_qp_delta_nonzero = mb_qp_delta != 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_cache_has_unavailable_slots() {
        let cache = InterSliceCache::new(8);
        assert_eq!(cache.top_row.len(), 8);
        for slot in &cache.top_row {
            assert!(!slot.available);
        }
        assert!(!cache.left_col.available);
        assert!(!cache.prev_qp_delta_nonzero);
    }

    #[test]
    fn begin_row_clears_left_col() {
        let mut cache = InterSliceCache::new(4);
        cache.left_col.available = true;
        cache.begin_row();
        assert!(!cache.left_col.available);
    }

    #[test]
    fn record_mb_populates_both_slots() {
        let mut cache = InterSliceCache::new(4);
        let mut decoded = InterMbDecoded::default();
        decoded.mb_type_code = 0; // P_L0_16x16
        decoded.ref_l0 = [0; 16];
        decoded.nz_count_luma = [3; 16];
        decoded.cbp = 0x0F;
        cache.record_inter_mb(1, &decoded, 0, 2);
        assert!(cache.top_row[1].available);
        assert!(cache.left_col.available);
        assert_eq!(cache.top_row[1].nz_count, [3; 4]);
        assert_eq!(cache.left_col.nz_count, [3; 4]);
        assert_eq!(cache.top_row[1].cbp, 0x0F);
        assert!(cache.prev_qp_delta_nonzero);
    }

    #[test]
    fn record_mb_sets_qp_flag_to_false_when_delta_zero() {
        let mut cache = InterSliceCache::new(4);
        cache.prev_qp_delta_nonzero = true;
        let decoded = InterMbDecoded::default();
        cache.record_inter_mb(0, &decoded, 0, 0);
        assert!(!cache.prev_qp_delta_nonzero);
    }
}
