//! High-level transcoding pipeline for `OxiMedia`.
//!
//! This crate provides a comprehensive, professional-grade transcoding system with:
//!
//! # Features
//!
//! ## High-Level API
//!
//! - **Simple One-Liner Transcoding** - Quick transcoding with sensible defaults
//! - **Preset Library** - Industry-standard presets for major platforms
//! - **Fluent Builder API** - Complex workflows with readable code
//!
//! ## Professional Features
//!
//! - **Multi-Pass Encoding** - 2-pass and 3-pass encoding for optimal quality
//! - **ABR Ladder Generation** - Adaptive bitrate encoding for HLS/DASH
//! - **Parallel Encoding** - Encode multiple outputs simultaneously
//! - **Hardware Acceleration** - Auto-detection and use of GPU encoders
//! - **Progress Tracking** - Real-time progress with ETA estimation
//! - **Audio Normalization** - Automatic loudness normalization (EBU R128/ATSC A/85)
//! - **Quality Control** - CRF, CBR, VBR, and constrained VBR modes
//! - **Subtitle Support** - Burn-in or soft subtitle embedding
//! - **Chapter Markers** - Preserve or add chapter information
//! - **Metadata Preservation** - Copy or map metadata fields
//!
//! ## Job Management
//!
//! - **Job Queuing** - Queue multiple transcode jobs
//! - **Priority Scheduling** - High, normal, and low priority jobs
//! - **Resource Management** - CPU/GPU limits and throttling
//! - **Error Recovery** - Automatic retry logic with exponential backoff
//! - **Validation** - Input/output validation before processing
//!
//! # Supported Platforms
//!
//! ## Streaming Platforms
//!
//! - **`YouTube`** - 1080p60, 4K, VP9/H.264 variants
//! - **Vimeo** - Professional quality presets
//! - **Twitch** - Live streaming optimized
//! - **Social Media** - Instagram, `TikTok`, Twitter optimized
//!
//! ## Broadcast
//!
//! - **`ProRes` Proxy** - High-quality editing proxies
//! - **`DNxHD` Proxy** - Avid editing proxies
//! - **Broadcast HD/4K** - Broadcast-ready deliverables
//!
//! ## Streaming Protocols
//!
//! - **HLS** - HTTP Live Streaming ABR ladders
//! - **DASH** - MPEG-DASH ABR ladders
//! - **CMAF** - Common Media Application Format
//!
//! ## Archive
//!
//! - **Lossless** - FFV1 lossless preservation
//! - **High Quality** - VP9/AV1 archival encoding
//!
//! # Quick Start
//!
//! ## Simple Transcoding
//!
//! ```rust,no_run
//! use oximedia_transcode::{Transcoder, presets};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Simple transcode to YouTube 1080p
//! Transcoder::new()
//!     .input("input.mp4")
//!     .output("output.mp4")
//!     .preset(presets::youtube::youtube_1080p())
//!     .transcode()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Complex Pipeline
//!
//! ```rust,ignore
//! use oximedia_transcode::{TranscodePipeline, Quality};
//! use oximedia_transcode::presets::streaming;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create HLS ABR ladder with multiple qualities
//! TranscodePipeline::builder()
//!     .input("source.mp4")
//!     .abr_ladder(streaming::hls_ladder())
//!     .audio_normalize(true)
//!     .quality(Quality::High)
//!     .parallel_encode(true)
//!     .progress(|p| {
//!         println!("Progress: {}% - ETA: {:?}", p.percent, p.eta);
//!     })
//!     .execute()
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Multi-Pass Encoding
//!
//! ```rust,no_run
//! use oximedia_transcode::{Transcoder, MultiPassMode};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // 2-pass encoding for optimal quality
//! Transcoder::new()
//!     .input("input.mp4")
//!     .output("output.webm")
//!     .multi_pass(MultiPassMode::TwoPass)
//!     .target_bitrate(5_000_000) // 5 Mbps
//!     .transcode()
//!     .await?;
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]
#![allow(clippy::too_many_arguments)]

