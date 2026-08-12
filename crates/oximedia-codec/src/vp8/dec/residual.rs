//! VP8 DCT-token residual decoding (RFC 6386 §13).
//!
//! Decodes the per-macroblock coefficient tokens: the Y2 (WHT) block when
//! present, the 16 luma sub-blocks, and the 4+4 chroma sub-blocks, each via
//! the shared [`decode_block`] token-tree walk. Coefficients come out
//! dequantised, in raster order, ready for the inverse transform in
//! [`super::recon`].

use super::bool_decoder::BoolDecoder;
use super::tables::{
    CAT_BASE, COEFF_BANDS, COEFF_TREE, DCT_0, DCT_1, DCT_2, DCT_3, DCT_4, DCT_CAT1, DCT_CAT2,
    DCT_CAT3, DCT_CAT4, DCT_CAT5, DCT_CAT6, DCT_EOB, PCAT1, PCAT2, PCAT3, PCAT4, PCAT5, PCAT6,
    ZIGZAG,
};
use super::{Decoder, DequantFactors};

impl Decoder<'_> {
    /// Decodes the DCT-token residuals for one macroblock.
    ///
    /// `has_y2` says whether this macroblock codes a second-order (Y2/WHT)
    /// block: every mode except intra `B_PRED` and inter `SPLITMV` (RFC 6386
    /// §13.1; dixie.c line 12833). The caller supplies it because the two
    /// frame types derive it from different mode enumerations.
    ///
    /// Returns `any_tokens`, the libvpx `eobtotal != 0` condition.
    /// Coefficients are written dequantised into `coeffs` in raster order
    /// per sub-block.
    pub(super) fn decode_residuals(
        &mut self,
        mb_x: usize,
        has_y2: bool,
        dq: &DequantFactors,
        coeffs: &mut [[i32; 16]; 25],
        part: usize,
        left_nz: &mut [bool; 9],
    ) -> bool {
        let mut any_tokens = false;
        let coeff_probs = &self.header.coeff_probs;

        // --- Y2 block (block type 1) ---
        if has_y2 {
            let ctx = usize::from(left_nz[8]) + usize::from(self.above_nz[mb_x * 9 + 8]);
            let nz = decode_block(
                &mut self.token_bd[part],
                coeff_probs,
                1,
                ctx,
                0,
                dq.y2_dc,
                dq.y2_ac,
                &mut coeffs[24],
            );
            left_nz[8] = nz;
            self.above_nz[mb_x * 9 + 8] = nz;
            any_tokens |= nz;
        }

        // Luma block type: 0 when Y2 carries DC, else 3 (coeff 0 included).
        let y_block_type = if has_y2 { 0 } else { 3 };
        let first_coeff = if has_y2 { 1 } else { 0 };

        // --- 16 luma sub-blocks ---
        for r in 0..4 {
            for col in 0..4 {
                let sb = r * 4 + col;
                let ctx = usize::from(self.above_nz[mb_x * 9 + col]) + usize::from(left_nz[r]);
                let nz = decode_block(
                    &mut self.token_bd[part],
                    coeff_probs,
                    y_block_type,
                    ctx,
                    first_coeff,
                    dq.y_dc,
                    dq.y_ac,
                    &mut coeffs[sb],
                );
                self.above_nz[mb_x * 9 + col] = nz;
                left_nz[r] = nz;
                any_tokens |= nz;
            }
        }

        // --- 4 U + 4 V chroma sub-blocks (block type 2) ---
        for (plane, base) in [(0usize, 16usize), (1usize, 20usize)] {
            // U uses above/left context slots 4,5; V uses 6,7.
            let ctx_base = 4 + plane * 2;
            for r in 0..2 {
                for col in 0..2 {
                    let sb = base + r * 2 + col;
                    let a_idx = mb_x * 9 + ctx_base + col;
                    let l_idx = ctx_base + r;
                    let ctx = usize::from(self.above_nz[a_idx]) + usize::from(left_nz[l_idx]);
                    let nz = decode_block(
                        &mut self.token_bd[part],
                        coeff_probs,
                        2,
                        ctx,
                        0,
                        dq.uv_dc,
                        dq.uv_ac,
                        &mut coeffs[sb],
                    );
                    self.above_nz[a_idx] = nz;
                    left_nz[l_idx] = nz;
                    any_tokens |= nz;
                }
            }
        }

        any_tokens
    }
}

