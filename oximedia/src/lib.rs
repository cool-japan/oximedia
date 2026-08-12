//! # OxiMedia — The Sovereign Media Framework
//!
//! A patent-free, memory-safe multimedia processing library written in pure Rust.
//! OxiMedia is the single crate that unifies the entire OxiMedia ecosystem —
//! covering everything from raw codec primitives to broadcast-grade MAM workflows.
//!
//! ## Design Principles
//!
//! - **Patent-Free**: Only royalty-free codecs (AV1, VP9, VP8, Opus, Vorbis, FLAC, PCM)
//! - **Memory Safe**: Pure Rust, `#![forbid(unsafe_code)]` throughout
//! - **Async-First**: Built on Tokio for high-concurrency media pipelines
//! - **Zero-Copy**: Efficient buffer management at every layer
//! - **Feature-Gated**: Pay only for what you use — the default build is lean
//!
//! ## Quick Start
//!
//! Add to `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! oximedia = { version = "0.1", features = ["audio", "video"] }
//! ```
//!
//! Then in your code:
//!
//! ```ignore
//! use oximedia::prelude::*;
//!
//! // Probe a media file
//! let format = probe_format(&data)?;
//! println!("Container: {:?}", format);
//! ```
//!
//! ## Runnable Examples
//!
//! The three doctests below are compiled *and executed* by `cargo test --doc`
//! (unlike the `ignore`d snippet above, which needs a real file on disk).
//! Feature-gated sections are wrapped in `# #[cfg(feature = "…")]` blocks so
//! the doctest as a whole always compiles; the gated assertions only run
//! when the corresponding feature is enabled (e.g. `cargo test --doc
//! --features dedup,transcode,quality`).
//!
//! ### Probing + deduplication
//!
//! ```
//! use oximedia::probe_format;
//!
//! // Minimal ftyp box: size(4) + "ftyp"(4) + brand "isom"(4) + version(4).
//! let mut mp4_header = vec![0u8; 16];
//! mp4_header[0..4].copy_from_slice(&16u32.to_be_bytes());
//! mp4_header[4..8].copy_from_slice(b"ftyp");
//! mp4_header[8..12].copy_from_slice(b"isom");
//!
//! // Probing a well-formed (if minimal) header must not panic.
//! let _ = probe_format(&mp4_header);
//!
//! # #[cfg(feature = "dedup")]
//! # {
//! use oximedia::dedup::DedupConfig;
//!
//! // A fresh deduplication engine ships with sane default thresholds.
//! let config = DedupConfig::default();
//! assert!(config.perceptual_threshold > 0.9);
//! assert!(config.parallel, "parallel scanning is on by default");
//! # }
//! ```
//!
//! ### Transcode configuration + quality assessment
//!
//! ```
//! # #[cfg(feature = "transcode")]
//! # {
//! use oximedia::transcode::TranscodeConfig;
//!
//! let config = TranscodeConfig {
//!     video_codec: Some("av1".to_string()),
//!     audio_codec: Some("opus".to_string()),
//!     ..TranscodeConfig::default()
//! };
//! assert_eq!(config.video_codec.as_deref(), Some("av1"));
//! # }
//!
//! # #[cfg(feature = "quality")]
//! # {
//! use oximedia::quality::{Frame, MetricType, QualityAssessor};
//! use oximedia::PixelFormat;
//!
//! let frame = Frame::new(16, 16, PixelFormat::Gray8).expect("frame creation must succeed");
//! let assessor = QualityAssessor::new();
//! let score = assessor
//!     .assess(&frame, &frame, MetricType::Psnr)
//!     .expect("PSNR of identical frames must succeed");
//! assert!(score.score.is_infinite() || score.score >= 60.0);
//! # }
//! ```
//!
//! ### Prelude quick-start
//!
//! ```
//! use oximedia::prelude::*;
//!
//! // `Timestamp` + `Rational` are always available (core re-exports).
//! let ts = Timestamp::new(90_000, Rational::new(1, 90_000));
//! assert!((ts.to_seconds() - 1.0).abs() < f64::EPSILON);
//!
//! // `CodecId` classification helpers.
//! let codec = CodecId::Av1;
//! assert!(codec.is_video());
//! ```
//!
//! ## Feature Flags
//!
//! | Feature | Crates enabled | Purpose |
//! |---------|---------------|---------|
//! | `audio` | `oximedia-audio` | Opus, Vorbis, FLAC, PCM codecs |
//! | `video` | `oximedia-codec` | AV1, VP9, VP8 video codecs |
//! | `graph` | `oximedia-graph` | Filter graph / processing pipeline |
//! | `effects` | `oximedia-effects` | Professional audio effects suite |
//! | `net` | `oximedia-net` | HLS, DASH, SRT, RTMP, WebRTC |
//! | `metering` | `oximedia-metering` | EBU R128, ATSC A/85 loudness |
//! | `normalize` | `oximedia-normalize` | Loudness normalization |
//! | `quality` | `oximedia-quality` | PSNR, SSIM, VMAF, NIQE |
//! | `metadata-ext` | `oximedia-metadata` | ID3v2, XMP, EXIF, IPTC |
//! | `timecode` | `oximedia-timecode` | SMPTE LTC/VITC timecode |
//! | `workflow` | `oximedia-workflow` | DAG workflow orchestration |
//! | `batch` | `oximedia-batch` | Batch job processing engine |
//! | `monitor` | `oximedia-monitor` | System monitoring and alerting |
//! | `lut` | `oximedia-lut` | 1D/3D LUT and HDR pipeline |
//! | `colormgmt` | `oximedia-colormgmt` | ICC, ACES, HDR color management |
//! | `transcode` | `oximedia-transcode` | Full transcoding pipeline |
//! | `subtitle` | `oximedia-subtitle` | SRT, ASS, WebVTT rendering |
//! | `captions` | `oximedia-captions` | Closed caption formats |
//! | `archive` | `oximedia-archive` | Archive verification & preservation |
//! | `dedup` | `oximedia-dedup` | Media deduplication |
//! | `search` | `oximedia-search` | Media search and indexing |
//! | `mam` | `oximedia-mam` | Media Asset Management system |
//! | `scene` | `oximedia-scene` | AI scene understanding |
//! | `shots` | `oximedia-shots` | Shot detection & classification |
//! | `scopes` | `oximedia-scopes` | Broadcast video scopes |
//! | `vfx` | `oximedia-vfx` | Visual effects and compositing |
//! | `image-ext` | `oximedia-image` | Advanced image processing (DPX, EXR, TIFF) |
//! | `watermark` | `oximedia-watermark` | Audio watermarking and forensic detection |
//! | `mir` | `oximedia-mir` | Music Information Retrieval |
//! | `recommend` | `oximedia-recommend` | Content recommendation engine |
//! | `playlist` | `oximedia-playlist` | Broadcast playlist management |
//! | `playout` | `oximedia-playout` | Broadcast playout server |
//! | `rights` | `oximedia-rights` | Digital rights management |
//! | `review` | `oximedia-review` | Collaborative media review |
//! | `restore` | `oximedia-restore` | Audio/video restoration |
//! | `repair` | `oximedia-repair` | Media file repair and recovery |
//! | `multicam` | `oximedia-multicam` | Multi-camera sync and switching |
//! | `stabilize` | `oximedia-stabilize` | Video stabilization |
//! | `cloud` | `oximedia-cloud` | Cloud storage abstraction (S3, Azure, GCS) |
//! | `edl` | `oximedia-edl` | EDL parsing and generation |
//! | `ndi` | `oximedia-ndi` | NDI protocol support |
//! | `imf` | `oximedia-imf` | IMF package support (SMPTE ST 2067) |
//! | `aaf` | `oximedia-aaf` | AAF interchange (SMPTE ST 377-1) |
//! | `timesync` | `oximedia-timesync` | PTP/NTP time synchronization |
//! | `forensics` | `oximedia-forensics` | Media forensics and tampering detection |
//! | `accel` | `oximedia-accel` | Hardware acceleration (Vulkan GPU, CPU fallback) |
//! | `simd` | `oximedia-simd` | SIMD-optimised media kernels (DCT, SAD, blending) |
//! | `switcher` | `oximedia-switcher` | Professional live video switcher |
//! | `timeline` | `oximedia-timeline` | Multi-track timeline editor |
//! | `optimize` | `oximedia-optimize` | Codec optimisation suite (RDO, psychovisual, AQ) |
//! | `profiler` | `oximedia-profiler` | Performance profiling tools |
//! | `renderfarm` | `oximedia-renderfarm` | Distributed render farm coordinator |
//! | `storage` | `oximedia-storage` | Cloud-agnostic object storage (S3, Azure, GCS) |
//! | `collab` | `oximedia-collab` | Real-time CRDT collaborative editing |
//! | `gaming` | `oximedia-gaming` | Game streaming and screen capture |
//! | `virtual-prod` | `oximedia-virtual` | Virtual production and LED wall tools |
//! | `access` | `oximedia-access` | Accessibility (audio description, captions, WCAG) |
//! | `conform` | `oximedia-conform` | Media conforming (EDL/XML/AAF matching) |
//! | `convert` | `oximedia-convert` | Media format conversion utilities |
//! | `automation` | `oximedia-automation` | Broadcast automation and master control |
//! | `clips` | `oximedia-clips` | Professional clip management and logging |
//! | `proxy` | `oximedia-proxy` | Proxy and offline editing workflows |
//! | `presets` | `oximedia-presets` | Encoding preset library (200+ presets) |
//! | `calibrate` | `oximedia-calibrate` | Color calibration and camera profiling |
//! | `denoise` | `oximedia-denoise` | Video denoising (spatial, temporal, hybrid) |
//! | `align` | `oximedia-align` | Multi-camera video alignment and registration |
//! | `analysis` | `oximedia-analysis` | Comprehensive media analysis and QA |
//! | `audiopost` | `oximedia-audiopost` | Audio post-production (ADR, Foley, mixing) |
//! | `qc` | `oximedia-qc` | Broadcast-grade quality control and validation |
//! | `jobs` | `oximedia-jobs` | Job queue and worker management |
//! | `auto` | `oximedia-auto` | Automated video editing and highlight detection |
//! | `edit` | `oximedia-edit` | Video timeline editor with effects |
//! | `routing` | `oximedia-routing` | Signal routing, NMOS IS-04/IS-05/IS-07 |
//! | `audio-analysis` | `oximedia-audio-analysis` | Spectral, voice, music, forensics analysis |
//! | `gpu` | `oximedia-gpu` | WGPU GPU compute (Vulkan, Metal, DX12, WebGPU) |
//! | `packager` | `oximedia-packager` | HLS/DASH adaptive streaming packaging |
//! | `drm` | `oximedia-drm` | CENC, Widevine, PlayReady, FairPlay DRM |
//! | `archive-pro` | `oximedia-archive-pro` | BagIt, OAIS, PREMIS digital preservation |
//! | `distributed` | `oximedia-distributed` | Distributed multi-node encoding |
//! | `farm` | `oximedia-farm` | Render farm coordinator |
//! | `dolbyvision` | `oximedia-dolbyvision` | Dolby Vision RPU metadata |
//! | `mixer` | `oximedia-mixer` | Professional digital audio mixer |
//! | `scaling` | `oximedia-scaling` | High-quality video scaling |
//! | `graphics` | `oximedia-graphics` | Broadcast graphics engine |
//! | `videoip` | `oximedia-videoip` | Video-over-IP protocol |
//! | `compat-ffmpeg` | `oximedia-compat-ffmpeg` | FFmpeg CLI compatibility layer |
//! | `plugin` | `oximedia-plugin` | Dynamic/static codec plugin system |
//! | `server` | `oximedia-server` | RESTful media server |
//! | `hdr` | `oximedia-hdr` | HDR video processing (PQ/HLG, tone mapping, HDR10+) |
//! | `spatial` | `oximedia-spatial` | Spatial audio (Ambisonics, binaural, room simulation) |
//! | `cache` | `oximedia-cache` | High-performance media caching (LRU, tiered, warming) |
//! | `stream` | `oximedia-stream` | Adaptive streaming pipeline, segment management, QoE |
//! | `video-proc` | `oximedia-video` | Scene detection, pulldown detection, temporal denoising, perceptual fingerprinting |
//! | `cdn` | `oximedia-cdn` | CDN edge management, cache invalidation, geographic routing, origin failover |
//! | `neural` | `oximedia-neural` | Lightweight neural network inference for media (tensor ops, conv2d, scene classification) |
//! | `vr360` | `oximedia-360` | 360° VR video: equirectangular/cubemap projections, fisheye, stereo 3D |
//! | `analytics` | `oximedia-analytics` | Media engagement analytics: sessions, retention curves, A/B testing, scoring |
//! | `caption-gen` | `oximedia-caption-gen` | Advanced caption generation: speech alignment, WCAG compliance, diarization |
//! | `image-transform` | `oximedia-image-transform` | Image transformations: affine, perspective, resize, crop, rotate, color conversion, lens distortion |
//! | `pipeline` | `oximedia-pipeline` | Declarative media processing DSL: typed filter graph, node composition, execution planning |
//! | `capture` | `oximedia-capture` | Live camera/device capture (AVFoundation/V4L2/Media Foundation); raw pixel formats + MJPEG passthrough |
//! | `mjpeg` | `oximedia-codec` (mjpeg) | Motion JPEG intra-frame video codec |
//! | `apv` | `oximedia-codec` (apv) | APV (Advanced Professional Video) intra-frame codec (ISO/IEC 23009-13) |
//! | `full` | all of the above | Everything enabled |
//!
//! ## Implicit Feature Dependencies
//!
//! Some features automatically activate other features:
//!
//! - **`normalize`** → also activates **`metering`**: enabling loudness normalization
//!   requires the EBU R128 metering infrastructure.  Users who enable `normalize` will
//!   find `LoudnessMeter` (`oximedia_metering::LoudnessMeter`) and related types in scope
//!   even if they did not explicitly request the `metering` feature.
//!
//! ## Meta-Feature Presets
//!
//! In addition to per-crate feature flags, OxiMedia provides convenience groupings:
//!
//! - **`minimal`**: `oximedia-core` + `oximedia-codec` + `oximedia-metadata` — the
//!   smallest useful set for basic decoding without the full feature tree.
//! - **`audio-stack`**: everything needed for a professional audio pipeline
//!   (`audio`, `effects`, `metering`, `normalize`, `audio-analysis`, `mixer`, `audiopost`).
//! - **`broadcast-stack`**: broadcast production tools
//!   (`automation`, `playout`, `playlist`, `switcher`, `routing`, `graphics`, `scopes`).
//! - **`streaming-stack`**: live streaming infrastructure
//!   (`net`, `packager`, `drm`, `stream`, `cdn`, `cache`, `server`).
//! - **`full`**: every optional feature enabled.
//!
//! ## Cookbook
//!
//! Worked examples live in the workspace `examples/` directory and are run
//! with `cargo run --example <name> --features <flags>` (a bare `--` with no
//! flags means the example only needs the always-on core). See the crate
//! README for a one-line description of what each example demonstrates.
//!
//! | Example file | Required features |
//! |---|---|
//! | `probe_file.rs` | *(none)* |
//! | `corner_detection.rs` | *(none)* |
//! | `optical_flow.rs` | *(none)* |
//! | `face_detection.rs` | *(none)* |
//! | `image_processing.rs` | *(none)* |
//! | `decode_video.rs` | *(none)* |
//! | `audio_metering.rs` | `metering` |
//! | `quality_assessment.rs` | `quality` |
//! | `timecode_operations.rs` | `timecode` |
//! | `dedup_detection.rs` | `dedup` |
//! | `workflow_pipeline.rs` | `workflow` |
//! | `video_scopes.rs` | `scopes` |
//! | `shot_detection.rs` | `shots` |
//! | `nmos_registry.rs` | `routing` |
//! | `color_pipeline.rs` | `colormgmt`, `lut` |
//! | `media_pipeline.rs` | `quality`, `metering`, `transcode`, `timecode`, `workflow`, `archive` |
//! | `nmos_server_demo.rs` | `routing` |
//! | `ml_scene_classify.rs` | `ml`, `ml-scene-classifier` |
//! | `ml_auto_caption.rs` | `ml` |
//! | `ml_model_zoo.rs` | `ml` |
//! | `ffmpeg_translate_demo.rs` | `compat-ffmpeg` |
//!
//! Every file above except `ffmpeg_translate_demo.rs` lives in the
//! workspace-root `examples/` directory (`cargo run --example <name>` finds
//! it via the `[[example]]` table in `Cargo.toml`); `ffmpeg_translate_demo.rs`
//! lives in `oximedia/examples/` instead.
//!

