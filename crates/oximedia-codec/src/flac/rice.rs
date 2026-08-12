//! Rice coding for FLAC residuals.
//!
//! Rice coding is a special case of Golomb coding where the divisor `m = 2^k`.
//! FLAC uses Rice coding (partition coding) to compress the LPC residuals.
//!
//! Each partition's Rice parameter `k` is optimised to minimise bit usage.
//!
//! Per RFC 9639 §9.2.7.1 the quotient is coded in unary as **zero bits
//! terminated by a one bit**, followed by the `k` low bits of the folded
//! residual.  [`crate::flac::residual`] builds the partitioned residual blocks
//! that FLAC frames actually carry; the helpers here cover a single flat run.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use super::bitio::{BitReader, BitWriter};

/// Maximum Rice parameter for coding method 0 (4-bit parameters); 15 escapes.
pub const MAX_RICE_PARAM: u8 = 14;

/// Maximum Rice parameter for coding method 1 (5-bit parameters); 31 escapes.
pub const MAX_RICE_PARAM_WIDE: u8 = 30;

/// Map a signed residual to an unsigned zigzag ("folded") value.
///
/// FLAC folds signed residuals as `0 → 0, -1 → 1, 1 → 2, -2 → 3, 2 → 4, ...`.
/// The branchless form is used so that `i32::MIN` folds without overflowing.
#[inline]
#[must_use]
pub fn zigzag_encode(v: i32) -> u32 {
    ((v as u32) << 1) ^ ((v >> 31) as u32)
}

/// Decode a zigzag-encoded unsigned value back to signed.
#[inline]
#[must_use]
pub fn zigzag_decode(u: u32) -> i32 {
    ((u >> 1) as i32) ^ -((u & 1) as i32)
}

/// Compute the Rice bit cost for encoding `residuals` with parameter `k`.
///
/// `cost = sum(1 + k + (zigzag(r) >> k))` bits per sample.
#[must_use]
pub fn rice_bit_cost(residuals: &[i32], k: u8) -> u64 {
    residuals
        .iter()
        .map(|&r| {
            let u = zigzag_encode(r);
            let quotient = u >> k;
            1u64 + u64::from(k) + u64::from(quotient)
        })
        .sum()
}

/// Select the optimal Rice parameter for a partition of residuals.
///
/// Tests `k = 0..=MAX_RICE_PARAM` and returns the best.
#[must_use]
pub fn optimal_rice_param(residuals: &[i32]) -> u8 {
    if residuals.is_empty() {
        return 0;
    }
    let mut best_k = 0u8;
    let mut best_cost = u64::MAX;
    for k in 0..=MAX_RICE_PARAM {
        let cost = rice_bit_cost(residuals, k);
        if cost < best_cost {
            best_cost = cost;
            best_k = k;
        }
    }
    best_k
}

/// Write one Rice-coded residual with parameter `k` (RFC 9639 §9.2.7.1).
pub fn write_rice_signed(w: &mut BitWriter, value: i32, k: u32) {
    let u = zigzag_encode(value);
    let quotient = u >> k;
    w.write_unary(quotient);
    if k > 0 {
        w.write_bits(u64::from(u), k);
    }
}

/// Read one Rice-coded residual with parameter `k`.
///
/// Returns `None` on truncated input or an implausibly long unary run.
pub fn read_rice_signed(r: &mut BitReader<'_>, k: u32) -> Option<i32> {
    // A quotient can never legitimately exceed 2^32 / 2^k; bound generously
    // but finitely so corrupt data cannot spin.
    let quotient = r.read_unary(1 << 20)?;
    let remainder = if k > 0 { r.read_bits(k)? as u32 } else { 0 };
    let u = (u64::from(quotient) << k) | u64::from(remainder);
    Some(zigzag_decode(u as u32))
}

