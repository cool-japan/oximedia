//! Bit-level reader for ProRes entropy-coded slice payloads.
//!
//! ProRes coefficients are coded with adaptive exp-Golomb / Rice codes
//! (RDD 36 §6.5.5–6.5.6), which require pulling individual bits and
//! variable-length codewords from a packed bytestream. This module is
//! the byte-aligned input → bit-by-bit consumer plumbing; the actual
//! Golomb decoder is built on top of it in [`super::decode`].
//!
//! Bit order within each byte is MSB-first: bit 7 of byte 0 is read
//! first, then bit 6, … bit 0, then bit 7 of byte 1, etc. This matches
//! the H.26x / ProRes / MPEG bitstream convention.

/// Errors produced by the bit reader.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BitReaderError {
    /// Caller asked for more bits than the buffer has remaining.
    #[error("bit reader out of input: needed {needed} bits, had {available}")]
    OutOfInput {
        /// Bits requested.
        needed: u32,
        /// Bits remaining at the time of the request.
        available: u32,
    },
    /// Caller asked for more than 32 bits in a single `read_bits` call.
    #[error("bit reader: cannot read {0} bits in a single read (max 32)")]
    TooManyBits(u32),
}

/// MSB-first bit reader over an immutable byte slice.
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Byte index of the next byte we'll fetch from.
    byte_pos: usize,
    /// Bits consumed within the current byte (0..=7). When this hits 8
    /// we advance `byte_pos` and reset to 0.
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    /// Wrap a byte slice for bit-level reading.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Number of bits left to read.
    #[must_use]
    pub fn bits_remaining(&self) -> u32 {
        (self.data.len() as u32)
            .saturating_sub(self.byte_pos as u32)
            .saturating_mul(8)
            .saturating_sub(self.bit_pos as u32)
    }

    /// True if every bit in the buffer has been consumed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bits_remaining() == 0
    }

    /// Read the next `n` bits (1..=32) as a big-endian `u32`.
    pub fn read_bits(&mut self, n: u32) -> Result<u32, BitReaderError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 {
            return Err(BitReaderError::TooManyBits(n));
        }
        if self.bits_remaining() < n {
            return Err(BitReaderError::OutOfInput {
                needed: n,
                available: self.bits_remaining(),
            });
        }
        let mut value: u32 = 0;
        for _ in 0..n {
            let byte = self.data[self.byte_pos];
            let bit = (byte >> (7 - self.bit_pos)) & 1;
            value = (value << 1) | u32::from(bit);
            self.bit_pos += 1;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.byte_pos += 1;
            }
        }
        Ok(value)
    }

    /// Read a single bit.
    pub fn read_bit(&mut self) -> Result<u32, BitReaderError> {
        self.read_bits(1)
    }

    /// Count the number of leading 1-bits before the next 0-bit, then
    /// consume the 0. Used by the unary prefix of exp-Golomb codes.
    /// Returns the prefix length (i.e. the count of 1s).
    pub fn count_leading_ones(&mut self) -> Result<u32, BitReaderError> {
        let mut count = 0u32;
        loop {
            let bit = self.read_bit()?;
            if bit == 0 {
                return Ok(count);
            }
            count += 1;
            // Defensive bound — a properly framed slice will hit the 0
            // long before this, but a malformed stream might loop forever.
            if count > 64 {
                return Err(BitReaderError::OutOfInput {
                    needed: count + 1,
                    available: self.bits_remaining(),
                });
            }
        }
    }

    /// Align the read cursor to the next byte boundary. No-op if already aligned.
    pub fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.byte_pos += 1;
            self.bit_pos = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_single_bits_msb_first() {
        // 0b10110100 = 0xB4
        let buf = [0xB4u8];
        let mut r = BitReader::new(&buf);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 1);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert_eq!(r.read_bit().unwrap(), 0);
        assert!(r.is_empty());
    }

    #[test]
    fn read_bits_packs_correctly_across_byte_boundary() {
        // 0xAB 0xCD = 0b1010_1011 1100_1101.
        // Read 12 bits = 0xABC.
        let buf = [0xAB, 0xCD];
        let mut r = BitReader::new(&buf);
        assert_eq!(r.read_bits(12).unwrap(), 0xABC);
        assert_eq!(r.bits_remaining(), 4);
        assert_eq!(r.read_bits(4).unwrap(), 0xD);
    }

    #[test]
    fn read_zero_bits_returns_zero() {
        let buf = [0xFFu8];
        let mut r = BitReader::new(&buf);
        assert_eq!(r.read_bits(0).unwrap(), 0);
        assert_eq!(r.bits_remaining(), 8);
    }

    #[test]
    fn read_too_many_bits_errors() {
        let buf = [0u8; 4];
        let mut r = BitReader::new(&buf);
        assert_eq!(
            r.read_bits(33).unwrap_err(),
            BitReaderError::TooManyBits(33)
        );
    }

    #[test]
    fn out_of_input_errors_clean() {
        let buf = [0xFFu8];
        let mut r = BitReader::new(&buf);
        r.read_bits(8).unwrap();
        assert!(matches!(
            r.read_bit().unwrap_err(),
            BitReaderError::OutOfInput { .. }
        ));
    }

    #[test]
    fn count_leading_ones_counts_unary_prefix() {
        // 0b1110_0000 = three 1s, then 0.
        let buf = [0xE0];
        let mut r = BitReader::new(&buf);
        assert_eq!(r.count_leading_ones().unwrap(), 3);
        // The four bits remaining are all 0.
        assert_eq!(r.read_bits(4).unwrap(), 0);
    }

    #[test]
    fn count_leading_ones_zero_when_first_bit_zero() {
        let buf = [0x00u8];
        let mut r = BitReader::new(&buf);
        assert_eq!(r.count_leading_ones().unwrap(), 0);
    }

    #[test]
    fn align_to_byte_advances_to_boundary() {
        let buf = [0xFF, 0x00];
        let mut r = BitReader::new(&buf);
        r.read_bits(3).unwrap();
        r.align_to_byte();
        assert_eq!(r.bits_remaining(), 8);
        assert_eq!(r.read_bits(8).unwrap(), 0);
    }
}