#![forbid(unsafe_code)]
#![warn(missing_docs)]

// ── Always-on core re-exports ───────────────────────────────────────────────

/// Core OxiMedia types: errors, codecs, pixel/sample formats, timestamps.
pub use oximedia_core::{
    CodecId, MediaType, OxiError, OxiResult, PixelFormat, Rational, SampleFormat, Timestamp,
};

#[cfg(not(target_arch = "wasm32"))]
pub use oximedia_io::FileSource;
/// I/O primitives: byte readers and media source abstractions.
///
/// `FileSource` is only re-exported off `wasm32-unknown-unknown` — WASM
/// targets have no `std::fs`, so the file-backed source is omitted and
/// callers are expected to use [`MemorySource`] or a custom
/// [`MediaSource`] implementation.
pub use oximedia_io::{BitReader, MediaSource, MemorySource};

/// Container layer: probing, demuxing, packets, stream descriptors.
pub use oximedia_container::{
    probe_format, CodecParams, ContainerFormat, Demuxer, Metadata, Packet, PacketFlags,
    ProbeResult, StreamInfo,
};

/// Computer vision primitives (always available).
pub use oximedia_cv as cv;

// ── Feature-gated domain modules ────────────────────────────────────────────

/// Audio processing: codecs (Opus, Vorbis, FLAC, PCM), frames, resampling.
///
/// Enable with `features = ["audio"]`.
///
/// See the [`oximedia_audio`] crate for the full API surface.
#[cfg(feature = "audio")]
pub mod audio {
    //! Audio processing subsystem.
    //!
    //! Provides audio encoding/decoding for all patent-free codecs supported by
    //! OxiMedia, together with resampling, channel layout management, and the
    //! unified `AudioDecoder` / `AudioEncoder` traits.
    pub use oximedia_audio::*;
}

/// Video codec support: AV1, VP9, VP8 encoding/decoding.
///
/// Enable with `features = ["video"]`.
///
/// See the [`oximedia_codec`] crate for the full API surface.
#[cfg(feature = "video")]
pub mod video {
    //! Video codec subsystem.
    //!
    //! Provides encoding and decoding for royalty-free video codecs: AV1 (primary),
    //! VP9, and VP8.  All codecs implement the unified `VideoDecoder` /
    //! `VideoEncoder` traits.
    pub use oximedia_codec::*;
}

/// Filter graph pipeline: nodes, ports, connections, frame routing.
///
/// Enable with `features = ["graph"]`.
///
/// See the [`oximedia_graph`] crate for the full API surface.
#[cfg(feature = "graph")]
pub mod graph {
    //! Filter graph processing pipeline.
    //!
    //! Builds directed-acyclic graphs of media processing nodes connected by typed
    //! ports.  Supports source, filter, and sink node types with automatic
    //! topological scheduling.
    pub use oximedia_graph::*;
}

/// Professional audio effects: reverb, delay, compression, EQ, pitch, and more.
///
/// Enable with `features = ["effects"]`.
///
/// See the [`oximedia_effects`] crate for the full API surface.
#[cfg(feature = "effects")]
pub mod effects {
    //! Professional audio effects suite.
    //!
    //! All effects implement the `AudioEffect` trait for a unified real-time
    //! processing interface.  Includes reverb, delay, modulation, distortion,
    //! dynamics, filtering, pitch/time, and vocoding effects.
    pub use oximedia_effects::*;
}

/// Network streaming: HLS, DASH, SRT, RTMP, WebRTC, SMPTE ST 2110.
///
/// Enable with `features = ["net"]`.
///
/// See the [`oximedia_net`] crate for the full API surface.
#[cfg(feature = "net")]
pub mod net {
    //! Network streaming subsystem.
    //!
    //! Adaptive-bitrate streaming (HLS/DASH), low-latency SRT, broadcast RTMP,
    //! browser-based WebRTC, and professional SMPTE ST 2110 media-over-IP.
    pub use oximedia_net::*;
}

/// Broadcast loudness metering: EBU R128, ATSC A/85, ITU-R BS.1770-4.
///
/// Enable with `features = ["metering"]`.
///
/// See the [`oximedia_metering`] crate for the full API surface.
#[cfg(feature = "metering")]
pub mod metering {
    //! Broadcast-standard audio loudness metering.
    //!
    //! Standards-compliant loudness measurement (LUFS/LKFS), true-peak detection,
    //! loudness-range (LRA), K-system meters, phase correlation, and spectrum
    //! analysis.
    pub use oximedia_metering::*;
}

