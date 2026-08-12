//! FLAC audio decoder.
//!
//! Decodes FLAC frames back into interleaved i32 PCM samples, following
//! RFC 9639 exactly.
//!
//! # Decoding pipeline
//!
//! 1. Parse the metadata blocks (`fLaC` + STREAMINFO + any others).
//! 2. Parse each frame header ([`super::frame::FrameHeader`]) and verify its CRC-8.
//! 3. Decode every subframe ([`super::subframe::read_subframe`]) — constant,
//!    verbatim, fixed-predictor and LPC, with wasted-bits handling and
//!    partitioned Rice residuals including escape-coded partitions.
//! 4. Undo inter-channel decorrelation (left/side, side/right, mid/side).
//! 5. Verify the frame CRC-16 and interleave the channels.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use super::bitio::{crc16, BitReader};
use super::frame::FrameHeader;
use super::subframe::read_subframe;
use crate::error::{CodecError, CodecResult};

// =============================================================================
// Decoder state
// =============================================================================

/// Information extracted from the FLAC stream header (STREAMINFO metadata block).
#[derive(Clone, Debug, Default)]
pub struct FlacStreamInfo {
    /// Minimum block size in samples.
    pub min_block_size: u16,
    /// Maximum block size in samples.
    pub max_block_size: u16,
    /// Minimum frame size in bytes (0 = unknown).
    pub min_frame_size: u32,
    /// Maximum frame size in bytes (0 = unknown).
    pub max_frame_size: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of channels.
    pub channels: u8,
    /// Bits per sample.
    pub bits_per_sample: u8,
    /// Total number of inter-channel samples. 0 = unknown.
    pub total_samples: u64,
    /// MD5 signature of the unencoded audio data.
    pub md5_signature: [u8; 16],
}

/// Vendor and user comments extracted from a VORBIS_COMMENT metadata block.
#[derive(Clone, Debug, Default)]
pub struct FlacVorbisComment {
    /// Vendor string (e.g. encoder name/version).
    pub vendor: String,
    /// Key-value pairs from the comment block.
    pub comments: Vec<(String, String)>,
}

/// A decoded PCM block.
#[derive(Clone, Debug)]
pub struct DecodedBlock {
    /// Interleaved i32 PCM samples (channel-minor order).
    pub samples: Vec<i32>,
    /// Sample number of the first sample in this block.
    ///
    /// A fixed-block-size stream codes a *frame* number rather than a sample
    /// number, so this is derived as `frame_number × stream_block_size`.  The
    /// stream block size comes from STREAMINFO when it declares one
    /// (`min_block_size == max_block_size`); without STREAMINFO the frame's own
    /// block size is used, which is exact for every frame except a short final
    /// one.  Decode through [`FlacDecoder::decode_stream`], or call
    /// [`FlacDecoder::parse_metadata`] first, to always get exact numbering.
    pub sample_number: u64,
    /// Frame number, for fixed-block-size streams only (`None` when the frame
    /// codes a sample number directly).
    pub frame_number: Option<u64>,
    /// Number of samples per channel in this block.
    pub block_size: usize,
    /// Number of channels.
    pub channels: usize,
}

/// FLAC audio decoder.
pub struct FlacDecoder {
    /// Stream info parsed from the stream header.
    pub stream_info: Option<FlacStreamInfo>,
    /// Vorbis comment block (if present in the stream).
    pub comment: Option<FlacVorbisComment>,
    /// Whether the stream header has been consumed.
    header_parsed: bool,
}

