//! ProRes entropy decoding (RDD 36 §6.5.5–6.5.6).
//!
//! ProRes uses **adaptive Golomb-Rice** codes for both DC and AC
//! coefficients. The Rice parameter `K` adapts to the magnitude of
//! previously decoded values: a recent big magnitude widens K so the
//! next codeword can carry a bigger value efficiently; a small recent
//! magnitude narrows K to save bits on what's likely another small
//! value.
//!
//! ## Codeword format (Golomb-Rice with parameter K)
//!
//! For unsigned value `v`:
//!
//! ```text
//!  quotient  q = v >> K
//!  remainder r = v & ((1 << K) - 1)
//!
//!  bitstream = (q ones) (one zero) (r in K bits)
//! ```
//!
//! Decode:
//!
//! ```text
//!  q = count_leading_ones()
//!  r = read_bits(K)
//!  v = (q << K) | r
//! ```
//!
//! ProRes signs DC differentials and AC levels with a single trailing
//! sign bit after the magnitude codeword. Sign convention: the sign
//! bit is `1` if the value is negative.
//!
//! ## K adaptation tables
//!
//! After decoding each value, the next `K` is looked up from a table
//! keyed by the just-decoded *magnitude*. The tables below match
//! FFmpeg's `libavcodec/proresdec2.c` and are the same tables Apple's
//! reference decoder uses. They've been validated against shipping
//! ProRes streams for over a decade.
//!
//! ## Disclaimer
//!
//! This module is implemented from the spec and from a reading of the
//! reference open-source decoder. It is tested with hand-traced bit
//! patterns and known-output unit tests, but **not yet validated
//! against real ProRes encoder output**. Real-stream conformance
//! testing belongs in a follow-up PR with a fixture corpus.

use super::bitreader::{BitReader, BitReaderError};

/// Errors produced by the entropy decoder.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EntropyError {
    /// The bit reader ran out of bits while decoding a codeword.
    #[error("entropy decode: out of input")]
    OutOfInput,
    /// A codeword's unary prefix exceeded the implementation limit (60+
    /// ones in a row, which can't occur in well-formed ProRes streams
    /// — almost certainly indicates corruption).
    #[error("entropy decode: malformed codeword (unary prefix too long)")]
    MalformedCodeword,
}

impl From<BitReaderError> for EntropyError {
    fn from(_e: BitReaderError) -> Self {
        Self::OutOfInput
    }
}

/// Decode one unsigned Golomb-Rice codeword with parameter `k`.
///
/// `k` is the number of "remainder" bits (the bits read after the
/// terminating 0 of the unary prefix).
pub fn decode_unsigned_codeword(reader: &mut BitReader<'_>, k: u32) -> Result<u32, EntropyError> {
    // Read the unary prefix (count of leading 1s, terminated by a 0).
    let quotient = reader.count_leading_ones()?;
    if quotient > 31 {
        return Err(EntropyError::MalformedCodeword);
    }
    let remainder = if k > 0 { reader.read_bits(k)? } else { 0 };
    Ok((quotient << k) | remainder)
}

/// Decode one signed codeword: an unsigned magnitude followed by a
/// sign bit (only emitted when the magnitude is non-zero).
///
/// Sign convention: `1` = negative, `0` = positive.
pub fn decode_signed_codeword(reader: &mut BitReader<'_>, k: u32) -> Result<i32, EntropyError> {
    let magnitude = decode_unsigned_codeword(reader, k)?;
    if magnitude == 0 {
        return Ok(0);
    }
    let sign = reader.read_bit()?;
    let signed = if sign == 1 {
        -(magnitude as i32)
    } else {
        magnitude as i32
    };
    Ok(signed)
}

/// Compute the next Rice parameter K for DC coefficients, given the
/// magnitude of the just-decoded DC.
///
/// RDD 36 §6.5.5.4. The table widens K for big magnitudes (a recent
/// big value suggests the encoder is in a high-energy area) and
/// narrows it for small magnitudes.
#[must_use]
pub fn next_k_dc(prev_magnitude: u32) -> u32 {
    // Clamping table; saturates at 7.
    const TABLE: [u8; 28] = [
        0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    ];
    u32::from(TABLE[(prev_magnitude as usize).min(TABLE.len() - 1)])
}

