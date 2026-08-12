//! Bit-level I/O and CRC primitives for the FLAC codec.
//!
//! FLAC is a *bit-oriented* format: subframes are packed back-to-back with no
//! byte alignment between them, and only the frame footer is byte-aligned
//! (RFC 9639 §9.3).  Both the encoder and the decoder therefore share the
//! [`BitWriter`] / [`BitReader`] pair defined here so that the two sides cannot
//! drift apart.
//!
//! # CRCs
//!
//! * Frame-header CRC-8 — polynomial `x^8 + x^2 + x^1 + x^0` (`0x07`),
//!   initial value 0, MSB-first, no reflection, no final XOR (RFC 9639 §9.1.7).
//! * Frame-footer CRC-16 — polynomial `x^16 + x^15 + x^2 + x^0` (`0x8005`),
//!   initial value 0, MSB-first, no reflection, no final XOR (RFC 9639 §9.3).

#![forbid(unsafe_code)]

// =============================================================================
// CRC tables
// =============================================================================

const fn build_crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn build_crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const CRC8_TABLE: [u8; 256] = build_crc8_table();
const CRC16_TABLE: [u16; 256] = build_crc16_table();

/// FLAC frame-header CRC-8 (poly `0x07`, init 0, no reflection, no final XOR).
///
/// Catalogued as the standalone "CRC-8" algorithm; its check value over ASCII
/// `"123456789"` is `0xF4`.
#[must_use]
pub fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        crc = CRC8_TABLE[usize::from(crc ^ byte)];
    }
    crc
}

/// FLAC frame-footer CRC-16 (poly `0x8005`, init 0, no reflection, no final XOR).
///
/// Catalogued as CRC-16/UMTS (a.k.a. CRC-16/BUYPASS); its check value over
/// ASCII `"123456789"` is `0xFEE8`.
#[must_use]
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in data {
        let idx = ((crc >> 8) ^ u16::from(byte)) & 0xFF;
        crc = (crc << 8) ^ CRC16_TABLE[idx as usize];
    }
    crc
}

#[inline]
const fn low_mask(n: u32) -> u64 {
    if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

// =============================================================================
// BitWriter
// =============================================================================

/// MSB-first bit writer used to serialise FLAC frames.
#[derive(Clone, Debug, Default)]
pub struct BitWriter {
    out: Vec<u8>,
    /// Pending bits, right-aligned in the low `nbits` positions (`nbits < 8`).
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    /// Create an empty writer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an empty writer with room for `bytes` output bytes.
    #[must_use]
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            out: Vec::with_capacity(bytes),
            acc: 0,
            nbits: 0,
        }
    }

    /// Total number of bits written so far.
    #[must_use]
    pub fn bit_len(&self) -> usize {
        self.out.len() * 8 + self.nbits as usize
    }

    /// Whether the writer currently sits on a byte boundary.
    #[must_use]
    pub fn is_byte_aligned(&self) -> bool {
        self.nbits == 0
    }

    /// Write the low `n` bits of `value`, most-significant bit first.
    ///
    /// `n` values above 64 are clamped to 64; `n == 0` is a no-op.
    pub fn write_bits(&mut self, value: u64, n: u32) {
        let n = n.min(64);
        if n == 0 {
            return;
        }
        let mut rem = n;
        while rem > 0 {
            let take = rem.min(32);
            let chunk = (value >> (rem - take)) & low_mask(take);
            self.push_small(chunk, take);
            rem -= take;
        }
    }

    /// Write the low `n` bits of a two's-complement signed value.
    pub fn write_bits_signed(&mut self, value: i64, n: u32) {
        self.write_bits((value as u64) & low_mask(n.min(64)), n);
    }

    /// Write a FLAC unary code: `q` zero bits followed by a single one bit
    /// (RFC 9639 §9.2.7.1).
    pub fn write_unary(&mut self, q: u32) {
        let mut rem = q;
        while rem >= 32 {
            self.write_bits(0, 32);
            rem -= 32;
        }
        // `rem` zeros followed by the terminating 1 bit.
        self.write_bits(1, rem + 1);
    }

    /// Pad with zero bits until the next byte boundary.
    pub fn align_to_byte(&mut self) {
        if self.nbits != 0 {
            let pad = 8 - self.nbits;
            self.write_bits(0, pad);
        }
    }

    /// Append whole bytes.  The writer must already be byte-aligned.
    ///
    /// # Errors
    ///
    /// Returns `false` (and writes nothing) when the writer is not aligned.
    pub fn write_aligned_bytes(&mut self, bytes: &[u8]) -> bool {
        if self.nbits != 0 {
            return false;
        }
        self.out.extend_from_slice(bytes);
        true
    }

    /// Bytes written so far.  Only meaningful when [`Self::is_byte_aligned`].
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.out
    }

    /// Consume the writer, returning the serialised bytes.
    ///
    /// Any residual partial byte is zero-padded first.
    #[must_use]
    pub fn into_bytes(mut self) -> Vec<u8> {
        self.align_to_byte();
        self.out
    }

    #[inline]
    fn push_small(&mut self, chunk: u64, n: u32) {
        debug_assert!(n <= 32 && self.nbits < 8);
        self.acc = (self.acc << n) | chunk;
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.out.push(((self.acc >> self.nbits) & 0xFF) as u8);
        }
        self.acc &= low_mask(self.nbits);
    }
}

