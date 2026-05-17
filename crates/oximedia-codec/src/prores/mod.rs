//! Apple ProRes 422 decoder.
//!
//! Implements the bitstream specified in **SMPTE RDD 36-2015**
//! ("Apple ProRes Bitstream Syntax"). ProRes is an intra-only 4:2:2
//! 10-bit codec widely used as a post-production intermediate. Patent
//! licensing for encoding is held by Apple; decoding is unencumbered
//! in practice — FFmpeg has shipped a decoder since 2010 without
//! consequence.
//!
//! # Scope of this revision
//!
//! Phase 1 (this revision) — **header & quantization parser**:
//!
//! - Frame container parsing (4-byte size + 'icpf' marker).
//! - Frame header parsing (size, identifier, dimensions, chroma format,
//!   interlace mode, framerate code, colour metadata, alpha info,
//!   custom quantization matrix flags).
//! - Optional custom 8×8 luma/chroma quantization matrices.
//! - Picture header parsing (size, slice count).
//! - Per-slice header parsing (luma/chroma sizes, scale code).
//! - Default ProRes quantization matrices from RDD 36 §6.5.4.
//! - Bit-reader primitive for entropy decode.
//!
//! Phase 2 (explicit follow-up — not in this revision):
//!
//! - Entropy decode: DC differential coding + AC run/level
//!   exp-Golomb coding with adaptive Rice parameter.
//! - Inverse zigzag scan, dequantization.
//! - 8×8 integer IDCT (RDD 36 §6.5.7).
//! - Plane assembly into [`oximedia_codec::VideoFrame`].
//!
//! See [`decode::decode_slice_to_yuv422`] for the entry point Phase 2
//! must finish. The parser layer here will not change — it produces
//! exactly the structured input that the decode pipeline consumes.
//!
//! # Profiles
//!
//! ProRes ships in five 4:2:2 sub-profiles, identified by the 4-byte
//! `encoder_identifier` in the frame header:
//!
//! | FourCC | Profile     | Target bitrate (1080p / 24fps) |
//! |--------|-------------|--------------------------------|
//! | `apco` | 422 Proxy   | ~45 Mbit/s                     |
//! | `apcs` | 422 LT      | ~102 Mbit/s                    |
//! | `apcn` | 422         | ~147 Mbit/s                    |
//! | `apch` | 422 HQ      | ~220 Mbit/s                    |
//!
//! All four use the same bitstream syntax; only the quantization
//! matrices and default qscale ranges differ. This parser recognises
//! every flavour and reports the profile back via
//! [`FrameHeader::profile`].

pub mod bitreader;
pub mod decode;
pub mod frame;
pub mod picture;
pub mod quant;

pub use bitreader::BitReader;
pub use decode::{decode_slice_to_yuv422, DecodeError, SliceData};
pub use frame::{
    parse_frame_header, ChromaFormat, FrameContainer, FrameHeader, InterlaceMode, ProResProfile,
};
pub use picture::{parse_picture_header, parse_slice_header, PictureHeader, SliceHeader};
pub use quant::{DEFAULT_CHROMA_QUANT_MATRIX, DEFAULT_LUMA_QUANT_MATRIX};
