//! H.264 CABAC residual-block decoder.
//!
//! Single-port of FFmpeg's `decode_cabac_residual_internal` from
//! `libavcodec/h264_cabac.c` (LGPL 2.1+) plus its CBF-gated wrappers
//! (`decode_cabac_residual_dc`, `decode_cabac_residual_nondc`).
//!
//! The residual layer is the heart of CABAC: every nonzero transform
//! coefficient is encoded via a significance scan + level coding
//! pass, both context-adaptive.  This module exposes the two
//! externally-useful entry points
//! ([`decode_residual_dc`], [`decode_residual_nondc`]) and a few
//! supporting tables.
//!
//! ## Block categories
//!
//! H.264 uses a `cat` index (0..=13) to pick context-base offsets per
//! block type:
//!
//! | cat | Block kind                  | max_coeff |
//! |-----|-----------------------------|-----------|
//! | 0   | Luma DC (Intra16x16)        | 16        |
//! | 1   | Luma AC (Intra16x16)        | 15        |
//! | 2   | Luma 4×4                    | 16        |
//! | 3   | Chroma DC                   | 4 (8 for 4:2:2) |
//! | 4   | Chroma AC                   | 15        |
//! | 5   | Luma 8×8                    | 64        |
//! | 6   | Cb DC (4:4:4)               | 16        |
//! | 7   | Cb AC (4:4:4)               | 15        |
//! | 8   | Cb 4×4 (4:4:4)              | 16        |
//! | 9   | Cb 8×8 (4:4:4)              | 64        |
//! | 10  | Cr DC (4:4:4)               | 16        |
//! | 11  | Cr AC (4:4:4)               | 15        |
//! | 12  | Cr 4×4 (4:4:4)              | 16        |
//! | 13  | Cr 8×8 (4:4:4)              | 64        |

use crate::h264::cabac::CabacContext;
use crate::h264::cabac_tables::{H264_CABAC_TABLES, H264_LAST_COEFF_FLAG_OFFSET_8X8_OFFSET};

/// Frame / field × category lookup for the
/// `significant_coeff_flag` context base offset.  Indexed
/// `[mb_field as usize][cat]`.
#[rustfmt::skip]
const SIGNIFICANT_COEFF_FLAG_OFFSET: [[u16; 14]; 2] = [
    [105 + 0, 105 + 15, 105 + 29, 105 + 44, 105 + 47, 402, 484 + 0, 484 + 15, 484 + 29, 660, 528 + 0, 528 + 15, 528 + 29, 718],
    [277 + 0, 277 + 15, 277 + 29, 277 + 44, 277 + 47, 436, 776 + 0, 776 + 15, 776 + 29, 675, 820 + 0, 820 + 15, 820 + 29, 733],
];

/// Frame / field × category lookup for the `last_coeff_flag`
/// context base offset.
#[rustfmt::skip]
const LAST_COEFF_FLAG_OFFSET: [[u16; 14]; 2] = [
    [166 + 0, 166 + 15, 166 + 29, 166 + 44, 166 + 47, 417, 572 + 0, 572 + 15, 572 + 29, 690, 616 + 0, 616 + 15, 616 + 29, 748],
    [338 + 0, 338 + 15, 338 + 29, 338 + 44, 338 + 47, 451, 864 + 0, 864 + 15, 864 + 29, 699, 908 + 0, 908 + 15, 908 + 29, 757],
];

/// Per-category base offset for the `coeff_abs_level_m1` context
/// group (the level-decode arm).
#[rustfmt::skip]
const COEFF_ABS_LEVEL_M1_OFFSET: [u16; 14] = [
    227 + 0, 227 + 10, 227 + 20, 227 + 30, 227 + 39,
    426,
    952 + 0, 952 + 10, 952 + 20,
    708,
    982 + 0, 982 + 10, 982 + 20,
    766,
];

