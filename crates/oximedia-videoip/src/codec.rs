//! Video and audio codec bridge for the video-over-IP protocol.
//!
//! # Honest capability matrix
//!
//! This module is a thin bridge onto [`oximedia_codec`]. It only ever exposes
//! a codec direction that is actually backed by working code; every other
//! direction fails with [`VideoIpError::CodecUnimplemented`] naming the
//! precise gap. Raw frames are **never** handed off as a compressed
//! bitstream, and dimensions / sample counts are **never** invented.
//!
//! | Codec | Encode | Decode |
//! |-------|--------|--------|
//! | VP8 / VP9 / AV1 | honest `Err` — no working encoder exists | **real** (`oximedia-codec`, dimensions from the bitstream) |
//! | Opus | honest `Err` | honest `Err` |
//! | v210 / UYVY / `Yuv420p` / `Yuv420p10` | real (uncompressed passthrough) | real (dimensions from the negotiated [`VideoFormat`](crate::types::VideoFormat)) |
//! | PCM 16 / 24 / f32 | real (uncompressed passthrough) | real (sample count derived from the payload length) |
//!
//! ## Why the compressed encoders fail
//!
//! Each was checked by encoding a frame and feeding the result back through
//! `oximedia-codec`'s *own* decoder, plus (for VP8) an independent one:
//!
//! * **VP9** — `crates/oximedia-codec/src/vp9/tile_encoder.rs:522` writes
//!   "one zero byte per superblock" as the tile payload. Its own decoder
//!   rejects the packet.
//! * **AV1** — `crates/oximedia-codec/src/av1/encoder.rs:186` and
//!   `crates/oximedia-codec/src/av1/tile_encoder.rs:516` write a placeholder
//!   header and zero-filled tile data. Its own decoder rejects the packet.
//! * **VP8** — `crates/oximedia-codec/src/vp8/encoder.rs:470` codes only
//!   quantised DC values behind a placeholder frame tag. The packet decodes
//!   to a near-uniform gray image, and libvpx and `oximedia-codec` disagree
//!   on the reconstruction, so it is not a valid VP8 bitstream.
//! * **Opus** — `crates/oximedia-codec/src/opus/encoder.rs:452`
//!   (`encode_configuration`) computes a TOC configuration of 32..=35 for
//!   fullband CELT, which does not fit the 5-bit TOC field and is silently
//!   truncated by `config << 3`, mislabelling the packet.
//!
//! ## Why Opus decode fails
//!
//! `oximedia-codec`'s Opus decoder returns `Ok` with output that is not the
//! decoded signal: real libopus CELT packets decode to all-zero samples, and
//! real libopus SILK packets decode to out-of-range values at the wrong frame
//! size. Returning that as audio would be fabrication, so it is refused.

use crate::error::{VideoIpError, VideoIpResult};
use crate::types::{AudioCodec, Resolution, VideoCodec};
use bytes::{Bytes, BytesMut};
use oximedia_codec::traits::{DecoderConfig, VideoDecoder as CodecFrameDecoder};
use oximedia_codec::FrameType;
use oximedia_core::PixelFormat;

/// Video frame data.
///
/// `data` is the *transport* payload: for an uncompressed [`VideoCodec`] it is
/// the raw frame in that codec's byte layout; for a compressed codec it is the
/// coded access unit on the wire, and the decoded form produced by
/// [`create_video_decoder`] is tightly packed planar 8-bit 4:2:0 (I420, Y then
/// U then V, each row exactly `width` / `width / 2` bytes with no padding).
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// Frame data.
    pub data: Bytes,
    /// Frame width.
    pub width: u32,
    /// Frame height.
    pub height: u32,
    /// Whether this is a keyframe.
    pub is_keyframe: bool,
    /// Presentation timestamp in microseconds.
    pub pts: u64,
}

