#![allow(unexpected_cfgs)]
#![allow(deprecated)]
//! Python bindings for `OxiMedia` using `PyO3`.
//!
//! This crate provides Python bindings for the `OxiMedia` multimedia framework,
//! enabling decoding, encoding, and muxing/demuxing of royalty-free media formats.
//!
//! # Supported Codecs
//!
//! ## Video
//! - AV1 (decode & encode)
//! - VP9 (decode)
//! - VP8 (decode)
//!
//! ## Audio
//! - Opus (decode)
//!
//! ## Containers
//! - Matroska/WebM (demux & mux)
//! - Ogg (demux & mux)
//! - FLAC (demux)
//! - WAV (demux)
//!
//! # Example Usage (Python)
//!
//! ```python
//! import oximedia
//!
//! # Decode video
//! decoder = oximedia.Av1Decoder()
//! decoder.send_packet(packet_data, pts=0)
//! frame = decoder.receive_frame()
//!
//! # Encode video
//! config = oximedia.EncoderConfig(
//!     width=1920,
//!     height=1080,
//!     framerate=(30, 1),
//!     crf=28.0
//! )
//! encoder = oximedia.Av1Encoder(config)
//! encoder.send_frame(frame)
//! packet = encoder.receive_packet()
//!
//! # Demux container
//! demuxer = oximedia.MatroskaDemuxer("video.mkv")
//! demuxer.probe()
//! streams = demuxer.streams()
//! packet = demuxer.read_packet()
//! ```

#![allow(clippy::used_underscore_binding)]
#![allow(clippy::borrow_deref_ref)]

