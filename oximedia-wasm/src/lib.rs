//! # `OxiMedia` WebAssembly Bindings
//!
//! This crate provides WebAssembly bindings for `OxiMedia`, enabling
//! patent-free multimedia processing in the browser.
//!
//! ## Features
//!
//! - **Format Probing**: Detect container formats from raw bytes (real
//!   magic-byte sniffing), plus dependency-free content hashing
//!   (`probe_hash`)
//! - **Container Demuxing**: `WasmDemuxer`/`WasmStreamingDemuxer` scaffolding
//!   for `WebM`, Matroska, Ogg, FLAC, WAV, MP4 -- format detection is real;
//!   full per-container packet extraction is not wired in yet and honestly
//!   errors rather than fabricating streams/packets (see `demuxer.rs` and
//!   `streaming_demuxer.rs` module docs)
//! - **Audio Decoding**: FLAC, Vorbis, and Opus decoders
//! - **Video Decoding**: VP8 decoder outputting YUV420 planar data
//! - **Audio Analysis**: Loudness measurement (EBU R128), beat detection, spectral features
//! - **Metadata Extraction**: ID3v2, Vorbis Comments, EXIF, iTunes, Matroska tags
//! - **Format Conversion**: Sample format and sample rate conversion
//! - **Zero-Copy**: Efficient buffer management using JavaScript `ArrayBuffer`
//! - **Browser-Native**: No file system, in-memory processing only
//!
//! ## JavaScript API
//!
//! ```javascript
//! import * as oximedia from 'oximedia-wasm';
//!
//! // Probe format from bytes
//! const data = new Uint8Array([0x1A, 0x45, 0xDF, 0xA3, ...]);
//! const result = oximedia.probe_format(data);
//! console.log('Format:', result.format, 'Confidence:', result.confidence);
//!
//! // Hash the full file (real CRC-32 / FNV-1a digests, not fabricated)
//! const hash = oximedia.probe_hash(data);
//! console.log('CRC32:', hash.crc32());
//!
//! // Demux a file. NOTE: full per-container parsing is not wired in yet
//! // for any format -- probe() currently always throws (see demuxer.rs
//! // module docs), so real callers must catch the rejection.
//! const demuxer = new oximedia.WasmDemuxer(data);
//! try {
//!     const probe = demuxer.probe();
//!     const streams = demuxer.streams();
//! } catch (e) {
//!     console.error('Full demux not yet available for this build:', e);
//! }
//!
//! // Decode audio
//! const decoder = new oximedia.WasmFlacDecoder();
//! decoder.init(streamInfoBytes);
//! const samples = decoder.decode_frame(frameData);
//!
//! // Analyze loudness
//! const metrics = JSON.parse(oximedia.wasm_analyze_loudness(samples, 48000, 2));
//! console.log('Loudness:', metrics.integrated_lufs, 'LUFS');
//!
//! // Parse metadata
//! const meta = JSON.parse(oximedia.wasm_parse_metadata(headerBytes));
//! console.log('Title:', meta.common.title);
//! ```
//!
//! ## Building
//!
//! ```bash
//! wasm-pack build --target web oximedia-wasm
//! ```

#![warn(missing_docs)]

use wasm_bindgen::prelude::*;