impl VideoFrame {
    /// Creates a new video frame.
    #[must_use]
    pub const fn new(data: Bytes, width: u32, height: u32, is_keyframe: bool, pts: u64) -> Self {
        Self {
            data,
            width,
            height,
            is_keyframe,
            pts,
        }
    }
}

/// Audio samples data.
#[derive(Debug, Clone)]
pub struct AudioSamples {
    /// Audio data.
    pub data: Bytes,
    /// Number of samples per channel.
    pub sample_count: usize,
    /// Number of channels.
    pub channels: u8,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Presentation timestamp in microseconds.
    pub pts: u64,
}

impl AudioSamples {
    /// Creates new audio samples.
    #[must_use]
    pub const fn new(
        data: Bytes,
        sample_count: usize,
        channels: u8,
        sample_rate: u32,
        pts: u64,
    ) -> Self {
        Self {
            data,
            sample_count,
            channels,
            sample_rate,
            pts,
        }
    }
}

/// Video encoder interface.
pub trait VideoEncoder: Send + Sync {
    /// Encodes a video frame.
    ///
    /// # Errors
    ///
    /// Returns an error if encoding fails.
    fn encode(&mut self, frame: &VideoFrame) -> VideoIpResult<Bytes>;

    /// Flushes any buffered frames.
    ///
    /// # Errors
    ///
    /// Returns an error if flushing fails.
    fn flush(&mut self) -> VideoIpResult<Vec<Bytes>>;
}

/// Video decoder interface.
pub trait VideoDecoder: Send + Sync {
    /// Decodes one complete access unit.
    ///
    /// `pts` and `packet_is_keyframe` come from the transport packet header.
    /// For compressed codecs the keyframe flag of the returned frame is taken
    /// from the *bitstream*, not from `packet_is_keyframe`, and so are the
    /// returned width and height.
    ///
    /// Returns `Ok(None)` when the decoder needs more data.
    ///
    /// # Errors
    ///
    /// Returns an error if decoding fails.
    fn decode(
        &mut self,
        data: &[u8],
        pts: u64,
        packet_is_keyframe: bool,
    ) -> VideoIpResult<Option<VideoFrame>>;
}

/// Audio encoder interface.
pub trait AudioEncoder: Send + Sync {
    /// Encodes audio samples.
    ///
    /// # Errors
    ///
    /// Returns an error if encoding fails.
    fn encode(&mut self, samples: &AudioSamples) -> VideoIpResult<Bytes>;
}

/// Audio decoder interface.
pub trait AudioDecoder: Send + Sync {
    /// Decodes audio data.
    ///
    /// `pts` comes from the transport packet header.
    ///
    /// # Errors
    ///
    /// Returns an error if decoding fails.
    fn decode(&mut self, data: &[u8], pts: u64) -> VideoIpResult<Option<AudioSamples>>;
}

/// Returns the exact size in bytes of one uncompressed frame.
///
/// Returns `None` for compressed codecs (whose frame size is not a function of
/// the resolution) and for resolutions whose frame size is not representable
/// as a `usize` on this target.
///
/// This is the single source of truth for uncompressed frame sizing in this
/// crate. Note that it models v210's real 128-byte row alignment, which only
/// coincides with the `VideoCodec::bytes_per_pixel()` estimate used by
/// [`VideoFormat::uncompressed_bitrate`](crate::types::VideoFormat::uncompressed_bitrate)
/// when the width is a multiple of 48.
#[must_use]
pub fn uncompressed_frame_size(codec: VideoCodec, width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 {
        return None;
    }
    let w = u64::from(width);
    let h = u64::from(height);
    let chroma = w.div_ceil(2) * h.div_ceil(2);
    let bytes = match codec {
        // Compressed: size is a property of the bitstream, not the geometry.
        VideoCodec::Vp9 | VideoCodec::Av1 | VideoCodec::Vp8 => return None,
        // 10-bit 4:2:2 packed: 6 pixels per 16 bytes, rows padded to 128 bytes.
        VideoCodec::V210 => w.div_ceil(48) * 128 * h,
        // 8-bit 4:2:2 packed: 2 bytes per pixel, pixel pairs share chroma.
        VideoCodec::Uyvy => w.div_ceil(2) * 2 * 2 * h,
        // 8-bit planar 4:2:0.
        VideoCodec::Yuv420p => w * h + 2 * chroma,
        // 10-bit planar 4:2:0, 2 bytes per sample.
        VideoCodec::Yuv420p10 => 2 * (w * h + 2 * chroma),
    };
    usize::try_from(bytes).ok()
}