/// AAF (Advanced Authoring Format) file handling bindings.
pub mod aaf_py;
/// Access control bindings (grant, revoke, check permissions).
pub mod access_py;
/// Media alignment and registration bindings.
pub mod align_py;
/// `oximedia.analytics` submodule — viewer session tracking, engagement
/// scoring, and attention heatmaps (real delegation to `oximedia-analytics`).
pub mod analytics_py;
/// Professional archive and digital preservation bindings.
pub mod archivepro_py;
/// Async pipeline via tokio + PyO3 (AsyncPipeline, PipelineResult, FilterSpec).
pub mod async_pipeline;
mod audio;
pub mod audio_analysis;
/// Audio processing graph builder (DAG) for Python bindings.
pub mod audio_graph;
/// Audio loudness normalization and dynamics processing.
pub mod audio_normalize;
/// Audio post-production bindings (ADR, stems, delivery, restoration).
pub mod audiopost_py;
/// Automated video editing bindings (highlight, social, trailer).
pub mod auto_py;
pub mod batch;
pub mod batch_bindings;
/// Batch media processing with GIL release for parallel Rust execution.
pub mod batch_parallel;
/// Batch processing bindings (job queuing, scheduling, execution).
pub mod batch_py;
/// `oximedia.benchmark` submodule — Python-accessible performance profiling.
pub mod benchmark_py;
/// Broadcast automation: PlayoutScheduler and BroadcastValidator.
pub mod broadcast;
/// Numpy-compatible buffer protocol support for zero-copy frame access.
pub mod buffer_protocol;
/// `oximedia.cache` submodule — LRU cache with TTL + pinning + stats (real
/// delegation to `oximedia-cache`).
pub mod cache_py;
/// Color calibration and matching bindings.
pub mod calibrate_py;
/// Caption processing bindings (parse, convert, validate).
pub mod captions_py;
/// Clip management and logging bindings.
pub mod clips_py;
/// Cloud storage bindings (upload, download, transcode, cost estimation).
pub mod cloud_py;
pub mod codec_info;
/// Collaborative editing session bindings.
pub mod collab_py;
/// Color management bindings (color spaces, pipelines, tone mapping, delta-E).
pub mod colormgmt_py;
/// Delivery conformance checking bindings (specs, checks, reports).
pub mod conform_py;
mod container;
/// Python context-manager protocol wrappers (ManagedDecoder, ManagedEncoder).
pub mod context_manager;
/// Media conversion bindings (format detection, profiles, batch convert).
pub mod convert_py;
/// Computer vision operations (face/motion detection, histogram, edge detection).
pub mod cv;
/// OpenCV-compatible Python API (oximedia.cv2 submodule).
pub mod cv2_compat;
/// DataFrame export (pandas / polars) from frame metadata.
pub mod dataframe;
/// Media deduplication and duplicate detection bindings.
pub mod dedup_py;
/// Video/audio denoising bindings.
pub mod denoise_py;
/// Distributed encoding cluster bindings.
pub mod distributed_py;
/// Dolby Vision RPU metadata bindings.
pub mod dolbyvision_py;
/// DRM encryption and key management bindings.
pub mod drm_py;
/// EDL parsing and timeline manipulation bindings.
pub mod edl;
/// Video and color effects (color grade, chroma key, blur, vignette).
pub mod effects;
mod error;
/// Structured error types with categories, severity, and batch collection.
pub mod error_types;
/// Python exception types for oximedia errors.
pub mod exceptions;
/// Render farm management bindings.
pub mod farm_py;
/// Filter graph node descriptions for Python-side graph building.
pub mod filter_bindings;
/// Filter graph pipeline for composing video/audio processing chains.
pub mod filter_graph;
mod filters;
/// Image/video forensic analysis bindings (ELA, noise, compression, tampering).
pub mod forensics_py;
/// Media format information, container capabilities, and codec queries.
pub mod format_info;
/// Frame format conversion utilities for Python bindings.
pub mod frame_converter;
/// Python iterator wrapper for frame-by-frame access.
pub mod frame_iterator;
/// Memory pool for frame allocation to reduce Python GC pressure.
pub mod frame_pool;
/// Gaming capture, highlight detection, and clip creation bindings.
pub mod gaming_py;
/// GPU device management and acceleration bindings.
pub mod gpu_py;
/// Broadcast graphics bindings (lower-thirds, tickers, overlays, templates).
pub mod graphics_py;
/// Professional image I/O and processing (DPX, EXR, TIFF, sequences).
pub mod image_py;
/// IMF (Interoperable Master Format) package bindings.
pub mod imf_py;
/// Jupyter notebook integration (inline display of frames and waveforms).
pub mod jupyter;
/// Python logging bridge — route Rust tracing events to Python logging module.
pub mod logging_py;
/// LUT (Look-Up Table) color grading operations.
pub mod lut;
/// Media Asset Management bindings (catalog, ingest, search, tag, export).
pub mod mam_py;
/// Media content hashing and fingerprinting for Python bindings.
pub mod media_hash;
/// Music Information Retrieval bindings (tempo, key, segmentation).
pub mod mir;
/// Audio mixer bindings (channels, mixing, panning, EQ).
pub mod mixer_py;
/// `oximedia.ml` submodule — typed ML pipelines (scene, shot boundary, aesthetic, detection, face).
#[cfg(feature = "ml")]
pub mod ml_py;
/// System monitoring bindings (metrics, alerts, health checks).
pub mod monitor_py;
/// MP4/ISOBMFF container demuxer bindings.
pub mod mp4;
/// Multi-camera bindings (timeline, switching, compositing).
pub mod multicam_py;
/// NDI source discovery and streaming bindings.
pub mod ndi_py;
/// `oximedia.neural` — multi-head attention and standalone attention/positional-encoding functions.
pub mod neural_attention_py;
/// `oximedia.neural` — object detection, face detection, and optical flow media pipelines.
pub mod neural_detect_py;
/// `oximedia.neural` — declarative `Sequential` / `ModelGraph` model builders with Python-callable layers.
pub mod neural_graph_py;
/// `oximedia.neural` — individual neural network layers (Linear, Conv2d, BatchNorm, pooling).
pub mod neural_layers_py;
/// `oximedia.neural` — ONNX model introspection and graph execution (`onnx` feature adds the full oxionnx backend).
pub mod neural_onnx_py;
/// `oximedia.neural` submodule — Tensor + media-model inference (scene
/// classifier, thumbnail ranker, super-resolution upscaler, feature
/// extractor; real delegation to `oximedia-neural`).
pub mod neural_py;
/// `oximedia.neural` — INT8 symmetric quantization for linear layers.
pub mod neural_quant_py;
/// `oximedia.neural` — GRU / LSTM recurrent sequence layers.
pub mod neural_recurrent_py;
/// `oximedia.neural` — pre-configured architecture catalogue (`MediaModelZoo`).
pub mod neural_zoo_py;
/// Codec optimization bindings (complexity analysis, CRF sweep, quality ladder).
pub mod optimize_py;
/// `oximedia.io` submodule — file open, probe, transcode operations.
pub mod oximedia_io_py;
/// Parallel processing utilities for Python bindings.
pub mod parallel;
/// Pickle serialization support for oximedia Python objects.
pub mod pickle_support;
pub mod pipeline_bindings;
pub mod pipeline_builder;
/// Broadcast playout server bindings (schedule, control, status).
pub mod playout_py;
/// Plugin system bindings (registry, plugin info, codec capabilities).
pub mod plugin_py;
/// Encoding presets bindings (list, get, manage presets).
pub mod presets_py;
mod probe;
/// Performance profiling bindings.
pub mod profiler_py;
/// Callback-based progress reporting for Python callers.
pub mod progress_callback;
/// Streaming progress reporting for async Python usage.
pub mod progress_reporting;
/// Progress tracking for long-running Python operations.
pub mod progress_tracker;
/// Proxy media generation and management bindings.
pub mod proxy_py;
/// Structured configuration sections and fluent builder for Python bindings.
pub mod py_config;
/// Typed error codes and Python-boundary error converter.
pub mod py_error;
/// Typed metadata fields and Python-interop converter.
pub mod py_metadata;
/// Read/write media metadata (ID3v2, Vorbis Comments, EXIF, APEv2, …).
pub mod py_metadata_rw;
/// Quality control and validation bindings (QC checks, reports, rules).
pub mod qc_py;
/// Quality assessment bindings (PSNR, SSIM, BRISQUE, NIQE, etc.).
pub mod quality;
/// Content recommendation engine bindings.
pub mod recommend_py;
/// Render queue management for Python bindings.
pub mod render_queue;
/// Render farm cluster management bindings.
pub mod renderfarm_py;
/// Python `__repr__` support for oximedia objects.
pub mod repr_support;
/// Audio restoration bindings (declip, decrackle, dehum, denoise).
pub mod restore_py;
/// Review and approval workflow bindings.
pub mod review_py;
/// Digital rights and license management bindings.
pub mod rights_py;
/// Audio/video routing bindings.
pub mod routing_py;
/// Video/image scaling bindings.
pub mod scaling_py;
/// Scene and shot detection bindings.
pub mod scene;
/// Video scopes bindings (waveform, vectorscope, histogram, parade, false color).
pub mod scopes_py;
/// Video stabilization bindings (motion estimation, trajectory smoothing).
pub mod stabilize_py;
/// Streaming media reader utilities for Python bindings.
pub mod stream_reader;
/// HLS/DASH adaptive streaming packaging bindings.
pub mod streaming_py;
/// Live production video switcher bindings (sources, transitions, macros).
pub mod switcher_py;
/// `oximedia.test` submodule — synthetic test media generators.
pub mod test_media;
/// Timecode manipulation bindings (SMPTE frame rates, TC arithmetic).
pub mod timecode_py;
pub mod timeline;
/// Timeline editing bindings (multi-track timeline, clips, tracks, transitions).
pub mod timeline_py;
/// Time synchronization bindings (PTP, NTP, drift, genlock).
pub mod timesync_py;
pub mod transcode_options;
/// Transcoding bindings (presets, ABR ladders, codec listing).
pub mod transcode_py;
mod types;
/// `oximedia.utils` submodule — common media helper functions.
pub mod utils_py;
/// Visual effects bindings (effects, chroma key, transitions, generators).
pub mod vfx_py;
mod video;
pub mod video_bindings;
pub mod video_meta;
/// Video-over-IP streaming bindings (RTP, SRT, RIST).
pub mod videoip_py;
/// Virtual production bindings.
pub mod virtual_py;
/// Audio watermarking and steganography bindings.
pub mod watermark_py;
/// Workflow orchestration bindings (DAG workflows, templates, execution).
pub mod workflow_py;

