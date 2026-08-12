// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Real video `FrameDecoder` / `FrameEncoder` adapters for the frame-level
//! transcode path.
//!
//! These adapters bridge the flat-buffer [`Frame`] representation used by
//! [`crate::pipeline_context`] (planar YUV 4:2:0: Y plane, then U, then V,
//! each row packed with no stride padding) and the plane-based
//! `oximedia_codec::frame::VideoFrame` consumed by the real
//! `oximedia_codec::traits::VideoEncoder` implementations.
//!
//! | Adapter                    | Role                                            |
//! |----------------------------|-------------------------------------------------|
//! | [`Y4mFrameDecoder`]        | lazy Y4M (YUV4MPEG2) file reader → raw frames   |
//! | [`Ffv1FrameDecoder`]       | lazy reader for [`crate::raw_sinks::Ffv1RawFileMuxer`] output → raw frames |
//! | [`FpsResamplingDecoder`]   | `-r` frame-rate conversion (dup/drop) wrapper   |
//! | [`CodecVideoFrameEncoder`] | any `oximedia_codec` `VideoEncoder` → packets   |

#![allow(clippy::module_name_repetitions)]

use std::collections::VecDeque;
use std::io::Read as _;

use crate::pipeline_context::{Frame, FrameDecoder, FrameEncoder};
use crate::pipeline_executor::TimestampManager;
use crate::{Result, TranscodeError};

use oximedia_codec::frame::{Plane, VideoFrame};
use oximedia_codec::traits::{VideoDecoder, VideoEncoder};
use oximedia_codec::Ffv1Decoder;
use oximedia_container::demux::y4m::{Y4mChroma, Y4mDemuxer};
use oximedia_core::{PixelFormat, Rational, Timestamp};

// ─── Layout helpers ───────────────────────────────────────────────────────────

/// Expected byte length of a flat YUV 4:2:0 frame (odd dims round chroma up).
#[must_use]
pub fn yuv420_frame_len(width: u32, height: u32) -> usize {
    let y = (width as usize) * (height as usize);
    let cw = width.div_ceil(2) as usize;
    let ch = height.div_ceil(2) as usize;
    y + 2 * cw * ch
}

/// Split a flat YUV 4:2:0 buffer into a plane-based `VideoFrame`.
///
/// # Errors
///
/// Returns [`TranscodeError::CodecError`] if the buffer length does not
/// match the expected planar 4:2:0 size for the given dimensions.
pub fn flat_yuv420_to_video_frame(
    data: &[u8],
    width: u32,
    height: u32,
    pts_ms: i64,
) -> Result<VideoFrame> {
    let expected = yuv420_frame_len(width, height);
    if data.len() != expected {
        return Err(TranscodeError::CodecError(format!(
            "video frame buffer is {} bytes but {width}x{height} YUV 4:2:0 requires {expected}",
            data.len()
        )));
    }
    let y_size = (width as usize) * (height as usize);
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let c_size = (cw as usize) * (ch as usize);

    let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
    frame.planes = vec![
        Plane::with_dimensions(data[..y_size].to_vec(), width as usize, width, height),
        Plane::with_dimensions(data[y_size..y_size + c_size].to_vec(), cw as usize, cw, ch),
        Plane::with_dimensions(data[y_size + c_size..].to_vec(), cw as usize, cw, ch),
    ];
    frame.timestamp = Timestamp::new(pts_ms, Rational::new(1, 1_000));
    Ok(frame)
}

