//! VP8 decoder implementation.
//!
//! This module provides the main VP8 decoder that implements the
//! `VideoDecoder` trait from this crate.
//!
//! VP8 is a royalty-free video codec developed by Google as part of
//! the `WebM` project. This decoder is based on RFC 6386.
//!
//! # Status: key frames and inter frames both decode to pixels
//!
//! Key frames (intra frames) are **fully decoded to pixels**: boolean
//! entropy decode, header parse, macroblock mode/token decode, per-segment
//! dequantisation, inverse DCT/WHT, all intra prediction modes, and the
//! in-loop deblocking filter (the private `vp8::dec` pipeline, ported
//! from the production-verified `oximedia-image` WebP/VP8 decoder).
//!
//! Inter frames (P-frames) are decoded by the same pipeline plus the
//! motion-vector entropy decode (RFC 6386 §17), the per-macroblock
//! prediction records (§16), sub-pixel motion compensation (§18) and
//! last/golden/altref reference-frame management (§9.7-§9.8). Both frame
//! types are verified bit-exact against libvpx on the multi-frame
//! conformance fixtures in `vp8/dec/testdata`.
//!
//! # Hidden frames
//!
//! A frame whose tag carries `show_frame == 0` (RFC 6386 §9.1) — the
//! invisible alternate-reference frame a two-pass encoder inserts — is fully
//! decoded and updates the reference buffers, but is **not** part of the
//! displayed sequence. [`Vp8Decoder::send_packet`] returns `Ok(())` for it
//! and queues nothing, so a packet-in/frame-out loop sees one fewer frame
//! than packets. No placeholder or duplicated frame is fabricated.

use crate::error::{CodecError, CodecResult};
use crate::frame::{FrameType as VideoFrameType, Plane, VideoFrame};
use crate::traits::{DecoderConfig, VideoDecoder};
use crate::vp8::dec;
use crate::vp8::frame_header::FrameHeader;
use oximedia_core::{CodecId, PixelFormat};

/// VP8 decoder (key and inter frames decode to pixels).
///
/// A pure Rust VP8 decoder based on RFC 6386. Frames are fully reconstructed
/// into YUV 4:2:0 [`VideoFrame`]s; inter frames predict from the last,
/// golden and altref reference buffers the decoder maintains across
/// [`Vp8Decoder::send_packet`] calls. A frame with `show_frame == 0` is
/// decoded and retained as a reference but never emitted (see the module
/// docs).
///
/// # Examples
///
/// ```
/// use oximedia_codec::vp8::Vp8Decoder;
/// use oximedia_codec::traits::{DecoderConfig, VideoDecoder};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = DecoderConfig::default();
/// let mut decoder = Vp8Decoder::new(config)?;
///
/// // Decoder is ready to receive packets
/// assert!(decoder.dimensions().is_none());
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Vp8Decoder {
    /// Decoder configuration.
    #[allow(dead_code)]
    config: DecoderConfig,
    /// Current frame width.
    width: Option<u32>,
    /// Current frame height.
    height: Option<u32>,
    /// Pending frame output queue (`VideoDecoder` push/pull contract).
    output_queue: Vec<VideoFrame>,
    /// Whether the decoder is in flush mode.
    flushing: bool,
    /// Cross-frame decode state: entropy context, segmentation, loop-filter
    /// deltas and the last/golden/altref reference surfaces.
    sequence: dec::Vp8SequenceDecoder,
}

