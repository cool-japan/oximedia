//! Network streaming for `OxiMedia`.
//!
//! This crate provides network streaming protocols for the `OxiMedia` multimedia
//! framework. It supports various streaming protocols including:
//!
//! - **HLS** (HTTP Live Streaming) - Apple's adaptive streaming protocol
//! - **DASH** (Dynamic Adaptive Streaming over HTTP) - MPEG-DASH streaming
//! - **RTMP** (Real-Time Messaging Protocol) - Flash streaming protocol
//! - **SRT** (Secure Reliable Transport) - Low-latency streaming
//! - **WebRTC** - Real-time browser communication
//! - **SMPTE ST 2110** - Professional media over IP (uncompressed video/audio/ANC)
//! - **CDN** - Multi-CDN failover and load balancing
//!
//! # Overview
//!
//! Each streaming protocol module provides:
//! - Protocol-specific packet/message types
//! - Parsing and serialization
//! - Session management
//! - Adaptive bitrate support where applicable
//!
//! The CDN module provides:
//! - Multi-CDN provider support (Cloudflare, Fastly, Akamai, CloudFront, Custom)
//! - Real-time health monitoring
//! - Automatic failover with circuit breaker pattern
//! - Intelligent routing strategies
//! - Performance metrics and SLA monitoring
//!
//! The SMPTE ST 2110 module provides:
//! - Uncompressed video transport (ST 2110-20)
//! - PCM audio transport (ST 2110-30)
//! - Ancillary data transport (ST 2110-40)
//! - PTP synchronization (IEEE 1588)
//! - SDP session description
//! - Broadcast-quality professional media over IP
//!
//! # Example
//!
//! ```ignore
//! use oximedia_net::hls::{MasterPlaylist, MediaPlaylist};
//! use oximedia_net::error::NetResult;
//!
//! async fn fetch_playlist(url: &str) -> NetResult<MasterPlaylist> {
//!     // Fetch and parse HLS master playlist
//!     todo!()
//! }
//! ```

#![warn(missing_docs)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    dead_code,
    clippy::pedantic,
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::option_map_unit_fn
)]

pub mod abr;
pub mod abr_buffer;
pub mod bandwidth_estimator;
pub mod bandwidth_probe;
pub mod bandwidth_throttle;
pub mod bandwidth_trigger;
pub mod buffer_model;
pub mod cdn;
pub mod connection_pool;
pub mod dash;
pub mod error;
pub mod fec;
pub mod fec_interleave;
pub mod flow_control;
pub mod hls;
pub mod http2;
pub mod ice;
pub mod live;
pub mod ll_dash;
pub mod ll_dash_config;
pub mod manifest_cache;
pub mod mdns;
pub mod multicast;
pub mod multicast_manager;
pub mod multipath;
pub mod network_path;
pub mod network_simulator;
pub mod pacing;
pub mod packet_buffer;
pub mod playlist_parser;
pub mod protocol_detect;
pub mod qos_monitor;
pub mod quic;
pub mod quic_datagram;
pub mod relay;
pub mod retry_policy;
pub mod rist;
pub mod rtmp;
pub mod rtp_session;
pub mod rtsp;
pub mod session_tracker;
pub mod smpte2022_7;
pub mod smpte2110;
pub mod srt;
pub mod srt_aes256gcm;
pub mod srt_config;
pub mod srt_group;
pub mod stream_health_monitor;
pub mod stream_mux;
pub mod webrtc;
pub mod websocket;
pub mod whep_client;
pub mod whip;
pub mod whip_whep;
pub mod zero_copy_serve;
pub mod zixi;

// Re-export commonly used items
pub use error::{NetError, NetResult};

// Re-export SRT stats and key exchange types
pub use srt::{DirectionStats, RttStats, SrtStreamStats, StreamQuality};
pub use srt::{EncryptionSession, EncryptionState, KmxKeyMaterial, KwAlgorithm};

// Re-export streaming ABR types
pub use abr::streaming::{
    AbrBandwidthEstimator as BandwidthEstimator, AbrController, AbrSwitchReason, AbrVariant,
    BandwidthSample, BufferedSegment, SegmentFetcher, SelectionResult,
};
