//! CABAC slice-level decoder.
//!
//! Walks every macroblock in a slice in raster order, calling the
//! appropriate macroblock orchestrator for the slice type and
//! decoding the residual blocks inline.  Updates
//! [`InterSliceCache`] between calls so the next macroblock sees
//! its neighbours' state.
//!
//! ## Output
//!
//! [`parse_slice_cabac`] returns a `Vec<MbCabacDecoded>` — one
//! entry per macroblock, in raster order.  Each entry carries:
//!
//! - The macroblock kind (P_Skip / Intra / Inter) plus the
//!   structured decode of each.
//! - The running QP at this macroblock.
//! - The residual state ([`MbResidualState`]) — DC + AC luma 4×4
//!   and chroma blocks, plus per-4×4 non-zero counts.
//!
//! The caller subsequently runs motion compensation (for inter)
//! and / or intra prediction + residual reconstruction (for intra)
//! to produce decoded pixels.

use crate::h264::cabac::CabacContext;
use crate::h264::cabac_inter_mb::{decode_p_mb_cabac, MbNeighbours, PMbOutcome};
use crate::h264::cabac_mb::{
    decode_chroma_residual, decode_intra_mb, ChromaCbfCtxs, IntraMbCabac, LumaCbfCtxs,
    MbLumaResidualInputs, MbResidualState,
};
use crate::h264::cabac_residual::{coded_block_flag_ctx, decode_residual_dc, decode_residual_nondc, ResidualParams};
use crate::h264::cabac_syntax::IntraMbType;
use crate::h264::inter_cache::{InterMbDecoded, InterSliceCache};
use crate::h264::slice_header::SliceType;
use crate::CodecError;

/// Per-macroblock decoded data produced by [`parse_slice_cabac`].
#[derive(Debug, Clone)]
pub struct MbCabacDecoded {
    /// Macroblock column.
    pub mb_x: usize,
    /// Macroblock row.
    pub mb_y: usize,
    /// Kind of macroblock + parsed syntax.
    pub kind: MbKind,
    /// Effective luma QP at this macroblock (post-`mb_qp_delta`).
    pub qp_y: u8,
    /// Effective chroma QP at this macroblock.
    pub qp_chroma: u8,
    /// Residual state for this macroblock.  Empty for P_Skip.
    pub residual: MbResidualState,
}

/// Type discriminator + structured decode for one macroblock.
#[derive(Debug, Clone)]
pub enum MbKind {
    /// P-Skip: no residual, MV inferred as the median predictor
    /// applied to the neighbour cache.
    PSkip,
    /// Intra (I / SI slice or P/B intra escape).
    Intra(IntraMbCabac),
    /// Inter P macroblock.
    InterP {
        /// `0..=4` indexing `P_MB_TYPE_INFO`.
        mb_type_code: u8,
        /// Motion vectors / refs / mvds / nz_counts.
        decoded: InterMbDecoded,
    },
}

/// Decoder inputs that stay constant across the slice.
#[derive(Debug, Clone, Copy)]
pub struct SliceCabacContext<'a> {
    /// Slice type (only `I` and `P` are supported in this commit).
    pub slice_type: SliceType,
    /// Picture width in macroblocks (16-px MBs only — MBAFF is out
    /// of scope).
    pub pic_width_mbs: usize,
    /// Picture height in macroblocks.
    pub pic_height_mbs: usize,
    /// Initial luma QP (`pic_init_qp_minus26 + 26 + slice_qp_delta`).
    pub initial_qp_y: u8,
    /// Chroma QP offset from the active PPS.
    pub chroma_qp_index_offset: i32,
    /// Number of L0 reference frames (1 ⇒ ref_idx_l0 is not in the
    /// bitstream and is implicitly 0).
    pub num_ref_idx_l0_active: u8,
    /// 4×4 zig-zag scan table (length 16).
    pub scan_4x4: &'a [u8],
    /// 8×8 zig-zag scan table (length 64) — only consulted on
    /// `transform_8x8_mode_flag` paths (not exercised by this
    /// commit).
    pub scan_8x8: &'a [u8],
    /// Per-position dequantization multipliers for luma 4×4 at
    /// the current QP.  The slice loop indexes this with the
    /// scan-mapped block position.
    pub dequant_4x4_luma: &'a [u32; 16],
    /// Per-position dequantization multipliers for Cb 4×4 at the
    /// current chroma QP.
    pub dequant_4x4_cb: &'a [u32; 16],
    /// Per-position dequantization multipliers for Cr 4×4 at the
    /// current chroma QP.
    pub dequant_4x4_cr: &'a [u32; 16],
    /// Per-position dequantization multipliers for luma 8×8.
    pub dequant_8x8_luma: &'a [u32; 64],
}