/// Flattens a decoded 8-bit YUV 4:2:0 `VideoFrame` (Y, then U, then V
/// planes) into the same no-stride-padding layout [`flat_yuv420_to_video_frame`]
/// consumes — the inverse conversion, used by video decoders (e.g.
/// [`Ffv1FrameDecoder`]) feeding the flat-buffer [`Frame`] pipeline.
///
/// # Errors
///
/// Returns [`TranscodeError::CodecError`] if the frame is not 8-bit
/// `Yuv420p`, does not have exactly 3 planes, or any plane's stride is
/// wider than its width (this pipeline has no use for stride padding and
/// would otherwise silently fold garbage padding bytes into the output).
fn video_frame_to_flat_yuv420(frame: &VideoFrame) -> Result<Vec<u8>> {
    if frame.format != PixelFormat::Yuv420p {
        return Err(TranscodeError::CodecError(format!(
            "expected 8-bit Yuv420p, decoder produced {:?}",
            frame.format
        )));
    }
    if frame.planes.len() != 3 {
        return Err(TranscodeError::CodecError(format!(
            "expected 3 YUV planes, decoder produced {}",
            frame.planes.len()
        )));
    }
    let expected = yuv420_frame_len(frame.width, frame.height);
    let mut out = Vec::with_capacity(expected);
    for (i, plane) in frame.planes.iter().enumerate() {
        if plane.stride != plane.width as usize {
            return Err(TranscodeError::CodecError(format!(
                "plane {i} has stride {} but width {} (padded planes are not supported)",
                plane.stride, plane.width
            )));
        }
        out.extend_from_slice(&plane.data);
    }
    if out.len() != expected {
        return Err(TranscodeError::CodecError(format!(
            "assembled {} bytes but {}x{} YUV 4:2:0 requires {expected}",
            out.len(),
            frame.width,
            frame.height
        )));
    }
    Ok(out)
}

// ─── Y4mFrameDecoder ──────────────────────────────────────────────────────────

/// A [`FrameDecoder`] that lazily reads raw YUV 4:2:0 frames from a Y4M
/// (YUV4MPEG2) file.
///
/// Y4M frame payloads are already planar Y/Cb/Cr with no stride padding —
/// exactly the flat layout [`Frame`] and the `FilterGraph` video-scale op
/// expect — so no pixel conversion is performed.
pub struct Y4mFrameDecoder {
    demux: Y4mDemuxer<std::fs::File>,
    width: u32,
    height: u32,
    fps: (u32, u32),
    frame_index: u64,
    done: bool,
}

impl Y4mFrameDecoder {
    /// Opens `path` and parses the Y4M header.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::InvalidInput`] if the file cannot be
    /// opened or is not a 4:2:0 Y4M stream (4:2:2 / 4:4:4 / mono inputs
    /// are not supported by the flat-YUV420 pipeline yet).
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let file = std::fs::File::open(path).map_err(|e| {
            TranscodeError::IoError(format!("cannot open Y4M input '{}': {e}", path.display()))
        })?;
        let demux = Y4mDemuxer::new(file).map_err(|e| {
            TranscodeError::InvalidInput(format!(
                "'{}' is not a valid Y4M stream: {e}",
                path.display()
            ))
        })?;
        let chroma = demux.chroma();
        if !matches!(
            chroma,
            Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv
        ) {
            return Err(TranscodeError::Unsupported(format!(
                "Y4M chroma format {chroma:?} is not yet supported for transcode \
                 (only 4:2:0 variants are)"
            )));
        }
        let (fps_num, fps_den) = demux.fps();
        Ok(Self {
            width: demux.width(),
            height: demux.height(),
            fps: (fps_num.max(1), fps_den.max(1)),
            demux,
            frame_index: 0,
            done: false,
        })
    }

    /// Source frame dimensions.
    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Source frame rate as a `(numerator, denominator)` pair.
    #[must_use]
    pub fn fps(&self) -> (u32, u32) {
        self.fps
    }
}

impl FrameDecoder for Y4mFrameDecoder {
    fn decode_next(&mut self) -> Option<Frame> {
        if self.done {
            return None;
        }
        match self.demux.read_frame() {
            Ok(Some(data)) => {
                // PTS from the frame index and header frame rate.
                let pts_ms = (self
                    .frame_index
                    .saturating_mul(1_000)
                    .saturating_mul(u64::from(self.fps.1))
                    / u64::from(self.fps.0)) as i64;
                self.frame_index += 1;
                Some(Frame::video(data, pts_ms, self.width, self.height))
            }
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                // The trait has no error channel; a truncated tail frame is
                // treated as end-of-stream rather than fabricating data.
                tracing::warn!("Y4M read error treated as EOF: {e}");
                self.done = true;
                None
            }
        }
    }

    fn eof(&self) -> bool {
        self.done
    }
}

// ─── Ffv1FrameDecoder ─────────────────────────────────────────────────────────