/// Loudness normalization: two-pass, real-time, ReplayGain, streaming targets.
///
/// Enable with `features = ["normalize"]`.
///
/// See the [`oximedia_normalize`] crate for the full API surface.
#[cfg(feature = "normalize")]
pub mod normalize {
    //! Broadcast loudness normalization.
    //!
    //! Two-pass and real-time normalization targeting EBU R128, ATSC A/85, Spotify,
    //! YouTube, Apple Music, Netflix, and other streaming platforms.
    pub use oximedia_normalize::*;
}

/// Video quality assessment: PSNR, SSIM, MS-SSIM, VMAF, VIF, NIQE, BRISQUE.
///
/// Enable with `features = ["quality"]`.
///
/// See the [`oximedia_quality`] crate for the full API surface.
#[cfg(feature = "quality")]
pub mod quality {
    //! Video quality assessment and objective metrics.
    //!
    //! Full-reference metrics (PSNR, SSIM, MS-SSIM, VMAF, VIF, FSIM) and
    //! no-reference metrics (NIQE, BRISQUE, blockiness, blur, noise estimation).
    pub use oximedia_quality::*;
}

/// Extended metadata: ID3v2, Vorbis Comments, APEv2, iTunes, XMP, EXIF, IPTC.
///
/// Enable with `features = ["metadata-ext"]`.
///
/// See the [`oximedia_metadata`] crate for the full API surface.
#[cfg(feature = "metadata-ext")]
pub mod metadata_ext {
    //! Comprehensive media metadata support.
    //!
    //! Parse and write all major metadata formats across audio, video, and image
    //! containers.  Includes picture/artwork handling, format conversion, and
    //! metadata validation.
    pub use oximedia_metadata::*;
}

/// SMPTE timecode: LTC and VITC reading/writing at all standard frame rates.
///
/// Enable with `features = ["timecode"]`.
///
/// See the [`oximedia_timecode`] crate for the full API surface.
#[cfg(feature = "timecode")]
pub mod timecode {
    //! SMPTE 12M timecode reading and writing.
    //!
    //! Supports LTC (Linear Timecode) and VITC (Vertical Interval Timecode) for
    //! all standard frame rates with drop-frame and non-drop-frame modes.
    pub use oximedia_timecode::*;
}

/// Workflow orchestration: DAG-based workflows, scheduling, persistence.
///
/// Enable with `features = ["workflow"]`.
///
/// See the [`oximedia_workflow`] crate for the full API surface.
#[cfg(feature = "workflow")]
pub mod workflow {
    //! DAG-based workflow orchestration engine.
    //!
    //! Define media processing workflows as directed-acyclic graphs of typed tasks
    //! (transcode, QC, transfer, notify, …) with dependency tracking, cron-style
    //! scheduling, SQLite state persistence, and real-time monitoring.
    pub use oximedia_workflow::*;
}

/// Batch processing: job queuing, worker pools, watch-folder automation.
///
/// Enable with `features = ["batch"]`.
///
/// See the [`oximedia_batch`] crate for the full API surface.
#[cfg(feature = "batch")]
pub mod batch {
    //! Production-ready batch processing engine.
    //!
    //! Job queuing and scheduling, configurable worker pools, template-based
    //! configuration, watch-folder automation, and distributed processing support.
    pub use oximedia_batch::*;
}

/// System monitoring: metrics, alerting, health checks, Prometheus export.
///
/// Enable with `features = ["monitor"]`.
///
/// See the [`oximedia_monitor`] crate for the full API surface.
#[cfg(feature = "monitor")]
pub mod monitor {
    //! Comprehensive system and application monitoring.
    //!
    //! CPU/memory/GPU metrics, encoding throughput, quality scores, multi-channel
    //! alerting (email, Slack, Discord, webhook), WebSocket streaming, health
    //! checks, and Prometheus-compatible metric export.
    pub use oximedia_monitor::*;
}

/// LUT processing: 1D/3D LUTs with tetrahedral interpolation, HDR pipeline.
///
/// Enable with `features = ["lut"]`.
///
/// See the [`oximedia_lut`] crate for the full API surface.
#[cfg(feature = "lut")]
pub mod lut {
    //! 1D and 3D LUT processing with HDR pipeline support.
    //!
    //! Parse and apply `.cube`, `.3dl`, and other LUT formats.  Supports linear,
    //! trilinear, and tetrahedral interpolation together with a full HDR tone-
    //! mapping pipeline.
    pub use oximedia_lut::*;
}

/// Color management: ICC profiles, ACES workflow, HDR, gamut mapping.
///
/// Enable with `features = ["colormgmt"]`.
///
/// See the [`oximedia_colormgmt`] crate for the full API surface.
#[cfg(feature = "colormgmt")]
pub mod colormgmt {
    //! Professional color management system.
    //!
    //! Standard color spaces (sRGB, Rec.709, Rec.2020, DCI-P3, ACES), ICC profile
    //! parsing/application, HDR transfer functions (PQ, HLG), gamut mapping, and
    //! full ACES IDT/RRT/ODT workflow.
    pub use oximedia_colormgmt::*;
}

/// Transcoding pipeline: parallel encoding, ABR ladders, multi-pass, audio normalization.
///
/// Enable with `features = ["transcode"]`.
///
/// See the [`oximedia_transcode`] crate for the full API surface.
#[cfg(feature = "transcode")]
pub mod transcode {
    //! Full-featured transcoding pipeline.
    //!
    //! Builder-pattern transcoder configuration, parallel encoding, ABR ladder
    //! generation, multi-pass encoding, integrated audio normalization, hardware
    //! acceleration, and job queuing.
    pub use oximedia_transcode::*;
}

/// Subtitle rendering: SRT, ASS/SSA, WebVTT with font rendering and animation.
///
/// Enable with `features = ["subtitle"]`.
///
/// See the [`oximedia_subtitle`] crate for the full API surface.
#[cfg(feature = "subtitle")]
pub mod subtitle {
    //! Subtitle rendering and format support.
    //!
    //! Parse and render SRT, ASS/SSA, and WebVTT subtitles with full styling,
    //! animation, positioning, and BiDi text layout.
    pub use oximedia_subtitle::*;
}

/// Closed captions: SRT, WebVTT, SCC, TTML, EBU-STL, and many more formats.
///
/// Enable with `features = ["captions"]`.
///
/// See the [`oximedia_captions`] crate for the full API surface.
#[cfg(feature = "captions")]
pub mod captions {
    //! Closed caption format support.
    //!
    //! Parse, convert, and write closed captions in SRT, WebVTT, SCC, TTML/DFXP,
    //! EBU-STL, iTunes Timed Text, and other broadcast caption formats.
    pub use oximedia_captions::*;
}

/// Archive verification: checksums, fixity checks, OAIS-compliant preservation.
///
/// Enable with `features = ["archive"]`.
///
/// See the [`oximedia_archive`] crate for the full API surface.
#[cfg(feature = "archive")]
pub mod archive {
    //! Media archive verification and long-term preservation.
    //!
    //! BLAKE3/SHA-256/CRC32 checksumming, scheduled fixity checking, PREMIS event
    //! logging, BagIt support, quarantine management, and OAIS-compliant digital
    //! preservation workflows.
    pub use oximedia_archive::*;
}

/// Deduplication: exact-hash, perceptual, SSIM, audio fingerprint, metadata matching.
///
/// Enable with `features = ["dedup"]`.
///
/// See the [`oximedia_dedup`] crate for the full API surface.
#[cfg(feature = "dedup")]
pub mod dedup {
    //! Media deduplication and duplicate detection.
    //!
    //! Multi-strategy detection combining cryptographic hashing, perceptual hashing,
    //! SSIM comparison, audio fingerprinting, and fuzzy metadata matching.
    pub use oximedia_dedup::*;
}

/// Media search: full-text, visual similarity, audio fingerprint, faceted, color, OCR.
///
/// Enable with `features = ["search"]`.
///
/// See the [`oximedia_search`] crate for the full API surface.
#[cfg(feature = "search")]
pub mod search {
    //! Advanced media search and indexing engine.
    //!
    //! Full-text search with fuzzy matching, visual similarity, audio fingerprinting,
    //! faceted search, boolean/range queries, face search, OCR text search, and
    //! color-based search.
    pub use oximedia_search::*;
}

/// Media Asset Management: asset lifecycle, collections, ingest, workflows, RBAC.
///
/// Enable with `features = ["mam"]`.
///
/// See the [`oximedia_mam`] crate for the full API surface.
#[cfg(feature = "mam")]
pub mod mam {
    //! Comprehensive Media Asset Management system.
    //!
    //! Asset ingestion with metadata extraction, collection management, workflow
    //! engine, proxy/thumbnail generation, RBAC permissions, cloud storage
    //! integration (S3, Azure, GCS), webhooks, and audit logging.
    pub use oximedia_mam::*;
}

/// Scene understanding: classification, object/face detection, composition analysis.
///
/// Enable with `features = ["scene"]`.
///
/// See the [`oximedia_scene`] crate for the full API surface.
#[cfg(feature = "scene")]
pub mod scene {
    //! AI-powered scene understanding and video analysis.
    //!
    //! Patent-free algorithms for scene classification (indoor/outdoor, day/night),
    //! object detection (HOG), face detection (Haar cascades), activity recognition,
    //! semantic segmentation, saliency detection, and aesthetic scoring.
    pub use oximedia_scene::*;
}

/// Shot detection: hard cuts, dissolves, fades, shot type and camera movement.
///
/// Enable with `features = ["shots"]`.
///
/// See the [`oximedia_shots`] crate for the full API surface.
#[cfg(feature = "shots")]
pub mod shots {
    //! Shot detection, classification, and analysis.
    //!
    //! Detect hard cuts, dissolves, fades, and wipes; classify shot types (ECU through
    //! ELS); detect camera angles and movements (pan, tilt, zoom, dolly, handheld);
    //! analyse composition and export shot lists / EDL.
    pub use oximedia_shots::*;
}

/// Broadcast video scopes: waveform, vectorscope, histogram, parade, false color.
///
/// Enable with `features = ["scopes"]`.
///
/// See the [`oximedia_scopes`] crate for the full API surface.
#[cfg(feature = "scopes")]
pub mod scopes {
    //! Professional broadcast video scopes.
    //!
    //! ITU-R BT.709/BT.2020-compliant waveform monitors, YUV vectorscopes, RGB/luma
    //! histograms, parade displays, false color, CIE diagram, focus assist, and HDR
    //! waveform with PQ/HLG/nits scale.
    pub use oximedia_scopes::*;
}