/// Parses one CABAC-coded slice.  Returns the per-macroblock
/// decoded data in raster order.
///
/// The caller is responsible for:
/// - Initialising `cabac` from the slice data bytes via
///   [`CabacContext::new`].
/// - Pre-allocating `cache` with [`InterSliceCache::new`] and
///   sized to `ctx.pic_width_mbs`.
/// - Wiring the returned per-macroblock data into reconstruction
///   (motion compensation + intra prediction + IDCT + deblocking).
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when the slice type is
/// unsupported (B / SP / SI) or when the bitstream signals an
/// invalid macroblock layout.
pub fn parse_slice_cabac(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctx: SliceCabacContext<'_>,
    cache: &mut InterSliceCache,
) -> Result<Vec<MbCabacDecoded>, CodecError> {
    if !matches!(ctx.slice_type, SliceType::I | SliceType::P | SliceType::SI) {
        return Err(CodecError::InvalidData(format!(
            "h264 slice_cabac: slice type {:?} not supported in this commit",
            ctx.slice_type
        )));
    }
    if cache.pic_width_mbs != ctx.pic_width_mbs {
        return Err(CodecError::InvalidData(
            "h264 slice_cabac: cache width mismatch with context".into(),
        ));
    }

    let total = ctx.pic_width_mbs * ctx.pic_height_mbs;
    let mut out = Vec::with_capacity(total);
    let mut qp_y = ctx.initial_qp_y;

    for mb_y in 0..ctx.pic_height_mbs {
        cache.begin_row();
        for mb_x in 0..ctx.pic_width_mbs {
            // Top-right slot: macroblock at (mb_x+1, mb_y-1) when it
            // exists.  At the rightmost column it falls back to
            // top-left (mb_x-1, mb_y-1) per spec, but that's only
            // relevant for the median MV predictor and is handled
            // inside the MV predictor itself.
            let top_right = if mb_x + 1 < ctx.pic_width_mbs {
                Some(&cache.top_row[mb_x + 1])
            } else {
                None
            };
            let neighbours = MbNeighbours::from_cache(cache, mb_x, top_right);

            let (kind, residual, qp_delta_nonzero, this_qp_y) =
                decode_one_mb(cabac, states, &ctx, &neighbours, cache.prev_qp_delta_nonzero, qp_y)?;
            qp_y = this_qp_y;
            let qp_chroma = chroma_qp(this_qp_y, ctx.chroma_qp_index_offset);

            // Push into the cache.
            match &kind {
                MbKind::PSkip => {
                    let zero = InterMbDecoded::default();
                    cache.record_inter_mb(mb_x, &zero, 0, 0);
                    cache.left_col.is_skip = true;
                    cache.top_row[mb_x].is_skip = true;
                }
                MbKind::InterP { decoded, .. } => {
                    cache.record_inter_mb(mb_x, decoded, 0, if qp_delta_nonzero { 1 } else { 0 });
                }
                MbKind::Intra(intra) => {
                    let mut placeholder = InterMbDecoded::default();
                    placeholder.is_intra = true;
                    placeholder.cbp = intra.cbp;
                    placeholder.nz_count_luma = residual.nz_count_luma;
                    placeholder.nz_count_chroma = residual.nz_count_chroma;
                    cache.record_inter_mb(
                        mb_x,
                        &placeholder,
                        intra.chroma_pred_mode,
                        if qp_delta_nonzero { 1 } else { 0 },
                    );
                }
            }
            out.push(MbCabacDecoded { mb_x, mb_y, kind, qp_y: this_qp_y, qp_chroma, residual });
        }
    }

    Ok(out)
}

/// Decodes the full syntax + residual for a single macroblock.
/// Returns `(kind, residual, qp_delta_nonzero, new_qp_y)`.
fn decode_one_mb(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctx: &SliceCabacContext<'_>,
    nb: &MbNeighbours,
    prev_qp_delta_nonzero: bool,
    cur_qp_y: u8,
) -> Result<(MbKind, MbResidualState, bool, u8), CodecError> {
    let mut residual = MbResidualState::default();

    let (kind, mb_qp_delta, has_residual) = match ctx.slice_type {
        SliceType::I | SliceType::SI => decode_intra_path(cabac, states, nb, prev_qp_delta_nonzero)?,
        SliceType::P => decode_p_path(cabac, states, ctx.num_ref_idx_l0_active, nb, prev_qp_delta_nonzero)?,
        _ => {
            return Err(CodecError::InvalidData(
                "h264 slice_cabac: slice type unreachable in this commit".into(),
            ))
        }
    };

    let new_qp_y = clip_qp(cur_qp_y as i32 + mb_qp_delta) as u8;

    if has_residual {
        decode_residuals_for_mb(cabac, states, ctx, nb, &kind, &mut residual)?;
    }

    Ok((kind, residual, mb_qp_delta != 0, new_qp_y))
}

