//! FLAC audio encoder.
//!
//! Encodes interleaved i32 PCM samples into RFC 9639-conformant FLAC frames.
//!
//! # Encoding pipeline
//!
//! 1. Frame blocking (split PCM into block-size chunks).
//! 2. Inter-channel decorrelation for stereo (independent / left-side /
//!    side-right / mid-side, chosen by exact coded size).
//! 3. Per-channel subframe selection — constant, verbatim, fixed predictor or
//!    quantised LPC, again by exact coded size ([`super::subframe`]).
//! 4. Partitioned Rice coding of the residuals ([`super::residual`]).
//! 5. Frame serialisation: bit-packed subframes, zero padding to the next byte
//!    boundary, CRC-8 in the header and CRC-16 in the footer.
//!
//! The residuals are computed with exactly the integer arithmetic the decoder
//! uses to invert them, so every frame this encoder emits is bit-exact.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_sign_loss)]

use md5::{Digest, Md5};

use super::bitio::{crc16, BitWriter};
use super::frame::{ChannelAssignment, FrameHeader, MAX_BLOCK_SIZE};
use super::residual::DEFAULT_MAX_PARTITION_ORDER;
use super::subframe::{plan_subframe, write_subframe, SubframePlan};
use crate::error::{CodecError, CodecResult};

// =============================================================================
// Configuration
// =============================================================================

/// FLAC encoder configuration.
#[derive(Clone, Debug)]
pub struct FlacConfig {
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Number of audio channels.
    pub channels: u8,
    /// Bits per sample (8, 12, 16, 20, 24 or 32 are coded directly in the
    /// frame header; other depths defer to STREAMINFO).
    pub bits_per_sample: u8,
}

impl FlacConfig {
    /// Default frame block size (number of samples per channel per frame).
    pub const BLOCK_SIZE: usize = 4096;
    /// Default maximum LPC order used for compression.
    pub const LPC_ORDER: usize = 8;
}

// =============================================================================
// Encoded frame
// =============================================================================

/// One encoded FLAC frame.
#[derive(Clone, Debug)]
pub struct FlacFrame {
    /// Raw FLAC frame bytes (sync code + header + subframes + CRC-16).
    pub data: Vec<u8>,
    /// Sample number of the first sample in this frame.
    pub sample_number: u64,
    /// Number of samples (per channel) in this frame.
    pub block_size: u32,
}

// =============================================================================
// Encoder
// =============================================================================

/// FLAC audio encoder.
pub struct FlacEncoder {
    config: FlacConfig,
    block_size: usize,
    max_lpc_order: usize,
    max_partition_order: u32,
    /// Whether frames carry a sample number (variable) or a frame number (fixed).
    variable_block_size: bool,
    /// Total samples encoded so far (per channel).
    samples_encoded: u64,
    /// Number of frames emitted so far (the fixed-block-size frame counter).
    frames_encoded: u64,
    min_frame_size: u32,
    max_frame_size: u32,
    /// Largest block emitted so far.
    max_block_seen: u32,
    /// Smallest block emitted so far, excluding the most recent one — a short
    /// final block does not lower STREAMINFO's minimum (RFC 9639 §8.2).
    min_block_seen: u32,
    /// The most recent block size, still pending inclusion in `min_block_seen`.
    last_block_size: Option<u32>,
    md5: Md5,
}

impl FlacEncoder {
    /// Create a new FLAC encoder with the default 4096-sample block size.
    #[must_use]
    pub fn new(config: FlacConfig) -> Self {
        Self {
            config,
            block_size: FlacConfig::BLOCK_SIZE,
            max_lpc_order: FlacConfig::LPC_ORDER,
            max_partition_order: DEFAULT_MAX_PARTITION_ORDER,
            variable_block_size: false,
            samples_encoded: 0,
            frames_encoded: 0,
            min_frame_size: u32::MAX,
            max_frame_size: 0,
            max_block_seen: 0,
            min_block_seen: u32::MAX,
            last_block_size: None,
            md5: Md5::new(),
        }
    }

