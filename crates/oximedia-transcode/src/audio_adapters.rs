// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Real audio `FrameDecoder` / `FrameEncoder` adapters for the frame-level
//! transcode path.
//!
//! The decoder side operates on interleaved i16 LE PCM held in memory
//! (produced by the WAV demuxer or a FLAC decode pass); this matches the
//! byte layout that [`crate::pipeline_context::FilterGraph`]'s audio-gain
//! op expects, so `-af volume=..dB` works unmodified between decode and
//! encode.
//!
//! The encoder side wraps the workspace's real audio encoders behind the
//! [`FrameEncoder`] trait consumed by [`crate::multi_track::MultiTrackExecutor`]:
//!
//! | Adapter             | Wrapped encoder                          | Output payload            |
//! |---------------------|------------------------------------------|---------------------------|
//! | [`FlacFrameEncoder`]| `oximedia_audio::flac::FlacEncoder`      | raw FLAC frames           |
//! | [`PcmFrameEncoder`] | (identity)                               | interleaved i16 LE PCM    |
//! | [`AlacFrameEncoder`]| `crate::alac_bitstream::AlacStreamEncoder` | raw ALAC frames (escape + compressed, both FFmpeg-verified) |
//!
//! Every adapter chunks its input to the wrapped encoder's native block
//! size internally, so callers may feed frames of any length.
//!
//! There is intentionally no Opus adapter here — see the note near the
//! bottom of this file for why.

#![allow(clippy::module_name_repetitions)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::flac_bitstream::FlacStreamEncoder;
use crate::pipeline_context::{Frame, FrameDecoder, FrameEncoder};
use crate::{Result, TranscodeError};

/// Shared counter of sample-frames actually encoded, read by output sinks
/// at trailer time for exact stream headers (survives `-ss`/`-t` trims).
pub type SharedSampleCounter = Arc<AtomicU64>;

/// Sample-frames per `Frame` emitted by [`PcmBufferFrameDecoder`] and per
/// FLAC block encoded by [`FlacFrameEncoder`].
const PCM_CHUNK_FRAMES: usize = 4096;

// ─── PcmBufferFrameDecoder ────────────────────────────────────────────────────

/// A [`FrameDecoder`] over a fully-decoded interleaved i16 LE PCM buffer.
///
/// Emits [`Frame::audio`] chunks of `PCM_CHUNK_FRAMES` sample-frames with
/// PTS derived from the sample position, so downstream muxing gets real
/// timestamps.
pub struct PcmBufferFrameDecoder {
    pcm: Vec<u8>,
    sample_rate: u32,
    channels: u16,
    chunk_frames: usize,
    cursor: usize,
}

impl PcmBufferFrameDecoder {
    /// Creates a decoder over `pcm` (interleaved i16 LE) emitting
    /// `PCM_CHUNK_FRAMES`-sample chunks.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::InvalidInput`] if `sample_rate` or
    /// `channels` is zero, or if `pcm` is not a whole number of samples.
    pub fn new(pcm: Vec<u8>, sample_rate: u32, channels: u16) -> Result<Self> {
        Self::with_chunk_frames(pcm, sample_rate, channels, PCM_CHUNK_FRAMES)
    }

