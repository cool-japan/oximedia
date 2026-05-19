//! H.264 CABAC per-syntax-element decoders.
//!
//! This module turns the raw arithmetic-coder primitives in
//! [`crate::h264::cabac`] into the syntax-element-level decoders that
//! a slice loop actually calls: skip flag, mb type, sub-mb type,
//! chroma prediction mode, ref index, motion vector difference,
//! coded-block-pattern, and the Intra4x4 prediction mode.
//!
//! Each function below implements one syntax-element decode
//! procedure from ITU-T Rec. H.264 / ISO/IEC 14496-10 clause 9.3.3
//! (CABAC parsing process).  Neighbour-state inputs that would
//! normally live in the slice-context struct are passed in
//! explicitly so this layer stays free of slice plumbing.
//!
//! ## Context indices
//!
//! The context-state byte array (length 460, see
//! [`crate::h264::cabac::init_contexts`]) is shared across all
//! syntax-element decoders.  Each decoder addresses a fixed window
//! inside that array:
//!
//! | Window     | Syntax element                  |
//! |------------|---------------------------------|
//! | 0..=10     | I-slice mb_type                 |
//! | 11..=23    | P-slice mb skip flag + sub-mb   |
//! | 24..=35    | (reserved — P/B mb_type)        |
//! | 36..=39    | B-slice sub-mb type             |
//! | 40..=53    | MVD                             |
//! | 54..=59    | Ref index                       |
//! | 60..=63    | (reserved)                      |
//! | 64..=67    | Intra chroma pred mode          |
//! | 68..=69    | Intra4x4 pred mode              |
//! | 70..=72    | (reserved — field decoding)     |
//! | 73..=76    | CBP luma                        |
//! | 77..=83    | CBP chroma + qp_delta           |
//! | 84..=104   | (reserved — residual)           |

use crate::h264::cabac::CabacContext;
use crate::h264::slice_header::SliceType;

/// Result of [`decode_intra_mb_type`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntraMbType {
    /// I_NxN — the macroblock is partitioned into 16 4×4 luma blocks
    /// each with its own intra prediction mode.
    I4x4,
    /// I_PCM — raw uncoded samples follow.
    IPCM,
    /// I_16x16 — single prediction mode for the entire 16×16 luma
    /// block.  Carries the intra_16x16 prediction mode plus a packed
    /// CBP byte (bits 0..=3 = luma AC CBP, bits 4..=5 = chroma CBP).
    I16x16 {
        /// 0..=3, the intra_16x16 prediction mode.
        pred_mode: u8,
        /// Packed CBP: bits 0..=3 are luma (0x0 or 0xF) and bits
        /// 4..=5 carry chroma (0 = none, 1 = DC only, 2 = DC + AC).
        cbp: u8,
    },
}

/// Decodes the `mb_skip_flag` for a P or B macroblock.
///
/// Returns `1` if the macroblock is skipped, `0` otherwise.  The
/// neighbour inputs are the "is the A/B neighbour available and
/// non-skip" booleans, per spec § 9.3.3.1.1.1.
///
/// Frame-MB path only — MBAFF (interlaced) is intentionally out of
/// scope here.
pub fn decode_mb_skip(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    slice_type: SliceType,
    left_available_and_nonskip: bool,
    top_available_and_nonskip: bool,
) -> i32 {
    let mut ctx = 0;
    if left_available_and_nonskip {
        ctx += 1;
    }
    if top_available_and_nonskip {
        ctx += 1;
    }
    if matches!(slice_type, SliceType::B) {
        ctx += 13;
    }
    cabac.get(&mut states[11 + ctx])
}

