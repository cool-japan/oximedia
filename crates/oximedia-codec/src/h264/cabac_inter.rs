//! H.264 CABAC inter-macroblock decoders.
//!
//! On top of [`crate::h264::cabac_syntax`] (per-bin syntax-element
//! decoders), the P and B slice macroblock loops need a higher-level
//! decoder that picks an `mb_type` code from the bitstream and maps
//! it to a partition shape + list-usage description.
//!
//! All tables and bin-decode trees are normative per ITU-T Rec.
//! H.264 / ISO/IEC 14496-10 Tables 7-13 (P) and 7-14 (B), with the
//! CABAC bin trees in clause 9.3.3.1.1.
//!
//! ## Output types
//!
//! Rather than re-export FFmpeg-style `MB_TYPE_*` bit flags, this
//! module returns structured enums:
//!
//! - [`InterPartShape`] — 16×16 / 16×8 / 8×16 / 8×8.
//! - [`RefListUse`] — which reference picture lists the partition
//!   draws from (L0 only for P; L0 / L1 / Bi / Direct for B).
//! - [`PMbInfo`] / [`BMbInfo`] — full per-table entry with partition
//!   count and per-partition list usage.

use crate::h264::cabac::CabacContext;
use crate::h264::cabac_syntax::IntraMbType;

/// Inter-partition shape (top-level macroblock partitioning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterPartShape {
    /// Whole macroblock is one 16×16 partition.
    P16x16,
    /// Two 16×8 partitions stacked vertically.
    P16x8,
    /// Two 8×16 partitions side-by-side.
    P8x16,
    /// Four 8×8 partitions — each may further split into sub-MB
    /// partitions selected by `sub_mb_type` bins.
    P8x8,
}

/// Per-partition list usage.  P slices only ever produce [`L0`];
/// B slices use all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefListUse {
    /// `ref_idx_l0` + `mvd_l0` decoded, `ref_idx_l1` skipped.
    L0,
    /// `ref_idx_l1` + `mvd_l1` decoded, `ref_idx_l0` skipped.
    L1,
    /// Both lists decoded.
    BiPred,
    /// B_Direct partition — no ref or mvd signalled; inferred from
    /// neighbour state via the direct prediction process.
    Direct,
}

/// Macroblock-level info for a P-slice inter `mb_type` code
/// (0..=4 per spec Table 7-13).
#[derive(Debug, Clone, Copy)]
pub struct PMbInfo {
    /// Top-level partition shape.
    pub shape: InterPartShape,
    /// Number of partitions covering the macroblock (1, 2, or 4).
    pub partition_count: u8,
    /// `true` only for `P_8x8ref0` (mb_type = 4): every partition
    /// is forced to ref_idx = 0 with no `ref_idx_lN` bin in the
    /// bitstream.
    pub ref0_only: bool,
}

/// Macroblock-level info for a B-slice inter `mb_type` code
/// (0..=22 per spec Table 7-14).
#[derive(Debug, Clone, Copy)]
pub struct BMbInfo {
    /// Top-level partition shape.
    pub shape: InterPartShape,
    /// Number of partitions covering the macroblock.
    pub partition_count: u8,
    /// Per-partition list usage (only `partition_count` entries are
    /// meaningful; trailing slots are filled with `Direct` as a
    /// don't-care placeholder).
    pub list_use: [RefListUse; 2],
    /// `true` when the macroblock is `B_Direct_16x16`.  In that
    /// case `list_use` is irrelevant — direct prediction supplies
    /// both motion vectors.
    pub direct: bool,
}

/// P-slice inter mb_type table (spec Table 7-13, codes 0..=4).
///
/// The CABAC bin decoder emits codes 0..=3 directly via
/// [`decode_p_mb_type`]; `P_8x8ref0` (code 4) is reachable via the
/// CAVLC path only.
pub const P_MB_TYPE_INFO: [PMbInfo; 5] = [
    PMbInfo { shape: InterPartShape::P16x16, partition_count: 1, ref0_only: false },
    PMbInfo { shape: InterPartShape::P16x8,  partition_count: 2, ref0_only: false },
    PMbInfo { shape: InterPartShape::P8x16,  partition_count: 2, ref0_only: false },
    PMbInfo { shape: InterPartShape::P8x8,   partition_count: 4, ref0_only: false },
    PMbInfo { shape: InterPartShape::P8x8,   partition_count: 4, ref0_only: true  },
];

