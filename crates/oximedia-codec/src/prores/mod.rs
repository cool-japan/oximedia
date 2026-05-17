//! Apple ProRes 422 decoder.
//!
//! Implements the bitstream specified in **SMPTE RDD 36-2015**
//! ("Apple ProRes Bitstream Syntax"). ProRes is an intra-only 4:2:2
//! 10-bit codec widely used as a post-production intermediate. Patent
//! licensing for encoding is held by Apple; decoding is unencumbered
//! in practice — FFmpeg has shipped a decoder since 2010 without
//! consequence.
//!
//! # Pipeline
//!
//! ```text
//!   ProRes file
//!     │
//!     ▼  FrameContainer::parse
//!   frame container ('icpf')
//!     │
//!     ▼  parse_frame_header
//!   FrameHeader (size, profile, dimensions, quant matrices)
//!     │
//!     ▼  parse_picture_header
//!   PictureHeader (slice count)
//!     │
//!     ▼  parse_slice_header (×N)
//!   SliceHeader (qscale, per-plane sizes)
//!     │
//!     ▼  split_slice_planes
//!   SliceData { luma, cb, cr, alpha? }
//!     │
//!     ▼  decode_slice_to_yuv422
//!         (entropy decode → inverse zigzag →
//!          dequantization → 8×8 IDCT →
//!          10-bit clip → plane assembly)
//!   10-bit YUV422 sample buffers
//! ```
//!
//! # Module map
//!
//! - [`frame`] — frame container + frame header parsing
//! - [`picture`] — picture header + slice header parsing
//! - [`quant`] — default 8×8 luma/chroma quantization matrices
//! - [`bitreader`] — MSB-first bit-level reader for entropy decode
//! - [`entropy`] — Golomb-Rice codeword decoder + DC/AC block decoder
//! - [`zigzag`] — progressive + alternate inverse zigzag scan tables
//! - [`dequant`] — dequantization (× matrix × qscale)
//! - [`idct`] — 8×8 inverse DCT + 10-bit output finalisation
//! - [`decode`] — slice-level decode pipeline wiring everything together
//!
//! # Profiles
//!
//! All five 4:2:2 sub-profiles share the same bitstream syntax; only
//! their quantization matrices and default qscale ranges differ.
//! [`FrameHeader::profile`] reports which profile a stream uses.
//!
//! # Status
//!
//! Phase 1 (parser) and Phase 2 (entropy + transform + plane
//! assembly) are both implemented and tested with synthetic vectors.
//! Real-stream conformance — bit-exact pixel comparison against
//! Apple-encoded ProRes files — is the explicit follow-up
//! (fixture corpus required).

pub mod bitreader;
pub mod decode;
pub mod dequant;
pub mod entropy;
pub mod frame;
pub mod idct;
pub mod picture;
pub mod quant;
pub mod zigzag;

pub use bitreader::BitReader;
pub use decode::{decode_slice_to_yuv422, split_slice_planes, DecodeError, SliceData};
pub use dequant::dequantize_block;
pub use entropy::{
    decode_block, decode_signed_codeword, decode_unsigned_codeword, next_k_ac_level,
    next_k_ac_run, next_k_dc, EntropyError,
};
pub use frame::{
    parse_frame_header, ChromaFormat, FrameContainer, FrameError, FrameHeader, InterlaceMode,
    ProResProfile,
};
pub use idct::{finalize_idct_output, idct_8x8};
pub use picture::{parse_picture_header, parse_slice_header, PictureHeader, SliceHeader};
pub use quant::{DEFAULT_CHROMA_QUANT_MATRIX, DEFAULT_LUMA_QUANT_MATRIX};
pub use zigzag::{inverse_scan, ALTERNATE_ZIGZAG, PROGRESSIVE_ZIGZAG};