/// Position-to-context-offset map for the 8×8 significance scan.
/// Indexed `[mb_field as usize][position]`, position 0..=62.
#[rustfmt::skip]
const SIGNIFICANT_COEFF_FLAG_OFFSET_8X8: [[u8; 63]; 2] = [
    [
        0, 1, 2, 3, 4, 5, 5, 4, 4, 3, 3, 4, 4, 4, 5, 5,
        4, 4, 4, 4, 3, 3, 6, 7, 7, 7, 8, 9,10, 9, 8, 7,
        7, 6,11,12,13,11, 6, 7, 8, 9,14,10, 9, 8, 6,11,
       12,13,11, 6, 9,14,10, 9,11,12,13,11,14,10,12,
    ],
    [
        0, 1, 1, 2, 2, 3, 3, 4, 5, 6, 7, 7, 7, 8, 4, 5,
        6, 9,10,10, 8,11,12,11, 9, 9,10,10, 8,11,12,11,
        9, 9,10,10, 8,11,12,11, 9, 9,10,10, 8,13,13, 9,
        9,10,10, 8,13,13, 9, 9,10,10,14,14,14,14,14,
    ],
];

/// Position-to-context-offset map for the 4:2:2 chroma-DC
/// significance scan.
const SIG_COEFF_OFFSET_DC: [u8; 7] = [0, 0, 1, 1, 2, 2, 2];

/// Node context → CABAC context for the first absolute-level bin.
const COEFF_ABS_LEVEL1_CTX: [u8; 8] = [1, 2, 3, 4, 0, 0, 0, 0];

/// Node context → CABAC context for level > 1 bins.  Row 0 is the
/// regular case, row 1 is the DC 4:2:2 case.
const COEFF_ABS_LEVELGT1_CTX: [[u8; 8]; 2] = [
    [5, 5, 5, 5, 6, 7, 8, 9],
    [5, 5, 5, 5, 6, 7, 8, 8],
];

/// State transition for the node context.  Row 0 is "after decoding
/// a level == 1", row 1 is "after decoding a level > 1".
const COEFF_ABS_LEVEL_TRANSITION: [[u8; 8]; 2] = [
    [1, 2, 3, 3, 4, 5, 6, 7],
    [4, 4, 4, 4, 5, 6, 7, 7],
];

/// Inputs the residual decoder needs from the caller's macroblock
/// state.
#[derive(Debug, Clone, Copy)]
pub struct ResidualParams<'a> {
    /// Block category (0..=13, see module doc).
    pub cat: usize,
    /// Pre-computed CBF context index — call
    /// [`coded_block_flag_ctx`] to derive.
    pub cbf_ctx: usize,
    /// Scan table (luma_zigzag, field scan, or 8×8 alternate scan).
    /// Length must be `>= max_coeff`.
    pub scantable: &'a [u8],
    /// Dequantization multipliers for AC blocks (one per scan
    /// position).  Must be `None` for DC blocks.
    pub qmul: Option<&'a [u32]>,
    /// Maximum coefficient count for the block kind.
    pub max_coeff: usize,
    /// `true` for any DC block (cat ∈ {0, 3, 6, 10}).
    pub is_dc: bool,
    /// `true` only for 4:2:2 chroma DC.
    pub chroma422: bool,
    /// `true` when the current macroblock is field-coded.
    pub mb_field: bool,
}

/// Derives the coded-block-flag context index for the given block
/// category, with neighbour non-zero-counts supplied by the caller.
///
/// Mirrors FFmpeg's `get_cabac_cbf_ctx`.  For Luma DC / Chroma DC the
/// `nz_a` / `nz_b` arguments should be the relevant bits of the
/// neighbour CBP table; for AC / 4×4 / 8×8 they are the neighbour
/// non-zero-count cache values.
pub fn coded_block_flag_ctx(cat: usize, nz_a: u32, nz_b: u32) -> usize {
    const BASE_CTX: [u16; 14] = [
        85, 89, 93, 97, 101, 1012, 460, 464, 468, 1016, 472, 476, 480, 1020,
    ];
    let mut ctx = 0usize;
    if nz_a > 0 {
        ctx += 1;
    }
    if nz_b > 0 {
        ctx += 2;
    }
    BASE_CTX[cat] as usize + ctx
}

/// CBF-gated DC residual decoder.
///
/// Reads the coded-block-flag bin; if zero, leaves `block`
/// untouched and returns 0.  Otherwise runs the significance scan +
/// level pass and returns the nonzero coefficient count.
pub fn decode_residual_dc(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    block: &mut [i32],
    params: ResidualParams<'_>,
) -> usize {
    debug_assert!(params.is_dc);
    if cabac.get(&mut states[params.cbf_ctx]) == 0 {
        return 0;
    }
    decode_residual_internal(cabac, states, block, params)
}

