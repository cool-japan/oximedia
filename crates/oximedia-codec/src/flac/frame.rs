//! FLAC frame header — code tables, serialisation and parsing.
//!
//! Implements RFC 9639 §9.1 exactly:
//!
//! ```text
//! <15>  frame sync  0b111111111111100
//! <1>   blocking strategy (0 = fixed block size, 1 = variable)
//! <4>   block size bits          (see BLOCK_SIZE_TABLE)
//! <4>   sample rate bits         (see SAMPLE_RATE_TABLE)
//! <4>   channel assignment       (see ChannelAssignment)
//! <3>   bit depth bits           (see BIT_DEPTH_TABLE)
//! <1>   reserved, must be 0
//! <?>   coded number (UTF-8-like, 1..7 bytes)
//! <?>   uncommon block size  (8 or 16 bits, stores size - 1)
//! <?>   uncommon sample rate (8 or 16 bits)
//! <8>   CRC-8 over every preceding header byte, including the sync code
//! ```

#![forbid(unsafe_code)]

use super::bitio::{crc8, BitReader};
use crate::error::{CodecError, CodecResult};

/// Largest block size representable in a FLAC frame header.
///
/// The uncommon 16-bit form stores `block_size - 1`, and the value `0xFFFF`
/// is forbidden by RFC 9639 §9.1.1, so 65535 samples is the ceiling.
pub const MAX_BLOCK_SIZE: u32 = 65535;

/// Common block sizes addressable without an uncommon-size field.
const BLOCK_SIZE_TABLE: [(u32, u8); 13] = [
    (192, 0b0001),
    (576, 0b0010),
    (1152, 0b0011),
    (2304, 0b0100),
    (4608, 0b0101),
    (256, 0b1000),
    (512, 0b1001),
    (1024, 0b1010),
    (2048, 0b1011),
    (4096, 0b1100),
    (8192, 0b1101),
    (16384, 0b1110),
    (32768, 0b1111),
];

/// Common sample rates addressable without an uncommon-rate field.
const SAMPLE_RATE_TABLE: [(u32, u8); 11] = [
    (88_200, 0b0001),
    (176_400, 0b0010),
    (192_000, 0b0011),
    (8_000, 0b0100),
    (16_000, 0b0101),
    (22_050, 0b0110),
    (24_000, 0b0111),
    (32_000, 0b1000),
    (44_100, 0b1001),
    (48_000, 0b1010),
    (96_000, 0b1011),
];

/// Bit depths addressable directly in the frame header.
const BIT_DEPTH_TABLE: [(u8, u8); 6] = [
    (8, 0b001),
    (12, 0b010),
    (16, 0b100),
    (20, 0b101),
    (24, 0b110),
    (32, 0b111),
];

// =============================================================================
// Channel assignment
// =============================================================================

/// Channel assignment / inter-channel decorrelation mode (RFC 9639 §9.1.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelAssignment {
    /// `n` independently coded channels (1..=8).
    Independent(u8),
    /// Stereo coded as `left`, `side = left - right`.
    LeftSide,
    /// Stereo coded as `side = left - right`, `right`.
    SideRight,
    /// Stereo coded as `mid = (left + right) >> 1`, `side = left - right`.
    MidSide,
}

impl ChannelAssignment {
    /// Number of coded subframes / decoded channels.
    #[must_use]
    pub fn channel_count(self) -> usize {
        match self {
            Self::Independent(n) => n as usize,
            Self::LeftSide | Self::SideRight | Self::MidSide => 2,
        }
    }

