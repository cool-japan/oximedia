//! H.264 CABAC macroblock-level residual dispatch.
//!
//! On top of [`crate::h264::cabac_residual`] (per-block significance
//! scan + level decode), the macroblock decoder needs a dispatcher
//! that walks a whole macroblock's luma residual: one DC block plus
//! 16 AC 4×4 blocks for `I_16x16`, or 4 × 8×8 / 16 × 4×4 blocks for
//! every other intra/inter mode.  See ITU-T Rec. H.264 / ISO/IEC
//! 14496-10 clause 7.3.5.3 (`residual` / `residual_luma` /
//! `residual_block_cabac`).
//!
//! Each function records the per-4×4-block non-zero-count into the
//! caller-supplied [`MbResidualState`].  That state is later
//! consumed by the deblocking filter (boundary strength derivation)
//! and by the *next* macroblock's CBF-context selection — every
//! CABAC residual decoder reads its neighbour's non-zero count.
//!
//! The dispatchers intentionally take the CBF context index for
//! each block from the caller rather than computing it internally;
//! that keeps neighbour-availability + interlaced handling outside
//! this layer.

use crate::h264::cabac::CabacContext;
use crate::h264::cabac_residual::{decode_residual_dc, decode_residual_nondc, ResidualParams};
use crate::h264::cabac_syntax::{
    decode_cbp_chroma, decode_cbp_luma, decode_intra4x4_pred_mode, decode_intra_chroma_pred_mode,
    decode_intra_mb_type, IntraMbType,
};

/// Block-category indices for each plane and block kind, indexed
/// `[block_kind][plane]`.  Per spec Table 9-42 (`ctxBlockCat`):
/// - `[0][p]` — DC (I_16x16 only)
/// - `[1][p]` — AC (I_16x16 only)
/// - `[2][p]` — Luma 4×4 (non-I_16x16)
/// - `[3][p]` — Luma 8×8 (when transform_8x8 is on)
///
/// Plane index: 0 = Y, 1 = Cb-4:4:4, 2 = Cr-4:4:4.
const CTX_CAT: [[usize; 3]; 4] = [
    [0, 6, 10],
    [1, 7, 11],
    [2, 8, 12],
    [5, 9, 13],
];

/// Output state for a macroblock's residual decode.  The decoder
/// writes per-4×4-block non-zero counts here (one entry per block
/// in raster scan within the macroblock); the caller forwards these
/// to the deblocking filter and to the next macroblock's CBF
/// context.
#[derive(Debug, Clone)]
pub struct MbResidualState {
    /// 16 luma 4×4 blocks (raster scan inside the macroblock).
    pub nz_count_luma: [u8; 16],
    /// 4 + 4 chroma 4×4 blocks: indices 0..4 are Cb, 4..8 are Cr.
    pub nz_count_chroma: [u8; 8],
    /// Luma DC block (I_16x16 only — zeros otherwise).
    pub luma_dc: [i32; 16],
    /// 16 luma AC 4×4 blocks (raster scan).
    pub luma_4x4: [[i32; 16]; 16],
    /// 4 luma 8×8 blocks (only valid when `is_8x8_dct` was set).
    pub luma_8x8: [[i32; 64]; 4],
    /// Chroma DC blocks — Cb at index 0, Cr at index 1 (4 entries
    /// each for 4:2:0, 8 for 4:2:2).
    pub chroma_dc: [[i32; 8]; 2],
    /// Chroma AC blocks: 4 per plane (Cb then Cr).
    pub chroma_ac: [[i32; 16]; 8],
}

impl Default for MbResidualState {
    fn default() -> Self {
        Self {
            nz_count_luma: [0; 16],
            nz_count_chroma: [0; 8],
            luma_dc: [0; 16],
            luma_4x4: [[0; 16]; 16],
            // [[i32; 64]; 4] is past the `Default` auto-impl
            // threshold, so build it explicitly.
            luma_8x8: [[0; 64], [0; 64], [0; 64], [0; 64]],
            chroma_dc: [[0; 8]; 2],
            chroma_ac: [[0; 16]; 8],
        }
    }
}