/// Visual effects and compositing: transitions, keying, particles, generators, stylization.
///
/// Enable with `features = ["vfx"]`.
///
/// See the [`oximedia_vfx`] crate for the full API surface.
#[cfg(feature = "vfx")]
pub mod vfx {
    //! Professional visual effects and compositing engine.
    //!
    //! Production-quality implementations of transitions (dissolve, wipe, push, 3D cube),
    //! chroma keying with spill suppression, particle systems (snow, rain, sparks), lens
    //! effects (flare, bloom, glow), time effects (remap, freeze, reverse), stylization
    //! (cartoon, sketch, oil paint, halftone), shape masks, and text animation.
    pub use oximedia_vfx::*;
}

/// Advanced image processing: DPX, OpenEXR, TIFF, ICC, DNG, XMP, pyramid, tone curves.
///
/// Enable with `features = ["image-ext"]`.
///
/// See the [`oximedia_image`] crate for the full API surface.
#[cfg(feature = "image-ext")]
pub mod image_ext {
    //! Professional image sequence I/O and processing for cinema and VFX workflows.
    //!
    //! High-performance reading and writing of DPX (SMPTE 268M), OpenEXR (HDR deep images),
    //! and TIFF (including BigTIFF).  Includes ICC profile embedding, DNG raw decoding, XMP
    //! metadata, image pyramids, tone curves, inpainting, lens correction, HDR merge,
    //! morphological operations, and sequence pattern matching.
    pub use oximedia_image::*;
}

/// Perceptual watermark embedding and forensic detection (DSSS, echo, phase, QIM).
///
/// Enable with `features = ["watermark"]`.
///
/// See the [`oximedia_watermark`] crate for the full API surface.
#[cfg(feature = "watermark")]
pub mod watermark {
    //! Professional audio watermarking and steganography.
    //!
    //! Multiple watermarking techniques: spread spectrum (DSSS), echo hiding, phase
    //! coding, LSB steganography, patchwork, and QIM.  Features psychoacoustic
    //! masking for imperceptibility, Reed-Solomon error correction, blind detection
    //! (no original required), and robustness testing against common attacks.
    pub use oximedia_watermark::*;
}

/// Music Information Retrieval: beat tracking, key detection, fingerprinting, MIR analysis.
///
/// Enable with `features = ["mir"]`.
///
/// See the [`oximedia_mir`] crate for the full API surface.
#[cfg(feature = "mir")]
pub mod mir {
    //! Music Information Retrieval (MIR) system.
    //!
    //! Comprehensive music analysis: BPM and beat tracking, musical key detection
    //! (Krumhansl-Schmuckler), chord recognition (chroma features), melody extraction,
    //! structural segmentation (intro/verse/chorus), genre classification, mood
    //! estimation (valence/arousal), spectral features, and audio fingerprinting.
    pub use oximedia_mir::*;
}

/// Content recommendation engine with collaborative filtering and personalization.
///
/// Enable with `features = ["recommend"]`.
///
/// See the [`oximedia_recommend`] crate for the full API surface.
#[cfg(feature = "recommend")]
pub mod recommend {
    //! Content recommendation and discovery engine.
    //!
    //! Unified recommendation system combining content-based filtering, collaborative
    //! filtering (matrix factorization), and hybrid approaches.  Includes user profile
    //! management, view-history tracking, explicit/implicit ratings, trending detection,
    //! diversity enforcement, freshness balancing, and recommendation explanations.
    pub use oximedia_recommend::*;
}

/// Broadcast playlist management, scheduling, SCTE-35, EPG, and automation.
///
/// Enable with `features = ["playlist"]`.
///
/// See the [`oximedia_playlist`] crate for the full API surface.
#[cfg(feature = "playlist")]
pub mod playlist {
    //! Broadcast playlist and scheduling system.
    //!
    //! Create and manage frame-accurate broadcast playlists with time-based scheduling,
    //! calendar recurrence, crossfade transitions, secondary events (logos, tickers),
    //! live feed integration, automatic failover/filler, SCTE-35 commercial breaks,
    //! XMLTV EPG generation, and as-run log metadata tracking.
    pub use oximedia_playlist::*;
}

/// Broadcast playout server with ad insertion, graphics overlays, and failover.
///
/// Enable with `features = ["playout"]`.
///
/// See the [`oximedia_playout`] crate for the full API surface.
#[cfg(feature = "playout")]
pub mod playout {
    //! Professional broadcast playout server.
    //!
    //! Frame-accurate, 24/7 reliable playout engine with genlock support, multiple
    //! simultaneous outputs (SDI, NDI, RTMP, SRT, IP multicast), CG graphics overlays
    //! (lower thirds, logos, tickers), SCTE-35 ad-break insertion, dynamic playlist
    //! management, comprehensive monitoring, and automatic emergency fallback.
    pub use oximedia_playout::*;
}

/// Digital rights management: licensing, territory restrictions, royalties, clearances.
///
/// Enable with `features = ["rights"]`.
///
/// See the [`oximedia_rights`] crate for the full API surface.
#[cfg(feature = "rights")]
pub mod rights {
    //! Content rights and licensing management.
    //!
    //! Comprehensive rights tracking covering ownership, license types (royalty-free,
    //! rights-managed), expiration and renewal, geographic territory restrictions,
    //! usage reporting, music/footage/talent clearances, royalty calculation, DRM
    //! metadata, watermark integration, and full audit trail.
    pub use oximedia_rights::*;
}

/// Collaborative media review and approval workflow with frame-accurate annotations.
///
/// Enable with `features = ["review"]`.
///
/// See the [`oximedia_review`] crate for the full API surface.
#[cfg(feature = "review")]
pub mod review {
    //! Collaborative media review and approval workflow.
    //!
    //! Frame-accurate commenting and annotation, real-time multi-reviewer collaboration,
    //! version comparison and tracking, multi-stage approval workflows, drawing tools
    //! for visual feedback, task assignment, email/webhook notifications, and export
    //! to PDF, CSV, and EDL.
    pub use oximedia_review::*;
}

/// Audio/video restoration: click/crackle removal, noise reduction, telecine, declipping.
///
/// Enable with `features = ["restore"]`.
///
/// See the [`oximedia_restore`] crate for the full API surface.
#[cfg(feature = "restore")]
pub mod restore {
    //! Professional audio/video restoration tools.
    //!
    //! Audio restoration: click/pop removal, hum removal (50/60 Hz + harmonics),
    //! spectral noise reduction, declipping, dehiss, decrackle, azimuth correction,
    //! wow/flutter removal, DC offset removal, and phase correction.  Video restoration:
    //! banding reduction, deflicker, dropout fix, film grain restoration, and telecine
    //! detection.  Pre-configured presets for vinyl, tape, broadcast, and archival.
    pub use oximedia_restore::*;
}

/// Media file repair and recovery: corruption detection, header repair, index rebuilding.
///
/// Enable with `features = ["repair"]`.
///
/// See the [`oximedia_repair`] crate for the full API surface.
#[cfg(feature = "repair")]
pub mod repair {
    //! Media file repair and recovery tools.
    //!
    //! Detect and repair corrupted media files: header reconstruction, seek-table
    //! rebuilding, timestamp validation and correction, packet recovery, audio/video
    //! sync fixes, truncation recovery, metadata repair, partial file recovery, frame
    //! reordering, and error concealment.  Safe, balanced, and aggressive repair modes.
    pub use oximedia_repair::*;
}

/// Multi-camera sync and angle switching with automatic camera selection.
///
/// Enable with `features = ["multicam"]`.
///
/// See the [`oximedia_multicam`] crate for the full API surface.
#[cfg(feature = "multicam")]
pub mod multicam {
    //! Multi-camera synchronization and switching.
    //!
    //! Temporal sync (frame-accurate, audio cross-correlation, LTC/VITC, visual clapper),
    //! multi-angle timeline editing with angle switching and transitions, AI-based automatic
    //! camera selection (speaker detection, action following), PiP/split-screen/grid
    //! composition, cross-camera color matching, spatial alignment, and panorama stitching.
    pub use oximedia_multicam::*;
}

/// Video stabilization: motion estimation, trajectory smoothing, rolling shutter correction.
///
/// Enable with `features = ["stabilize"]`.
///
/// See the [`oximedia_stabilize`] crate for the full API surface.
#[cfg(feature = "stabilize")]
pub mod stabilize {
    //! Professional video stabilization.
    //!
    //! Complete stabilization pipeline: feature tracking, camera motion estimation
    //! (translation, affine, perspective, 3D), trajectory building and smoothing
    //! (Gaussian, Kalman, adaptive), rolling shutter correction, 3D stabilization,
    //! horizon leveling, zoom optimization to minimize black borders, and optional
    //! synthetic motion blur.
    pub use oximedia_stabilize::*;
}

/// Cloud storage and processing abstraction: S3, Azure Blob, GCS, CDN, cost optimization.
///
/// Enable with `features = ["cloud"]`.
///
/// See the [`oximedia_cloud`] crate for the full API surface.
#[cfg(feature = "cloud")]
pub mod cloud {
    //! Cloud storage and media services integration.
    //!
    //! Unified multi-cloud abstraction over AWS S3, Azure Blob Storage, and Google Cloud
    //! Storage.  Includes media processing service integration, transfer management with
    //! retry and resume, bandwidth throttling, CDN configuration, cost estimation,
    //! encryption/credentials management, object lifecycle rules, and multi-region
    //! replication policies.
    pub use oximedia_cloud::*;
}

/// EDL (Edit Decision List) parsing and generation: CMX 3600, GVG, Sony BVE-9000.
///
/// Enable with `features = ["edl"]`.
///
/// See the [`oximedia_edl`] crate for the full API surface.
#[cfg(feature = "edl")]
pub mod edl {
    //! EDL (Edit Decision List) parsing and generation.
    //!
    //! Full CMX 3600, CMX 3400, GVG, and Sony BVE-9000 format support.  Handles all
    //! event types (cut, dissolve, wipe, key), drop/non-drop-frame timecode at all
    //! standard rates, motion effects (speed changes, freeze frames, reverse), audio
    //! channel routing, reel references, EDL validation, format conversion, and
    //! round-trip conformance checking.
    pub use oximedia_edl::*;
}

/// NDI (Network Device Interface) clean-room protocol implementation for IP video.
///
/// Enable with `features = ["ndi"]`.
///
/// See the [`oximedia_ndi`] crate for the full API surface.
#[cfg(feature = "ndi")]
pub mod ndi {
    //! NDI (Network Device Interface) protocol support.
    //!
    //! Clean-room implementation of the NDI protocol without the official SDK.
    //! Provides mDNS-based source discovery, low-latency video streaming (<1 frame),
    //! Full HD and 4K support, tally light signalling (program/preview), PTZ control,
    //! audio/video synchronization, and bandwidth adaptation.
    pub use oximedia_ndi::*;
}