/// Creates a video encoder for the specified codec.
///
/// Only the uncompressed codecs are supported: no working VP8/VP9/AV1 encoder
/// exists behind this crate (see the [module docs](self)), and emitting raw
/// frames labelled as a compressed bitstream would be a lie.
///
/// # Errors
///
/// Returns [`VideoIpError::CodecUnimplemented`] for VP8, VP9 and AV1, and
/// [`VideoIpError::InvalidVideoConfig`] if the resolution is unusable.
pub fn create_video_encoder(
    codec: VideoCodec,
    width: u32,
    height: u32,
    _bitrate: Option<u64>,
) -> VideoIpResult<Box<dyn VideoEncoder>> {
    match codec {
        VideoCodec::Vp9 => Err(vp9_encode_gap()),
        VideoCodec::Av1 => Err(av1_encode_gap()),
        VideoCodec::Vp8 => Err(vp8_encode_gap()),
        VideoCodec::V210 | VideoCodec::Uyvy | VideoCodec::Yuv420p | VideoCodec::Yuv420p10 => {
            // Uncompressed formats need no encoding, but the geometry still
            // has to describe a representable frame.
            if uncompressed_frame_size(codec, width, height).is_none() {
                return Err(VideoIpError::InvalidVideoConfig(format!(
                    "{codec:?}: {width}x{height} is not a usable uncompressed frame geometry"
                )));
            }
            Ok(Box::new(PassthroughVideoEncoder))
        }
    }
}

/// Creates a video decoder for the specified codec.
///
/// For the compressed codecs this wires `oximedia-codec`'s real decoders and
/// `resolution` is ignored — width and height come from the bitstream. For the
/// uncompressed codecs `resolution` is **required**, because the payload alone
/// does not carry its geometry and this crate will not invent one.
///
/// # Errors
///
/// Returns [`VideoIpError::InvalidVideoConfig`] if an uncompressed codec is
/// requested without a resolution, and [`VideoIpError::Codec`] if the backing
/// decoder cannot be constructed.
pub fn create_video_decoder(
    codec: VideoCodec,
    resolution: Option<Resolution>,
) -> VideoIpResult<Box<dyn VideoDecoder>> {
    let decoder_config = DecoderConfig::default();
    match codec {
        VideoCodec::Vp8 => Ok(Box::new(CompressedVideoDecoder::new(
            oximedia_codec::Vp8Decoder::new(decoder_config).map_err(codec_error)?,
            "VP8",
        ))),
        VideoCodec::Vp9 => Ok(Box::new(CompressedVideoDecoder::new(
            oximedia_codec::Vp9Decoder::new(decoder_config).map_err(codec_error)?,
            "VP9",
        ))),
        VideoCodec::Av1 => Ok(Box::new(CompressedVideoDecoder::new(
            oximedia_codec::Av1Decoder::new(decoder_config).map_err(codec_error)?,
            "AV1",
        ))),
        VideoCodec::V210 | VideoCodec::Uyvy | VideoCodec::Yuv420p | VideoCodec::Yuv420p10 => {
            let Some(resolution) = resolution else {
                return Err(VideoIpError::InvalidVideoConfig(format!(
                    "uncompressed {codec:?} decode needs the source resolution, which the codec \
                     alone does not carry; build the receiver from the announced VideoFormat \
                     (VideoIpReceiver::new/connect) instead of a bare codec"
                )));
            };
            let frame_size = uncompressed_frame_size(codec, resolution.width, resolution.height)
                .ok_or_else(|| {
                    VideoIpError::InvalidVideoConfig(format!(
                        "{codec:?}: {}x{} is not a usable uncompressed frame geometry",
                        resolution.width, resolution.height
                    ))
                })?;
            Ok(Box::new(RawVideoDecoder {
                codec,
                width: resolution.width,
                height: resolution.height,
                frame_size,
            }))
        }
    }
}

