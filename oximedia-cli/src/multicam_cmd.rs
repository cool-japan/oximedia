//! Multi-camera CLI commands for OxiMedia.
//!
//! Provides multi-camera synchronization, switching, compositing,
//! color matching, and export commands.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Multi-camera command subcommands.
#[derive(Subcommand, Debug)]
pub enum MulticamCommand {
    /// Synchronize multiple camera angles
    Sync {
        /// Input camera files
        #[arg(short, long, required = true, num_args = 2..)]
        inputs: Vec<PathBuf>,

        /// Output synchronized timeline file (JSON)
        #[arg(short, long)]
        output: PathBuf,

        /// Sync method: audio, timecode, marker
        #[arg(long, default_value = "audio")]
        method: String,

        /// Drift tolerance in frames
        #[arg(long, default_value = "2")]
        drift_tolerance: u32,
    },

    /// Switch between camera angles at specified points
    Switch {
        /// Input camera files
        #[arg(short, long, required = true, num_args = 2..)]
        inputs: Vec<PathBuf>,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// JSON switch points: [{"time": 1.0, "camera": 0}, ...]
        #[arg(long)]
        switch_points: Option<String>,

        /// Enable automatic switching based on content analysis
        #[arg(long)]
        auto_switch: bool,

        /// Minimum shot duration in seconds for auto-switch
        #[arg(long, default_value = "2.0")]
        min_duration: f64,
    },

    /// Composite multiple cameras into a single frame layout
    Composite {
        /// Input camera files
        #[arg(short, long, required = true, num_args = 2..)]
        inputs: Vec<PathBuf>,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Layout type: grid, pip, side_by_side, stack
        #[arg(long, default_value = "grid")]
        layout: String,

        /// Output width in pixels
        #[arg(long)]
        width: Option<u32>,

        /// Output height in pixels
        #[arg(long)]
        height: Option<u32>,

        /// Grid spacing in pixels
        #[arg(long, default_value = "4")]
        spacing: u32,
    },

    /// Measure colour statistics across camera angles and compute corrections
    ColorMatch {
        /// Reference camera file (uncompressed YUV4MPEG2 / .y4m)
        #[arg(long)]
        reference: PathBuf,

        /// Input camera files to match (uncompressed YUV4MPEG2 / .y4m)
        #[arg(short, long, required = true, num_args = 1..)]
        inputs: Vec<PathBuf>,

        /// Output directory for per-angle colour-correction reports
        /// (JSON metadata; no video is re-encoded)
        #[arg(short, long)]
        output_dir: PathBuf,
    },

    /// Export multi-camera timeline in various formats
    Export {
        /// Input timeline file (JSON)
        #[arg(short, long)]
        timeline: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Export format: multicam_edl, xml, json
        #[arg(long, default_value = "multicam_edl")]
        format: String,
    },

    /// Show information about multi-camera layouts
    Layouts {},
}

/// Handle multicam command dispatch.
pub async fn handle_multicam_command(command: MulticamCommand, json_output: bool) -> Result<()> {
    match command {
        MulticamCommand::Sync {
            inputs,
            output,
            method,
            drift_tolerance,
        } => sync_cameras(&inputs, &output, &method, drift_tolerance, json_output).await,
        MulticamCommand::Switch {
            inputs,
            output,
            switch_points,
            auto_switch,
            min_duration,
        } => {
            switch_cameras(
                &inputs,
                &output,
                switch_points.as_deref(),
                auto_switch,
                min_duration,
                json_output,
            )
            .await
        }
        MulticamCommand::Composite {
            inputs,
            output,
            layout,
            width,
            height,
            spacing,
        } => {
            composite_cameras(
                &inputs,
                &output,
                &layout,
                width,
                height,
                spacing,
                json_output,
            )
            .await
        }
        MulticamCommand::ColorMatch {
            reference,
            inputs,
            output_dir,
        } => color_match(&reference, &inputs, &output_dir, json_output).await,
        MulticamCommand::Export {
            timeline,
            output,
            format,
        } => export_timeline(&timeline, &output, &format, json_output).await,
        MulticamCommand::Layouts {} => list_layouts(json_output).await,
    }
}

/// Validate sync method.
fn validate_sync_method(method: &str) -> Result<()> {
    match method {
        "audio" | "timecode" | "marker" => Ok(()),
        other => Err(anyhow::anyhow!(
            "Unknown sync method '{}'. Expected: audio, timecode, marker",
            other
        )),
    }
}

/// Validate layout type and return description.
fn layout_description(layout: &str) -> Result<&'static str> {
    match layout {
        "grid" => Ok("Grid layout (auto-sized)"),
        "pip" => Ok("Picture-in-picture (main + inset)"),
        "side_by_side" => Ok("Side-by-side (horizontal split)"),
        "stack" => Ok("Vertical stack layout"),
        other => Err(anyhow::anyhow!(
            "Unknown layout '{}'. Expected: grid, pip, side_by_side, stack",
            other
        )),
    }
}