/// IMF (Interoperable Master Format) package support per SMPTE ST 2067.
///
/// Enable with `features = ["imf"]`.
///
/// See the [`oximedia_imf`] crate for the full API surface.
#[cfg(feature = "imf")]
pub mod imf {
    //! IMF (Interoperable Master Format) package support.
    //!
    //! SMPTE ST 2067-compliant reading and writing of IMF packages: Composition
    //! Playlist (CPL), Packing List (PKL), ASSETMAP, Output Profile List (OPL), and
    //! MXF essence track files.  Includes timeline validation, hash verification,
    //! SMPTE conformance checking, HDR metadata, supplemental packages, and
    //! incremental versioning.
    pub use oximedia_imf::*;
}

/// AAF (Advanced Authoring Format) interchange per SMPTE ST 377-1.
///
/// Enable with `features = ["aaf"]`.
///
/// See the [`oximedia_aaf`] crate for the full API surface.
#[cfg(feature = "aaf")]
pub mod aaf {
    //! AAF (Advanced Authoring Format) interchange support.
    //!
    //! Full SMPTE ST 377-1 object model: Mobs (composition, master, source), Segments,
    //! Components, Effects, Dictionary, and Microsoft Structured Storage compound-file
    //! parsing.  Supports embedded and external essence references, edit rate management,
    //! timeline export, and conversion to OpenTimelineIO and EDL formats.
    pub use oximedia_aaf::*;
}

/// PTP/NTP time synchronization for broadcast production (IEEE 1588-2019, RFC 5905).
///
/// Enable with `features = ["timesync"]`.
///
/// See the [`oximedia_timesync`] crate for the full API surface.
#[cfg(feature = "timesync")]
pub mod timesync {
    //! Precision time synchronization for broadcast media production.
    //!
    //! IEEE 1588-2019 PTP with sub-microsecond accuracy (Ordinary/Boundary/Transparent
    //! Clock, BMCA), NTP v4 client with server pool management, SMPTE 12M/LTC/MTC
    //! timecode synchronization, clock discipline (PID controller, drift compensation,
    //! holdover), genlock reference generation, frame-accurate video sync, and
    //! sample-accurate audio sync.
    pub use oximedia_timesync::*;
}

/// Media forensics: tampering detection, ELA, noise analysis, copy-move, provenance.
///
/// Enable with `features = ["forensics"]`.
///
/// See the [`oximedia_forensics`] crate for the full API surface.
#[cfg(feature = "forensics")]
pub mod forensics {
    //! Media forensics analysis and tampering detection.
    //!
    //! Comprehensive forensic analysis: JPEG compression artifact analysis, Error Level
    //! Analysis (ELA), Photo Response Non-Uniformity (PRNU) noise patterns, metadata
    //! verification, copy-move detection, illumination inconsistency detection, splicing
    //! analysis, steganography detection, source camera identification, chain-of-custody
    //! tracking, and comprehensive forensic reporting with confidence scores.
    pub use oximedia_forensics::*;
}

/// Hardware acceleration: Vulkan GPU compute, CPU fallback, device management.
///
/// Enable with `features = ["accel"]`.
///
/// See the [`oximedia_accel`] crate for the full API surface.
#[cfg(feature = "accel")]
pub mod accel {
    //! Hardware acceleration abstraction layer.
    //!
    //! Provides GPU-accelerated computation (Vulkan) for scaling, color conversion,
    //! and motion estimation, with automatic CPU fallback on systems without a GPU.
    pub use oximedia_accel::*;
}

/// SIMD-optimised kernels: DCT, SAD, interpolation, blending, color conversion.
///
/// Enable with `features = ["simd"]`.
///
/// See the [`oximedia_simd`] crate for the full API surface.
#[cfg(feature = "simd")]
pub mod simd {
    //! Hand-tuned SIMD media kernels.
    //!
    //! Runtime-detected AVX2 / AVX-512 / NEON paths for DCT, SAD (motion estimation),
    //! bilinear/bicubic/8-tap interpolation, pixel blending, and YUV operations.
    pub use oximedia_simd::*;
}

/// Professional live video switcher: M/E rows, keyers, DVE, tally, macros.
///
/// Enable with `features = ["switcher"]`.
///
/// See the [`oximedia_switcher`] crate for the full API surface.
#[cfg(feature = "switcher")]
pub mod switcher {
    //! Professional live production video switcher.
    //!
    //! Full program/preview bus architecture, upstream and downstream keyers
    //! (luma, chroma, linear, pattern), DVE transitions, multi-viewer monitoring,
    //! tally signalling, macro recording, media pool, and audio follow video.
    pub use oximedia_switcher::*;
}

/// Multi-track timeline editor: frame-accurate editing, keyframes, EDL/XML/AAF.
///
/// Enable with `features = ["timeline"]`.
///
/// See the [`oximedia_timeline`] crate for the full API surface.
#[cfg(feature = "timeline")]
pub mod timeline {
    //! Professional multi-track timeline editor.
    //!
    //! Frame-accurate video/audio editing, slip/slide/roll/ripple operations,
    //! transition effects with keyframe animation, multi-camera sequences, nested
    //! sequences, markers, and EDL/XML/AAF import/export.
    pub use oximedia_timeline::*;
}

/// Codec optimisation suite: RDO, psychovisual, adaptive quantization, motion search.
///
/// Enable with `features = ["optimize"]`.
///
/// See the [`oximedia_optimize`] crate for the full API surface.
#[cfg(feature = "optimize")]
pub mod optimize {
    //! Advanced codec optimisation and tuning.
    //!
    //! Rate-distortion optimisation (RDO), psychovisual masking, advanced motion
    //! search (TZSearch, EPZS, UMH), intra prediction, adaptive quantization,
    //! loop filter tuning, and entropy coding context optimisation.
    pub use oximedia_optimize::*;
}

/// Performance profiling: CPU, GPU, memory, frame timing, flame graphs, bottlenecks.
///
/// Enable with `features = ["profiler"]`.
///
/// See the [`oximedia_profiler`] crate for the full API surface.
#[cfg(feature = "profiler")]
pub mod profiler {
    //! Comprehensive performance profiling tools.
    //!
    //! Sampling and instrumentation-based CPU profiling, GPU timeline analysis,
    //! memory allocation tracking, frame budget analysis, cache miss profiling,
    //! thread contention detection, flame graph generation, and regression detection.
    pub use oximedia_profiler::*;
}

/// Distributed render farm coordinator: job management, worker pools, cloud bursting.
///
/// Enable with `features = ["renderfarm"]`.
///
/// See the [`oximedia_renderfarm`] crate for the full API surface.
#[cfg(feature = "renderfarm")]
pub mod renderfarm {
    //! Enterprise-grade render farm coordinator.
    //!
    //! Submit, schedule, and track render jobs across distributed worker pools with
    //! priority-based scheduling, dependency graphs, health monitoring, cost tracking,
    //! cloud bursting, fault tolerance, and automated retry/checkpointing.
    pub use oximedia_renderfarm::*;
}

/// Cloud-agnostic object storage: S3, Azure Blob, GCS, local, caching, lifecycle.
///
/// Enable with `features = ["storage"]`.
///
/// See the [`oximedia_storage`] crate for the full API surface.
#[cfg(feature = "storage")]
pub mod storage {
    //! Unified cloud and local object storage abstraction.
    //!
    //! Streaming uploads/downloads, multipart transfers, progress tracking, resume
    //! capability, LRU/LFU/ARC cache layer, content-addressable deduplication,
    //! compression, integrity verification, namespace management, and lifecycle policies.
    pub use oximedia_storage::*;
}

/// Real-time CRDT-based collaborative editing for multi-user video production.
///
/// Enable with `features = ["collab"]`.
///
/// See the [`oximedia_collab`] crate for the full API surface.
#[cfg(feature = "collab")]
pub mod collab {
    //! Real-time multi-user collaborative video editing.
    //!
    //! CRDT-based synchronisation supporting 10+ concurrent editors with sub-second
    //! latency.  Features session management, conflict resolution, annotation,
    //! approval workflows, asset locking, audit trail, and team role management.
    pub use oximedia_collab::*;
}

/// Game streaming and screen capture: ultra-low latency, overlays, replay buffer.
///
/// Enable with `features = ["gaming"]`.
///
/// See the [`oximedia_gaming`] crate for the full API surface.
#[cfg(feature = "gaming")]
pub mod gaming {
    //! Game streaming and screen capture optimisation.
    //!
    //! <100 ms glass-to-glass streaming, monitor/window/region capture, NVENC/QSV/VCE
    //! hardware encoding, overlay system (alerts, widgets, scoreboards), scene switching,
    //! replay buffer, highlight detection, and platform metadata for Twitch/YouTube/Facebook.
    pub use oximedia_gaming::*;
}

/// Virtual production and LED wall tools: camera tracking, in-camera VFX, genlock.
///
/// Enable with `features = ["virtual-prod"]`.
///
/// See the [`oximedia_virtual`] crate for the full API surface.
#[cfg(feature = "virtual-prod")]
pub mod virtual_prod {
    //! Virtual production and LED wall tooling.
    //!
    //! Camera tracking and calibration, LED wall rendering with perspective correction,
    //! in-camera VFX compositing, colour pipeline management, genlock synchronisation,
    //! motion capture integration, real-time keying, and Unreal Engine integration.
    pub use oximedia_virtual::*;
}

/// Accessibility: audio description, captions, sign language, compliance (WCAG, EBU).
///
/// Enable with `features = ["access"]`.
///
/// See the [`oximedia_access`] crate for the full API surface.
#[cfg(feature = "access")]
pub mod access {
    //! Inclusive media accessibility tools.
    //!
    //! Audio description generation and mixing, closed caption styling, sign language
    //! video overlays, transcript generation, multi-language translation, text-to-speech,
    //! speech-to-text, visual/audio enhancement, and WCAG/Section 508/EBU compliance.
    pub use oximedia_access::*;
}

/// Media conforming: EDL/XML/AAF timeline-to-media matching and reconstruction.
///
/// Enable with `features = ["conform"]`.
///
/// See the [`oximedia_conform`] crate for the full API surface.
#[cfg(feature = "conform")]
pub mod conform {
    //! Professional media conforming system.
    //!
    //! Import CMX 3600 EDLs, Final Cut Pro / Premiere / DaVinci XML, and Avid AAF
    //! timelines; match media by filename, timecode, content hash, or duration;
    //! reconstruct multi-track timelines; and export to MP4, Matroska, EDL, or XML.
    pub use oximedia_conform::*;
}

