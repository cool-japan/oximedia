//! Bit-level reader for H.264 bitstream syntax.
//!
//! H.264 (ITU-T Rec. H.264 / ISO/IEC 14496-10) packs its parameter sets
//! and slice headers as a sequence of variable-width fields with no byte
//! alignment between them.  Parsing requires a reader that exposes
//! arbitrary-width bit reads, plus the two exp-Golomb codes used
//! throughout the spec:
//!
//! - `ue(v)` — unsigned exp-Golomb (e.g. `seq_parameter_set_id`)
//! - `se(v)` — signed exp-Golomb (e.g. `slice_qp_delta`)
//!
//! Both are self-delimiting: the codeword's leading zero count tells the
//! reader its length.
//!
//! The reader operates over a borrowed `&[u8]` buffer (typically an RBSP
//! produced by [`crate::h264::rbsp::strip_emulation_prevention`]) and is
//! MSB-first, matching the H.264 spec convention.

use crate::CodecError;

/// Maximum number of leading zeros accepted in an exp-Golomb prefix
/// before the reader bails with [`CodecError::InvalidData`].  H.264
/// values never exceed 32 bits, so a prefix of more than 31 leading
/// zeros indicates either a corrupt bitstream or that emulation
/// prevention bytes were not stripped.
const MAX_EXP_GOLOMB_LEADING_ZEROS: u32 = 32;

