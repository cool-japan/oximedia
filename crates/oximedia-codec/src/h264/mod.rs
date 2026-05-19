//! H.264 bitstream-syntax parsers.
//!
//! This module provides the typed parsers that turn raw NAL-unit
//! payloads into structured data: SPS, PPS, and slice header.  It is
//! intentionally *parsing-only* — no reconstruction is performed.
//!
//! ## Pipeline
//!
//! 1. Carve the byte stream into NAL units via the existing
//!    [`crate::nal_unit`] helpers or the RTP depacketizer in
//!    `oximedia-net`.
//! 2. Pass the NAL payload (after the 1-byte header) through
//!    [`rbsp::strip_emulation_prevention`] to recover the raw RBSP
//!    bytes.
//! 3. Dispatch on the NAL unit type:
//!    - Type 7 → [`sps::parse_sps`]
//!    - Type 8 → [`pps::parse_pps`]
//!    - Type 1 or 5 → [`slice_header::parse_slice_header`] (with the
//!      SPS / PPS / nal context produced above)
//!
//! ## Scope
//!
//! - **SPS**: profile / level / dimensions / picture order count /
//!   reference frame count / cropping / VUI presence flag.  Scaling
//!   lists are consumed but not retained.  VUI body is not parsed.
//! - **PPS**: entropy coder, default ref list sizes, QP, weighted
//!   prediction flags, deblocking control, transform_8x8 mode, and
//!   chroma QP offsets.  Slice-group maps for `num_slice_groups > 0`
//!   are consumed but not retained.
//! - **Slice header**: the prefix every decoder needs to place a slice
//!   in its picture, pick references, and recover the slice's effective
//!   QP.  Reference-list modification, prediction weight tables, and
//!   adaptive MMCO are consumed but not retained.
//!
//! Future PRs are expected to extend retention rather than rewrite the
//! parser shape.

pub mod bit_reader;
pub mod pps;
pub mod rbsp;
pub mod slice_header;
pub mod sps;

pub use bit_reader::BitReader;
pub use pps::{parse_pps, PpsRbsp};
pub use rbsp::{strip_emulation_prevention, trailing_bits_len};
pub use slice_header::{parse_slice_header, NalContext, SliceHeader, SliceType};
pub use sps::{parse_sps, SpsRbsp};