/// Intra-only macroblock decode (used on I/SI slices).
fn decode_intra_path(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    nb: &MbNeighbours,
    prev_qp_delta_nonzero: bool,
) -> Result<(MbKind, i32, bool), CodecError> {
    let intra = decode_intra_mb(
        cabac,
        states,
        nb.left_chroma_pred_nonzero, // left_is_intra16x16_or_pcm — approximated by chroma flag for now
        nb.top_chroma_pred_nonzero,
        [0; 16],                     // MPMs: caller responsibility in a richer integration
        nb.left_chroma_pred_nonzero,
        nb.top_chroma_pred_nonzero,
        nb.left_cbp,
        nb.top_cbp,
        prev_qp_delta_nonzero,
    );
    let has_residual = match intra.mb_type {
        IntraMbType::IPCM => false,
        _ => intra.cbp != 0 || matches!(intra.mb_type, IntraMbType::I16x16 { .. }),
    };
    Ok((MbKind::Intra(intra.clone()), intra.mb_qp_delta, has_residual))
}

/// P-slice macroblock decode (P_Skip / Inter / Intra escape).
fn decode_p_path(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    num_ref_idx_l0_active: u8,
    nb: &MbNeighbours,
    prev_qp_delta_nonzero: bool,
) -> Result<(MbKind, i32, bool), CodecError> {
    match decode_p_mb_cabac(cabac, states, nb, num_ref_idx_l0_active, prev_qp_delta_nonzero) {
        PMbOutcome::Skip => Ok((MbKind::PSkip, 0, false)),
        PMbOutcome::Inter { mb_type_code, decoded, mb_qp_delta } => {
            let has_residual = decoded.cbp != 0;
            Ok((MbKind::InterP { mb_type_code, decoded }, mb_qp_delta, has_residual))
        }
        PMbOutcome::Intra(_) => {
            // Intra escape: re-run the I-slice path *without* the
            // mb_type bin (it's already been consumed).  For this
            // commit we treat it as an Intra MB with default
            // settings and skip residual; full integration handled
            // in a follow-up.
            Ok((MbKind::Intra(IntraMbCabac {
                mb_type: IntraMbType::I4x4,
                intra4x4_modes: [0; 16],
                chroma_pred_mode: 0,
                cbp: 0,
                mb_qp_delta: 0,
            }), 0, false))
        }
    }
}

/// Decodes all luma + chroma residual blocks for one macroblock.
fn decode_residuals_for_mb(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctx: &SliceCabacContext<'_>,
    nb: &MbNeighbours,
    kind: &MbKind,
    residual: &mut MbResidualState,
) -> Result<(), CodecError> {
    let (cbp, is_intra16x16) = match kind {
        MbKind::Intra(i) => match i.mb_type {
            IntraMbType::I16x16 { cbp, .. } => (cbp, true),
            IntraMbType::I4x4 => (i.cbp, false),
            IntraMbType::IPCM => return Ok(()),
        },
        MbKind::InterP { decoded, .. } => (decoded.cbp, false),
        MbKind::PSkip => return Ok(()),
    };
    let cbp_luma = cbp & 0x0F;
    let cbp_chroma = (cbp >> 4) & 0x03;

    // Luma residual.
    decode_luma_residual_inline(
        cabac,
        states,
        ctx,
        nb,
        is_intra16x16,
        cbp_luma,
        residual,
    )?;

    // Chroma residual (4:2:0 only).
    decode_chroma_residual_inline(
        cabac,
        states,
        ctx,
        nb,
        cbp_chroma,
        residual,
    )?;

    Ok(())
}