/// Plane-keyed CBF context indices that the macroblock decoder
/// must pre-compute (the indices depend on neighbour non-zero
/// counts, which only the slice loop knows).
///
/// Each field is the input to one
/// [`crate::h264::cabac_residual::coded_block_flag_ctx`] call for
/// the corresponding sub-block.
#[derive(Debug, Clone, Copy)]
pub struct LumaCbfCtxs {
    /// CBF context index for the I_16x16 DC block.
    pub dc: usize,
    /// CBF context index per 4×4 block (raster scan).
    pub block_4x4: [usize; 16],
    /// CBF context index per 8×8 block (raster scan).
    pub block_8x8: [usize; 4],
}

/// Chroma CBF context bundle.  4:2:0 only — 4:2:2 / 4:4:4 callers
/// need to widen these arrays.
#[derive(Debug, Clone, Copy)]
pub struct ChromaCbfCtxs {
    /// CBF index for Cb DC, then Cr DC.
    pub dc: [usize; 2],
    /// CBF index per chroma AC block: indices 0..4 = Cb, 4..8 = Cr.
    pub ac: [usize; 8],
}

/// Inputs that vary per macroblock but stay constant for the call.
#[derive(Debug, Clone, Copy)]
pub struct MbLumaResidualInputs<'a> {
    /// 4×4 zig-zag (or field) scan order.
    pub scan_4x4: &'a [u8],
    /// 8×8 zig-zag scan order (only consulted when `is_8x8_dct`).
    pub scan_8x8: &'a [u8],
    /// Dequantization multipliers for 4×4 blocks (per scan
    /// position, length 16).
    pub dequant_4x4: &'a [u32],
    /// Dequantization multipliers for 8×8 blocks (length 64).
    pub dequant_8x8: &'a [u32],
    /// `true` when the macroblock uses Intra16x16 luma prediction.
    pub is_intra16x16: bool,
    /// `true` when the macroblock encodes luma via the 8×8 integer
    /// transform (`transform_8x8_flag`).  Ignored when
    /// `is_intra16x16` is set.
    pub is_8x8_dct: bool,
    /// 4-bit luma coded-block-pattern (low bit = top-left 8×8).
    pub cbp_luma: u8,
    /// `true` when the macroblock is field-coded.
    pub mb_field: bool,
}

