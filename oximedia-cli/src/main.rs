//! OxiMedia CLI — Patent-free multimedia processing framework.
//!
//! `oximedia` is a command-line tool for working with media files using only
//! royalty-free codecs. It is part of the [OxiMedia](https://github.com/cool-japan/oximedia)
//! sovereign media framework — a pure-Rust reconstruction of both FFmpeg and OpenCV.
//!
//! # Quick Start
//!
//! ```notrust
//! # Probe a media file
//! oximedia probe -i input.mkv
//!
//! # Transcode video (alias: convert)
//! oximedia transcode -i input.mkv -o output.webm --video-codec vp9 --video-bitrate 2M
//!
//! # Extract frames
//! oximedia extract -i input.mkv -o frames_%04d.png
//!
//! # Batch process a directory
//! oximedia batch input_dir/ output_dir/ config.toml
//!
//! # FFmpeg compatibility layer
//! oximedia ff -i input.mp4 -c:v libvpx-vp9 output.webm
//! ```
//!
//! # Supported Formats
//!
//! OxiMedia uses only patent-free codecs:
//! - **Video:** AV1, VP9, VP8, Theora, FFV1
//! - **Audio:** Opus, Vorbis, FLAC, PCM, WAV
//! - **Containers:** Matroska (.mkv), WebM, Ogg, FLAC, WAV
//!
//! ## Subcommand Families
//!
//! ### Inspection
//!
//! | Subcommand | Description |
//! |---|---|
//! | `probe` | Inspect media file metadata: codec, resolution, duration, tracks, chapters |
//! | `info` | Show supported formats, codecs, and feature flags |
//! | `version` | Print OxiMedia version and build info |
//! | `doctor` | Diagnose the runtime environment: GPU adapters, temp dir writability |
//! | `analyze` | Video/audio quality metrics analysis |
//!
//! ### Transcode and Convert
//!
//! | Subcommand | Description |
//! |---|---|
//! | `transcode` | Re-encode video/audio with codec and quality control (alias: `convert`) |
//! | `extract` | Extract individual frames to PNG, JPEG, or PPM |
//! | `thumbnail` | Generate single thumbnails from a video file |
//! | `sprite` | Generate thumbnail sprite sheets for video scrubbing |
//! | `concat` | Concatenate multiple video/audio files |
//! | `batch` | Batch-process multiple files in a directory |
//! | `batch-engine` | SQLite-backed job queue: submit, status, list, cancel, report |
//! | `scaling` | Upscale, downscale, analyze, compare, batch scale operations |
//! | `optimize` | Codec complexity analysis, CRF sweep, quality ladder, benchmark |
//!
//! ### Audio Processing
//!
//! | Subcommand | Description |
//! |---|---|
//! | `audio` | Loudness metering, normalization, spectrum, and beat detection |
//! | `loudness` | EBU R128 / ITU-R BS.1770 loudness analysis and validation |
//! | `normalize` | Normalize audio to a target loudness standard |
//! | `mixer` | Mix multiple audio tracks with a declarative routing bus |
//! | `audiopost` | ADR, stem mixing, delivery, and audio restoration |
//! | `filter` | Standalone filter graph processing |
//!
//! ### Video Analysis and Enhancement
//!
//! | Subcommand | Description |
//! |---|---|
//! | `scopes` | Waveform, vectorscope, histogram, parade, false-colour scopes |
//! | `quality` | Perceptual quality metrics: VMAF, SSIM, PSNR comparison |
//! | `dedup` | Detect near-duplicate clips via perceptual hashing |
//! | `restore` | Audio/video restoration: de-noise, de-click, upscale, deinterlace |
//! | `denoise` | Video noise reduction with spatial and temporal modes |
//! | `stabilize` | Remove camera shake (translation, affine, perspective, 3D models) |
//! | `scene` | Shot detection, classification, and storyboard generation |
//! | `lut` | Apply, inspect, convert, or generate LUT files (.cube, Hald) |
//! | `graphics` | 2D broadcast graphics: lower-thirds, tickers, overlays, templates |
//! | `image` | Professional image operations (DPX, EXR, TIFF, sequences) |
//! | `color` | Color management: convert, inspect matrix, compute Delta E |
//! | `forensics` | Tamper detection, integrity analysis, media provenance |
//! | `mir` | Music Information Retrieval: tempo, key, chords, segmentation |
//! | `ml` | Sovereign ML pipelines via Pure-Rust ONNX (oximedia-ml) |
//!
//! ### Broadcast and Live Production
//!
//! | Subcommand | Description |
//! |---|---|
//! | `playout` | Schedule and play out media files on an output timeline |
//! | `switcher` | Software vision mixer with keying, transitions, and macro recording |
//! | `ndi` | NDI source discovery, streaming, and monitoring |
//! | `videoip` | Video-over-IP: send, receive, discover (RTP/SRT/RIST) |
//! | `multicam` | Multi-camera sync, switch, composite, color-match, export |
//! | `monitor` | Real-time system and pipeline health monitoring with alerting |
//! | `stream` | HLS/DASH adaptive streaming: serve, ingest, record |
//! | `timesync` | Time synchronisation: analyze, align, offset, drift, report |
//! | `gaming` | Gaming: capture, clip, overlay |
//!
//! ### MAM and Archive
//!
//! | Subcommand | Description |
//! |---|---|
//! | `mam` | Media asset management: ingest, tag, search, export, catalogue |
//! | `search` | Full-text and visual similarity search over the MAM catalogue |
//! | `archive` | IMF/archive packaging and extraction |
//! | `archive-pro` | Archive Pro: ingest, verify, migrate, report, policy |
//! | `clips` | Clip management: create, list, export, trim, merge, tag |
//! | `playlist` | Playlist generation, validation, and playout automation |
//! | `cloud` | Cloud storage: upload, download, transcode (S3/GCS/Azure/R2/B2) |
//! | `storage` | Inspect and manage local storage backends |
//!
//! ### Post Production
//!
//! | Subcommand | Description |
//! |---|---|
//! | `timeline` | Timeline editing: inspect, edit, export (OTIO, EDL, FCP XML) |
//! | `edl` | EDL parse, validate, export, and conform |
//! | `conform` | QC/conformance checking and automated fixing |
//! | `qc` | Quality Control: check, validate, report, rules, fix |
//! | `imf` | IMF: validate, package, info, extract, create |
//! | `aaf` | AAF: info, extract, convert, validate, merge |
//! | `vfx` | Visual effects and compositing |
//! | `subtitle` | Subtitle conversion, extraction, burn-in, and synchronization |
//! | `captions` | Generate and synchronise closed captions |
//! | `dolby-vision` | Dolby Vision: analyze, convert, metadata, validate, info |
//! | `review` | Review: create, annotate, approve, reject, export, status |
//! | `collab` | Collaboration: create, join, share, comment, export, status |
//! | `align` | Audio/video alignment: sync, offset detection |
//! | `calibrate` | Display, audio, color calibration and pattern generation |
//!
//! ### Rights and Security
//!
//! | Subcommand | Description |
//! |---|---|
//! | `drm` | DRM: encrypt, decrypt, manage keys, validate |
//! | `watermark` | Digital audio/video watermarking |
//! | `access` | Access control: grant, revoke, list, policy, audit |
//! | `rights` | Digital rights: register, check, transfer, license, search |
//!
//! ### Infrastructure
//!
//! | Subcommand | Description |
//! |---|---|
//! | `distributed` | Distributed encoding: coordinator, workers, job submission |
//! | `farm` | Render farm: start, submit, status, cancel, nodes |
//! | `renderfarm` | Render farm cluster: init, add-node, submit, status, dashboard |
//! | `routing` | Signal routing: create, connect, validate, list |
//! | `virtual` | Virtual production: LED volume, camera tracking, configure |
//! | `profiler` | Profiler: run, report, compare, export, bottleneck analysis |
//! | `package` | HLS/DASH adaptive-bitrate packaging with optional encryption |
//!
//! ### Analysis and Recommendations
//!
//! | Subcommand | Description |
//! |---|---|
//! | `recommend` | Codec, settings, and workflow recommendations |
//! | `workflow` | Workflow orchestration: create, run, status, list, template |
//! | `auto` | Auto editing: run, schedule, create, delete, log |
//! | `benchmark` | Run encoding benchmarks |
//! | `validate` | Validate media file integrity |
//! | `metadata` | Edit media metadata and tags |
//!
//! ### Compatibility
//!
//! | Subcommand | Description |
//! |---|---|
//! | `ff` / `ffcompat` | FFmpeg argument compatibility layer — translates FFmpeg CLI flags to OxiMedia |
//!
//! ### Tooling
//!
//! | Subcommand | Description |
//! |---|---|
//! | `completions` | Print shell completion script (bash, zsh, fish, powershell, elvish) |
//! | `man-page` | Print roff man page to stdout |
//! | `tui` | Launch the interactive terminal UI |
//! | `plugin` | Manage OxiMedia plugins: list, info, codecs, validate, paths |
//! | `preset` | List and manage encoding and processing presets |
//! | `timecode` | Timecode convert, calculate, validate, burn-in |
//! | `proxy` | Proxy media: generate, list, link, info, clean |
//! | `concat` | Concatenate multiple media files |
//! | `repair` | Media file repair and recovery |
//!
//! ## Global Flags
//!
//! | Flag | Default | Description |
//! |---|---|---|
//! | `--json` | off | Output pure JSON on stdout; suppress coloured text |
//! | `--ndjson` | off | Emit one NDJSON record per item (conflicts with `--json`) |
//! | `--no-color` | off | Disable colour output |
//! | `--log-format json` | plain | Structured JSON log output via `tracing-subscriber` |
//! | `--progress <plain\|json>` | plain | Progress reporting format |
//! | `-v` / `--verbose` | off | Enable debug/trace log output (repeatable: `-vv`, `-vvv`) |
//! | `-q` / `--quiet` | off | Suppress logs and action-command status/banner stdout (most subcommands); query/report results (info/status/list/verify/...), `--json`/`--ndjson` output, and errors still print |
//!
//! ## See Also
//!
//! - Plugin system: `plugin_cmd` — see `$OXIMEDIA_PLUGIN_PATH` for search paths.
//! - Preset system: `presets` — see built-in categories (Web, Device, Streaming, Archival, Custom).
//! - FFmpeg compatibility: `ffcompat_cmd` and the `ff` alias.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::unused_async)]
#![allow(clippy::unnested_or_patterns)]
#![allow(clippy::unnecessary_wraps)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::trivially_copy_pass_by_ref)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]
#![allow(clippy::struct_excessive_bools)]
#![allow(clippy::cast_precision_loss)]
#![allow(clippy::similar_names)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::if_not_else)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::case_sensitive_file_extension_comparisons)]
#![allow(clippy::doc_link_with_quotes)]
#![allow(clippy::match_same_arms)]
#![allow(clippy::needless_continue)]
#![allow(clippy::single_char_pattern)]