    /// The 4-bit channel-assignment code.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Self::Independent(n) => n.saturating_sub(1),
            Self::LeftSide => 0b1000,
            Self::SideRight => 0b1001,
            Self::MidSide => 0b1010,
        }
    }

    /// Decode a 4-bit channel-assignment code.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` for the reserved codes 0b1011..=0b1111.
    pub fn from_code(code: u8) -> CodecResult<Self> {
        match code {
            0..=7 => Ok(Self::Independent(code + 1)),
            0b1000 => Ok(Self::LeftSide),
            0b1001 => Ok(Self::SideRight),
            0b1010 => Ok(Self::MidSide),
            _ => Err(CodecError::InvalidData(format!(
                "FLAC: reserved channel assignment code {code:#06b}"
            ))),
        }
    }

    /// Extra bit-depth for subframe `index`.
    ///
    /// Side subframes carry one extra bit because a difference of two
    /// `n`-bit values needs `n + 1` bits (RFC 9639 §9.2).
    #[must_use]
    pub fn extra_bits(self, index: usize) -> u32 {
        match self {
            Self::Independent(_) => 0,
            Self::LeftSide | Self::MidSide => u32::from(index == 1),
            Self::SideRight => u32::from(index == 0),
        }
    }

    /// Undo inter-channel decorrelation in place.
    ///
    /// `channels` must hold exactly the coded subframes for this assignment.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` when the channel count or the
    /// per-channel lengths do not match.
    pub fn undo_decorrelation(self, channels: &mut [Vec<i32>]) -> CodecResult<()> {
        if matches!(self, Self::Independent(_)) {
            return Ok(());
        }
        if channels.len() != 2 {
            return Err(CodecError::InvalidData(
                "FLAC: stereo decorrelation needs exactly 2 subframes".to_string(),
            ));
        }
        let (first, rest) = channels.split_at_mut(1);
        let ch0 = &mut first[0];
        let ch1 = &mut rest[0];
        if ch0.len() != ch1.len() {
            return Err(CodecError::InvalidData(
                "FLAC: decorrelated subframes have differing lengths".to_string(),
            ));
        }

        match self {
            Self::LeftSide => {
                // ch0 = left, ch1 = side = left - right  =>  right = left - side
                for (l, s) in ch0.iter().zip(ch1.iter_mut()) {
                    *s = l.wrapping_sub(*s);
                }
            }
            Self::SideRight => {
                // ch0 = side = left - right, ch1 = right  =>  left = side + right
                for (s, r) in ch0.iter_mut().zip(ch1.iter()) {
                    *s = s.wrapping_add(*r);
                }
            }
            Self::MidSide => {
                // ch0 = mid = (left + right) >> 1, ch1 = side = left - right.
                // The low bit dropped by the mid's shift is recoverable from
                // the parity of `side`, since left + right and left - right
                // always share the same parity.
                for (m, s) in ch0.iter_mut().zip(ch1.iter_mut()) {
                    let side = i64::from(*s);
                    let mid = (i64::from(*m) << 1) | (side & 1);
                    *m = ((mid + side) >> 1) as i32;
                    *s = ((mid - side) >> 1) as i32;
                }
            }
            // Independent streams need no inverse; handled above.
            Self::Independent(_) => {}
        }
        Ok(())
    }

    /// Apply inter-channel decorrelation to `left` / `right`.
    ///
    /// Returns the two subframe signals in coded order.
    #[must_use]
    pub fn apply_decorrelation(self, left: &[i32], right: &[i32]) -> (Vec<i32>, Vec<i32>) {
        match self {
            Self::LeftSide => (
                left.to_vec(),
                left.iter()
                    .zip(right)
                    .map(|(&l, &r)| l.wrapping_sub(r))
                    .collect(),
            ),
            Self::SideRight => (
                left.iter()
                    .zip(right)
                    .map(|(&l, &r)| l.wrapping_sub(r))
                    .collect(),
                right.to_vec(),
            ),
            Self::MidSide => (
                left.iter()
                    .zip(right)
                    .map(|(&l, &r)| ((i64::from(l) + i64::from(r)) >> 1) as i32)
                    .collect(),
                left.iter()
                    .zip(right)
                    .map(|(&l, &r)| l.wrapping_sub(r))
                    .collect(),
            ),
            Self::Independent(_) => (left.to_vec(), right.to_vec()),
        }
    }
}

// =============================================================================
// UTF-8-like coded number
// =============================================================================