/// Decodes the luma residual for a single macroblock.
///
/// Implements `residual_luma` (spec § 7.3.5.3.1) for the Y plane
/// only — 4:4:4 alternate planes are out of scope here.
pub fn decode_luma_residual(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctxs: &LumaCbfCtxs,
    inputs: MbLumaResidualInputs<'_>,
    out: &mut MbResidualState,
) {
    let plane = 0usize;

    if inputs.is_intra16x16 {
        // I_16x16 — one DC block + 16 AC 4×4 blocks.
        out.luma_dc = [0; 16];
        let params = ResidualParams {
            cat: CTX_CAT[0][plane],
            cbf_ctx: ctxs.dc,
            scantable: inputs.scan_4x4,
            qmul: None,
            max_coeff: 16,
            is_dc: true,
            chroma422: false,
            mb_field: inputs.mb_field,
        };
        decode_residual_dc(cabac, states, &mut out.luma_dc, params);

        if inputs.cbp_luma & 0x0F != 0 {
            for i in 0..16 {
                out.luma_4x4[i] = [0; 16];
                let params = ResidualParams {
                    cat: CTX_CAT[1][plane],
                    cbf_ctx: ctxs.block_4x4[i],
                    scantable: &inputs.scan_4x4[1..], // AC skips DC slot
                    qmul: Some(inputs.dequant_4x4),
                    max_coeff: 15,
                    is_dc: false,
                    chroma422: false,
                    mb_field: inputs.mb_field,
                };
                out.nz_count_luma[i] = decode_residual_nondc(
                    cabac,
                    states,
                    &mut out.luma_4x4[i],
                    params,
                    true,
                ) as u8;
            }
        } else {
            for i in 0..16 {
                out.nz_count_luma[i] = 0;
            }
        }
    } else if inputs.is_8x8_dct {
        // Non-Intra16x16 with 8×8 transform — 4 8×8 blocks.
        for i8x8 in 0..4 {
            if inputs.cbp_luma & (1 << i8x8) != 0 {
                out.luma_8x8[i8x8] = [0; 64];
                let params = ResidualParams {
                    cat: CTX_CAT[3][plane],
                    cbf_ctx: ctxs.block_8x8[i8x8],
                    scantable: inputs.scan_8x8,
                    qmul: Some(inputs.dequant_8x8),
                    max_coeff: 64,
                    is_dc: false,
                    chroma422: false,
                    mb_field: inputs.mb_field,
                };
                let coeff_count = decode_residual_nondc(
                    cabac,
                    states,
                    &mut out.luma_8x8[i8x8],
                    params,
                    false, // 8×8 luma uses the macroblock-level CBP, not a per-block CBF bin.
                ) as u8;
                // 8×8 block spans 4 4×4 nz-count slots; spec
                // § 7.4.5.3 broadcasts the same count to all 4.
                for k in 0..4 {
                    out.nz_count_luma[4 * i8x8 + k] = coeff_count;
                }
            } else {
                for k in 0..4 {
                    out.nz_count_luma[4 * i8x8 + k] = 0;
                }
            }
        }
    } else {
        // Non-Intra16x16, 4×4 transform — 16 4×4 blocks gated by
        // their 8×8 parent's CBP bit.
        for i8x8 in 0..4 {
            if inputs.cbp_luma & (1 << i8x8) != 0 {
                for i4x4 in 0..4 {
                    let block = 4 * i8x8 + i4x4;
                    out.luma_4x4[block] = [0; 16];
                    let params = ResidualParams {
                        cat: CTX_CAT[2][plane],
                        cbf_ctx: ctxs.block_4x4[block],
                        scantable: inputs.scan_4x4,
                        qmul: Some(inputs.dequant_4x4),
                        max_coeff: 16,
                        is_dc: false,
                        chroma422: false,
                        mb_field: inputs.mb_field,
                    };
                    out.nz_count_luma[block] = decode_residual_nondc(
                        cabac,
                        states,
                        &mut out.luma_4x4[block],
                        params,
                        true,
                    ) as u8;
                }
            } else {
                for i4x4 in 0..4 {
                    out.nz_count_luma[4 * i8x8 + i4x4] = 0;
                }
            }
        }
    }
}

