//! Safe Rust wrappers over Apple VideoToolbox.
//!
//! This crate sits on top of [`oximedia_vtb_sys`] (the raw FFI bindings)
//! and exposes Rust types that drive the same APIs FFmpeg's
//! `libavcodec/videotoolbox.c` uses for hardware H.264 / HEVC video
//! encode and decode on Apple platforms.
//!
//! Scope of the **current** revision:
//! - H.264 **decode** via `VTDecompressionSession`. AVCC sample buffers
//!   are built from Annex-B input; decoded frames come out as NV12
//!   `VideoFrame`s.
//!
//! Scope of explicit **follow-ups**:
//! - H.264 encode via `VTCompressionSession`.
//! - HEVC decode/encode (same APIs, different codec-type constants).
//! - AAC decode/encode via `AudioConverter` (AudioToolbox, separate
//!   framework — gets its own module).
//! - Integration with the `oximedia_codec::VideoDecoder` / `VideoEncoder`
//!   traits + registration in the workspace codec registry. The wrappers
//!   here are deliberately decoupled from those traits until the surface
//!   has been validated against real streams.
//!
//! # Platform gating
//!
//! VideoToolbox is macOS / iOS only. On other platforms this crate
//! compiles to an empty module so the workspace builds everywhere.
//! Downstream callers should gate on
//! `cfg(any(target_os = "macos", target_os = "ios"))`.

#![cfg(any(target_os = "macos", target_os = "ios"))]

pub mod cf;
pub mod decoder;
pub mod error;
pub mod format;
pub mod nal;
pub mod session;

pub use decoder::H264Decoder;
pub use error::{Result, StatusContext, VtbError};
pub use format::H264FormatDescription;
pub use nal::{
    annex_b_to_avcc, avcc_to_annex_b, extract_sps_pps, nal_unit_type, AnnexBIter, NAL_TYPE_PPS,
    NAL_TYPE_SPS,
};
pub use session::DecompressionSession;