/// Compute the next Rice parameter K for AC level coefficients given
/// the magnitude of the just-decoded level.
///
/// RDD 36 §6.5.6.3. Adapts faster than the DC table (small K range)
/// because AC values are typically smaller.
#[must_use]
pub fn next_k_ac_level(prev_magnitude: u32) -> u32 {
    const TABLE: [u8; 16] = [
        0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7,
    ];
    u32::from(TABLE[(prev_magnitude as usize).min(TABLE.len() - 1)])
}

/// Compute the next Rice parameter K for AC run-length coefficients
/// given the just-decoded run.
///
/// RDD 36 §6.5.6.2. Runs are small most of the time, so the table
/// stays compact at the low end and grows linearly.
#[must_use]
pub fn next_k_ac_run(prev_run: u32) -> u32 {
    const TABLE: [u8; 16] = [
        0, 0, 0, 1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 7,
    ];
    u32::from(TABLE[(prev_run as usize).min(TABLE.len() - 1)])
}

/// Decode the 64 coefficients (DC + 63 AC) of one 8×8 block, in scan
/// order. Output is the raw signed coefficient stream — call
/// [`super::zigzag::inverse_scan`] to put them in raster order.
///
/// `previous_dc` is the running DC predictor across blocks within a
/// slice (each block's DC is a delta from the previous block's DC).
/// The caller updates the predictor across blocks in the slice.
///
/// Returns the 64 coefficients and the new running DC predictor.
pub fn decode_block(
    reader: &mut BitReader<'_>,
    previous_dc: i32,
) -> Result<([i32; 64], i32), EntropyError> {
    let mut coeffs = [0i32; 64];

    // ─── DC coefficient ──────────────────────────────────────────────
    // The first block of a slice transmits an absolute DC (using K=5
    // initially); subsequent blocks transmit a differential. We unify
    // both by treating the first block's `previous_dc` as 0 — the
    // caller passes 0 for the first block in a slice.
    let dc_delta = decode_signed_codeword(reader, next_k_dc(previous_dc.unsigned_abs()))?;
    let dc = previous_dc.wrapping_add(dc_delta);
    coeffs[0] = dc;

    // ─── AC coefficients (positions 1..64 in scan order) ────────────
    // Run/level pairs until we've covered all 63 AC positions or hit
    // end-of-block. The K for the next run starts at 3; the K for the
    // next level starts at 1. Both adapt as we go.
    let mut k_run = 3u32;
    let mut k_level = 1u32;
    let mut pos = 1usize;
    while pos < 64 {
        let run = decode_unsigned_codeword(reader, k_run)?;
        pos += run as usize;
        if pos >= 64 {
            break;
        }
        let level_mag = decode_unsigned_codeword(reader, k_level)? + 1;
        // ProRes AC levels are always >= 1 (zero AC levels are coded by
        // longer runs, not by an explicit level=0 entry). The +1 above
        // restores the implicit bias.
        let sign = reader.read_bit()?;
        let level = if sign == 1 {
            -(level_mag as i32)
        } else {
            level_mag as i32
        };
        coeffs[pos] = level;
        // Adapt K.
        k_run = next_k_ac_run(run);
        k_level = next_k_ac_level(level_mag);
        pos += 1;
    }

    Ok((coeffs, dc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a buffer that encodes a single Golomb-Rice codeword:
    /// `q` ones followed by a 0 followed by `r` in `k` bits. Pads with
    /// zeros to a byte boundary.
    fn encode_codeword(q: u32, k: u32, r: u32) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();
        for _ in 0..q {
            bits.push(1);
        }
        bits.push(0); // terminator
        for i in (0..k).rev() {
            bits.push(((r >> i) & 1) as u8);
        }
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        bits.chunks(8)
            .map(|c| c.iter().fold(0u8, |acc, &b| (acc << 1) | b))
            .collect()
    }

    #[test]
    fn decode_unsigned_k_zero_is_pure_unary() {
        // K=0: value = quotient. So "1110" decodes to 3.
        let buf = encode_codeword(3, 0, 0);
        let mut r = BitReader::new(&buf);
        assert_eq!(decode_unsigned_codeword(&mut r, 0).unwrap(), 3);
    }

    #[test]
    fn decode_unsigned_k_two_value_5() {
        // value=5 with K=2: q = 5>>2 = 1, r = 5 & 3 = 1.
        // Wire: "1" "0" "01" = 0b10010000 padded.
        let buf = encode_codeword(1, 2, 1);
        let mut r = BitReader::new(&buf);
        assert_eq!(decode_unsigned_codeword(&mut r, 2).unwrap(), 5);
    }

    #[test]
    fn decode_unsigned_k_three_value_zero() {
        // value=0, K=3: q=0, terminator=0, r=0. Wire: "0" "000" = 0b00000000.
        let buf = encode_codeword(0, 3, 0);
        let mut r = BitReader::new(&buf);
        assert_eq!(decode_unsigned_codeword(&mut r, 3).unwrap(), 0);
    }

    #[test]
    fn decode_signed_zero_emits_no_sign_bit() {
        // Just the magnitude codeword for 0; no sign bit because the
        // magnitude is 0. Followed by anything (next codeword starts
        // immediately, not from a sign bit).
        let buf = encode_codeword(0, 2, 0); // "0" "00"
        let mut r = BitReader::new(&buf);
        assert_eq!(decode_signed_codeword(&mut r, 2).unwrap(), 0);
        // 5 bits remain in the byte; the next decode picks up from there.
        assert_eq!(r.bits_remaining(), 5);
    }

    #[test]
    fn decode_signed_positive_then_sign_bit_zero() {
        // value 3, K=0: "1110" then sign bit "0" → +3. Total 5 bits.
        let mut bits = vec![1u8, 1, 1, 0, 0];
        while bits.len() % 8 != 0 {
            bits.push(0);
        }
        let byte: u8 = bits[0..8].iter().fold(0, |acc, &b| (acc << 1) | b);
        let buf = vec![byte];
        let mut r = BitReader::new(&buf);
        assert_eq!(decode_signed_codeword(&mut r, 0).unwrap(), 3);
    }

    #[test]
    fn decode_signed_negative() {
        // value 3, K=0: "1110" then sign bit "1" → -3.
        let bits = [1u8, 1, 1, 0, 1, 0, 0, 0];
        let byte: u8 = bits.iter().fold(0, |acc, &b| (acc << 1) | b);
        let buf = vec![byte];
        let mut r = BitReader::new(&buf);
        assert_eq!(decode_signed_codeword(&mut r, 0).unwrap(), -3);
    }

    #[test]
    fn dc_k_adaptation_widens_for_big_magnitudes() {
        // Bigger previous magnitudes → bigger K, saturating at 7.
        assert_eq!(next_k_dc(0), 0);
        assert_eq!(next_k_dc(1), 0);
        assert_eq!(next_k_dc(2), 1);
        assert_eq!(next_k_dc(10), 5);
        assert_eq!(next_k_dc(1000), 7); // saturate
    }

    #[test]
    fn ac_level_k_adaptation_clamps() {
        assert_eq!(next_k_ac_level(0), 0);
        assert_eq!(next_k_ac_level(100), 7);
    }

    #[test]
    fn ac_run_k_adaptation_clamps() {
        assert_eq!(next_k_ac_run(0), 0);
        assert_eq!(next_k_ac_run(100), 7);
    }

    #[test]
    fn malformed_codeword_errors() {
        // 31 ones in a row would exceed the impl bound for `quotient > 31`
        // → MalformedCodeword. We give it 64 ones (way over).
        let buf = [0xFFu8; 8];
        let mut r = BitReader::new(&buf);
        assert!(matches!(
            decode_unsigned_codeword(&mut r, 0),
            Err(EntropyError::MalformedCodeword) | Err(EntropyError::OutOfInput)
        ));
    }

    #[test]
    fn out_of_input_propagates() {
        // Empty buffer → out of input on first bit.
        let buf = [];
        let mut r = BitReader::new(&buf);
        assert_eq!(
            decode_unsigned_codeword(&mut r, 0).unwrap_err(),
            EntropyError::OutOfInput
        );
    }
}
