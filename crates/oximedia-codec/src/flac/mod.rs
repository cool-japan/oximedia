//! FLAC (Free Lossless Audio Codec) encoder and decoder.
//!
//! Both directions implement RFC 9639 exactly, and are cross-checked against
//! libFLAC and ffmpeg in `tests/flac_external.rs`: streams this encoder emits
//! pass `flac -t` (per-frame CRC-16 plus the STREAMINFO MD5 of the decoded
//! audio) and decode byte-identically under ffmpeg, while files produced by
//! those tools decode sample-exact here.
//!
//! # Structure
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`bitio`] | MSB-first bit reader/writer, CRC-8 and CRC-16 |
//! | [`frame`] | Frame header code tables, serialisation, channel assignment |
//! | [`subframe`] | Constant / verbatim / fixed / LPC subframes, wasted bits |
//! | [`residual`] | Partitioned Rice coding, including escaped partitions |
//! | [`lpc`] | LPC analysis plus the exact integer predictors |
//! | [`rice`] | Zigzag folding and single-run Rice helpers |
//!
//! # Losslessness
//!
//! FLAC is bit-packed: subframes run back-to-back with no byte alignment, and
//! only the frame footer is aligned.  The encoder computes residuals with the
//! very same `i64` accumulation and arithmetic right shift the decoder uses to
//! invert them ([`lpc::lpc_residual`] / [`lpc::lpc_restore_exact`]), so every
//! frame is exact by construction rather than by luck.
//!
//! # Reference
//!
//! RFC 9639 — <https://www.rfc-editor.org/rfc/rfc9639.html>
//!
//! # Example
//!
//! ```rust
//! use oximedia_codec::flac::{FlacConfig, FlacDecoder, FlacEncoder};
//!
//! let config = FlacConfig { sample_rate: 44100, channels: 2, bits_per_sample: 16 };
//! let mut encoder = FlacEncoder::new(config);
//!
//! // Interleaved i32 PCM: 2048 stereo frames of a quiet tone.
//! let pcm: Vec<i32> = (0..2048 * 2)
//!     .map(|i| ((i / 2) as f64 * 0.05).sin().mul_add(8000.0, 0.0) as i32)
//!     .collect();
//!
//! let (header, frames) = encoder.encode(&pcm)?;
//! let mut stream = header;
//! for frame in &frames {
//!     stream.extend_from_slice(&frame.data);
//! }
//! // Rewrite STREAMINFO with the real sample count, frame sizes and MD5.
//! stream[..42].copy_from_slice(&encoder.finalized_stream_header());
//!
//! let mut decoder = FlacDecoder::new();
//! assert_eq!(decoder.decode_stream(&stream)?, pcm, "FLAC is lossless");
//! # Ok::<(), oximedia_codec::error::CodecError>(())
//! ```

pub mod bitio;
pub mod decoder;
pub mod encoder;
pub mod frame;
pub mod lpc;
pub mod residual;
pub mod rice;
pub mod subframe;

pub use decoder::{DecodedBlock, FlacDecoder, FlacStreamInfo, FlacVorbisComment};
pub use encoder::{FlacConfig, FlacEncoder, FlacFrame};
pub use frame::{ChannelAssignment, FrameHeader};