impl Vp8Decoder {
    /// Creates a new VP8 decoder with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - Decoder configuration
    ///
    /// # Errors
    ///
    /// Returns an error if the configuration is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_codec::vp8::Vp8Decoder;
    /// use oximedia_codec::traits::DecoderConfig;
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let config = DecoderConfig::default();
    /// let decoder = Vp8Decoder::new(config)?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn new(config: DecoderConfig) -> CodecResult<Self> {
        Ok(Self {
            config,
            width: None,
            height: None,
            output_queue: Vec::new(),
            flushing: false,
            sequence: dec::Vp8SequenceDecoder::new(),
        })
    }

    /// Decodes one VP8 frame payload.
    ///
    /// Both frame types run the full RFC 6386 reconstruction pipeline and
    /// queue a YUV 4:2:0 [`VideoFrame`] — unless the frame is hidden
    /// (`show_frame == 0`), in which case it still updates the reference
    /// buffers but queues nothing (see the module docs).
    ///
    /// An inter frame that arrives before any key frame has been decoded is
    /// rejected with [`CodecError::InvalidBitstream`]: there are no
    /// reference surfaces to predict from, and inventing them would
    /// fabricate pixels.
    fn decode_frame(&mut self, data: &[u8], pts: i64) -> CodecResult<()> {
        // Parse the uncompressed frame tag first, so that a key frame's
        // dimensions become available even if full reconstruction fails
        // below.
        let tag = FrameHeader::parse(data)?;
        if tag.is_keyframe() {
            self.width = Some(u32::from(tag.width));
            self.height = Some(u32::from(tag.height));
        }

        let decoded = self.sequence.decode_frame(data)?;
        let image = decoded.image;

        // Hidden frames (show_frame == 0) are decoded — they refresh
        // reference buffers — but never emitted for display.
        if decoded.show_frame {
            let mut frame = VideoFrame::new(PixelFormat::Yuv420p, image.width, image.height);
            frame.frame_type = if decoded.is_keyframe {
                VideoFrameType::Key
            } else {
                VideoFrameType::Inter
            };
            frame.timestamp.pts = pts;
            let cw = image.chroma_width();
            let ch = image.chroma_height();
            frame.planes = vec![
                Plane::with_dimensions(image.y, image.width as usize, image.width, image.height),
                Plane::with_dimensions(image.u, cw as usize, cw, ch),
                Plane::with_dimensions(image.v, cw as usize, cw, ch),
            ];
            self.output_queue.push(frame);
        }
        Ok(())
    }
}

impl VideoDecoder for Vp8Decoder {
    fn codec(&self) -> CodecId {
        CodecId::Vp8
    }

    /// Sends a packet to the decoder.
    ///
    /// Key and inter frames are decoded to pixels and queued for
    /// [`Vp8Decoder::receive_frame`]. A hidden frame (`show_frame == 0`)
    /// returns `Ok(())` having queued nothing — see the module docs.
    fn send_packet(&mut self, data: &[u8], pts: i64) -> CodecResult<()> {
        if self.flushing {
            return Err(CodecError::InvalidParameter(
                "VP8: Cannot send packet while flushing".to_string(),
            ));
        }

        self.decode_frame(data, pts)
    }

    fn receive_frame(&mut self) -> CodecResult<Option<VideoFrame>> {
        if self.output_queue.is_empty() {
            if self.flushing {
                return Err(CodecError::Eof);
            }
            return Ok(None);
        }

        Ok(Some(self.output_queue.remove(0)))
    }

    fn flush(&mut self) -> CodecResult<()> {
        self.flushing = true;
        Ok(())
    }

    /// Drops all decoder state that a seek invalidates.
    ///
    /// The reference surfaces, entropy context, segmentation map and
    /// loop-filter deltas are all cleared: after a seek the next packet may
    /// be any frame of the stream, and predicting it from the *pre-seek*
    /// references would silently produce wrong pixels. The next packet must
    /// therefore be a key frame; an inter frame is rejected honestly until
    /// one arrives.
    ///
    /// Width/height are deliberately kept, so stream properties stay
    /// available to a caller that is still probing.
    fn reset(&mut self) {
        self.output_queue.clear();
        self.flushing = false;
        self.sequence.reset();
    }

    fn output_format(&self) -> Option<PixelFormat> {
        if self.width.is_some() && self.height.is_some() {
            Some(PixelFormat::Yuv420p)
        } else {
            None
        }
    }