mod abr;
pub mod adaptive_bitrate;
pub mod alac_bitstream;
#[cfg(not(target_arch = "wasm32"))]
pub mod audio_adapters;
pub mod audio_transcode;
pub mod bitrate_estimator;
mod builder;
mod codec_config;
pub mod codec_dispatch;
pub mod codec_mapping;
#[cfg(not(target_arch = "wasm32"))]
mod container_audio;
pub mod crf_optimizer;
mod filters;
pub mod flac_bitstream;
pub mod flac_decode;
#[cfg(not(target_arch = "wasm32"))]
pub mod frame_adapters;
#[cfg(not(target_arch = "wasm32"))]
mod frame_level;
#[cfg(not(target_arch = "wasm32"))]
pub mod frame_pipeline;
mod hw_accel;
#[cfg(not(target_arch = "wasm32"))]
pub mod multi_track;
mod multipass;
mod normalization;
mod parallel;
#[cfg(not(target_arch = "wasm32"))]
mod pipeline;
#[cfg(not(target_arch = "wasm32"))]
pub mod pipeline_context;
mod progress;
mod quality;
#[cfg(not(target_arch = "wasm32"))]
pub mod raw_sinks;
pub mod segment_encoder;
pub mod segment_transcoder;
pub mod thumbnail;
mod transcode_job;
pub mod two_pass;
pub mod validation;

pub mod ab_compare;
pub mod abr_ladder;
pub mod audio_channel_map;
pub mod audio_only;
pub mod benchmark;
pub mod bitrate_control;
pub mod burn_subs;
pub mod codec_profile;
/// Concatenation and joining of multiple media sources.
pub mod concat_transcode;
pub mod crop_scale;
pub mod encoding_log;
#[cfg(not(target_arch = "wasm32"))]
pub mod examples;
pub mod frame_stats;
pub mod frame_trim;
pub mod hdr_passthrough;
/// Rate-distortion analysis for optimal encoding parameter selection.
pub mod hwaccel;
pub mod output_verify;
pub mod per_scene_encode;
pub mod presets;
pub mod quality_ladder_gen;
pub mod rate_distortion;
pub mod resolution_select;
pub mod running_stats;
pub mod scene_cut;
pub mod stage_graph;
/// Watermark and graphic overlay embedding during transcoding.
pub mod stream_copy;
pub mod stream_map;
pub mod transcode_metrics;
pub mod transcode_preset;
pub mod transcode_profile;
pub mod transcode_session;
pub mod utils;
pub mod watch_folder;
pub mod watermark_overlay;

