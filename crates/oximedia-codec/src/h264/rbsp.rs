//! RBSP (Raw Byte Sequence Payload) helpers.
//!
//! H.264 (and HEVC) NAL units carry an *escaped* byte stream so that
//! the start-code prefix `0x00 0x00 0x01` cannot occur inside a payload.
//! Whenever the encoder would otherwise emit `0x00 0x00 0x00 |
//! 0x01 | 0x02 | 0x03`, it inserts an "emulation prevention byte" of
//! value `0x03` so the pattern becomes `0x00 0x00 0x03 ...`.
//!
//! Before the bit-level parsers in [`crate::h264::sps`],
//! [`crate::h264::pps`], or [`crate::h264::slice_header`] can run, those
//! `0x03` bytes must be removed.  Failing to do so causes spurious
//! `exp-Golomb prefix exceeds 32 zeros` errors on streams whose payloads
//! happen to contain long zero runs.

/// Strips emulation prevention bytes from a NAL unit payload.
///
/// The function copies into a fresh `Vec`.  Pass the bytes *after* the
/// NAL header (the first byte) so the result starts with the first byte
/// of the RBSP proper.
///
/// # Examples
///
/// ```
/// use oximedia_codec::h264::rbsp::strip_emulation_prevention;
/// let payload = [0xAB, 0xCD, 0x00, 0x00, 0x03, 0x00, 0xEF];
/// assert_eq!(
///     strip_emulation_prevention(&payload),
///     vec![0xAB, 0xCD, 0x00, 0x00, 0x00, 0xEF],
/// );
/// ```
#[must_use]
pub fn strip_emulation_prevention(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zero_run: u8 = 0;
    for &b in payload {
        if zero_run >= 2 && b == 0x03 {
            // Drop the emulation prevention byte; do not advance the run,
            // because the *next* byte still sits two zeros deep.
            zero_run = 0;
            continue;
        }
        out.push(b);
        if b == 0x00 {
            zero_run = zero_run.saturating_add(1);
        } else {
            zero_run = 0;
        }
    }
    out
}

/// Length of the trailing RBSP stop bit + zero byte padding.  Returns
/// the number of *bits* the parser should not treat as payload — usually
/// 8 plus a small variable amount, depending on how much zero padding
/// the encoder added.
///
/// The result is used by exact conformance checks; most callers just
/// rely on [`crate::h264::bit_reader::BitReader::more_rbsp_data`] to
/// stop at the right place.
///
/// Returns 0 when no stop bit is present (the caller should treat this
/// as a parse error).
#[must_use]
pub fn trailing_bits_len(rbsp: &[u8]) -> usize {
    // Walk backwards over zero bytes until a non-zero byte appears; that
    // last byte must contain the stop bit (a single 1 followed by zero
    // padding).
    let mut idx = rbsp.len();
    while idx > 0 && rbsp[idx - 1] == 0x00 {
        idx -= 1;
    }
    if idx == 0 {
        return 0;
    }
    let last = rbsp[idx - 1];
    // Number of trailing zero bits before the stop bit's `1`.
    let trailing_zeros = last.trailing_zeros();
    if (last >> trailing_zeros) & 1 != 1 {
        // Should be unreachable given the loop above, but stay safe.
        return 0;
    }
    let stop_byte_bits = 1 + trailing_zeros as usize;
    let zero_padding_bytes = rbsp.len() - idx;
    stop_byte_bits + zero_padding_bytes * 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_keeps_short_runs() {
        let raw = [0x12, 0x00, 0x34, 0x00, 0x56];
        assert_eq!(strip_emulation_prevention(&raw), raw.to_vec());
    }

    #[test]
    fn strip_removes_emulation_byte_after_two_zeros() {
        let raw = [0x12, 0x00, 0x00, 0x03, 0x01, 0xFF];
        assert_eq!(
            strip_emulation_prevention(&raw),
            vec![0x12, 0x00, 0x00, 0x01, 0xFF],
        );
    }

    #[test]
    fn strip_handles_back_to_back_emulation_bytes() {
        // Original payload `00 00 00 00 01` (long zero run + trigger).
        // Encoder must insert an emulation byte after each pair of zeros
        // that precedes a trigger value, so the wire form is:
        //   00 00 03 00 00 03 01
        // and decoding must recover the original.
        let raw = [0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x01];
        assert_eq!(
            strip_emulation_prevention(&raw),
            vec![0x00, 0x00, 0x00, 0x00, 0x01],
        );
    }

    #[test]
    fn strip_passes_through_03_with_only_one_preceding_zero() {
        // Only one preceding zero -> the 03 is real data, not emulation.
        let raw = [0x00, 0x03, 0xAB];
        assert_eq!(strip_emulation_prevention(&raw), raw.to_vec());
    }

    #[test]
    fn trailing_bits_len_one_bit_stop() {
        // Last byte is 0x80 = 1000_0000 -> trailing_zeros=7, stop=1+7=8 bits.
        assert_eq!(trailing_bits_len(&[0xAB, 0x80]), 8);
    }

    #[test]
    fn trailing_bits_len_with_zero_padding() {
        // 0x80 0x00 0x00 -> stop_byte=8, padding=2*8 = 24 bits.
        assert_eq!(trailing_bits_len(&[0xAB, 0x80, 0x00, 0x00]), 24);
    }

    #[test]
    fn trailing_bits_len_no_stop_bit_returns_zero() {
        assert_eq!(trailing_bits_len(&[0x00, 0x00]), 0);
        assert_eq!(trailing_bits_len(&[]), 0);
    }
}