/// Decodes one sub-block of DCT coefficient tokens (RFC 6386 §13).
///
/// `block_type` selects the probability plane; `ctx` is the initial token
/// context (0..2); `first_coeff` is the starting zig-zag position (1 for luma
/// when a Y2 block carries DC). Coefficients are written dequantised into
/// `out` in raster order.
///
/// Returns `true` when the block carried any token (its end-of-block
/// position exceeds `first_coeff`). This is the value that feeds both the
/// above/left entropy context of neighbouring blocks and the loop filter's
/// `eobtotal == 0` skip decision in libvpx (`eob > first_coeff`, not
/// "any coefficient non-zero" — the two differ only for a block coded
/// entirely as literal zeros).
#[allow(clippy::too_many_arguments)]
fn decode_block(
    bd: &mut BoolDecoder<'_>,
    coeff_probs: &[[[[u8; 11]; 3]; 8]; 4],
    block_type: usize,
    ctx: usize,
    first_coeff: usize,
    dc_factor: i32,
    ac_factor: i32,
    out: &mut [i32; 16],
) -> bool {
    let mut prev_ctx = ctx;
    let mut i = first_coeff;
    // `skip_eob` mirrors the libvpx semantics: after a literal 0 token the EOB
    // branch is skipped for the immediately-following token (the token tree is
    // re-entered past the EOB branch at node 2 — RFC 6386 §13.3).
    let mut skip_eob = false;

    while i < 16 {
        let band = COEFF_BANDS[i];
        let probs = &coeff_probs[block_type][band][prev_ctx];

        // Enter the token tree either at the root (node 0) or — if the previous
        // token was a literal zero — past the EOB branch at node 2.
        let token = if skip_eob {
            bd.read_tree_from(&COEFF_TREE, probs, 2)
        } else {
            bd.read_tree(&COEFF_TREE, probs)
        };

        if token == DCT_EOB {
            break;
        }

        // Decode the magnitude of the token.
        let abs_value = match token {
            DCT_0 => 0,
            DCT_1 => 1,
            DCT_2 => 2,
            DCT_3 => 3,
            DCT_4 => 4,
            DCT_CAT1 => decode_category(bd, &PCAT1, CAT_BASE[0]),
            DCT_CAT2 => decode_category(bd, &PCAT2, CAT_BASE[1]),
            DCT_CAT3 => decode_category(bd, &PCAT3, CAT_BASE[2]),
            DCT_CAT4 => decode_category(bd, &PCAT4, CAT_BASE[3]),
            DCT_CAT5 => decode_category(bd, &PCAT5, CAT_BASE[4]),
            DCT_CAT6 => decode_category(bd, &PCAT6, CAT_BASE[5]),
            _ => 0,
        };

        if abs_value == 0 {
            // Literal zero: next token context is 0, EOB is skipped.
            prev_ctx = 0;
            skip_eob = true;
        } else {
            // Non-zero coefficient: read the sign and dequantise.
            let sign = bd.get_flag();
            let signed = if sign { -abs_value } else { abs_value };
            // Coefficient 0 uses the DC factor, all others the AC factor.
            let factor = if i == 0 { dc_factor } else { ac_factor };
            let zz = ZIGZAG[i];
            out[zz] = signed * factor;
            // Token context for the next coefficient: 1 for |v|==1, else 2.
            prev_ctx = if abs_value == 1 { 1 } else { 2 };
            skip_eob = false;
        }
        i += 1;
    }
    // End-of-block position past the first coefficient <=> tokens present.
    i > first_coeff
}

/// Decodes a category token's extra bits and adds the category base.
///
/// RFC 6386 §13.2: each category reads `probs.len()` extra bits MSB-first,
/// each against its own probability, then adds `base`.
fn decode_category(bd: &mut BoolDecoder<'_>, probs: &[u8], base: i32) -> i32 {
    let mut extra = 0i32;
    for &p in probs {
        extra = (extra << 1) | i32::from(bd.get_bool(p));
    }
    base + extra
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_category_base_only() {
        // With zero input every extra bit is 0, so the result is the base.
        let data = [0u8; 16];
        let mut bd = BoolDecoder::new(&data);
        assert_eq!(decode_category(&mut bd, &PCAT1, CAT_BASE[0]), CAT_BASE[0]);
    }
}