// ── Newly registered orphan modules ──────────────────────────────────────────
/// Subtitle burn-in pipeline specification (style, safe area, render cues).
pub mod burn_in_spec;
/// Chapter-aware transcoding utilities (chapter map, trim adjustment, merge).
pub mod chapter_transcode;
/// Chroma subsampling selector for per-codec optimal chroma mode.
pub mod chroma_selector;
/// Common Media Application Format (CMAF) init- and media-segment writer.
pub mod cmaf;
/// Codec/container/profile compatibility matrix with constraint validation.
pub mod codec_compat;
/// Multi-level codec capability negotiation with fallback chains.
pub mod codec_negotiation;
/// Audio down-mix coefficient table for multi-channel to fewer-channel reduction.
pub mod downmix_table;
/// ABR encode-ladder validation against CMAF, HLS, and DASH specifications.
pub mod encode_ladder_validator;
/// ETA smoothing — rolling-average / exponential-decay ETA for progress reports.
pub mod eta_smoother;
/// Scene-aware adaptive Group-of-Pictures (GOP) size optimizer.
pub mod gop_optimizer;
/// HDR format enumeration (HDR10, HDR10+, HLG, Dolby Vision) and passthrough config.
pub mod hdr_format;
/// HDR metadata passthrough/conversion bridge across the transcode pipeline.
pub mod hdr_metadata_bridge;
/// Lookahead buffer for scene-adaptive CRF adjustment.
pub mod lookahead_buffer;
/// Large-file I/O abstraction for memory-efficient transcoding.
pub mod mmap_io;
/// Transcode output validation via spec-comparison with configurable tolerances.
pub mod output_validator;
/// Frame-level decode → filter → encode pipeline execution engine.
pub mod pipeline_executor;
/// TranscodeProfile import/export in a compact JSON format.
pub mod profile_io;
/// Exponential back-off retry scheduling for failed transcode jobs.
pub mod retry_backoff;
/// Dolby Atmos and spatial audio passthrough support for the transcode pipeline.
pub mod spatial_audio_passthrough;
/// VTT-compatible sprite sheet / thumbnail strip generator.
pub mod thumbnail_strip;
/// Hash-based transcode result caching with LRU/LFU eviction policy.
pub mod transcode_cache;
/// Enhanced transcode time and resource estimator (CPU, RAM, file size).
pub mod transcode_estimator;
/// Watch folder automation for auto-transcoding with debounce and profiles.
pub mod watch_transcode;
/// Watch folder automation — lightweight stable-file detector.
pub mod watcher;
pub use codec_config::{
    codec_config_from_quality, Av1Config, Av1Usage, CodecConfig, Ffv1Coder, Ffv1Config, Ffv1Level,
    FlacConfig, H264Config, H264Profile, JxlConfig, JxlEffort, OpusApplication, OpusConfig,
    Vp9Config,
};
pub use codec_dispatch::{make_video_encoder, VideoEncoderParams};
pub use codec_profile::CodecTunePreset;
pub use filters::{AudioFilter, FilterNode, VideoFilter};
pub use hw_accel::{
    detect_available_hw_accel, detect_best_hw_accel_for_codec, detect_hw_accel_caps,
    detect_hw_accel_with_probe, get_available_encoders, HwAccelCapabilities, HwAccelConfig,
    HwAccelDevice, HwAccelType, HwEncoder, HwFeature, HwKind, HwProbe, MockProbe, SystemProbe,
};
pub use stream_copy::{
    CopyDecision, StreamCopyConfig, StreamCopyDetector, StreamCopyMode, StreamInfo, StreamType,
};
pub use stream_map::{
    build_index_remap, resolve_stream_selection, StreamKind, StreamMap, StreamMapSelector,
};

pub use abr::{AbrLadder, AbrLadderBuilder, AbrRung, AbrStrategy};
pub use builder::TranscodeBuilder;
pub use concat_transcode::{
    AnnotatedSegment, ConcatPlan, ConcatStep, MixedSourceConcatenator, SourceProperties,
};
#[cfg(not(target_arch = "wasm32"))]
pub use frame_pipeline::{
    wire_hdr_into_pipeline, AudioFrameOp, FramePipelineConfig, FramePipelineExecutor,
    FramePipelineResult, VideoFrameOp,
};
#[cfg(not(target_arch = "wasm32"))]
pub use multi_track::{MultiTrackExecutor, MultiTrackStats, PerTrack};
pub use multipass::{MultiPassConfig, MultiPassEncoder, MultiPassMode};
pub use normalization::{AudioNormalizer, LoudnessStandard, LoudnessTarget, NormalizationConfig};
pub use parallel::{
    assemble_av1_tile_bitstream, Av1TileConfig, Av1TileParallelEncoder, Av1TileStats,
    ParallelConfig, ParallelEncodeBuilder, ParallelEncoder,
};
#[cfg(not(target_arch = "wasm32"))]
pub use pipeline::{
    Pipeline, PipelineConfig, PipelineStage, ScaleSpec, TranscodePipeline, TranscodePipelineBuilder,
};
#[cfg(not(target_arch = "wasm32"))]
pub use pipeline_context::{
    FilterGraph, Frame, FrameDecoder, FrameEncoder, HdrPassthroughConfig, HdrSeiInjector,
    PassStats, TranscodeContext, TranscodeStats,
};
pub use progress::{ProgressCallback, ProgressInfo, ProgressTracker};
pub use quality::{QualityConfig, QualityMode, QualityPreset, RateControlMode, TuneMode};
pub use segment_encoder::{
    ParallelSegmentEncoder, ParallelSegmentResult, ParallelSegmentStats, SegmentSpec,
};
pub use thumbnail::{format_vtt_time, SpriteSheet, SpriteSheetConfig};
pub use transcode_job::{JobPriority, JobQueue, TranscodeJob, TranscodeJobConfig, TranscodeStatus};
pub use transcode_preset::{TranscodeEstimator, TranscodePreset};
pub use validation::{InputValidator, OutputValidator, ValidationError};