/// Creates an audio encoder for the specified codec.
///
/// # Errors
///
/// Returns [`VideoIpError::CodecUnimplemented`] for Opus (see the
/// [module docs](self)) and [`VideoIpError::InvalidAudioConfig`] for an
/// unusable PCM configuration.
pub fn create_audio_encoder(
    codec: AudioCodec,
    sample_rate: u32,
    channels: u8,
) -> VideoIpResult<Box<dyn AudioEncoder>> {
    match codec {
        AudioCodec::Opus => Err(opus_encode_gap()),
        AudioCodec::Pcm16 | AudioCodec::Pcm24 | AudioCodec::PcmF32 => {
            validate_pcm(sample_rate, channels)?;
            Ok(Box::new(PassthroughAudioEncoder))
        }
    }
}

/// Creates an audio decoder for the specified codec.
///
/// PCM decode derives the sample count from the payload length; nothing is
/// assumed about frame sizes.
///
/// # Errors
///
/// Returns [`VideoIpError::CodecUnimplemented`] for Opus (see the
/// [module docs](self)) and [`VideoIpError::InvalidAudioConfig`] for an
/// unusable PCM configuration.
pub fn create_audio_decoder(
    codec: AudioCodec,
    sample_rate: u32,
    channels: u8,
) -> VideoIpResult<Box<dyn AudioDecoder>> {
    match codec {
        AudioCodec::Opus => Err(opus_decode_gap()),
        AudioCodec::Pcm16 | AudioCodec::Pcm24 | AudioCodec::PcmF32 => {
            validate_pcm(sample_rate, channels)?;
            let bytes_per_sample = codec.bytes_per_sample().ok_or_else(|| {
                VideoIpError::InvalidAudioConfig(format!("{codec:?} has no fixed sample width"))
            })?;
            Ok(Box::new(RawAudioDecoder {
                sample_rate,
                channels,
                bytes_per_sample,
            }))
        }
    }
}

// ── Honest gaps ──────────────────────────────────────────────────────────────

fn vp9_encode_gap() -> VideoIpError {
    VideoIpError::CodecUnimplemented {
        codec: "VP9",
        operation: "encode",
        details: "oximedia-codec's VP9 encoder writes a placeholder tile payload (one zero byte \
                  per superblock, crates/oximedia-codec/src/vp9/tile_encoder.rs) and its output \
                  is rejected by oximedia-codec's own VP9 decoder (\"VP9 show_existing_frame: \
                  reference slot 0 holds no decoded frame\"). Transport uncompressed video \
                  (VideoCodec::Yuv420p / Uyvy / V210 / Yuv420p10) instead"
            .to_string(),
    }
}

fn av1_encode_gap() -> VideoIpError {
    VideoIpError::CodecUnimplemented {
        codec: "AV1",
        operation: "encode",
        details: "oximedia-codec's AV1 encoder writes a placeholder frame header and a \
                  zero-filled tile payload (crates/oximedia-codec/src/av1/encoder.rs, \
                  crates/oximedia-codec/src/av1/tile_encoder.rs); oximedia-codec's own AV1 \
                  decoder rejects the result (\"AV1: bitstream over-read in header\"). \
                  Transport uncompressed video (VideoCodec::Yuv420p / Uyvy / V210 / Yuv420p10) \
                  instead"
            .to_string(),
    }
}