    /// Create an encoder with an explicit block size.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidParameter` when `block_size` is outside
    /// `1..=65535` (the range a FLAC frame header can express).
    pub fn with_block_size(config: FlacConfig, block_size: usize) -> CodecResult<Self> {
        if block_size == 0 || block_size > MAX_BLOCK_SIZE as usize {
            return Err(CodecError::InvalidParameter(format!(
                "FLAC block size {block_size} out of range 1..={MAX_BLOCK_SIZE}"
            )));
        }
        let mut encoder = Self::new(config);
        encoder.block_size = block_size;
        Ok(encoder)
    }

    /// Block size this encoder emits.
    #[must_use]
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Set the maximum LPC order searched (1..=32); 0 disables LPC entirely.
    pub fn set_max_lpc_order(&mut self, order: usize) {
        self.max_lpc_order = order.min(super::subframe::MAX_LPC_ORDER);
    }

    /// Set the maximum residual partition order searched (0..=15).
    pub fn set_max_partition_order(&mut self, order: u32) {
        self.max_partition_order = order.min(super::residual::MAX_PARTITION_ORDER);
    }

    /// Switch the stream between fixed and variable block sizes.
    ///
    /// A fixed-block-size stream (the default, and what libFLAC and ffmpeg
    /// emit) codes a **frame number** in each frame header, so every frame but
    /// the last must be exactly [`Self::block_size`] samples long.  Feeding
    /// [`Self::encode`] a sample count that is not a multiple of the block size
    /// therefore ends the stream — a later call would produce frames whose
    /// numbers no longer map to their true sample positions, and
    /// [`Self::encode`] rejects that rather than emitting a mis-numbered
    /// stream.
    ///
    /// Enabling variable block sizes codes an explicit **sample number**
    /// instead, so blocks of any length may appear anywhere.  Frames are a few
    /// bytes larger, and the stream is no longer in the FLAC subset.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidParameter` once any frame has been emitted:
    /// the blocking strategy must be identical for every frame in a stream
    /// (RFC 9639 §9.1.1).
    pub fn set_variable_block_size(&mut self, enabled: bool) -> CodecResult<()> {
        if self.frames_encoded > 0 && self.variable_block_size != enabled {
            return Err(CodecError::InvalidParameter(
                "FLAC: the blocking strategy must be the same for every frame in a stream; \
                 set it before the first encode() call"
                    .to_string(),
            ));
        }
        self.variable_block_size = enabled;
        Ok(())
    }

    /// Whether this encoder emits variable-block-size frames.
    #[must_use]
    pub fn is_variable_block_size(&self) -> bool {
        self.variable_block_size
    }

    /// Total samples per channel encoded so far.
    #[must_use]
    pub fn samples_encoded(&self) -> u64 {
        self.samples_encoded
    }

    /// Generate the FLAC stream header (`fLaC` magic + STREAMINFO block).
    ///
    /// Must be placed at the start of the stream before any frames.  Total
    /// sample count, frame sizes and the MD5 signature are unknown before
    /// encoding, so they are written as "unknown" (zero); use
    /// [`Self::finalized_stream_header`] afterwards to rewrite these 42 bytes
    /// with the real values.
    #[must_use]
    pub fn stream_header(&self) -> Vec<u8> {
        self.build_stream_header(0, 0, 0, [0u8; 16])
    }

    /// Rebuild the 42-byte stream header with the values learned while
    /// encoding: exact total sample count, minimum/maximum frame size and the
    /// MD5 signature of the unencoded audio.
    ///
    /// Overwrite the first 42 bytes of the stream with the result, exactly as
    /// a seekable FLAC encoder does when it finishes.
    #[must_use]
    pub fn finalized_stream_header(&self) -> Vec<u8> {
        let md5: [u8; 16] = self.md5.clone().finalize().into();
        let min_frame = if self.min_frame_size == u32::MAX {
            0
        } else {
            self.min_frame_size
        };
        // STREAMINFO's minimum excludes the final block, so a short tail does
        // not make a fixed-block-size stream look variable.
        let max_block = if self.max_block_seen == 0 {
            self.block_size.min(u16::MAX as usize) as u32
        } else {
            self.max_block_seen
        };
        let min_block = if self.min_block_seen == u32::MAX {
            max_block
        } else {
            self.min_block_seen
        };
        self.build_stream_header_with_blocks(
            min_block,
            max_block,
            min_frame,
            self.max_frame_size,
            self.samples_encoded,
            md5,
        )
    }