use thiserror::Error;

/// Errors that can occur during transcoding operations.
#[derive(Debug, Clone, Error)]
pub enum TranscodeError {
    /// Invalid input file or format.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// Invalid output configuration.
    #[error("Invalid output: {0}")]
    InvalidOutput(String),

    /// Codec error during encoding/decoding.
    #[error("Codec error: {0}")]
    CodecError(String),

    /// Container format error.
    #[error("Container error: {0}")]
    ContainerError(String),

    /// I/O error during transcoding.
    #[error("I/O error: {0}")]
    IoError(String),

    /// Pipeline execution error.
    #[error("Pipeline error: {0}")]
    PipelineError(String),

    /// Multi-pass encoding error.
    #[error("Multi-pass error: {0}")]
    MultiPassError(String),

    /// Audio normalization error.
    #[error("Normalization error: {0}")]
    NormalizationError(String),

    /// Validation error.
    #[error("Validation error: {0}")]
    ValidationError(#[from] ValidationError),

    /// Job execution error.
    #[error("Job error: {0}")]
    JobError(String),

    /// Unsupported operation or feature.
    #[error("Unsupported: {0}")]
    Unsupported(String),
}

impl From<std::io::Error> for TranscodeError {
    fn from(err: std::io::Error) -> Self {
        TranscodeError::IoError(err.to_string())
    }
}

/// Result type for transcoding operations.
pub type Result<T> = std::result::Result<T, TranscodeError>;

/// Main transcoding interface with simple API.
///
/// # Example
///
/// ```rust,no_run
/// use oximedia_transcode::Transcoder;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// Transcoder::new()
///     .input("input.mp4")
///     .output("output.webm")
///     .video_codec("vp9")
///     .audio_codec("opus")
///     .transcode()
///     .await?;
/// # Ok(())
/// # }
/// ```
pub struct Transcoder {
    config: TranscodeConfig,
}

/// Transcoding configuration.
#[derive(Debug, Clone)]
pub struct TranscodeConfig {
    /// Input file path.
    pub input: Option<String>,
    /// Output file path.
    pub output: Option<String>,
    /// Video codec name.
    pub video_codec: Option<String>,
    /// Audio codec name.
    pub audio_codec: Option<String>,
    /// Target video bitrate in bits per second.
    pub video_bitrate: Option<u64>,
    /// Target audio bitrate in bits per second.
    pub audio_bitrate: Option<u64>,
    /// Video width in pixels.
    pub width: Option<u32>,
    /// Video height in pixels.
    pub height: Option<u32>,
    /// Frame rate as a rational number (numerator, denominator).
    pub frame_rate: Option<(u32, u32)>,
    /// Multi-pass encoding mode.
    pub multi_pass: Option<MultiPassMode>,
    /// Quality mode for encoding.
    pub quality_mode: Option<QualityMode>,
    /// Enable audio normalization.
    pub normalize_audio: bool,
    /// Loudness normalization standard.
    pub loudness_standard: Option<LoudnessStandard>,
    /// Enable hardware acceleration.
    pub hw_accel: bool,
    /// Preserve metadata from input.
    pub preserve_metadata: bool,
    /// Subtitle handling mode.
    pub subtitle_mode: Option<SubtitleMode>,
    /// Chapter handling mode.
    pub chapter_mode: Option<ChapterMode>,
    /// Stream copy mode for passthrough without re-encoding.
    pub stream_copy: Option<StreamCopyMode>,
    /// Audio channel layout for output.
    pub audio_channel_layout: Option<audio_channel_map::AudioLayout>,
}

/// Subtitle handling modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleMode {
    /// Ignore subtitles.
    Ignore,
    /// Copy subtitles as separate stream.
    Copy,
    /// Burn subtitles into video.
    BurnIn,
}

/// Chapter handling modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChapterMode {
    /// Ignore chapters.
    Ignore,
    /// Copy chapters from input.
    Copy,
    /// Add custom chapters.
    Custom,
}

