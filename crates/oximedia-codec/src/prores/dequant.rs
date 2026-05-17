//! Dequantization — undo the encoder's frequency-domain scaling.
//!
//! The encoder divided each transform coefficient by `matrix[i] *
//! qscale` (with appropriate rounding/dead-zone) to produce the
//! quantized integer that ended up in the bitstream. The decoder
//! reverses:
//!
//! ```text
//!   coeff[i] = quantized[i] * matrix[i] * qscale
//! ```
//!
//! `matrix` is the 8×8 quantization matrix from the frame header (one
//! for luma, one for chroma). `qscale` is the per-slice quantization
//! parameter from the slice header (1..=224 per RDD 36 §6.5.3).
//!
//! The result is left in the IDCT-input scale; subsequent IDCT then
//! finalize-output stages bring it down to 10-bit samples.

/// Dequantize one 8×8 coefficient block.
///
/// `quantized` is the block in **raster order** (i.e. already inverse-
/// zigzagged from scan order). `matrix` is the 8×8 quantization
/// matrix in matching raster order. `qscale` is 1..=224.
#[must_use]
pub fn dequantize_block(
    quantized: &[i32; 64],
    matrix: &[u8; 64],
    qscale: u8,
) -> [i32; 64] {
    let qscale = i32::from(qscale);
    let mut out = [0i32; 64];
    for i in 0..64 {
        out[i] = quantized[i] * i32::from(matrix[i]) * qscale;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dequantize_zero_block_stays_zero() {
        let q = [0i32; 64];
        let m = [4u8; 64];
        let out = dequantize_block(&q, &m, 10);
        assert!(out.iter().all(|&v| v == 0));
    }

    #[test]
    fn dequantize_inverts_scalar_quantization() {
        // If encoder did q = coeff / (matrix[i] * qscale), decoder
        // should reconstruct ~coeff. Test with values that divide cleanly.
        let m = [4u8; 64];
        let qscale = 5u8;
        let coeff_target = 200; // = 10 * 4 * 5
        let q = [10i32; 64];
        let out = dequantize_block(&q, &m, qscale);
        assert!(out.iter().all(|&v| v == coeff_target));
    }

    #[test]
    fn dequantize_preserves_sign() {
        let m = [4u8; 64];
        let mut q = [0i32; 64];
        q[0] = -7;
        let out = dequantize_block(&q, &m, 3);
        assert_eq!(out[0], -7 * 4 * 3);
    }

    #[test]
    fn dequantize_uses_per_position_matrix() {
        // Matrix that's small at DC, large at high frequency — typical.
        let mut m = [0u8; 64];
        m[0] = 4;
        m[63] = 64;
        let mut q = [0i32; 64];
        q[0] = 1;
        q[63] = 1;
        let out = dequantize_block(&q, &m, 1);
        assert_eq!(out[0], 4);
        assert_eq!(out[63], 64);
    }
}