/// Synchronize multiple camera angles.
async fn sync_cameras(
    inputs: &[PathBuf],
    output: &PathBuf,
    method: &str,
    drift_tolerance: u32,
    json_output: bool,
) -> Result<()> {
    validate_sync_method(method)?;

    for input in inputs {
        if !input.exists() {
            return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
        }
    }

    // Build a multicam config
    let config = oximedia_multicam::MultiCamConfig {
        angle_count: inputs.len(),
        enable_audio_sync: method == "audio",
        enable_timecode_sync: method == "timecode",
        enable_visual_sync: method == "marker",
        drift_tolerance,
        ..oximedia_multicam::MultiCamConfig::default()
    };

    // Generate sync result data
    let sync_result = serde_json::json!({
        "cameras": inputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "method": method,
        "angle_count": config.angle_count,
        "drift_tolerance": drift_tolerance,
        "frame_rate": config.frame_rate,
        "status": "sync_ready",
        "offsets": inputs.iter().enumerate().map(|(i, _)| {
            serde_json::json!({ "camera": i, "offset_frames": 0, "confidence": 1.0 })
        }).collect::<Vec<_>>(),
        "message": "Cameras configured; audio/timecode sync requires frame decoding pipeline",
    });

    let json_str =
        serde_json::to_string_pretty(&sync_result).context("Failed to serialize sync result")?;

    tokio::fs::write(output, json_str.as_bytes())
        .await
        .context("Failed to write output file")?;

    if json_output {
        println!("{}", json_str);
    } else {
        println!("{}", "Multi-Camera Sync".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Cameras:", inputs.len());
        println!("{:20} {}", "Method:", method);
        println!("{:20} {} frames", "Drift tolerance:", drift_tolerance);
        println!("{:20} {}", "Output:", output.display());
        println!();
        for (i, input) in inputs.iter().enumerate() {
            println!("  Camera {}: {}", i, input.display());
        }
        println!();
        println!(
            "{}",
            "Sync configuration written. Frame decoding pipeline needed for actual sync.".yellow()
        );
    }

    Ok(())
}

/// Switch between camera angles.
async fn switch_cameras(
    inputs: &[PathBuf],
    output: &PathBuf,
    switch_points_json: Option<&str>,
    auto_switch: bool,
    min_duration: f64,
    json_output: bool,
) -> Result<()> {
    for input in inputs {
        if !input.exists() {
            return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
        }
    }

    let switch_points: Vec<serde_json::Value> = if let Some(json) = switch_points_json {
        serde_json::from_str(json).context("Failed to parse switch points JSON")?
    } else {
        Vec::new()
    };

    let switch_result = serde_json::json!({
        "cameras": inputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "auto_switch": auto_switch,
        "min_shot_duration_secs": min_duration,
        "switch_points": switch_points,
        "output": output.display().to_string(),
        "status": "switch_ready",
        "message": "Switch list configured. Frame decoding pipeline needed for rendering.",
    });

    let json_str = serde_json::to_string_pretty(&switch_result)
        .context("Failed to serialize switch result")?;

    tokio::fs::write(output, json_str.as_bytes())
        .await
        .context("Failed to write output file")?;

    if json_output {
        println!("{}", json_str);
    } else {
        println!("{}", "Multi-Camera Switch".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Cameras:", inputs.len());
        println!("{:20} {}", "Auto-switch:", auto_switch);
        println!("{:20} {:.1}s", "Min duration:", min_duration);
        println!("{:20} {}", "Switch points:", switch_points.len());
        println!("{:20} {}", "Output:", output.display());
        println!();
        if !switch_points.is_empty() {
            println!("{}", "Switch Points".cyan().bold());
            println!("{}", "-".repeat(40));
            for sp in &switch_points {
                let time = sp.get("time").and_then(|t| t.as_f64()).unwrap_or(0.0);
                let cam = sp.get("camera").and_then(|c| c.as_u64()).unwrap_or(0);
                println!("  {:.2}s -> Camera {}", time, cam);
            }
        }
    }

    Ok(())
}

/// Composite multiple cameras into a single frame layout.
async fn composite_cameras(
    inputs: &[PathBuf],
    output: &PathBuf,
    layout: &str,
    width: Option<u32>,
    height: Option<u32>,
    spacing: u32,
    json_output: bool,
) -> Result<()> {
    let layout_desc = layout_description(layout)?;

    for input in inputs {
        if !input.exists() {
            return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
        }
    }

    let out_w = width.unwrap_or(1920);
    let out_h = height.unwrap_or(1080);

    // Use grid compositor to calculate layout
    let (grid_rows, grid_cols) =
        oximedia_multicam::composite::grid::GridCompositor::optimal_grid_for_angles(inputs.len());

    let mut grid = oximedia_multicam::composite::grid::GridCompositor::new(out_w, out_h);
    grid.set_spacing(spacing);
    let cells = grid.calculate_grid(grid_rows, grid_cols);

    let composite_result = serde_json::json!({
        "cameras": inputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "layout": layout,
        "layout_description": layout_desc,
        "output_width": out_w,
        "output_height": out_h,
        "grid_rows": grid_rows,
        "grid_cols": grid_cols,
        "spacing": spacing,
        "cells": cells.iter().map(|(x, y, w, h)| {
            serde_json::json!({ "x": x, "y": y, "width": w, "height": h })
        }).collect::<Vec<_>>(),
        "output": output.display().to_string(),
        "status": "composite_ready",
        "message": "Layout computed. Frame decoding pipeline needed for rendering.",
    });

    let json_str = serde_json::to_string_pretty(&composite_result)
        .context("Failed to serialize composite result")?;

    tokio::fs::write(output, json_str.as_bytes())
        .await
        .context("Failed to write output file")?;

    if json_output {
        println!("{}", json_str);
    } else {
        println!("{}", "Multi-Camera Composite".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Cameras:", inputs.len());
        println!("{:20} {} ({})", "Layout:", layout, layout_desc);
        println!("{:20} {}x{}", "Output size:", out_w, out_h);
        println!("{:20} {}x{}", "Grid:", grid_rows, grid_cols);
        println!("{:20} {}px", "Spacing:", spacing);
        println!("{:20} {}", "Output:", output.display());
        println!();
        println!("{}", "Cell Layout".cyan().bold());
        println!("{}", "-".repeat(40));
        for (i, (cx, cy, cw, ch)) in cells.iter().enumerate() {
            if i < inputs.len() {
                println!("  Camera {}: {}x{} at ({}, {})", i, cw, ch, cx, cy);
            }
        }
    }

    Ok(())
}

/// Operation label used in the frame harness's error messages.
const COLOR_MATCH_OP: &str = "multicam color-match";

/// Match colors across camera angles.
///
/// Every number here is measured from real decoded pixels:
///
/// 1. Each angle (reference first, then the inputs) is decoded with
///    [`crate::frame_harness::read_y4m_clip`].
/// 2. [`crate::frame_harness::ops::clip_rgb_stats`] converts every pixel of
///    every frame to RGB and accumulates per-channel mean and standard
///    deviation; the colour temperature and green–magenta tint are derived
///    from those means.
/// 3. The resulting [`oximedia_multicam::color::ColorStats`] are fed to
///    [`oximedia_multicam::color::ColorMatcher::update_stats`] and
///    [`oximedia_multicam::color::ColorMatcher::calculate_corrections`], whose
///    per-angle correction matrices are written to `output_dir` as JSON.
///
/// `ColorStats::new` is deliberately *not* used: its defaults
/// (`mean_rgb: [0.5, 0.5, 0.5]`, `temperature: 6500.0`) are the fabricated
/// "neutral" values this command previously refused to emit. Every field is
/// constructed from measured statistics.
///
/// This command writes correction *metadata*, not corrected video: applying a
/// correction matrix to every pixel and re-encoding is a separate operation.
/// The output says so explicitly.
async fn color_match(
    reference: &PathBuf,
    inputs: &[PathBuf],
    output_dir: &PathBuf,
    json_output: bool,
) -> Result<()> {
    use oximedia_multicam::color::{ColorMatcher, ColorStats};

    if !reference.exists() {
        return Err(anyhow::anyhow!(
            "Reference file not found: {}",
            reference.display()
        ));
    }
    for input in inputs {
        if !input.exists() {
            return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
        }
    }

    // Angle 0 is the reference; angles 1..=n are the inputs to be matched.
    let mut angle_paths: Vec<PathBuf> = Vec::with_capacity(inputs.len() + 1);
    angle_paths.push(reference.clone());
    angle_paths.extend(inputs.iter().cloned());

    // Per angle: the multicam `ColorStats`, the raw sample counts, and the
    // colour-temperature estimate — `None` when the chromaticity is too far
    // off the Planckian locus for McCamy's approximation to mean anything.
    let mut measured: Vec<(ColorStats, crate::frame_harness::ops::RgbStats, Option<f32>)> =
        Vec::with_capacity(angle_paths.len());
    for (angle, path) in angle_paths.iter().enumerate() {
        let clip = crate::frame_harness::read_y4m_clip(COLOR_MATCH_OP, path)?;
        let stats = crate::frame_harness::ops::clip_rgb_stats(&clip)
            .with_context(|| format!("Failed to measure colour of '{}'", path.display()))?;
        let temperature = crate::frame_harness::ops::correlated_color_temperature(stats.mean_rgb);
        measured.push((
            ColorStats {
                angle,
                mean_rgb: stats.mean_rgb,
                std_rgb: stats.std_rgb,
                // `ColorStats` has no "unknown" temperature, and its own
                // constructor would seed the fabricated 6500 K default. 0 K is
                // physically impossible, so it cannot be mistaken for a
                // measurement; the reported value is the `Option` below.
                temperature: temperature.unwrap_or(0.0),
                tint: crate::frame_harness::ops::green_magenta_tint(stats.mean_rgb),
            },
            stats,
            temperature,
        ));
    }

    let mut matcher = ColorMatcher::new(angle_paths.len(), 0);
    for (stats, _, _) in &measured {
        matcher.update_stats(*stats);
    }
    matcher
        .calculate_corrections()
        .map_err(|e| anyhow::anyhow!("Colour correction calculation failed: {e}"))?;

    tokio::fs::create_dir_all(output_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create output directory: {}",
                output_dir.display()
            )
        })?;

    let reference_stats = measured
        .first()
        .map(|(stats, _, _)| *stats)
        .ok_or_else(|| anyhow::anyhow!("no reference statistics were measured"))?;

    let mut angle_reports = Vec::with_capacity(angle_paths.len());
    let mut written_files = Vec::with_capacity(inputs.len());

    for (angle, path) in angle_paths.iter().enumerate() {
        let (stats, sampled, temperature) = measured[angle];
        let matrix = matcher
            .get_correction(angle)
            .ok_or_else(|| anyhow::anyhow!("no correction matrix for angle {angle}"))?;

        let report = serde_json::json!({
            "angle": angle,
            "role": if angle == 0 { "reference" } else { "input" },
            "source": path.display().to_string(),
            "frames_sampled": sampled.frame_count,
            "pixels_sampled": sampled.pixel_count,
            "mean_rgb": stats.mean_rgb,
            "std_rgb": stats.std_rgb,
            // null when the chromaticity is too far off the Planckian locus
            // for McCamy's approximation to be meaningful.
            "temperature_kelvin": temperature,
            "tint_green_magenta": stats.tint,
            "distance_to_reference": stats.distance_to(&reference_stats),
            "correction_matrix": matrix.matrix,
        });

        if angle > 0 {
            let stem = path
                .file_stem()
                .map_or_else(|| format!("angle{angle}"), |s| s.to_string_lossy().into());
            let out_path = output_dir.join(format!("{stem}.colormatch.json"));
            let body = serde_json::to_string_pretty(&serde_json::json!({
                "reference": reference.display().to_string(),
                "reference_mean_rgb": reference_stats.mean_rgb,
                "angle": report,
                "note": "Correction metadata measured from decoded frames. \
                         No video was re-encoded by `multicam color-match`.",
            }))
            .context("Failed to serialise colour-match report")?;
            tokio::fs::write(&out_path, body.as_bytes())
                .await
                .with_context(|| format!("Failed to write {}", out_path.display()))?;
            written_files.push(out_path.display().to_string());
        }

        angle_reports.push(report);
    }

    let summary = serde_json::json!({
        "reference": reference.display().to_string(),
        "cameras": inputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
        "output_dir": output_dir.display().to_string(),
        "reference_angle": 0,
        "color_space": "BT.709 limited-range YCbCr decoded to gamma-encoded sRGB",
        "temperature_model": "McCamy's CCT approximation over linearised channel means; \
                              null when the result falls outside 1000-25000 K, where the \
                              cubic has clearly broken down",
        "tint_model": "linear-light G - (R + B) / 2 (positive = green, negative = magenta)",
        "angles": angle_reports,
        "written_files": written_files,
        "note": "Correction matrices measured from decoded frames. \
                 This command writes correction metadata only; no video was re-encoded.",
    });

    if json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    println!("{}", "Multi-Camera Color Match".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Reference:", reference.display());
    println!("{:20} {}", "Cameras:", inputs.len());
    println!("{:20} {}", "Output dir:", output_dir.display());
    println!();
    println!("{}", "Measured Statistics".cyan().bold());
    println!("{}", "-".repeat(60));
    for (angle, path) in angle_paths.iter().enumerate() {
        let (stats, sampled, temperature) = measured[angle];
        let label = if angle == 0 { "ref" } else { "cam" };
        println!(
            "  [{label} {angle}] {}  ({} frames, {} px)",
            path.display(),
            sampled.frame_count,
            sampled.pixel_count
        );
        println!(
            "        mean RGB {:.4}/{:.4}/{:.4}   std {:.4}/{:.4}/{:.4}",
            stats.mean_rgb[0],
            stats.mean_rgb[1],
            stats.mean_rgb[2],
            stats.std_rgb[0],
            stats.std_rgb[1],
            stats.std_rgb[2]
        );
        println!(
            "        CCT {}   tint {:+.4}   Δ to reference {:.4}",
            temperature.map_or_else(
                || "n/a (off the Planckian locus)".to_string(),
                |k| format!("{k:.0} K")
            ),
            stats.tint,
            stats.distance_to(&reference_stats)
        );
        if angle > 0 {
            if let Some(matrix) = matcher.get_correction(angle) {
                println!(
                    "        gain R {:.4}  G {:.4}  B {:.4}",
                    matrix.matrix[0][0], matrix.matrix[1][1], matrix.matrix[2][2]
                );
            }
        }
    }
    println!();
    for file in &written_files {
        println!("  {} {}", "Wrote:".cyan(), file);
    }
    println!();
    println!(
        "  {} correction metadata only; no video was re-encoded.",
        "Note:".yellow()
    );

    Ok(())
}

/// Build a CMX3600-style multi-camera EDL from a parsed timeline JSON value.
///
/// Only real fields already present in the timeline (`cameras`,
/// `switch_points`, optionally `frame_rate`) are read; nothing here is
/// fabricated placeholder data. Returns an error if the JSON doesn't look
/// like a timeline this command (or `multicam sync`/`multicam switch`)
/// produced.
fn build_multicam_edl(timeline: &serde_json::Value) -> Result<String> {
    let cameras = timeline
        .get("cameras")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Timeline JSON has no 'cameras' array; expected the output of \
                 'oximedia multicam sync' or 'oximedia multicam switch'"
            )
        })?;

    let fps = timeline
        .get("frame_rate")
        .and_then(serde_json::Value::as_f64)
        .filter(|f| *f > 0.0)
        .unwrap_or(25.0);

    let mut out = String::new();
    out.push_str("TITLE: OxiMedia Multi-Camera Export\n");
    out.push_str("FCM: NON-DROP FRAME\n\n");

    for (i, cam) in cameras.iter().enumerate() {
        let path = cam.as_str().unwrap_or("(unknown)");
        out.push_str(&format!(
            "* CAM {i}: {} <- {path}\n",
            reel_name_from_path(path, i)
        ));
    }
    out.push('\n');

    let switch_points = timeline
        .get("switch_points")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if switch_points.is_empty() {
        out.push_str("* No switch points defined in this timeline (no cuts to list)\n");
    } else {
        for (i, sp) in switch_points.iter().enumerate() {
            let time = sp
                .get("time")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0);
            let cam_idx = sp
                .get("camera")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as usize;
            let reel = cameras.get(cam_idx).and_then(|c| c.as_str()).map_or_else(
                || format!("CAM{cam_idx}"),
                |p| reel_name_from_path(p, cam_idx),
            );
            let tc = seconds_to_edl_timecode(time, fps);
            out.push_str(&format!(
                "{:03}  {:<8} V     C        {tc} {tc} {tc} {tc}\n",
                i + 1,
                reel
            ));
        }
    }

    Ok(out)
}