fn vp8_encode_gap() -> VideoIpError {
    VideoIpError::CodecUnimplemented {
        codec: "VP8",
        operation: "encode",
        details: "oximedia-codec's VP8 encoder codes only quantised DC values behind a \
                  placeholder frame tag (crates/oximedia-codec/src/vp8/encoder.rs); its output \
                  decodes to a near-uniform gray image, and libvpx and oximedia-codec's own VP8 \
                  decoder reconstruct different images from the same bytes (10240 of 12288 luma \
                  samples differ), so it is not a valid VP8 bitstream. Transport uncompressed \
                  video (VideoCodec::Yuv420p / Uyvy / V210 / Yuv420p10) instead"
            .to_string(),
    }
}

fn opus_encode_gap() -> VideoIpError {
    VideoIpError::CodecUnimplemented {
        codec: "Opus",
        operation: "encode",
        details: "oximedia-codec's Opus encoder mislabels its packets: for the fullband CELT \
                  configuration it selects at 48 kHz, encode_configuration() \
                  (crates/oximedia-codec/src/opus/encoder.rs) computes a TOC configuration of \
                  32..=35, which does not fit the 5-bit TOC field and is silently truncated by \
                  `config << 3`, so the packet claims SILK/narrowband/60 ms and decodes to 3x \
                  the sample count. In the configurations where the sample count does match, \
                  the decoded signal is all-zero (correlation 0.0 with the input). Use \
                  AudioCodec::Pcm16 / Pcm24 / PcmF32 instead"
            .to_string(),
    }
}

fn opus_decode_gap() -> VideoIpError {
    VideoIpError::CodecUnimplemented {
        codec: "Opus",
        operation: "decode",
        details: "oximedia-codec's Opus decoder returns Ok with output it did not decode: 26 \
                  real libopus packets (CELT fullband, 20 ms, 48 kHz stereo) decode to 49920 \
                  samples of which 0 are non-zero, and real libopus SILK packets decode to \
                  out-of-range samples (max |x| = 4.0) at 960 samples per frame regardless of \
                  the configured 16 kHz rate. Use AudioCodec::Pcm16 / Pcm24 / PcmF32 instead"
            .to_string(),
    }
}

fn codec_error(err: oximedia_codec::CodecError) -> VideoIpError {
    VideoIpError::Codec(err.to_string())
}

fn validate_pcm(sample_rate: u32, channels: u8) -> VideoIpResult<()> {
    if sample_rate == 0 {
        return Err(VideoIpError::InvalidAudioConfig(
            "sample rate must be non-zero".to_string(),
        ));
    }
    if channels == 0 {
        return Err(VideoIpError::InvalidAudioConfig(
            "channel count must be non-zero".to_string(),
        ));
    }
    Ok(())
}

// ── Real implementations ─────────────────────────────────────────────────────

/// Decodes a compressed bitstream with `oximedia-codec`'s real decoder.
///
/// Generic over the concrete decoder so the wrapper inherits its `Send + Sync`
/// (`oximedia_codec::traits::VideoDecoder` itself only requires `Send`).
struct CompressedVideoDecoder<D> {
    inner: D,
    codec: &'static str,
}

impl<D: CodecFrameDecoder + Send + Sync> CompressedVideoDecoder<D> {
    const fn new(inner: D, codec: &'static str) -> Self {
        Self { inner, codec }
    }
}

impl<D: CodecFrameDecoder + Send + Sync> VideoDecoder for CompressedVideoDecoder<D> {
    fn decode(
        &mut self,
        data: &[u8],
        pts: u64,
        _packet_is_keyframe: bool,
    ) -> VideoIpResult<Option<VideoFrame>> {
        // u64 microseconds only exceeds i64::MAX beyond ~292 471 years.
        let codec_pts = i64::try_from(pts).unwrap_or(i64::MAX);
        self.inner
            .send_packet(data, codec_pts)
            .map_err(codec_error)?;
        let Some(frame) = self.inner.receive_frame().map_err(codec_error)? else {
            return Ok(None);
        };
        let payload = pack_planar(&frame, self.codec)?;
        Ok(Some(VideoFrame::new(
            payload,
            frame.width,
            frame.height,
            frame.frame_type == FrameType::Key,
            pts,
        )))
    }
}