/// AAF file operations for the browser.
mod aaf_wasm;
/// Access control checking for the browser.
mod access_wasm;
/// Media alignment for the browser.
mod align_wasm;
mod analysis;
/// Session analytics for browser-side event tracking.
mod analytics_wasm;
/// Professional archive and preservation utilities for the browser.
mod archivepro_wasm;
mod audio_decoder;
/// Audio post-production utilities for the browser.
mod audiopost_wasm;
/// Automated editing utilities for the browser.
mod auto_wasm;
/// Batch processing utilities for the browser.
mod batch_wasm;
/// Synchronous single-poll driver for `MemorySource`-backed async futures
/// (see module docs for the soundness argument).
mod block_on;
/// In-memory LRU cache for browser-side use.
mod cache_wasm;
/// Color calibration for the browser.
mod calibrate_wasm;
/// Caption processing for the browser.
mod captions_wasm;
/// Clip management utilities for the browser.
mod clips_wasm;
/// Collaborative editing session utilities for the browser.
mod collab_wasm;
mod colormgmt_wasm;
/// Delivery conformance checking for the browser.
mod conform_wasm;
mod container;
/// Conversions from `oximedia_container`'s real demux types to this
/// crate's local `src/container.rs` mirror types.
mod container_bridge;
mod convert;
mod convert_wasm;
/// Media deduplication utilities for the browser.
mod dedup_wasm;
mod demuxer;
/// Video/audio denoising for the browser.
mod denoise_wasm;
/// Dolby Vision metadata utilities for the browser.
mod dolbyvision_wasm;
/// DRM encryption and decryption utilities for the browser.
mod drm_wasm;
mod filter_graph;
/// Image forensic analysis for the browser.
mod forensics_wasm;
/// Gaming highlight detection and capture utilities for the browser.
mod gaming_wasm;
/// Broadcast graphics for the browser.
mod graphics_wasm;
/// HDR transfer functions (PQ/HLG) and tone mapping for the browser.
mod hdr_wasm;
/// Professional image operations (DPX/EXR header parsing, filtering, histograms).
mod image_wasm;
/// IMF package operations for the browser.
mod imf_wasm;
mod io;
/// 3-D LUT application, photographic presets, and `.cube` parsing for the browser.
mod lut_wasm;
mod media_player;
mod metadata_wasm;
/// Music Information Retrieval for the browser.
mod mir_wasm;
/// Audio mixer for the browser.
mod mixer_wasm;
/// System monitoring for the browser.
mod monitor_wasm;
/// Multi-camera compositing for the browser.
mod multicam_wasm;
mod muxer;
/// Neural network tensor operations for the browser.
mod neural_wasm;
/// Loudness normalization for the browser.
mod normalize_wasm;
/// Broadcast playout schedule management for the browser.
mod playout_wasm;
/// Plugin system information for the browser.
mod plugin_wasm;
/// Encoding presets for the browser.
mod presets_wasm;
mod probe;
/// Performance profiling for the browser.
mod profiler_wasm;
/// Proxy media utilities for the browser.
mod proxy_wasm;
/// Quality control for the browser.
mod qc_wasm;
mod quality_wasm;
/// Content recommendation for the browser.
mod recommend_wasm;
/// Render farm cluster utilities for the browser.
mod renderfarm_wasm;
/// Audio restoration for the browser.
mod restore_wasm;
/// Review and approval workflow utilities for the browser.
mod review_wasm;
/// Digital rights checking for the browser.
mod rights_wasm;
/// Audio/video routing for the browser.
mod routing_wasm;
/// Video/image scaling for the browser.
mod scaling_wasm;
mod scene_wasm;
/// Video scopes for the browser.
mod scopes_wasm;
/// Shot boundary (cut) detection for the browser.
mod shots_wasm;
/// Ambisonics (HOA) encode/decode and VBAP panning for the browser.
mod spatial_wasm;
/// Video stabilization for the browser.
mod stabilize_wasm;
/// Streaming manifest utilities for the browser.
mod stream_wasm;
mod streaming_demuxer;
mod subtitle_wasm;
/// Live production video switching for the browser.
mod switcher_wasm;
mod timecode_wasm;
/// Timeline editing for the browser.
mod timeline_wasm;
/// Time synchronization for the browser.
mod timesync_wasm;
mod transcode_wasm;
mod types;
mod utils;
/// Visual effects and compositing for the browser.
mod vfx_wasm;
mod video_encoder;
/// Virtual production for the browser.
mod virtual_wasm;
/// Audio/image watermarking for the browser.
mod watermark_wasm;
mod webcodecs_bridge;
mod worker_helpers;
/// Workflow orchestration for the browser.
mod workflow_wasm;