impl Default for TranscodeConfig {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            video_codec: None,
            audio_codec: None,
            video_bitrate: None,
            audio_bitrate: None,
            width: None,
            height: None,
            frame_rate: None,
            multi_pass: None,
            quality_mode: None,
            normalize_audio: false,
            loudness_standard: None,
            hw_accel: true,
            preserve_metadata: true,
            subtitle_mode: None,
            chapter_mode: None,
            stream_copy: None,
            audio_channel_layout: None,
        }
    }
}

impl Transcoder {
    /// Get a reference to the transcoder configuration.
    #[must_use]
    pub fn config(&self) -> &TranscodeConfig {
        &self.config
    }

    /// Creates a new transcoder with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: TranscodeConfig::default(),
        }
    }

    /// Sets the input file path.
    #[must_use]
    pub fn input(mut self, path: impl Into<String>) -> Self {
        self.config.input = Some(path.into());
        self
    }

    /// Sets the output file path.
    #[must_use]
    pub fn output(mut self, path: impl Into<String>) -> Self {
        self.config.output = Some(path.into());
        self
    }

    /// Sets the video codec.
    #[must_use]
    pub fn video_codec(mut self, codec: impl Into<String>) -> Self {
        self.config.video_codec = Some(codec.into());
        self
    }

    /// Sets the audio codec.
    #[must_use]
    pub fn audio_codec(mut self, codec: impl Into<String>) -> Self {
        self.config.audio_codec = Some(codec.into());
        self
    }

    /// Sets the target video bitrate.
    #[must_use]
    pub fn video_bitrate(mut self, bitrate: u64) -> Self {
        self.config.video_bitrate = Some(bitrate);
        self
    }

    /// Sets the target audio bitrate.
    #[must_use]
    pub fn audio_bitrate(mut self, bitrate: u64) -> Self {
        self.config.audio_bitrate = Some(bitrate);
        self
    }

    /// Sets the output resolution.
    #[must_use]
    pub fn resolution(mut self, width: u32, height: u32) -> Self {
        self.config.width = Some(width);
        self.config.height = Some(height);
        self
    }

    /// Sets the output frame rate.
    #[must_use]
    pub fn frame_rate(mut self, num: u32, den: u32) -> Self {
        self.config.frame_rate = Some((num, den));
        self
    }

    /// Sets the multi-pass encoding mode.
    #[must_use]
    pub fn multi_pass(mut self, mode: MultiPassMode) -> Self {
        self.config.multi_pass = Some(mode);
        self
    }

    /// Sets the quality mode.
    #[must_use]
    pub fn quality(mut self, mode: QualityMode) -> Self {
        self.config.quality_mode = Some(mode);
        self
    }

    /// Sets the target bitrate (convenience method for video bitrate).
    #[must_use]
    pub fn target_bitrate(mut self, bitrate: u64) -> Self {
        self.config.video_bitrate = Some(bitrate);
        self
    }

    /// Enables or disables audio normalization.
    #[must_use]
    pub fn normalize_audio(mut self, enable: bool) -> Self {
        self.config.normalize_audio = enable;
        self
    }

    /// Sets the loudness normalization standard.
    #[must_use]
    pub fn loudness_standard(mut self, standard: LoudnessStandard) -> Self {
        self.config.loudness_standard = Some(standard);
        self.config.normalize_audio = true;
        self
    }

    /// Enables or disables hardware acceleration.
    #[must_use]
    pub fn hw_accel(mut self, enable: bool) -> Self {
        self.config.hw_accel = enable;
        self
    }

    /// Sets the stream copy mode for passthrough without re-encoding.
    ///
    /// When codecs match between input and output, stream copy avoids
    /// re-encoding and preserves the original quality.
    #[must_use]
    pub fn stream_copy(mut self, mode: StreamCopyMode) -> Self {
        self.config.stream_copy = Some(mode);
        self
    }

    /// Sets the audio channel layout for the output.
    #[must_use]
    pub fn audio_channel_layout(mut self, layout: audio_channel_map::AudioLayout) -> Self {
        self.config.audio_channel_layout = Some(layout);
        self
    }

    /// Applies a preset configuration.
    #[must_use]
    pub fn preset(mut self, preset: PresetConfig) -> Self {
        if let Some(codec) = preset.video_codec {
            self.config.video_codec = Some(codec);
        }
        if let Some(codec) = preset.audio_codec {
            self.config.audio_codec = Some(codec);
        }
        if let Some(bitrate) = preset.video_bitrate {
            self.config.video_bitrate = Some(bitrate);
        }
        if let Some(bitrate) = preset.audio_bitrate {
            self.config.audio_bitrate = Some(bitrate);
        }
        if let Some(width) = preset.width {
            self.config.width = Some(width);
        }
        if let Some(height) = preset.height {
            self.config.height = Some(height);
        }
        if let Some(fps) = preset.frame_rate {
            self.config.frame_rate = Some(fps);
        }
        if let Some(mode) = preset.quality_mode {
            self.config.quality_mode = Some(mode);
        }
        if let Some(layout) = preset.audio_channel_layout {
            self.config.audio_channel_layout = Some(layout);
        }
        self
    }

    /// Executes the transcode operation.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Input or output path is not set
    /// - Input file is invalid or cannot be opened
    /// - Output configuration is invalid
    /// - Transcoding fails
    /// - On wasm32 targets (filesystem-based transcoding is not supported)
    pub async fn transcode(self) -> Result<TranscodeOutput> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self;
            return Err(TranscodeError::Unsupported(
                "Filesystem-based transcoding is not supported on wasm32".to_string(),
            ));
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            // Validate configuration
            let input = self.config.input.ok_or_else(|| {
                TranscodeError::InvalidInput("No input file specified".to_string())
            })?;
            let output = self.config.output.ok_or_else(|| {
                TranscodeError::InvalidOutput("No output file specified".to_string())
            })?;

            // Create a basic pipeline and execute
            let mut pipeline = TranscodePipeline::builder()
                .input(&input)
                .output(&output)
                .build()?;

            // Apply configuration to pipeline
            if let Some(codec) = &self.config.video_codec {
                pipeline.set_video_codec(codec);
            }
            if let Some(codec) = &self.config.audio_codec {
                pipeline.set_audio_codec(codec);
            }

            // Execute pipeline
            pipeline.execute().await
        }
    }
}