    fn build_stream_header(
        &self,
        min_frame_size: u32,
        max_frame_size: u32,
        total_samples: u64,
        md5: [u8; 16],
    ) -> Vec<u8> {
        let bs = self.block_size.min(u16::MAX as usize) as u32;
        self.build_stream_header_with_blocks(
            bs,
            bs,
            min_frame_size,
            max_frame_size,
            total_samples,
            md5,
        )
    }

    fn build_stream_header_with_blocks(
        &self,
        min_block_size: u32,
        max_block_size: u32,
        min_frame_size: u32,
        max_frame_size: u32,
        total_samples: u64,
        md5: [u8; 16],
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(42);
        out.extend_from_slice(b"fLaC");

        // METADATA_BLOCK_HEADER: last-metadata-block=1, type=0 (STREAMINFO), length=34
        out.push(0x80);
        out.push(0x00);
        out.push(0x00);
        out.push(34);

        out.extend_from_slice(&(min_block_size.min(u32::from(u16::MAX)) as u16).to_be_bytes());
        out.extend_from_slice(&(max_block_size.min(u32::from(u16::MAX)) as u16).to_be_bytes());
        let min_frame = min_frame_size.min(0x00FF_FFFF);
        let max_frame = max_frame_size.min(0x00FF_FFFF);
        out.extend_from_slice(&min_frame.to_be_bytes()[1..]);
        out.extend_from_slice(&max_frame.to_be_bytes()[1..]);

        // 20-bit sample rate | 3-bit channels-1 | 5-bit bps-1 | 36-bit total samples
        let sr = u64::from(self.config.sample_rate.min(0x000F_FFFF));
        let ch = u64::from(self.config.channels.clamp(1, 8) - 1);
        let bps = u64::from(self.config.bits_per_sample.clamp(1, 32) - 1);
        let total = total_samples & 0x000F_FFFF_FFFF;
        let packed = (sr << 44) | (ch << 41) | (bps << 36) | total;
        out.extend_from_slice(&packed.to_be_bytes());

        out.extend_from_slice(&md5);
        out
    }