/// Copies a decoded frame into a tightly packed I420 buffer (no row padding).
fn pack_planar(frame: &oximedia_codec::VideoFrame, codec: &'static str) -> VideoIpResult<Bytes> {
    if frame.format != PixelFormat::Yuv420p {
        return Err(VideoIpError::Codec(format!(
            "{codec} decoder produced {:?}, but this transport only carries 8-bit 4:2:0 \
             (PixelFormat::Yuv420p)",
            frame.format
        )));
    }
    if frame.planes.len() != 3 {
        return Err(VideoIpError::Codec(format!(
            "{codec} decoder produced {} planes, expected 3 (Y, U, V)",
            frame.planes.len()
        )));
    }

    let total: usize = frame
        .planes
        .iter()
        .map(|plane| plane.width as usize * plane.height as usize)
        .sum();
    let mut out = BytesMut::with_capacity(total);

    for (index, plane) in frame.planes.iter().enumerate() {
        let row_bytes = plane.width as usize;
        for row in 0..plane.height as usize {
            let start = row * plane.stride;
            let row_data = plane.data.get(start..start + row_bytes).ok_or_else(|| {
                VideoIpError::Codec(format!(
                    "{codec} decoder plane {index} is short: {} bytes for {}x{} at stride {}",
                    plane.data.len(),
                    plane.width,
                    plane.height,
                    plane.stride
                ))
            })?;
            out.extend_from_slice(row_data);
        }
    }

    Ok(out.freeze())
}

/// Uncompressed video "encoder": the payload already is the wire format.
struct PassthroughVideoEncoder;

impl VideoEncoder for PassthroughVideoEncoder {
    fn encode(&mut self, frame: &VideoFrame) -> VideoIpResult<Bytes> {
        Ok(frame.data.clone())
    }

    fn flush(&mut self) -> VideoIpResult<Vec<Bytes>> {
        Ok(Vec::new())
    }
}

/// Uncompressed video decoder.
///
/// The payload is already the picture, so "decoding" is validating that it has
/// exactly the size the negotiated geometry implies and stamping that geometry
/// on it. A mismatch is reported rather than papered over, because the width
/// and height attached to the frame would otherwise be a guess.
struct RawVideoDecoder {
    codec: VideoCodec,
    width: u32,
    height: u32,
    frame_size: usize,
}

impl VideoDecoder for RawVideoDecoder {
    fn decode(
        &mut self,
        data: &[u8],
        pts: u64,
        packet_is_keyframe: bool,
    ) -> VideoIpResult<Option<VideoFrame>> {
        if data.len() != self.frame_size {
            return Err(VideoIpError::InvalidPacket(format!(
                "uncompressed {:?} frame is {} bytes, but {}x{} requires exactly {}",
                self.codec,
                data.len(),
                self.width,
                self.height,
                self.frame_size
            )));
        }
        Ok(Some(VideoFrame::new(
            Bytes::copy_from_slice(data),
            self.width,
            self.height,
            packet_is_keyframe,
            pts,
        )))
    }
}

/// Uncompressed audio "encoder": the payload already is the wire format.
struct PassthroughAudioEncoder;

impl AudioEncoder for PassthroughAudioEncoder {
    fn encode(&mut self, samples: &AudioSamples) -> VideoIpResult<Bytes> {
        Ok(samples.data.clone())
    }
}

/// Uncompressed PCM decoder: the sample count is derived from the payload.
struct RawAudioDecoder {
    sample_rate: u32,
    channels: u8,
    bytes_per_sample: usize,
}

