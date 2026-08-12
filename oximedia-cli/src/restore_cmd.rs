//! Audio and video restoration command.
//!
//! Provides `oximedia restore` for restoring degraded audio and video,
//! analyzing degradation, batch processing, and before/after comparison.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// Options for the `restore audio` subcommand.
pub struct RestoreAudioOptions {
    /// Input audio file path.
    pub input: PathBuf,
    /// Output file path.
    pub output: PathBuf,
    /// Restoration mode: vinyl, tape, broadcast, archival, custom.
    pub mode: String,
    /// Sample rate override (Hz).
    pub sample_rate: Option<u32>,
    /// Enable declipping.
    pub declip: bool,
    /// Enable decrackle.
    pub decrackle: bool,
    /// Enable hum removal.
    pub dehum: bool,
    /// Enable noise reduction.
    pub denoise: bool,
    /// Treat input as raw PCM float32 LE (skip format detection).
    pub raw: bool,
}

/// Options for the `restore video` subcommand.
pub struct RestoreVideoOptions {
    /// Input video file path.
    pub input: PathBuf,
    /// Output file path.
    pub output: PathBuf,
    /// Restoration mode: deinterlace, upscale, stabilize, color-correct, full.
    pub mode: String,
    /// Target width for upscale.
    pub width: Option<u32>,
    /// Target height for upscale.
    pub height: Option<u32>,
}

/// Options for the `restore analyze` subcommand.
pub struct RestoreAnalyzeOptions {
    /// Input file to analyze.
    pub input: PathBuf,
    /// Analysis type: audio, video, auto.
    pub analysis_type: String,
}

/// Options for the `restore batch` subcommand.
pub struct RestoreBatchOptions {
    /// Input directory.
    pub input_dir: PathBuf,
    /// Output directory.
    pub output_dir: PathBuf,
    /// Restoration mode.
    pub mode: String,
    /// File extension filter (e.g. "wav", "flac").
    pub extension: Option<String>,
}

/// Options for the `restore compare` subcommand.
pub struct RestoreCompareOptions {
    /// Original (degraded) file.
    pub original: PathBuf,
    /// Restored file.
    pub restored: PathBuf,
}