impl Default for Transcoder {
    fn default() -> Self {
        Self::new()
    }
}

/// Preset configuration for common transcoding scenarios.
#[derive(Debug, Clone, Default)]
pub struct PresetConfig {
    /// Video codec name.
    pub video_codec: Option<String>,
    /// Audio codec name.
    pub audio_codec: Option<String>,
    /// Video bitrate.
    pub video_bitrate: Option<u64>,
    /// Audio bitrate.
    pub audio_bitrate: Option<u64>,
    /// Video width.
    pub width: Option<u32>,
    /// Video height.
    pub height: Option<u32>,
    /// Frame rate.
    pub frame_rate: Option<(u32, u32)>,
    /// Quality mode.
    pub quality_mode: Option<QualityMode>,
    /// Container format.
    pub container: Option<String>,
    /// Audio channel layout (mono, stereo, 5.1, 7.1).
    pub audio_channel_layout: Option<audio_channel_map::AudioLayout>,
}

/// Output from a successful transcode operation.
#[derive(Debug, Clone)]
pub struct TranscodeOutput {
    /// Output file path.
    pub output_path: String,
    /// File size in bytes.
    pub file_size: u64,
    /// Duration in seconds.
    pub duration: f64,
    /// Video bitrate in bits per second.
    pub video_bitrate: u64,
    /// Audio bitrate in bits per second.
    pub audio_bitrate: u64,
    /// Actual encoding time in seconds.
    pub encoding_time: f64,
    /// Speed factor (input duration / encoding time).
    pub speed_factor: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transcoder_builder() {
        let transcoder = Transcoder::new()
            .input("input.mp4")
            .output("output.webm")
            .video_codec("vp9")
            .audio_codec("opus")
            .resolution(1920, 1080)
            .frame_rate(30, 1);

        assert_eq!(transcoder.config.input, Some("input.mp4".to_string()));
        assert_eq!(transcoder.config.output, Some("output.webm".to_string()));
        assert_eq!(transcoder.config.video_codec, Some("vp9".to_string()));
        assert_eq!(transcoder.config.audio_codec, Some("opus".to_string()));
        assert_eq!(transcoder.config.width, Some(1920));
        assert_eq!(transcoder.config.height, Some(1080));
        assert_eq!(transcoder.config.frame_rate, Some((30, 1)));
    }