/// Build a minimal real XML export from a parsed timeline JSON value.
///
/// Mirrors [`build_multicam_edl`]: only actual timeline fields are encoded,
/// no synthetic placeholder content.
fn build_multicam_xml(
    timeline: &serde_json::Value,
    timeline_path: &std::path::Path,
) -> Result<String> {
    let cameras = timeline
        .get("cameras")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Timeline JSON has no 'cameras' array; expected the output of \
                 'oximedia multicam sync' or 'oximedia multicam switch'"
            )
        })?;

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<multicam>\n");
    out.push_str("  <source>OxiMedia</source>\n");
    out.push_str(&format!(
        "  <timeline_file>{}</timeline_file>\n",
        xml_escape(&timeline_path.display().to_string())
    ));

    out.push_str("  <cameras>\n");
    for (i, cam) in cameras.iter().enumerate() {
        let path = cam.as_str().unwrap_or("");
        out.push_str(&format!(
            "    <camera index=\"{i}\" path=\"{}\"/>\n",
            xml_escape(path)
        ));
    }
    out.push_str("  </cameras>\n");

    let switch_points = timeline
        .get("switch_points")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    out.push_str("  <switch_points>\n");
    for sp in &switch_points {
        let time = sp
            .get("time")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        let cam_idx = sp
            .get("camera")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        out.push_str(&format!(
            "    <switch time=\"{time}\" camera=\"{cam_idx}\"/>\n"
        ));
    }
    out.push_str("  </switch_points>\n");
    out.push_str("</multicam>\n");

    Ok(out)
}