    /// Encode interleaved i32 PCM samples into one or more FLAC frames.
    ///
    /// `samples` is interleaved: `[ch0_s0, ch1_s0, ch0_s1, ch1_s1, ...]`.
    ///
    /// Returns `(stream_header, frames)`.  The header is the provisional one
    /// from [`Self::stream_header`]; call [`Self::finalized_stream_header`]
    /// once the whole stream has been encoded to obtain the accurate version.
    ///
    /// # Errors
    ///
    /// Returns `CodecError::InvalidParameter` if the configuration is
    /// unsupported, the sample count is not a multiple of `channels`, or a
    /// sample does not fit in the configured bit depth.
    pub fn encode(&mut self, samples: &[i32]) -> CodecResult<(Vec<u8>, Vec<FlacFrame>)> {
        self.validate_config()?;
        let ch = self.config.channels as usize;
        if samples.len() % ch != 0 {
            return Err(CodecError::InvalidParameter(
                "Sample count must be a multiple of channel count".to_string(),
            ));
        }

        // A fixed-block-size stream codes frame numbers, so every frame but the
        // last must be a whole block.  If a previous call ended mid-block, any
        // further frame would be numbered as if it started a whole block later,
        // silently corrupting every sample position from here on.
        if !self.variable_block_size && self.samples_encoded % self.block_size as u64 != 0 {
            return Err(CodecError::InvalidParameter(format!(
                "FLAC: a fixed-block-size stream cannot contain a short non-final frame, but a \
                 previous encode() call ended mid-block ({} samples encoded, block size {}). \
                 Feed whole blocks, or call set_variable_block_size(true) before encoding.",
                self.samples_encoded, self.block_size
            )));
        }

        let bps = u32::from(self.config.bits_per_sample);
        if let Some(&bad) = samples.iter().find(|&&s| !fits_signed(s, bps)) {
            return Err(CodecError::InvalidParameter(format!(
                "FLAC: sample {bad} does not fit in {bps} bits"
            )));
        }
        self.md5_update(samples);

        let frame_samples = samples.len() / ch;
        let header = self.stream_header();
        let mut frames = Vec::new();

        let mut offset = 0usize;
        while offset < frame_samples {
            let end = (offset + self.block_size).min(frame_samples);
            let block_len = end - offset;

            let channels: Vec<Vec<i32>> = (0..ch)
                .map(|c| (offset..end).map(|s| samples[s * ch + c]).collect())
                .collect();

            let frame = self.encode_frame(&channels, block_len)?;
            self.samples_encoded += block_len as u64;
            self.frames_encoded += 1;
            let size = frame.data.len() as u32;
            self.min_frame_size = self.min_frame_size.min(size);
            self.max_frame_size = self.max_frame_size.max(size);
            // The previous block is now known not to be the last one, so it
            // may lower STREAMINFO's minimum.
            if let Some(previous) = self.last_block_size.replace(block_len as u32) {
                self.min_block_seen = self.min_block_seen.min(previous);
            }
            self.max_block_seen = self.max_block_seen.max(block_len as u32);
            frames.push(frame);

            offset = end;
        }

        Ok((header, frames))
    }

    fn validate_config(&self) -> CodecResult<()> {
        if !(1..=8).contains(&self.config.channels) {
            return Err(CodecError::InvalidParameter(format!(
                "FLAC supports 1..=8 channels, got {}",
                self.config.channels
            )));
        }
        if !(4..=32).contains(&self.config.bits_per_sample) {
            return Err(CodecError::InvalidParameter(format!(
                "FLAC supports 4..=32 bits per sample, got {}",
                self.config.bits_per_sample
            )));
        }
        if self.config.sample_rate == 0 || self.config.sample_rate > 0x000F_FFFF {
            return Err(CodecError::InvalidParameter(format!(
                "FLAC sample rate {} out of range 1..=1048575",
                self.config.sample_rate
            )));
        }
        Ok(())
    }

    /// Accumulate the MD5 of the unencoded audio, exactly as libFLAC does:
    /// interleaved samples, little-endian, `ceil(bps / 8)` bytes each.
    fn md5_update(&mut self, samples: &[i32]) {
        let bytes_per_sample = ((self.config.bits_per_sample as usize) + 7) / 8;
        let mut buf = Vec::with_capacity(samples.len() * bytes_per_sample);
        for &s in samples {
            let le = s.to_le_bytes();
            buf.extend_from_slice(&le[..bytes_per_sample.min(4)]);
        }
        self.md5.update(&buf);
    }

    /// Encode one block of deinterleaved channel data into a FLAC frame.
    fn encode_frame(&self, channels: &[Vec<i32>], block_len: usize) -> CodecResult<FlacFrame> {
        let bps = u32::from(self.config.bits_per_sample);
        let (assignment, plans) = self.plan_channels(channels, bps)?;

        let header = FrameHeader {
            variable_block_size: self.variable_block_size,
            block_size: block_len as u32,
            sample_rate: Some(self.config.sample_rate),
            channel_assignment: assignment,
            bits_per_sample: Some(self.config.bits_per_sample),
            coded_number: if self.variable_block_size {
                self.samples_encoded
            } else {
                self.frames_encoded
            },
        };
        let header_bytes = header.to_bytes()?;

        let estimated =
            header_bytes.len() + plans.iter().map(|p| p.bits / 8 + 1).sum::<usize>() + 4;
        let mut w = BitWriter::with_capacity(estimated);
        if !w.write_aligned_bytes(&header_bytes) {
            return Err(CodecError::Internal(
                "FLAC: frame writer was not byte-aligned at the header".to_string(),
            ));
        }
        for (index, plan) in plans.iter().enumerate() {
            write_subframe(&mut w, bps + assignment.extra_bits(index), plan)?;
        }
        w.align_to_byte();

        let mut data = w.into_bytes();
        let crc = crc16(&data);
        data.extend_from_slice(&crc.to_be_bytes());

        Ok(FlacFrame {
            data,
            sample_number: self.samples_encoded,
            block_size: block_len as u32,
        })
    }

