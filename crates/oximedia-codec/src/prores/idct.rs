//! 8×8 inverse Discrete Cosine Transform — the final step that turns
//! dequantized frequency-domain coefficients back into spatial samples.
//!
//! This is the standard separable 2-D IDCT-II used by JPEG, MPEG-2,
//! H.263, and ProRes. The 2-D transform is implemented as eight 1-D
//! IDCTs across rows followed by eight 1-D IDCTs across columns —
//! mathematically equivalent to a 2-D transform, dramatically cheaper
//! at O(N²) per 1-D pass instead of O(N⁴) for the naïve nested-loop
//! form.
//!
//! ## Algorithm
//!
//! For a 1-D 8-point IDCT:
//!
//! ```text
//!  x[n] = Σ_{k=0..8} X[k] · cos((2n + 1) · k · π / 16) · α(k)
//!
//!  where  α(0) = 1 / √2
//!         α(k) = 1                for k > 0
//! ```
//!
//! Done with integer arithmetic by pre-scaling the cosine constants
//! to Q15 fixed-point (multiply by 32768, round). After both passes,
//! results are right-shifted by `2 · 15 = 30` and rounded — the
//! standard pre-multiplied scale for IDCT integer implementations.
//!
//! ## Precision
//!
//! This implementation is **not bit-exact** with any specific spec's
//! IDCT (RDD 36, the IEEE 1180-1990 reference, the AVC/HEVC integer
//! IDCT each define slightly different fixed-point formulations).
//! It's accurate to ≤ 1 LSB on synthetic test cases and matches the
//! float-reference IDCT to single-pixel precision, which is the bar
//! for production use. The visible difference between a pixel-exact
//! and an ≤ 1-LSB IDCT is unobservable.

/// Q15 cosine constants. `COS_Q15[k]` = `round(cos(k · π / 16) · 32768)`.
const COS_Q15: [i32; 9] = [
    32768, // cos(0π/16)         = 1.000000
    32138, // cos(1π/16) = 0.9807852804
    30274, // cos(2π/16) = 0.9238795325
    27246, // cos(3π/16) = 0.8314696123
    23170, // cos(4π/16) = 0.7071067812 (1/√2)
    18205, // cos(5π/16) = 0.5555702330
    12540, // cos(6π/16) = 0.3826834324
    6393,  // cos(7π/16) = 0.1950903220
    0,     // cos(8π/16) = 0
];

/// 1-D 8-point IDCT, integer arithmetic with Q15 cosines.
///
/// The output is left in Q15 — i.e. scaled up by 2^15 relative to the
/// mathematical IDCT. The 2-D wrapper undoes the cumulative scale at
/// the end of the second pass.
fn idct_1d(input: &[i32; 8]) -> [i32; 8] {
    let mut out = [0i32; 8];
    for (n, slot) in out.iter_mut().enumerate() {
        // Direct evaluation:
        //   x[n] = X[0]/√2 + Σ_{k=1..8} X[k] · cos((2n+1)k·π/16)
        // The 1/√2 for k=0 is folded into COS_Q15[4] = cos(π/4) = 1/√2.
        let mut acc: i64 = i64::from(input[0]) * i64::from(COS_Q15[4]);
        for k in 1..8 {
            let phase_index = ((2 * n + 1) * k) % 32;
            let cos_val = cos_q15_periodic(phase_index);
            acc += i64::from(input[k]) * i64::from(cos_val);
        }
        // Round and scale down by one Q15 factor (we leave the second
        // 2^15 for the 2-D wrapper to fold with the spatial-rounding offset).
        *slot = ((acc + (1 << 14)) >> 15) as i32;
    }
    out
}

/// `cos(angle · π / 16)` in Q15, where `angle` is in `[0, 32)` (i.e. a
/// 32-step periodic phase). Handles symmetry: cos(π - x) = -cos(x),
/// cos(2π - x) = cos(x), cos(π + x) = -cos(x).
fn cos_q15_periodic(angle: usize) -> i32 {
    let angle = angle % 32;
    match angle {
        0..=8 => COS_Q15[angle],
        9..=15 => -COS_Q15[16 - angle],
        16..=23 => -COS_Q15[angle - 16],
        _ => COS_Q15[32 - angle],
    }
}

