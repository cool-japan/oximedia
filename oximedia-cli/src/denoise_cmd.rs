//! Video denoising command.
//!
//! Provides the `oximedia denoise` subcommand for noise reduction using the
//! `oximedia-denoise` crate's `Denoiser` and `DenoiseConfig`.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// Options for the `denoise` command.
pub struct DenoiseOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub mode: String,
    pub strength: f32,
    pub spatial: bool,
    pub temporal: bool,
    pub preserve_grain: bool,
}

/// Operation label used in the frame harness's error messages.
const DENOISE_OP: &str = "denoise";

/// Entry point called from `main.rs`.
///
/// A real decode -> denoise -> encode pass:
///
/// 1. [`crate::frame_harness::process_frames`] demuxes the input Y4M one
///    frame at a time.
/// 2. Each packed planar frame is bridged to an
///    [`oximedia_codec::VideoFrame`] via [`crate::frame_harness::adapt`].
/// 3. A single [`oximedia_denoise::Denoiser`] — reused across every frame, so
///    its temporal buffer and noise estimate actually accumulate across the
///    clip — processes it for real (bilateral / NL-means / spatio-temporal
///    hybrid / grain-aware, depending on `--mode`).
/// 4. The denoised frame is packed back and muxed into the output Y4M.
///
/// `--spatial`/`--temporal` steer the algorithm *selection*, not a
/// `temporal_window` knob: [`oximedia_denoise::DenoiseConfig::validate`]
/// requires `temporal_window` in `3..=15`, so 3 is the hybrid's structural
/// floor and cannot be pushed lower to suppress it. The only mode with a
/// temporal component is `Balanced` (its hybrid engages once >= 3 frames are
/// buffered); `Fast` (bilateral), `Quality` (NL-means) and `GrainAware`
/// (per-frame grain map) are already spatial-only end to end. So `--spatial`
/// without `--temporal` downgrades a `Balanced` selection to `Fast` — a real,
/// verifiable, purely-spatial substitution — and leaves every other mode
/// untouched (there is nothing to downgrade). `--temporal` alone is
/// already `Balanced`'s default behaviour, so it changes nothing there.
///
/// # Errors
///
/// Returns an error if `--mode`/`--strength`/`--preserve-grain` do not form a
/// valid [`oximedia_denoise::DenoiseConfig`], if the input is not Y4M, or if
/// any frame fails to denoise. No output file is written unless the whole
/// clip succeeds.
pub async fn run_denoise(opts: DenoiseOptions, json_output: bool) -> Result<()> {
    use oximedia_denoise::{DenoiseMode, Denoiser};

    if !opts.input.exists() {
        return Err(anyhow::anyhow!(
            "Input file not found: {}",
            opts.input.display()
        ));
    }

    let requested_mode = parse_mode(&opts.mode)?;
    let effective_mode =
        if opts.spatial && !opts.temporal && requested_mode == DenoiseMode::Balanced {
            DenoiseMode::Fast
        } else {
            requested_mode
        };
    let config = build_config(effective_mode, opts.strength, opts.preserve_grain)?;

    let (header, layout) = crate::frame_harness::peek_y4m_header(DENOISE_OP, &opts.input)?;
    let fps_num = header.fps_num.max(1);
    let fps_den = header.fps_den.max(1);

    // Denoising decodes, filters and re-encodes every frame: CPU-bound and
    // fully synchronous, so it runs on the blocking pool rather than
    // stalling the async runtime.
    let input = opts.input.clone();
    let output = opts.output.clone();
    let stats = tokio::task::spawn_blocking(move || -> Result<crate::frame_harness::ClipStats> {
        let mut denoiser = Denoiser::new(config)
            .map_err(|e| anyhow::anyhow!("Failed to initialise Denoiser: {e}"))?;

        crate::frame_harness::process_frames(DENOISE_OP, &input, &output, |index, frame| {
            let video =
                crate::frame_harness::adapt::planar_to_video_frame(frame, index, fps_num, fps_den)?;
            let processed = denoiser
                .process(&video)
                .map_err(|e| anyhow::anyhow!("Denoise failed on frame {index}: {e}"))?;
            *frame = crate::frame_harness::adapt::video_frame_to_planar(&processed, frame.layout)?;
            Ok(())
        })
    })
    .await
    .map_err(|join_err| anyhow::anyhow!("denoise task panicked: {join_err}"))??;

    if stats.bytes_changed == 0 {
        // A denoiser that "succeeded" without touching a single byte is a
        // copy dressed up as a denoise pass. Remove the output and say so.
        let _ = std::fs::remove_file(&opts.output);
        anyhow::bail!(
            "denoise changed no pixels, so no output was written to {}: strength {:.2} may be \
             too low, or the source may already be noise-free at this precision. Refusing to \
             report a successful denoise.",
            opts.output.display(),
            opts.strength
        );
    }

    if json_output {
        let obj = serde_json::json!({
            "input": opts.input.display().to_string(),
            "output": opts.output.display().to_string(),
            "operation": "denoise",
            "input_format": "y4m",
            "output_format": "y4m",
            "width": layout.luma_w,
            "height": layout.luma_h,
            "mode": opts.mode,
            "effective_mode": format!("{effective_mode:?}"),
            "strength": opts.strength,
            "spatial_only": opts.spatial,
            "temporal_preferred": opts.temporal,
            "preserve_grain": opts.preserve_grain,
            "frame_count": stats.frame_count,
            "input_size_bytes": stats.bytes_in,
            "output_size_bytes": stats.bytes_out,
            "bytes_changed": stats.bytes_changed,
            "note": "Processed with a single oximedia_denoise::Denoiser reused across every \
                     frame, so temporal state (noise estimate, frame buffer) accumulates across \
                     the clip exactly as it would in a real pipeline.",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Denoise Complete".green().bold());
    println!("  {} {}", "Input:".cyan(), opts.input.display());
    println!("  {} {}", "Output:".cyan(), opts.output.display());
    println!(
        "  {} {}x{} @ {}/{} fps",
        "Video:".cyan(),
        layout.luma_w,
        layout.luma_h,
        fps_num,
        fps_den
    );
    println!(
        "  {} {} (strength {:.2}{})",
        "Mode:".cyan(),
        opts.mode,
        opts.strength,
        if effective_mode != requested_mode {
            format!(", downgraded to {effective_mode:?} for --spatial")
        } else {
            String::new()
        }
    );
    println!(
        "  {} {} bytes ({} bytes changed, {} frames)",
        "Output size:".cyan(),
        stats.bytes_out,
        stats.bytes_changed,
        stats.frame_count
    );

    Ok(())
}

/// Map CLI mode string to `DenoiseMode`.
fn parse_mode(mode: &str) -> Result<oximedia_denoise::DenoiseMode> {
    use oximedia_denoise::DenoiseMode;
    match mode.to_lowercase().replace('-', "_").as_str() {
        "fast" => Ok(DenoiseMode::Fast),
        "balanced" => Ok(DenoiseMode::Balanced),
        "quality" => Ok(DenoiseMode::Quality),
        "grain_aware" | "grain-aware" => Ok(DenoiseMode::GrainAware),
        other => anyhow::bail!(
            "Unknown denoise mode '{}'. Use: fast, balanced, quality, grain-aware",
            other
        ),
    }
}

/// Build a `DenoiseConfig` from the provided options.
fn build_config(
    mode: oximedia_denoise::DenoiseMode,
    strength: f32,
    preserve_grain: bool,
) -> Result<oximedia_denoise::DenoiseConfig> {
    use oximedia_denoise::DenoiseConfig;

    let base = match mode {
        oximedia_denoise::DenoiseMode::Fast => DenoiseConfig::light(),
        oximedia_denoise::DenoiseMode::Quality => DenoiseConfig::strong(),
        _ => DenoiseConfig::medium(),
    };

    let config = DenoiseConfig {
        mode,
        strength: strength.clamp(0.0, 1.0),
        preserve_grain,
        ..base
    };

    config
        .validate()
        .with_context(|| "Invalid denoise configuration")?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch path for a fixture.
    ///
    /// The PID matters: this module is compiled into *both* the `oximedia`
    /// binary and the `oximedia-cli` lib target, so every test here runs
    /// twice in two concurrent processes. Fixed names would let one copy's
    /// cleanup delete the other copy's fixture mid-test.
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_denoise_cmd_{}_{name}",
            std::process::id()
        ))
    }

    fn opts(mode: &str, input: PathBuf, output: PathBuf) -> DenoiseOptions {
        DenoiseOptions {
            input,
            output,
            mode: mode.to_string(),
            strength: 0.5,
            spatial: true,
            temporal: false,
            preserve_grain: false,
        }
    }

    #[test]
    fn test_parse_mode_variants() {
        assert!(parse_mode("fast").is_ok());
        assert!(parse_mode("balanced").is_ok());
        assert!(parse_mode("quality").is_ok());
        assert!(parse_mode("grain-aware").is_ok());
        assert!(parse_mode("bogus").is_err());
    }

    #[tokio::test]
    async fn denoise_missing_input_errors() {
        let input = temp_path("missing_in.y4m");
        let output = temp_path("missing_out.y4m");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);

        let err = run_denoise(opts("balanced", input, output.clone()), false)
            .await
            .expect_err("missing input must error");
        assert!(err.to_string().contains("not found"));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn denoise_invalid_mode_errors() {
        let input = temp_path("badmode_in.y4m");
        let output = temp_path("badmode_out.y4m");
        std::fs::write(&input, b"YUV4MPEG2 W4 H4 F25:1 Ip A1:1 C420jpeg\n").expect("write stub");

        let result = run_denoise(opts("bogus-mode", input.clone(), output), false).await;
        assert!(result.is_err(), "invalid mode must error");

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn denoise_requires_y4m_input() {
        let input = temp_path("not_y4m_in.mp4");
        let output = temp_path("not_y4m_out.y4m");
        std::fs::write(&input, b"\x00\x00\x00\x18ftypmp42not-a-y4m").expect("write fake mp4");
        let _ = std::fs::remove_file(&output);

        let err = run_denoise(opts("balanced", input.clone(), output.clone()), false)
            .await
            .expect_err("non-Y4M input must be refused");
        let msg = err.to_string();
        assert!(msg.contains("YUV4MPEG2"), "got: {msg}");
        assert!(!output.exists());

        std::fs::remove_file(&input).ok();
    }
}