use pyo3::prelude::*;

/// `OxiMedia` Python module - Royalty-free multimedia processing library.
///
/// Provides video/audio encoding, decoding, and container muxing/demuxing
/// for patent-free codecs like AV1, VP9, VP8, Opus, and Vorbis.
#[pymodule]
#[pyo3(name = "oximedia")]
fn oximedia_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Error types
    m.add("OxiMediaError", m.py().get_type::<error::OxiMediaError>())?;

    // Core types
    m.add_class::<types::PixelFormat>()?;
    m.add_class::<types::SampleFormat>()?;
    m.add_class::<types::ChannelLayout>()?;
    m.add_class::<types::VideoFrame>()?;
    m.add_class::<types::AudioFrame>()?;
    m.add_class::<types::PyVideoPlaneBuffer>()?;
    m.add_class::<types::EncoderConfig>()?;
    m.add_class::<types::EncoderPreset>()?;
    m.add_class::<types::Rational>()?;

    // Video codecs
    m.add_class::<video::Av1Decoder>()?;
    m.add_class::<video::Av1Encoder>()?;
    m.add_class::<video::Vp9Decoder>()?;
    m.add_class::<video::Vp8Decoder>()?;

    // Audio codecs
    m.add_class::<audio::OpusDecoder>()?;
    m.add_class::<audio::VorbisDecoder>()?;
    m.add_class::<audio::FlacDecoder>()?;
    m.add_class::<audio::OpusEncoderConfig>()?;
    m.add_class::<audio::OpusEncoder>()?;

    // Filter graph configuration types
    m.add_class::<filters::PyScaleConfig>()?;
    m.add_class::<filters::PyCropConfig>()?;
    m.add_class::<filters::PyVolumeConfig>()?;
    m.add_class::<filters::PyNormalizeConfig>()?;

    // Container
    m.add_class::<container::Packet>()?;
    m.add_class::<container::StreamInfo>()?;
    m.add_class::<container::MatroskaDemuxer>()?;
    m.add_class::<container::OggDemuxer>()?;
    m.add_class::<container::MatroskaMuxer>()?;
    m.add_class::<container::OggMuxer>()?;

    // Probe / media-info types
    m.add_class::<probe::PyVideoInfo>()?;
    m.add_class::<probe::PyAudioInfo>()?;
    m.add_class::<probe::PyStreamInfo>()?;
    m.add_class::<probe::PyMediaInfo>()?;

    // Quality analysis
    m.add_class::<quality::PyQualityScore>()?;
    m.add_class::<quality::PyQualityAssessor>()?;
    m.add_function(wrap_pyfunction!(quality::compute_psnr, m)?)?;
    m.add_function(wrap_pyfunction!(quality::compute_ssim, m)?)?;
    m.add_function(wrap_pyfunction!(quality::quality_report, m)?)?;

    // Audio analysis & metering
    m.add_class::<audio_analysis::PyLoudnessResult>()?;
    m.add_class::<audio_analysis::PySpectralFeatures>()?;
    m.add_function(wrap_pyfunction!(audio_analysis::measure_loudness, m)?)?;
    m.add_function(wrap_pyfunction!(audio_analysis::detect_beats, m)?)?;
    m.add_function(wrap_pyfunction!(audio_analysis::spectral_features, m)?)?;
    m.add_function(wrap_pyfunction!(audio_analysis::detect_silence, m)?)?;

    // Scene / shot detection
    m.add_class::<scene::PyShot>()?;
    m.add_class::<scene::PyScene>()?;
    m.add_function(wrap_pyfunction!(scene::detect_scenes, m)?)?;
    m.add_function(wrap_pyfunction!(scene::classify_shots, m)?)?;

    // Filter graph
    m.add_class::<filter_graph::FilterGraph>()?;
    m.add_class::<filter_graph::PyFilterNode>()?;

    // Video / color effects
    m.add_function(wrap_pyfunction!(effects::apply_color_grade, m)?)?;
    m.add_function(wrap_pyfunction!(effects::apply_chromakey, m)?)?;
    m.add_function(wrap_pyfunction!(effects::apply_blur, m)?)?;
    m.add_function(wrap_pyfunction!(effects::apply_vignette, m)?)?;

    // Audio normalization & dynamics
    m.add_function(wrap_pyfunction!(audio_normalize::normalize_loudness, m)?)?;
    m.add_function(wrap_pyfunction!(audio_normalize::apply_compressor, m)?)?;
    m.add_function(wrap_pyfunction!(audio_normalize::apply_limiter, m)?)?;

    // LUT color grading
    m.add_class::<lut::PyLut3d>()?;
    m.add_function(wrap_pyfunction!(lut::load_lut, m)?)?;
    m.add_function(wrap_pyfunction!(lut::apply_lut, m)?)?;
    m.add_function(wrap_pyfunction!(lut::generate_identity_lut, m)?)?;

    // MP4 demuxer
    m.add_class::<mp4::Mp4Demuxer>()?;

    // Computer vision (cv.rs)
    cv::register(m)?;

    // Music Information Retrieval (mir.rs)
    mir::register(m)?;

    // Metadata read/write (py_metadata_rw.rs)
    py_metadata_rw::register(m)?;

    // EDL parsing and timeline (edl.rs)
    edl::register(m)?;

    // HLS/DASH adaptive streaming packaging (streaming_py.rs)
    streaming_py::register(m)?;

    // Async pipeline (async_pipeline.rs)
    async_pipeline::register(m)?;

    // Frame pool for reduced GC pressure (frame_pool.rs)
    frame_pool::register(m)?;

    // Jupyter / notebook display (jupyter.rs)
    jupyter::register(m)?;

    // DataFrame export — pandas / polars (dataframe.rs)
    dataframe::register(m)?;

    // Broadcast automation (broadcast.rs)
    broadcast::register(m)?;

    // Transcoding bindings
    transcode_py::register(m)?;

    // Color management bindings
    colormgmt_py::register(m)?;

    // Timecode bindings
    timecode_py::register(m)?;

    // Media conversion bindings
    convert_py::register(m)?;

    // Video stabilization bindings
    stabilize_py::register(m)?;

    // Forensic analysis bindings
    forensics_py::register(m)?;

    // System monitoring bindings
    monitor_py::register(m)?;

    // Audio restoration bindings
    restore_py::register(m)?;

    // Caption processing bindings
    captions_py::register(m)?;

    // Image I/O and processing (DPX, EXR, TIFF, sequences, filters)
    image_py::register(m)?;

    // Audio watermarking and steganography
    watermark_py::register(m)?;

    // Video scopes (waveform, vectorscope, histogram, parade, false color)
    scopes_py::register(m)?;

    // Video/audio denoising
    denoise_py::register(m)?;

    // Encoding presets
    presets_py::register(m)?;

    // Delivery conformance checking
    conform_py::register(m)?;

    // Batch processing
    batch_py::register(m)?;

    // Codec optimization
    optimize_py::register(m)?;

    // Audio mixer bindings
    mixer_py::register(m)?;

    // Audio post-production bindings
    audiopost_py::register(m)?;

    // Broadcast graphics
    graphics_py::register(m)?;

    // Multi-camera production
    multicam_py::register(m)?;

    // Timeline editing bindings
    timeline_py::register(m)?;

    // Visual effects bindings
    vfx_py::register(m)?;

    // Distributed encoding cluster bindings
    distributed_py::register(m)?;

    // Render farm management bindings
    farm_py::register(m)?;

    // NDI source discovery and streaming
    ndi_py::register(m)?;

    // Video-over-IP streaming (RTP, SRT, RIST)
    videoip_py::register(m)?;

    // Gaming capture, highlight detection, clip creation
    gaming_py::register(m)?;

    // GPU device management and acceleration
    gpu_py::register(m)?;

    // Media Asset Management
    mam_py::register(m)?;

    // Cloud storage operations
    cloud_py::register(m)?;

    // Plugin system bindings
    plugin_py::register(m)?;

    // Quality control bindings
    qc_py::register(m)?;

    // IMF package bindings
    imf_py::register(m)?;

    // AAF file bindings
    aaf_py::register(m)?;

    // Broadcast playout server bindings
    playout_py::register(m)?;

    // Live production video switcher bindings
    switcher_py::register(m)?;

    // Workflow orchestration bindings
    workflow_py::register(m)?;

    // Collaborative editing session bindings
    collab_py::register(m)?;

    // Proxy media generation and management
    proxy_py::register(m)?;

    // Clip management and logging
    clips_py::register(m)?;

    // Review and approval workflow
    review_py::register(m)?;

    // DRM encryption and key management
    drm_py::register(m)?;

    // Media deduplication
    dedup_py::register(m)?;

    // Professional archive and digital preservation
    archivepro_py::register(m)?;

    // Dolby Vision metadata
    dolbyvision_py::register(m)?;

    // Time synchronization bindings
    timesync_py::register(m)?;

    // Media alignment bindings
    align_py::register(m)?;

    // Audio/video routing bindings
    routing_py::register(m)?;

    // Color calibration bindings
    calibrate_py::register(m)?;

    // Virtual production bindings
    virtual_py::register(m)?;

    // Performance profiling bindings
    profiler_py::register(m)?;

    // Content recommendation engine
    recommend_py::register(m)?;

    // Video/image scaling
    scaling_py::register(m)?;

    // Render farm cluster management
    renderfarm_py::register(m)?;

    // Access control
    access_py::register(m)?;

    // Digital rights management
    rights_py::register(m)?;

    // Automated video editing
    auto_py::register(m)?;

    // cv2-compatible submodule (OpenCV Python API compat)
    let cv2_module = PyModule::new(m.py(), "cv2")?;
    cv2_compat::register_cv2(&cv2_module)?;
    m.add_submodule(&cv2_module)?;

    // oximedia.io submodule
    oximedia_io_py::register_submodule(m)?;

    // oximedia.logging submodule
    logging_py::register_submodule(m)?;

    // oximedia.utils submodule
    utils_py::register_submodule(m)?;

    // oximedia.benchmark submodule
    benchmark_py::register_submodule(m)?;

    // oximedia.test submodule
    test_media::register_submodule(m)?;

    // Python context-manager wrappers (ManagedDecoder, ManagedEncoder)
    context_manager::register(m)?;

    // oximedia.ml submodule (feature-gated)
    #[cfg(feature = "ml")]
    ml_py::register_submodule(m)?;

    // oximedia.neural submodule (Tensor + media-model inference)
    neural_py::register_submodule(m)?;

    // oximedia.analytics submodule (viewer sessions, engagement, heatmaps)
    analytics_py::register_submodule(m)?;

    // oximedia.cache submodule (LRU cache with TTL + pinning + stats)
    cache_py::register_submodule(m)?;

    Ok(())
}
