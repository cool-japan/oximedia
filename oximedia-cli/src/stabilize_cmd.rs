//! Video stabilisation command.
//!
//! Provides `oximedia stabilize` using `oximedia-stabilize` to remove unwanted
//! camera shake via configurable motion models and quality presets.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// Options for the `stabilize` command.
pub struct StabilizeOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub mode: String,
    pub quality: String,
    pub smoothing: u32,
    pub zoom: bool,
}

/// Operation label used in the frame harness's error messages.
const STABILIZE_OP: &str = "stabilize";

/// Entry point called from `main.rs`.
///
/// A real decode -> stabilise -> encode pass:
///
/// 1. [`crate::frame_harness::process_clip`] demuxes the whole input Y4M
///    clip (stabilisation needs global context: one motion trajectory
///    computed across every frame).
/// 2. [`crate::frame_harness::ops::stabilize_clip`] runs the real
///    `oximedia-stabilize` offline multi-pass pipeline — multi-pass
///    analysis, Harris-corner feature tracking, trajectory smoothing, and
///    (when `--zoom` is set) a real
///    [`oximedia_stabilize::zoom::calculate::ZoomOptimizer`] pass — with the
///    [`oximedia_stabilize::StabilizeConfig`] built from this command's own
///    `--mode`/`--quality`/`--smoothing`/`--zoom` flags, not a hard-coded
///    configuration.
/// 3. The stabilised clip is muxed back into the output Y4M.
///
/// # Errors
///
/// Returns an error if `--mode`/`--quality`/`--smoothing` do not form a valid
/// [`oximedia_stabilize::StabilizeConfig`], if the input is not Y4M, or if
/// any stage of the stabilisation pipeline fails. No output file is written
/// unless the whole clip succeeds.
pub async fn run_stabilize(opts: StabilizeOptions, json_output: bool) -> Result<()> {
    use oximedia_stabilize::StabilizeConfig;

    if !opts.input.exists() {
        return Err(anyhow::anyhow!(
            "Input file not found: {}",
            opts.input.display()
        ));
    }

    let stab_mode = parse_mode(&opts.mode)?;
    let quality = parse_quality(&opts.quality)?;

    // Smoothing strength as normalised 0.0-1.0 from the frame-count window
    let smoothing_strength = (opts.smoothing as f64 / 100.0).clamp(0.01, 1.0);

    let config = StabilizeConfig::new()
        .with_mode(stab_mode)
        .with_quality(quality)
        .with_smoothing_strength(smoothing_strength)
        .with_zoom_optimization(opts.zoom);

    config
        .validate()
        .with_context(|| "Invalid stabilisation configuration")?;

    // Stabilisation decodes the whole clip, tracks motion across it, warps
    // every plane of every frame and re-encodes: CPU-bound and fully
    // synchronous, so it runs on the blocking pool rather than stalling the
    // async runtime.
    let input = opts.input.clone();
    let output = opts.output.clone();
    let (stats, (width, height)) = tokio::task::spawn_blocking(
        move || -> Result<(crate::frame_harness::ClipStats, (u32, u32))> {
            // `process_clip`'s operation closure only sees the *input* clip,
            // so the geometry it reports is captured as it passes through —
            // `Cell` is enough (both closures run on this single blocking
            // thread, one strictly nested inside the other; nothing else
            // touches it concurrently).
            let geometry = std::cell::Cell::new((0u32, 0u32));
            let stats =
                crate::frame_harness::process_clip(STABILIZE_OP, &input, &output, |clip| {
                    geometry.set((clip.width(), clip.height()));
                    crate::frame_harness::ops::stabilize_clip(clip, &config)
                })?;
            Ok((stats, geometry.get()))
        },
    )
    .await
    .map_err(|join_err| anyhow::anyhow!("stabilize task panicked: {join_err}"))??;

    if json_output {
        let obj = serde_json::json!({
            "input": opts.input.display().to_string(),
            "output": opts.output.display().to_string(),
            "operation": "stabilize",
            "input_format": "y4m",
            "output_format": "y4m",
            "width": width,
            "height": height,
            "mode": opts.mode,
            "quality": opts.quality,
            "smoothing": opts.smoothing,
            "zoom_optimization": opts.zoom,
            "frame_count": stats.frame_count,
            "input_size_bytes": stats.bytes_in,
            "output_size_bytes": stats.bytes_out,
            "bytes_changed": stats.bytes_changed,
            "note": "Offline multi-pass stabilisation: Harris-corner feature tracking, motion \
                     estimation, trajectory smoothing, optional zoom optimisation, and per-plane \
                     frame warping via oximedia-stabilize. Featureless footage falls back to \
                     identity transforms (bytes_changed may then be 0) rather than failing.",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Video Stabilisation Complete".green().bold());
    println!("  {} {}", "Input:".cyan(), opts.input.display());
    println!("  {} {}", "Output:".cyan(), opts.output.display());
    println!("  {} {}x{}", "Video:".cyan(), width, height);
    println!(
        "  {} {} / {} (smoothing {}{})",
        "Mode:".cyan(),
        opts.mode,
        opts.quality,
        opts.smoothing,
        if opts.zoom { ", zoom on" } else { "" }
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

/// Map CLI mode string to `StabilizationMode`.
fn parse_mode(mode: &str) -> Result<oximedia_stabilize::StabilizationMode> {
    use oximedia_stabilize::StabilizationMode;
    match mode.to_lowercase().as_str() {
        "translation" => Ok(StabilizationMode::Translation),
        "affine" => Ok(StabilizationMode::Affine),
        "perspective" => Ok(StabilizationMode::Perspective),
        "3d" | "threed" => Ok(StabilizationMode::ThreeD),
        other => anyhow::bail!(
            "Unknown stabilisation mode '{}'. Use: translation, affine, perspective, 3d",
            other
        ),
    }
}

/// Map CLI quality string to `QualityPreset`.
fn parse_quality(quality: &str) -> Result<oximedia_stabilize::QualityPreset> {
    use oximedia_stabilize::QualityPreset;
    match quality.to_lowercase().as_str() {
        "fast" => Ok(QualityPreset::Fast),
        "balanced" => Ok(QualityPreset::Balanced),
        "maximum" | "max" => Ok(QualityPreset::Maximum),
        other => anyhow::bail!(
            "Unknown quality preset '{}'. Use: fast, balanced, maximum",
            other
        ),
    }
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
            "oximedia_stabilize_cmd_{}_{name}",
            std::process::id()
        ))
    }

    fn opts(mode: &str, quality: &str, input: PathBuf, output: PathBuf) -> StabilizeOptions {
        StabilizeOptions {
            input,
            output,
            mode: mode.to_string(),
            quality: quality.to_string(),
            smoothing: 30,
            zoom: true,
        }
    }

    /// Write a flat 4:2:0 Y4M clip.
    fn write_flat_y4m(path: &std::path::Path, w: u32, h: u32, frames: usize, y: u8) {
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        let mut buf = format!("YUV4MPEG2 W{w} H{h} F25:1 Ip A1:1 C420jpeg\n").into_bytes();
        for _ in 0..frames {
            buf.extend_from_slice(b"FRAME\n");
            buf.extend(std::iter::repeat_n(y, (w * h) as usize));
            buf.extend(std::iter::repeat_n(128u8, (cw * ch * 2) as usize));
        }
        std::fs::write(path, buf).expect("write y4m fixture");
    }

    #[test]
    fn test_parse_mode_variants() {
        assert!(parse_mode("translation").is_ok());
        assert!(parse_mode("affine").is_ok());
        assert!(parse_mode("perspective").is_ok());
        assert!(parse_mode("3d").is_ok());
        assert!(parse_mode("bogus").is_err());
    }

    #[test]
    fn test_parse_quality_variants() {
        assert!(parse_quality("fast").is_ok());
        assert!(parse_quality("balanced").is_ok());
        assert!(parse_quality("maximum").is_ok());
        assert!(parse_quality("bogus").is_err());
    }

    #[tokio::test]
    async fn stabilize_missing_input_errors() {
        let input = temp_path("missing_in.y4m");
        let output = temp_path("missing_out.y4m");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);

        let err = run_stabilize(opts("affine", "fast", input, output.clone()), false)
            .await
            .expect_err("missing input must error");
        assert!(err.to_string().contains("not found"));
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn stabilize_invalid_mode_errors_before_touching_the_file() {
        let input = temp_path("badmode_in.y4m");
        let output = temp_path("badmode_out.y4m");
        write_flat_y4m(&input, 16, 16, 2, 128);
        let _ = std::fs::remove_file(&output);

        let result =
            run_stabilize(opts("bogus", "fast", input.clone(), output.clone()), false).await;
        assert!(result.is_err(), "invalid mode must error");
        assert!(!output.exists());

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn stabilize_requires_y4m_input() {
        let input = temp_path("not_y4m_in.mov");
        let output = temp_path("not_y4m_out.y4m");
        std::fs::write(&input, b"\x00\x00\x00\x18ftypqt  not-a-y4m").expect("write fake mov");
        let _ = std::fs::remove_file(&output);

        let err = run_stabilize(opts("affine", "fast", input.clone(), output.clone()), false)
            .await
            .expect_err("non-Y4M input must be refused");
        let msg = err.to_string();
        assert!(msg.contains("YUV4MPEG2"), "got: {msg}");
        assert!(!output.exists());

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn stabilize_flat_clip_preserves_geometry_via_identity_fallback() {
        let input = temp_path("flat_in.y4m");
        let output = temp_path("flat_out.y4m");
        write_flat_y4m(&input, 32, 24, 4, 128);
        let _ = std::fs::remove_file(&output);

        run_stabilize(opts("affine", "fast", input.clone(), output.clone()), false)
            .await
            .expect("stabilise must succeed on flat footage via the identity fallback");

        let bytes = std::fs::read(&output).expect("read output");
        assert!(bytes.starts_with(b"YUV4MPEG2"));
        assert!(
            bytes.starts_with(b"YUV4MPEG2 W32 H24"),
            "geometry must be preserved, got header: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(40)])
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }
}
