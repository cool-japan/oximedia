//! RTP payload depacketization — assembling codec bitstreams from RTP
//! packet payloads.
//!
//! An RTP packet's payload is the bytes *after* the 12-byte RTP header.
//! What's in those bytes depends on the codec's RTP payload format:
//!
//! - **H.264** uses RFC 6184: single NAL / STAP-A / FU-A framing.
//! - **HEVC** uses RFC 7798: single NAL / aggregation / fragmentation.
//!   (not yet implemented)
//! - **AAC** uses RFC 3640: AU-headers + bit-aligned AU payload.
//!   (not yet implemented)
//! - **Opus** uses RFC 7587: payload = raw Opus frames.
//!   (not yet implemented)
//!
//! Each depacketizer is a small state machine that consumes RTP payloads
//! one at a time and emits codec access units when the RTP marker bit
//! signals end-of-frame.
//!
//! The depacketizers in this module are intentionally independent of the
//! RTSP client — they take only the payload bytes and the marker bit, so
//! they work equally well with bare UDP RTP, WebRTC, ST 2110, or any
//! other RTP source.

pub mod h264;

pub use h264::{AccessUnit, H264Depacketizer};