// =============================================================================
// BitReader
// =============================================================================

/// MSB-first bit reader used to parse FLAC frames.
#[derive(Clone, Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    /// Create a reader over `data`, positioned at bit 0.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    /// The underlying byte slice.
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Current absolute bit position.
    #[must_use]
    pub fn bit_pos(&self) -> usize {
        self.bit_pos
    }

    /// Current byte position (rounded down).
    #[must_use]
    pub fn byte_pos(&self) -> usize {
        self.bit_pos / 8
    }

    /// Whether the reader sits on a byte boundary.
    #[must_use]
    pub fn is_byte_aligned(&self) -> bool {
        self.bit_pos % 8 == 0
    }

    /// Number of bits left in the buffer.
    #[must_use]
    pub fn bits_remaining(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.bit_pos)
    }

    /// Read a single bit, or `None` at end of data.
    #[inline]
    pub fn read_bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.bit_pos / 8)?;
        let shift = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Some(u32::from((byte >> shift) & 1))
    }

    /// Read `n` bits (`n <= 64`) MSB-first, or `None` at end of data.
    pub fn read_bits(&mut self, n: u32) -> Option<u64> {
        let n = n.min(64);
        if n == 0 {
            return Some(0);
        }
        if self.bits_remaining() < n as usize {
            return None;
        }
        let mut value = 0u64;
        let mut rem = n;
        while rem > 0 {
            let bit_off = self.bit_pos % 8;
            let avail = 8 - bit_off as u32;
            let take = rem.min(avail);
            let byte = u64::from(self.data[self.bit_pos / 8]);
            let chunk = (byte >> (avail - take)) & low_mask(take);
            value = (value << take) | chunk;
            self.bit_pos += take as usize;
            rem -= take;
        }
        Some(value)
    }

    /// Read `n` bits as a two's-complement signed integer.
    pub fn read_bits_signed(&mut self, n: u32) -> Option<i64> {
        let n = n.min(64);
        let raw = self.read_bits(n)?;
        if n == 0 {
            return Some(0);
        }
        if n == 64 {
            return Some(raw as i64);
        }
        let shift = 64 - n;
        Some(((raw << shift) as i64) >> shift)
    }

    /// Read a FLAC unary code: count zero bits up to and including the
    /// terminating one bit, returning the zero count.
    ///
    /// `limit` bounds the count so corrupt data cannot spin forever.
    pub fn read_unary(&mut self, limit: u32) -> Option<u32> {
        let mut count = 0u32;
        loop {
            if self.read_bit()? == 1 {
                return Some(count);
            }
            count += 1;
            if count > limit {
                return None;
            }
        }
    }

    /// Skip forward to the next byte boundary.
    pub fn align_to_byte(&mut self) {
        let rem = self.bit_pos % 8;
        if rem != 0 {
            self.bit_pos += 8 - rem;
        }
    }

    /// Read one byte; requires the reader to be byte-aligned.
    pub fn read_aligned_byte(&mut self) -> Option<u8> {
        if !self.is_byte_aligned() {
            return None;
        }
        let byte = *self.data.get(self.bit_pos / 8)?;
        self.bit_pos += 8;
        Some(byte)
    }

    /// Skip `n` bits; returns `None` when that would run past the end.
    pub fn skip_bits(&mut self, n: usize) -> Option<()> {
        if self.bits_remaining() < n {
            return None;
        }
        self.bit_pos += n;
        Some(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc8_catalogue_check_value() {
        assert_eq!(crc8(b"123456789"), 0xF4);
    }

    #[test]
    fn crc16_catalogue_check_value() {
        // CRC-16/UMTS check value over "123456789" is 0xFEE8.
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }

    #[test]
    fn crc_empty_inputs_are_zero() {
        assert_eq!(crc8(&[]), 0);
        assert_eq!(crc16(&[]), 0);
    }

    /// The table-driven CRCs must agree with the textbook bitwise reference.
    #[test]
    fn crc_tables_match_bitwise_reference() {
        fn crc8_ref(data: &[u8]) -> u8 {
            let mut crc = 0u8;
            for &b in data {
                crc ^= b;
                for _ in 0..8 {
                    crc = if crc & 0x80 != 0 {
                        (crc << 1) ^ 0x07
                    } else {
                        crc << 1
                    };
                }
            }
            crc
        }
        fn crc16_ref(data: &[u8]) -> u16 {
            let mut crc = 0u16;
            for &b in data {
                crc ^= u16::from(b) << 8;
                for _ in 0..8 {
                    crc = if crc & 0x8000 != 0 {
                        (crc << 1) ^ 0x8005
                    } else {
                        crc << 1
                    };
                }
            }
            crc
        }
        let mut state = 0x1234_5678u32;
        let mut buf = Vec::new();
        for _ in 0..512 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12345);
            buf.push((state >> 16) as u8);
            assert_eq!(crc8(&buf), crc8_ref(&buf));
            assert_eq!(crc16(&buf), crc16_ref(&buf));
        }
    }

    #[test]
    fn write_then_read_round_trips_bit_patterns() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0xDEAD_BEEF, 32);
        w.write_bits_signed(-3, 5);
        w.write_unary(0);
        w.write_unary(7);
        w.write_bits(0, 1);
        w.write_bits_signed(-1_234_567, 24);
        let bits = w.bit_len();
        let bytes = w.into_bytes();

        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_bits(3), Some(0b101));
        assert_eq!(r.read_bits(32), Some(0xDEAD_BEEF));
        assert_eq!(r.read_bits_signed(5), Some(-3));
        assert_eq!(r.read_unary(64), Some(0));
        assert_eq!(r.read_unary(64), Some(7));
        assert_eq!(r.read_bits(1), Some(0));
        assert_eq!(r.read_bits_signed(24), Some(-1_234_567));
        assert_eq!(r.bit_pos(), bits);
    }

    #[test]
    fn write_bits_is_msb_first() {
        let mut w = BitWriter::new();
        w.write_bits(1, 1);
        w.write_bits(0, 7);
        assert_eq!(w.into_bytes(), vec![0x80]);
    }

    #[test]
    fn unary_zero_is_single_one_bit() {
        let mut w = BitWriter::new();
        w.write_unary(0);
        w.align_to_byte();
        assert_eq!(w.into_bytes(), vec![0x80]);
    }

    #[test]
    fn unary_three_is_three_zeros_then_one() {
        let mut w = BitWriter::new();
        w.write_unary(3);
        w.align_to_byte();
        // 0001 0000
        assert_eq!(w.into_bytes(), vec![0x10]);
    }

    #[test]
    fn unary_large_counts_round_trip() {
        for q in [0u32, 1, 7, 8, 31, 32, 33, 100, 1000] {
            let mut w = BitWriter::new();
            w.write_unary(q);
            let bytes = w.into_bytes();
            let mut r = BitReader::new(&bytes);
            assert_eq!(r.read_unary(4096), Some(q), "unary round-trip for {q}");
        }
    }

    #[test]
    fn align_to_byte_pads_with_zeros() {
        let mut w = BitWriter::new();
        w.write_bits(0b111, 3);
        assert!(!w.is_byte_aligned());
        w.align_to_byte();
        assert!(w.is_byte_aligned());
        assert_eq!(w.as_bytes(), &[0b1110_0000]);
    }

    #[test]
    fn reader_signed_sign_extends() {
        let bytes = [0b1111_0000u8];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_bits_signed(4), Some(-1));
        assert_eq!(r.read_bits_signed(4), Some(0));
    }

    #[test]
    fn reader_returns_none_past_end() {
        let bytes = [0xFFu8];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_bits(8), Some(0xFF));
        assert_eq!(r.read_bit(), None);
        assert_eq!(r.read_bits(1), None);
        assert_eq!(r.read_aligned_byte(), None);
    }

    #[test]
    fn reader_unary_respects_limit() {
        let bytes = [0u8; 8];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_unary(16), None);
    }

    #[test]
    fn write_aligned_bytes_requires_alignment() {
        let mut w = BitWriter::new();
        w.write_bits(1, 1);
        assert!(!w.write_aligned_bytes(&[0xAA]));
        w.align_to_byte();
        assert!(w.write_aligned_bytes(&[0xAA]));
        assert_eq!(w.into_bytes(), vec![0x80, 0xAA]);
    }

    #[test]
    fn read_bits_spanning_many_bytes() {
        let mut w = BitWriter::new();
        w.write_bits(0b1, 1);
        w.write_bits(0x0123_4567_89AB_CDEF, 64);
        let bytes = w.into_bytes();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_bits(1), Some(1));
        assert_eq!(r.read_bits(64), Some(0x0123_4567_89AB_CDEF));
    }

    #[test]
    fn skip_and_align_track_positions() {
        let bytes = [0u8; 4];
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.skip_bits(3), Some(()));
        assert_eq!(r.bit_pos(), 3);
        assert_eq!(r.byte_pos(), 0);
        r.align_to_byte();
        assert_eq!(r.bit_pos(), 8);
        assert_eq!(r.byte_pos(), 1);
        assert_eq!(r.skip_bits(1000), None);
    }
}