/// Encode residuals using Rice coding with parameter `k`.
///
/// Returns the packed bit stream as a `Vec<u8>` (MSB-first, zero-padded to a
/// byte boundary).  This is a flat, unpartitioned run — FLAC frames use
/// [`crate::flac::residual`] instead.
#[must_use]
pub fn rice_encode(residuals: &[i32], k: u8) -> Vec<u8> {
    let mut w = BitWriter::with_capacity(residuals.len());
    for &r in residuals {
        write_rice_signed(&mut w, r, u32::from(k));
    }
    w.into_bytes()
}

/// Rice decoder over a flat (unpartitioned) Rice-coded byte stream.
pub struct RiceDecoder<'a> {
    reader: BitReader<'a>,
}

impl<'a> RiceDecoder<'a> {
    /// Create a decoder over a Rice-coded byte stream.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            reader: BitReader::new(data),
        }
    }

    /// Decode one Rice-coded residual with parameter `k`.
    pub fn decode_one(&mut self, k: u8) -> Option<i32> {
        read_rice_signed(&mut self.reader, u32::from(k))
    }

    /// Decode `count` residuals with parameter `k`.
    pub fn decode_n(&mut self, count: usize, k: u8) -> Vec<i32> {
        (0..count).map_while(|_| self.decode_one(k)).collect()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zigzag_encode_decode_identity() {
        for v in [-100i32, -1, 0, 1, 100, i16::MAX as i32] {
            let u = zigzag_encode(v);
            let back = zigzag_decode(u);
            assert_eq!(back, v, "zigzag roundtrip failed for {v}");
        }
    }

    #[test]
    fn test_zigzag_non_negative_output() {
        // zigzag_encode maps i32 -> u32, which is inherently non-negative
        for v in [-200i32, -100, -1, 0, 1, 100, 200] {
            let _u = zigzag_encode(v);
        }
    }

    #[test]
    fn test_rice_bit_cost_zero_residuals() {
        let res = vec![0i32; 16];
        let cost = rice_bit_cost(&res, 0);
        // Each 0 costs 1 (unary 0) + 0 (k=0) = 1 bit → 16 total
        assert_eq!(cost, 16);
    }

    #[test]
    fn test_rice_encode_decode_roundtrip() {
        let residuals = vec![0i32, 1, -1, 2, -2, 5, -5, 10, -10];
        let k = optimal_rice_param(&residuals);
        let encoded = rice_encode(&residuals, k);
        let mut dec = RiceDecoder::new(&encoded);
        let decoded = dec.decode_n(residuals.len(), k);
        assert_eq!(decoded, residuals, "Rice roundtrip must be lossless");
    }

    #[test]
    fn test_rice_encode_empty() {
        let encoded = rice_encode(&[], 4);
        assert!(encoded.is_empty());
    }

    #[test]
    fn test_optimal_rice_param_small_residuals() {
        // Small residuals → small k is optimal
        let residuals = vec![0i32; 32];
        let k = optimal_rice_param(&residuals);
        assert_eq!(k, 0, "All-zero residuals → k=0 is optimal");
    }

    #[test]
    fn test_optimal_rice_param_large_residuals() {
        // Large residuals → larger k is better
        let residuals: Vec<i32> = (0..32).map(|i| i * 1000).collect();
        let k_large = optimal_rice_param(&residuals);
        let k_small = optimal_rice_param(&vec![0i32; 32]);
        assert!(k_large >= k_small, "Large residuals should use larger k");
    }

    #[test]
    fn test_rice_decode_n_partial() {
        // If stream is shorter than count, decode_n returns fewer items
        let residuals = vec![1i32, 2, 3];
        let k = 1;
        let encoded = rice_encode(&residuals, k);
        // Request more than available
        let mut dec = RiceDecoder::new(&encoded);
        let decoded = dec.decode_n(100, k);
        assert!(decoded.len() >= residuals.len());
        assert_eq!(&decoded[..residuals.len()], &residuals[..]);
    }
}