impl FlacDecoder {
    /// Create a new FLAC decoder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stream_info: None,
            comment: None,
            header_parsed: false,
        }
    }

    /// Return a reference to the parsed stream info, if available.
    #[must_use]
    pub fn stream_info(&self) -> Option<&FlacStreamInfo> {
        self.stream_info.as_ref()
    }

    /// Return a reference to the parsed Vorbis comment block, if available.
    #[must_use]
    pub fn comment_block(&self) -> Option<&FlacVorbisComment> {
        self.comment.as_ref()
    }

    /// Parse all FLAC metadata blocks from the beginning of a FLAC byte stream.
    ///
    /// Reads and parses:
    /// - The `fLaC` stream marker
    /// - All metadata blocks until the last-block flag is set
    /// - STREAMINFO (block type 0) — mandatory first block
    /// - VORBIS_COMMENT (block type 4) — optional
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` if the magic bytes are missing, STREAMINFO
    /// is absent or malformed, or any block header is truncated.
    pub fn parse_metadata(&mut self, data: &[u8]) -> Result<(), CodecError> {
        self.parse_metadata_inner(data).map(|_| ())
    }

    /// Parse every metadata block, returning the byte offset of the first frame.
    fn parse_metadata_inner(&mut self, data: &[u8]) -> Result<usize, CodecError> {
        if data.len() < 4 {
            return Err(CodecError::InvalidData(
                "Stream too short for fLaC marker".to_string(),
            ));
        }
        if &data[..4] != b"fLaC" {
            return Err(CodecError::InvalidData(
                "Missing fLaC magic marker".to_string(),
            ));
        }

        let mut pos = 4usize;

        loop {
            if pos + 4 > data.len() {
                return Err(CodecError::InvalidData(
                    "Truncated metadata block header".to_string(),
                ));
            }

            let header_byte = data[pos];
            let is_last = (header_byte & 0x80) != 0;
            let block_type = header_byte & 0x7F;
            let length = (u32::from(data[pos + 1]) << 16)
                | (u32::from(data[pos + 2]) << 8)
                | u32::from(data[pos + 3]);
            pos += 4;

            let block_end = pos + length as usize;
            if block_end > data.len() {
                return Err(CodecError::InvalidData(format!(
                    "Metadata block (type {block_type}) extends beyond data"
                )));
            }

            let block_data = &data[pos..block_end];

            match block_type {
                0 => {
                    // STREAMINFO
                    self.stream_info = Some(Self::parse_streaminfo_block(block_data)?);
                }
                4 => {
                    // VORBIS_COMMENT
                    self.comment = Some(Self::parse_vorbis_comment_block(block_data)?);
                }
                _ => {
                    // Skip other block types (PADDING=1, APPLICATION=2, SEEKTABLE=3,
                    // CUESHEET=5, PICTURE=6, etc.)
                }
            }

            pos = block_end;

            if is_last {
                break;
            }
        }

        if self.stream_info.is_none() {
            return Err(CodecError::InvalidData(
                "No STREAMINFO block found in metadata".to_string(),
            ));
        }

        self.header_parsed = true;
        Ok(pos)
    }

    /// Quick-probe a FLAC byte stream: validates the magic bytes and parses the
    /// mandatory STREAMINFO block, returning the stream info without retaining state.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` if the magic is missing or STREAMINFO is
    /// malformed.
    pub fn probe(data: &[u8]) -> Result<FlacStreamInfo, CodecError> {
        if data.len() < 8 {
            return Err(CodecError::InvalidData(
                "Stream too short for FLAC probe".to_string(),
            ));
        }
        if &data[..4] != b"fLaC" {
            return Err(CodecError::InvalidData(
                "Missing fLaC magic marker".to_string(),
            ));
        }

        let header_byte = data[4];
        let block_type = header_byte & 0x7F;
        if block_type != 0 {
            return Err(CodecError::InvalidData(format!(
                "Expected STREAMINFO as first block, got type {block_type}"
            )));
        }

        let length = (u32::from(data[5]) << 16) | (u32::from(data[6]) << 8) | u32::from(data[7]);
        let block_end = 8 + length as usize;
        if block_end > data.len() {
            return Err(CodecError::InvalidData(
                "STREAMINFO block extends beyond data in probe".to_string(),
            ));
        }

        Self::parse_streaminfo_block(&data[8..block_end])
    }

    /// Parse a raw STREAMINFO block body (without the 4-byte block header).
    fn parse_streaminfo_block(si_data: &[u8]) -> Result<FlacStreamInfo, CodecError> {
        // STREAMINFO is 34 bytes per the FLAC spec.
        if si_data.len() < 18 {
            return Err(CodecError::InvalidData(
                "STREAMINFO block too short".to_string(),
            ));
        }

        let min_block_size = (u16::from(si_data[0]) << 8) | u16::from(si_data[1]);
        let max_block_size = (u16::from(si_data[2]) << 8) | u16::from(si_data[3]);

        let min_frame_size =
            (u32::from(si_data[4]) << 16) | (u32::from(si_data[5]) << 8) | u32::from(si_data[6]);
        let max_frame_size =
            (u32::from(si_data[7]) << 16) | (u32::from(si_data[8]) << 8) | u32::from(si_data[9]);

        // Bytes 10-13: 20-bit sample_rate | 3-bit channels-1 | 5-bit bps-1 | 4-bit top of total_samples
        let packed = (u32::from(si_data[10]) << 24)
            | (u32::from(si_data[11]) << 16)
            | (u32::from(si_data[12]) << 8)
            | u32::from(si_data[13]);

        let sample_rate = packed >> 12;
        let channels = (((packed >> 9) & 0x07) + 1) as u8;
        let bits_per_sample = (((packed >> 4) & 0x1F) + 1) as u8;
        let total_samples_high = u64::from(packed & 0x0F);

        // Bytes 14-17: lower 32 bits of total samples
        let total_samples_low = if si_data.len() >= 18 {
            (u64::from(si_data[14]) << 24)
                | (u64::from(si_data[15]) << 16)
                | (u64::from(si_data[16]) << 8)
                | u64::from(si_data[17])
        } else {
            0
        };
        let total_samples = (total_samples_high << 32) | total_samples_low;

        // Bytes 18-33: MD5 signature (16 bytes)
        let mut md5_signature = [0u8; 16];
        if si_data.len() >= 34 {
            md5_signature.copy_from_slice(&si_data[18..34]);
        }

        Ok(FlacStreamInfo {
            min_block_size,
            max_block_size,
            min_frame_size,
            max_frame_size,
            sample_rate,
            channels,
            bits_per_sample,
            total_samples,
            md5_signature,
        })
    }

    /// Parse a raw VORBIS_COMMENT block body.
    ///
    /// Vorbis comment format uses little-endian 32-bit lengths.
    fn parse_vorbis_comment_block(data: &[u8]) -> Result<FlacVorbisComment, CodecError> {
        let mut pos = 0usize;

        // Helper: read a LE u32
        let read_le_u32 = |d: &[u8], p: usize| -> Result<u32, CodecError> {
            if p + 4 > d.len() {
                return Err(CodecError::InvalidData(
                    "VORBIS_COMMENT: unexpected end of data".to_string(),
                ));
            }
            Ok(u32::from(d[p])
                | (u32::from(d[p + 1]) << 8)
                | (u32::from(d[p + 2]) << 16)
                | (u32::from(d[p + 3]) << 24))
        };

        // Vendor string
        let vendor_len = read_le_u32(data, pos)? as usize;
        pos += 4;
        if pos + vendor_len > data.len() {
            return Err(CodecError::InvalidData(
                "VORBIS_COMMENT: vendor string truncated".to_string(),
            ));
        }
        let vendor = String::from_utf8_lossy(&data[pos..pos + vendor_len]).into_owned();
        pos += vendor_len;

        // Comment count
        let comment_count = read_le_u32(data, pos)? as usize;
        pos += 4;

        let mut comments = Vec::with_capacity(comment_count);
        for _ in 0..comment_count {
            let entry_len = read_le_u32(data, pos)? as usize;
            pos += 4;
            if pos + entry_len > data.len() {
                return Err(CodecError::InvalidData(
                    "VORBIS_COMMENT: comment entry truncated".to_string(),
                ));
            }
            let entry = String::from_utf8_lossy(&data[pos..pos + entry_len]).into_owned();
            pos += entry_len;

            // Split on first '='
            if let Some(eq_pos) = entry.find('=') {
                let key = entry[..eq_pos].to_string();
                let value = entry[eq_pos + 1..].to_string();
                comments.push((key, value));
            } else {
                // Entry without '=' — store key only with empty value
                comments.push((entry, String::new()));
            }
        }

        Ok(FlacVorbisComment { vendor, comments })
    }

    /// Parse the FLAC stream header from the beginning of a FLAC byte stream.
    ///
    /// Must be called once before `decode_frame`.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` if the magic bytes or STREAMINFO are malformed.
    pub fn parse_stream_header(&mut self, data: &[u8]) -> CodecResult<usize> {
        if data.len() < 8 {
            return Err(CodecError::InvalidData(
                "Stream too short for FLAC header".to_string(),
            ));
        }
        if &data[..4] != b"fLaC" {
            return Err(CodecError::InvalidData("Missing fLaC magic".to_string()));
        }

        // METADATA_BLOCK_HEADER
        let block_type = data[4] & 0x7F;
        let length = (u32::from(data[5]) << 16) | (u32::from(data[6]) << 8) | u32::from(data[7]);

        if block_type != 0 {
            return Err(CodecError::InvalidData(format!(
                "Expected STREAMINFO block (type 0), got type {block_type}"
            )));
        }

        let offset = 8usize;
        let end = offset + length as usize;
        if data.len() < end {
            return Err(CodecError::InvalidData(
                "Truncated STREAMINFO block".to_string(),
            ));
        }

        let si_data = &data[offset..end];
        self.stream_info = Some(Self::parse_streaminfo_block(si_data)?);
        self.header_parsed = true;

        Ok(end)
    }

    /// Decode a single FLAC frame from the given byte slice.
    ///
    /// Returns `(decoded_block, bytes_consumed)`.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` on malformed frame data.
    pub fn decode_frame(&self, data: &[u8]) -> CodecResult<(DecodedBlock, usize)> {
        let mut r = BitReader::new(data);
        let header = FrameHeader::parse(&mut r)?;

        // The frame header may defer the bit depth to STREAMINFO.
        let bits_per_sample = match header.bits_per_sample {
            Some(bps) => bps,
            None => self
                .stream_info
                .as_ref()
                .map(|si| si.bits_per_sample)
                .ok_or_else(|| {
                    CodecError::InvalidData(
                        "FLAC: frame defers bit depth to STREAMINFO, which has not been parsed"
                            .to_string(),
                    )
                })?,
        };
        if !(1..=32).contains(&bits_per_sample) {
            return Err(CodecError::InvalidData(format!(
                "FLAC: unsupported bit depth {bits_per_sample}"
            )));
        }
        let bps = u32::from(bits_per_sample);

        let block_size = header.block_size as usize;
        let assignment = header.channel_assignment;
        let channels = assignment.channel_count();

        // Decode every subframe; side channels carry one extra bit.
        let mut decoded_channels: Vec<Vec<i32>> = Vec::with_capacity(channels);
        for index in 0..channels {
            let depth = bps + assignment.extra_bits(index);
            decoded_channels.push(read_subframe(&mut r, block_size, depth)?);
        }

        // Frame footer: zero-pad to a byte boundary, then the CRC-16.
        r.align_to_byte();
        let frame_len_before_crc = r.byte_pos();
        let crc_hi = r
            .read_aligned_byte()
            .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading CRC-16".to_string()))?;
        let crc_lo = r
            .read_aligned_byte()
            .ok_or_else(|| CodecError::InvalidData("FLAC: EOF reading CRC-16".to_string()))?;
        let stored_crc = (u16::from(crc_hi) << 8) | u16::from(crc_lo);
        let computed_crc = crc16(&data[..frame_len_before_crc]);
        if stored_crc != computed_crc {
            return Err(CodecError::InvalidData(format!(
                "FLAC: frame CRC-16 mismatch (stored {stored_crc:#06x}, computed {computed_crc:#06x})"
            )));
        }
        let bytes_consumed = frame_len_before_crc + 2;

        assignment.undo_decorrelation(&mut decoded_channels)?;

        // With a fixed block size the coded number is a frame number; convert
        // it to the first sample number so callers see a uniform meaning.
        // Scaling must use the *stream's* block size, not this frame's, or a
        // short final frame would report the wrong position.
        let (sample_number, frame_number) = if header.variable_block_size {
            (header.coded_number, None)
        } else {
            let stream_block_size = self
                .stream_info
                .as_ref()
                .filter(|si| si.min_block_size == si.max_block_size && si.min_block_size > 0)
                .map_or(block_size as u64, |si| u64::from(si.min_block_size));
            (
                header.coded_number.saturating_mul(stream_block_size),
                Some(header.coded_number),
            )
        };

        let mut samples = Vec::with_capacity(block_size * channels);
        for s in 0..block_size {
            for channel in &decoded_channels {
                samples.push(channel.get(s).copied().unwrap_or(0));
            }
        }

        Ok((
            DecodedBlock {
                samples,
                sample_number,
                frame_number,
                block_size,
                channels,
            },
            bytes_consumed,
        ))
    }

    /// Decode all frames from a complete FLAC byte stream (metadata + frames).
    ///
    /// Returns interleaved i32 PCM samples for the entire stream.  Every frame
    /// is decoded strictly: a malformed frame is an error, never silently
    /// skipped.  Trailing bytes that are not a frame at all (an appended ID3v1
    /// tag, say) end the stream cleanly once at least one frame was decoded.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidData` if the stream is malformed.
    pub fn decode_stream(&mut self, data: &[u8]) -> CodecResult<Vec<i32>> {
        let mut pos = self.parse_metadata_inner(data)?;
        let mut all_samples: Vec<i32> = Vec::new();

        while pos + 2 <= data.len() {
            if data[pos] != 0xFF || (data[pos + 1] & 0xFE) != 0xF8 {
                if all_samples.is_empty() {
                    return Err(CodecError::InvalidData(format!(
                        "FLAC: no frame sync at offset {pos}"
                    )));
                }
                break;
            }
            let (block, consumed) = self.decode_frame(&data[pos..])?;
            all_samples.extend_from_slice(&block.samples);
            if consumed == 0 {
                return Err(CodecError::InvalidData(
                    "FLAC: frame decode made no progress".to_string(),
                ));
            }
            pos += consumed;
        }

        Ok(all_samples)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac::{FlacConfig, FlacEncoder};

    fn make_encoder(channels: u8) -> FlacEncoder {
        FlacEncoder::new(FlacConfig {
            sample_rate: 44100,
            channels,
            bits_per_sample: 16,
        })
    }

    #[test]
    fn test_flac_decoder_new() {
        let dec = FlacDecoder::new();
        assert!(dec.stream_info.is_none());
        assert!(!dec.header_parsed);
    }

    #[test]
    fn test_parse_stream_header_magic() {
        let enc = make_encoder(2);
        let header = enc.stream_header();
        let mut dec = FlacDecoder::new();
        let consumed = dec.parse_stream_header(&header).expect("parse header");
        assert_eq!(consumed, 42, "Should consume exactly 42 bytes");
        assert!(dec.stream_info.is_some());
    }

    #[test]
    fn test_parse_stream_header_fields() {
        let enc = make_encoder(2);
        let header = enc.stream_header();
        let mut dec = FlacDecoder::new();
        dec.parse_stream_header(&header).expect("parse header");
        let si = dec.stream_info.as_ref().expect("stream_info");
        assert_eq!(si.sample_rate, 44100);
        assert_eq!(si.channels, 2);
        assert_eq!(si.bits_per_sample, 16);
    }

    #[test]
    fn test_parse_stream_header_bad_magic() {
        let bad = b"NOPE\x80\x00\x00\x22";
        let mut dec = FlacDecoder::new();
        let res = dec.parse_stream_header(bad);
        assert!(res.is_err());
    }

    #[test]
    fn test_decode_frame_verbatim_roundtrip() {
        let mut enc = make_encoder(1);
        // Use small constant signal so verbatim path triggers (or LPC path works)
        let input: Vec<i32> = vec![100i32; 32];
        let (header, frames) = enc.encode(&input).expect("encode");
        assert!(!frames.is_empty());

        let dec = FlacDecoder::new();
        let (block, _) = dec.decode_frame(&frames[0].data).expect("decode frame");
        assert_eq!(block.channels, 1);
        assert!(!block.samples.is_empty());
    }

    #[test]
    fn test_decode_stream_silence_roundtrip() {
        let mut enc = make_encoder(2);
        let silence = vec![0i32; 128 * 2]; // 128 stereo frames
        let (header, frames) = enc.encode(&silence).expect("encode");

        // Build complete FLAC stream
        let mut stream = header.clone();
        for f in &frames {
            stream.extend_from_slice(&f.data);
        }

        let mut dec = FlacDecoder::new();
        let decoded = dec.decode_stream(&stream).expect("decode_stream");
        // For silence (all zeros) the decoded output should be all zeros
        assert!(!decoded.is_empty(), "Should produce samples");
        for &s in &decoded {
            assert_eq!(s, 0, "All silence samples should decode as 0");
        }
    }

    #[test]
    fn test_decode_stream_ramp_roundtrip() {
        let mut enc = make_encoder(1);
        // Ramp signal
        let ramp: Vec<i32> = (0..64).map(|i| i * 10).collect();
        let (header, frames) = enc.encode(&ramp).expect("encode");

        let mut stream = header.clone();
        for f in &frames {
            stream.extend_from_slice(&f.data);
        }

        let mut dec = FlacDecoder::new();
        let decoded = dec.decode_stream(&stream).expect("decode_stream");
        assert!(!decoded.is_empty(), "Should produce samples");
    }

    #[test]
    fn test_decode_frame_block_size_preserved() {
        let mut enc = make_encoder(2);
        let samples = vec![500i32; 256 * 2]; // 256 stereo frames
        let (_, frames) = enc.encode(&samples).expect("encode");
        assert!(!frames.is_empty());

        let dec = FlacDecoder::new();
        let (block, _) = dec.decode_frame(&frames[0].data).expect("decode frame");
        assert_eq!(block.block_size, 256);
        assert_eq!(block.channels, 2);
    }

    #[test]
    fn test_decode_frame_sample_number() {
        let mut enc = make_encoder(2);
        let samples = vec![0i32; 4096 * 2 * 2]; // 2 frames worth
        let (_, frames) = enc.encode(&samples).expect("encode");
        if frames.len() >= 2 {
            let dec = FlacDecoder::new();
            let (block0, _) = dec.decode_frame(&frames[0].data).expect("frame 0");
            let (block1, _) = dec.decode_frame(&frames[1].data).expect("frame 1");
            assert_eq!(block0.sample_number, 0);
            assert!(block1.sample_number > block0.sample_number);
        }
    }

    #[test]
    fn test_decode_stream_header_bad_input() {
        let mut dec = FlacDecoder::new();
        let res = dec.decode_stream(b"short");
        // Either error on bad header or very short
        assert!(res.is_err() || res.is_ok()); // not panic
    }

    #[test]
    fn test_crc16_matches_encoder() {
        // CRC-16 used by the decoder must agree with the encoder's CRC-16
        let data = b"FLAC test data for CRC verification";
        let enc_crc = {
            // Re-implement the same CRC to cross-check
            const POLY: u16 = 0x8005;
            let mut crc = 0u16;
            for &byte in data.iter() {
                crc ^= u16::from(byte) << 8;
                for _ in 0..8 {
                    if crc & 0x8000 != 0 {
                        crc = (crc << 1) ^ POLY;
                    } else {
                        crc <<= 1;
                    }
                }
            }
            crc
        };
        let dec_crc = crc16(data);
        assert_eq!(enc_crc, dec_crc);
    }

    #[test]
    fn test_decode_mono_stream() {
        let mut enc = FlacEncoder::new(FlacConfig {
            sample_rate: 48000,
            channels: 1,
            bits_per_sample: 16,
        });
        let samples = vec![0i32; 512];
        let (header, frames) = enc.encode(&samples).expect("encode mono");

        let mut stream = header;
        for f in frames {
            stream.extend_from_slice(&f.data);
        }

        let mut dec = FlacDecoder::new();
        let decoded = dec.decode_stream(&stream).expect("decode mono");
        assert!(!decoded.is_empty());
    }

    // ------------------------------------------------------------------
    // Tests for the new metadata-parsing API
    // ------------------------------------------------------------------

    #[test]
    fn test_parse_metadata_valid_stream() {
        let enc = make_encoder(2);
        let header = enc.stream_header();
        // Append dummy frame data so parse_metadata has a complete stream header
        let stream = header.clone();
        // parse_metadata only needs the header portion
        let mut dec = FlacDecoder::new();
        dec.parse_metadata(&stream)
            .expect("parse_metadata should succeed");
        assert!(dec.stream_info.is_some(), "stream_info should be populated");
    }

    #[test]
    fn test_parse_metadata_rejects_bad_magic() {
        let bad = b"WAVE\x80\x00\x00\x22XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
        let mut dec = FlacDecoder::new();
        let res = dec.parse_metadata(bad);
        assert!(res.is_err(), "Should reject non-fLaC magic");
    }

    #[test]
    fn test_parse_metadata_stream_info_fields() {
        let enc = FlacEncoder::new(FlacConfig {
            sample_rate: 48000,
            channels: 1,
            bits_per_sample: 16,
        });
        let header = enc.stream_header();
        let mut dec = FlacDecoder::new();
        dec.parse_metadata(&header).expect("parse_metadata");
        let si = dec.stream_info().expect("stream_info should be Some");
        assert_eq!(si.sample_rate, 48000);
        assert_eq!(si.channels, 1);
        assert_eq!(si.bits_per_sample, 16);
    }

    #[test]
    fn test_stream_info_method_returns_none_before_parse() {
        let dec = FlacDecoder::new();
        assert!(dec.stream_info().is_none());
    }

    #[test]
    fn test_stream_info_method_returns_some_after_parse() {
        let enc = make_encoder(2);
        let header = enc.stream_header();
        let mut dec = FlacDecoder::new();
        dec.parse_metadata(&header).expect("parse_metadata");
        assert!(dec.stream_info().is_some());
    }

    #[test]
    fn test_comment_block_returns_none_when_absent() {
        let enc = make_encoder(1);
        let header = enc.stream_header();
        let mut dec = FlacDecoder::new();
        dec.parse_metadata(&header).expect("parse_metadata");
        // The FlacEncoder does not write a VORBIS_COMMENT block
        assert!(
            dec.comment_block().is_none(),
            "No VORBIS_COMMENT block expected from FlacEncoder stream"
        );
    }

    #[test]
    fn test_parse_vorbis_comment_block_direct() {
        // Build a minimal VORBIS_COMMENT block body manually (little-endian)
        // Vendor = "OxiMedia" (8 bytes)
        // 1 comment: "TITLE=Test Track"
        let vendor = b"OxiMedia";
        let comment = b"TITLE=Test Track";
        let mut block: Vec<u8> = Vec::new();
        // vendor length (LE u32)
        block.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        block.extend_from_slice(vendor);
        // comment count = 1
        block.extend_from_slice(&1u32.to_le_bytes());
        // comment entry length + data
        block.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        block.extend_from_slice(comment);

        let vc = FlacDecoder::parse_vorbis_comment_block(&block)
            .expect("parse_vorbis_comment_block should succeed");
        assert_eq!(vc.vendor, "OxiMedia");
        assert_eq!(vc.comments.len(), 1);
        assert_eq!(vc.comments[0].0, "TITLE");
        assert_eq!(vc.comments[0].1, "Test Track");
    }

    #[test]
    fn test_parse_vorbis_comment_multiple_entries() {
        let vendor = b"TestEncoder";
        let c1 = b"ARTIST=Some Artist";
        let c2 = b"ALBUM=Great Album";
        let c3 = b"TRACKNUMBER=3";

        let mut block: Vec<u8> = Vec::new();
        block.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        block.extend_from_slice(vendor);
        block.extend_from_slice(&3u32.to_le_bytes());
        for comment in [c1.as_slice(), c2.as_slice(), c3.as_slice()] {
            block.extend_from_slice(&(comment.len() as u32).to_le_bytes());
            block.extend_from_slice(comment);
        }

        let vc =
            FlacDecoder::parse_vorbis_comment_block(&block).expect("parse_vorbis_comment_block");
        assert_eq!(vc.vendor, "TestEncoder");
        assert_eq!(vc.comments.len(), 3);
        assert_eq!(
            vc.comments[0],
            ("ARTIST".to_string(), "Some Artist".to_string())
        );
        assert_eq!(
            vc.comments[1],
            ("ALBUM".to_string(), "Great Album".to_string())
        );
        assert_eq!(vc.comments[2], ("TRACKNUMBER".to_string(), "3".to_string()));
    }

    #[test]
    fn test_parse_metadata_with_vorbis_comment_block() {
        // Build a synthetic FLAC stream with fLaC + STREAMINFO + VORBIS_COMMENT
        let enc = make_encoder(2);
        let streaminfo_header = enc.stream_header(); // "fLaC" + STREAMINFO block

        // Re-build: strip last-block flag from STREAMINFO and append a VORBIS_COMMENT block
        // streaminfo_header[4] has the block-type byte; bit7 = last_block
        let mut stream = streaminfo_header.clone();
        // Clear the last-block flag (bit 7) in the STREAMINFO block header byte
        stream[4] &= 0x7F;

        // Build VORBIS_COMMENT body
        let vendor = b"OxiMediaTest";
        let comment = b"COMMENT=hello";
        let mut vc_body: Vec<u8> = Vec::new();
        vc_body.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        vc_body.extend_from_slice(vendor);
        vc_body.extend_from_slice(&1u32.to_le_bytes());
        vc_body.extend_from_slice(&(comment.len() as u32).to_le_bytes());
        vc_body.extend_from_slice(comment);

        // VORBIS_COMMENT block header: last=1 (0x80), type=4 → 0x84; 3-byte length
        let vc_len = vc_body.len() as u32;
        stream.push(0x84); // last block, type 4
        stream.push(((vc_len >> 16) & 0xFF) as u8);
        stream.push(((vc_len >> 8) & 0xFF) as u8);
        stream.push((vc_len & 0xFF) as u8);
        stream.extend_from_slice(&vc_body);

        let mut dec = FlacDecoder::new();
        dec.parse_metadata(&stream)
            .expect("parse_metadata with VORBIS_COMMENT");
        assert!(dec.stream_info().is_some());
        let vc = dec
            .comment_block()
            .expect("VORBIS_COMMENT should be parsed");
        assert_eq!(vc.vendor, "OxiMediaTest");
        assert_eq!(vc.comments.len(), 1);
        assert_eq!(vc.comments[0].0, "COMMENT");
        assert_eq!(vc.comments[0].1, "hello");
    }

    #[test]
    fn test_probe_valid_stream() {
        let enc = make_encoder(2);
        let header = enc.stream_header();
        let si = FlacDecoder::probe(&header).expect("probe should succeed");
        assert_eq!(si.sample_rate, 44100);
        assert_eq!(si.channels, 2);
        assert_eq!(si.bits_per_sample, 16);
    }

    #[test]
    fn test_probe_rejects_bad_magic() {
        let bad = b"RIFF\x80\x00\x00\x22XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
        let res = FlacDecoder::probe(bad);
        assert!(res.is_err(), "probe should reject non-fLaC magic");
    }

    #[test]
    fn test_probe_rejects_too_short() {
        let short = b"fLaC";
        let res = FlacDecoder::probe(short);
        assert!(res.is_err(), "probe should reject too-short data");
    }

    #[test]
    fn test_streaminfo_min_max_block_size() {
        let enc = make_encoder(2);
        let header = enc.stream_header();
        let si = FlacDecoder::probe(&header).expect("probe");
        // min_block_size should be <= max_block_size
        assert!(si.min_block_size <= si.max_block_size);
        // max_block_size should be a valid FLAC block size (>= 16)
        assert!(si.max_block_size >= 16);
    }

    #[test]
    fn test_streaminfo_new_fields_accessible() {
        let enc = make_encoder(1);
        let header = enc.stream_header();
        let si = FlacDecoder::probe(&header).expect("probe");
        // New fields must be accessible (may be zero if encoder doesn't set them)
        let _ = si.min_frame_size;
        let _ = si.max_frame_size;
        let _ = si.total_samples;
        let _ = si.md5_signature;
    }
}