/// Escape the five predefined XML entities in `s`.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Derive a short reel-style label from a camera file path, e.g.
/// `/media/cam_a.mov` -> `CAM_A`. Falls back to `CAM{index}` for paths with
/// no usable file stem.
fn reel_name_from_path(path: &str, index: usize) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map_or_else(
            || format!("CAM{index}"),
            |stem| {
                stem.chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() {
                            c.to_ascii_uppercase()
                        } else {
                            '_'
                        }
                    })
                    .take(8)
                    .collect()
            },
        )
}

/// Convert a time in seconds to an `HH:MM:SS:FF` EDL timecode at `fps`.
///
/// `fps` is clamped to a sane positive value (defaulting to 25) so the same
/// effective rate is used consistently for both the frame-count and the
/// frames-per-second modulus below.
fn seconds_to_edl_timecode(total_secs: f64, fps: f64) -> String {
    let effective_fps = if fps > 0.0 { fps } else { 25.0 };
    let fps_int = effective_fps.round().max(1.0) as u64;
    let total_frames = (total_secs.max(0.0) * effective_fps).round() as u64;
    let frames = total_frames % fps_int;
    let total_whole_secs = total_frames / fps_int;
    let s = total_whole_secs % 60;
    let m = (total_whole_secs / 60) % 60;
    let h = total_whole_secs / 3600;
    format!("{h:02}:{m:02}:{s:02}:{frames:02}")
}