mod aaf_cmd;
mod access_cmd;
mod align_cmd;
mod analyze;
mod archive_cmd;
mod archivepro_cmd;
mod completions_cmd;
mod doctor_cmd;
mod exit_codes;
mod man_cmd;
mod output;

mod audio_cmd;
mod audiopost_cmd;
mod auto_cmd;
mod batch;
mod batch_cmd;
mod benchmark;
mod calibrate_cmd;
mod captions_cmd;
mod clips_cmd;
mod cloud_cmd;
mod collab_cmd;
mod color_cmd;
mod commands;
mod concat;
mod conform_cmd;
mod decode_helper;
mod dedup_cmd;
mod denoise_cmd;
mod distributed_cmd;
mod dolbyvision_cmd;
mod drm_cmd;
mod edl_cmd;
mod extract;
mod farm_cmd;
mod ffcompat_cmd;
mod filter_cmd;
mod forensics_cmd;
mod frame_extract;
// The frame harness is shared with the lib target (see `lib.rs`), which
// exports the whole foundation — the `f32` plane bridges, the clip helpers —
// for the integration tests and for the ops the follow-up slice adds. This
// binary reaches for only part of it, so unused-item warnings *here* are
// expected rather than a sign of dead code.
#[allow(dead_code)]
mod frame_harness;
mod gaming_cmd;
mod graphics_cmd;
mod handlers;
mod image_cmd;
mod imf_cmd;
mod loudness_cmd;
mod lut_cmd;
mod mam_cmd;
mod metadata;
mod mir_cmd;
mod mixer_cmd;
mod ml_cmd;
mod monitor_cmd;
mod multicam_cmd;
mod ndi_cmd;
mod normalize_cmd;
mod optimize_cmd;
mod package_cmd;
mod playlist_cmd;
mod profiler_cmd;