/// Serialise a FLAC "coded number" (UTF-8-like, up to 36 bits / 7 bytes).
///
/// # Errors
///
/// Returns `CodecError::InvalidParameter` when `value` needs more than 36 bits.
pub fn write_coded_number(out: &mut Vec<u8>, value: u64) -> CodecResult<()> {
    // (bit budget, byte count, first-byte prefix)
    const FORMS: [(u32, usize, u8); 7] = [
        (7, 1, 0x00),
        (11, 2, 0xC0),
        (16, 3, 0xE0),
        (21, 4, 0xF0),
        (26, 5, 0xF8),
        (31, 6, 0xFC),
        (36, 7, 0xFE),
    ];
    for &(bits, len, prefix) in &FORMS {
        if value < (1u64 << bits) {
            let payload_bits = 6 * (len as u32 - 1);
            let head_bits = bits - payload_bits;
            let head = (value >> payload_bits) as u8;
            debug_assert!(u32::from(head) < (1u32 << head_bits));
            out.push(prefix | head);
            for i in (0..len - 1).rev() {
                out.push(0x80 | ((value >> (6 * i)) as u8 & 0x3F));
            }
            return Ok(());
        }
    }
    Err(CodecError::InvalidParameter(format!(
        "FLAC coded number {value} exceeds the 36-bit maximum"
    )))
}

/// Parse a FLAC "coded number" from a byte-aligned reader.
///
/// # Errors
///
/// Returns `CodecError::InvalidData` on truncated or malformed input.
pub fn read_coded_number(r: &mut BitReader<'_>) -> CodecResult<u64> {
    let b0 = r
        .read_aligned_byte()
        .ok_or_else(|| CodecError::InvalidData("FLAC: EOF in coded number".to_string()))?;
    let extra = match b0 {
        0x00..=0x7F => 0usize,
        0xC0..=0xDF => 1,
        0xE0..=0xEF => 2,
        0xF0..=0xF7 => 3,
        0xF8..=0xFB => 4,
        0xFC..=0xFD => 5,
        0xFE => 6,
        _ => {
            return Err(CodecError::InvalidData(format!(
                "FLAC: invalid coded-number lead byte {b0:#04x}"
            )))
        }
    };
    let mut value = if extra == 0 {
        u64::from(b0)
    } else {
        u64::from(b0 & (0x7F >> extra))
    };
    for _ in 0..extra {
        let cb = r.read_aligned_byte().ok_or_else(|| {
            CodecError::InvalidData("FLAC: EOF in coded-number continuation".to_string())
        })?;
        if cb & 0xC0 != 0x80 {
            return Err(CodecError::InvalidData(format!(
                "FLAC: invalid coded-number continuation byte {cb:#04x}"
            )));
        }
        value = (value << 6) | u64::from(cb & 0x3F);
    }
    Ok(value)
}

// =============================================================================
// Frame header
// =============================================================================

/// A parsed / to-be-written FLAC frame header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// `false` = fixed block size (coded number is a frame number),
    /// `true` = variable block size (coded number is a sample number).
    pub variable_block_size: bool,
    /// Number of samples per channel in this frame.
    pub block_size: u32,
    /// Sample rate in Hz, or `None` when the frame defers to STREAMINFO.
    pub sample_rate: Option<u32>,
    /// Channel assignment / decorrelation mode.
    pub channel_assignment: ChannelAssignment,
    /// Bit depth, or `None` when the frame defers to STREAMINFO.
    pub bits_per_sample: Option<u8>,
    /// Frame number (fixed block size) or first sample number (variable).
    pub coded_number: u64,
}

impl FrameHeader {
    /// Serialise the header, including its trailing CRC-8.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidParameter` for block sizes, sample rates or
    /// bit depths that cannot be represented in a FLAC frame header.
    pub fn to_bytes(&self) -> CodecResult<Vec<u8>> {
        if self.block_size == 0 || self.block_size > MAX_BLOCK_SIZE {
            return Err(CodecError::InvalidParameter(format!(
                "FLAC block size {} out of range 1..={MAX_BLOCK_SIZE}",
                self.block_size
            )));
        }

        let (bs_code, bs_extra) = Self::encode_block_size(self.block_size);
        let (sr_code, sr_extra) = Self::encode_sample_rate(self.sample_rate)?;
        let bps_code = Self::encode_bit_depth(self.bits_per_sample)?;

        let mut out = Vec::with_capacity(16);
        out.push(0xFF);
        out.push(0xF8 | u8::from(self.variable_block_size));
        out.push((bs_code << 4) | sr_code);
        out.push((self.channel_assignment.code() << 4) | (bps_code << 1));
        write_coded_number(&mut out, self.coded_number)?;
        out.extend_from_slice(&bs_extra);
        out.extend_from_slice(&sr_extra);
        let crc = crc8(&out);
        out.push(crc);
        Ok(out)
    }