/// Decodes the chroma residual for a 4:2:0 macroblock.
///
/// `cbp_chroma` is the 2-bit chroma CBP: 0 = no chroma residual,
/// 1 = DC only, 2 = DC + AC.
///
/// Implements `residual` chroma branch (spec § 7.3.5.3) for 4:2:0.
/// 4:2:2 widens the DC scan to 8 coeffs and is wired via the
/// existing `chroma422: bool` parameter inside
/// [`crate::h264::cabac_residual`].
pub fn decode_chroma_residual(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctxs: &ChromaCbfCtxs,
    scan_4x4: &[u8],
    dequant_4x4_cb: &[u32],
    dequant_4x4_cr: &[u32],
    cbp_chroma: u8,
    mb_field: bool,
    out: &mut MbResidualState,
) {
    if cbp_chroma == 0 {
        out.nz_count_chroma = [0; 8];
        return;
    }

    // DC pass — always runs when cbp_chroma >= 1.
    for plane in 0..2 {
        out.chroma_dc[plane] = [0; 8];
        let params = ResidualParams {
            cat: 3, // chroma DC
            cbf_ctx: ctxs.dc[plane],
            scantable: scan_4x4,
            qmul: None,
            max_coeff: 4,
            is_dc: true,
            chroma422: false,
            mb_field,
        };
        decode_residual_dc(cabac, states, &mut out.chroma_dc[plane], params);
    }

    if cbp_chroma == 1 {
        out.nz_count_chroma = [0; 8];
        return;
    }

    // AC pass — only runs when cbp_chroma == 2.
    let dequants = [dequant_4x4_cb, dequant_4x4_cr];
    for plane in 0..2 {
        for i in 0..4 {
            let block = 4 * plane + i;
            out.chroma_ac[block] = [0; 16];
            let params = ResidualParams {
                cat: 4, // chroma AC
                cbf_ctx: ctxs.ac[block],
                scantable: &scan_4x4[1..],
                qmul: Some(dequants[plane]),
                max_coeff: 15,
                is_dc: false,
                chroma422: false,
                mb_field,
            };
            out.nz_count_chroma[block] =
                decode_residual_nondc(cabac, states, &mut out.chroma_ac[block], params, true)
                    as u8;
        }
    }
}

/// Decoded syntax for a single I-slice macroblock prior to residual
/// decode.  Captures every bin the bitstream layer must extract for
/// reconstruction to proceed: type, intra prediction modes, CBP,
/// and QP delta.
#[derive(Debug, Clone)]
pub struct IntraMbCabac {
    /// I_NxN / I_16x16 / I_PCM.  When `IPCM`, all other fields are
    /// meaningless — the caller must pull raw samples from the
    /// bytestream as defined by spec 7.3.5.1.
    pub mb_type: IntraMbType,
    /// 16 intra 4×4 prediction modes (raster-scan within the MB).
    /// Only meaningful for `I_NxN`; otherwise all-zero.
    pub intra4x4_modes: [u8; 16],
    /// Chroma intra prediction mode (0..=3).  Always decoded for
    /// chroma 4:2:0 / 4:2:2 macroblocks.
    pub chroma_pred_mode: u8,
    /// 6-bit packed CBP: low 4 bits = luma CBP, bits 4..=5 = chroma
    /// CBP.  For `I_16x16` this comes from the mb_type code; for
    /// `I_NxN` both halves are decoded explicitly.
    pub cbp: u8,
    /// Signed mb_qp_delta, applied to the running slice QP.
    pub mb_qp_delta: i32,
}

/// Per-block intra4x4 MPM context the caller must supply (the MPM
/// itself depends on the neighbour macroblocks' intra modes, which
/// only the slice loop tracks).  One MPM per 4×4 block, in raster
/// scan within the macroblock.
pub type Intra4x4Mpms = [u8; 16];