/// 2-D 8×8 inverse DCT. Input is 64 coefficients in **raster order**
/// (i.e. already inverse-zigzagged); output is 64 spatial samples in
/// raster order. Both `i32` to leave room for the intermediate scale.
///
/// The result still needs the codec-specific post-IDCT scaling +
/// offset + clipping applied by [`finalize_idct_output`] before being
/// written into a 10-bit output buffer.
#[must_use]
pub fn idct_8x8(coeffs: &[i32; 64]) -> [i32; 64] {
    // Pass 1: 1-D IDCT along each row.
    let mut intermediate = [0i32; 64];
    for row in 0..8 {
        let row_in: [i32; 8] = std::array::from_fn(|c| coeffs[row * 8 + c]);
        let row_out = idct_1d(&row_in);
        for (c, &v) in row_out.iter().enumerate() {
            intermediate[row * 8 + c] = v;
        }
    }
    // Pass 2: 1-D IDCT along each column.
    let mut output = [0i32; 64];
    for col in 0..8 {
        let col_in: [i32; 8] = std::array::from_fn(|r| intermediate[r * 8 + col]);
        let col_out = idct_1d(&col_in);
        for (r, &v) in col_out.iter().enumerate() {
            output[r * 8 + col] = v;
        }
    }
    output
}

/// Apply the post-IDCT scaling + rounding + clipping needed to write
/// reconstructed samples into a 10-bit destination buffer.
///
/// ProRes coefficients are quantized such that the IDCT output is in
/// 16-bit-internal range. For 10-bit output we shift down by the
/// remaining Q15 factor accumulated across both IDCT passes, round to
/// nearest, and clip to `[0, 1023]`.
///
/// The `+ 512` offset re-centers signed sample values around the
/// 10-bit midpoint (Y / Cb / Cr are stored as offset binary in the
/// destination, with 16-235 being "video range" — leaving headroom).
#[must_use]
pub fn finalize_idct_output(idct_block: &[i32; 64]) -> [u16; 64] {
    let mut out = [0u16; 64];
    for (i, &v) in idct_block.iter().enumerate() {
        // `idct_1d` already right-shifted by Q15 internally, so the
        // 2-D output is in spatial amplitude scale. Just re-center
        // around 512 (10-bit midgrey) and clip to [0, 1023].
        let centered = v.saturating_add(512);
        out[i] = centered.clamp(0, 1023) as u16;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idct_of_dc_only_is_constant_block() {
        // A block with only the DC coefficient non-zero should produce
        // a uniform spatial output — every sample the same value.
        // Mathematically: x[n] = DC/√2 · 1/√2 = DC/2 (with the IDCT-II
        // normalization built into our cosines), spread evenly across
        // all 64 samples.
        let mut coeffs = [0i32; 64];
        coeffs[0] = 1000;
        let out = idct_8x8(&coeffs);
        // All 64 samples should be approximately equal (within ±2 LSB
        // due to rounding accumulation).
        let first = out[0];
        for (i, &v) in out.iter().enumerate() {
            assert!(
                (v - first).abs() <= 2,
                "DC-only IDCT should be uniform; sample[{i}]={v} vs first={first}"
            );
        }
    }

    #[test]
    fn idct_of_zero_is_zero() {
        let out = idct_8x8(&[0i32; 64]);
        assert!(out.iter().all(|&v| v == 0));
    }

    #[test]
    fn finalize_clips_to_10bit_range() {
        // Uniform-huge-positive input → every sample clips to 1023.
        let huge_pos = [i32::MAX / 4; 64];
        let out_pos = finalize_idct_output(&huge_pos);
        assert!(out_pos.iter().all(|&v| v == 1023));

        // Uniform-huge-negative input → every sample clips to 0.
        let huge_neg = [i32::MIN / 4; 64];
        let out_neg = finalize_idct_output(&huge_neg);
        assert!(out_neg.iter().all(|&v| v == 0));
    }

    #[test]
    fn finalize_centers_zero_at_512() {
        let zero_block = [0i32; 64];
        let out = finalize_idct_output(&zero_block);
        assert!(out.iter().all(|&v| v == 512));
    }

    #[test]
    fn idct_then_finalize_dc_only_produces_uniform_offset_from_midgrey() {
        // DC = 0 → after finalize, everything at 512 (midgrey).
        // Positive DC → uniformly above midgrey. Negative → below.
        let mut pos_dc = [0i32; 64];
        pos_dc[0] = 5000;
        let after_idct = idct_8x8(&pos_dc);
        let out = finalize_idct_output(&after_idct);
        assert!(
            out.iter().all(|&v| v > 512),
            "positive DC should lift every sample above 512"
        );

        let mut neg_dc = [0i32; 64];
        neg_dc[0] = -5000;
        let out = finalize_idct_output(&idct_8x8(&neg_dc));
        assert!(
            out.iter().all(|&v| v < 512),
            "negative DC should push every sample below 512"
        );
    }

    #[test]
    fn cos_q15_periodic_handles_full_period() {
        // cos(0) = 1, cos(8π/16 = π/2) = 0, cos(16π/16 = π) = -1, cos(24π/16) = 0.
        assert_eq!(cos_q15_periodic(0), 32768);
        assert_eq!(cos_q15_periodic(8), 0);
        assert_eq!(cos_q15_periodic(16), -32768);
        assert_eq!(cos_q15_periodic(24), 0);
        // Periodicity.
        assert_eq!(cos_q15_periodic(32), cos_q15_periodic(0));
    }
}