mod playout_cmd;
mod plugin_cmd;
mod presets;
mod progress;
mod proxy_cmd;
mod qc_cmd;
mod quality_cmd;
mod quality_snapshot;
mod recommend_cmd;
mod renderfarm_cmd;
mod repair_cmd;
mod restore_cmd;
mod review_cmd;
mod rights_cmd;
mod routing_cmd;
mod scaling_cmd;
mod scene;
mod scopes_cmd;
mod search_cmd;
mod sprite;
mod stabilize_cmd;
mod stream_cmd;
mod subtitle_cmd;
mod switcher_cmd;
mod thumbnail;
mod timecode_cmd;
mod timeline_cmd;
mod timesync_cmd;
mod transcode;
mod tui_cmd;
mod validate;
mod vfx_cmd;
mod videoip_cmd;
mod virtual_cmd;
mod watermark_cmd;
mod workflow_cmd;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;

use commands::Commands;
use handlers::{
    handle_captions_command, handle_monitor_command, handle_preset_command, handle_restore_command,
    init_color, init_logging, probe_file, show_info, show_version, LogFormat,
};
use progress::ProgressFormat;

/// Patent-free multimedia framework CLI
#[derive(Parser)]
#[command(name = "oximedia")]
#[command(author, version, about, long_about = None)]
struct Cli {
    /// Subcommand to execute
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output (can be used multiple times: -v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Suppress log output and status/banner text on stdout (plans,
    /// summaries, and other decoration printed by action commands across
    /// most subcommand handlers). Query/report command results (info,
    /// status, list, verify, validate, probe data, analysis values,
    /// --json/--ndjson output) and errors still print — see
    /// `progress::is_quiet` for the full accounting and the nine
    /// frame-harness handlers still pending a later pass.
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    /// Output results in JSON format
    #[arg(long, global = true)]
    json: bool,

    /// Output as NDJSON (one record per line); conflicts with --json
    #[arg(long, global = true, conflicts_with = "json")]
    ndjson: bool,

    /// Log output format
    #[arg(long, global = true, default_value = "plain", value_enum)]
    log_format: LogFormat,

    /// Progress output format (plain bar or streaming JSON to stderr)
    #[arg(long, global = true, default_value = "plain", value_enum)]
    progress: ProgressFormat,
}

/// Dispatch CLI subcommands and return a `Result`.
async fn run(cli: Cli) -> Result<()> {
    // Resolve the output format once so individual handlers can query it.
    let _output_fmt = output::resolve_format(cli.json, cli.ndjson);

    let result = match cli.command {
        Commands::Probe {
            input,
            detail,
            streams,
            hash,
            quality_snapshot,
            format,
            chapters,
            metadata,
        } => {
            probe_file(
                &input,
                detail,
                streams,
                hash,
                quality_snapshot,
                &format,
                chapters,
                metadata,
                cli.ndjson,
            )
            .await
        }

        Commands::Info => {
            show_info();
            Ok(())
        }

        Commands::Version => {
            show_version(cli.json);
            Ok(())
        }

        Commands::Transcode {
            input,
            output,
            preset_name,
            video_codec,
            audio_codec,
            video_bitrate,
            audio_bitrate,
            scale,
            video_filter,
            start_time,
            duration,
            framerate,
            preset,
            two_pass,
            crf,
            threads,
            overwrite,
            audio_filter,
            map,
            normalize_audio,
        } => {
            let options = transcode::TranscodeOptions {
                input,
                output,
                preset_name,
                video_codec,
                audio_codec,
                video_bitrate,
                audio_bitrate,
                scale,
                video_filter,
                audio_filter,
                start_time,
                duration,
                framerate,
                preset,
                two_pass,
                crf,
                threads,
                overwrite,
                map,
                normalize_audio,
                progress_format: cli.progress,
            };
            transcode::transcode(options).await
        }

        Commands::Extract {
            input,
            output_pattern,
            format,
            start_time,
            frames,
            every,
            quality,
        } => {
            let options = extract::ExtractOptions {
                input,
                output_pattern,
                format,
                start_time,
                frames,
                every,
                quality,
            };
            extract::extract_frames(options).await
        }

        Commands::Batch {
            input_dir,
            output_dir,
            config,
            jobs,
            continue_on_error,
            dry_run,
        } => {
            let options = batch::BatchOptions {
                input_dir,
                output_dir,
                config,
                jobs,
                continue_on_error,
                dry_run,
                progress_format: cli.progress,
            };
            batch::batch_process(options).await
        }

        Commands::Concat {
            inputs,
            output,
            method,
            validate,
            overwrite,
        } => {
            let concat_method = concat::ConcatMethod::from_str(&method)?;
            let options = concat::ConcatOptions {
                inputs,
                output,
                method: concat_method,
                validate,
                overwrite,
                json_output: cli.json,
                transition: concat::TransitionType::CleanCut,
                chapter_options: concat::ChapterOptions::default(),
                stream_selection: None,
                trim_ranges: Vec::new(),
                edl_file: None,
                force_format: None,
                keyframe_align: false,
                max_audio_desync_ms: 100.0,
            };
            concat::concat_videos(options).await
        }

        Commands::Thumbnail {
            input,
            output,
            mode,
            timestamp,
            count,
            rows,
            cols,
            width,
            height,
            format,
            quality,
        } => {
            let thumb_mode = match mode.to_lowercase().as_str() {
                "single" => {
                    let ts = if let Some(ref ts_str) = timestamp {
                        thumbnail::parse_timestamp(ts_str)?
                    } else {
                        return Err(anyhow::anyhow!(
                            "Timestamp is required for single mode (use --timestamp)"
                        ));
                    };
                    thumbnail::ThumbnailMode::Single { timestamp: ts }
                }
                "multiple" => thumbnail::ThumbnailMode::Multiple { count },
                "grid" => thumbnail::ThumbnailMode::Grid { rows, cols },
                "auto" => thumbnail::ThumbnailMode::Auto,
                _ => {
                    return Err(anyhow::anyhow!(
                        "Invalid mode. Use: single, multiple, grid, or auto"
                    ))
                }
            };

            let thumb_format = thumbnail::ThumbnailFormat::from_str(&format)?;

            let options = thumbnail::ThumbnailOptions {
                input,
                output,
                mode: thumb_mode,
                width,
                height,
                quality,
                format: thumb_format,
                json_output: cli.json,
            };
            thumbnail::generate_thumbnails(options).await
        }

        Commands::Sprite {
            input,
            output,
            interval,
            count,
            cols,
            rows,
            width,
            height,
            format,
            quality,
            compression,
            strategy,
            layout,
            spacing,
            margin,
            vtt,
            vtt_output,
            manifest,
            manifest_output,
            timestamps,
            aspect,
        } => {
            // Parse interval if provided
            let interval_secs = if let Some(ref interval_str) = interval {
                Some(sprite::parse_duration(interval_str)?)
            } else {
                None
            };

            // Parse format
            let img_format = sprite::ImageFormat::from_str(&format)?;

            // Parse sampling strategy
            let sampling_strategy = sprite::SamplingStrategy::from_str(&strategy)?;

            // Parse layout mode
            let layout_mode = sprite::LayoutMode::from_str(&layout)?;

            // Create sprite sheet configuration
            let mut config = sprite::SpriteSheetConfig {
                interval: interval_secs,
                count,
                thumbnail_width: width,
                thumbnail_height: height,
                columns: cols,
                rows,
                format: img_format,
                quality,
                strategy: sampling_strategy,
                layout: layout_mode,
                spacing,
                margin,
                maintain_aspect_ratio: aspect,
                compression,
            };

            // Validate and adjust configuration
            config = sprite::validate_and_adjust_config(config)?;

            // Create options
            let options = sprite::SpriteSheetOptions {
                input,
                output,
                config,
                generate_vtt: vtt,
                vtt_output,
                generate_manifest: manifest,
                manifest_output,
                show_timestamps: timestamps,
                json_output: cli.json,
            };

            sprite::generate_sprite_sheet(options).await
        }

        Commands::Metadata {
            input,
            output,
            show,
            set,
            remove,
            clear,
            copy_from,
        } => {
            use std::collections::HashMap;

            let operation = if show {
                metadata::MetadataOperation::Show
            } else if !set.is_empty() {
                let mut fields = HashMap::new();
                for (key, value) in set {
                    fields.insert(key, value);
                }
                metadata::MetadataOperation::Set { fields }
            } else if !remove.is_empty() {
                metadata::MetadataOperation::Remove { fields: remove }
            } else if clear {
                metadata::MetadataOperation::Clear
            } else if let Some(source) = copy_from {
                metadata::MetadataOperation::Copy { source }
            } else {
                // Default to show if no operation specified
                metadata::MetadataOperation::Show
            };

            let options = metadata::MetadataOptions {
                input,
                output,
                operation,
                json_output: cli.json,
            };
            metadata::manage_metadata(options).await
        }

        Commands::Benchmark {
            input,
            codecs,
            presets,
            duration,
            iterations,
            output_dir,
        } => {
            let options = benchmark::BenchmarkOptions {
                input,
                codecs,
                presets,
                duration,
                iterations,
                output_dir,
                json_output: cli.json,
            };
            benchmark::run_benchmark(options).await
        }

        Commands::Validate {
            inputs,
            checks,
            strict,
            fix,
            loudness_check,
            gamut_check,
        } => {
            let validation_checks: Result<Vec<validate::ValidationCheck>> = checks
                .iter()
                .map(|s| validate::ValidationCheck::from_str(s))
                .collect();

            let options = validate::ValidateOptions {
                inputs,
                checks: validation_checks?,
                strict,
                fix,
                loudness_check,
                gamut_check,
                json_output: cli.json,
            };
            validate::validate_files(options).await
        }

        Commands::Analyze {
            input,
            reference,
            metrics,
            output_format,
            per_frame,
            summary,
        } => {
            let options = analyze::AnalyzeOptions {
                input,
                reference,
                metrics,
                output_format,
                per_frame,
                summary,
                json_output: cli.json,
            };
            analyze::analyze_quality(options).await
        }

        Commands::Scene { command } => scene::handle_scene_command(command, cli.json).await,

        Commands::Scopes { command } => scopes_cmd::handle_scopes_command(command, cli.json).await,

        Commands::Audio { command } => audio_cmd::handle_audio_command(command, cli.json).await,

        Commands::Subtitle { command } => {
            subtitle_cmd::handle_subtitle_command(command, cli.json).await
        }

        Commands::Filter { command } => filter_cmd::handle_filter_command(command, cli.json).await,

        Commands::Lut { command } => lut_cmd::handle_lut_command(command, cli.json).await,

        Commands::Denoise {
            input,
            output,
            mode,
            strength,
            spatial,
            temporal,
            preserve_grain,
        } => {
            let opts = denoise_cmd::DenoiseOptions {
                input,
                output,
                mode,
                strength,
                spatial,
                temporal,
                preserve_grain,
            };
            denoise_cmd::run_denoise(opts, cli.json).await
        }

        Commands::Stabilize {
            input,
            output,
            mode,
            quality,
            smoothing,
            zoom,
        } => {
            let opts = stabilize_cmd::StabilizeOptions {
                input,
                output,
                mode,
                quality,
                smoothing,
                zoom,
            };
            stabilize_cmd::run_stabilize(opts, cli.json).await
        }

        Commands::Edl { command } => edl_cmd::handle_edl_command(command, cli.json).await,

        Commands::Package {
            input,
            output,
            format,
            segments,
            ladders,
            encrypt,
            low_latency,
        } => {
            let opts = package_cmd::PackageOptions {
                input,
                output,
                format,
                segments,
                ladders,
                encrypt,
                low_latency,
            };
            package_cmd::run_package(opts, cli.json).await
        }

        Commands::Forensics {
            input,
            all,
            tests,
            output_format,
            report,
        } => {
            let opts = forensics_cmd::ForensicsOptions {
                input,
                all,
                tests,
                output_format,
                report,
            };
            forensics_cmd::run_forensics(opts, cli.json).await
        }

        Commands::Monitor { command } => handle_monitor_command(command, cli.json).await,
        Commands::Restore { command } => handle_restore_command(command, cli.json).await,
        Commands::Captions { command } => handle_captions_command(command, cli.json).await,

        Commands::Preset { command } => handle_preset_command(command, cli.json).await,

        Commands::Stream { command } => stream_cmd::run_stream(command, cli.json).await,
        Commands::Search { command } => search_cmd::run_search(command, cli.json).await,
        Commands::Timecode { command } => timecode_cmd::run_timecode(command, cli.json).await,
        Commands::Repair { command } => repair_cmd::run_repair(command, cli.json).await,
        Commands::Color { command } => color_cmd::run_color(command, cli.json).await,
        Commands::Playlist { command } => {
            playlist_cmd::handle_playlist_command(command, cli.json).await
        }
        Commands::Conform { command } => {
            conform_cmd::handle_conform_command(command, cli.json).await
        }
        Commands::Archive { command } => {
            archive_cmd::handle_archive_command(command, cli.json).await
        }
        Commands::Watermark { command } => {
            watermark_cmd::handle_watermark_command(command, cli.json).await
        }
        Commands::Image { command } => image_cmd::handle_image_command(command, cli.json).await,
        Commands::Graphics { command } => {
            graphics_cmd::handle_graphics_command(command, cli.json).await
        }
        Commands::Multicam { command } => {
            multicam_cmd::handle_multicam_command(command, cli.json).await
        }
        Commands::Timeline { command } => {
            timeline_cmd::handle_timeline_command(command, cli.json).await
        }
        Commands::Vfx { command } => vfx_cmd::handle_vfx_command(command, cli.json).await,
        Commands::Ffcompat { args } => ffcompat_cmd::run(args).await,
        Commands::Tui => {
            tui_cmd::run_tui()?;
            Ok(())
        }
        Commands::Optimize { command } => {
            optimize_cmd::handle_optimize_command(command, cli.json).await
        }
        Commands::Mixer { command } => mixer_cmd::handle_mixer_command(command, cli.json).await,
        Commands::Audiopost { command } => {
            audiopost_cmd::handle_audiopost_command(command, cli.json).await
        }
        Commands::Distributed { command } => {
            distributed_cmd::handle_distributed_command(command, cli.json).await
        }
        Commands::Farm { command } => farm_cmd::handle_farm_command(command, cli.json).await,
        Commands::Ndi { command } => ndi_cmd::handle_ndi_command(command, cli.json).await,
        Commands::Videoip { command } => {
            videoip_cmd::handle_videoip_command(command, cli.json).await
        }
        Commands::Gaming { command } => gaming_cmd::handle_gaming_command(command, cli.json).await,
        Commands::Mam { command } => mam_cmd::handle_mam_command(command, cli.json).await,
        Commands::Cloud { command } => cloud_cmd::handle_cloud_command(command, cli.json).await,
        Commands::Plugin { command } => plugin_cmd::handle_plugin_command(command, cli.json).await,
        Commands::Mir { command } => mir_cmd::handle_mir_command(command, cli.json).await,
        Commands::Qc { command } => qc_cmd::handle_qc_command(command, cli.json).await,
        Commands::Imf { command } => imf_cmd::handle_imf_command(command, cli.json).await,
        Commands::Aaf { command } => aaf_cmd::handle_aaf_command(command, cli.json).await,
        Commands::Playout { command } => {
            playout_cmd::handle_playout_command(command, cli.json).await
        }
        Commands::Switcher { command } => {
            switcher_cmd::handle_switcher_command(command, cli.json).await
        }
        Commands::Workflow { command } => {
            workflow_cmd::handle_workflow_command(command, cli.json).await
        }
        Commands::Collab { command } => collab_cmd::handle_collab_command(command, cli.json).await,
        Commands::Proxy { command } => proxy_cmd::handle_proxy_command(command, cli.json).await,
        Commands::Clips { command } => clips_cmd::handle_clips_command(command, cli.json).await,
        Commands::Review { command } => review_cmd::handle_review_command(command, cli.json).await,
        Commands::Drm { command } => drm_cmd::handle_drm_command(command, cli.json).await,
        Commands::Dedup { command } => dedup_cmd::handle_dedup_command(command, cli.json).await,
        Commands::ArchivePro { command } => {
            archivepro_cmd::handle_archivepro_command(command, cli.json).await
        }
        Commands::DolbyVision { command } => {
            dolbyvision_cmd::handle_dolbyvision_command(command, cli.json).await
        }
        Commands::TimeSync { command } => {
            timesync_cmd::handle_timesync_command(command, cli.json).await
        }
        Commands::Align { command } => align_cmd::handle_align_command(command, cli.json).await,
        Commands::Routing { command } => {
            routing_cmd::handle_routing_command(command, cli.json).await
        }
        Commands::Calibrate { command } => {
            calibrate_cmd::handle_calibrate_command(command, cli.json).await
        }
        Commands::Virtual { command } => {
            virtual_cmd::handle_virtual_command(command, cli.json).await
        }
        Commands::Profiler { command } => {
            profiler_cmd::handle_profiler_command(command, cli.json).await
        }
        Commands::Recommend { command } => {
            recommend_cmd::handle_recommend_command(command, cli.json).await
        }
        Commands::Scaling { command } => {
            scaling_cmd::handle_scaling_command(command, cli.json).await
        }
        Commands::Renderfarm { command } => {
            renderfarm_cmd::handle_renderfarm_command(command, cli.json).await
        }
        Commands::Access { command } => access_cmd::handle_access_command(command, cli.json).await,
        Commands::Rights { command } => rights_cmd::handle_rights_command(command, cli.json).await,
        Commands::Auto { command } => auto_cmd::handle_auto_command(command, cli.json).await,
        Commands::Loudness { command } => {
            loudness_cmd::run_loudness(command, cli.json, cli.ndjson).await
        }
        Commands::Quality { command } => {
            quality_cmd::run_quality(command, cli.json, cli.ndjson).await
        }
        Commands::Normalize { command } => normalize_cmd::run_normalize(command, cli.json).await,
        Commands::BatchEngine { command } => batch_cmd::run_batch_engine(command, cli.json).await,
        Commands::Ml { command } => ml_cmd::run_ml(command, cli.json).await,
        Commands::Completions { shell } => completions_cmd::run(shell),
        Commands::ManPage => man_cmd::run(),
        Commands::Doctor { json, full } => doctor_cmd::run(json, full),
    };

    result
}

/// Main entry point for the OxiMedia CLI.
#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Install the Pure-Rust `rustls-rustcrypto` crypto provider as the
    // process-wide default before any subcommand can open a TLS connection
    // (cloud storage, distributed rendering, monitoring exporters, ...). The
    // workspace builds `reqwest`/`rustls` without a compiled-in default
    // provider (`rustls-no-provider`) to keep the default build Pure Rust,
    // so this must run first or the first TLS use would panic. Idempotent.
    oximedia_net::install_default_crypto_provider();

    let cli = Cli::parse();

    // Disable colours before anything else — must run before any Colorize use.
    // JSON / NDJSON output also forces no-colour for clean machine-readable output.
    init_color(cli.no_color || cli.json || cli.ndjson);

    // Record --quiet globally so status/banner printers (transcode plan,
    // extract plan, image status output) can suppress themselves. Command
    // results and machine-readable output are never suppressed.
    progress::set_quiet(cli.quiet);

    // Initialize logging before dispatching subcommands
    if let Err(e) = init_logging(cli.verbose, cli.quiet, cli.log_format) {
        eprintln!("error: failed to initialise logging: {:#}", e);
        return exit_codes::OxiExitCode::GenericError.into();
    }

    // Dispatch and map errors to exit codes
    match run(cli).await {
        Ok(()) => exit_codes::OxiExitCode::Ok.into(),
        Err(ref e) => {
            eprintln!("{} {:#}", "Error:".red().bold(), e);
            if let Some(source) = e.source() {
                eprintln!("{} {}", "Caused by:".yellow(), source);
            }
            exit_codes::classify_error(e).into()
        }
    }
}