/// Export multi-camera timeline.
async fn export_timeline(
    timeline: &PathBuf,
    output: &PathBuf,
    format: &str,
    json_output: bool,
) -> Result<()> {
    if !timeline.exists() {
        return Err(anyhow::anyhow!(
            "Timeline file not found: {}",
            timeline.display()
        ));
    }

    match format {
        "multicam_edl" | "xml" | "json" => {}
        other => {
            return Err(anyhow::anyhow!(
                "Unknown export format '{}'. Expected: multicam_edl, xml, json",
                other
            ));
        }
    }

    let timeline_data = tokio::fs::read_to_string(timeline)
        .await
        .context("Failed to read timeline file")?;

    // For "json" the timeline is already in the target format: pass it
    // through verbatim. For "multicam_edl"/"xml" we parse the *real*
    // timeline JSON and re-encode its actual camera list and switch points
    // -- previously these branches ignored `timeline_data` entirely and
    // wrote a fixed boilerplate string naming only the input file path.
    let export_data = match format {
        "json" => timeline_data,
        "multicam_edl" | "xml" => {
            let parsed: serde_json::Value = serde_json::from_str(&timeline_data)
                .with_context(|| {
                    format!(
                        "Timeline file '{}' is not valid JSON; cannot export its real contents to {}",
                        timeline.display(),
                        format
                    )
                })?;
            if format == "multicam_edl" {
                build_multicam_edl(&parsed)?
            } else {
                build_multicam_xml(&parsed, timeline)?
            }
        }
        _ => unreachable!("format already validated above"),
    };

    tokio::fs::write(output, export_data.as_bytes())
        .await
        .context("Failed to write export file")?;

    if json_output {
        let result = serde_json::json!({
            "timeline": timeline.display().to_string(),
            "output": output.display().to_string(),
            "format": format,
            "status": "exported",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize export result")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Timeline Export".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Timeline:", timeline.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Format:", format);
    }

    Ok(())
}

/// List available multi-camera layouts.
async fn list_layouts(json_output: bool) -> Result<()> {
    let layouts = vec![
        ("grid", "Auto-sized grid layout (2x2, 3x3, etc.)"),
        ("pip", "Picture-in-picture with main view and corner inset"),
        ("side_by_side", "Horizontal split between two cameras"),
        ("stack", "Vertical stack of camera views"),
    ];

    if json_output {
        let items: Vec<serde_json::Value> = layouts
            .iter()
            .map(|(name, desc)| serde_json::json!({ "name": name, "description": desc }))
            .collect();
        let json_str =
            serde_json::to_string_pretty(&items).context("Failed to serialize layouts")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Available Multi-Camera Layouts".green().bold());
        println!("{}", "=".repeat(60));
        for (name, desc) in &layouts {
            println!("  {:20} {}", name.cyan(), desc);
        }
        println!();
        println!(
            "{}",
            "Use 'oximedia multicam composite --layout <name>' to apply a layout.".dimmed()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Scratch path for a fixture.
    ///
    /// The PID matters: this module is compiled into *both* the `oximedia`
    /// binary and the `oximedia-cli` lib target (`lib.rs` exposes
    /// `multicam_cmd` so the integration tests can drive it in-process), so
    /// every test here runs twice in two concurrent processes. Fixed names
    /// would let one copy's cleanup delete the other copy's fixture mid-test.
    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia_mc_{}_{name}", std::process::id()))
    }

    #[test]
    fn test_validate_sync_method() {
        assert!(validate_sync_method("audio").is_ok());
        assert!(validate_sync_method("timecode").is_ok());
        assert!(validate_sync_method("marker").is_ok());
        assert!(validate_sync_method("invalid").is_err());
    }

    #[test]
    fn test_layout_description() {
        assert!(layout_description("grid").is_ok());
        assert!(layout_description("pip").is_ok());
        assert!(layout_description("side_by_side").is_ok());
        assert!(layout_description("stack").is_ok());
        assert!(layout_description("unknown").is_err());
    }

    #[test]
    fn test_layout_description_values() {
        let desc = layout_description("grid").expect("valid layout");
        assert!(desc.contains("Grid"));
    }

    #[test]
    fn test_validate_sync_method_error_message() {
        let err = validate_sync_method("xyz").expect_err("should fail");
        let msg = format!("{}", err);
        assert!(msg.contains("xyz"));
    }

    #[test]
    fn test_layout_description_pip() {
        let desc = layout_description("pip").expect("valid layout");
        assert!(desc.contains("Picture"));
    }

    // ── color_match: real statistics, honest-Err on unusable input ──────────

    #[tokio::test]
    async fn test_color_match_missing_reference_errors() {
        let reference = temp_path("missing_reference.mov");
        let output_dir = temp_path("color_match_out_1");
        let _ = std::fs::remove_file(&reference);
        let _ = std::fs::remove_dir_all(&output_dir);

        let err = color_match(&reference, &[], &output_dir, false)
            .await
            .expect_err("missing reference must fail");
        assert!(err.to_string().contains("Reference file not found"));
        assert!(
            !output_dir.exists(),
            "no output directory should be created on failure"
        );
    }

    /// Colour matching is real now, but it needs decodable frames. Files that
    /// are not Y4M are refused with the shared, actionable error — and, as
    /// before, nothing is fabricated on disk.
    #[tokio::test]
    async fn test_color_match_real_inputs_returns_honest_err_no_files() {
        let reference = temp_path("color_match_ref.mov");
        let input = temp_path("color_match_in.mov");
        let output_dir = temp_path("color_match_out_2");
        std::fs::write(
            &reference,
            b"not a real video, just bytes for existence checks",
        )
        .expect("write reference");
        std::fs::write(&input, b"not a real video either").expect("write input");
        let _ = std::fs::remove_dir_all(&output_dir);

        let err = color_match(&reference, std::slice::from_ref(&input), &output_dir, false)
            .await
            .expect_err("color_match must not fabricate success");
        let msg = err.to_string();
        assert!(
            msg.contains("YUV4MPEG2") && msg.contains("oximedia transcode"),
            "error should name the input contract and how to satisfy it, got: {msg}"
        );
        // What must never reappear is the old success-shaped JSON fragment.
        assert!(
            !msg.contains("\"status\": \"color_match_ready\"")
                && !msg.contains("\"status\":\"color_match_ready\""),
            "must not resurrect the old fabricated status JSON, got: {msg}"
        );
        assert!(
            !output_dir.exists(),
            "no output directory or matched files should be fabricated"
        );

        std::fs::remove_file(&reference).ok();
        std::fs::remove_file(&input).ok();
    }

    /// Real angles produce measured statistics and a non-identity correction.
    ///
    /// The fabricated `ColorStats::new` defaults (mean 0.5 / std 0.1 /
    /// 6500 K) must not appear anywhere in the output.
    #[tokio::test]
    async fn test_color_match_measures_real_statistics() {
        /// Write a flat 4:2:0 Y4M clip.
        fn write_flat(path: &std::path::Path, y: u8, u: u8, v: u8) {
            let (w, h, frames) = (16usize, 16usize, 2usize);
            let mut buf = format!("YUV4MPEG2 W{w} H{h} F25:1 Ip A1:1 C420jpeg\n").into_bytes();
            for _ in 0..frames {
                buf.extend_from_slice(b"FRAME\n");
                buf.extend(std::iter::repeat_n(y, w * h));
                buf.extend(std::iter::repeat_n(u, (w / 2) * (h / 2)));
                buf.extend(std::iter::repeat_n(v, (w / 2) * (h / 2)));
            }
            std::fs::write(path, buf).expect("write y4m fixture");
        }

        let reference = temp_path("cm_ref.y4m");
        let input = temp_path("cm_dim.y4m");
        let output_dir = temp_path("cm_out");
        let _ = std::fs::remove_dir_all(&output_dir);
        write_flat(&reference, 200, 128, 128);
        write_flat(&input, 90, 128, 128);

        color_match(&reference, std::slice::from_ref(&input), &output_dir, false)
            .await
            .expect("color_match must succeed on real Y4M angles");

        // The report is named after the input clip's file stem.
        let stem = input
            .file_stem()
            .expect("stem")
            .to_string_lossy()
            .to_string();
        let report = output_dir.join(format!("{stem}.colormatch.json"));
        let body = std::fs::read_to_string(&report).expect("per-angle report must exist");
        let json: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");

        let mean = json["angle"]["mean_rgb"][0].as_f64().expect("mean_rgb[0]");
        let std_dev = json["angle"]["std_rgb"][0].as_f64().expect("std_rgb[0]");
        let gain = json["angle"]["correction_matrix"][0][0]
            .as_f64()
            .expect("correction gain");
        assert!(
            (mean - 0.5).abs() > 1e-3,
            "mean must be measured, not the ColorStats::new default 0.5"
        );
        assert!(
            std_dev < 1e-4,
            "a flat clip must measure zero spread, got {std_dev}"
        );
        assert!(
            gain > 1.05,
            "matching a dark angle to a bright reference must gain it up, got {gain}"
        );

        let _ = std::fs::remove_file(&reference);
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_dir_all(&output_dir);
    }

    // ── export_timeline: real EDL/XML content, not boilerplate ──────────────

    fn sample_switch_timeline() -> serde_json::Value {
        serde_json::json!({
            "cameras": ["/media/cam_a.mov", "/media/cam_b.mov"],
            "auto_switch": false,
            "min_shot_duration_secs": 2.0,
            "switch_points": [
                { "time": 0.0, "camera": 0 },
                { "time": 5.0, "camera": 1 },
            ],
            "output": "/media/out.json",
            "status": "switch_ready",
        })
    }

    #[test]
    fn test_build_multicam_edl_contains_real_camera_and_switch_data() {
        let timeline = sample_switch_timeline();
        let edl = build_multicam_edl(&timeline).expect("should build EDL from real timeline");
        assert!(
            edl.contains("cam_a.mov"),
            "must reference the real camera path"
        );
        assert!(
            edl.contains("cam_b.mov"),
            "must reference the real camera path"
        );
        assert!(edl.contains("CAM_A"), "must include a derived reel name");
        // Second switch point at 5.0s, default 25fps -> 00:00:05:00.
        assert!(
            edl.contains("00:00:05:00"),
            "must encode the real switch-point timecode, got:\n{edl}"
        );
    }

    #[test]
    fn test_build_multicam_edl_rejects_non_timeline_json() {
        let not_a_timeline = serde_json::json!({ "hello": "world" });
        let err = build_multicam_edl(&not_a_timeline).expect_err("must reject unrecognized JSON");
        assert!(err.to_string().contains("cameras"));
    }

    #[test]
    fn test_build_multicam_xml_contains_real_data() {
        let timeline = sample_switch_timeline();
        let xml = build_multicam_xml(&timeline, std::path::Path::new("/tmp/in.json"))
            .expect("should build XML from real timeline");
        assert!(xml.contains("cam_a.mov"));
        assert!(xml.contains("cam_b.mov"));
        assert!(xml.contains("time=\"5\"") || xml.contains("time=\"5.0\""));
        assert!(xml.contains("<multicam>") && xml.contains("</multicam>"));
    }

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("a & b < c"), "a &amp; b &lt; c");
    }

    #[test]
    fn test_reel_name_from_path() {
        assert_eq!(reel_name_from_path("/media/cam_a.mov", 0), "CAM_A");
        assert_eq!(reel_name_from_path("", 3), "CAM3");
    }

    #[test]
    fn test_seconds_to_edl_timecode() {
        assert_eq!(seconds_to_edl_timecode(0.0, 25.0), "00:00:00:00");
        assert_eq!(seconds_to_edl_timecode(61.0, 25.0), "00:01:01:00");
        assert_eq!(seconds_to_edl_timecode(5.0, 25.0), "00:00:05:00");
    }

    #[test]
    fn test_seconds_to_edl_timecode_invalid_fps_falls_back_consistently() {
        // fps <= 0 must fall back to a single consistent effective rate for
        // both the frame count and the frames-per-second modulus, not mix a
        // raw invalid `fps` in one place and a defaulted one in another.
        let zero = seconds_to_edl_timecode(2.0, 0.0);
        let neg = seconds_to_edl_timecode(2.0, -10.0);
        let explicit_default = seconds_to_edl_timecode(2.0, 25.0);
        assert_eq!(zero, explicit_default);
        assert_eq!(neg, explicit_default);
    }

    #[tokio::test]
    async fn test_export_timeline_edl_writes_real_content_not_boilerplate() {
        let timeline_path = temp_path("export_timeline.json");
        let output_path = temp_path("export_output.edl");
        let _ = std::fs::remove_file(&output_path);

        let timeline_json = serde_json::to_string_pretty(&sample_switch_timeline())
            .expect("serialize sample timeline");
        std::fs::write(&timeline_path, &timeline_json).expect("write sample timeline");

        export_timeline(&timeline_path, &output_path, "multicam_edl", false)
            .await
            .expect("export should succeed for a real timeline");

        let written = std::fs::read_to_string(&output_path).expect("read exported EDL");
        assert!(
            written.contains("cam_a.mov") && written.contains("cam_b.mov"),
            "exported EDL must contain the real camera paths, got:\n{written}"
        );
        assert!(
            !written.contains("Exported from OxiMedia multicam timeline"),
            "must not fall back to the old generic boilerplate line"
        );

        std::fs::remove_file(&timeline_path).ok();
        std::fs::remove_file(&output_path).ok();
    }

    #[tokio::test]
    async fn test_export_timeline_rejects_non_json_timeline_for_edl() {
        let timeline_path = temp_path("export_bad_timeline.json");
        let output_path = temp_path("export_bad_output.edl");
        std::fs::write(&timeline_path, b"not json at all").expect("write bad timeline");
        let _ = std::fs::remove_file(&output_path);

        let result = export_timeline(&timeline_path, &output_path, "multicam_edl", false).await;
        assert!(
            result.is_err(),
            "invalid JSON timeline must not export silently"
        );
        assert!(
            !output_path.exists(),
            "no output file should be written when the timeline can't be parsed"
        );

        std::fs::remove_file(&timeline_path).ok();
    }
}