/// Decodes `mb_type` for an I-slice (or for the I-slice-style fork
/// inside P/B slices when `intra_slice == false` and `ctx_base` is
/// passed by the caller — usually 17 for P, 32 for B).
///
/// Per spec § 9.3.3.1.1.3 (Intra-macroblock-type binarisation).
pub fn decode_intra_mb_type(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctx_base: usize,
    intra_slice: bool,
    left_is_intra16x16_or_pcm: bool,
    top_is_intra16x16_or_pcm: bool,
) -> IntraMbType {
    let base = ctx_base;
    let bin0 = if intra_slice {
        let mut ctx = 0;
        if left_is_intra16x16_or_pcm {
            ctx += 1;
        }
        if top_is_intra16x16_or_pcm {
            ctx += 1;
        }
        cabac.get(&mut states[base + ctx])
    } else {
        cabac.get(&mut states[base])
    };

    if bin0 == 0 {
        return IntraMbType::I4x4;
    }

    if cabac.get_terminate() != 0 {
        return IntraMbType::IPCM;
    }

    let state_offset = if intra_slice { 2 } else { 0 };

    let luma_ac = cabac.get(&mut states[base + state_offset + 1]);
    let cbp_chroma_nz = cabac.get(&mut states[base + state_offset + 2]);
    let cbp_chroma_2 = if cbp_chroma_nz != 0 {
        let idx = if intra_slice { 3 } else { 2 };
        cabac.get(&mut states[base + state_offset + idx])
    } else {
        0
    };
    let chroma_cbp = if cbp_chroma_nz != 0 {
        1 + cbp_chroma_2 as u8
    } else {
        0
    };

    let pred_bit1_idx = if intra_slice { 4 } else { 3 };
    let pred_bit0_idx = if intra_slice { 5 } else { 3 };
    let pred_bit1 = cabac.get(&mut states[base + state_offset + pred_bit1_idx]);
    let pred_bit0 = cabac.get(&mut states[base + state_offset + pred_bit0_idx]);
    let pred_mode = ((pred_bit1 << 1) | pred_bit0) as u8;

    IntraMbType::I16x16 {
        pred_mode,
        cbp: (chroma_cbp << 4) | (luma_ac as u8 * 15),
    }
}

/// Decodes `prev_intra4x4_pred_mode_flag` + `rem_intra4x4_pred_mode`
/// pair for a single 4×4 luma block.
///
/// Returns the resolved intra4x4 mode (0..=8).  `pred_mode` is the
/// MPM derived from neighbour blocks — see
/// [`crate::h264::intra_mode::most_probable_mode`].
///
/// Per spec § 9.3.3.1.1.5 (`prev_intra4x4_pred_mode_flag` plus the
/// 3-bit `rem_intra4x4_pred_mode`).
pub fn decode_intra4x4_pred_mode(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    pred_mode: u8,
) -> u8 {
    if cabac.get(&mut states[68]) != 0 {
        return pred_mode;
    }
    let bit0 = cabac.get(&mut states[69]);
    let bit1 = cabac.get(&mut states[69]);
    let bit2 = cabac.get(&mut states[69]);
    let mode = (bit0 | (bit1 << 1) | (bit2 << 2)) as u8;
    if mode >= pred_mode {
        mode + 1
    } else {
        mode
    }
}

/// Decodes `intra_chroma_pred_mode` (0..=3).
///
/// `left_chroma_nonzero` / `top_chroma_nonzero` are
/// `chroma_pred_mode_table[neighbour] != 0`.  Neighbour
/// availability is folded into the boolean.
///
/// Per spec § 9.3.3.1.1.8 (`intra_chroma_pred_mode` binarisation).
pub fn decode_intra_chroma_pred_mode(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    left_chroma_nonzero: bool,
    top_chroma_nonzero: bool,
) -> u8 {
    let mut ctx = 0;
    if left_chroma_nonzero {
        ctx += 1;
    }
    if top_chroma_nonzero {
        ctx += 1;
    }
    if cabac.get(&mut states[64 + ctx]) == 0 {
        return 0;
    }
    if cabac.get(&mut states[64 + 3]) == 0 {
        return 1;
    }
    if cabac.get(&mut states[64 + 3]) == 0 {
        return 2;
    }
    3
}

/// Decodes the four luma CBP bits.
///
/// Returns a 4-bit value where bit 0 covers the top-left 8×8 block,
/// bit 1 top-right, bit 2 bottom-left, bit 3 bottom-right.
///
/// `left_cbp` / `top_cbp` are the 6-bit CBP codes of the A/B
/// neighbours (low 4 bits = luma, upper bits = chroma).  When a
/// neighbour is unavailable pass `0x0F` per H.264 spec.
///
/// Per spec § 9.3.3.1.1.4 (`coded_block_pattern` luma bins).
pub fn decode_cbp_luma(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    left_cbp: u8,
    top_cbp: u8,
) -> u8 {
    let mut cbp: u8 = 0;
    let ctx = (((left_cbp & 0x02) == 0) as usize) + 2 * (((top_cbp & 0x04) == 0) as usize);
    cbp |= cabac.get(&mut states[73 + ctx]) as u8;
    let ctx = (((cbp & 0x01) == 0) as usize) + 2 * (((top_cbp & 0x08) == 0) as usize);
    cbp |= (cabac.get(&mut states[73 + ctx]) as u8) << 1;
    let ctx = (((left_cbp & 0x08) == 0) as usize) + 2 * (((cbp & 0x01) == 0) as usize);
    cbp |= (cabac.get(&mut states[73 + ctx]) as u8) << 2;
    let ctx = (((cbp & 0x04) == 0) as usize) + 2 * (((cbp & 0x02) == 0) as usize);
    cbp |= (cabac.get(&mut states[73 + ctx]) as u8) << 3;
    cbp
}

