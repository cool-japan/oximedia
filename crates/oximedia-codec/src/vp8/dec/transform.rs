//! VP8 inverse transforms (RFC 6386 §14.3, §14.4).
//!
//! VP8 uses two integer transforms:
//! - a 4x4 inverse DCT applied to every residual sub-block, and
//! - a 4x4 inverse Walsh-Hadamard transform (WHT) applied to the "Y2" block
//!   that holds the 16 luma DC coefficients of a whole-16x16-predicted
//!   macroblock.
//!
//! Both are exact integer transforms; the constants below are the fixed-point
//! cosine multipliers given in RFC 6386 §14.3 ("idct4x4llm").

//!
//! Ported from the production-verified `oximedia-image` `webp/vp8` decoder
//! (same workspace; a WebP lossy frame is a VP8 key frame).

/// Fixed-point multiplier `sqrt(2) * cos(pi / 8) ≈ 1.30656`, scaled by 2^16
/// (RFC 6386 §14.3).
const COS_PI8_SQRT2: i64 = 20091;
/// Fixed-point multiplier `sqrt(2) * sin(pi / 8) ≈ 0.54120`, scaled by 2^16.
const SIN_PI8_SQRT2: i64 = 35468;

/// Applies the inverse 4x4 DCT to `block` in place.
///
/// `block` holds 16 dequantised coefficients in raster order. On return it
/// holds the 16 spatial-domain residual values (rounded, but not clamped).
///
/// This is a direct transcription of the `idct4x4llm` reference: a 1-D
/// transform along columns followed by a 1-D transform along rows, with the
/// final `+4 >> 3` rounding.
pub fn idct4x4(block: &mut [i32; 16]) {
    let mut tmp = [0i32; 16];

    // Vertical pass (operate on each of the 4 columns).
    for i in 0..4 {
        let c0 = block[i];
        let c1 = block[i + 4];
        let c2 = block[i + 8];
        let c3 = block[i + 12];

        let a1 = c0 + c2;
        let b1 = c0 - c2;

        let t1 = (i64::from(c1) * SIN_PI8_SQRT2) >> 16;
        let t2 = i64::from(c3) + ((i64::from(c3) * COS_PI8_SQRT2) >> 16);
        let c1_t = (t1 - t2) as i32;

        let t1 = i64::from(c1) + ((i64::from(c1) * COS_PI8_SQRT2) >> 16);
        let t2 = (i64::from(c3) * SIN_PI8_SQRT2) >> 16;
        let d1 = (t1 + t2) as i32;

        tmp[i] = a1 + d1;
        tmp[i + 12] = a1 - d1;
        tmp[i + 4] = b1 + c1_t;
        tmp[i + 8] = b1 - c1_t;
    }

    // Horizontal pass (operate on each of the 4 rows), with rounding.
    for i in 0..4 {
        let base = i * 4;
        let c0 = tmp[base];
        let c1 = tmp[base + 1];
        let c2 = tmp[base + 2];
        let c3 = tmp[base + 3];

        let a1 = c0 + c2;
        let b1 = c0 - c2;

        let t1 = (i64::from(c1) * SIN_PI8_SQRT2) >> 16;
        let t2 = i64::from(c3) + ((i64::from(c3) * COS_PI8_SQRT2) >> 16);
        let c1_t = (t1 - t2) as i32;

        let t1 = i64::from(c1) + ((i64::from(c1) * COS_PI8_SQRT2) >> 16);
        let t2 = (i64::from(c3) * SIN_PI8_SQRT2) >> 16;
        let d1 = (t1 + t2) as i32;

        block[base] = (a1 + d1 + 4) >> 3;
        block[base + 3] = (a1 - d1 + 4) >> 3;
        block[base + 1] = (b1 + c1_t + 4) >> 3;
        block[base + 2] = (b1 - c1_t + 4) >> 3;
    }
}