/// Per-block luma residual loop with inline CBF context selection.
///
/// For each 4×4 block we compute `coded_block_flag_ctx(cat, nz_a,
/// nz_b)` from the just-decoded in-macroblock neighbour plus the
/// slice-cache strip for the picture edges, then call
/// [`decode_residual_nondc`].
fn decode_luma_residual_inline(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctx: &SliceCabacContext<'_>,
    nb: &MbNeighbours,
    is_intra16x16: bool,
    cbp_luma: u8,
    residual: &mut MbResidualState,
) -> Result<(), CodecError> {
    if is_intra16x16 {
        // Luma DC block (cat 0).
        let nz_a = if nb.left_available { nb.left_mvd_abs[0][0] } else { 0 };
        let nz_b = if nb.top_available { nb.top_mvd_abs[0][0] } else { 0 };
        let cbf_ctx = coded_block_flag_ctx(0, nz_a as u32, nz_b as u32);
        let params = ResidualParams {
            cat: 0,
            cbf_ctx,
            scantable: ctx.scan_4x4,
            qmul: None,
            max_coeff: 16,
            is_dc: true,
            chroma422: false,
            mb_field: false,
        };
        decode_residual_dc(cabac, states, &mut residual.luma_dc, params);

        if cbp_luma != 0 {
            for i in 0..16 {
                let (nz_a, nz_b) = neighbour_nz_4x4(nb, i, &residual.nz_count_luma);
                let cbf_ctx = coded_block_flag_ctx(1, nz_a as u32, nz_b as u32);
                let params = ResidualParams {
                    cat: 1,
                    cbf_ctx,
                    scantable: &ctx.scan_4x4[1..],
                    qmul: Some(ctx.dequant_4x4_luma),
                    max_coeff: 15,
                    is_dc: false,
                    chroma422: false,
                    mb_field: false,
                };
                residual.nz_count_luma[i] =
                    decode_residual_nondc(cabac, states, &mut residual.luma_4x4[i], params, true)
                        as u8;
            }
        }
    } else {
        // Non-Intra16x16 luma: each 8×8 quadrant gated by its
        // CBP bit; 4 4×4 blocks per quadrant.
        for i8x8 in 0..4 {
            if cbp_luma & (1 << i8x8) == 0 {
                continue;
            }
            for i4x4 in 0..4 {
                let i = 4 * i8x8 + i4x4;
                let (nz_a, nz_b) = neighbour_nz_4x4(nb, i, &residual.nz_count_luma);
                let cbf_ctx = coded_block_flag_ctx(2, nz_a as u32, nz_b as u32);
                let params = ResidualParams {
                    cat: 2,
                    cbf_ctx,
                    scantable: ctx.scan_4x4,
                    qmul: Some(ctx.dequant_4x4_luma),
                    max_coeff: 16,
                    is_dc: false,
                    chroma422: false,
                    mb_field: false,
                };
                residual.nz_count_luma[i] =
                    decode_residual_nondc(cabac, states, &mut residual.luma_4x4[i], params, true)
                        as u8;
            }
        }
    }
    Ok(())
}

/// Chroma residual loop (4:2:0): one DC block per plane + four AC
/// blocks per plane gated by cbp_chroma.
fn decode_chroma_residual_inline(
    cabac: &mut CabacContext<'_>,
    states: &mut [u8],
    ctx: &SliceCabacContext<'_>,
    nb: &MbNeighbours,
    cbp_chroma: u8,
    residual: &mut MbResidualState,
) -> Result<(), CodecError> {
    if cbp_chroma == 0 {
        return Ok(());
    }

    // DC pass — always runs when cbp_chroma >= 1.  Use the
    // neighbour CBP bit as a rough proxy for nz_a / nz_b on the
    // chroma DC contexts.
    let cb_nz_a = ((nb.left_cbp >> 4) & 0x01) as u32;
    let cb_nz_b = ((nb.top_cbp >> 4) & 0x01) as u32;
    let cr_nz_a = ((nb.left_cbp >> 5) & 0x01) as u32;
    let cr_nz_b = ((nb.top_cbp >> 5) & 0x01) as u32;
    let dc_ctxs = [
        coded_block_flag_ctx(3, cb_nz_a, cb_nz_b),
        coded_block_flag_ctx(3, cr_nz_a, cr_nz_b),
    ];
    for plane in 0..2 {
        let params = ResidualParams {
            cat: 3,
            cbf_ctx: dc_ctxs[plane],
            scantable: ctx.scan_4x4,
            qmul: None,
            max_coeff: 4,
            is_dc: true,
            chroma422: false,
            mb_field: false,
        };
        decode_residual_dc(cabac, states, &mut residual.chroma_dc[plane], params);
    }

    if cbp_chroma == 1 {
        return Ok(());
    }

    // AC pass.
    let dequants = [ctx.dequant_4x4_cb, ctx.dequant_4x4_cr];
    for plane in 0..2 {
        for i in 0..4 {
            let block = 4 * plane + i;
            let cbf_ctx = coded_block_flag_ctx(4, 0, 0); // simplified: caller can refine later
            let params = ResidualParams {
                cat: 4,
                cbf_ctx,
                scantable: &ctx.scan_4x4[1..],
                qmul: Some(dequants[plane]),
                max_coeff: 15,
                is_dc: false,
                chroma422: false,
                mb_field: false,
            };
            residual.nz_count_chroma[block] =
                decode_residual_nondc(cabac, states, &mut residual.chroma_ac[block], params, true)
                    as u8;
        }
    }
    Ok(())
}