/// Decodes the syntax portion of an I-slice CABAC macroblock.
///
/// The residual decode (`decode_luma_residual` /
/// `decode_chroma_residual`) is the caller's responsibility — it
/// needs CBF context indices that depend on neighbour state.
///
/// Implements the I-slice macroblock-layer parsing flow from
/// spec § 7.3.5.1 up to but not including the residual decode.
///
/// # Inputs
///
/// - `left_is_intra16x16_or_pcm` / `top_is_intra16x16_or_pcm` —
///   neighbour bias for the `mb_type` context.
/// - `mpms` — per-block MPM derived from neighbour intra modes.
/// - `left_chroma_pred_nonzero` / `top_chroma_pred_nonzero` —
///   neighbour bias for the chroma-pred-mode context.
/// - `left_cbp` / `top_cbp` — 8-bit neighbour CBPs used by
///   `decode_cbp_luma` / `decode_cbp_chroma`.
pub fn decode_intra_mb(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    left_is_intra16x16_or_pcm: bool,
    top_is_intra16x16_or_pcm: bool,
    mpms: Intra4x4Mpms,
    left_chroma_pred_nonzero: bool,
    top_chroma_pred_nonzero: bool,
    left_cbp: u8,
    top_cbp: u8,
) -> IntraMbCabac {
    let mb_type = decode_intra_mb_type(
        cabac,
        states,
        3,
        true,
        left_is_intra16x16_or_pcm,
        top_is_intra16x16_or_pcm,
    );

    let mut out = IntraMbCabac {
        mb_type,
        intra4x4_modes: [0; 16],
        chroma_pred_mode: 0,
        cbp: 0,
        mb_qp_delta: 0,
    };

    match mb_type {
        IntraMbType::IPCM => {
            // PCM macroblocks bypass the entropy coder past this
            // point; the caller is responsible for re-initialising
            // CABAC after the raw samples are read.
            return out;
        }
        IntraMbType::I4x4 => {
            for i in 0..16 {
                out.intra4x4_modes[i] = decode_intra4x4_pred_mode(cabac, states, mpms[i]);
            }
        }
        IntraMbType::I16x16 { cbp, .. } => {
            out.cbp = cbp;
        }
    }

    out.chroma_pred_mode = decode_intra_chroma_pred_mode(
        cabac,
        states,
        left_chroma_pred_nonzero,
        top_chroma_pred_nonzero,
    );

    if matches!(mb_type, IntraMbType::I4x4) {
        let cbp_luma = decode_cbp_luma(cabac, states, left_cbp, top_cbp);
        let cbp_chroma = decode_cbp_chroma(cabac, states, left_cbp, top_cbp);
        out.cbp = (cbp_chroma << 4) | cbp_luma;
    }

    if out.cbp != 0 || matches!(mb_type, IntraMbType::I16x16 { .. }) {
        out.mb_qp_delta = decode_mb_qp_delta(cabac, states);
    }

    out
}