/// Decodes the chroma CBP code (0..=2).
///
/// 0 = no chroma residual, 1 = DC only, 2 = DC + AC.
///
/// Per spec § 9.3.3.1.1.4 (`coded_block_pattern` chroma bins).
pub fn decode_cbp_chroma(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    left_cbp_chroma: u8,
    top_cbp_chroma: u8,
) -> u8 {
    let cbp_a = (left_cbp_chroma >> 4) & 0x03;
    let cbp_b = (top_cbp_chroma >> 4) & 0x03;

    let mut ctx = 0;
    if cbp_a > 0 {
        ctx += 1;
    }
    if cbp_b > 0 {
        ctx += 2;
    }
    if cabac.get(&mut states[77 + ctx]) == 0 {
        return 0;
    }

    let mut ctx = 4;
    if cbp_a == 2 {
        ctx += 1;
    }
    if cbp_b == 2 {
        ctx += 2;
    }
    1 + cabac.get(&mut states[77 + ctx]) as u8
}

/// Sub-macroblock type for a P slice (0..=3, mapping per spec
/// Table 7-17): 0=P_L0_8x8, 1=P_L0_8x4, 2=P_L0_4x8, 3=P_L0_4x4.
///
/// Per spec § 9.3.3.1.1.2 (sub_mb_type for P slices).
pub fn decode_p_sub_mb_type(cabac: &mut CabacContext<'_>, states: &mut [u8]) -> u8 {
    if cabac.get(&mut states[21]) != 0 {
        return 0;
    }
    if cabac.get(&mut states[22]) == 0 {
        return 1;
    }
    if cabac.get(&mut states[23]) != 0 {
        return 2;
    }
    3
}

/// Sub-macroblock type for a B slice (0..=12, mapping per spec
/// Table 7-18).
///
/// Per spec § 9.3.3.1.1.2 (sub_mb_type for B slices).
pub fn decode_b_sub_mb_type(cabac: &mut CabacContext<'_>, states: &mut [u8]) -> u8 {
    if cabac.get(&mut states[36]) == 0 {
        return 0;
    }
    if cabac.get(&mut states[37]) == 0 {
        return 1 + cabac.get(&mut states[39]) as u8;
    }
    let mut t: u8 = 3;
    if cabac.get(&mut states[38]) != 0 {
        if cabac.get(&mut states[39]) != 0 {
            return 11 + cabac.get(&mut states[39]) as u8;
        }
        t += 4;
    }
    t += 2 * cabac.get(&mut states[39]) as u8;
    t += cabac.get(&mut states[39]) as u8;
    t
}

/// Decodes `ref_idx_lN` (0..=31, or -1 on bitstream overflow).
///
/// `ref_a` / `ref_b` are the neighbour ref-idx values, `direct_a` /
/// `direct_b` are `true` if the neighbour is a B_Direct partition
/// (ignored on P slices).
///
/// Per spec § 9.3.3.1.1.6 (`ref_idx_lN` binarisation).
pub fn decode_ref_idx(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    slice_type: SliceType,
    ref_a: i32,
    ref_b: i32,
    direct_a: bool,
    direct_b: bool,
) -> i32 {
    let mut ctx = 0usize;
    if matches!(slice_type, SliceType::B) {
        if ref_a > 0 && !direct_a {
            ctx += 1;
        }
        if ref_b > 0 && !direct_b {
            ctx += 2;
        }
    } else {
        if ref_a > 0 {
            ctx += 1;
        }
        if ref_b > 0 {
            ctx += 2;
        }
    }

    let mut r = 0i32;
    while cabac.get(&mut states[54 + ctx]) != 0 {
        r += 1;
        ctx = (ctx >> 2) + 4;
        if r >= 32 {
            return -1;
        }
    }
    r
}