    fn dimensions(&self) -> Option<(u32, u32)> {
        match (self.width, self.height) {
            (Some(w), Some(h)) => Some((w, h)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal spec-shaped key-frame header with an *empty* first
    /// partition: the tag/start-code/dimensions parse, but full
    /// reconstruction must reject it (no header partition to decode).
    const TRUNCATED_KEYFRAME: [u8; 10] =
        [0x10, 0x00, 0x00, 0x9D, 0x01, 0x2A, 0x40, 0x01, 0xF0, 0x00];

    #[test]
    fn test_vp8_decoder_new() {
        let config = DecoderConfig::default();
        let decoder = Vp8Decoder::new(config).expect("should succeed");
        assert_eq!(decoder.codec(), CodecId::Vp8);
        assert!(decoder.output_format().is_none());
        assert!(decoder.dimensions().is_none());
    }

    #[test]
    fn test_truncated_keyframe_parses_tag_then_errors() {
        let config = DecoderConfig::default();
        let mut decoder = Vp8Decoder::new(config).expect("should succeed");

        // The 10-byte header carries no compressed header partition, so the
        // full reconstruction pipeline must reject it (and must NOT emit a
        // fabricated frame).
        let result = decoder.send_packet(&TRUNCATED_KEYFRAME, 0);
        assert!(
            matches!(result, Err(CodecError::InvalidBitstream(_))),
            "expected InvalidBitstream for empty first partition, got {result:?}"
        );

        // But tag parsing still made stream properties available.
        assert_eq!(decoder.dimensions(), Some((320, 240)));
        assert_eq!(decoder.output_format(), Some(PixelFormat::Yuv420p));

        // No fabricated frame may be emitted.
        let frame = decoder.receive_frame().expect("should succeed");
        assert!(frame.is_none(), "no fake frame may be output");
    }

    #[test]
    fn test_inter_frame_without_keyframe() {
        let config = DecoderConfig::default();
        let mut decoder = Vp8Decoder::new(config).expect("should succeed");

        // Inter frame (without prior keyframe)
        let inter = [
            0x01, // frame_type=1 (inter)
            0x00, 0x00,
        ];

        // Should fail because no keyframe was received
        assert!(decoder.send_packet(&inter, 0).is_err());
    }

    #[test]
    fn test_inter_frame_after_a_failed_keyframe_is_rejected_honestly() {
        let config = DecoderConfig::default();
        let mut decoder = Vp8Decoder::new(config).expect("should succeed");

        // Prime dimensions via a (truncated) keyframe header parse. The
        // reconstruction failed, so no reference surface was published.
        let _ = decoder.send_packet(&TRUNCATED_KEYFRAME, 0);
        assert_eq!(decoder.dimensions(), Some((320, 240)));

        // An inter frame therefore has nothing to predict from and must
        // fail honestly rather than emit a blank frame.
        let inter = [0x11, 0x00, 0x00, 0x00];
        let err = match decoder.send_packet(&inter, 1) {
            Err(e) => e,
            Ok(()) => panic!("inter frames must not decode without references"),
        };
        assert!(
            matches!(err, CodecError::InvalidBitstream(_)),
            "expected InvalidBitstream, got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("key frame"),
            "error must name the missing key frame, got: {msg}"
        );
        assert!(
            decoder.receive_frame().expect("should succeed").is_none(),
            "no blank frame may be emitted for an inter frame"
        );
    }

    #[test]
    fn test_flush() {
        let config = DecoderConfig::default();
        let mut decoder = Vp8Decoder::new(config).expect("should succeed");

        // A truncated keyframe errors and queues nothing.
        assert!(decoder.send_packet(&TRUNCATED_KEYFRAME, 0).is_err());

        // Flush
        decoder.flush().expect("should succeed");

        // Should return EOF when no more frames
        assert!(matches!(decoder.receive_frame(), Err(CodecError::Eof)));

        // Cannot send more packets while flushing
        assert!(decoder.send_packet(&TRUNCATED_KEYFRAME, 0).is_err());
    }

    #[test]
    fn test_reset() {
        let config = DecoderConfig::default();
        let mut decoder = Vp8Decoder::new(config).expect("should succeed");

        assert!(decoder.send_packet(&TRUNCATED_KEYFRAME, 0).is_err());

        // Flush and reset
        decoder.flush().expect("should succeed");
        decoder.reset();

        // After reset, packets are accepted again (and this one still ends
        // in its bitstream error, not an InvalidParameter "flushing" error).
        assert!(matches!(
            decoder.send_packet(&TRUNCATED_KEYFRAME, 0),
            Err(CodecError::InvalidBitstream(_))
        ));
        // Dimensions survive reset (allows continued probing after seek).
        assert_eq!(decoder.dimensions(), Some((320, 240)));
    }

    #[test]
    fn test_no_frame_available() {
        let config = DecoderConfig::default();
        let mut decoder = Vp8Decoder::new(config).expect("should succeed");

        // No packets sent, no frames available
        let result = decoder.receive_frame().expect("should succeed");
        assert!(result.is_none());
    }
}
