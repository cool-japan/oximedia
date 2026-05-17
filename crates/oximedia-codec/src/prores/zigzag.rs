//! Zigzag scan tables for ProRes coefficient ordering (RDD 36 §6.5.7 Table 11).
//!
//! ProRes transmits the 64 coefficients of each 8×8 DCT block in a
//! **scan order** that prioritises low-frequency coefficients (where
//! most energy concentrates after the DCT) — concentrating non-zero
//! values at the start of the stream and runs of zeros at the end,
//! which the entropy coder can exploit.
//!
//! Two scan orders are defined:
//!
//! - **Progressive** (Z scan) — the classic JPEG zigzag, used for
//!   progressive frames and the inter-field difference of interlaced
//!   frames.
//! - **Alternate** (zigzag-alt / interlaced) — slightly different
//!   ordering used for interlaced frames where the encoder wants to
//!   exploit vertical correlation differently.
//!
//! The decoder receives quantized coefficients in scan order; this
//! module provides the lookup tables to write them back into raster
//! order (`row * 8 + col`) before the IDCT.

/// Progressive (JPEG) zigzag scan: `PROGRESSIVE_ZIGZAG[scan_index] = raster_position`.
///
/// Read coefficient `k` (0..64) from the bitstream and place it at
/// raster position `PROGRESSIVE_ZIGZAG[k]` in the 8×8 block.
pub const PROGRESSIVE_ZIGZAG: [u8; 64] = [
     0,  1,  8, 16,  9,  2,  3, 10,
    17, 24, 32, 25, 18, 11,  4,  5,
    12, 19, 26, 33, 40, 48, 41, 34,
    27, 20, 13,  6,  7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36,
    29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46,
    53, 60, 61, 54, 47, 55, 62, 63,
];

/// Alternate (interlaced) scan: prioritises vertical frequencies
/// differently than the progressive zigzag. Used for ProRes frames
/// with `interlace_mode != Progressive`.
///
/// Same indexing semantics as [`PROGRESSIVE_ZIGZAG`].
pub const ALTERNATE_ZIGZAG: [u8; 64] = [
     0,  8,  1,  9, 16, 24, 17,  2,
    10, 18, 25, 32, 40, 33, 26, 19,
    11,  3,  4, 12, 20, 27, 34, 41,
    48, 56, 49, 42, 35, 28, 21, 13,
     5,  6, 14, 22, 29, 36, 43, 50,
    57, 58, 51, 44, 37, 30, 23, 15,
     7, 31, 38, 45, 52, 59, 60, 53,
    46, 39, 47, 54, 61, 62, 55, 63,
];

/// Inverse-scan a 64-entry coefficient array.
///
/// `coeffs_in_scan_order[k]` is interpreted as the coefficient whose
/// raster position is `scan_table[k]`. The result is laid out in
/// raster order (`row * 8 + col`).
#[must_use]
pub fn inverse_scan(coeffs_in_scan_order: &[i32; 64], scan_table: &[u8; 64]) -> [i32; 64] {
    let mut out = [0i32; 64];
    for (scan_idx, &raster_idx) in scan_table.iter().enumerate() {
        out[raster_idx as usize] = coeffs_in_scan_order[scan_idx];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progressive_zigzag_is_a_permutation() {
        let mut seen = [false; 64];
        for &raster in &PROGRESSIVE_ZIGZAG {
            assert!(raster < 64);
            assert!(!seen[raster as usize], "duplicate raster index {raster}");
            seen[raster as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn alternate_zigzag_is_a_permutation() {
        let mut seen = [false; 64];
        for &raster in &ALTERNATE_ZIGZAG {
            assert!(raster < 64);
            assert!(!seen[raster as usize], "duplicate raster index {raster}");
            seen[raster as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }

    #[test]
    fn dc_lives_at_index_zero_in_both_scans() {
        // The DC coefficient (raster position 0) must always be the
        // first to be transmitted — bin 0 of the scan order.
        assert_eq!(PROGRESSIVE_ZIGZAG[0], 0);
        assert_eq!(ALTERNATE_ZIGZAG[0], 0);
    }

    #[test]
    fn highest_frequency_at_end_of_progressive_scan() {
        // The bottom-right of the 8×8 (raster 63) is the highest
        // spatial frequency — should be last in a progressive scan.
        assert_eq!(PROGRESSIVE_ZIGZAG[63], 63);
    }

    #[test]
    fn inverse_scan_dc_only_block() {
        // A block whose only non-zero coefficient is the DC (scan
        // index 0, raster index 0) should produce a result with the
        // DC value at position 0 and zeros elsewhere.
        let mut scan = [0i32; 64];
        scan[0] = 42;
        let raster = inverse_scan(&scan, &PROGRESSIVE_ZIGZAG);
        assert_eq!(raster[0], 42);
        assert!(raster[1..].iter().all(|&v| v == 0));
    }

    #[test]
    fn inverse_scan_round_trip_identity() {
        // Build a block where coeff[i] = i in raster order, scan it
        // forward, then inverse-scan it back and verify identity.
        let raster: [i32; 64] = std::array::from_fn(|i| i as i32);
        let mut scan = [0i32; 64];
        for (scan_idx, &raster_idx) in PROGRESSIVE_ZIGZAG.iter().enumerate() {
            scan[scan_idx] = raster[raster_idx as usize];
        }
        let back = inverse_scan(&scan, &PROGRESSIVE_ZIGZAG);
        assert_eq!(back, raster);
    }
}
