//! ProRes default quantization matrices (RDD 36 §6.5.4 Table 10).
//!
//! These two 8×8 matrices are used by every ProRes frame that doesn't
//! signal custom matrices in its frame header — which is the vast
//! majority of real-world streams. Custom matrices, when present, are
//! delivered inline at the end of the frame header.
//!
//! Layout convention: row-major, i.e. element at `[row * 8 + col]`
//! corresponds to spatial frequency `(row, col)` in the 8×8 DCT block.
//! Entry at `[0]` is the DC quantizer; bottom-right entries quantize the
//! highest-frequency coefficients.

/// Default ProRes luma quantization matrix (per SMPTE RDD 36 §6.5.4).
///
/// The matrix is "weighted but flat" — luma is rarely quantized
/// aggressively, so the values are uniform low.
pub const DEFAULT_LUMA_QUANT_MATRIX: [u8; 64] = [
    4, 4, 5, 5, 6, 7, 8, 9,
    4, 4, 5, 6, 7, 8, 9, 10,
    5, 5, 6, 7, 8, 9, 10, 12,
    5, 6, 7, 8, 9, 10, 12, 14,
    6, 7, 8, 9, 10, 12, 14, 17,
    7, 8, 9, 10, 12, 14, 17, 21,
    8, 9, 10, 12, 14, 17, 21, 26,
    9, 10, 12, 14, 17, 21, 26, 33,
];

/// Default ProRes chroma quantization matrix (per SMPTE RDD 36 §6.5.4).
///
/// Chroma is quantized more aggressively at high frequencies than luma
/// — your eye is less sensitive to high-frequency colour detail, so
/// the encoder spends fewer bits there.
pub const DEFAULT_CHROMA_QUANT_MATRIX: [u8; 64] = [
    4, 4, 5, 5, 6, 7, 9, 11,
    4, 4, 5, 6, 7, 9, 11, 14,
    5, 5, 6, 7, 9, 11, 14, 18,
    5, 6, 7, 9, 11, 14, 18, 23,
    6, 7, 9, 11, 14, 18, 23, 29,
    7, 9, 11, 14, 18, 23, 29, 36,
    9, 11, 14, 18, 23, 29, 36, 45,
    11, 14, 18, 23, 29, 36, 45, 56,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrices_are_64_entries() {
        assert_eq!(DEFAULT_LUMA_QUANT_MATRIX.len(), 64);
        assert_eq!(DEFAULT_CHROMA_QUANT_MATRIX.len(), 64);
    }

    #[test]
    fn dc_quantizers_match_rdd_36() {
        // DC quantizer for both luma and chroma is 4 in the default tables.
        assert_eq!(DEFAULT_LUMA_QUANT_MATRIX[0], 4);
        assert_eq!(DEFAULT_CHROMA_QUANT_MATRIX[0], 4);
    }

    #[test]
    fn chroma_quantizes_high_frequency_more_than_luma() {
        // Bottom-right entries (highest frequency) — chroma > luma by design.
        assert!(
            DEFAULT_CHROMA_QUANT_MATRIX[63] > DEFAULT_LUMA_QUANT_MATRIX[63],
            "chroma HF should be quantized more aggressively than luma"
        );
    }

    #[test]
    fn matrices_monotonically_increase_along_diagonal() {
        // Spatial-frequency-weighted matrices: each diagonal step away from
        // (0,0) should generally increase quantization strength.
        for k in 0..7 {
            let lo = DEFAULT_LUMA_QUANT_MATRIX[k * 9];
            let hi = DEFAULT_LUMA_QUANT_MATRIX[(k + 1) * 9];
            assert!(hi >= lo, "luma diag should not decrease at step {k}");
        }
    }
}
