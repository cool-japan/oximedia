//! Rice coding for FLAC residuals.
//!
//! FLAC uses Rice coding for the residual signal after linear prediction.
//! Rice coding is a form of Golomb coding that is efficient for encoding
//! values with a geometric distribution.
//!
//! # Rice Coding
//!
//! A value `n` is encoded with parameter `k` as:
//! - Quotient `q = n >> k` in unary (q ones followed by a zero)
//! - Remainder `r = n & ((1 << k) - 1)` in binary (k bits)
//!
//! Negative values are mapped to positive using zig-zag encoding:
//! - 0 -> 0, -1 -> 1, 1 -> 2, -2 -> 3, 2 -> 4, ...

#![forbid(unsafe_code)]

use crate::AudioError;

/// Encode a signed value using zig-zag encoding.
#[must_use]
#[allow(clippy::cast_sign_loss)]
pub fn zigzag_encode(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

/// Decode a zig-zag encoded value.
#[must_use]
#[allow(clippy::cast_possible_wrap)]
pub fn zigzag_decode(value: u32) -> i32 {
    ((value >> 1) as i32) ^ (-((value & 1) as i32))
}

/// Rice partition within a residual block.
#[derive(Debug, Clone, Default)]
pub struct RicePartition {
    /// Rice parameter (number of bits for remainder).
    pub parameter: u8,
    /// Number of samples in this partition.
    pub sample_count: usize,
    /// Decoded residual values.
    pub residuals: Vec<i32>,
}

impl RicePartition {
    /// Create a new partition.
    #[must_use]
    pub fn new(parameter: u8, sample_count: usize) -> Self {
        Self {
            parameter,
            sample_count,
            residuals: Vec::with_capacity(sample_count),
        }
    }

    /// Decode a single Rice-coded value.
    ///
    /// # Errors
    ///
    /// Returns error if decoding fails.
    #[allow(dead_code)]
    pub fn decode_value(&self, bits: &mut dyn Iterator<Item = bool>) -> Result<i32, AudioError> {
        // Count unary part: RFC 9639 section 9.2.7 — zero bits before a
        // terminating one bit.
        let mut quotient = 0u32;
        loop {
            match bits.next() {
                Some(false) => quotient += 1,
                Some(true) => break,
                None => return Err(AudioError::NeedMoreData),
            }
        }

        // Read binary part (parameter bits)
        let mut remainder = 0u32;
        for _ in 0..self.parameter {
            match bits.next() {
                Some(bit) => {
                    remainder = (remainder << 1) | u32::from(bit);
                }
                None => return Err(AudioError::NeedMoreData),
            }
        }

        // Combine and zig-zag decode
        let unsigned = (quotient << self.parameter) | remainder;
        Ok(zigzag_decode(unsigned))
    }
}

/// Rice decoder for FLAC residuals.
#[derive(Debug, Clone, Default)]
pub struct RiceDecoder {
    /// Partition order (log2 of partition count).
    pub partition_order: u8,
    /// Coding method (0 = 4-bit params, 1 = 5-bit params).
    pub coding_method: u8,
    /// Partitions.
    pub partitions: Vec<RicePartition>,
}

impl RiceDecoder {
    /// Create a new Rice decoder.
    #[must_use]
    pub fn new(partition_order: u8, coding_method: u8) -> Self {
        let partition_count = 1usize << partition_order;
        Self {
            partition_order,
            coding_method,
            partitions: Vec::with_capacity(partition_count),
        }
    }

    /// Get number of partitions.
    #[must_use]
    pub fn partition_count(&self) -> usize {
        1 << self.partition_order
    }

    /// Get parameter bits based on coding method.
    #[must_use]
    pub fn parameter_bits(&self) -> u8 {
        if self.coding_method == 0 {
            4
        } else {
            5
        }
    }

    /// Get escape code value based on coding method.
    #[must_use]
    pub fn escape_code(&self) -> u8 {
        if self.coding_method == 0 {
            0x0F
        } else {
            0x1F
        }
    }

    /// Get the number of residual samples in the **first** Rice partition.
    ///
    /// Per the FLAC spec, partition 0 is `predictor_order` samples shorter than
    /// every other partition, because the warmup samples occupy that space
    /// instead of being Rice-coded. Every later partition has exactly
    /// `block_size >> partition_order` samples with no such adjustment — this
    /// helper only covers the first partition's (shorter) size.
    #[must_use]
    pub fn samples_per_partition(&self, block_size: usize, predictor_order: usize) -> usize {
        let partition_count = self.partition_count();
        if partition_count == 0 {
            return 0;
        }
        (block_size >> self.partition_order).saturating_sub(predictor_order)
    }

    /// Decode all residuals.
    ///
    /// # Errors
    ///
    /// Returns error if decoding fails.
    pub fn decode(
        &mut self,
        data: &[u8],
        block_size: usize,
        predictor_order: usize,
    ) -> Result<Vec<i32>, AudioError> {
        if data.len() < 2 {
            return Err(AudioError::InvalidData("Rice data too short".into()));
        }

        let _partition_count = self.partition_count();
        let total_residuals = block_size - predictor_order;

        // Skeleton: actual implementation would bit-read parameters and values
        // For now, just return zeros
        let residuals = vec![0; total_residuals];

        Ok(residuals)
    }

    /// Decode residuals for one partition.
    #[allow(dead_code)]
    fn decode_partition(
        &self,
        bits: &mut dyn Iterator<Item = bool>,
        sample_count: usize,
        parameter: u8,
    ) -> Result<Vec<i32>, AudioError> {
        let mut residuals = Vec::with_capacity(sample_count);

        for _ in 0..sample_count {
            let value = self.decode_rice_value(bits, parameter)?;
            residuals.push(value);
        }

        Ok(residuals)
    }

    /// Decode a single Rice-coded value.
    #[allow(clippy::unused_self)]
    fn decode_rice_value(
        &self,
        bits: &mut dyn Iterator<Item = bool>,
        parameter: u8,
    ) -> Result<i32, AudioError> {
        // Count unary part: RFC 9639 section 9.2.7 — zero bits before a
        // terminating one bit.
        let mut quotient = 0u32;
        loop {
            match bits.next() {
                Some(false) => quotient += 1,
                Some(true) => break,
                None => return Err(AudioError::NeedMoreData),
            }
        }

        // Read binary part
        let mut remainder = 0u32;
        for _ in 0..parameter {
            match bits.next() {
                Some(bit) => {
                    remainder = (remainder << 1) | u32::from(bit);
                }
                None => return Err(AudioError::NeedMoreData),
            }
        }

        // Combine and zig-zag decode
        let unsigned = (quotient << parameter) | remainder;
        Ok(zigzag_decode(unsigned))
    }
}

/// Bit reader for Rice decoding.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BitReader<'a> {
    /// Data buffer.
    data: &'a [u8],
    /// Current byte position.
    byte_pos: usize,
    /// Current bit position (0-7).
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    /// Create a new bit reader.
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    /// Read a single bit.
    #[must_use]
    pub fn read_bit(&mut self) -> Option<bool> {
        if self.byte_pos >= self.data.len() {
            return None;
        }

        let bit = (self.data[self.byte_pos] >> (7 - self.bit_pos)) & 1;
        self.bit_pos += 1;
        if self.bit_pos >= 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Some(bit != 0)
    }

    /// Read multiple bits as unsigned integer.
    #[must_use]
    pub fn read_bits(&mut self, count: u8) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..count {
            let bit = self.read_bit()?;
            value = (value << 1) | u32::from(bit);
        }
        Some(value)
    }

    /// Read unary coded value (count of 0 bits before a terminating 1 bit).
    ///
    /// Per RFC 9639 section 9.2.7, FLAC's unary coding is zero bits
    /// terminated by a one bit.
    #[must_use]
    pub fn read_unary(&mut self) -> Option<u32> {
        let mut count = 0u32;
        loop {
            match self.read_bit() {
                Some(false) => count += 1,
                Some(true) => return Some(count),
                None => return None,
            }
        }
    }

    /// Get bits remaining (approximate).
    #[must_use]
    pub fn bits_remaining(&self) -> usize {
        let remaining_bytes = self.data.len().saturating_sub(self.byte_pos);
        remaining_bytes * 8 - self.bit_pos as usize
    }

    /// Check if at end of data.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.byte_pos >= self.data.len()
    }
}