    /// Creates a decoder emitting `chunk_frames`-sample chunks — use when
    /// the downstream encoder needs exactly one codec frame per pipeline
    /// frame (e.g. Opus packets into Ogg, where packet boundaries are
    /// container-significant).
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::InvalidInput`] on zero parameters or a
    /// ragged PCM buffer.
    pub fn with_chunk_frames(
        pcm: Vec<u8>,
        sample_rate: u32,
        channels: u16,
        chunk_frames: usize,
    ) -> Result<Self> {
        if sample_rate == 0 || channels == 0 || chunk_frames == 0 {
            return Err(TranscodeError::InvalidInput(
                "audio decode: sample rate, channels, and chunk size must be non-zero".into(),
            ));
        }
        let bpf = usize::from(channels) * 2;
        if pcm.len() % bpf != 0 {
            return Err(TranscodeError::InvalidInput(format!(
                "audio decode: PCM byte length {} is not a multiple of the \
                 {bpf}-byte sample-frame size",
                pcm.len()
            )));
        }
        Ok(Self {
            pcm,
            sample_rate,
            channels,
            chunk_frames,
            cursor: 0,
        })
    }

    /// Bytes per interleaved sample-frame (all channels).
    fn bytes_per_frame(&self) -> usize {
        usize::from(self.channels) * 2
    }

    /// Total sample-frames in the buffer.
    #[must_use]
    pub fn total_sample_frames(&self) -> u64 {
        (self.pcm.len() / self.bytes_per_frame()) as u64
    }
}

impl FrameDecoder for PcmBufferFrameDecoder {
    fn decode_next(&mut self) -> Option<Frame> {
        if self.cursor >= self.pcm.len() {
            return None;
        }
        let bpf = self.bytes_per_frame();
        let chunk_bytes = self.chunk_frames * bpf;
        let end = (self.cursor + chunk_bytes).min(self.pcm.len());
        let frames_before = (self.cursor / bpf) as u64;
        let pts_ms = (frames_before.saturating_mul(1000) / u64::from(self.sample_rate)) as i64;
        let data = self.pcm[self.cursor..end].to_vec();
        self.cursor = end;
        Some(Frame::audio(data, pts_ms))
    }

    fn eof(&self) -> bool {
        self.cursor >= self.pcm.len()
    }
}

// ─── FlacFrameEncoder ─────────────────────────────────────────────────────────

/// A [`FrameEncoder`] producing spec-compliant raw FLAC frames via
/// [`crate::flac_bitstream::FlacStreamEncoder`] (verified against libFLAC
/// and FFmpeg — the other FLAC encoders in the workspace emit bitstreams
/// only their own decoders accept).
///
/// Output packets are container-less FLAC frames — pair with the container
/// crate's `FlacMuxer` (which writes the `fLaC` magic + STREAMINFO itself)
/// or prepend [`crate::flac_bitstream::stream_info_block`].
pub struct FlacFrameEncoder {
    inner: FlacStreamEncoder,
    channels: u16,
    /// Interleaved i16 LE bytes not yet submitted to the encoder.
    pending: Vec<u8>,
    /// Sample-frames encoded so far, shared with the output sink.
    samples_encoded: SharedSampleCounter,
}

impl FlacFrameEncoder {
    /// Creates a FLAC encoder for 16-bit interleaved PCM input.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported channel counts (1–8) or sample
    /// rates.
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self> {
        Ok(Self {
            inner: FlacStreamEncoder::new(sample_rate, channels)?,
            channels,
            pending: Vec::new(),
            samples_encoded: Arc::new(AtomicU64::new(0)),
        })
    }

    /// A handle to the exact number of sample-frames this encoder has
    /// produced; give it to the output sink for exact STREAMINFO totals.
    #[must_use]
    pub fn sample_counter(&self) -> SharedSampleCounter {
        Arc::clone(&self.samples_encoded)
    }

    /// Bytes per interleaved sample-frame.
    fn bytes_per_frame(&self) -> usize {
        usize::from(self.channels) * 2
    }

    /// Encode one block of interleaved i16 LE bytes into one FLAC frame.
    fn encode_block_bytes(&mut self, chunk: &[u8]) -> Result<Vec<u8>> {
        let samples: Vec<i16> = chunk
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let encoded = self.inner.encode_block(&samples)?;
        self.samples_encoded.fetch_add(
            (samples.len() / usize::from(self.channels)) as u64,
            Ordering::Relaxed,
        );
        Ok(encoded)
    }
}

impl FrameEncoder for FlacFrameEncoder {
    fn encode_frame(&mut self, frame: &Frame) -> Result<Vec<u8>> {
        self.pending.extend_from_slice(&frame.data);
        let block_bytes = PCM_CHUNK_FRAMES * self.bytes_per_frame();
        let mut out = Vec::new();
        while self.pending.len() >= block_bytes {
            let chunk: Vec<u8> = self.pending.drain(..block_bytes).collect();
            out.extend_from_slice(&self.encode_block_bytes(&chunk)?);
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<u8>> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        // Final short block — legal in a fixed-blocksize stream as the
        // last frame.
        let chunk = std::mem::take(&mut self.pending);
        self.encode_block_bytes(&chunk)
    }
}

// ─── PcmFrameEncoder ──────────────────────────────────────────────────────────

/// A [`FrameEncoder`] that passes interleaved i16 LE PCM through unchanged.
///
/// Pair with the container crate's `WavMuxer` (which writes RIFF headers
/// itself and expects raw PCM packet payloads).
pub struct PcmFrameEncoder;

impl PcmFrameEncoder {
    /// Creates a new PCM passthrough encoder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for PcmFrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameEncoder for PcmFrameEncoder {
    fn encode_frame(&mut self, frame: &Frame) -> Result<Vec<u8>> {
        Ok(frame.data.clone())
    }

    fn flush(&mut self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

// ─── AlacFrameEncoder ─────────────────────────────────────────────────────────

/// A [`FrameEncoder`] producing spec-compliant raw ALAC frames via
/// [`crate::alac_bitstream::AlacStreamEncoder`], which picks the escape
/// (uncompressed) or compressed (adaptive Rice + LPC) element form per
/// block automatically — both forms are verified against FFmpeg (see that
/// module's doc). The workspace's other ALAC encoder
/// (`oximedia_codec::alac::AlacEncoder`) is not used here: its compressed
/// elements are rejected by reference decoders.
///
/// Output packets are raw ALAC frames, exactly one 4096-sample packet per
/// non-empty payload (packet boundaries are container-significant for
/// CAF's packet table); the stream needs the encoder's magic cookie (see
/// [`AlacFrameEncoder::magic_cookie`]) as codec private data.
pub struct AlacFrameEncoder {
    inner: crate::alac_bitstream::AlacStreamEncoder,
    channels: u16,
    frame_length: usize,
    magic_cookie: Vec<u8>,
    /// Interleaved i16 LE bytes not yet submitted to the encoder.
    pending: Vec<u8>,
    /// Sample-frames encoded so far, shared with the output sink.
    samples_encoded: SharedSampleCounter,
}

impl AlacFrameEncoder {
    /// Creates an ALAC encoder for 16-bit interleaved PCM input.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported channel counts or sample rates.
    pub fn new(sample_rate: u32, channels: u16) -> Result<Self> {
        let inner = crate::alac_bitstream::AlacStreamEncoder::new(
            sample_rate,
            channels,
            PCM_CHUNK_FRAMES as u32,
        )?;
        let magic_cookie = inner.magic_cookie();
        Ok(Self {
            inner,
            channels,
            frame_length: PCM_CHUNK_FRAMES,
            magic_cookie,
            pending: Vec::new(),
            samples_encoded: Arc::new(AtomicU64::new(0)),
        })
    }

    /// The ALAC magic cookie (decoder configuration) for container
    /// codec-private data.
    #[must_use]
    pub fn magic_cookie(&self) -> &[u8] {
        &self.magic_cookie
    }

    /// A handle to the exact number of sample-frames this encoder has
    /// produced; give it to the output sink for exact packet-table totals.
    #[must_use]
    pub fn sample_counter(&self) -> SharedSampleCounter {
        Arc::clone(&self.samples_encoded)
    }

    fn bytes_per_frame(&self) -> usize {
        usize::from(self.channels) * 2
    }

    fn encode_block(&mut self, chunk: &[u8]) -> Result<Vec<u8>> {
        let samples: Vec<i16> = chunk
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let encoded = self.inner.encode_block(&samples)?;
        self.samples_encoded.fetch_add(
            (samples.len() / usize::from(self.channels)) as u64,
            Ordering::Relaxed,
        );
        Ok(encoded)
    }
}

impl FrameEncoder for AlacFrameEncoder {
    fn encode_frame(&mut self, frame: &Frame) -> Result<Vec<u8>> {
        self.pending.extend_from_slice(&frame.data);
        let block_bytes = self.frame_length * self.bytes_per_frame();
        let mut out = Vec::new();
        while self.pending.len() >= block_bytes {
            let chunk: Vec<u8> = self.pending.drain(..block_bytes).collect();
            out.extend_from_slice(&self.encode_block(&chunk)?);
        }
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<u8>> {
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let chunk = std::mem::take(&mut self.pending);
        self.encode_block(&chunk)
    }
}

// NOTE: There is intentionally no Opus adapter. Both the encoder and the
// decoder were evaluated for this pipeline this session and rejected:
// - Encoder: the audio-crate encoder serializes a custom non-spec payload,
//   and the codec-crate CELT encoder emits byte-identical packets
//   regardless of input (verified empirically against FFmpeg) — its output
//   is unverified against any reference decoder.
// - Decoder: the codec-crate CELT decoder fails to reconstruct real-world
//   CELT packets, decoding them to silence instead of audio.
// Neither direction is trustworthy, so transcoding to/from Opus returns a
// descriptive unsupported-codec error instead of fabricating output.
// TODO(0.2.x): wire a real Opus encoder AND decoder once both pass
// reference-decoder / real-packet verification, then re-enable the
// `.ogg`/`.opus` target in `frame_level`.

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sine_pcm_i16(freq_hz: f32, sample_rate: u32, channels: u16, frames: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(frames * usize::from(channels) * 2);
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let s = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
            let v = (s * 12000.0) as i16;
            for _ in 0..channels {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    // ── PcmBufferFrameDecoder ─────────────────────────────────────────────

    #[test]
    fn test_pcm_decoder_chunking_and_pts() {
        let sr = 8_000u32;
        let frames = PCM_CHUNK_FRAMES + 100;
        let pcm = sine_pcm_i16(440.0, sr, 1, frames);
        let mut dec = PcmBufferFrameDecoder::new(pcm, sr, 1).expect("decoder should build");
        assert_eq!(dec.total_sample_frames(), frames as u64);

        let f0 = dec.decode_next().expect("first chunk");
        assert!(f0.is_audio);
        assert_eq!(f0.pts_ms, 0);
        assert_eq!(f0.data.len(), PCM_CHUNK_FRAMES * 2);

        let f1 = dec.decode_next().expect("second chunk");
        // 4096 frames at 8 kHz = 512 ms.
        assert_eq!(f1.pts_ms, 512);
        assert_eq!(f1.data.len(), 100 * 2);

        assert!(dec.decode_next().is_none());
        assert!(dec.eof());
    }

    #[test]
    fn test_pcm_decoder_rejects_ragged_buffer() {
        assert!(PcmBufferFrameDecoder::new(vec![0u8; 5], 48_000, 2).is_err());
        assert!(PcmBufferFrameDecoder::new(vec![0u8; 4], 0, 2).is_err());
        assert!(PcmBufferFrameDecoder::new(vec![0u8; 4], 48_000, 0).is_err());
    }

    // ── FlacFrameEncoder ──────────────────────────────────────────────────

    #[test]
    fn test_flac_encoder_produces_flac_frames() {
        let sr = 48_000u32;
        let pcm = sine_pcm_i16(1000.0, sr, 2, PCM_CHUNK_FRAMES * 2 + 500);
        let mut enc = FlacFrameEncoder::new(sr, 2).expect("flac encoder");

        let mut encoded = Vec::new();
        encoded.extend_from_slice(&enc.encode_frame(&Frame::audio(pcm, 0)).expect("encode"));
        encoded.extend_from_slice(&enc.flush().expect("flush"));

        assert!(
            encoded.len() > 64,
            "FLAC output too small: {} bytes",
            encoded.len()
        );
        // FLAC frame sync code: 14 bits 0b11111111111110 → first byte 0xFF,
        // second byte top 6 bits 0b111110.
        assert_eq!(encoded[0], 0xFF, "must start with FLAC frame sync");
        assert_eq!(encoded[1] & 0xFC, 0xF8, "second sync byte mismatch");
    }

    #[test]
    fn test_flac_encoder_output_is_not_input_copy() {
        let sr = 44_100u32;
        let pcm = sine_pcm_i16(500.0, sr, 1, PCM_CHUNK_FRAMES);
        let mut enc = FlacFrameEncoder::new(sr, 1).expect("flac encoder");
        let mut out = enc
            .encode_frame(&Frame::audio(pcm.clone(), 0))
            .expect("encode");
        out.extend_from_slice(&enc.flush().expect("flush"));
        assert_ne!(out, pcm, "FLAC output must differ from raw PCM input");
        assert!(
            out.len() < pcm.len(),
            "a sine tone must compress: {} vs {} bytes",
            out.len(),
            pcm.len()
        );
    }

    // ── PcmFrameEncoder ───────────────────────────────────────────────────

    #[test]
    fn test_pcm_encoder_is_identity() {
        let mut enc = PcmFrameEncoder::new();
        let data = vec![1u8, 2, 3, 4];
        let out = enc
            .encode_frame(&Frame::audio(data.clone(), 0))
            .expect("encode");
        assert_eq!(out, data);
        assert!(enc.flush().expect("flush").is_empty());
    }

    // ── AlacFrameEncoder ──────────────────────────────────────────────────

    #[test]
    fn test_alac_encoder_packet_per_block() {
        let sr = 44_100u32;
        let pcm = sine_pcm_i16(880.0, sr, 2, PCM_CHUNK_FRAMES + 500);
        let mut enc = AlacFrameEncoder::new(sr, 2).expect("alac encoder");
        assert_eq!(enc.magic_cookie().len(), 24, "ALACSpecificConfig cookie");

        // A sine tone compresses well, so `AlacStreamEncoder::encode_block`
        // (self-verified against its own decoder, see alac_bitstream.rs)
        // is expected to prefer the compressed element form here — assert
        // real compression + real round-trip through the stream decoder,
        // not a byte-exact escape-coding size (which no longer applies
        // once the encoder is allowed to pick the smaller form).
        let full = enc
            .encode_frame(&Frame::audio(pcm[..PCM_CHUNK_FRAMES * 4].to_vec(), 0))
            .expect("encode full block");
        assert!(!full.is_empty(), "one packet per 4096 frames");
        let escape_size = (23 + PCM_CHUNK_FRAMES * 2 * 16 + 3).div_ceil(8);
        assert!(
            full.len() < escape_size,
            "a sine tone must compress smaller than escape coding: {} vs {escape_size}",
            full.len()
        );

        let tail = enc
            .encode_frame(&Frame::audio(pcm[PCM_CHUNK_FRAMES * 4..].to_vec(), 0))
            .expect("buffer partial");
        assert!(
            tail.is_empty(),
            "partial block is buffered, not flushed yet"
        );
        let tail = enc.flush().expect("flush");
        assert!(!tail.is_empty(), "short final packet");
        assert_eq!(
            enc.sample_counter()
                .load(std::sync::atomic::Ordering::Relaxed),
            (PCM_CHUNK_FRAMES + 500) as u64
        );

        // Round-trip both packets through the real stream decoder to
        // confirm this wrapper's chunking didn't corrupt anything.
        let pcm_i16: Vec<i16> = pcm
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let dec = crate::alac_bitstream::AlacStreamDecoder::new(2, PCM_CHUNK_FRAMES as u32)
            .expect("decoder");
        let decoded_full = dec.decode_block(&full).expect("decode full packet");
        assert_eq!(decoded_full, pcm_i16[..PCM_CHUNK_FRAMES * 2]);
        let decoded_tail = dec.decode_block(&tail).expect("decode tail packet");
        assert_eq!(decoded_tail, pcm_i16[PCM_CHUNK_FRAMES * 2..]);
    }
}