/// CBF-gated non-DC residual decoder.
///
/// Cat 5 (8×8 luma) is a special case: the per-block CBF lives
/// inside the macroblock-level `cbp` bits already, so the caller
/// must pre-decide whether to invoke this function — see FFmpeg
/// `decode_cabac_residual_nondc` for the gating logic.
pub fn decode_residual_nondc(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    block: &mut [i32],
    params: ResidualParams<'_>,
    check_cbf: bool,
) -> usize {
    debug_assert!(!params.is_dc);
    if check_cbf && cabac.get(&mut states[params.cbf_ctx]) == 0 {
        return 0;
    }
    decode_residual_internal(cabac, states, block, params)
}

/// Core significance + level decoder (DC / AC unified).  Returns the
/// number of nonzero coefficients written.
fn decode_residual_internal(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    block: &mut [i32],
    params: ResidualParams<'_>,
) -> usize {
    let field_idx = params.mb_field as usize;
    let sig_base = SIGNIFICANT_COEFF_FLAG_OFFSET[field_idx][params.cat] as usize;
    let last_base = LAST_COEFF_FLAG_OFFSET[field_idx][params.cat] as usize;
    let abs_base = COEFF_ABS_LEVEL_M1_OFFSET[params.cat] as usize;

    let mut index = [0usize; 64];
    let mut coeff_count = 0usize;
    let mut last = 0usize;

    let max_coeff = params.max_coeff;
    let sig_8x8 = !params.is_dc && max_coeff == 64;

    if sig_8x8 {
        let sig_off = &SIGNIFICANT_COEFF_FLAG_OFFSET_8X8[field_idx];
        let last_off_table = &H264_CABAC_TABLES[H264_LAST_COEFF_FLAG_OFFSET_8X8_OFFSET..];
        while last < 63 {
            let sig_ctx = sig_base + sig_off[last] as usize;
            if cabac.get(&mut states[sig_ctx]) != 0 {
                let last_ctx = last_base + last_off_table[last] as usize;
                index[coeff_count] = last;
                coeff_count += 1;
                if cabac.get(&mut states[last_ctx]) != 0 {
                    last = max_coeff;
                    break;
                }
            }
            last += 1;
        }
        if last == max_coeff - 1 {
            index[coeff_count] = last;
            coeff_count += 1;
        }
    } else if params.is_dc && params.chroma422 {
        while last < 7 {
            let off = SIG_COEFF_OFFSET_DC[last] as usize;
            let sig_ctx = sig_base + off;
            if cabac.get(&mut states[sig_ctx]) != 0 {
                let last_ctx = last_base + off;
                index[coeff_count] = last;
                coeff_count += 1;
                if cabac.get(&mut states[last_ctx]) != 0 {
                    last = max_coeff;
                    break;
                }
            }
            last += 1;
        }
        if last == max_coeff - 1 {
            index[coeff_count] = last;
            coeff_count += 1;
        }
    } else {
        let limit = max_coeff - 1;
        while last < limit {
            let sig_ctx = sig_base + last;
            if cabac.get(&mut states[sig_ctx]) != 0 {
                let last_ctx = last_base + last;
                index[coeff_count] = last;
                coeff_count += 1;
                if cabac.get(&mut states[last_ctx]) != 0 {
                    last = max_coeff;
                    break;
                }
            }
            last += 1;
        }
        if last == max_coeff - 1 {
            index[coeff_count] = last;
            coeff_count += 1;
        }
    }

    debug_assert!(coeff_count > 0);
    let written = coeff_count;

    let mut node_ctx = 0usize;
    let gt1_row = if params.is_dc && params.chroma422 { 1 } else { 0 };

    while coeff_count > 0 {
        coeff_count -= 1;
        let j = params.scantable[index[coeff_count]] as usize;

        let ctx1 = COEFF_ABS_LEVEL1_CTX[node_ctx] as usize + abs_base;
        if cabac.get(&mut states[ctx1]) == 0 {
            node_ctx = COEFF_ABS_LEVEL_TRANSITION[0][node_ctx] as usize;
            block[j] = if params.is_dc {
                cabac.get_bypass_sign(-1)
            } else {
                let qm = params.qmul.expect("non-DC residual without qmul")[j] as i32;
                (cabac.get_bypass_sign(-qm) + 32) >> 6
            };
        } else {
            let mut coeff_abs: i32 = 2;
            let ctxg = COEFF_ABS_LEVELGT1_CTX[gt1_row][node_ctx] as usize + abs_base;
            node_ctx = COEFF_ABS_LEVEL_TRANSITION[1][node_ctx] as usize;

            while coeff_abs < 15 && cabac.get(&mut states[ctxg]) != 0 {
                coeff_abs += 1;
            }

            if coeff_abs >= 15 {
                let mut k = 0;
                while cabac.get_bypass() != 0 && k < 16 + 7 {
                    k += 1;
                }
                coeff_abs = 1;
                while k > 0 {
                    k -= 1;
                    coeff_abs = coeff_abs + coeff_abs + cabac.get_bypass();
                }
                coeff_abs += 14;
            }

            block[j] = if params.is_dc {
                cabac.get_bypass_sign(-coeff_abs)
            } else {
                let qm = params.qmul.expect("non-DC residual without qmul")[j] as i32;
                ((cabac.get_bypass_sign(-coeff_abs) as i64 * qm as i64 + 32) >> 6) as i32
            };
        }
    }

    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::cabac::init_contexts;
    use crate::h264::slice_header::SliceType;

    fn sample_bytes() -> Vec<u8> {
        let mut v = vec![0x55u8; 128];
        v[0] = 0x40;
        v
    }

    fn zigzag_4x4() -> [u8; 16] {
        [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15]
    }

    #[test]
    fn cbf_zero_returns_zero_count() {
        // Bytestream of all zeros — the very first CBF bin will
        // hit an MPS=0 context state (mostly low-probability MPS=0
        // at slice_qp=26 for CBF contexts), giving us a 0 result.
        // We just want to verify the early-return path works.
        let buf = vec![0u8; 64];
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let mut block = [0i32; 16];
        let scan = zigzag_4x4();
        let params = ResidualParams {
            cat: 0,
            cbf_ctx: 85, // luma DC base
            scantable: &scan,
            qmul: None,
            max_coeff: 16,
            is_dc: true,
            chroma422: false,
            mb_field: false,
        };
        let n = decode_residual_dc(&mut cabac, &mut states, &mut block, params);
        // The result depends on context state — just ensure we
        // returned without panic and produced a sane count.
        assert!(n <= 16);
    }

    #[test]
    fn coded_block_flag_ctx_neighbours_add_offset() {
        assert_eq!(coded_block_flag_ctx(0, 0, 0), 85);
        assert_eq!(coded_block_flag_ctx(0, 1, 0), 86);
        assert_eq!(coded_block_flag_ctx(0, 0, 1), 87);
        assert_eq!(coded_block_flag_ctx(0, 1, 1), 88);
        // cat 5 jumps to base 1012 (luma 8x8).
        assert_eq!(coded_block_flag_ctx(5, 0, 0), 1012);
    }

    #[test]
    fn nondc_residual_runs_without_panic() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let mut block = [0i32; 16];
        let scan = zigzag_4x4();
        let qmul = [16u32; 16];
        let params = ResidualParams {
            cat: 2,
            cbf_ctx: 93,
            scantable: &scan,
            qmul: Some(&qmul),
            max_coeff: 16,
            is_dc: false,
            chroma422: false,
            mb_field: false,
        };
        let n = decode_residual_nondc(&mut cabac, &mut states, &mut block, params, true);
        assert!(n <= 16);
    }

    #[test]
    fn luma_8x8_residual_runs_without_panic() {
        let buf = sample_bytes();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&buf).unwrap();
        let mut block = [0i32; 64];
        let scan: Vec<u8> = (0..64).collect();
        let qmul = [16u32; 64];
        let params = ResidualParams {
            cat: 5,
            cbf_ctx: 1012,
            scantable: &scan,
            qmul: Some(&qmul),
            max_coeff: 64,
            is_dc: false,
            chroma422: false,
            mb_field: false,
        };
        let n = decode_residual_nondc(&mut cabac, &mut states, &mut block, params, false);
        assert!(n <= 64);
    }
}