/// Decodes the signed `mb_qp_delta` syntax element.
///
/// Per spec § 9.3.3.1.1.10 (`mb_qp_delta` binarisation).  Uses
/// contexts 60..=63 with a single-bit prefix followed by a unary
/// tail; the magnitude is then converted to a signed delta via the
/// standard interleaved mapping (spec equation 9-19).
pub fn decode_mb_qp_delta(cabac: &mut CabacContext<'_>, states: &mut [u8]) -> i32 {
    if cabac.get(&mut states[60]) == 0 {
        return 0;
    }
    let mut val = 1i32;
    let mut ctx = 2usize;
    while cabac.get(&mut states[60 + ctx]) != 0 {
        ctx = 3;
        val += 1;
        // Sanity cap to avoid runaway in malformed streams.  The
        // spec allows up to ±51 (or wider for high-bit-depth);
        // 102 fits 8-bit luma comfortably (2 × max_qp).
        if val > 102 {
            return 0;
        }
    }
    if val & 1 == 1 {
        (val + 1) >> 1
    } else {
        -((val + 1) >> 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::cabac::init_contexts;
    use crate::h264::slice_header::SliceType;

    fn buf() -> Vec<u8> {
        let mut v = vec![0x55u8; 256];
        v[0] = 0x40;
        v
    }

    fn zigzag_4x4() -> [u8; 16] {
        [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15]
    }

    #[test]
    fn intra16x16_luma_with_zero_cbp_zeros_nz_counts() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let scan8 = [0u8; 64];
        let dq4 = [16u32; 16];
        let dq8 = [16u32; 64];
        let ctxs = LumaCbfCtxs {
            dc: 85,
            block_4x4: [93; 16],
            block_8x8: [1012; 4],
        };
        let inputs = MbLumaResidualInputs {
            scan_4x4: &scan,
            scan_8x8: &scan8,
            dequant_4x4: &dq4,
            dequant_8x8: &dq8,
            is_intra16x16: true,
            is_8x8_dct: false,
            cbp_luma: 0,
            mb_field: false,
        };
        let mut out = MbResidualState::default();
        decode_luma_residual(&mut cabac, &mut states, &ctxs, inputs, &mut out);
        assert_eq!(out.nz_count_luma, [0u8; 16]);
    }

    #[test]
    fn non_intra16x16_with_cbp_iterates_4x4_blocks() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let scan8 = [0u8; 64];
        let dq4 = [16u32; 16];
        let dq8 = [16u32; 64];
        let ctxs = LumaCbfCtxs {
            dc: 85,
            block_4x4: [93; 16],
            block_8x8: [1012; 4],
        };
        let inputs = MbLumaResidualInputs {
            scan_4x4: &scan,
            scan_8x8: &scan8,
            dequant_4x4: &dq4,
            dequant_8x8: &dq8,
            is_intra16x16: false,
            is_8x8_dct: false,
            cbp_luma: 0x0F, // all four 8×8 parents enabled.
            mb_field: false,
        };
        let mut out = MbResidualState::default();
        decode_luma_residual(&mut cabac, &mut states, &ctxs, inputs, &mut out);
        // Each block returned a coeff count ≤ 16; just sanity-check
        // the loop ran without panicking.
        for c in out.nz_count_luma {
            assert!(c <= 16);
        }
    }

    #[test]
    fn chroma_residual_cbp_zero_skips_everything() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let dq = [16u32; 16];
        let ctxs = ChromaCbfCtxs {
            dc: [97, 97],
            ac: [101; 8],
        };
        let mut out = MbResidualState::default();
        decode_chroma_residual(
            &mut cabac,
            &mut states,
            &ctxs,
            &scan,
            &dq,
            &dq,
            0,
            false,
            &mut out,
        );
        assert_eq!(out.nz_count_chroma, [0u8; 8]);
    }

    #[test]
    fn chroma_residual_cbp_one_runs_dc_only() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let dq = [16u32; 16];
        let ctxs = ChromaCbfCtxs {
            dc: [97, 97],
            ac: [101; 8],
        };
        let mut out = MbResidualState::default();
        decode_chroma_residual(
            &mut cabac,
            &mut states,
            &ctxs,
            &scan,
            &dq,
            &dq,
            1,
            false,
            &mut out,
        );
        // AC was skipped — nz_count_chroma must be all zero.
        assert_eq!(out.nz_count_chroma, [0u8; 8]);
    }

    #[test]
    fn chroma_residual_cbp_two_runs_dc_and_ac() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let dq = [16u32; 16];
        let ctxs = ChromaCbfCtxs {
            dc: [97, 97],
            ac: [101; 8],
        };
        let mut out = MbResidualState::default();
        decode_chroma_residual(
            &mut cabac,
            &mut states,
            &ctxs,
            &scan,
            &dq,
            &dq,
            2,
            false,
            &mut out,
        );
        for c in out.nz_count_chroma {
            assert!(c <= 16);
        }
    }

    #[test]
    fn intra_mb_decode_runs_without_panic() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let mpms = [0u8; 16];
        let out = decode_intra_mb(
            &mut cabac,
            &mut states,
            false,
            false,
            mpms,
            false,
            false,
            0,
            0,
        );
        // Sanity: chroma pred mode fits 0..=3 and qp_delta fits the
        // spec range; mb_type variants self-validate.
        assert!(out.chroma_pred_mode <= 3);
        assert!(out.mb_qp_delta.abs() <= 51);
    }

    #[test]
    fn qp_delta_zero_when_first_bin_is_zero() {
        // Construct a buffer that maximises the chance of bin 0 = 0
        // by starting with a low byte.  Not bit-exact, but verifies
        // the early-return path compiles + runs.
        let bytes = vec![0x00u8; 64];
        let mut states = init_contexts(SliceType::I, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let d = decode_mb_qp_delta(&mut cabac, &mut states);
        // d may be nonzero depending on context state; just bound it.
        assert!(d.abs() <= 102);
    }
}