impl Iterator for BitReader<'_> {
    type Item = bool;

    fn next(&mut self) -> Option<Self::Item> {
        self.read_bit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zigzag_encode() {
        assert_eq!(zigzag_encode(0), 0);
        assert_eq!(zigzag_encode(-1), 1);
        assert_eq!(zigzag_encode(1), 2);
        assert_eq!(zigzag_encode(-2), 3);
        assert_eq!(zigzag_encode(2), 4);
        assert_eq!(zigzag_encode(-3), 5);
    }

    #[test]
    fn test_zigzag_decode() {
        assert_eq!(zigzag_decode(0), 0);
        assert_eq!(zigzag_decode(1), -1);
        assert_eq!(zigzag_decode(2), 1);
        assert_eq!(zigzag_decode(3), -2);
        assert_eq!(zigzag_decode(4), 2);
        assert_eq!(zigzag_decode(5), -3);
    }

    #[test]
    fn test_zigzag_roundtrip() {
        for i in -1000..=1000 {
            assert_eq!(zigzag_decode(zigzag_encode(i)), i);
        }
    }

    #[test]
    fn test_rice_partition() {
        let partition = RicePartition::new(4, 100);
        assert_eq!(partition.parameter, 4);
        assert_eq!(partition.sample_count, 100);
    }

    #[test]
    fn test_rice_decoder() {
        let decoder = RiceDecoder::new(4, 0);
        assert_eq!(decoder.partition_count(), 16);
        assert_eq!(decoder.parameter_bits(), 4);
        assert_eq!(decoder.escape_code(), 0x0F);
    }

    #[test]
    fn test_rice_decoder_method1() {
        let decoder = RiceDecoder::new(2, 1);
        assert_eq!(decoder.partition_count(), 4);
        assert_eq!(decoder.parameter_bits(), 5);
        assert_eq!(decoder.escape_code(), 0x1F);
    }

    #[test]
    fn test_samples_per_partition() {
        let decoder = RiceDecoder::new(2, 0);
        // 4096 samples, predictor order 4, 4 partitions: first partition is
        // (4096 >> 2) - 4 = 1024 - 4 = 1020 samples (spec: shorter by the
        // predictor order; every later partition has the full 1024).
        assert_eq!(decoder.samples_per_partition(4096, 4), 1020);
    }

    #[test]
    fn test_bit_reader() {
        let data = vec![0b10110001, 0b01100011];
        let mut reader = BitReader::new(&data);

        assert_eq!(reader.read_bit(), Some(true));
        assert_eq!(reader.read_bit(), Some(false));
        assert_eq!(reader.read_bit(), Some(true));
        assert_eq!(reader.read_bit(), Some(true));
    }

    #[test]
    fn test_bit_reader_read_bits() {
        let data = vec![0b10110001];
        let mut reader = BitReader::new(&data);

        assert_eq!(reader.read_bits(4), Some(0b1011));
        assert_eq!(reader.read_bits(4), Some(0b0001));
    }

    #[test]
    fn test_bit_reader_read_unary() {
        let data = vec![0b0001_0100];
        let mut reader = BitReader::new(&data);

        assert_eq!(reader.read_unary(), Some(3)); // 000 followed by 1
        assert_eq!(reader.read_unary(), Some(1)); // 0 followed by 1
    }

    #[test]
    fn test_bit_reader_empty() {
        let data = vec![];
        let reader = BitReader::new(&data);
        assert!(reader.is_empty());
    }

    #[test]
    fn test_bit_reader_bits_remaining() {
        let data = vec![0xFF, 0xFF];
        let mut reader = BitReader::new(&data);
        assert_eq!(reader.bits_remaining(), 16);

        let _ = reader.read_bits(3);
        assert_eq!(reader.bits_remaining(), 13);
    }

    #[test]
    fn test_bit_reader_iterator() {
        let data = vec![0b10100000];
        let mut reader = BitReader::new(&data);

        let bits: Vec<bool> = reader.by_ref().take(4).collect();
        assert_eq!(bits, vec![true, false, true, false]);
    }
}