/// Media format conversion: batch transcoding, format detection, metadata preservation.
///
/// Enable with `features = ["convert"]`.
///
/// See the [`oximedia_convert`] crate for the full API surface.
#[cfg(feature = "convert")]
pub mod convert {
    //! Comprehensive media format conversion utilities.
    //!
    //! Batch conversion with templates, codec and format detection, 200+ conversion
    //! profiles (web, mobile, archive), quality preservation, metadata round-tripping,
    //! subtitle/audio/video extraction, frame extraction, and file concatenation/splitting.
    pub use oximedia_convert::*;
}

/// Broadcast automation: master control, 24/7 playout, device control, failover, EAS.
///
/// Enable with `features = ["automation"]`.
///
/// See the [`oximedia_automation`] crate for the full API surface.
#[cfg(feature = "automation")]
pub mod automation {
    //! 24/7 broadcast automation and master control system.
    //!
    //! Multi-channel automated playout, VDCP/Sony 9-pin/GPI-GPO device control,
    //! frame-accurate playlist execution, live switching, automatic redundancy and
    //! hot-standby failover, Emergency Alert System (EAS) insertion, and Lua scripting.
    pub use oximedia_automation::*;
}

/// Professional clip management: logging, subclips, bins, smart collections, export.
///
/// Enable with `features = ["clips"]`.
///
/// See the [`oximedia_clips`] crate for the full API surface.
#[cfg(feature = "clips")]
pub mod clips {
    //! Professional clip management and logging system.
    //!
    //! Clip database with full metadata, subclip in/out point management, bin/folder
    //! organisation, keyword logging, ratings, markers, take management, proxy
    //! association, smart auto-collections, advanced search, and EDL/XML/CSV/JSON export.
    pub use oximedia_clips::*;
}

/// Proxy and offline editing workflows: generation, linking, conforming, relink.
///
/// Enable with `features = ["proxy"]`.
///
/// See the [`oximedia_proxy`] crate for the full API surface.
#[cfg(feature = "proxy")]
pub mod proxy {
    //! Proxy and offline editing workflow system.
    //!
    //! Quarter/half/full-resolution proxy generation, persistent proxy–original link
    //! database, EDL/XML conform with frame-accurate relink, smart cache management,
    //! metadata synchronisation, and validation reporting.
    pub use oximedia_proxy::*;
}

/// Encoding preset library: 200+ platform, broadcast, streaming, and archive presets.
///
/// Enable with `features = ["presets"]`.
///
/// See the [`oximedia_presets`] crate for the full API surface.
#[cfg(feature = "presets")]
pub mod presets {
    //! Advanced encoding preset library.
    //!
    //! 200+ professional presets: YouTube, Vimeo, Facebook, Instagram, TikTok, Twitter,
    //! ATSC/DVB/ISDB broadcast, HLS/DASH/SmoothStreaming ABR ladders, iOS/Android
    //! mobile, lossless archive, mezzanine, with auto-selection and JSON import/export.
    pub use oximedia_presets::*;
}

/// Color calibration: ColorChecker profiling, display calibration, ICC, LUT generation.
///
/// Enable with `features = ["calibrate"]`.
///
/// See the [`oximedia_calibrate`] crate for the full API surface.
#[cfg(feature = "calibrate")]
pub mod calibrate {
    //! Professional color calibration and matching tools.
    //!
    //! Automatic ColorChecker detection and camera profiling, display gamma calibration,
    //! cross-camera color matching, ICC profile generation and application, calibration
    //! LUT generation, white balance, chromatic adaptation, and gamut mapping.
    pub use oximedia_calibrate::*;
}

/// Spatial and temporal video denoising: bilateral, NLM, Wiener, wavelet, Kalman.
///
/// Enable with `features = ["denoise"]`.
///
/// See the [`oximedia_denoise`] crate for the full API surface.
#[cfg(feature = "denoise")]
pub mod denoise {
    //! Professional video denoising.
    //!
    //! Spatial denoising (bilateral, non-local means, Wiener, wavelet), temporal
    //! denoising (weighted averaging, median, motion-compensated, Kalman), hybrid
    //! spatio-temporal, film grain preservation, and automatic noise level estimation.
    pub use oximedia_denoise::*;
}

/// Video alignment and registration: temporal sync, spatial homography, feature matching.
///
/// Enable with `features = ["align"]`.
///
/// See the [`oximedia_align`] crate for the full API surface.
#[cfg(feature = "align")]
pub mod align {
    //! Multi-camera video alignment and registration.
    //!
    //! Audio cross-correlation sync, LTC/VITC timecode alignment, visual clapper
    //! detection, homography estimation, FAST/BRIEF/ORB feature matching, RANSAC
    //! robust estimation, lens distortion correction, and color matching across cameras.
    pub use oximedia_align::*;
}

/// Comprehensive media analysis: scene detection, quality assessment, content classification.
///
/// Enable with `features = ["analysis"]`.
///
/// See the [`oximedia_analysis`] crate for the full API surface.
#[cfg(feature = "analysis")]
pub mod analysis {
    //! Comprehensive media analysis and quality assessment.
    //!
    //! Scene/shot detection (cuts, fades, dissolves), black frame detection, quality
    //! scoring (blockiness, blur, noise), content classification, thumbnail generation,
    //! motion/color/temporal analysis, silence/clipping detection, and JSON/HTML reports.
    pub use oximedia_analysis::*;
}

/// Professional audio post-production: ADR, Foley, mixing console, stems, delivery.
///
/// Enable with `features = ["audiopost"]`.
///
/// See the [`oximedia_audiopost`] crate for the full API surface.
#[cfg(feature = "audiopost")]
pub mod audiopost {
    //! Professional audio post-production suite.
    //!
    //! ADR session management and synchronisation, Foley recording and library,
    //! sound design synthesisers and spatial audio, professional mixing console with
    //! channel strips and aux sends, stem management, loudness compliance, and delivery.
    pub use oximedia_audiopost::*;
}

/// Broadcast-grade quality control: video, audio, container, and compliance validation.
///
/// Enable with `features = ["qc"]`.
///
/// See the [`oximedia_qc`] crate for the full API surface.
#[cfg(feature = "qc")]
pub mod qc {
    //! Quality control and delivery validation.
    //!
    //! Video checks (codec, resolution, frame rate, artifacts, black/freeze frames),
    //! audio checks (loudness EBU R128/ATSC A/85, clipping, silence, phase, DC offset),
    //! container checks (sync, timestamps, keyframes), and streaming platform compliance.
    pub use oximedia_qc::*;
}

/// Job queue and worker management for scalable media transcoding pipelines.
///
/// Enable with `features = ["jobs"]`.
///
/// See the [`oximedia_jobs`] crate for the full API surface.
#[cfg(feature = "jobs")]
pub mod jobs {
    //! Job queue and worker management for video transcoding.
    //!
    //! Priority queue, flexible scheduling, dependency graphs, automatic retry with
    //! exponential backoff, job cancellation, SQLite persistence, configurable worker
    //! pool, auto-scaling, job pipelines, deadline scheduling, and metrics tracking.
    pub use oximedia_jobs::*;
}

/// Automated video editing: highlight detection, smart cutting, auto-assembly, rules engine.
///
/// Enable with `features = ["auto"]`.
///
/// See the [`oximedia_auto`] crate for the full API surface.
#[cfg(feature = "auto")]
pub mod auto {
    //! Automated video editing system.
    //!
    //! AI-powered highlight detection (motion, faces, audio peaks), intelligent shot
    //! boundary cutting, beat-synced music editing, auto-assembly for highlight reels
    //! and trailers, configurable rules engine, scene scoring, and interest curve generation.
    pub use oximedia_auto::*;
}

/// Video timeline editor: multi-track, effects, transitions, keyframes, rendering.
///
/// Enable with `features = ["edit"]`.
///
/// See the [`oximedia_edit`] crate for the full API surface.
#[cfg(feature = "edit")]
pub mod edit {
    //! Video timeline editor.
    //!
    //! Multi-track video/audio/subtitle timeline, clip add/remove/move/trim/split,
    //! ripple/roll/slip/slide edits, effects system with keyframe animation,
    //! cross-fade/dissolve/wipe transitions, and real-time preview with high-quality export.
    pub use oximedia_edit::*;
}

/// Signal routing, NMOS IS-04/IS-05/IS-07, crosspoint matrix, virtual patch bay.
///
/// Enable with `features = ["routing"]`.
///
/// See the [`oximedia_routing`] crate for the full API surface.
#[cfg(feature = "routing")]
pub mod routing {
    //! Professional audio/video signal routing and patching.
    //!
    //! Full any-to-any crosspoint matrix, virtual patch bay, complex channel mapping,
    //! SDI audio embed/de-embed, format conversion, per-channel gain with metering,
    //! AFL/PFL monitoring, MADI 64-channel routing, and NMOS IS-04/IS-05/IS-07 support.
    pub use oximedia_routing::*;
}

/// Advanced audio analysis: spectral, voice, music, source separation, forensics, pitch.
///
/// Enable with `features = ["audio-analysis"]`.
///
/// See the [`oximedia_audio_analysis`] crate for the full API surface.
#[cfg(feature = "audio-analysis")]
pub mod audio_analysis {
    //! Comprehensive audio analysis and forensics.
    //!
    //! Spectral analysis (centroid, flatness, crest), voice analysis (F0, formants, gender,
    //! emotion, speaker ID), music analysis (chords, rhythm, timbre, instrument ID), source
    //! separation (vocal/drums/bass), echo/reverb/distortion measurement, transient detection,
    //! pitch tracking (YIN), formant tracking, audio forensics (ENF, edit detection), and
    //! noise profiling.
    pub use oximedia_audio_analysis::*;
}

/// GPU compute pipeline: WGPU-based acceleration (Vulkan, Metal, DX12, WebGPU).
///
/// Enable with `features = ["gpu"]`.
///
/// See the [`oximedia_gpu`] crate for the full API surface.
#[cfg(feature = "gpu")]
pub mod gpu {
    //! Cross-platform GPU acceleration layer.
    //!
    //! Color space conversions (RGB↔YUV BT.601/709/2020), image scaling (nearest/bilinear/
    //! bicubic), convolution filters, DCT/FFT transforms, multi-GPU support, automatic CPU
    //! fallback, memory management, pipeline caching, and profiling.
    pub use oximedia_gpu::*;
}