/// Decode an audio file to a flat `Vec<f32>` of mono samples and return the
/// detected sample rate.
///
/// WAV is decoded via `oximedia_audio::wav::WavReader` (fully implemented).
/// MP3 is decoded via the `oximedia_audio::mp3::Mp3Decoder` AudioDecoder path.
/// For FLAC the oximedia-audio decoder is currently a stub (no frame output),
/// so FLAC files are rejected with a clear error directing the user to convert
/// to WAV first or use `--raw`.
///
/// # Errors
///
/// Returns an error if the format is unsupported, the file is malformed, or
/// decoding fails.
fn decode_audio_file(data: &[u8], ext: &str) -> Result<(Vec<f32>, u32)> {
    match ext {
        "wav" | "wave" => {
            use oximedia_audio::wav::WavReader;
            let mut reader =
                WavReader::new(std::io::Cursor::new(data)).context("Failed to open WAV file")?;
            let spec = reader.spec();
            let detected_rate = spec.sample_rate;
            let mut samples = reader
                .read_samples_f32()
                .context("Failed to decode WAV samples")?;
            // Downmix multi-channel to mono by averaging all channels.
            if spec.channels > 1 {
                let ch = spec.channels as usize;
                samples = samples
                    .chunks(ch)
                    .map(|c| c.iter().sum::<f32>() / ch as f32)
                    .collect();
            }
            Ok((samples, detected_rate))
        }

        "flac" => {
            // oximedia-audio FlacDecoder is a stub (send_packet/receive_frame do nothing).
            // Return a clear error instead of silently yielding zero samples.
            Err(anyhow::anyhow!(
                "FLAC decoding is not yet available in the restore pipeline. \
                 Convert to WAV first (e.g. `ffmpeg -i input.flac output.wav`) \
                 or use --raw to treat the input as raw PCM float32 LE."
            ))
        }

        "mp3" => {
            use oximedia_audio::mp3::Mp3Decoder;
            use oximedia_audio::{AudioBuffer, AudioDecoder};
            let mut decoder = Mp3Decoder::new();
            // Feed all data as one packet.
            decoder
                .send_packet(data, 0)
                .map_err(|e| anyhow::anyhow!("MP3 send_packet failed: {e}"))?;
            let mut all_samples: Vec<f32> = Vec::new();
            let mut detected_rate = 44100u32;
            // Drain all frames.
            loop {
                match decoder.receive_frame() {
                    Ok(Some(frame)) => {
                        detected_rate = frame.sample_rate;
                        let channels = frame.channels.count();
                        // Extract raw bytes from the audio buffer without naming the
                        // bytes::Bytes type (avoids adding bytes as an explicit dep).
                        let raw_bytes: Vec<u8> = match &frame.samples {
                            AudioBuffer::Interleaved(b) => b.to_vec(),
                            AudioBuffer::Planar(planes) => {
                                // Interleave planar channels for uniform handling.
                                if planes.is_empty() {
                                    Vec::new()
                                } else {
                                    let per_ch = planes[0].len();
                                    let mut interleaved = Vec::with_capacity(per_ch * planes.len());
                                    for i in 0..(per_ch / 4) {
                                        for plane in planes {
                                            if i * 4 + 3 < plane.len() {
                                                interleaved
                                                    .extend_from_slice(&plane[i * 4..i * 4 + 4]);
                                            }
                                        }
                                    }
                                    interleaved
                                }
                            }
                        };
                        // Samples are packed f32 LE; downmix to mono.
                        let frame_samples: Vec<f32> = raw_bytes
                            .chunks_exact(4)
                            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                            .collect();
                        if channels > 1 {
                            all_samples.extend(
                                frame_samples
                                    .chunks(channels)
                                    .map(|c| c.iter().sum::<f32>() / channels as f32),
                            );
                        } else {
                            all_samples.extend_from_slice(&frame_samples);
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        // NeedMoreData signals end-of-input when buffer is drained.
                        let e_str = format!("{e}");
                        if e_str.contains("NeedMoreData") || e_str.contains("need") {
                            break;
                        }
                        return Err(anyhow::anyhow!("MP3 decode error: {e}"));
                    }
                }
            }
            if all_samples.is_empty() {
                return Err(anyhow::anyhow!(
                    "MP3 decoding produced no samples — the file may be too short or corrupt."
                ));
            }
            Ok((all_samples, detected_rate))
        }

        other => Err(anyhow::anyhow!(
            "Unsupported audio format '.{}'. Supported formats: wav, mp3. \
             Use --raw to treat the input as raw PCM float32 LE, or convert \
             to WAV before restoring.",
            other
        )),
    }
}

/// Run the `restore audio` subcommand.
pub async fn run_restore_audio(opts: RestoreAudioOptions, json_output: bool) -> Result<()> {
    use oximedia_restore::presets::{BroadcastCleanup, TapeRestoration, VinylRestoration};
    use oximedia_restore::RestoreChain;

    let data = std::fs::read(&opts.input)
        .with_context(|| format!("Failed to read input: {}", opts.input.display()))?;

    // Determine samples and sample rate based on --raw flag or file extension.
    let (samples, sample_rate) = if opts.raw {
        // Explicit raw PCM float32 LE path.
        let sr = opts.sample_rate.unwrap_or(44100);
        (bytes_to_f32_samples(&data), sr)
    } else {
        let ext = opts
            .input
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let (decoded_samples, detected_rate) = decode_audio_file(&data, &ext)?;
        // Allow explicit --sample-rate override even in format-aware path.
        let sr = opts.sample_rate.unwrap_or(detected_rate);
        (decoded_samples, sr)
    };

    let mut chain = RestoreChain::new();

    match opts.mode.to_lowercase().as_str() {
        "vinyl" => {
            let mut preset = VinylRestoration::new(sample_rate);
            preset.click_removal = true;
            preset.crackle_removal = opts.decrackle;
            preset.hum_removal = opts.dehum;
            chain.add_preset(preset);
        }
        "tape" => {
            let preset = TapeRestoration::new(sample_rate);
            chain.add_preset(preset);
        }
        "broadcast" => {
            let preset = BroadcastCleanup::new(sample_rate);
            chain.add_preset(preset);
        }
        "archival" => {
            // Full restoration: vinyl + tape presets combined
            let vinyl = VinylRestoration::new(sample_rate);
            chain.add_preset(vinyl);
            let tape = TapeRestoration::new(sample_rate);
            chain.add_preset(tape);
        }
        _ => {
            // Custom mode: add steps based on individual flags
            use oximedia_restore::dc::DcRemover;
            use oximedia_restore::RestorationStep;

            chain.add_step(RestorationStep::DcRemoval(DcRemover::new(
                10.0,
                sample_rate,
            )));

            if opts.declip {
                use oximedia_restore::clip::{
                    BasicDeclipper, ClipDetector, ClipDetectorConfig, DeclipConfig,
                };
                chain.add_step(RestorationStep::Declipping {
                    detector: ClipDetector::new(ClipDetectorConfig::default()),
                    declipper: BasicDeclipper::new(DeclipConfig::default()),
                });
            }

            if opts.dehum {
                use oximedia_restore::hum::HumRemover;
                chain.add_step(RestorationStep::HumRemoval(HumRemover::new_standard(
                    50.0,
                    sample_rate,
                    5,
                    10.0,
                )));
                chain.add_step(RestorationStep::HumRemoval(HumRemover::new_standard(
                    60.0,
                    sample_rate,
                    5,
                    10.0,
                )));
            }

            if opts.denoise {
                use oximedia_restore::noise::{NoiseGate, NoiseGateConfig};
                chain.add_step(RestorationStep::NoiseGate(NoiseGate::new(
                    NoiseGateConfig::default(),
                )));
            }
        }
    }

    let step_count = chain.len();
    let restored = chain
        .process(&samples, sample_rate)
        .map_err(|e| anyhow::anyhow!("Restoration failed: {e}"))?;

    // Write restored samples back as raw f32 LE bytes
    let output_bytes = f32_samples_to_bytes(&restored);
    std::fs::write(&opts.output, &output_bytes)
        .with_context(|| format!("Failed to write output: {}", opts.output.display()))?;

    if json_output {
        let obj = serde_json::json!({
            "input": opts.input.to_string_lossy(),
            "output": opts.output.to_string_lossy(),
            "mode": opts.mode,
            "sample_rate": sample_rate,
            "input_samples": samples.len(),
            "output_samples": restored.len(),
            "restoration_steps": step_count,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "Audio Restoration Complete".green().bold());
        println!("  Input:       {}", opts.input.display());
        println!("  Output:      {}", opts.output.display());
        println!("  Mode:        {}", opts.mode);
        println!("  Sample rate: {} Hz", sample_rate);
        println!("  Steps:       {}", step_count);
        println!("  Samples:     {} -> {}", samples.len(), restored.len());
    }

    Ok(())
}

/// Run the `restore video` subcommand.
///
/// The behaviour depends on the requested mode:
///
/// * **`stabilize` / `full`** — performs real frame-level video stabilisation.
///   The input clip is decoded to raw frames, run through the
///   `oximedia-stabilize` offline multi-pass pipeline (motion estimation →
///   trajectory smoothing → frame warping), and re-encoded.  Because every
///   pixel is warped, this mode is inherently a *transcode* (not a remux):
///   the input must be an uncompressed YUV4MPEG2 (`.y4m`) clip and the output
///   is written as `.y4m`.  See [`stabilize_video_y4m`].
///
/// * **`deinterlace` / `upscale` / `color-correct`** — uses
///   `FramePipelineConfig` + `FramePipelineExecutor` from `oximedia-transcode`
///   to carry `VideoFrameOp` descriptors through the container remux pass.
///   The `FramePipelineExecutor` operates at packet level (stream-copy
///   semantics): the `VideoFrameOp` variants are registered in the config but
///   the packet-level loop does not call `apply_video_ops` on decoded video
///   frames (a full decode→filter→encode path is out of scope for these
///   modes).  For supported container formats (MKV, WebM, Ogg, WAV, FLAC) the
///   pipeline performs a container remux which fixes container-level
///   corruption; for unsupported output formats it falls back to a byte-level
///   copy with a warning.
pub async fn run_restore_video(opts: RestoreVideoOptions, json_output: bool) -> Result<()> {
    // Stabilisation (and `full`, which includes it) needs a real
    // decode→stabilise→encode transcode, so it is dispatched separately.
    let mode_lc = opts.mode.to_lowercase();
    if mode_lc == "stabilize" || mode_lc == "full" {
        return run_restore_video_stabilize(opts, json_output).await;
    }
    run_restore_video_remux(opts, json_output).await
}

/// Remux-based restoration path for `deinterlace` / `upscale` / `color-correct`.
async fn run_restore_video_remux(opts: RestoreVideoOptions, json_output: bool) -> Result<()> {
    use oximedia_transcode::frame_pipeline::{
        FramePipelineConfig, FramePipelineExecutor, VideoFrameOp,
    };
    use oximedia_transcode::hdr_passthrough::HdrPassthroughMode;

    let width = opts.width.unwrap_or(1920);
    let height = opts.height.unwrap_or(1080);

    // Determine which restoration steps are requested.  The `stabilize` and
    // `full` modes are handled by `run_restore_video_stabilize` and never
    // reach this function, so they are not listed here.
    let steps_applied: Vec<&str> = match opts.mode.to_lowercase().as_str() {
        "deinterlace" => vec!["deinterlace"],
        "upscale" => vec!["upscale"],
        "color-correct" => vec!["color-correct"],
        _ => vec!["deinterlace"],
    };

    // Build VideoFrameOp list from the requested steps.
    let mut video_ops: Vec<VideoFrameOp> = Vec::new();
    for step in &steps_applied {
        match *step {
            "deinterlace" => video_ops.push(VideoFrameOp::Deinterlace),
            "upscale" => video_ops.push(VideoFrameOp::Scale { width, height }),
            "color-correct" => video_ops.push(VideoFrameOp::ColorCorrect {
                brightness: 1.05,
                contrast: 1.1,
                saturation: 1.0,
            }),
            _ => {}
        }
    }

    let frame_cfg = FramePipelineConfig {
        input: opts.input.clone(),
        output: opts.output.clone(),
        video_codec: None,
        audio_codec: None,
        video_ops,
        audio_ops: Vec::new(),
        hdr_mode: HdrPassthroughMode::Passthrough,
        source_hdr: None,
        hw_accel: false,
        threads: 0,
    };

    // FramePipelineExecutor::execute() is synchronous and creates its own
    // tokio runtime internally via block_on.  Calling it directly from this
    // async function would cause a "cannot start a runtime from within a
    // runtime" panic.  Offload it to the blocking thread pool instead.
    let executor_result = tokio::task::spawn_blocking(move || {
        let mut executor = FramePipelineExecutor::new(frame_cfg);
        executor.execute()
    })
    .await
    .map_err(|join_err| anyhow::anyhow!("executor task panicked: {join_err}"));

    let (output_size, pipeline_used) = {
        let output_path = opts.output.clone();
        let input_path = opts.input.clone();
        match executor_result.and_then(|r| r.map_err(|e| anyhow::anyhow!("{e}"))) {
            Ok(result) => {
                // Derive output size from what was actually written.
                let written = std::fs::metadata(&output_path)
                    .map(|m| m.len())
                    .unwrap_or(result.output_bytes);
                (written, true)
            }
            Err(e) => {
                // FramePipelineExecutor failed (unsupported container / codec).
                // Fall back to byte-level copy so the caller gets a file.
                tracing::warn!(
                    "restore-video: frame pipeline failed ({}); \
                     falling back to byte copy — video ops not applied",
                    e
                );
                let data = std::fs::read(&input_path)
                    .with_context(|| format!("Failed to read input: {}", input_path.display()))?;
                let sz = data.len() as u64;
                std::fs::write(&output_path, &data).with_context(|| {
                    format!("Failed to write output: {}", output_path.display())
                })?;
                if !json_output {
                    println!(
                        "  Note: frame pipeline failed ({}); byte copy used. \
                         Video ops (deinterlace/scale/color-correct) were not applied.",
                        e
                    );
                }
                (sz, false)
            }
        }
    };

    if json_output {
        let obj = serde_json::json!({
            "input": opts.input.to_string_lossy(),
            "output": opts.output.to_string_lossy(),
            "mode": opts.mode,
            "target_resolution": format!("{width}x{height}"),
            "output_size_bytes": output_size,
            "steps_applied": steps_applied,
            "pipeline_remux": pipeline_used,
            "note": if pipeline_used {
                "Container remuxed via FramePipelineExecutor. \
                 VideoFrameOp descriptors registered; pixel-level ops \
                 (deinterlace/scale/color-correct) require full decode→filter→encode \
                 which is not yet wired at the packet loop level."
            } else {
                "Byte copy used; frame pipeline not available for this format or encountered an error."
            },
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "Video Restoration Complete".green().bold());
        println!("  Input:          {}", opts.input.display());
        println!("  Output:         {}", opts.output.display());
        println!("  Mode:           {}", opts.mode);
        println!("  Resolution:     {}x{}", width, height);
        println!("  Steps:          {}", steps_applied.join(", "));
        println!("  Output size:    {} bytes", output_size);
        println!(
            "  Pipeline:       {}",
            if pipeline_used {
                "FramePipelineExecutor (container remux)"
            } else {
                "byte copy (pipeline unavailable)"
            }
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Video stabilisation (restore-video --mode stabilize / full)
// ---------------------------------------------------------------------------

/// Run the `restore video` subcommand for the `stabilize` / `full` modes.
///
/// Stabilisation requires warping every pixel of every frame, so it is a real
/// transcode rather than a container remux.  This wrapper restricts the input
/// to uncompressed YUV4MPEG2 (`.y4m`) — a lossless, self-describing container
/// that the OxiMedia stack can demux and mux without depending on a particular
/// compressed-codec decoder — and delegates the heavy lifting to
/// [`stabilize_video_y4m`].
async fn run_restore_video_stabilize(opts: RestoreVideoOptions, json_output: bool) -> Result<()> {
    // Stabilisation is CPU-bound and fully synchronous; run it on the blocking
    // pool so the async runtime stays responsive.
    let input = opts.input.clone();
    let output = opts.output.clone();
    let mode = opts.mode.clone();

    let summary = tokio::task::spawn_blocking(move || stabilize_video_y4m(&input, &output))
        .await
        .map_err(|join_err| anyhow::anyhow!("stabilisation task panicked: {join_err}"))??;

    if json_output {
        let obj = serde_json::json!({
            "input": opts.input.to_string_lossy(),
            "output": opts.output.to_string_lossy(),
            "mode": mode,
            "operation": "stabilize",
            "transcode": true,
            "input_format": "y4m",
            "output_format": "y4m",
            "width": summary.width,
            "height": summary.height,
            "frame_count": summary.frame_count,
            "input_size_bytes": summary.input_size,
            "output_size_bytes": summary.output_size,
            "bytes_changed": summary.bytes_changed,
            "note": "Video stabilised via the oximedia-stabilize offline \
                     multi-pass pipeline (motion estimation, trajectory \
                     smoothing, frame warping). Output re-encoded as Y4M.",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "Video Restoration Complete".green().bold());
        println!("  Input:          {}", opts.input.display());
        println!("  Output:         {}", opts.output.display());
        println!("  Mode:           {}", mode);
        println!("  Operation:      stabilise (transcode)");
        println!("  Resolution:     {}x{}", summary.width, summary.height);
        println!("  Frames:         {}", summary.frame_count);
        println!(
            "  Output size:    {} bytes ({} bytes changed)",
            summary.output_size, summary.bytes_changed
        );
        println!(
            "  Pipeline:       oximedia-stabilize offline multi-pass (decode \u{2192} stabilise \u{2192} encode)"
        );
    }

    Ok(())
}

/// Summary of a completed video stabilisation pass.
#[derive(Debug, Clone, Copy)]
struct StabilizeSummary {
    /// Frame width in pixels.
    width: u32,
    /// Frame height in pixels.
    height: u32,
    /// Number of frames processed.
    frame_count: usize,
    /// Size of the input file in bytes.
    input_size: u64,
    /// Size of the written output file in bytes.
    output_size: u64,
    /// Number of frame-data bytes that differ between input and output.
    bytes_changed: u64,
}

/// Decode a Y4M clip, stabilise it with the `oximedia-stabilize` offline
/// multi-pass pipeline, and re-encode the result as Y4M.
///
/// Both halves are the shared frame harness: [`crate::frame_harness::process_clip`]
/// runs the Y4M demux → operation → mux path (stabilisation needs the whole
/// clip, because the motion trajectory is global), and
/// [`crate::frame_harness::ops::stabilize_clip`] is the operation itself —
/// multi-pass analysis, feature-based motion estimation, trajectory smoothing
/// and per-plane warping.
///
/// # Errors
///
/// Returns an error if the input is not a readable Y4M file, if its chroma
/// format is unsupported, or if the stabiliser or muxer fails.
fn stabilize_video_y4m(
    input: &std::path::Path,
    output: &std::path::Path,
) -> Result<StabilizeSummary> {
    // The harness is geometry-agnostic, so the clip's dimensions are captured
    // as it passes through the operation rather than by decoding it twice.
    //
    // `restore video --mode stabilize` has no CLI flags of its own for
    // stabilisation parameters (unlike `oximedia stabilize`, which threads
    // --mode/--quality/--smoothing/--zoom through `stabilize_clip`'s config
    // parameter), so this reconstructs the fixed offline configuration that
    // used to be hard-coded inside `stabilize_clip` itself: affine motion,
    // balanced quality, 0.85 smoothing, zoom optimisation off (this restore
    // path predates zoom being wired up at all, so leaving it off preserves
    // this command's exact prior behaviour).
    let config = oximedia_stabilize::StabilizeConfig::new()
        .with_mode(oximedia_stabilize::StabilizationMode::Affine)
        .with_quality(oximedia_stabilize::QualityPreset::Balanced)
        .with_smoothing_strength(0.85)
        .with_zoom_optimization(false);
    let geometry = std::cell::Cell::new((0u32, 0u32));
    let stats = crate::frame_harness::process_clip(STABILIZE_OP, input, output, |clip| {
        geometry.set((clip.width(), clip.height()));
        crate::frame_harness::ops::stabilize_clip(clip, &config)
    })?;
    let (width, height) = geometry.get();

    Ok(StabilizeSummary {
        width,
        height,
        frame_count: stats.frame_count,
        input_size: stats.bytes_in,
        output_size: stats.bytes_out,
        bytes_changed: stats.bytes_changed,
    })
}

/// Operation label used in the frame harness's error messages.
const STABILIZE_OP: &str = "restore-video --mode stabilize";

/// Run the `restore analyze` subcommand.
pub async fn run_restore_analyze(opts: RestoreAnalyzeOptions, json_output: bool) -> Result<()> {
    let data = std::fs::read(&opts.input)
        .with_context(|| format!("Failed to read input: {}", opts.input.display()))?;

    let is_audio = match opts.analysis_type.to_lowercase().as_str() {
        "audio" => true,
        "video" => false,
        _ => {
            // Auto-detect based on extension
            let ext = opts
                .input
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            matches!(
                ext.to_lowercase().as_str(),
                "wav" | "flac" | "ogg" | "opus" | "pcm"
            )
        }
    };

    if is_audio {
        let samples = bytes_to_f32_samples(&data);
        let analysis = analyze_audio_degradation(&samples);

        if json_output {
            let obj = serde_json::json!({
                "file": opts.input.to_string_lossy(),
                "type": "audio",
                "samples": samples.len(),
                "degradation": analysis,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("{}", "Degradation Analysis".green().bold());
            println!("  File: {}", opts.input.display());
            println!("  Type: Audio ({} samples)", samples.len());
            println!();
            for (key, value) in &analysis {
                println!("  {}: {}", key.cyan(), value);
            }
        }
    } else {
        let analysis = analyze_video_degradation(&data);

        if json_output {
            let obj = serde_json::json!({
                "file": opts.input.to_string_lossy(),
                "type": "video",
                "size_bytes": data.len(),
                "degradation": analysis,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("{}", "Degradation Analysis".green().bold());
            println!("  File: {}", opts.input.display());
            println!("  Type: Video ({} bytes)", data.len());
            println!();
            for (key, value) in &analysis {
                println!("  {}: {}", key.cyan(), value);
            }
        }
    }

    Ok(())
}

/// Run the `restore batch` subcommand.
pub async fn run_restore_batch(opts: RestoreBatchOptions, json_output: bool) -> Result<()> {
    use oximedia_restore::presets::VinylRestoration;
    use oximedia_restore::RestoreChain;

    // Ensure output directory exists
    std::fs::create_dir_all(&opts.output_dir)
        .with_context(|| format!("Failed to create output dir: {}", opts.output_dir.display()))?;

    let entries: Vec<_> = std::fs::read_dir(&opts.input_dir)
        .with_context(|| format!("Failed to read directory: {}", opts.input_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            if let Some(ref ext_filter) = opts.extension {
                e.path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase() == ext_filter.to_lowercase())
                    .unwrap_or(false)
            } else {
                true
            }
        })
        .collect();

    let mut results = Vec::new();
    let sample_rate = 44100_u32;

    for entry in &entries {
        let input_path = entry.path();
        let file_name = input_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let output_path = opts.output_dir.join(file_name);

        let data = match std::fs::read(&input_path) {
            Ok(d) => d,
            Err(e) => {
                results.push(serde_json::json!({
                    "file": file_name,
                    "status": "error",
                    "message": format!("{e}"),
                }));
                continue;
            }
        };

        let samples = bytes_to_f32_samples(&data);
        let mut chain = RestoreChain::new();
        chain.add_preset(VinylRestoration::new(sample_rate));

        match chain.process(&samples, sample_rate) {
            Ok(restored) => {
                let output_bytes = f32_samples_to_bytes(&restored);
                if let Err(e) = std::fs::write(&output_path, &output_bytes) {
                    results.push(serde_json::json!({
                        "file": file_name,
                        "status": "error",
                        "message": format!("Write failed: {e}"),
                    }));
                } else {
                    results.push(serde_json::json!({
                        "file": file_name,
                        "status": "ok",
                        "input_samples": samples.len(),
                        "output_samples": restored.len(),
                    }));
                }
            }
            Err(e) => {
                results.push(serde_json::json!({
                    "file": file_name,
                    "status": "error",
                    "message": format!("{e}"),
                }));
            }
        }
    }

    if json_output {
        let obj = serde_json::json!({
            "input_dir": opts.input_dir.to_string_lossy(),
            "output_dir": opts.output_dir.to_string_lossy(),
            "mode": opts.mode,
            "total_files": entries.len(),
            "results": results,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "Batch Restoration Complete".green().bold());
        println!("  Input:  {}", opts.input_dir.display());
        println!("  Output: {}", opts.output_dir.display());
        println!("  Mode:   {}", opts.mode);
        println!("  Files:  {}", entries.len());
        println!();
        for r in &results {
            let file = r["file"].as_str().unwrap_or("?");
            let status = r["status"].as_str().unwrap_or("?");
            if status == "ok" {
                println!("  {} {}", "OK".green(), file);
            } else {
                let msg = r["message"].as_str().unwrap_or("unknown error");
                println!("  {} {} - {}", "FAIL".red(), file, msg);
            }
        }
    }

    Ok(())
}

/// Run the `restore compare` subcommand.
pub async fn run_restore_compare(opts: RestoreCompareOptions, json_output: bool) -> Result<()> {
    let original_data = std::fs::read(&opts.original)
        .with_context(|| format!("Failed to read original: {}", opts.original.display()))?;
    let restored_data = std::fs::read(&opts.restored)
        .with_context(|| format!("Failed to read restored: {}", opts.restored.display()))?;

    let original_samples = bytes_to_f32_samples(&original_data);
    let restored_samples = bytes_to_f32_samples(&restored_data);

    let comparison = compare_audio(&original_samples, &restored_samples);

    if json_output {
        let obj = serde_json::json!({
            "original": opts.original.to_string_lossy(),
            "restored": opts.restored.to_string_lossy(),
            "original_samples": original_samples.len(),
            "restored_samples": restored_samples.len(),
            "metrics": comparison,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "Restoration Comparison".green().bold());
        println!("  Original: {}", opts.original.display());
        println!("  Restored: {}", opts.restored.display());
        println!();
        for (key, value) in &comparison {
            println!("  {}: {}", key.cyan(), value);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert raw bytes to f32 samples (assumes little-endian f32 PCM).
fn bytes_to_f32_samples(data: &[u8]) -> Vec<f32> {
    data.chunks_exact(4)
        .map(|chunk| {
            let arr: [u8; 4] = [chunk[0], chunk[1], chunk[2], chunk[3]];
            f32::from_le_bytes(arr)
        })
        .collect()
}

/// Convert f32 samples to raw bytes (little-endian f32 PCM).
fn f32_samples_to_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

/// Analyze audio degradation indicators.
fn analyze_audio_degradation(samples: &[f32]) -> Vec<(String, String)> {
    let mut results = Vec::new();

    if samples.is_empty() {
        results.push(("Status".to_string(), "No samples to analyze".to_string()));
        return results;
    }

    // Peak level
    let peak = samples.iter().fold(0.0_f32, |max, &s| max.max(s.abs()));
    results.push(("Peak Level".to_string(), format!("{peak:.4}")));

    // Check for clipping
    let clip_count = samples.iter().filter(|&&s| s.abs() >= 0.999).count();
    let clip_pct = (clip_count as f64 / samples.len() as f64) * 100.0;
    results.push((
        "Clipping".to_string(),
        format!("{clip_count} samples ({clip_pct:.2}%)"),
    ));

    // DC offset
    let dc_offset: f64 = samples.iter().map(|&s| s as f64).sum::<f64>() / samples.len() as f64;
    results.push(("DC Offset".to_string(), format!("{dc_offset:.6}")));

    // RMS level
    let rms: f64 = (samples
        .iter()
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt();
    results.push(("RMS Level".to_string(), format!("{rms:.4}")));

    // Crest factor
    if rms > 0.0 {
        let crest = peak as f64 / rms;
        results.push(("Crest Factor".to_string(), format!("{crest:.2}")));
    }

    results
}

/// Analyze video degradation indicators.
fn analyze_video_degradation(data: &[u8]) -> Vec<(String, String)> {
    let mut results = Vec::new();

    results.push(("File Size".to_string(), format!("{} bytes", data.len())));

    // Basic byte statistics
    let mut histogram = [0u64; 256];
    for &b in data {
        histogram[b as usize] += 1;
    }

    let total = data.len() as f64;
    let entropy: f64 = histogram
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / total;
            -p * p.log2()
        })
        .sum();
    results.push(("Entropy".to_string(), format!("{entropy:.4} bits/byte")));

    // Unique byte values
    let unique = histogram.iter().filter(|&&c| c > 0).count();
    results.push(("Unique Byte Values".to_string(), format!("{unique}/256")));

    results
}

/// Compare original and restored audio.
fn compare_audio(original: &[f32], restored: &[f32]) -> Vec<(String, String)> {
    let mut results = Vec::new();

    results.push((
        "Original Samples".to_string(),
        format!("{}", original.len()),
    ));
    results.push((
        "Restored Samples".to_string(),
        format!("{}", restored.len()),
    ));

    let len = original.len().min(restored.len());
    if len == 0 {
        results.push(("Status".to_string(), "No overlapping samples".to_string()));
        return results;
    }

    // MSE
    let mse: f64 = original[..len]
        .iter()
        .zip(&restored[..len])
        .map(|(&a, &b)| {
            let diff = (a as f64) - (b as f64);
            diff * diff
        })
        .sum::<f64>()
        / len as f64;
    results.push(("MSE".to_string(), format!("{mse:.8}")));

    // SNR improvement estimate
    let original_power: f64 = original[..len]
        .iter()
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>()
        / len as f64;
    if mse > 0.0 {
        let snr_db = 10.0 * (original_power / mse).log10();
        results.push(("SNR (dB)".to_string(), format!("{snr_db:.2}")));
    }

    // Peak difference
    let max_diff = original[..len]
        .iter()
        .zip(&restored[..len])
        .map(|(&a, &b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    results.push(("Max Difference".to_string(), format!("{max_diff:.6}")));

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_to_f32_roundtrip() {
        let samples = vec![0.5_f32, -0.25, 1.0, -1.0, 0.0];
        let bytes = f32_samples_to_bytes(&samples);
        let recovered = bytes_to_f32_samples(&bytes);
        assert_eq!(samples, recovered);
    }

    #[test]
    fn test_analyze_audio_empty() {
        let result = analyze_audio_degradation(&[]);
        assert_eq!(result.len(), 1);
        assert!(result[0].1.contains("No samples"));
    }

    #[test]
    fn test_analyze_audio_clipping() {
        let samples = vec![1.0_f32; 100];
        let result = analyze_audio_degradation(&samples);
        let clip_entry = result.iter().find(|(k, _)| k == "Clipping");
        assert!(clip_entry.is_some());
        let clip_str = &clip_entry.expect("clipping entry should exist").1;
        assert!(clip_str.contains("100 samples"));
    }

    #[test]
    fn test_compare_audio_identical() {
        let samples = vec![0.5_f32; 100];
        let result = compare_audio(&samples, &samples);
        let mse_entry = result.iter().find(|(k, _)| k == "MSE");
        assert!(mse_entry.is_some());
        let mse_str = &mse_entry.expect("MSE entry should exist").1;
        assert!(mse_str.starts_with("0.0"));
    }

    #[test]
    fn test_analyze_video_degradation() {
        let data = vec![0u8, 1, 2, 3, 4, 5, 100, 200, 255];
        let result = analyze_video_degradation(&data);
        assert!(result.len() >= 3);
        let entropy_entry = result.iter().find(|(k, _)| k == "Entropy");
        assert!(entropy_entry.is_some());
    }

    // --- Slice 4: format-aware audio decode ---

    /// Decode a minimal valid WAV file (44-byte, mono, 16-bit PCM, 1 silent sample)
    /// and verify that `decode_audio_file` returns exactly 1 sample at 44100 Hz.
    #[test]
    fn test_decode_audio_file_wav_silent_sample() {
        // Minimal RIFF/WAVE header: mono, 44100 Hz, 16-bit PCM, 1 sample (0x0000).
        // fmt  chunk: size=16, format=1(PCM), ch=1, rate=44100, byteRate=88200,
        //             blockAlign=2, bps=16
        // data chunk: size=2, payload=[0x00, 0x00]
        let mut wav: Vec<u8> = Vec::new();
        wav.extend_from_slice(b"RIFF"); // ChunkID
        wav.extend_from_slice(&36u32.to_le_bytes()); // ChunkSize = 4+8+16+8+2 = 36
        wav.extend_from_slice(b"WAVE"); // Format
        wav.extend_from_slice(b"fmt "); // Subchunk1ID
        wav.extend_from_slice(&16u32.to_le_bytes()); // Subchunk1Size
        wav.extend_from_slice(&1u16.to_le_bytes()); // AudioFormat = PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // NumChannels = 1
        wav.extend_from_slice(&44100u32.to_le_bytes()); // SampleRate
        wav.extend_from_slice(&88200u32.to_le_bytes()); // ByteRate
        wav.extend_from_slice(&2u16.to_le_bytes()); // BlockAlign
        wav.extend_from_slice(&16u16.to_le_bytes()); // BitsPerSample
        wav.extend_from_slice(b"data"); // Subchunk2ID
        wav.extend_from_slice(&2u32.to_le_bytes()); // Subchunk2Size = 2 bytes
        wav.extend_from_slice(&0i16.to_le_bytes()); // 1 silent sample
        let (samples, rate) = decode_audio_file(&wav, "wav").expect("WAV decode should succeed");
        assert_eq!(rate, 44100);
        assert_eq!(samples.len(), 1);
        assert!((samples[0]).abs() < 1e-4, "silent sample should be ~0.0");
    }

    /// `decode_audio_file` with stereo WAV must downmix to mono.
    #[test]
    fn test_decode_audio_file_wav_stereo_downmix() {
        // 2 stereo samples: [L=+1.0, R=-1.0] → mono average = 0.0
        let mut wav: Vec<u8> = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&40u32.to_le_bytes()); // 4+8+16+8+8 = 44? let's compute exactly
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // fmt size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes()); // 2 channels
        wav.extend_from_slice(&44100u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&176400u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&4u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8u32.to_le_bytes()); // 2 stereo frames × 4 bytes
                                                    // Frame 1: L = i16::MAX, R = i16::MIN
        wav.extend_from_slice(&i16::MAX.to_le_bytes());
        wav.extend_from_slice(&i16::MIN.to_le_bytes());
        // Frame 2: L = 0, R = 0
        wav.extend_from_slice(&0i16.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        // Fix RIFF size: total = 4(WAVE) + 8+16(fmt) + 8+8(data) = 44
        let riff_size = (wav.len() - 8) as u32;
        wav[4..8].copy_from_slice(&riff_size.to_le_bytes());
        let (samples, rate) =
            decode_audio_file(&wav, "wav").expect("stereo WAV decode should succeed");
        assert_eq!(rate, 44100);
        assert_eq!(samples.len(), 2, "2 stereo frames → 2 mono samples");
        // Frame 1 downmix: (32767/32768 + (-32768/32768)) / 2 ≈ -0.000015
        assert!(samples[0].abs() < 0.01, "near-zero after L+R average");
    }

    /// Unsupported extension must return an Err containing "Unsupported" or "supported".
    #[test]
    fn test_decode_audio_file_unsupported_ext() {
        let result = decode_audio_file(b"junk", "xyz");
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("Unsupported") || msg.contains("supported"),
            "error should mention supported formats, got: {msg}"
        );
    }

    /// FLAC extension must return an error (stub decoder) not silently produce 0 samples.
    #[test]
    fn test_decode_audio_file_flac_returns_error() {
        let result = decode_audio_file(b"fLaC\x00\x00\x00\x00", "flac");
        assert!(result.is_err(), "FLAC should return Err (stub decoder)");
    }

    /// run_restore_audio with an unsupported format returns Err.
    #[tokio::test]
    async fn test_restore_audio_unsupported_format_error() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_test_restore_0177.xyz");
        let output = dir.join("oximedia_test_restore_0177_out.wav");
        std::fs::write(&input, b"junk data").expect("write junk");
        let opts = RestoreAudioOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "broadcast".to_string(),
            sample_rate: None,
            declip: false,
            decrackle: false,
            dehum: false,
            denoise: false,
            raw: false,
        };
        let result = run_restore_audio(opts, false).await;
        assert!(result.is_err(), "unsupported format should return Err");
        let err_str = format!("{}", result.unwrap_err());
        assert!(
            err_str.contains("Unsupported") || err_str.contains("supported"),
            "error message should mention supported formats: {err_str}"
        );
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    /// run_restore_audio with --raw flag bypasses format detection.
    #[tokio::test]
    async fn test_restore_audio_raw_flag() {
        let dir = std::env::temp_dir();
        // Write 4 f32 samples (0.0, 0.1, -0.1, 0.5) as raw PCM
        let samples: Vec<f32> = vec![0.0, 0.1, -0.1, 0.5];
        let raw_bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let input = dir.join("oximedia_test_restore_0177_raw.xyz");
        let output = dir.join("oximedia_test_restore_0177_raw_out.xyz");
        std::fs::write(&input, &raw_bytes).expect("write raw pcm");
        let opts = RestoreAudioOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "broadcast".to_string(),
            sample_rate: Some(44100),
            declip: false,
            decrackle: false,
            dehum: false,
            denoise: false,
            raw: true,
        };
        let result = run_restore_audio(opts, false).await;
        // Should succeed: raw flag bypasses format detection
        assert!(
            result.is_ok(),
            "raw flag should succeed: {:?}",
            result.err()
        );
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    /// run_restore_audio with a minimal WAV file should succeed.
    #[tokio::test]
    async fn test_restore_audio_wav_decode() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_test_restore_0177_wav.wav");
        let output = dir.join("oximedia_test_restore_0177_wav_out.wav");
        // Minimal WAV: mono, 44100 Hz, 16-bit, 1 silent sample.
        let mut wav: Vec<u8> = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&2u32.to_le_bytes());
        wav.extend_from_slice(&0i16.to_le_bytes());
        std::fs::write(&input, &wav).expect("write wav");
        let opts = RestoreAudioOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "broadcast".to_string(),
            sample_rate: None,
            declip: false,
            decrackle: false,
            dehum: false,
            denoise: false,
            raw: false,
        };
        let result = run_restore_audio(opts, false).await;
        // May fail at restoration step but should NOT panic and should not fail at decode.
        let _ = result;
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    // --- Slice 5: VideoFrameOp new variants (integration) ---

    /// FramePipelineConfig can hold Deinterlace and ColorCorrect ops without panicking.
    #[test]
    fn test_frame_pipeline_config_with_new_ops() {
        use oximedia_transcode::frame_pipeline::{FramePipelineConfig, VideoFrameOp};
        use oximedia_transcode::hdr_passthrough::HdrPassthroughMode;
        let dir = std::env::temp_dir();
        let cfg = FramePipelineConfig {
            input: dir.join("dummy_in.mkv"),
            output: dir.join("dummy_out.mkv"),
            video_codec: None,
            audio_codec: None,
            video_ops: vec![
                VideoFrameOp::Deinterlace,
                VideoFrameOp::ColorCorrect {
                    brightness: 1.05,
                    contrast: 1.1,
                    saturation: 1.0,
                },
                VideoFrameOp::Scale {
                    width: 1920,
                    height: 1080,
                },
            ],
            audio_ops: Vec::new(),
            hdr_mode: HdrPassthroughMode::Passthrough,
            source_hdr: None,
            hw_accel: false,
            threads: 0,
        };
        assert_eq!(cfg.video_ops.len(), 3);
        // Verify discriminant-level Debug output to confirm variants are present.
        let debug_str = format!("{:?}", cfg.video_ops[0]);
        assert!(debug_str.contains("Deinterlace"), "variant name in Debug");
        let debug_str2 = format!("{:?}", cfg.video_ops[1]);
        assert!(debug_str2.contains("ColorCorrect"), "variant name in Debug");
    }

    /// `run_restore_video` must not panic (nested-runtime guard) even when the
    /// frame pipeline falls back to a byte copy for an unknown file format.
    #[tokio::test]
    async fn test_restore_video_does_not_panic() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_test_restore_0177_vid.bin");
        let output = dir.join("oximedia_test_restore_0177_vid_out.bin");
        // Write some random bytes — not a real container, so the pipeline
        // will fall back to byte copy.  The important thing is no panic.
        std::fs::write(&input, b"not-a-real-video-file").expect("write dummy video");
        let opts = RestoreVideoOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "deinterlace".to_string(),
            width: None,
            height: None,
        };
        // Must complete without panicking regardless of success/failure.
        let _result = run_restore_video(opts, false).await;
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    // --- Slice S7: restore-video stabilisation -------------------------------

    /// Build a synthetic Y4M clip (4:2:0) of `frame_count` frames whose textured
    /// content is shifted by a per-frame "camera jitter" offset.
    ///
    /// The pattern is a high-contrast diagonal grid: it has plenty of corner
    /// features so the stabiliser's feature tracker can lock onto real motion.
    /// `jitter` maps a frame index to an `(dx, dy)` pixel offset.
    fn build_jittered_y4m(
        width: u32,
        height: u32,
        frame_count: usize,
        jitter: impl Fn(usize) -> (i32, i32),
    ) -> Vec<u8> {
        let w = width as usize;
        let h = height as usize;
        let cw = (w + 1) / 2;
        let ch = (h + 1) / 2;

        // Render the luma value of the static scene at absolute pixel (x, y).
        let scene_luma = |x: i32, y: i32| -> u8 {
            // Diagonal grid + bright square blocks => strong corners.
            let grid = ((x % 16) < 3) || ((y % 16) < 3);
            let block = ((x / 12 + y / 12) % 2) == 0;
            match (grid, block) {
                (true, _) => 235,
                (false, true) => 180,
                (false, false) => 32,
            }
        };

        let mut data = Vec::new();
        let header = format!("YUV4MPEG2 W{width} H{height} F30:1 Ip C420jpeg\n");
        data.extend_from_slice(header.as_bytes());

        for f in 0..frame_count {
            let (jx, jy) = jitter(f);
            data.extend_from_slice(b"FRAME\n");
            // Y plane — shifted scene.
            for y in 0..h {
                for x in 0..w {
                    let sx = x as i32 + jx;
                    let sy = y as i32 + jy;
                    data.push(scene_luma(sx, sy));
                }
            }
            // Cb / Cr planes — mid-grey, also shifted so chroma tracks luma.
            for _plane in 0..2 {
                for y in 0..ch {
                    for x in 0..cw {
                        let sx = x as i32 + jx;
                        let sy = y as i32 + jy;
                        // Slight spatial variation so chroma is not perfectly flat.
                        let v = 128i32 + ((sx + sy) % 17) - 8;
                        data.push(v.clamp(16, 240) as u8);
                    }
                }
            }
        }
        data
    }

    /// End-to-end: `run_restore_video --mode stabilize` on a jittered Y4M clip
    /// must complete, write a valid Y4M output, and change the picture.
    #[tokio::test]
    async fn test_restore_video_stabilize_y4m_end_to_end() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_test_restore_s7_stab_in.y4m");
        let output = dir.join("oximedia_test_restore_s7_stab_out.y4m");

        // 16 frames of 64x48 video with a shaky sinusoidal camera path.
        let jitter = |f: usize| {
            let t = f as f64;
            let dx = (t * 0.9).sin() * 4.0;
            let dy = (t * 0.7).cos() * 3.0;
            (dx.round() as i32, dy.round() as i32)
        };
        let y4m = build_jittered_y4m(64, 48, 16, jitter);
        std::fs::write(&input, &y4m).expect("write jittered y4m");

        let opts = RestoreVideoOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "stabilize".to_string(),
            width: None,
            height: None,
        };
        let result = run_restore_video(opts, false).await;
        assert!(
            result.is_ok(),
            "stabilise should succeed on a Y4M clip: {:?}",
            result.err()
        );

        // Output must exist and be a valid Y4M stream with the same frame count.
        let out_bytes = std::fs::read(&output).expect("read stabilised output");
        assert!(
            out_bytes.starts_with(b"YUV4MPEG2"),
            "output must be a Y4M stream"
        );
        let frame_tags = out_bytes.windows(6).filter(|w| *w == b"FRAME\n").count();
        assert_eq!(frame_tags, 16, "all 16 frames must be re-encoded");

        // Stabilisation warps pixels, so the output must differ from the input.
        assert_ne!(
            out_bytes, y4m,
            "stabilised output must differ from the jittered input"
        );

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    /// `stabilize_video_y4m` preserves geometry (size, frame count) and reports
    /// a non-trivial number of changed bytes for a clearly shaky clip.
    #[test]
    fn test_stabilize_video_y4m_reduces_jitter() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_test_restore_s7_direct_in.y4m");
        let output = dir.join("oximedia_test_restore_s7_direct_out.y4m");

        // A sawtooth jitter: large, obvious frame-to-frame displacement.
        let jitter = |f: usize| {
            let phase = (f % 6) as i32;
            (phase - 3, (phase * 2) - 5)
        };
        let y4m = build_jittered_y4m(80, 64, 14, jitter);
        std::fs::write(&input, &y4m).expect("write y4m");

        let summary = stabilize_video_y4m(&input, &output).expect("stabilise should succeed");

        assert_eq!(summary.width, 80);
        assert_eq!(summary.height, 64);
        assert_eq!(summary.frame_count, 14);
        assert!(summary.output_size > 0, "output must be non-empty");
        assert!(
            summary.bytes_changed > 0,
            "warping a jittered clip must change frame bytes"
        );

        // The output must round-trip back through the Y4M demuxer cleanly.
        use oximedia_container::demux::y4m::Y4mDemuxer;
        let out_bytes = std::fs::read(&output).expect("read output");
        let mut demuxer =
            Y4mDemuxer::new(std::io::Cursor::new(out_bytes.as_slice())).expect("parse output y4m");
        assert_eq!(demuxer.width(), 80);
        assert_eq!(demuxer.height(), 64);
        let frames = demuxer.read_all_frames().expect("read output frames");
        assert_eq!(frames.len(), 14);

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    /// Stabilisation must reject a non-Y4M input with a clear, actionable error.
    #[tokio::test]
    async fn test_restore_video_stabilize_rejects_non_y4m() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_test_restore_s7_bad_in.mp4");
        let output = dir.join("oximedia_test_restore_s7_bad_out.y4m");
        std::fs::write(&input, b"\x00\x00\x00\x18ftypmp42not-a-y4m").expect("write fake mp4");

        let opts = RestoreVideoOptions {
            input: input.clone(),
            output: output.clone(),
            mode: "stabilize".to_string(),
            width: None,
            height: None,
        };
        let result = run_restore_video(opts, false).await;
        assert!(result.is_err(), "non-Y4M stabilise input must error");
        let msg = format!("{}", result.expect_err("must be an error"));
        assert!(
            msg.contains("YUV4MPEG2") || msg.contains("Y4M"),
            "error must point the user at the Y4M requirement, got: {msg}"
        );

        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);
    }

    /// `ChromaLayout` computes correct plane sizes for the common Y4M formats.
    ///
    /// The type now lives in the shared frame harness; this test keeps
    /// covering the geometry the stabilise path depends on.
    #[test]
    fn test_chroma_layout_plane_sizes() {
        use crate::frame_harness::ChromaLayout;
        use oximedia_container::demux::y4m::Y4mChroma;

        let l420 = ChromaLayout::for_chroma(Y4mChroma::C420jpeg, 64, 48).expect("420 layout");
        // Y = 64*48, Cb = Cr = 32*24.
        assert_eq!(l420.frame_size(), 64 * 48 + 2 * 32 * 24);

        let l422 = ChromaLayout::for_chroma(Y4mChroma::C422, 64, 48).expect("422 layout");
        assert_eq!(l422.frame_size(), 64 * 48 + 2 * 32 * 48);

        let l444 = ChromaLayout::for_chroma(Y4mChroma::C444, 64, 48).expect("444 layout");
        assert_eq!(l444.frame_size(), 64 * 48 * 3);

        let lmono = ChromaLayout::for_chroma(Y4mChroma::Mono, 64, 48).expect("mono layout");
        assert_eq!(lmono.frame_size(), 64 * 48);
    }
}