    /// Pick the channel assignment with the smallest total coded size.
    fn plan_channels(
        &self,
        channels: &[Vec<i32>],
        bps: u32,
    ) -> CodecResult<(ChannelAssignment, Vec<SubframePlan>)> {
        let count = channels.len();
        // Stereo decorrelation needs one extra bit for the side channel.
        if count == 2 && bps < 32 {
            let left = &channels[0];
            let right = &channels[1];

            let plan_left = self.plan_one(left, bps)?;
            let plan_right = self.plan_one(right, bps)?;

            let (mid, side) = ChannelAssignment::MidSide.apply_decorrelation(left, right);
            let plan_mid = self.plan_one(&mid, bps)?;
            let plan_side = self.plan_one(&side, bps + 1)?;

            let candidates = [
                (
                    ChannelAssignment::Independent(2),
                    plan_left.bits + plan_right.bits,
                ),
                (ChannelAssignment::LeftSide, plan_left.bits + plan_side.bits),
                (
                    ChannelAssignment::SideRight,
                    plan_side.bits + plan_right.bits,
                ),
                (ChannelAssignment::MidSide, plan_mid.bits + plan_side.bits),
            ];
            let best = candidates
                .iter()
                .min_by_key(|(_, bits)| *bits)
                .map(|(assignment, _)| *assignment)
                .unwrap_or(ChannelAssignment::Independent(2));

            let plans = match best {
                ChannelAssignment::LeftSide => vec![plan_left, plan_side],
                ChannelAssignment::SideRight => vec![plan_side, plan_right],
                ChannelAssignment::MidSide => vec![plan_mid, plan_side],
                _ => vec![plan_left, plan_right],
            };
            return Ok((best, plans));
        }

        let assignment = ChannelAssignment::Independent(count as u8);
        let mut plans = Vec::with_capacity(count);
        for channel in channels {
            plans.push(self.plan_one(channel, bps)?);
        }
        Ok((assignment, plans))
    }

    fn plan_one(&self, samples: &[i32], bps: u32) -> CodecResult<SubframePlan> {
        plan_subframe(samples, bps, self.max_lpc_order, self.max_partition_order)
    }
}