/// HLS/DASH adaptive streaming packager: manifests, segments, bitrate ladders, encryption.
///
/// Enable with `features = ["packager"]`.
///
/// See the [`oximedia_packager`] crate for the full API surface.
#[cfg(feature = "packager")]
pub mod packager {
    //! Adaptive streaming packaging for HLS and DASH.
    //!
    //! Automatic bitrate ladder generation, multi-variant streams, TS/fMP4/CMAF segments,
    //! M3U8/MPD manifest generation, AES-128/SAMPLE-AES/CENC encryption, keyframe alignment,
    //! fast-start optimisation, S3/cloud upload integration, and live/VOD modes.
    pub use oximedia_packager::*;
}

/// Content protection: CENC, Widevine, PlayReady, FairPlay, Clear Key DRM.
///
/// Enable with `features = ["drm"]`.
///
/// See the [`oximedia_drm`] crate for the full API surface.
#[cfg(feature = "drm")]
pub mod drm {
    //! Digital Rights Management and encryption.
    //!
    //! CENC common encryption, Widevine/PlayReady/FairPlay/Clear Key DRM systems, key
    //! management and rotation, license server infrastructure, geo-fencing, output control,
    //! offline playback, entitlement management, and audit trails.
    pub use oximedia_drm::*;
}

/// Professional digital preservation: BagIt, OAIS, PREMIS, METS, fixity, migration.
///
/// Enable with `features = ["archive-pro"]`.
///
/// See the [`oximedia_archive_pro`] crate for the full API surface.
#[cfg(feature = "archive-pro")]
pub mod archive_pro {
    //! Advanced digital preservation system.
    //!
    //! BagIt and OAIS (SIP/AIP/DIP) packaging, multi-algorithm checksums (MD5, SHA-256,
    //! SHA-512, BLAKE3), PREMIS/METS/Dublin Core metadata, version control, periodic fixity
    //! checking, format obsolescence risk assessment, emulation planning, and cold storage.
    pub use oximedia_archive_pro::*;
}

/// Distributed encoding: multi-node coordinator, load balancing, segment/tile/GOP splitting.
///
/// Enable with `features = ["distributed"]`.
///
/// See the [`oximedia_distributed`] crate for the full API surface.
#[cfg(feature = "distributed")]
pub mod distributed {
    //! Distributed video encoding system.
    //!
    //! Central job coordinator, worker nodes, segment/tile/GOP-based splitting strategies,
    //! Raft-based consensus, leader election, work-stealing scheduler, circuit breaker,
    //! backpressure, message bus, metrics aggregation, and fault-tolerant task retry.
    pub use oximedia_distributed::*;
}

/// Render farm coordinator: gRPC workers, priority scheduling, SQLite persistence.
///
/// Enable with `features = ["farm"]`.
///
/// See the [`oximedia_farm`] crate for the full API surface.
#[cfg(feature = "farm")]
pub mod farm {
    //! Distributed render farm coordinator.
    //!
    //! Priority job queue, worker registry with capability tracking, intelligent load
    //! balancing, automatic retry, real-time progress aggregation, resource-aware
    //! scheduling (CPU/GPU/memory), gRPC + TLS communication, and Prometheus metrics.
    pub use oximedia_farm::*;
}

/// Dolby Vision RPU metadata: parser and writer for Profiles 5, 7, 8, 8.1, 8.4.
///
/// Enable with `features = ["dolbyvision"]`.
///
/// See the [`oximedia_dolbyvision`] crate for the full API surface.
#[cfg(feature = "dolbyvision")]
pub mod dolbyvision {
    //! Dolby Vision RPU (Reference Processing Unit) metadata support.
    //!
    //! Metadata-only implementation respecting Dolby's IP. Parses and generates RPU
    //! structures for Profiles 5/7/8/8.1/8.4, level metadata (L1–L11), tone-mapping
    //! curves, shot boundary detection, trim passes, and XML metadata interchange.
    pub use oximedia_dolbyvision::*;
}

/// Professional digital audio mixer: 100+ channels, automation, effects, bus architecture.
///
/// Enable with `features = ["mixer"]`.
///
/// See the [`oximedia_mixer`] crate for the full API surface.
#[cfg(feature = "mixer")]
pub mod mixer {
    //! Full-featured digital audio mixing console.
    //!
    //! 100+ channels (mono/stereo/5.1/7.1/ambisonics), full parameter automation (read/
    //! write/touch/latch/trim), dynamics/EQ/reverb/modulation/distortion effects, master/
    //! group/auxiliary/matrix buses, professional metering (Peak/RMS/VU/LUFS), and
    //! session save/load with undo/redo.
    pub use oximedia_mixer::*;
}

/// High-quality video scaling: bilinear, bicubic, Lanczos, super-resolution, content-aware.
///
/// Enable with `features = ["scaling"]`.
///
/// See the [`oximedia_scaling`] crate for the full API surface.
#[cfg(feature = "scaling")]
pub mod scaling {
    //! Professional video scaling operations.
    //!
    //! Bilinear, bicubic, and Lanczos interpolation, aspect ratio preservation, batch
    //! scaling, chroma-aware resampling, content-aware scaling, super-resolution upscaling,
    //! deinterlacing, resolution ladders, ROI scaling, and quality metrics.
    pub use oximedia_scaling::*;
}

/// Broadcast graphics engine: lower thirds, tickers, tally, keyframe animation, templates.
///
/// Enable with `features = ["graphics"]`.
///
/// See the [`oximedia_graphics`] crate for the full API surface.
#[cfg(feature = "graphics")]
pub mod graphics {
    //! Broadcast graphics and overlay engine.
    //!
    //! 2D vector rendering, advanced text layout and typography, broadcast elements
    //! (lower thirds, tickers, bugs, scoreboards, clocks, countdowns), keyframe
    //! animation system, template-based graphics, real-time video overlay, and
    //! particle effects.
    pub use oximedia_graphics::*;
}

/// Patent-free video-over-IP: VP9/AV1 transport, mDNS discovery, FEC, tally, PTZ.
///
/// Enable with `features = ["videoip"]`.
///
/// See the [`oximedia_videoip`] crate for the full API surface.
#[cfg(feature = "videoip")]
pub mod videoip {
    //! Professional video-over-IP protocol (patent-free NDI alternative).
    //!
    //! Low-latency UDP transport with FEC, mDNS/DNS-SD service discovery, VP9/AV1
    //! (compressed) and v210/UYVY (uncompressed), Opus/PCM audio, tally lights,
    //! PTZ control, timecode, metadata, jitter buffering, and multi-stream support.
    pub use oximedia_videoip::*;
}

/// FFmpeg CLI compatibility: parse FFmpeg arguments and translate to OxiMedia operations.
///
/// Enable with `features = ["compat-ffmpeg"]`.
///
/// See the [`oximedia_compat_ffmpeg`] crate for the full API surface.
#[cfg(feature = "compat-ffmpeg")]
pub mod compat_ffmpeg {
    //! FFmpeg CLI argument compatibility layer.
    //!
    //! Parses FFmpeg-style command-line arguments (`-i`, `-c:v`, `-crf`, `-vf`, etc.)
    //! and translates them to OxiMedia `TranscodeConfig` and `FilterGraph` operations.
    //! Includes 80+ codec mappings and diagnostic reporting in FFmpeg log format.
    pub use oximedia_compat_ffmpeg::*;
}

/// Dynamic and static codec plugin system for extending OxiMedia with external codecs.
///
/// Enable with `features = ["plugin"]`.
///
/// See the [`oximedia_plugin`] crate for the full API surface.
#[cfg(feature = "plugin")]
pub mod plugin {
    //! Codec plugin system.
    //!
    //! Enables dynamic loading of external codec implementations from shared libraries
    //! (.so/.dylib/.dll) and static plugin registration. Plugins implement `CodecPlugin`,
    //! declare capabilities, and are managed by `PluginRegistry`. Supports the
    //! `declare_plugin!` macro for shared library entry points.
    pub use oximedia_plugin::*;
}

/// RESTful media server: JWT auth, HLS/DASH streaming, WebSocket progress, media library.
///
/// Enable with `features = ["server"]`.
///
/// See the [`oximedia_server`] crate for the full API surface.
#[cfg(feature = "server")]
pub mod server {
    //! Production-ready RESTful media server.
    //!
    //! File upload, transcoding, thumbnail generation, full-text search media library
    //! (SQLite), HLS/DASH adaptive bitrate streaming, progressive download with range
    //! requests, JWT + API-key authentication, role-based access control, WebSocket
    //! real-time job updates, multi-part upload, and rate limiting.
    pub use oximedia_server::*;
}

/// HDR video processing: PQ/HLG transfer functions, tone mapping, HDR10/HDR10+ metadata.
///
/// Enable with `features = ["hdr"]`.
///
/// See the [`oximedia_hdr`] crate for the full API surface.
#[cfg(feature = "hdr")]
pub mod hdr {
    //! HDR video processing: PQ/HLG transfer functions, tone mapping, HDR10+ metadata.
    //!
    //! SMPTE ST 2084 (PQ) and HLG EOTF/OETF, Rec. 2020/2100 gamut conversion matrices,
    //! HDR10 and HDR10+ dynamic metadata (SMPTE ST 2094-40), tone-mapping operators
    //! (Reinhard, ACES, Hable/Uncharted2, BT.2390, luminance-range clamp), and a
    //! full HDR-to-SDR conversion pipeline.
    pub use oximedia_hdr::*;
}

/// Spatial audio processing: Higher-Order Ambisonics, binaural HRTF rendering, room simulation.
///
/// Enable with `features = ["spatial"]`.
///
/// See the [`oximedia_spatial`] crate for the full API surface.
#[cfg(feature = "spatial")]
pub mod spatial {
    //! Spatial audio processing: Ambisonics, binaural HRTF, room acoustics.
    //!
    //! Higher-Order Ambisonics (HOA) up to 3rd order encoding/decoding, HRTF-based
    //! binaural rendering for headphone spatialization, and image-source room acoustics
    //! simulation with early reflections and late reverberation modelling.
    pub use oximedia_spatial::*;
}

/// High-performance caching infrastructure: LRU, tiered cache, predictive warming.
///
/// Enable with `features = ["cache"]`.
///
/// See the [`oximedia_cache`] crate for the full API surface.
#[cfg(feature = "cache")]
pub mod cache {
    //! High-performance media caching primitives.
    //!
    //! Arena-backed O(1) LRU cache with hit/miss/eviction statistics; multi-tier
    //! cache (L1 → L2 → disk-sim) with pluggable eviction policies (LRU, LFU, FIFO,
    //! Random, TinyLFU) and automatic tier promotion; predictive cache warming via
    //! access-pattern analysis, EMA inter-arrival tracking, and auto-correlation
    //! periodicity detection.
    pub use oximedia_cache::*;
}