/// A [`FrameDecoder`] over [`crate::raw_sinks::Ffv1RawFileMuxer`] output.
///
/// Every packet is decoded with a **fresh** context-state via
/// [`VideoDecoder::reset`] between frames: the frame-level FFV1 encoder
/// always configures `keyint = 1` (every packet is an independently
/// intra-coded frame — see `codec_dispatch::make_ffv1_encoder`), so
/// correctness never depends on adaptive range-coder state surviving
/// across packets. Resetting proactively sidesteps relying on the
/// decoder's own internal keyframe bookkeeping.
pub struct Ffv1FrameDecoder {
    decoder: Ffv1Decoder,
    width: u32,
    height: u32,
    fps: (u32, u32),
    /// Remaining `[len][payload]`-framed packets, in file order.
    remaining: VecDeque<Vec<u8>>,
    frame_index: u64,
    done: bool,
}

impl Ffv1FrameDecoder {
    /// Opens a [`crate::raw_sinks::Ffv1RawFileMuxer`]-written file and
    /// parses its header + frame table.
    ///
    /// # Errors
    ///
    /// Returns [`TranscodeError::InvalidInput`] if the file cannot be read,
    /// is too short, does not start with the expected magic, or is
    /// internally truncated; [`TranscodeError::Unsupported`] for an
    /// unrecognized format version; [`TranscodeError::CodecError`] if the
    /// embedded extradata cannot initialize an [`Ffv1Decoder`].
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|mut f| f.read_to_end(&mut bytes))
            .map_err(|e| {
                TranscodeError::IoError(format!(
                    "cannot open raw FFV1 input '{}': {e}",
                    path.display()
                ))
            })?;

        let magic = crate::raw_sinks::FFV1_RAW_MAGIC;
        if bytes.len() < magic.len() + 1 + 16 + 4 || bytes[..magic.len()] != magic[..] {
            return Err(TranscodeError::InvalidInput(format!(
                "'{}' is not a raw FFV1 file written by Ffv1RawFileMuxer (bad magic)",
                path.display()
            )));
        }
        let version = bytes[magic.len()];
        if version != 1 {
            return Err(TranscodeError::Unsupported(format!(
                "raw FFV1 file format version {version} is not supported \
                 (this build writes/reads version 1)"
            )));
        }

        let read_u32 = |b: &[u8], o: usize| -> u32 {
            u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
        };

        let mut off = magic.len() + 1;
        let width = read_u32(&bytes, off);
        off += 4;
        let height = read_u32(&bytes, off);
        off += 4;
        let fps_num = read_u32(&bytes, off);
        off += 4;
        let fps_den = read_u32(&bytes, off);
        off += 4;
        let extradata_len = read_u32(&bytes, off) as usize;
        off += 4;
        if off + extradata_len > bytes.len() {
            return Err(TranscodeError::InvalidInput(format!(
                "'{}' is truncated: extradata declares {extradata_len} bytes past EOF",
                path.display()
            )));
        }
        let extradata = &bytes[off..off + extradata_len];
        off += extradata_len;

        let decoder = Ffv1Decoder::with_extradata(extradata)
            .map_err(|e| TranscodeError::CodecError(format!("FFV1 extradata rejected: {e}")))?;

        let mut remaining = VecDeque::new();
        while off + 4 <= bytes.len() {
            let len = read_u32(&bytes, off) as usize;
            off += 4;
            if off + len > bytes.len() {
                return Err(TranscodeError::InvalidInput(format!(
                    "'{}' is truncated: a frame declares {len} bytes past EOF",
                    path.display()
                )));
            }
            remaining.push_back(bytes[off..off + len].to_vec());
            off += len;
        }

        Ok(Self {
            decoder,
            width,
            height,
            fps: (fps_num.max(1), fps_den.max(1)),
            remaining,
            frame_index: 0,
            done: false,
        })
    }

    /// Source frame dimensions.
    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Source frame rate as a `(numerator, denominator)` pair.
    #[must_use]
    pub fn fps(&self) -> (u32, u32) {
        self.fps
    }
}