    #[test]
    fn test_default_config() {
        let config = TranscodeConfig::default();
        assert!(config.input.is_none());
        assert!(config.output.is_none());
        assert!(config.hw_accel);
        assert!(config.preserve_metadata);
        assert!(!config.normalize_audio);
    }

    #[test]
    fn test_preset_application() {
        let preset = PresetConfig {
            video_codec: Some("vp9".to_string()),
            audio_codec: Some("opus".to_string()),
            video_bitrate: Some(5_000_000),
            audio_bitrate: Some(128_000),
            width: Some(1920),
            height: Some(1080),
            frame_rate: Some((60, 1)),
            quality_mode: Some(QualityMode::High),
            container: Some("webm".to_string()),
            audio_channel_layout: None,
        };

        let transcoder = Transcoder::new().preset(preset);

        assert_eq!(transcoder.config.video_codec, Some("vp9".to_string()));
        assert_eq!(transcoder.config.audio_codec, Some("opus".to_string()));
        assert_eq!(transcoder.config.video_bitrate, Some(5_000_000));
        assert_eq!(transcoder.config.audio_bitrate, Some(128_000));
        assert_eq!(transcoder.config.width, Some(1920));
        assert_eq!(transcoder.config.height, Some(1080));
        assert_eq!(transcoder.config.frame_rate, Some((60, 1)));
        assert_eq!(transcoder.config.quality_mode, Some(QualityMode::High));
    }

    #[test]
    fn test_stream_copy_mode() {
        let transcoder = Transcoder::new()
            .input("input.mp4")
            .output("output.mp4")
            .stream_copy(StreamCopyMode::CopyVideo);

        assert_eq!(
            transcoder.config.stream_copy,
            Some(StreamCopyMode::CopyVideo)
        );
    }

    #[test]
    fn test_audio_channel_layout_on_transcoder() {
        let transcoder =
            Transcoder::new().audio_channel_layout(audio_channel_map::AudioLayout::FivePointOne);

        assert_eq!(
            transcoder.config.audio_channel_layout,
            Some(audio_channel_map::AudioLayout::FivePointOne)
        );
    }

    #[test]
    fn test_preset_with_audio_channel_layout() {
        let preset = PresetConfig {
            audio_codec: Some("opus".to_string()),
            audio_bitrate: Some(384_000),
            audio_channel_layout: Some(audio_channel_map::AudioLayout::FivePointOne),
            ..PresetConfig::default()
        };

        let transcoder = Transcoder::new().preset(preset);
        assert_eq!(
            transcoder.config.audio_channel_layout,
            Some(audio_channel_map::AudioLayout::FivePointOne)
        );
        assert_eq!(transcoder.config.audio_bitrate, Some(384_000));
    }

    #[test]
    fn test_preset_config_default_has_no_channel_layout() {
        let preset = PresetConfig::default();
        assert!(preset.audio_channel_layout.is_none());
    }

    #[test]
    fn test_config_default_has_no_stream_copy() {
        let config = TranscodeConfig::default();
        assert!(config.stream_copy.is_none());
        assert!(config.audio_channel_layout.is_none());
    }

    #[test]
    fn test_subtitle_modes() {
        assert_eq!(SubtitleMode::Ignore, SubtitleMode::Ignore);
        assert_ne!(SubtitleMode::Ignore, SubtitleMode::Copy);
        assert_ne!(SubtitleMode::Copy, SubtitleMode::BurnIn);
    }

    #[test]
    fn test_chapter_modes() {
        assert_eq!(ChapterMode::Ignore, ChapterMode::Ignore);
        assert_ne!(ChapterMode::Ignore, ChapterMode::Copy);
        assert_ne!(ChapterMode::Copy, ChapterMode::Custom);
    }
}