    /// Parse a frame header from `r`, which must be byte-aligned.
    ///
    /// Verifies the header CRC-8 and returns the parsed header.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` for a bad sync code, a reserved field
    /// value, a truncated header or a CRC-8 mismatch.
    pub fn parse(r: &mut BitReader<'_>) -> CodecResult<Self> {
        if !r.is_byte_aligned() {
            return Err(CodecError::InvalidData(
                "FLAC: frame header must start byte-aligned".to_string(),
            ));
        }
        let start = r.byte_pos();

        let b0 = r
            .read_aligned_byte()
            .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading frame sync".to_string()))?;
        let b1 = r
            .read_aligned_byte()
            .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading frame sync".to_string()))?;
        if b0 != 0xFF || (b1 & 0xFE) != 0xF8 {
            return Err(CodecError::InvalidData(format!(
                "FLAC: invalid frame sync {b0:#04x} {b1:#04x}"
            )));
        }
        let variable_block_size = b1 & 1 != 0;

        let b2 = r
            .read_aligned_byte()
            .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading frame header".to_string()))?;
        let b3 = r
            .read_aligned_byte()
            .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading frame header".to_string()))?;
        let bs_code = b2 >> 4;
        let sr_code = b2 & 0x0F;
        let ch_code = b3 >> 4;
        let bps_code = (b3 >> 1) & 0x07;
        if b3 & 1 != 0 {
            return Err(CodecError::InvalidData(
                "FLAC: reserved frame-header bit is set".to_string(),
            ));
        }
        if bs_code == 0 {
            return Err(CodecError::InvalidData(
                "FLAC: reserved block size code 0".to_string(),
            ));
        }
        if sr_code == 0x0F {
            return Err(CodecError::InvalidData(
                "FLAC: forbidden sample rate code 0b1111".to_string(),
            ));
        }
        if bps_code == 0b011 {
            return Err(CodecError::InvalidData(
                "FLAC: reserved bit depth code 0b011".to_string(),
            ));
        }
        let channel_assignment = ChannelAssignment::from_code(ch_code)?;

        let coded_number = read_coded_number(r)?;

        let block_size = match bs_code {
            0b0110 => {
                let v = r.read_aligned_byte().ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading uncommon block size".to_string())
                })?;
                u32::from(v) + 1
            }
            0b0111 => {
                let hi = r.read_aligned_byte().ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading uncommon block size".to_string())
                })?;
                let lo = r.read_aligned_byte().ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading uncommon block size".to_string())
                })?;
                let raw = (u32::from(hi) << 8) | u32::from(lo);
                if raw == 0xFFFF {
                    return Err(CodecError::InvalidData(
                        "FLAC: forbidden uncommon block size 0xFFFF".to_string(),
                    ));
                }
                raw + 1
            }
            code => BLOCK_SIZE_TABLE
                .iter()
                .find(|&&(_, c)| c == code)
                .map(|&(size, _)| size)
                .ok_or_else(|| {
                    CodecError::InvalidData(format!("FLAC: unusable block size code {code:#06b}"))
                })?,
        };

        let sample_rate = match sr_code {
            0 => None,
            0b1100 => {
                let v = r.read_aligned_byte().ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading uncommon sample rate".to_string())
                })?;
                Some(u32::from(v) * 1000)
            }
            0b1101 | 0b1110 => {
                let hi = r.read_aligned_byte().ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading uncommon sample rate".to_string())
                })?;
                let lo = r.read_aligned_byte().ok_or_else(|| {
                    CodecError::InvalidData("FLAC: EOF reading uncommon sample rate".to_string())
                })?;
                let raw = (u32::from(hi) << 8) | u32::from(lo);
                Some(if sr_code == 0b1101 { raw } else { raw * 10 })
            }
            code => Some(
                SAMPLE_RATE_TABLE
                    .iter()
                    .find(|&&(_, c)| c == code)
                    .map(|&(rate, _)| rate)
                    .ok_or_else(|| {
                        CodecError::InvalidData(format!(
                            "FLAC: unusable sample rate code {code:#06b}"
                        ))
                    })?,
            ),
        };

        let bits_per_sample = if bps_code == 0 {
            None
        } else {
            Some(
                BIT_DEPTH_TABLE
                    .iter()
                    .find(|&&(_, c)| c == bps_code)
                    .map(|&(depth, _)| depth)
                    .ok_or_else(|| {
                        CodecError::InvalidData(format!(
                            "FLAC: unusable bit depth code {bps_code:#05b}"
                        ))
                    })?,
            )
        };

        let end = r.byte_pos();
        let stored_crc = r
            .read_aligned_byte()
            .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading header CRC-8".to_string()))?;
        let computed = crc8(&r.data()[start..end]);
        if stored_crc != computed {
            return Err(CodecError::InvalidData(format!(
                "FLAC: frame header CRC-8 mismatch (stored {stored_crc:#04x}, computed {computed:#04x})"
            )));
        }

        Ok(Self {
            variable_block_size,
            block_size,
            sample_rate,
            channel_assignment,
            bits_per_sample,
            coded_number,
        })
    }

    fn encode_block_size(block_size: u32) -> (u8, Vec<u8>) {
        if let Some(&(_, code)) = BLOCK_SIZE_TABLE.iter().find(|&&(s, _)| s == block_size) {
            return (code, Vec::new());
        }
        let stored = block_size - 1;
        if stored <= 0xFF {
            (0b0110, vec![stored as u8])
        } else {
            (0b0111, vec![(stored >> 8) as u8, stored as u8])
        }
    }

    fn encode_sample_rate(sample_rate: Option<u32>) -> CodecResult<(u8, Vec<u8>)> {
        let Some(rate) = sample_rate else {
            return Ok((0, Vec::new()));
        };
        if rate == 0 {
            return Err(CodecError::InvalidParameter(
                "FLAC sample rate must be non-zero".to_string(),
            ));
        }
        if let Some(&(_, code)) = SAMPLE_RATE_TABLE.iter().find(|&&(r, _)| r == rate) {
            return Ok((code, Vec::new()));
        }
        if rate % 1000 == 0 && rate / 1000 <= 0xFF {
            return Ok((0b1100, vec![(rate / 1000) as u8]));
        }
        if rate <= 0xFFFF {
            return Ok((0b1101, vec![(rate >> 8) as u8, rate as u8]));
        }
        if rate % 10 == 0 && rate / 10 <= 0xFFFF {
            let v = rate / 10;
            return Ok((0b1110, vec![(v >> 8) as u8, v as u8]));
        }
        // Not representable in the frame header — defer to STREAMINFO.
        Ok((0, Vec::new()))
    }

    fn encode_bit_depth(bits_per_sample: Option<u8>) -> CodecResult<u8> {
        let Some(depth) = bits_per_sample else {
            return Ok(0);
        };
        Ok(BIT_DEPTH_TABLE
            .iter()
            .find(|&&(d, _)| d == depth)
            .map_or(0, |&(_, code)| code))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn header(block_size: u32, rate: Option<u32>, bps: Option<u8>, n: u64) -> FrameHeader {
        FrameHeader {
            variable_block_size: false,
            block_size,
            sample_rate: rate,
            channel_assignment: ChannelAssignment::Independent(2),
            bits_per_sample: bps,
            coded_number: n,
        }
    }

    fn round_trip(h: &FrameHeader) -> FrameHeader {
        let bytes = h.to_bytes().expect("serialise header");
        let mut r = BitReader::new(&bytes);
        let parsed = FrameHeader::parse(&mut r).expect("parse header");
        assert_eq!(r.byte_pos(), bytes.len(), "parser must consume the header");
        parsed
    }

    #[test]
    fn frame_header_round_trips_common_values() {
        for &bs in &[
            192u32, 576, 1152, 2304, 4608, 256, 512, 1024, 2048, 4096, 8192,
        ] {
            let h = header(bs, Some(44_100), Some(16), 7);
            assert_eq!(round_trip(&h), h, "block size {bs}");
        }
    }

    #[test]
    fn frame_header_round_trips_uncommon_block_sizes() {
        for &bs in &[1u32, 2, 17, 128, 255, 256 + 1, 4095, 4097, 65535] {
            let h = header(bs, Some(44_100), Some(16), 3);
            assert_eq!(round_trip(&h), h, "block size {bs}");
        }
    }

    #[test]
    fn frame_header_round_trips_sample_rates() {
        for rate in [
            None,
            Some(8_000),
            Some(16_000),
            Some(22_050),
            Some(24_000),
            Some(32_000),
            Some(44_100),
            Some(48_000),
            Some(88_200),
            Some(96_000),
            Some(176_400),
            Some(192_000),
            Some(11_000),  // uncommon 8-bit kHz form
            Some(37_800),  // uncommon 16-bit Hz form
            Some(655_350), // uncommon 16-bit Hz/10 form
        ] {
            let h = header(1024, rate, Some(16), 1);
            assert_eq!(round_trip(&h), h, "sample rate {rate:?}");
        }
    }

    #[test]
    fn non_representable_sample_rate_defers_to_streaminfo() {
        // 768000 Hz: not in the table, > 65535 Hz, and 76800 exceeds the
        // Hz/10 field, so the frame must defer to STREAMINFO.
        let parsed = round_trip(&header(1024, Some(768_000), Some(16), 1));
        assert_eq!(parsed.sample_rate, None);
    }

    #[test]
    fn frame_header_round_trips_bit_depths() {
        for depth in [
            None,
            Some(8),
            Some(12),
            Some(16),
            Some(20),
            Some(24),
            Some(32),
        ] {
            let h = header(1024, Some(44_100), depth, 1);
            assert_eq!(round_trip(&h), h, "bit depth {depth:?}");
        }
    }

    #[test]
    fn non_representable_bit_depth_defers_to_streaminfo() {
        let h = header(1024, Some(44_100), Some(14), 1);
        let parsed = round_trip(&header(1024, Some(44_100), Some(14), 1));
        assert_eq!(parsed.bits_per_sample, None, "14-bit must defer");
        assert_eq!(parsed.block_size, h.block_size);
    }

    #[test]
    fn frame_header_round_trips_channel_assignments() {
        for ca in [
            ChannelAssignment::Independent(1),
            ChannelAssignment::Independent(2),
            ChannelAssignment::Independent(8),
            ChannelAssignment::LeftSide,
            ChannelAssignment::SideRight,
            ChannelAssignment::MidSide,
        ] {
            let mut h = header(1024, Some(44_100), Some(16), 5);
            h.channel_assignment = ca;
            assert_eq!(round_trip(&h), h, "assignment {ca:?}");
        }
    }

    #[test]
    fn frame_header_round_trips_coded_numbers() {
        for n in [
            0u64,
            1,
            0x7F,
            0x80,
            0x7FF,
            0x800,
            0xFFFF,
            0x1_0000,
            0x1F_FFFF,
            0x20_0000,
            0x3FF_FFFF,
            0x400_0000,
            0x7FFF_FFFF,
            0x8000_0000,
            0xF_FFFF_FFFF,
        ] {
            let mut h = header(1024, Some(44_100), Some(16), n);
            h.variable_block_size = true;
            assert_eq!(round_trip(&h), h, "coded number {n}");
        }
    }

    #[test]
    fn coded_number_uses_minimal_length() {
        let mut out = Vec::new();
        write_coded_number(&mut out, 0).expect("write");
        assert_eq!(out, vec![0x00]);
        out.clear();
        write_coded_number(&mut out, 127).expect("write");
        assert_eq!(out, vec![0x7F]);
        out.clear();
        write_coded_number(&mut out, 128).expect("write");
        assert_eq!(out, vec![0xC2, 0x80]);
        out.clear();
        write_coded_number(&mut out, 0x1F_FFFF).expect("write");
        assert_eq!(out, vec![0xF7, 0xBF, 0xBF, 0xBF]);
    }

    #[test]
    fn coded_number_rejects_overlarge_values() {
        let mut out = Vec::new();
        assert!(write_coded_number(&mut out, 1u64 << 36).is_err());
    }

    #[test]
    fn parse_rejects_bad_sync() {
        let bytes = [0xFEu8, 0xF8, 0xC9, 0x08, 0x00, 0x00];
        let mut r = BitReader::new(&bytes);
        assert!(FrameHeader::parse(&mut r).is_err());
    }

    #[test]
    fn parse_rejects_crc_mismatch() {
        let h = header(1024, Some(44_100), Some(16), 2);
        let mut bytes = h.to_bytes().expect("serialise");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        let mut r = BitReader::new(&bytes);
        let err = FrameHeader::parse(&mut r).expect_err("must reject bad CRC-8");
        assert!(format!("{err}").contains("CRC-8"), "got: {err}");
    }

    #[test]
    fn parse_rejects_reserved_fields() {
        let h = header(1024, Some(44_100), Some(16), 2);
        // Reserved trailing bit of byte 3.
        let mut bytes = h.to_bytes().expect("serialise");
        bytes[3] |= 1;
        let last = bytes.len() - 1;
        bytes[last] = crc8(&bytes[..last]);
        let mut r = BitReader::new(&bytes);
        assert!(FrameHeader::parse(&mut r).is_err());

        // Reserved block size code 0.
        let mut bytes = h.to_bytes().expect("serialise");
        bytes[2] &= 0x0F;
        let last = bytes.len() - 1;
        bytes[last] = crc8(&bytes[..last]);
        let mut r = BitReader::new(&bytes);
        assert!(FrameHeader::parse(&mut r).is_err());
    }

    #[test]
    fn channel_assignment_codes_match_rfc() {
        assert_eq!(ChannelAssignment::Independent(1).code(), 0b0000);
        assert_eq!(ChannelAssignment::Independent(8).code(), 0b0111);
        assert_eq!(ChannelAssignment::LeftSide.code(), 0b1000);
        assert_eq!(ChannelAssignment::SideRight.code(), 0b1001);
        assert_eq!(ChannelAssignment::MidSide.code(), 0b1010);
        for code in 0b1011..=0b1111u8 {
            assert!(
                ChannelAssignment::from_code(code).is_err(),
                "code {code:#b}"
            );
        }
    }

    #[test]
    fn side_channel_gets_one_extra_bit() {
        assert_eq!(ChannelAssignment::LeftSide.extra_bits(0), 0);
        assert_eq!(ChannelAssignment::LeftSide.extra_bits(1), 1);
        assert_eq!(ChannelAssignment::SideRight.extra_bits(0), 1);
        assert_eq!(ChannelAssignment::SideRight.extra_bits(1), 0);
        assert_eq!(ChannelAssignment::MidSide.extra_bits(0), 0);
        assert_eq!(ChannelAssignment::MidSide.extra_bits(1), 1);
        assert_eq!(ChannelAssignment::Independent(2).extra_bits(1), 0);
    }

    #[test]
    fn stereo_decorrelation_is_exactly_invertible() {
        let left: Vec<i32> = (0..64).map(|i| (i * 977) % 30011 - 15000).collect();
        let right: Vec<i32> = (0..64).map(|i| (i * 613) % 30011 - 15000).collect();
        for ca in [
            ChannelAssignment::LeftSide,
            ChannelAssignment::SideRight,
            ChannelAssignment::MidSide,
        ] {
            let (a, b) = ca.apply_decorrelation(&left, &right);
            let mut chans = vec![a, b];
            ca.undo_decorrelation(&mut chans).expect("undo");
            assert_eq!(chans[0], left, "{ca:?} left");
            assert_eq!(chans[1], right, "{ca:?} right");
        }
    }

    #[test]
    fn stereo_decorrelation_handles_odd_sums() {
        // Mid/side must survive odd (l + r), which is where the low bit of
        // `side` carries the information lost by the mid's right shift.
        let left = vec![1i32, -1, 5, -6, 32767, -32768];
        let right = vec![0i32, 0, -4, 7, -32768, 32767];
        let ca = ChannelAssignment::MidSide;
        let (a, b) = ca.apply_decorrelation(&left, &right);
        let mut chans = vec![a, b];
        ca.undo_decorrelation(&mut chans).expect("undo");
        assert_eq!(chans[0], left);
        assert_eq!(chans[1], right);
    }
}