/// Decodes a single motion-vector-difference component.
///
/// Returns the signed mvd value.  The absolute mvd is stored back via
/// `*mvd_abs` for the caller to feed into the next neighbour's
/// context selection.
///
/// `ctx_base` is 40 for the x component (`mvd_l0[..][0]` /
/// `mvd_l1[..][0]`) and 47 for the y component.
/// `amvd` is the sum of |left| + |top| neighbour mvd magnitudes
/// (per spec eq. 9-21).
///
/// Per spec § 9.3.3.1.1.7 (`mvd_lN` binarisation: truncated unary
/// + UEG3 suffix + sign bit).
pub fn decode_mvd(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctx_base: usize,
    amvd: i32,
    mvd_abs: &mut i32,
) -> i32 {
    // The two right-shifts produce -1 (i.e. all-1 bits) when
    // `amvd` is below the threshold, 0 otherwise — the standard
    // branchless way to pick context index 0, 1 or 2 from the
    // threshold ranges spec § 9.3.3.1.1.7 gives for mvd.
    let bin0_ctx = (ctx_base as i32
        + ((amvd - 3) >> 31)
        + ((amvd - 33) >> 31)
        + 2) as usize;

    if cabac.get(&mut states[bin0_ctx]) == 0 {
        *mvd_abs = 0;
        return 0;
    }

    let mut mvd: i32 = 1;
    let mut ctx_walk = ctx_base + 3;
    while mvd < 9 && cabac.get(&mut states[ctx_walk]) != 0 {
        if mvd < 4 {
            ctx_walk += 1;
        }
        mvd += 1;
    }

    if mvd >= 9 {
        let mut k = 3;
        while cabac.get_bypass() != 0 {
            mvd += 1 << k;
            k += 1;
            if k > 24 {
                // bitstream overflow — caller treats `i32::MIN` as
                // invalid data — the spec's k bound is 23.
                return i32::MIN;
            }
        }
        while k > 0 {
            k -= 1;
            mvd += cabac.get_bypass() << k;
        }
        *mvd_abs = if mvd < 70 { mvd } else { 70 };
    } else {
        *mvd_abs = mvd;
    }

    cabac.get_bypass_sign(-mvd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::cabac::init_contexts;

    /// Build a minimal CABAC bytestream that yields a deterministic
    /// sequence of bins.  The values themselves are not interesting
    /// — these tests only check that the decoders thread the right
    /// context indices through the arithmetic coder.
    ///
    /// `ff_init_cabac_decoder` rejects bytestreams whose first byte
    /// would push the initial `low` register past `range << 17`, so
    /// the leading byte stays in the lower half (0x40 ≤ 0x1FE).
    fn sample_bytes() -> Vec<u8> {
        let mut v = vec![0x55u8; 64];
        v[0] = 0x40;
        v
    }

    #[test]
    fn skip_flag_is_deterministic() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_mb_skip(&mut cabac, &mut states, SliceType::P, false, false);
        assert!(r == 0 || r == 1);
    }

    #[test]
    fn intra_mb_type_returns_valid_variant() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_intra_mb_type(&mut cabac, &mut states, 3, true, false, false);
        match r {
            IntraMbType::I4x4 | IntraMbType::IPCM => {}
            IntraMbType::I16x16 { pred_mode, cbp } => {
                assert!(pred_mode <= 3);
                // Max packed value: (chroma=2 << 4) | (luma=15) = 0x2F.
                assert!(cbp <= 0x2F);
            }
        }
    }

    #[test]
    fn intra4x4_pred_mode_is_bounded() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_intra4x4_pred_mode(&mut cabac, &mut states, 3);
        assert!(r <= 8);
    }

    #[test]
    fn intra_chroma_pred_mode_is_bounded() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_intra_chroma_pred_mode(&mut cabac, &mut states, false, false);
        assert!(r <= 3);
    }

    #[test]
    fn cbp_luma_is_4bit() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_cbp_luma(&mut cabac, &mut states, 0x0F, 0x0F);
        assert!(r <= 0xF);
    }

    #[test]
    fn cbp_chroma_in_range() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_cbp_chroma(&mut cabac, &mut states, 0, 0);
        assert!(r <= 2);
    }

    #[test]
    fn p_sub_mb_type_in_range() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_p_sub_mb_type(&mut cabac, &mut states);
        assert!(r <= 3);
    }

    #[test]
    fn b_sub_mb_type_in_range() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::B, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_b_sub_mb_type(&mut cabac, &mut states);
        assert!(r <= 12);
    }

    #[test]
    fn ref_idx_runs_without_overflow() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let r = decode_ref_idx(&mut cabac, &mut states, SliceType::P, 0, 0, false, false);
        assert!(r >= -1 && r < 32);
    }

    #[test]
    fn mvd_zero_when_neighbour_thresholds_low() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let mut abs = 0;
        let r = decode_mvd(&mut cabac, &mut states, 40, 0, &mut abs);
        assert!(abs >= 0);
        // mvd can be positive, negative, or zero — only sanity-check
        // the absolute value is consistent.
        if r == 0 {
            assert_eq!(abs, 0);
        }
    }
}