impl FrameDecoder for Ffv1FrameDecoder {
    fn decode_next(&mut self) -> Option<Frame> {
        if self.done {
            return None;
        }
        let Some(packet) = self.remaining.pop_front() else {
            self.done = true;
            return None;
        };
        let pts_ms = (self
            .frame_index
            .saturating_mul(1_000)
            .saturating_mul(u64::from(self.fps.1))
            / u64::from(self.fps.0)) as i64;
        self.frame_index += 1;

        // Proactive per-frame reset — see the struct doc for why.
        self.decoder.reset();
        if let Err(e) = self.decoder.send_packet(&packet, pts_ms) {
            tracing::warn!("FFV1 decode error treated as EOF: {e}");
            self.done = true;
            return None;
        }
        let frame = match self.decoder.receive_frame() {
            Ok(Some(f)) => f,
            Ok(None) => {
                tracing::warn!("FFV1 decoder produced no frame for a packet; treating as EOF");
                self.done = true;
                return None;
            }
            Err(e) => {
                tracing::warn!("FFV1 decode error treated as EOF: {e}");
                self.done = true;
                return None;
            }
        };
        match video_frame_to_flat_yuv420(&frame) {
            Ok(data) => Some(Frame::video(data, pts_ms, self.width, self.height)),
            Err(e) => {
                tracing::warn!("FFV1 decoded frame rejected: {e}");
                self.done = true;
                None
            }
        }
    }

    fn eof(&self) -> bool {
        self.done || self.remaining.is_empty()
    }
}

// ─── FpsResamplingDecoder ─────────────────────────────────────────────────────

/// A [`FrameDecoder`] wrapper implementing `-r` frame-rate conversion at the
/// decoder-pump level: frames from the inner decoder are duplicated
/// (up-conversion) or dropped (down-conversion) using the same
/// [`TimestampManager`] arithmetic as the packet pipeline's
/// `FrameRateConverter`, and output PTS values are regenerated on the
/// target-rate grid.
///
/// Audio frames pass through untouched.
pub struct FpsResamplingDecoder {
    inner: Box<dyn FrameDecoder>,
    ts: TimestampManager,
    input_index: u64,
    queue: VecDeque<Frame>,
}

impl FpsResamplingDecoder {
    /// Wraps `inner`, converting from `input_fps` to `output_fps`
    /// (both `(num, den)` pairs).
    #[must_use]
    pub fn new(
        inner: Box<dyn FrameDecoder>,
        input_fps: (u32, u32),
        output_fps: (u32, u32),
    ) -> Self {
        Self {
            inner,
            ts: TimestampManager::new(input_fps, output_fps),
            input_index: 0,
            queue: VecDeque::new(),
        }
    }
}

impl FrameDecoder for FpsResamplingDecoder {
    fn decode_next(&mut self) -> Option<Frame> {
        if let Some(f) = self.queue.pop_front() {
            return Some(f);
        }
        loop {
            let frame = self.inner.decode_next()?;
            if frame.is_audio {
                return Some(frame);
            }
            let count = self.ts.frames_at_boundary(self.input_index);
            self.input_index += 1;
            if count == 0 {
                // Down-conversion: this source frame produces no output.
                continue;
            }
            for _ in 0..count {
                let mut dup = frame.clone();
                // TimestampManager works in microseconds.
                dup.pts_ms = self.ts.map_pts(frame.pts_ms.saturating_mul(1_000)) / 1_000;
                self.queue.push_back(dup);
            }
            // `count >= 1`, so the queue is non-empty here.
            return self.queue.pop_front();
        }
    }

    fn eof(&self) -> bool {
        self.queue.is_empty() && self.inner.eof()
    }
}

// ─── CodecVideoFrameEncoder ───────────────────────────────────────────────────

/// A [`FrameEncoder`] that drives any real `oximedia_codec` [`VideoEncoder`]
/// (MJPEG, APV, MPEG-2, FFV1, …) from flat YUV 4:2:0 frames.
///
/// The encoder must have been configured for the exact frame dimensions
/// this adapter will receive (i.e. post-`-vf scale` dimensions); mismatched
/// frames are rejected rather than silently rescaled.
pub struct CodecVideoFrameEncoder {
    inner: Box<dyn VideoEncoder>,
    width: u32,
    height: u32,
}

impl CodecVideoFrameEncoder {
    /// Wraps `inner`, which must accept `width`×`height` YUV 4:2:0 frames.
    #[must_use]
    pub fn new(inner: Box<dyn VideoEncoder>, width: u32, height: u32) -> Self {
        Self {
            inner,
            width,
            height,
        }
    }

    /// Drains every pending packet from the inner encoder.
    fn drain_packets(&mut self, out: &mut Vec<u8>) -> Result<()> {
        while let Some(pkt) = self
            .inner
            .receive_packet()
            .map_err(|e| TranscodeError::CodecError(format!("video encode failed: {e}")))?
        {
            out.extend_from_slice(&pkt.data);
        }
        Ok(())
    }
}