// Re-export main types
pub use aaf_wasm::{wasm_aaf_track_list, wasm_parse_aaf_header, wasm_validate_aaf};
pub use access_wasm::{wasm_access_roles, wasm_check_permission, wasm_list_policies};
pub use align_wasm::{wasm_align_audio, wasm_align_methods, wasm_detect_offset};
pub use analysis::{wasm_analyze_loudness, wasm_detect_beats, wasm_spectral_features};
pub use analytics_wasm::SessionTracker;
pub use archivepro_wasm::{
    wasm_archive_formats, wasm_validate_archive_policy, wasm_verify_checksum,
};
pub use audio_decoder::{WasmFlacDecoder, WasmOpusDecoder};
pub use audiopost_wasm::{
    wasm_check_delivery_spec, wasm_export_stems_info, wasm_mix_audio, wasm_restore_audio,
};
pub use auto_wasm::{wasm_auto_templates, wasm_list_auto_tasks, wasm_validate_automation};
pub use batch_wasm::{wasm_estimate_batch_time, wasm_validate_batch_config, WasmBatchQueue};
pub use cache_wasm::WasmLruCache;
pub use calibrate_wasm::{
    wasm_analyze_pattern, wasm_calibration_targets, wasm_generate_test_pattern,
};
pub use captions_wasm::{wasm_convert_captions, wasm_parse_captions, wasm_validate_captions};
pub use clips_wasm::{wasm_create_clip, wasm_merge_clips, WasmClipManager};
pub use collab_wasm::{
    wasm_collab_features, wasm_collab_status, wasm_create_session_config, wasm_merge_edits,
};
pub use colormgmt_wasm::{
    wasm_apply_tone_map, wasm_convert_colorspace, wasm_convert_colorspace_u8, wasm_delta_e,
    wasm_delta_e_2000, wasm_gamut_check, wasm_list_colorspaces, wasm_list_tone_map_operators,
    WasmColorPipeline,
};
pub use conform_wasm::{
    wasm_check_conform, wasm_get_delivery_spec, wasm_list_conform_specs, wasm_validate_against_spec,
};
pub use convert::{wasm_convert_sample_format, wasm_resample};
pub use convert_wasm::{
    wasm_detect_format, wasm_list_convert_codecs, wasm_list_formats, wasm_list_profiles,
    wasm_pixel_convert, wasm_recommend_codec, wasm_validate_conversion,
};
pub use dedup_wasm::{wasm_compare_frames, wasm_compute_media_hash, wasm_dedup_strategies};
pub use demuxer::WasmDemuxer;
pub use denoise_wasm::{
    wasm_denoise_audio, wasm_denoise_frame, wasm_denoise_modes, wasm_denoise_presets,
    wasm_estimate_noise,
};
pub use dolbyvision_wasm::{wasm_dv_profiles, wasm_parse_dv_metadata, wasm_validate_dv};
pub use drm_wasm::{wasm_decrypt_data, wasm_drm_info, wasm_drm_schemes, wasm_encrypt_data};
pub use filter_graph::WasmFilterGraph;
pub use forensics_wasm::{
    wasm_check_integrity, wasm_compression_analysis, wasm_ela_analysis, wasm_forensic_report,
    wasm_forensic_tests, wasm_noise_analysis,
};
pub use gaming_wasm::{
    wasm_detect_audio_peak, wasm_detect_motion_intensity, wasm_gaming_capture_settings,
    wasm_generate_clip_preview, WasmHighlightDetector,
};
pub use graphics_wasm::{
    wasm_list_templates, wasm_render_color_bars, wasm_render_template, WasmGraphicsRenderer,
};
pub use hdr_wasm::{
    wasm_apply_eotf, wasm_apply_oetf, wasm_hdr_tone_map, wasm_hlg_eotf, wasm_hlg_eotf_frame,
    wasm_hlg_oetf, wasm_hlg_oetf_frame, wasm_list_tone_operators, wasm_list_transfer_functions,
    wasm_pq_eotf, wasm_pq_eotf_frame, wasm_pq_oetf, wasm_pq_oetf_frame, wasm_sdr_gamma,
    wasm_sdr_gamma_inv, WasmHdrConverter,
};
pub use image_wasm::{
    wasm_apply_image_filter, wasm_convert_pixel_depth, wasm_detect_image_format,
    wasm_image_histogram, wasm_image_info, wasm_read_dpx, wasm_read_exr,
};
pub use imf_wasm::{wasm_imf_track_info, wasm_parse_imf_cpl, wasm_validate_imf_cpl};
pub use lut_wasm::{
    wasm_apply_cube_lut, wasm_apply_lut_preset_frame, wasm_apply_lut_preset_pixel,
    wasm_inspect_cube_lut, wasm_lut_identity, wasm_lut_preset_names, WasmLut3d,
};
pub use media_player::WasmMediaPlayer;
pub use metadata_wasm::wasm_parse_metadata;
pub use mir_wasm::{
    wasm_detect_chords, wasm_detect_key, wasm_detect_tempo, wasm_mir_analyze, wasm_segment_audio,
};
pub use mixer_wasm::{wasm_apply_gain, wasm_apply_pan, wasm_mix_stereo, WasmAudioMixer};
pub use monitor_wasm::{wasm_analyze_signal_quality, wasm_check_stream_health, WasmStreamMonitor};
pub use multicam_wasm::{
    wasm_composite_grid, wasm_composite_pip, wasm_list_layouts, WasmMultiCamCompositor,
};
pub use muxer::WasmMuxer;
pub use neural_wasm::TensorWasm;
pub use normalize_wasm::LoudnessNormalizer;
pub use playout_wasm::{
    wasm_playout_formats, wasm_playout_status, wasm_validate_schedule, WasmPlayoutScheduler,
};
pub use plugin_wasm::{
    wasm_list_builtin_codecs, wasm_plugin_api_version, wasm_plugin_system_info, WasmPluginRegistry,
};
pub use presets_wasm::{
    wasm_get_preset, wasm_list_encoding_presets, wasm_merge_presets, wasm_preset_names,
    wasm_validate_preset,
};
pub use probe::{probe_format, probe_hash, WasmHashResult, WasmProbeResult};
pub use profiler_wasm::{
    wasm_benchmark_codec, wasm_bottleneck_report, wasm_profile_operation, wasm_profiling_modes,
};
pub use proxy_wasm::{
    wasm_estimate_proxy_size, wasm_proxy_formats, wasm_proxy_resolutions, wasm_proxy_settings,
};
pub use qc_wasm::{wasm_list_qc_rules, wasm_qc_check, wasm_qc_validate_frame};
pub use quality_wasm::{wasm_compute_psnr, wasm_compute_ssim, wasm_frame_quality};
pub use recommend_wasm::{
    wasm_analyze_content, wasm_recommend_codec_for_use_case, wasm_recommend_encoding_settings,
    wasm_recommendation_strategies,
};
pub use renderfarm_wasm::{wasm_estimate_render_time, wasm_farm_node_types, wasm_farm_status};
pub use restore_wasm::{wasm_analyze_degradation, wasm_declip_audio, wasm_restore_audio_samples};
pub use review_wasm::{
    wasm_annotation_types, wasm_create_annotation, wasm_export_annotations, wasm_review_formats,
    wasm_review_workflows,
};
pub use rights_wasm::{wasm_check_rights, wasm_license_types, wasm_rights_report};
pub use routing_wasm::{wasm_list_node_types, wasm_route_info, wasm_validate_route};
pub use scaling_wasm::{
    wasm_calculate_dimensions, wasm_downscale_frame, wasm_scaling_algorithms, wasm_upscale_frame,
};
pub use scene_wasm::wasm_detect_scenes;
pub use scopes_wasm::{wasm_analyze_exposure, wasm_false_color, wasm_scope_types, WasmVideoScopes};
pub use shots_wasm::ShotDetector;
pub use spatial_wasm::{
    wasm_ambisonics_channel_count, wasm_ambisonics_decode_stereo, wasm_ambisonics_encode,
    wasm_ambisonics_orders, wasm_spatial_speaker_layouts, wasm_vbap_pan, WasmAmbisonicsEncoder,
};
pub use stabilize_wasm::{wasm_estimate_motion, wasm_stabilization_modes, WasmStabilizer};
pub use stream_wasm::{parse_manifest_info, wasm_build_master_playlist, ManifestInfo};
pub use streaming_demuxer::WasmStreamingDemuxer;
pub use subtitle_wasm::{
    wasm_convert_subtitles, wasm_parse_ass, wasm_parse_srt, wasm_parse_vtt, wasm_shift_subtitles,
};
pub use switcher_wasm::{
    wasm_create_transition, wasm_list_switcher_presets, wasm_list_transition_types, WasmSwitcher,
};
pub use timecode_wasm::{
    wasm_frames_to_timecode, wasm_list_frame_rates, wasm_parse_timecode, wasm_seconds_to_timecode,
    wasm_timecode_add, wasm_timecode_subtract, wasm_timecode_to_frames, wasm_timecode_to_seconds,
};
pub use timeline_wasm::{
    wasm_create_timeline, wasm_timeline_formats, wasm_validate_timeline, WasmTimeline,
};
pub use timesync_wasm::{wasm_detect_sync_offset, wasm_measure_drift, wasm_sync_methods};
pub use transcode_wasm::{
    wasm_estimate_output_size, wasm_list_codecs, wasm_list_presets, wasm_recommend_settings,
    wasm_validate_transcode_config, WasmTranscodeWorker,
};
pub use types::{WasmCodecParams, WasmMetadata, WasmPacket, WasmStreamInfo};
pub use vfx_wasm::{
    wasm_apply_effect, wasm_chroma_key, wasm_dissolve, wasm_generate_pattern, wasm_list_effects,
    wasm_list_patterns, wasm_list_transitions,
};
pub use video_encoder::WasmVideoEncoder;
pub use virtual_wasm::{
    wasm_generate_test_source, wasm_virtual_settings, wasm_virtual_source_types,
    wasm_virtual_workflows, WasmVirtualSource,
};
pub use watermark_wasm::{
    wasm_detect_audio_watermark, wasm_detect_image_watermark, wasm_embed_audio_watermark,
    wasm_embed_image_watermark, wasm_list_watermark_algorithms, wasm_watermark_capacity,
    wasm_watermark_quality,
};
pub use webcodecs_bridge::{WasmEncodedChunkInfo, WasmVideoDecoderConfig, WasmWebCodecsBridge};
pub use worker_helpers::{
    parse_transfer_header, split_transfer_planes, transferable_frame, transferable_frame_rgba,
};
pub use workflow_wasm::{
    wasm_get_workflow_template, wasm_list_workflow_templates, wasm_validate_workflow,
    wasm_workflow_step_types,
};

/// Initialize the WASM module.
///
/// This should be called once when the module is loaded.
/// It sets up panic hooks for better error messages in the browser console.
#[wasm_bindgen(start)]
pub fn init() {
    console_error_panic_hook::set_once();
}

/// Get the version of `OxiMedia` WASM.
///
/// Returns the version string in semver format.
///
/// # Example
///
/// ```javascript
/// import * as oximedia from 'oximedia-wasm';
/// console.log('OxiMedia version:', oximedia.version());
/// ```
#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
