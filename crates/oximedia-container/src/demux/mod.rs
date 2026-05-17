//! Demuxer implementations.
//!
//! This module provides demuxers for extracting compressed packets
//! from container formats.
//!
//! # Supported Formats
//!
//! - Matroska/`WebM` via [`MatroskaDemuxer`]
//! - FLAC via [`FlacDemuxer`]
//! - Ogg via [`OggDemuxer`] (Opus, Vorbis, FLAC, Theora)
//! - WAV/RIFF via [`WavDemuxer`]
//! - MP4/ISOBMFF via [`Mp4Demuxer`] (AV1/VP9 only)
//! - MPEG-TS via [`MpegTsDemuxer`] (AV1/VP9/VP8/Opus/FLAC only)
//! - `WebVTT` via [`WebVttDemuxer`]
//! - `SubRip` (SRT) via [`SrtDemuxer`]
//! - YUV4MPEG2 via [`Y4mDemuxer`]
//!
//! # Patent Protection
//!
//! The MP4 and MPEG-TS demuxers only support royalty-free codecs. Attempting to
//! demux files containing H.264, H.265, AAC, or other patent-encumbered
//! codecs will result in a [`PatentViolation`](oximedia_core::OxiError::PatentViolation) error.

pub mod avi;
pub mod buffer;
pub mod flac;
pub mod flv;
pub mod matroska;
#[cfg(all(feature = "mmap", not(target_arch = "wasm32")))]
pub mod mmap;
pub mod mp4;
pub mod mpegts;
pub mod mpegts_enhanced;
pub mod ogg;
pub mod srt;
mod traits;
pub mod wav;
pub mod webvtt;
pub mod y4m;

pub use avi::{AviDemuxError, AviMjpegReader};
pub use flac::FlacDemuxer;
pub use flv::{FlvDemuxer, FlvError, FlvHeader, FlvTag};
pub use matroska::MatroskaDemuxer;
pub use mp4::Mp4Demuxer;
pub use mpegts::MpegTsDemuxer;
pub use mpegts_enhanced::{
    parse_pat, parse_pmt, parse_ts_packet, Pat, PatEntry, PidInfo, Pmt, PmtStream, TsDemuxer,
    TsPacket, TsStreamInfo,
};
pub use ogg::OggDemuxer;
pub use srt::SrtDemuxer;
pub use traits::Demuxer;
pub use wav::WavDemuxer;
pub use webvtt::WebVttDemuxer;
pub use y4m::Y4mDemuxer;