impl AudioDecoder for RawAudioDecoder {
    fn decode(&mut self, data: &[u8], pts: u64) -> VideoIpResult<Option<AudioSamples>> {
        let frame_bytes = self.bytes_per_sample * usize::from(self.channels);
        if frame_bytes == 0 || data.len() % frame_bytes != 0 {
            return Err(VideoIpError::InvalidPacket(format!(
                "PCM payload of {} bytes is not a whole number of {}-channel {}-byte samples",
                data.len(),
                self.channels,
                self.bytes_per_sample
            )));
        }
        Ok(Some(AudioSamples::new(
            Bytes::copy_from_slice(data),
            data.len() / frame_bytes,
            self.channels,
            self.sample_rate,
            pts,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every compressed codec must refuse to encode, and say why.
    #[test]
    fn compressed_video_encoders_fail_honestly() {
        for (codec, name) in [
            (VideoCodec::Vp9, "VP9"),
            (VideoCodec::Av1, "AV1"),
            (VideoCodec::Vp8, "VP8"),
        ] {
            let err = create_video_encoder(codec, 1920, 1080, None)
                .err()
                .expect("compressed encode must not be available");
            match err {
                VideoIpError::CodecUnimplemented {
                    codec: reported,
                    operation,
                    ref details,
                } => {
                    assert_eq!(reported, name);
                    assert_eq!(operation, "encode");
                    assert!(
                        details.contains("oximedia-codec"),
                        "{name} detail must name the backing gap: {details}"
                    );
                    assert!(
                        details.contains("uncompressed"),
                        "{name} detail must name the working alternative: {details}"
                    );
                }
                other => panic!("{name}: unexpected error {other}"),
            }
        }
    }

    #[test]
    fn opus_fails_honestly_in_both_directions() {
        let enc = create_audio_encoder(AudioCodec::Opus, 48000, 2)
            .err()
            .expect("opus encode must not be available");
        assert!(matches!(
            enc,
            VideoIpError::CodecUnimplemented {
                codec: "Opus",
                operation: "encode",
                ..
            }
        ));
        assert!(enc.to_string().contains("TOC"), "{enc}");

        let dec = create_audio_decoder(AudioCodec::Opus, 48000, 2)
            .err()
            .expect("opus decode must not be available");
        assert!(matches!(
            dec,
            VideoIpError::CodecUnimplemented {
                codec: "Opus",
                operation: "decode",
                ..
            }
        ));
        assert!(dec.to_string().contains("libopus"), "{dec}");
    }

    #[test]
    fn compressed_video_decoders_are_available() {
        for codec in [VideoCodec::Vp8, VideoCodec::Vp9, VideoCodec::Av1] {
            assert!(
                create_video_decoder(codec, None).is_ok(),
                "{codec:?} decode must be wired"
            );
        }
    }

    #[test]
    fn uncompressed_decode_requires_a_resolution() {
        let err = create_video_decoder(VideoCodec::Uyvy, None)
            .err()
            .expect("uncompressed decode without geometry must fail");
        assert!(matches!(err, VideoIpError::InvalidVideoConfig(_)));
        assert!(err.to_string().contains("resolution"), "{err}");

        assert!(create_video_decoder(VideoCodec::Uyvy, Some(Resolution::new(64, 48))).is_ok());
    }

    #[test]
    fn uncompressed_frame_sizes_are_exact() {
        assert_eq!(
            uncompressed_frame_size(VideoCodec::Yuv420p, 64, 48),
            Some(4608)
        );
        assert_eq!(
            uncompressed_frame_size(VideoCodec::Yuv420p10, 64, 48),
            Some(9216)
        );
        assert_eq!(
            uncompressed_frame_size(VideoCodec::Uyvy, 64, 48),
            Some(6144)
        );
        // v210 pads each row to a 128-byte boundary: 1920 px -> 40 * 128 bytes.
        assert_eq!(
            uncompressed_frame_size(VideoCodec::V210, 1920, 1080),
            Some(5_529_600)
        );
        // ...which is *not* width * 8/3 once the width is not a multiple of 48.
        assert_eq!(
            uncompressed_frame_size(VideoCodec::V210, 64, 48),
            Some(12288)
        );
        assert_eq!(uncompressed_frame_size(VideoCodec::Vp9, 64, 48), None);
        assert_eq!(uncompressed_frame_size(VideoCodec::Yuv420p, 0, 48), None);
    }

    #[test]
    fn raw_video_roundtrip_preserves_geometry_and_payload() {
        let resolution = Resolution::new(64, 48);
        let size = uncompressed_frame_size(VideoCodec::Yuv420p, 64, 48).expect("known size");
        let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();

        let mut encoder = create_video_encoder(VideoCodec::Yuv420p, 64, 48, None)
            .expect("uncompressed encode is available");
        let frame = VideoFrame::new(Bytes::from(payload.clone()), 64, 48, true, 12_345);
        let wire = encoder.encode(&frame).expect("passthrough encode");
        assert_eq!(&wire[..], &payload[..]);

        let mut decoder = create_video_decoder(VideoCodec::Yuv420p, Some(resolution))
            .expect("uncompressed decode is available");
        let decoded = decoder
            .decode(&wire, 12_345, true)
            .expect("passthrough decode")
            .expect("frame produced");
        assert_eq!(decoded.width, 64);
        assert_eq!(decoded.height, 48);
        assert_eq!(decoded.pts, 12_345);
        assert!(decoded.is_keyframe);
        assert_eq!(&decoded.data[..], &payload[..]);
    }

    #[test]
    fn raw_video_decode_rejects_a_mis_sized_frame() {
        let mut decoder = create_video_decoder(VideoCodec::Yuv420p, Some(Resolution::new(64, 48)))
            .expect("uncompressed decode is available");
        let err = decoder
            .decode(b"not a frame", 0, true)
            .expect_err("short payload must not be accepted");
        assert!(matches!(err, VideoIpError::InvalidPacket(_)));
        assert!(err.to_string().contains("4608"), "{err}");
    }

    #[test]
    fn pcm_decode_derives_the_sample_count() {
        let mut decoder =
            create_audio_decoder(AudioCodec::Pcm16, 44100, 2).expect("pcm decode is available");
        // 300 bytes = 150 stereo-interleaved 16-bit samples = 75 per channel.
        let samples = decoder
            .decode(&vec![7u8; 300], 999)
            .expect("pcm decode")
            .expect("samples produced");
        assert_eq!(samples.sample_count, 75);
        assert_eq!(samples.channels, 2);
        assert_eq!(samples.sample_rate, 44100);
        assert_eq!(samples.pts, 999);

        let err = decoder
            .decode(&vec![7u8; 301], 0)
            .expect_err("ragged payload must not be accepted");
        assert!(matches!(err, VideoIpError::InvalidPacket(_)));
    }

    #[test]
    fn pcm_24_bit_sample_count_uses_the_declared_width() {
        let mut decoder =
            create_audio_decoder(AudioCodec::Pcm24, 48000, 1).expect("pcm decode is available");
        let samples = decoder
            .decode(&vec![0u8; 300], 0)
            .expect("pcm decode")
            .expect("samples produced");
        assert_eq!(samples.sample_count, 100);
    }

    #[test]
    fn pcm_config_is_validated() {
        assert!(create_audio_decoder(AudioCodec::Pcm16, 0, 2).is_err());
        assert!(create_audio_decoder(AudioCodec::Pcm16, 48000, 0).is_err());
        assert!(create_audio_encoder(AudioCodec::Pcm16, 0, 2).is_err());
    }

    #[test]
    fn test_video_frame_creation() {
        let frame = VideoFrame::new(Bytes::from_static(b"data"), 1920, 1080, true, 12345);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert!(frame.is_keyframe);
        assert_eq!(frame.pts, 12345);
    }

    #[test]
    fn test_audio_samples_creation() {
        let samples = AudioSamples::new(Bytes::from_static(b"data"), 1024, 2, 48000, 12345);
        assert_eq!(samples.sample_count, 1024);
        assert_eq!(samples.channels, 2);
        assert_eq!(samples.sample_rate, 48000);
        assert_eq!(samples.pts, 12345);
    }
}