/// B-slice inter mb_type table (spec Table 7-14, codes 0..=22).
///
/// Code 0 is `B_Direct_16x16`; codes 1..=22 are explicit-MV
/// partitions across L0 / L1 / Bi combinations.  The bin tree in
/// [`decode_b_mb_type`] selects one of these codes (or escapes to
/// the intra path).
pub const B_MB_TYPE_INFO: [BMbInfo; 23] = [
    // 0: B_Direct_16x16
    BMbInfo { shape: InterPartShape::P16x16, partition_count: 1,
              list_use: [RefListUse::Direct, RefListUse::Direct], direct: true },
    // 1, 2, 3: B_L0_16x16, B_L1_16x16, B_Bi_16x16
    BMbInfo { shape: InterPartShape::P16x16, partition_count: 1,
              list_use: [RefListUse::L0, RefListUse::Direct], direct: false },
    BMbInfo { shape: InterPartShape::P16x16, partition_count: 1,
              list_use: [RefListUse::L1, RefListUse::Direct], direct: false },
    BMbInfo { shape: InterPartShape::P16x16, partition_count: 1,
              list_use: [RefListUse::BiPred, RefListUse::Direct], direct: false },
    // 4..=21: 16x8 / 8x16 list combinations.
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::L0, RefListUse::L0], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::L0, RefListUse::L0], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::L1, RefListUse::L1], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::L1, RefListUse::L1], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::L0, RefListUse::L1], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::L0, RefListUse::L1], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::L1, RefListUse::L0], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::L1, RefListUse::L0], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::L0, RefListUse::BiPred], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::L0, RefListUse::BiPred], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::L1, RefListUse::BiPred], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::L1, RefListUse::BiPred], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::BiPred, RefListUse::L0], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::BiPred, RefListUse::L0], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::BiPred, RefListUse::L1], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::BiPred, RefListUse::L1], direct: false },
    BMbInfo { shape: InterPartShape::P16x8, partition_count: 2,
              list_use: [RefListUse::BiPred, RefListUse::BiPred], direct: false },
    BMbInfo { shape: InterPartShape::P8x16, partition_count: 2,
              list_use: [RefListUse::BiPred, RefListUse::BiPred], direct: false },
    // 22: B_8x8 — list usage decided per sub-mb.
    BMbInfo { shape: InterPartShape::P8x8, partition_count: 4,
              list_use: [RefListUse::Direct, RefListUse::Direct], direct: false },
];

/// Result of the P/B mb_type decoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterMbResult {
    /// Macroblock is inter-coded: `code` indexes [`P_MB_TYPE_INFO`]
    /// (for P slices) or [`B_MB_TYPE_INFO`] (for B slices).
    Inter(u8),
    /// Macroblock escaped to the intra branch — caller continues
    /// with the I-slice flow (Intra4x4 / Intra16x16 / I_PCM).
    Intra(IntraMbType),
}

/// Decodes the P-slice macroblock type via CABAC.
///
/// Returns `Inter(0..=3)` for inter mb types or `Intra(...)` when
/// the bitstream escapes to the intra branch.  Implements the bin
/// tree at spec clause 9.3.3.1.1.3 / FFmpeg slice path lines
/// 2003..=2018 (context indices 14..=17 for inter, 17.. for intra
/// escape).
pub fn decode_p_mb_type(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
) -> InterMbResult {
    // Bin 0: inter vs intra escape.
    if cabac.get(&mut states[14]) == 0 {
        // Inter path.
        let bin1 = cabac.get(&mut states[15]);
        let code = if bin1 == 0 {
            // P_L0_16x16 (0) or P_8x8 (3) selected by bin 2.
            3 * cabac.get(&mut states[16])
        } else {
            // P_16x8 (1) or P_8x16 (2) selected by bin 2.
            2 - cabac.get(&mut states[17])
        };
        InterMbResult::Inter(code as u8)
    } else {
        // Intra path: decode_cabac_intra_mb_type(sl, 17, 0).
        InterMbResult::Intra(crate::h264::cabac_syntax::decode_intra_mb_type(
            cabac, states, 17, false, false, false,
        ))
    }
}

