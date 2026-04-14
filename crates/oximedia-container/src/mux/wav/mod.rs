//! WAV/RIFF container muxer.
//!
//! This module provides a muxer for creating WAV (Waveform Audio File Format)
//! files. WAV is a simple container format for uncompressed PCM audio.
//!
//! # Supported Formats
//!
//! - PCM 8-bit unsigned
//! - PCM 16-bit signed
//! - PCM 24-bit signed
//! - PCM 32-bit signed
//! - IEEE Float 32-bit
//! - IEEE Float 64-bit
//!
//! # Example
//!
//! ```ignore
//! use oximedia_container::mux::{WavMuxer, Muxer, MuxerConfig};
//!
//! let config = MuxerConfig::new();
//! let mut muxer = WavMuxer::new(sink, config);
//!
//! muxer.add_stream(audio_info)?;
//! muxer.write_header().await?;
//!
//! for packet in packets {
//!     muxer.write_packet(&packet).await?;
//! }
//!
//! muxer.write_trailer().await?;
//! ```

mod writer;

pub use writer::WavFormatConfig;
pub use writer::WavMuxer;