impl FrameEncoder for CodecVideoFrameEncoder {
    fn encode_frame(&mut self, frame: &Frame) -> Result<Vec<u8>> {
        if frame.is_audio {
            return Err(TranscodeError::CodecError(
                "video encoder received an audio frame".into(),
            ));
        }
        if frame.width != self.width || frame.height != self.height {
            return Err(TranscodeError::CodecError(format!(
                "video encoder configured for {}x{} received a {}x{} frame",
                self.width, self.height, frame.width, frame.height
            )));
        }
        let vf = flat_yuv420_to_video_frame(&frame.data, frame.width, frame.height, frame.pts_ms)?;
        self.inner
            .send_frame(&vf)
            .map_err(|e| TranscodeError::CodecError(format!("video encode failed: {e}")))?;
        let mut out = Vec::new();
        self.drain_packets(&mut out)?;
        Ok(out)
    }

    fn flush(&mut self) -> Result<Vec<u8>> {
        self.inner
            .flush()
            .map_err(|e| TranscodeError::CodecError(format!("video encoder flush failed: {e}")))?;
        let mut out = Vec::new();
        self.drain_packets(&mut out)?;
        Ok(out)
    }
}

// ─── RawVideoFrameEncoder ─────────────────────────────────────────────────────

/// A [`FrameEncoder`] that passes raw planar YUV 4:2:0 bytes through
/// unchanged — used for Y4M (rawvideo) output where the container carries
/// uncompressed frames.
pub struct RawVideoFrameEncoder;

impl RawVideoFrameEncoder {
    /// Creates a new raw passthrough encoder.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Default for RawVideoFrameEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameEncoder for RawVideoFrameEncoder {
    fn encode_frame(&mut self, frame: &Frame) -> Result<Vec<u8>> {
        Ok(frame.data.clone())
    }

