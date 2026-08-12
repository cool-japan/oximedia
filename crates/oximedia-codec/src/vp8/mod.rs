//! VP8 codec implementation.
//!
//! This module provides a pure Rust VP8 decoder (key frames), encoder
//! building blocks, and bitstream primitives based on RFC 6386. VP8 is a
//! royalty-free video codec developed by Google as part of the `WebM`
//! project.
//!
//! # Status: key frames and inter frames both decode to pixels
//!
//! Key-frame (intra) decoding is **fully implemented**: `Vp8Decoder`
//! reconstructs real YUV 4:2:0 pixels via the complete RFC 6386 intra
//! pipeline — boolean entropy decoding, full key-frame header parsing
//! (segmentation, loop-filter parameters, quantiser indices, token
//! probability updates), macroblock mode and DCT-token decoding,
//! per-segment dequantisation, inverse DCT/WHT, every 16x16/4x4/chroma
//! intra prediction mode, and both (simple + normal) in-loop deblocking
//! filters. The pipeline is ported from the production-verified
//! `oximedia-image` WebP/VP8 decoder (a WebP lossy image *is* a VP8 key
//! frame).
//!
//! Inter frames (P-frames) are **also fully implemented**, on top of that
//! same pipeline: per-macroblock prediction records and the near-motion-
//! vector survey (§16), motion-vector entropy decoding (§17), sub-pixel
//! motion compensation (§18), and last/golden/altref reference-frame
//! management with the cross-frame entropy, segmentation and loop-filter
//! state an inter frame inherits (§9.3-§9.10). Both frame types are
//! verified bit-exact against libvpx on multi-frame conformance streams
//! (see `docs/codec_status.md`).
//!
//! A frame coded with `show_frame == 0` (an invisible alternate-reference
//! frame) is decoded and updates the reference buffers, but is never
//! emitted: `send_packet` returns `Ok(())` and queues nothing.
//!
//! This module also exposes standalone primitives (boolean decoder,
//! DCT/WHT transforms, prediction, motion-compensation and loop-filter
//! helpers) that predate the key-frame pipeline and remain available as
//! public building blocks.
//!
//! # Codec Details
//!
//! VP8 always outputs YUV 4:2:0 planar format (`Yuv420p`).
//! It supports two frame types:
//! - Keyframes: Can be decoded independently
//! - Inter frames: Use motion compensation from reference frames
//!
//! # Architecture
//!
//! VP8 operates on 16x16 macroblocks which can use:
//! - Intra prediction: I16 (16x16) or I4 (4x4) modes
//! - Inter prediction: Motion compensation with quarter-pixel precision
//! - Transform: 4x4 DCT or WHT for DC coefficients
//! - Loop filter: Deblocking filter at macroblock boundaries
//!
//! # Examples
//!
//! ```
//! use oximedia_codec::vp8::{Vp8Decoder, FrameHeader, FrameType};
//! use oximedia_codec::traits::{VideoDecoder, DecoderConfig};
//! use oximedia_codec::error::CodecError;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Create a decoder
//! let config = DecoderConfig::default();
//! let mut decoder = Vp8Decoder::new(config)?;
//!
//! // Parse a frame header directly (tag + start code + dimensions).
//! let header_data = [
//!     0x10, 0x00, 0x00,       // frame tag
//!     0x9D, 0x01, 0x2A,       // sync code
//!     0x40, 0x01, 0xF0, 0x00, // 320x240
//! ];
//! let header = FrameHeader::parse(&header_data)?;
//! assert!(header.is_keyframe());
//! assert_eq!(header.width, 320);
//! assert_eq!(header.height, 240);
//!
//! // Full decode needs the compressed header/token partitions too; this
//! // 10-byte stub has none, so decoding reports a bitstream error while
//! // stream dimensions stay available. A complete key-frame payload
//! // (e.g. the `VP8 ` chunk of a lossy WebP) decodes to real pixels.
//! let result = decoder.send_packet(&header_data, 0);
//! assert!(matches!(result, Err(CodecError::InvalidBitstream(_))));
//! assert_eq!(decoder.dimensions(), Some((320, 240)));
//! assert!(decoder.receive_frame()?.is_none());
//!
//! // An inter frame needs reference frames: with no key frame decoded
//! // there are none, so it errors honestly rather than inventing pixels.
//! let inter = [0x11, 0x00, 0x00, 0x00];
//! let result = decoder.send_packet(&inter, 1);
//! assert!(matches!(result, Err(CodecError::InvalidBitstream(_))));
//! # Ok(())
//! # }
//! ```
//!
//! # References
//!
//! - [RFC 6386: VP8 Data Format and Decoding Guide](https://tools.ietf.org/html/rfc6386)
//! - [WebM Project](https://www.webmproject.org/)

mod bool_decoder;
mod dct;
mod dec;
mod decoder;
mod encoder;
mod frame_header;
mod loopfilter;
mod mb_mode;
mod motion;
mod prediction;

/// Crate-internal re-export of the *sequence* (key + inter frame) decode
/// entry point, for the same path-visibility reason as the line above.
pub(crate) use dec::Vp8SequenceDecoder;
/// Crate-internal re-export of the key-frame decode entry point and its
/// output type. `dec` is a private submodule, so — even though
/// `dec::decode_keyframe`/`dec::DecodedImage` are themselves declared
/// `pub(crate)` — sibling modules outside `vp8` (e.g. `crate::webp`, which
/// wires a lossy WebP `VP8 ` chunk to this same decoder: a lossy WebP image
/// *is* a VP8 key frame per RFC 6386) cannot name `crate::vp8::dec::...`
/// directly, because path resolution requires every segment to be visible
/// from the call site, and `dec` itself is not. This re-export fixes only
/// that path-visibility gap; it does not change what is public API (still
/// `pub(crate)`, not `pub`).
pub(crate) use dec::{decode_keyframe, DecodedImage};

pub use bool_decoder::BoolDecoder;
pub use dct::{dequantize_block, dequantize_coeff, idct4x4, iwht4x4, Block4x4, PixelBlock4x4};
pub use decoder::Vp8Decoder;
pub use encoder::{SimpleVp8Encoder, Vp8EncConfig, Vp8Encoder, Vp8EncoderConfig, Vp8Packet};
pub use frame_header::{ClampingType, ColorSpace, FrameHeader, FrameType};
pub use loopfilter::{
    calculate_filter_params, filter_horizontal_edge, filter_vertical_edge, LoopFilterConfig,
    MAX_LOOP_FILTER, MAX_SHARPNESS,
};
// `mb_mode` and `motion` are deprecated (0.2.1, defective early seeds
// superseded by the internal RFC 6386 pipeline behind `Vp8Decoder`; see
// their module docs). Re-exporting a deprecated item from a non-deprecated
// `pub use` still names it, so this internal reference site needs a
// targeted allow.
#[allow(deprecated)]
pub use mb_mode::{
    ChromaMode, InterMode, IntraMode16, IntraMode4, MacroblockType, PartitionType, RefFrame,
    NUM_CHROMA_MODES, NUM_I16_MODES, NUM_I4_MODES, NUM_MV_MODES,
};
#[allow(deprecated)]
pub use motion::{clamp_mv, motion_compensate, MotionVector};
pub use prediction::{predict_chroma, predict_intra_16x16, predict_intra_4x4};