/// MSB-first bit reader over a borrowed byte buffer.
///
/// Designed for parsing H.264 RBSP payloads.  See the module docs for
/// usage.
#[derive(Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    /// Wraps `data` in a fresh bit reader positioned at the first bit.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Returns the number of bits already consumed.
    #[must_use]
    pub fn bits_consumed(&self) -> usize {
        self.byte_pos * 8 + self.bit_pos as usize
    }

    /// Returns true once every bit of the underlying buffer has been
    /// consumed.  Useful for verifying that an SPS/PPS parse landed
    /// exactly at the RBSP stop bit.
    #[must_use]
    pub fn is_at_end(&self) -> bool {
        self.byte_pos >= self.data.len()
    }

    /// Reads one bit, MSB first.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::InvalidData`] when the buffer is exhausted.
    pub fn read_bit(&mut self) -> Result<bool, CodecError> {
        if self.byte_pos >= self.data.len() {
            return Err(CodecError::InvalidData(
                "h264 bit reader: unexpected end of bitstream".into(),
            ));
        }
        let byte = self.data[self.byte_pos];
        let bit = (byte >> (7 - self.bit_pos)) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(bit != 0)
    }

    /// Reads `n` bits as an unsigned integer.  `n` must be in `1..=32`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::InvalidData`] if `n` is out of range or the
    /// buffer is exhausted.
    pub fn read_bits(&mut self, n: u32) -> Result<u32, CodecError> {
        if n == 0 || n > 32 {
            return Err(CodecError::InvalidData(format!(
                "h264 bit reader: invalid bit count {n}"
            )));
        }
        let mut value: u32 = 0;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Ok(value)
    }

    /// Reads an H.264 unsigned exp-Golomb code (`ue(v)`).
    ///
    /// Encoding (from H.264 §9.1):
    ///
    /// ```text
    /// codeword = (N leading zeros) 1 (N significant bits)
    /// value    = (1 << N) - 1 + suffix
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::InvalidData`] when the prefix exceeds the
    /// internal sanity bound of 32 zeros — generally a sign that
    /// emulation prevention bytes were not stripped from the RBSP.
    pub fn read_ue(&mut self) -> Result<u32, CodecError> {
        let mut leading_zeros: u32 = 0;
        while !self.read_bit()? {
            leading_zeros += 1;
            if leading_zeros > MAX_EXP_GOLOMB_LEADING_ZEROS {
                return Err(CodecError::InvalidData(
                    "h264 bit reader: exp-Golomb prefix exceeds 32 zeros".into(),
                ));
            }
        }
        if leading_zeros == 0 {
            return Ok(0);
        }
        let suffix = self.read_bits(leading_zeros)?;
        Ok((1u32 << leading_zeros) - 1 + suffix)
    }

    /// Reads an H.264 signed exp-Golomb code (`se(v)`).
    ///
    /// The encoder maps signed `s` to unsigned `k` via the H.264 §9.1.1
    /// table: 0 → 0, 1 → 1, -1 → 2, 2 → 3, -2 → 4, ...
    ///
    /// # Errors
    ///
    /// Propagates errors from [`Self::read_ue`].
    pub fn read_se(&mut self) -> Result<i32, CodecError> {
        let k = self.read_ue()? as i64;
        // Inverse mapping: positive integers got odd k, negatives got even k.
        let v = if k & 1 == 0 { -(k / 2) } else { (k + 1) / 2 };
        Ok(v as i32)
    }

    /// Skips `n` bits.  Useful when a syntax element has been recognised
    /// as not-of-interest (e.g. reserved or VUI fields the caller does
    /// not consume).
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::InvalidData`] when the buffer is exhausted.
    pub fn skip_bits(&mut self, n: u32) -> Result<(), CodecError> {
        for _ in 0..n {
            self.read_bit()?;
        }
        Ok(())
    }

    /// Returns true while there is at least one further bit that is
    /// neither the RBSP stop bit nor trailing zero padding.  Implements
    /// the H.264 `more_rbsp_data()` helper used to gate optional
    /// trailing syntax elements (notably the `vui_parameters_present_flag`
    /// guarding the VUI section of an SPS).
    #[must_use]
    pub fn more_rbsp_data(&self) -> bool {
        let total_bits = self.data.len() * 8;
        let pos = self.bits_consumed();
        if pos >= total_bits {
            return false;
        }
        // Look for a 1 bit anywhere strictly after the current position.
        for i in (pos + 1)..total_bits {
            let byte = self.data[i / 8];
            let bit = (byte >> (7 - (i % 8))) & 1;
            if bit == 1 {
                return true;
            }
        }
        // Only zeros remain after the current bit; combined with the
        // expected RBSP stop bit at `pos`, there is no further payload.
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_bits_returns_msb_first() {
        // 1010_0110 0011_1100 -> first 4 bits = 1010 = 10, next 12 = 0110_0011_1100 = 1596
        let mut r = BitReader::new(&[0xA6, 0x3C]);
        assert_eq!(r.read_bits(4).unwrap(), 0b1010);
        assert_eq!(r.read_bits(12).unwrap(), 0b0110_0011_1100);
        assert!(r.is_at_end());
    }

    #[test]
    fn read_bit_advances_across_byte_boundary() {
        let mut r = BitReader::new(&[0x01, 0x80]);
        // Seven zero bits, then 1, then 1, then six zeros.
        for _ in 0..7 {
            assert!(!r.read_bit().unwrap());
        }
        assert!(r.read_bit().unwrap());
        assert!(r.read_bit().unwrap());
    }

    #[test]
    fn read_ue_decodes_table() {
        // From H.264 §9.1 worked table:
        //   0    -> codeword `1`            -> single bit set
        //   1    -> codeword `010`          -> 0b0100_0000
        //   2    -> codeword `011`
        //   3    -> codeword `00100`
        //   8    -> codeword `0001001`
        let cases = [
            (vec![0b1000_0000u8], 0),
            (vec![0b0100_0000], 1),
            (vec![0b0110_0000], 2),
            (vec![0b0010_0000], 3),
            (vec![0b0001_0010, 0x00], 8),
        ];
        for (bytes, expected) in cases {
            let mut r = BitReader::new(&bytes);
            assert_eq!(r.read_ue().unwrap(), expected, "bytes = {bytes:02x?}");
        }
    }

    #[test]
    fn read_se_round_trip_signs() {
        // 0->0, 1->1, 2->-1, 3->2, 4->-2 ...
        // Codewords concatenated: 1 | 010 | 011 | 00100 | 00101
        // = 1 010 011 0010 0001 01
        //   1010_0110 0100_0010 1xxx_xxxx
        let data = [0b1010_0110u8, 0b0100_0010, 0b1000_0000];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read_se().unwrap(), 0);
        assert_eq!(r.read_se().unwrap(), 1);
        assert_eq!(r.read_se().unwrap(), -1);
        assert_eq!(r.read_se().unwrap(), 2);
        assert_eq!(r.read_se().unwrap(), -2);
    }

    #[test]
    fn read_bits_errors_on_exhaustion() {
        let mut r = BitReader::new(&[]);
        assert!(r.read_bit().is_err());
    }

    #[test]
    fn read_bits_rejects_out_of_range() {
        let mut r = BitReader::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(r.read_bits(0).is_err());
        assert!(r.read_bits(33).is_err());
    }

    #[test]
    fn read_ue_rejects_runaway_prefix() {
        // 33 zero bits then 1: way past sanity bound.
        let mut data = vec![0u8; 5];
        data.push(0b1000_0000);
        let mut r = BitReader::new(&data);
        assert!(r.read_ue().is_err());
    }

    #[test]
    fn more_rbsp_data_finds_trailing_one() {
        // bits: 1 0000_0001 -> at position 1, future 1 bit exists.
        let r_at_one = {
            let mut r = BitReader::new(&[0b1000_0001]);
            r.read_bit().unwrap();
            r
        };
        assert!(r_at_one.more_rbsp_data());
    }

    #[test]
    fn more_rbsp_data_returns_false_at_stop_bit() {
        // bits: 0000_1000 -> at position 4, only the stop bit + zeros follow.
        let r = {
            let mut r = BitReader::new(&[0b0000_1000]);
            for _ in 0..4 {
                r.read_bit().unwrap();
            }
            r
        };
        assert!(!r.more_rbsp_data());
    }
}