/// Adaptive streaming pipeline: quality ladders, ABR switching, segment management, QoE health.
///
/// Enable with `features = ["stream"]`.
///
/// See the [`oximedia_stream`] crate for the full API surface.
#[cfg(feature = "stream")]
pub mod stream {
    //! Adaptive streaming pipeline, segment lifecycle management, and stream health monitoring.
    //!
    //! Quality ladder management with BOLA-inspired ABR switching, segment state-machine
    //! (pending/loading/ready/evicted) with prefetch and eviction, QoE scoring, issue
    //! detection (stalls, rebuffering, quality drops), and streaming health history.
    pub use oximedia_stream::*;
}

/// Video processing algorithms: scene detection, pulldown detection, temporal denoising, and perceptual fingerprinting.
///
/// Enable with `features = ["video-proc"]`.
///
/// See the [`oximedia_video`] crate for the full API surface.
#[cfg(feature = "video-proc")]
pub mod video_proc {
    //! Video processing: scene detection, 3:2 pulldown, temporal denoising, perceptual fingerprinting.
    pub use oximedia_video::*;
}

/// CDN edge management, cache invalidation, geographic routing, and origin failover.
///
/// Enable with `features = ["cdn"]`.
///
/// See the [`oximedia_cdn`] crate for the full API surface.
#[cfg(feature = "cdn")]
pub mod cdn {
    //! CDN edge management, cache invalidation, geo-routing, and origin failover.
    pub use oximedia_cdn::*;
}

/// Lightweight neural network inference for media: tensor ops, conv2d, scene classification.
///
/// Enable with `features = ["neural"]`.
///
/// See the [`oximedia_neural`] crate for the full API surface.
#[cfg(feature = "neural")]
pub mod neural {
    //! Lightweight neural network inference for media processing.
    //! Tensor operations, conv2d, batch norm, activations, and pre-defined media models.
    pub use oximedia_neural::*;
}

/// 360° VR video processing: equirectangular/cubemap projections, fisheye, stereo 3D.
///
/// Enable with `features = ["vr360"]`.
///
/// See the [`oximedia_360`] crate for the full API surface.
#[cfg(feature = "vr360")]
pub mod vr360 {
    //! 360° VR video: spherical projection conversions, fisheye, stereo 3D, and spatial metadata.
    pub use oximedia_360::*;
}

/// Media engagement analytics: session tracking, retention curves, A/B testing, scoring.
///
/// Enable with `features = ["analytics"]`.
///
/// See the [`oximedia_analytics`] crate for the full API surface.
#[cfg(feature = "analytics")]
pub mod analytics {
    //! Media analytics: viewer sessions, retention curves, A/B testing, engagement scoring.
    pub use oximedia_analytics::*;
}

/// Advanced caption generation: speech alignment, WCAG compliance, speaker diarization.
///
/// Enable with `features = ["caption-gen"]`.
///
/// See the [`oximedia_caption_gen`] crate for the full API surface.
#[cfg(feature = "caption-gen")]
pub mod caption_gen {
    //! Caption generation: speech alignment, Knuth-Plass line breaking, WCAG 2.1 compliance, diarization.
    pub use oximedia_caption_gen::*;
}

/// Image transformation: resize, crop, rotate, flip, color conversion.
///
/// Enable with `features = ["image-transform"]`.
///
/// See the [`oximedia_image_transform`] crate for the full API surface.
#[cfg(feature = "image-transform")]
pub mod image_transform {
    //! Image transformation operations: resize, crop, rotate, flip, and color conversion.
    //! Includes affine transforms, perspective correction, lens distortion, and color-space conversion.
    pub use oximedia_image_transform::*;
}

/// Declarative media processing pipeline DSL: typed filter graph, node composition, execution planning.
///
/// Enable with `features = ["pipeline"]`.
///
/// # Example
///
/// ```rust,no_run
/// # #[cfg(feature = "pipeline")]
/// # {
/// use oximedia::pipeline::{PipelineBuilder, SourceConfig, SinkConfig};
///
/// let graph = PipelineBuilder::new()
///     .source("in", SourceConfig::File("input.mp4".into()))
///     .scale(1280, 720)
///     .sink("out", SinkConfig::Null)
///     .build()
///     .expect("pipeline validation");
/// # }
/// ```
///
/// See the [`oximedia_pipeline`] crate for the full API surface.
#[cfg(feature = "pipeline")]
pub mod pipeline {
    //! Declarative media processing pipeline DSL.
    //!
    //! Build typed filter graphs from a fluent API or by composing nodes directly.
    //! The execution planner topologically sorts the graph and returns an optimised
    //! execution plan suitable for single-threaded or parallel execution.
    pub use oximedia_pipeline::*;
}

/// Sovereign ML pipelines: Pure-Rust ONNX inference (scene classification, shot boundary
/// detection, aesthetic scoring, object detection, face embedding).
///
/// Enable with `features = ["ml"]`. Combine with `ml-scene-classifier`, `ml-shot-boundary`,
/// `ml-aesthetic-score`, `ml-object-detector`, `ml-face-embedder`, and `ml-onnx` to select
/// individual pipelines and the ONNX runtime itself.
///
/// ## WebAssembly support
///
/// Every `ml-*` feature above compiles cleanly on `wasm32-unknown-unknown`,
/// including `ml-onnx` (the OxiONNX Pure-Rust runtime is WASM-friendly).
/// The only exception is `cuda`, which transitively depends on
/// `libloading` for NVIDIA driver binding and therefore is **native
/// only**. See [`oximedia_ml`] crate docs for the full feature /
/// target matrix.
#[cfg(feature = "ml")]
pub mod ml {
    //! Sovereign ML pipelines built on OxiONNX (Pure-Rust, patent-free).
    //!
    //! This module is a flat re-export of the [`oximedia_ml`] crate;
    //! consult that crate's documentation for the full type index,
    //! pipeline contracts, and doc-tests. The brief guide below is a
    //! pointer into the most common entry points.
    //!
    //! ## Entry points
    //!
    //! * [`DeviceType::auto`](oximedia_ml::DeviceType::auto) — pick the
    //!   strongest available backend (CUDA → DirectML → WebGPU → CPU),
    //!   memoised for the lifetime of the process.
    //! * [`ModelCache`](oximedia_ml::ModelCache) — thread-safe LRU
    //!   cache of `Arc<OnnxModel>` keyed by canonical model path.
    //! * [`TypedPipeline`](oximedia_ml::TypedPipeline) — trait
    //!   implemented by every built-in pipeline.
    //!
    //! ## Pipelines (feature-gated)
    //!
    //! | Feature                 | Pipeline                                                   |
    //! |-------------------------|-------------------------------------------------------------|
    //! | `ml-scene-classifier`   | [`oximedia_ml::pipelines::SceneClassifier`] (Places365)    |
    //! | `ml-shot-boundary`      | [`oximedia_ml::pipelines::ShotBoundaryDetector`] (TransNet)|
    //! | `ml-aesthetic-score`    | [`oximedia_ml::pipelines::AestheticScorer`] (NIMA)         |
    //! | `ml-object-detector`    | [`oximedia_ml::pipelines::ObjectDetector`] (YOLOv8)        |
    //! | `ml-face-embedder`      | [`oximedia_ml::pipelines::FaceEmbedder`] (ArcFace)         |
    //!
    //! ## Target matrix
    //!
    //! Builds on every target that OxiONNX supports — including
    //! `wasm32-unknown-unknown`. The `cuda` feature is native-only
    //! (libloading requires `cfg(any(unix, windows))`); every other
    //! `ml-*` feature is WASM-compatible and runs on the CPU path by
    //! default (or on WebGPU via the `webgpu` feature).
    pub use oximedia_ml::*;
}

/// Pure-Rust live camera/device capture: open a camera, negotiate a format
/// against what it advertises, and receive frames on a bounded queue.
///
/// Enable with `features = ["capture"]`.
///
/// See the [`oximedia_capture`] crate for the full API surface and its README
/// for the honest, per-platform verification status: macOS enumeration was
/// empirically validated against real hardware during package A5 (streaming
/// itself is gated behind the system TCC permission prompt); Linux (V4L2)
/// and Windows (Media Foundation) are fully implemented and pass their
/// host-side unit and const-evaluated ABI checks, but neither has yet been
/// exercised against a live device in this workspace's own verification.
/// `backend_kind()` and [`oximedia_capture::CaptureError::UnsupportedPlatform`]
/// make "no backend compiled in" and "backend present, no camera attached"
/// two distinguishable outcomes rather than the same empty list.
#[cfg(feature = "capture")]
pub mod capture {
    //! Live camera/device capture.
    //!
    //! Backends: AVFoundation (macOS), Video4Linux2 (Linux), Media Foundation
    //! (Windows). Delivers raw pixel formats or an MJPEG bitstream
    //! passthrough only — never a decode of an encumbered codec. A target
    //! with no backend reports [`oximedia_capture::CaptureError::UnsupportedPlatform`]
    //! rather than an empty device list.
    pub use oximedia_capture::*;
}

/// Motion JPEG (MJPEG) intra-frame video codec.
///
/// Enable with `features = ["mjpeg"]`.
///
/// See the [`oximedia_codec`] crate for the full API surface.
#[cfg(feature = "mjpeg")]
pub mod mjpeg {
    //! Motion JPEG video codec.
    //!
    //! MJPEG encodes each video frame independently as a baseline JPEG image.
    //! All frames are keyframes, providing random access, simple editing, and
    //! low encoding latency.  Commonly used in digital cameras, webcams, and
    //! AVI/MOV containers.
    pub use oximedia_codec::mjpeg::*;
}

/// APV (Advanced Professional Video) intra-frame codec (ISO/IEC 23009-13).
///
/// Enable with `features = ["apv"]`.
///
/// See the [`oximedia_codec`] crate for the full API surface.
#[cfg(feature = "apv")]
pub mod apv {
    //! APV (Advanced Professional Video) codec.
    //!
    //! APV is a royalty-free, intra-frame professional video codec for high quality,
    //! low latency, and random-access editing workflows.  Supports 8/10/12-bit
    //! depth, 4:2:0/4:2:2/4:4:4 chroma, QP-based quantization, and tile
    //! parallelism.
    pub use oximedia_codec::apv::*;
}

// ── Prelude ──────────────────────────────────────────────────────────────────

#[allow(ambiguous_glob_reexports)]
pub mod prelude;
