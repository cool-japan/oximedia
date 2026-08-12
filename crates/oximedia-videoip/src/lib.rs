//! Professional video-over-IP protocol for `OxiMedia`.
//!
//! This crate provides a patent-free alternative to NDI (Network Device Interface)
//! for professional video streaming over IP networks. It implements:
//!
//! - **Low-latency video/audio transport** over UDP with FEC (Forward Error Correction)
//! - **mDNS/DNS-SD service discovery** for automatic source detection
//! - **Professional features**: Tally lights, PTZ control, timecode, metadata
//! - **Network resilience**: FEC, jitter buffering, packet loss recovery
//! - **Multi-stream support**: Program, preview, alpha channels
//!
//! # Codec support (send vs. receive are not symmetric)
//!
//! - **Send** ([`VideoIpSource`]): uncompressed video (v210, UYVY, 8-bit and
//!   10-bit 4:2:0 planar) and uncompressed audio (PCM 16/24-bit, 32-bit
//!   float). There is **no compressed send path**: no working VP8, VP9, AV1
//!   or Opus encoder exists behind this crate, so those codecs are refused at
//!   construction rather than broadcasting raw frames labelled as a
//!   compressed bitstream.
//! - **Receive** ([`VideoIpReceiver`]): everything above, **plus real VP8, VP9
//!   and AV1 decode** through `oximedia-codec` — dimensions come from the
//!   bitstream, and the decoders are bit-exact against libvpx/libaom on the
//!   fixtures in `tests/fixtures/`. Opus decode is refused.
//!
//! [`codec`] documents, per codec and per direction, exactly what is missing
//! and how that was verified; every unavailable direction fails with
//! [`VideoIpError::CodecUnimplemented`] carrying the same detail at runtime.
//!
//! # Protocol Design
//!
//! The protocol is designed for professional broadcast environments with:
//! - Target latency < 16ms at 60fps (less than 1 frame)
//! - Support for SD, HD, and UHD resolutions
//! - Frame rates: 23.976, 24, 25, 29.97, 30, 50, 59.94, 60 fps
//! - Up to 16 audio channels at 48kHz or 96kHz
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────┐                           ┌─────────────┐
//! │  VideoIP    │  ─── UDP + FEC ────>      │  VideoIP    │
//! │  Source     │  <── Control Msgs ──      │  Receiver   │
//! └─────────────┘                           └─────────────┘
//!       │                                          │
//!       └──── mDNS Announcement                   │
//!                                                  │
//!                                  mDNS Discovery ─┘
//! ```
//!
//! # Example
//!
//! ## Broadcasting a Video Stream
//!
//! ```ignore
//! use oximedia_videoip::types::{AudioCodec, VideoCodec};
//! use oximedia_videoip::{VideoIpSource, VideoConfig, AudioConfig};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // `VideoConfig::new` defaults to VP9 and `AudioConfig::new` to Opus,
//!     // neither of which this crate can encode -- pick transmittable ones.
//!     let video_config = VideoConfig::new(1920, 1080, 60.0)?.with_codec(VideoCodec::Uyvy);
//!     let audio_config = AudioConfig::new(48000, 2)?.with_codec(AudioCodec::Pcm16)?;
//!
//!     let mut source = VideoIpSource::new("Camera 1", video_config, audio_config)?;
//!     source.start_broadcasting().await?;
//!
//!     // Send frames...
//!     let video_frame = get_video_frame();
//!     let audio_samples = get_audio_samples();
//!     source.send_frame(video_frame, audio_samples).await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Receiving a Video Stream
//!
//! ```ignore
//! use oximedia_videoip::VideoIpReceiver;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let mut receiver = VideoIpReceiver::discover("Camera 1").await?;
//!     receiver.start_receiving().await?;
//!
//!     loop {
//!         let (video_frame, audio_samples) = receiver.receive_frame().await?;
//!         // Process frame...
//!     }
//! }
//! ```

#![warn(missing_docs)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::too_many_arguments)]

pub mod adaptive_jitter_buffer;
pub mod bandwidth_est;
pub mod bandwidth_shaping;
pub mod bbr;
pub mod bbr_congestion;
pub mod bonding;
pub mod codec;
pub mod color_space_conv;
pub mod color_space_simd;
pub mod congestion;
pub mod diagnostic_overlay;
pub mod discovery;
pub mod dtls_srtp;
pub mod encryption;
pub mod error;
pub mod fec;
pub mod flow_monitor;
pub mod flow_stats;
pub mod frame_pacing;
pub mod gf_simd;
pub mod jitter;
pub mod metadata;
pub mod multicast;
pub mod multicast_group;
pub mod multiview;
pub mod ndi_bridge;
pub mod network_sim;
pub mod nmos;
pub mod packet;
pub mod packet_loss;
pub mod precise_timer;
pub mod ptp;
pub mod ptp_boundary;
pub mod ptp_clock;
pub mod ptz;
pub mod quic_transport;
/// Real QUIC transport backed by `quinn` (enabled by the `quic-quinn` feature).
#[cfg(feature = "quic-quinn")]
pub mod quic_transport_quinn;
pub mod receiver;
pub mod redundancy;
pub mod rist;
pub mod rtcp_sender_report;
pub mod rtp_2110;
pub mod rtp_jitter_buffer;
pub mod rtsp_server;
pub mod scatter_gather_io;
pub mod sdp;
pub mod sdp_gen;
pub mod sdp_negotiation;
pub mod sfp_monitor;
pub mod smpte2110;
pub mod source;
pub mod spsc_ring;
pub mod srt_config;
pub mod srt_handshake;
pub mod srt_transport;
pub mod st2110_20;
pub mod st2110_metadata;
pub mod stats;
pub mod stream_descriptor;
pub mod stream_health;
pub mod stream_recorder;
pub mod stream_recording_mux;
pub mod stream_relay;
pub mod stream_sync;
pub mod tally;
pub mod transport;
pub mod types;
pub mod udp_scatter_gather;
pub mod utils;
pub mod videoip_ext;
pub mod whip_whep;
pub mod zero_copy_packet;

// Re-export main types
pub use error::{VideoIpError, VideoIpResult};
pub use receiver::VideoIpReceiver;
pub use source::VideoIpSource;
pub use types::{AudioConfig, AudioFormat, VideoConfig, VideoFormat};