    fn flush(&mut self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "mjpeg")]
    use crate::pipeline_context::FilterGraph;

    /// Build a flat YUV420 frame with distinct plane fills.
    fn flat_yuv420(w: u32, h: u32, y: u8, u: u8, v: u8) -> Vec<u8> {
        let y_size = (w * h) as usize;
        let c_size = (w.div_ceil(2) * h.div_ceil(2)) as usize;
        let mut data = vec![y; y_size];
        data.extend(std::iter::repeat_n(u, c_size));
        data.extend(std::iter::repeat_n(v, c_size));
        data
    }

    /// In-memory FrameDecoder for fps-wrapper tests.
    struct SeqDecoder {
        frames: VecDeque<Frame>,
    }

    impl FrameDecoder for SeqDecoder {
        fn decode_next(&mut self) -> Option<Frame> {
            self.frames.pop_front()
        }
        fn eof(&self) -> bool {
            self.frames.is_empty()
        }
    }

    #[test]
    fn test_yuv420_frame_len() {
        assert_eq!(yuv420_frame_len(4, 4), 16 + 2 * 4);
        assert_eq!(yuv420_frame_len(5, 3), 15 + 2 * (3 * 2));
    }

    #[test]
    fn test_flat_to_video_frame_planes() {
        let data = flat_yuv420(4, 2, 10, 20, 30);
        let vf = flat_yuv420_to_video_frame(&data, 4, 2, 123).expect("convert");
        assert_eq!(vf.width, 4);
        assert_eq!(vf.height, 2);
        assert_eq!(vf.planes.len(), 3);
        assert!(vf.planes[0].data.iter().all(|&b| b == 10));
        assert!(vf.planes[1].data.iter().all(|&b| b == 20));
        assert!(vf.planes[2].data.iter().all(|&b| b == 30));
        assert_eq!(vf.planes[1].data.len(), 2);
        assert_eq!(vf.timestamp.pts, 123);
    }

    #[test]
    fn test_flat_to_video_frame_rejects_bad_len() {
        let data = vec![0u8; 10];
        assert!(flat_yuv420_to_video_frame(&data, 4, 4, 0).is_err());
    }

    #[test]
    fn test_fps_resampler_upconversion_duplicates() {
        // 2 fps → 4 fps: every input frame produces two output frames.
        let frames: VecDeque<Frame> = (0..4)
            .map(|i| Frame::video(flat_yuv420(2, 2, i as u8, 0, 0), i64::from(i) * 500, 2, 2))
            .collect();
        let inner = Box::new(SeqDecoder { frames });
        let mut dec = FpsResamplingDecoder::new(inner, (2, 1), (4, 1));

        let mut out = Vec::new();
        while let Some(f) = dec.decode_next() {
            out.push(f);
        }
        assert_eq!(out.len(), 8, "2→4 fps must double the frame count");
        // PTS must be on the 250 ms output grid.
        assert_eq!(out[0].pts_ms, 0);
        assert_eq!(out[1].pts_ms, 250);
        assert_eq!(out[2].pts_ms, 500);
        assert!(dec.eof());
    }

    #[test]
    fn test_fps_resampler_downconversion_drops() {
        // 4 fps → 2 fps: half the frames are dropped.
        let frames: VecDeque<Frame> = (0..8)
            .map(|i| Frame::video(flat_yuv420(2, 2, i as u8, 0, 0), i64::from(i) * 250, 2, 2))
            .collect();
        let inner = Box::new(SeqDecoder { frames });
        let mut dec = FpsResamplingDecoder::new(inner, (4, 1), (2, 1));

        let mut out = Vec::new();
        while let Some(f) = dec.decode_next() {
            out.push(f);
        }
        assert_eq!(out.len(), 4, "4→2 fps must halve the frame count");
    }

    #[test]
    fn test_fps_resampler_passthrough_same_rate() {
        let frames: VecDeque<Frame> = (0..5)
            .map(|i| Frame::video(flat_yuv420(2, 2, i as u8, 0, 0), i64::from(i) * 40, 2, 2))
            .collect();
        let inner = Box::new(SeqDecoder { frames });
        let mut dec = FpsResamplingDecoder::new(inner, (25, 1), (25, 1));
        let mut n = 0;
        while dec.decode_next().is_some() {
            n += 1;
        }
        assert_eq!(n, 5);
    }

    #[cfg(feature = "mjpeg")]
    #[test]
    fn test_codec_video_encoder_mjpeg_end_to_end() {
        use crate::codec_dispatch::{make_video_encoder, VideoEncoderParams};
        use oximedia_core::CodecId;

        let (w, h) = (32u32, 32u32);
        let params = VideoEncoderParams::new(w, h, 85).expect("params");
        let inner = make_video_encoder(CodecId::Mjpeg, &params).expect("mjpeg encoder");
        let mut enc = CodecVideoFrameEncoder::new(inner, w, h);

        let frame = Frame::video(flat_yuv420(w, h, 128, 128, 128), 0, w, h);
        let bytes = enc.encode_frame(&frame).expect("encode");
        assert!(bytes.len() > 4, "JPEG output too small");
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "must start with JPEG SOI");
        assert!(enc.flush().expect("flush").is_empty());
    }

    #[cfg(feature = "mjpeg")]
    #[test]
    fn test_codec_video_encoder_rejects_wrong_dims() {
        use crate::codec_dispatch::{make_video_encoder, VideoEncoderParams};
        use oximedia_core::CodecId;

        let params = VideoEncoderParams::new(32, 32, 85).expect("params");
        let inner = make_video_encoder(CodecId::Mjpeg, &params).expect("mjpeg encoder");
        let mut enc = CodecVideoFrameEncoder::new(inner, 32, 32);
        let frame = Frame::video(flat_yuv420(16, 16, 0, 0, 0), 0, 16, 16);
        assert!(enc.encode_frame(&frame).is_err());
    }

    #[cfg(feature = "mjpeg")]
    #[test]
    fn test_scale_then_encode_chain() {
        use crate::codec_dispatch::{make_video_encoder, VideoEncoderParams};
        use oximedia_core::CodecId;

        // Source 64x64 → FilterGraph scale to 32x32 → MJPEG encode.
        let fg = FilterGraph::new().add_video_scale(32, 32);
        let src = Frame::video(flat_yuv420(64, 64, 200, 100, 50), 0, 64, 64);
        let scaled = fg.apply(src).expect("scale apply").expect("frame survives");
        assert_eq!(scaled.width, 32);
        assert_eq!(scaled.height, 32);

        let params = VideoEncoderParams::new(32, 32, 85).expect("params");
        let inner = make_video_encoder(CodecId::Mjpeg, &params).expect("mjpeg encoder");
        let mut enc = CodecVideoFrameEncoder::new(inner, 32, 32);
        let bytes = enc.encode_frame(&scaled).expect("encode scaled frame");
        assert_eq!(&bytes[..2], &[0xFF, 0xD8]);
    }
}