/// Decodes the B-slice macroblock type via CABAC.
///
/// Returns `Inter(0..=22)` for inter mb types or `Intra(...)` on the
/// intra escape (`bits == 13` in the suffix loop).  Implements the
/// bin tree at spec clause 9.3.3.1.1.3 / FFmpeg slice path lines
/// 1968..=1999 (context indices 27..=39).
///
/// `left_is_b_direct` / `top_is_b_direct` are
/// `IS_DIRECT(neighbour.mb_type - 1)` — used to bias the first-bin
/// context per spec eq. 9-22.
pub fn decode_b_mb_type(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    left_is_b_direct: bool,
    top_is_b_direct: bool,
) -> InterMbResult {
    let mut ctx = 0;
    if !left_is_b_direct {
        ctx += 1;
    }
    if !top_is_b_direct {
        ctx += 1;
    }

    if cabac.get(&mut states[27 + ctx]) == 0 {
        return InterMbResult::Inter(0); // B_Direct_16x16
    }

    if cabac.get(&mut states[27 + 3]) == 0 {
        // B_L0_16x16 (1) or B_L1_16x16 (2).
        let suffix = cabac.get(&mut states[27 + 5]);
        return InterMbResult::Inter(1 + suffix as u8);
    }

    // 4-bit prefix follows: bits ∈ 0..=15.
    let bits = (cabac.get(&mut states[27 + 4]) << 3)
        + (cabac.get(&mut states[27 + 5]) << 2)
        + (cabac.get(&mut states[27 + 5]) << 1)
        + cabac.get(&mut states[27 + 5]);

    let code = if bits < 8 {
        // B_Bi_16x16 (3) .. B_L1_L0_16x8 (10).
        (bits + 3) as u8
    } else if bits == 13 {
        // Intra escape: decode_cabac_intra_mb_type(sl, 32, 0).
        return InterMbResult::Intra(crate::h264::cabac_syntax::decode_intra_mb_type(
            cabac, states, 32, false, false, false,
        ));
    } else if bits == 14 {
        11 // B_L1_L0_8x16
    } else if bits == 15 {
        22 // B_8x8
    } else {
        // One more bin appended → 5-bit value, maps to 12..=21.
        let appended = (bits << 1) + cabac.get(&mut states[27 + 5]);
        (appended - 4) as u8
    };

    InterMbResult::Inter(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::cabac::init_contexts;
    use crate::h264::slice_header::SliceType;

    fn buf() -> Vec<u8> {
        let mut v = vec![0x55u8; 64];
        v[0] = 0x40;
        v
    }

    #[test]
    fn p_mb_type_info_partition_counts() {
        assert_eq!(P_MB_TYPE_INFO[0].partition_count, 1);
        assert_eq!(P_MB_TYPE_INFO[1].partition_count, 2);
        assert_eq!(P_MB_TYPE_INFO[2].partition_count, 2);
        assert_eq!(P_MB_TYPE_INFO[3].partition_count, 4);
        assert_eq!(P_MB_TYPE_INFO[4].partition_count, 4);
        assert!(P_MB_TYPE_INFO[4].ref0_only);
        assert!(!P_MB_TYPE_INFO[0].ref0_only);
    }

    #[test]
    fn b_mb_type_info_direct_is_only_code_0() {
        assert!(B_MB_TYPE_INFO[0].direct);
        for (i, info) in B_MB_TYPE_INFO.iter().enumerate().skip(1) {
            assert!(!info.direct, "code {i} should not be direct");
        }
    }

    #[test]
    fn b_mb_type_info_code_22_is_8x8() {
        assert_eq!(B_MB_TYPE_INFO[22].shape, InterPartShape::P8x8);
        assert_eq!(B_MB_TYPE_INFO[22].partition_count, 4);
    }

    #[test]
    fn p_mb_type_decode_returns_valid_variant() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let r = decode_p_mb_type(&mut cabac, &mut states);
        match r {
            InterMbResult::Inter(c) => assert!(c <= 3),
            InterMbResult::Intra(_) => {} // intra escape is valid
        }
    }

    #[test]
    fn b_mb_type_decode_returns_valid_variant() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::B, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let r = decode_b_mb_type(&mut cabac, &mut states, false, false);
        match r {
            InterMbResult::Inter(c) => assert!(c <= 22),
            InterMbResult::Intra(_) => {} // intra escape is valid
        }
    }
}