/// Whether `value` fits in a `bits`-wide two's-complement integer.
fn fits_signed(value: i32, bits: u32) -> bool {
    if bits >= 32 {
        return true;
    }
    let lo = -(1i64 << (bits - 1));
    let hi = (1i64 << (bits - 1)) - 1;
    (lo..=hi).contains(&i64::from(value))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac::bitio::crc8;
    use crate::flac::decoder::FlacDecoder;

    fn make_encoder() -> FlacEncoder {
        FlacEncoder::new(FlacConfig {
            sample_rate: 44100,
            channels: 2,
            bits_per_sample: 16,
        })
    }

    /// Encode then decode, asserting exact byte accounting and sample equality.
    fn assert_round_trip(encoder: &mut FlacEncoder, pcm: &[i32], channels: usize) {
        let (header, frames) = encoder.encode(pcm).expect("encode");
        assert!(!frames.is_empty(), "encoder must emit at least one frame");

        let decoder = FlacDecoder::new();
        let mut decoded: Vec<i32> = Vec::new();
        for frame in &frames {
            let (block, consumed) = decoder.decode_frame(&frame.data).expect("decode frame");
            assert_eq!(
                consumed,
                frame.data.len(),
                "decoder must consume exactly the bytes the encoder wrote"
            );
            assert_eq!(block.block_size, frame.block_size as usize);
            assert_eq!(block.channels, channels);
            assert_eq!(block.sample_number, frame.sample_number);
            decoded.extend_from_slice(&block.samples);
        }
        assert_eq!(decoded.len(), pcm.len(), "sample count must be preserved");
        assert_eq!(decoded, pcm, "FLAC must be lossless");
        assert!(header.starts_with(b"fLaC"));
    }

    #[test]
    fn test_flac_stream_header_magic() {
        let enc = make_encoder();
        let header = enc.stream_header();
        assert!(header.starts_with(b"fLaC"), "Header must start with fLaC");
    }

    #[test]
    fn test_flac_stream_header_length() {
        let enc = make_encoder();
        // fLaC (4) + METADATA_BLOCK_HEADER (4) + STREAMINFO (34) = 42
        assert_eq!(enc.stream_header().len(), 42);
        assert_eq!(enc.finalized_stream_header().len(), 42);
    }

    #[test]
    fn test_flac_encode_silence() {
        let mut enc = make_encoder();
        let silence = vec![0i32; 4096 * 2];
        assert_round_trip(&mut enc, &silence, 2);
    }

    #[test]
    fn test_flac_encode_ramp() {
        let mut enc = make_encoder();
        let ramp: Vec<i32> = (0..4096 * 2).map(|i| (i % 1000 - 500) as i32).collect();
        assert_round_trip(&mut enc, &ramp, 2);
    }

    #[test]
    fn test_flac_encode_wrong_channel_count_errors() {
        let mut enc = make_encoder();
        assert!(enc.encode(&[0i32; 3]).is_err());
    }

    #[test]
    fn test_flac_encode_rejects_out_of_range_samples() {
        let mut enc = make_encoder();
        // 70000 needs 18 bits; the encoder declares 16.
        assert!(enc.encode(&[70000i32, 0]).is_err());
    }

    #[test]
    fn test_flac_encode_rejects_bad_config() {
        let mut enc = FlacEncoder::new(FlacConfig {
            sample_rate: 0,
            channels: 2,
            bits_per_sample: 16,
        });
        assert!(enc.encode(&[0i32; 4]).is_err());

        let mut enc = FlacEncoder::new(FlacConfig {
            sample_rate: 44100,
            channels: 9,
            bits_per_sample: 16,
        });
        assert!(enc.encode(&[0i32; 9]).is_err());
    }

    #[test]
    fn test_flac_frame_sample_number_increases() {
        let mut enc = make_encoder();
        let samples: Vec<i32> = vec![0i32; FlacConfig::BLOCK_SIZE * 4];
        let (_, frames) = enc.encode(&samples).expect("encode");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].sample_number, 0);
        assert_eq!(frames[1].sample_number, FlacConfig::BLOCK_SIZE as u64);
    }

    #[test]
    fn test_flac_encode_mono() {
        let mut enc = FlacEncoder::new(FlacConfig {
            sample_rate: 48000,
            channels: 1,
            bits_per_sample: 16,
        });
        let samples: Vec<i32> = (0..2048).map(|i| ((i * 71) % 4001) - 2000).collect();
        assert_round_trip(&mut enc, &samples, 1);
    }

    /// The frame-header CRC-8 byte must be the real CRC-8 of the preceding
    /// header bytes, not a hardcoded value.
    #[test]
    fn test_flac_frame_header_crc8_is_real() {
        let mut enc = make_encoder();
        let samples = vec![12345i32; 4096 * 2];
        let (_, frames) = enc.encode(&samples).expect("encode");
        let frame = &frames[0].data;
        // sync(2) + bs/sr(1) + ch/bps(1) + coded number(1, frame 0) = 5 bytes
        // then the explicit block size is absent (4096 is a table value).
        let header_len = 5;
        assert_eq!(
            frame[header_len],
            crc8(&frame[..header_len]),
            "frame header CRC-8 must cover every preceding header byte"
        );
    }

    /// The frame footer CRC-16 must cover the whole frame including the sync.
    #[test]
    fn test_flac_frame_footer_crc16_is_real() {
        let mut enc = make_encoder();
        let samples: Vec<i32> = (0..512 * 2).map(|i| (i % 733) as i32 - 366).collect();
        let (_, frames) = enc.encode(&samples).expect("encode");
        for frame in &frames {
            let body = &frame.data[..frame.data.len() - 2];
            let stored = u16::from_be_bytes([
                frame.data[frame.data.len() - 2],
                frame.data[frame.data.len() - 1],
            ]);
            assert_eq!(stored, crc16(body), "frame CRC-16 mismatch");
        }
    }

    /// The regression this slice exists for: the encoder's declared frame
    /// length and the decoder's bit-exact consumption must agree exactly.
    ///
    /// The previous byte-oriented subframe writer disagreed with the
    /// bit-oriented reader (8243-vs-42104 bytes consumed for a ramp,
    /// 16399-vs-16400 for silence), so the test that used to live here could
    /// only assert `consumed <= frame.data.len()`.  Both sides are now
    /// RFC 9639-exact, so equality holds.
    #[test]
    fn test_encoded_frame_length_matches_decoder_consumption() {
        for pcm in [
            vec![0i32; 4096 * 2],
            (0..4096 * 2).map(|i| (i % 1000 - 500) as i32).collect(),
            (0..4096 * 2)
                .map(|i| ((f64::from(i) * 0.01).sin() * 30000.0) as i32)
                .collect(),
        ] {
            let mut enc = make_encoder();
            let (_, frames) = enc.encode(&pcm).expect("encode");
            let dec = FlacDecoder::new();
            for frame in &frames {
                let (block, consumed) = dec.decode_frame(&frame.data).expect("decode");
                assert_eq!(
                    consumed,
                    frame.data.len(),
                    "consumed {consumed} of {} bytes",
                    frame.data.len()
                );
                assert_eq!(block.block_size, frame.block_size as usize);
                assert_eq!(block.channels, 2);
            }
        }
    }

    #[test]
    fn test_finalized_header_carries_totals_and_md5() {
        let mut enc = make_encoder();
        let pcm: Vec<i32> = (0..1000 * 2).map(|i| (i % 257) as i32 - 128).collect();
        enc.encode(&pcm).expect("encode");
        let header = enc.finalized_stream_header();

        // total_samples occupies the low 36 bits of bytes 18..26 of the stream.
        let si = &header[8..];
        let packed = u64::from_be_bytes([
            si[10], si[11], si[12], si[13], si[14], si[15], si[16], si[17],
        ]);
        assert_eq!(packed & 0x000F_FFFF_FFFF, 1000);
        assert_eq!(packed >> 44, 44100);

        let md5 = &si[18..34];
        assert_ne!(md5, [0u8; 16], "MD5 must be computed, not left blank");

        // Reference MD5 over the interleaved little-endian 16-bit PCM.
        let mut reference = Md5::new();
        let mut buf = Vec::new();
        for &s in &pcm {
            buf.extend_from_slice(&s.to_le_bytes()[..2]);
        }
        reference.update(&buf);
        let expected: [u8; 16] = reference.finalize().into();
        assert_eq!(md5, expected);
    }

    #[test]
    fn test_with_block_size_validates_range() {
        let config = FlacConfig {
            sample_rate: 44100,
            channels: 1,
            bits_per_sample: 16,
        };
        assert!(FlacEncoder::with_block_size(config.clone(), 0).is_err());
        assert!(FlacEncoder::with_block_size(config.clone(), 65536).is_err());
        let enc = FlacEncoder::with_block_size(config, 1024).expect("valid block size");
        assert_eq!(enc.block_size(), 1024);
    }
}
