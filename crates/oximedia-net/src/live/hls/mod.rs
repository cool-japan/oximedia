//! HLS (HTTP Live Streaming) server implementation.
//!
//! This module provides a complete HLS server with support for:
//! - Master playlists (variant streams)
//! - Media playlists (segment lists)
//! - LL-HLS (Low Latency HLS)
//! - DVR/time-shifting
//! - Multiple quality variants

pub mod client;
pub mod ll_hls;
pub mod playlist;
pub mod server;

pub use client::{extract_program_date_times, latency_from_pdt, LatencyEstimator, LatencySample};
pub use ll_hls::LlHlsConfig;
pub use playlist::{MasterPlaylistBuilder, MediaPlaylistBuilder};
pub use server::HlsServer;