/// Applies the inverse 4x4 Walsh-Hadamard transform to `block` in place.
///
/// Used to reconstruct the 16 luma DC values of a whole-16x16 macroblock from
/// the transmitted Y2 block (RFC 6386 §14.3, `iwalsh4x4`). On return `block`
/// holds the 16 DC values, each of which becomes coefficient 0 of one of the
/// macroblock's 16 luma sub-blocks.
pub fn iwht4x4(block: &mut [i32; 16]) {
    let mut tmp = [0i32; 16];

    // Vertical pass.
    for i in 0..4 {
        let a1 = block[i] + block[i + 12];
        let b1 = block[i + 4] + block[i + 8];
        let c1 = block[i + 4] - block[i + 8];
        let d1 = block[i] - block[i + 12];

        tmp[i] = a1 + b1;
        tmp[i + 4] = d1 + c1;
        tmp[i + 8] = a1 - b1;
        tmp[i + 12] = d1 - c1;
    }

    // Horizontal pass, with rounding (`+3 >> 3`).
    for i in 0..4 {
        let base = i * 4;
        let a1 = tmp[base] + tmp[base + 3];
        let b1 = tmp[base + 1] + tmp[base + 2];
        let c1 = tmp[base + 1] - tmp[base + 2];
        let d1 = tmp[base] - tmp[base + 3];

        let a2 = a1 + b1;
        let b2 = d1 + c1;
        let c2 = a1 - b1;
        let d2 = d1 - c1;

        block[base] = (a2 + 3) >> 3;
        block[base + 1] = (b2 + 3) >> 3;
        block[base + 2] = (c2 + 3) >> 3;
        block[base + 3] = (d2 + 3) >> 3;
    }
}

/// Adds a residual sub-block to a prediction, clamping to `[0, 255]`.
///
/// `residual` is the 16-element output of [`idct4x4`]. `dst` is a 4x4 window
/// into the reconstruction plane with stride `stride`, anchored at `dst_off`.
pub fn add_residual(dst: &mut [u8], dst_off: usize, stride: usize, residual: &[i32; 16]) {
    for r in 0..4 {
        let row = dst_off + r * stride;
        for c in 0..4 {
            let idx = row + c;
            if idx < dst.len() {
                let v = i32::from(dst[idx]) + residual[r * 4 + c];
                dst[idx] = v.clamp(0, 255) as u8;
            }
        }
    }
}

/// Specialised DC-only inverse DCT.
///
/// When only coefficient 0 is non-zero the full transform reduces to a single
/// constant added to every pixel: `dc = (coeff[0] + 4) >> 3`.
#[must_use]
pub fn idct4x4_dc(dc_coeff: i32) -> i32 {
    (dc_coeff + 4) >> 3
}

/// Adds a constant DC residual to a 4x4 window, clamping to `[0, 255]`.
pub fn add_residual_dc(dst: &mut [u8], dst_off: usize, stride: usize, dc: i32) {
    for r in 0..4 {
        let row = dst_off + r * stride;
        for c in 0..4 {
            let idx = row + c;
            if idx < dst.len() {
                let v = i32::from(dst[idx]) + dc;
                dst[idx] = v.clamp(0, 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idct_all_zero_is_zero() {
        let mut block = [0i32; 16];
        idct4x4(&mut block);
        assert_eq!(block, [0i32; 16]);
    }

    #[test]
    fn test_idct_dc_only_constant() {
        // A pure DC coefficient produces a constant block.
        let mut block = [0i32; 16];
        block[0] = 64;
        idct4x4(&mut block);
        let first = block[0];
        for &v in &block {
            assert_eq!(v, first, "DC-only IDCT must be constant");
        }
        // (64 + 4) >> 3 contribution propagated: idct4x4_dc agreement.
        assert_eq!(first, idct4x4_dc(64));
    }

    #[test]
    fn test_iwht_all_zero_is_zero() {
        let mut block = [0i32; 16];
        iwht4x4(&mut block);
        assert_eq!(block, [0i32; 16]);
    }

    #[test]
    fn test_iwht_dc_only_constant() {
        let mut block = [0i32; 16];
        block[0] = 80;
        iwht4x4(&mut block);
        let first = block[0];
        for &v in &block {
            assert_eq!(v, first, "DC-only WHT must be constant");
        }
    }

    #[test]
    fn test_add_residual_clamps() {
        let mut dst = vec![250u8; 16];
        let residual = [50i32; 16];
        add_residual(&mut dst, 0, 4, &residual);
        for &v in &dst {
            assert_eq!(v, 255, "residual addition must clamp to 255");
        }
    }

    #[test]
    fn test_add_residual_dc_negative_clamps() {
        let mut dst = vec![10u8; 16];
        add_residual_dc(&mut dst, 0, 4, -50);
        for &v in &dst {
            assert_eq!(v, 0, "negative residual must clamp to 0");
        }
    }
}