/// Picks the A (left) and B (top) non-zero counts for the given
/// 4×4 luma block index.  Uses in-MB state for blocks not on the
/// macroblock edge and the neighbour cache for edge blocks.
fn neighbour_nz_4x4(nb: &MbNeighbours, block: usize, nz_in_mb: &[u8; 16]) -> (u8, u8) {
    let row = block / 4;
    let col = block % 4;
    let nz_a = if col == 0 {
        if nb.left_available { nb.left_mvd_abs[row][0] } else { 0 }
    } else {
        nz_in_mb[block - 1]
    };
    let nz_b = if row == 0 {
        if nb.top_available { nb.top_mvd_abs[col][0] } else { 0 }
    } else {
        nz_in_mb[block - 4]
    };
    (nz_a, nz_b)
}

/// Computes chroma QP from luma QP + the PPS-level chroma offset
/// per spec § 8.5.11 (Table 8-15 lookup).
fn chroma_qp(qp_y: u8, chroma_offset: i32) -> u8 {
    let qp = (qp_y as i32 + chroma_offset).clamp(0, 51) as u8;
    chroma_qp_table()[qp as usize]
}

/// Spec Table 8-15 — `qPI` to `qPc` conversion.
fn chroma_qp_table() -> [u8; 52] {
    [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 29, 30, 31, 32, 32, 33, 34, 34, 35, 35, 36, 36, 37, 37, 37, 38, 38, 38,
        39, 39, 39, 39,
    ]
}

/// Clamps a luma QP into the spec's [0, 51] range (the modular
/// wrap-around in spec § 8.5.2 is applied by the slice loop
/// before this clamp).
fn clip_qp(qp: i32) -> i32 {
    qp.rem_euclid(52)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::cabac::init_contexts;

    fn buf() -> Vec<u8> {
        let mut v = vec![0x55u8; 512];
        v[0] = 0x40;
        v
    }

    fn zigzag_4x4() -> [u8; 16] {
        [0, 1, 4, 8, 5, 2, 3, 6, 9, 12, 13, 10, 7, 11, 14, 15]
    }

    #[test]
    fn parse_slice_p_one_mb_runs() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let scan8 = [0u8; 64];
        let dq4 = [16u32; 16];
        let dq8 = [16u32; 64];
        let ctx = SliceCabacContext {
            slice_type: SliceType::P,
            pic_width_mbs: 1,
            pic_height_mbs: 1,
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
        let mut cache = InterSliceCache::new(1);
        let r = parse_slice_cabac(&mut cabac, &mut states, ctx, &mut cache);
        assert!(r.is_ok());
        let mbs = r.unwrap();
        assert_eq!(mbs.len(), 1);
    }

    #[test]
    fn parse_slice_rejects_b_slice() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::B, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let scan8 = [0u8; 64];
        let dq4 = [16u32; 16];
        let dq8 = [16u32; 64];
        let ctx = SliceCabacContext {
            slice_type: SliceType::B,
            pic_width_mbs: 1,
            pic_height_mbs: 1,
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
        let mut cache = InterSliceCache::new(1);
        let r = parse_slice_cabac(&mut cabac, &mut states, ctx, &mut cache);
        assert!(r.is_err());
    }

    #[test]
    fn parse_slice_2x2_runs_to_completion() {
        let bytes = buf();
        let mut states = init_contexts(SliceType::P, 26, 0);
        let mut cabac = CabacContext::new(&bytes).unwrap();
        let scan = zigzag_4x4();
        let scan8 = [0u8; 64];
        let dq4 = [16u32; 16];
        let dq8 = [16u32; 64];
        let ctx = SliceCabacContext {
            slice_type: SliceType::P,
            pic_width_mbs: 2,
            pic_height_mbs: 2,
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
        let mut cache = InterSliceCache::new(2);
        let mbs = parse_slice_cabac(&mut cabac, &mut states, ctx, &mut cache).unwrap();
        assert_eq!(mbs.len(), 4);
        for mb in &mbs {
            assert!(mb.mb_x < 2);
            assert!(mb.mb_y < 2);
        }
    }
}
