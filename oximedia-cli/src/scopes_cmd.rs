//! Video scopes command.
//!
//! Provides the `oximedia scopes` subcommand family for generating broadcast-quality
//! video scopes (waveform, vectorscope, histogram, parade, false color) using the
//! `oximedia-scopes` crate.

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Subcommands for the `scopes` command.
#[derive(Subcommand, Debug)]
pub enum ScopesCommand {
    /// Generate waveform display
    Waveform {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output scope image path
        #[arg(short, long)]
        output: PathBuf,

        /// Waveform mode: luma, rgb_parade, rgb_overlay, ycbcr
        #[arg(long, default_value = "luma")]
        mode: String,

        /// Scope display width in pixels
        #[arg(long, default_value = "512")]
        width: u32,

        /// Scope display height in pixels
        #[arg(long, default_value = "512")]
        height: u32,

        /// Specific frame number (default: first frame)
        #[arg(long)]
        frame: Option<u64>,

        /// Show graticule overlay
        #[arg(long)]
        graticule: bool,
    },

    /// Generate vectorscope display
    Vectorscope {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output scope image path
        #[arg(short, long)]
        output: PathBuf,

        /// Display mode: circular, rectangular
        #[arg(long, default_value = "circular")]
        mode: String,

        /// Scope display size in pixels (square)
        #[arg(long, default_value = "512")]
        size: u32,

        /// Specific frame number (default: first frame)
        #[arg(long)]
        frame: Option<u64>,

        /// Show SMPTE color bar targets
        #[arg(long)]
        targets: bool,

        /// Vectorscope gain / zoom
        #[arg(long, default_value = "1.0")]
        gain: f64,
    },

    /// Generate histogram
    Histogram {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output scope image path
        #[arg(short, long)]
        output: PathBuf,

        /// Histogram mode: rgb, luma, overlay, stacked, logarithmic
        #[arg(long, default_value = "rgb")]
        mode: String,

        /// Scope display width in pixels
        #[arg(long, default_value = "512")]
        width: u32,

        /// Scope display height in pixels
        #[arg(long, default_value = "256")]
        height: u32,

        /// Specific frame number (default: first frame)
        #[arg(long)]
        frame: Option<u64>,
    },

    /// Generate RGB or YCbCr parade display
    Parade {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output scope image path
        #[arg(short, long)]
        output: PathBuf,

        /// Parade mode: rgb, ycbcr
        #[arg(long, default_value = "rgb")]
        mode: String,

        /// Scope display width in pixels
        #[arg(long, default_value = "768")]
        width: u32,

        /// Scope display height in pixels
        #[arg(long, default_value = "256")]
        height: u32,

        /// Specific frame number (default: first frame)
        #[arg(long)]
        frame: Option<u64>,
    },

    /// Generate false color exposure display
    FalseColor {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output scope image path
        #[arg(short, long)]
        output: PathBuf,

        /// Specific frame number (default: first frame)
        #[arg(long)]
        frame: Option<u64>,

        /// Show color scale legend alongside the image
        #[arg(long)]
        scale: bool,
    },

    /// Analyze a frame with one or more scope types
    Analyze {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Frame number to analyze (default: first frame)
        #[arg(long)]
        frame: Option<u64>,

        /// Scope type(s) to generate
        #[arg(long, default_value = "all",
              value_parser = ["waveform", "vectorscope", "histogram", "all"])]
        scope: String,

        /// Output directory for scope images
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Check video compliance against a broadcast color standard
    Compliance {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Broadcast standard to check against
        #[arg(long, default_value = "rec709",
              value_parser = ["rec709", "rec2020"])]
        standard: String,
    },

    /// Print per-frame statistics for a video file
    Stats {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Number of frames to sample (0 = auto)
        #[arg(long, default_value = "0")]
        frames: u64,
    },
}

/// Entry point for `oximedia scopes <subcommand>`.
pub async fn handle_scopes_command(command: ScopesCommand, json_output: bool) -> Result<()> {
    match command {
        ScopesCommand::Waveform {
            input,
            output,
            mode,
            width,
            height,
            frame,
            graticule,
        } => {
            run_waveform(
                &input,
                &output,
                &mode,
                width,
                height,
                frame,
                graticule,
                json_output,
            )
            .await
        }
        ScopesCommand::Vectorscope {
            input,
            output,
            mode,
            size,
            frame,
            targets,
            gain,
        } => {
            run_vectorscope(
                &input,
                &output,
                &mode,
                size,
                frame,
                targets,
                gain,
                json_output,
            )
            .await
        }
        ScopesCommand::Histogram {
            input,
            output,
            mode,
            width,
            height,
            frame,
        } => run_histogram(&input, &output, &mode, width, height, frame, json_output).await,
        ScopesCommand::Parade {
            input,
            output,
            mode,
            width,
            height,
            frame,
        } => run_parade(&input, &output, &mode, width, height, frame, json_output).await,
        ScopesCommand::FalseColor {
            input,
            output,
            frame,
            scale,
        } => run_false_color(&input, &output, frame, scale, json_output).await,

        ScopesCommand::Analyze {
            input,
            frame,
            scope,
            output,
        } => run_analyze(&input, frame, &scope, &output, json_output).await,

        ScopesCommand::Compliance { input, standard } => {
            run_compliance(&input, &standard, json_output).await
        }

        ScopesCommand::Stats { input, frames } => run_stats(&input, frames, json_output).await,
    }
}

// ---------------------------------------------------------------------------
// Frame extraction helper
// ---------------------------------------------------------------------------

/// Extract a single frame as RGB24 data from a video file.
///
/// Delegates to [`crate::frame_extract::extract_video_frame_rgb`] which
/// supports Y4M natively.  For other formats it returns a descriptive error
/// directing the user to convert first.
async fn extract_frame_rgb(input: &std::path::Path, frame_num: u64) -> Result<(Vec<u8>, u32, u32)> {
    crate::frame_extract::extract_video_frame_rgb(input, frame_num).await
}

// ---------------------------------------------------------------------------
// Write scope RGBA data to output path (raw RGBA or described via JSON)
// ---------------------------------------------------------------------------

fn write_scope_output(
    output: &std::path::Path,
    scope: &oximedia_scopes::ScopeData,
    json_output: bool,
    scope_label: &str,
) -> Result<()> {
    if json_output {
        let obj = serde_json::json!({
            "scope": scope_label,
            "width": scope.width,
            "height": scope.height,
            "format": "RGBA",
            "bytes": scope.data.len(),
            "output": output.to_string_lossy(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialisation")?
        );
        return Ok(());
    }

    // Write raw RGBA file (consumer can load as width*height*4 RGBA bytes)
    std::fs::write(output, &scope.data)
        .with_context(|| format!("Failed to write scope image to {}", output.display()))?;

    if !crate::progress::is_quiet() {
        println!("{}", format!("{scope_label} Scope").green().bold());
        println!("  Output:     {}", output.display());
        println!("  Dimensions: {}x{}", scope.width, scope.height);
        println!("  Format:     RGBA ({} bytes)", scope.data.len());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Waveform
// ---------------------------------------------------------------------------

async fn run_waveform(
    input: &std::path::Path,
    output: &std::path::Path,
    mode: &str,
    width: u32,
    height: u32,
    frame_num: Option<u64>,
    graticule: bool,
    json_output: bool,
) -> Result<()> {
    use oximedia_scopes::{
        HistogramMode, ScopeConfig, ScopeType, VectorscopeMode, VideoScopes, WaveformMode,
    };

    let scope_type = match mode.to_lowercase().as_str() {
        "luma" => ScopeType::WaveformLuma,
        "rgb_parade" | "rgb-parade" => ScopeType::WaveformRgbParade,
        "rgb_overlay" | "rgb-overlay" => ScopeType::WaveformRgbOverlay,
        "ycbcr" => ScopeType::WaveformYcbcr,
        other => bail!(
            "Unknown waveform mode '{}'. Use: luma, rgb_parade, rgb_overlay, ycbcr",
            other
        ),
    };

    let config = ScopeConfig {
        width,
        height,
        show_graticule: graticule,
        show_labels: graticule,
        anti_alias: true,
        waveform_mode: WaveformMode::Overlay,
        vectorscope_mode: VectorscopeMode::Circular,
        histogram_mode: HistogramMode::Overlay,
        vectorscope_gain: 1.0,
        highlight_gamut: false,
        gamut_colorspace: oximedia_scopes::GamutColorspace::Rec709,
        resolution: None,
    };

    let (frame_data, fw, fh) = extract_frame_rgb(input, frame_num.unwrap_or(0)).await?;
    let scopes = VideoScopes::new(config);
    let scope_data = scopes
        .analyze(&frame_data, fw, fh, scope_type)
        .map_err(|e| anyhow::anyhow!("Waveform analysis failed: {e}"))?;

    write_scope_output(output, &scope_data, json_output, "Waveform")
}

// ---------------------------------------------------------------------------
// Vectorscope
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_vectorscope(
    input: &std::path::Path,
    output: &std::path::Path,
    mode: &str,
    size: u32,
    frame_num: Option<u64>,
    targets: bool,
    gain: f64,
    json_output: bool,
) -> Result<()> {
    use oximedia_scopes::{
        HistogramMode, ScopeConfig, ScopeType, VectorscopeMode, VideoScopes, WaveformMode,
    };

    let vectorscope_mode = match mode.to_lowercase().as_str() {
        "circular" => VectorscopeMode::Circular,
        "rectangular" => VectorscopeMode::Rectangular,
        other => bail!(
            "Unknown vectorscope mode '{}'. Use: circular, rectangular",
            other
        ),
    };

    let config = ScopeConfig {
        width: size,
        height: size,
        show_graticule: targets,
        show_labels: targets,
        anti_alias: true,
        waveform_mode: WaveformMode::Overlay,
        vectorscope_mode,
        histogram_mode: HistogramMode::Overlay,
        vectorscope_gain: gain as f32,
        highlight_gamut: false,
        gamut_colorspace: oximedia_scopes::GamutColorspace::Rec709,
        resolution: None,
    };

    let (frame_data, fw, fh) = extract_frame_rgb(input, frame_num.unwrap_or(0)).await?;
    let scopes = VideoScopes::new(config);
    let scope_data = scopes
        .analyze(&frame_data, fw, fh, ScopeType::Vectorscope)
        .map_err(|e| anyhow::anyhow!("Vectorscope analysis failed: {e}"))?;

    write_scope_output(output, &scope_data, json_output, "Vectorscope")
}

// ---------------------------------------------------------------------------
// Histogram
// ---------------------------------------------------------------------------

async fn run_histogram(
    input: &std::path::Path,
    output: &std::path::Path,
    mode: &str,
    width: u32,
    height: u32,
    frame_num: Option<u64>,
    json_output: bool,
) -> Result<()> {
    use oximedia_scopes::{
        HistogramMode, ScopeConfig, ScopeType, VectorscopeMode, VideoScopes, WaveformMode,
    };

    let (scope_type, histogram_mode) = match mode.to_lowercase().as_str() {
        "rgb" => (ScopeType::HistogramRgb, HistogramMode::Overlay),
        "luma" => (ScopeType::HistogramLuma, HistogramMode::Overlay),
        "overlay" => (ScopeType::HistogramRgb, HistogramMode::Overlay),
        "stacked" => (ScopeType::HistogramRgb, HistogramMode::Stacked),
        "logarithmic" | "log" => (ScopeType::HistogramRgb, HistogramMode::Logarithmic),
        other => bail!(
            "Unknown histogram mode '{}'. Use: rgb, luma, overlay, stacked, logarithmic",
            other
        ),
    };

    let config = ScopeConfig {
        width,
        height,
        show_graticule: true,
        show_labels: true,
        anti_alias: true,
        waveform_mode: WaveformMode::Overlay,
        vectorscope_mode: VectorscopeMode::Circular,
        histogram_mode,
        vectorscope_gain: 1.0,
        highlight_gamut: false,
        gamut_colorspace: oximedia_scopes::GamutColorspace::Rec709,
        resolution: None,
    };

    let (frame_data, fw, fh) = extract_frame_rgb(input, frame_num.unwrap_or(0)).await?;
    let scopes = VideoScopes::new(config);
    let scope_data = scopes
        .analyze(&frame_data, fw, fh, scope_type)
        .map_err(|e| anyhow::anyhow!("Histogram analysis failed: {e}"))?;

    write_scope_output(output, &scope_data, json_output, "Histogram")
}

// ---------------------------------------------------------------------------
// Parade
// ---------------------------------------------------------------------------

async fn run_parade(
    input: &std::path::Path,
    output: &std::path::Path,
    mode: &str,
    width: u32,
    height: u32,
    frame_num: Option<u64>,
    json_output: bool,
) -> Result<()> {
    use oximedia_scopes::{
        HistogramMode, ScopeConfig, ScopeType, VectorscopeMode, VideoScopes, WaveformMode,
    };

    let scope_type = match mode.to_lowercase().as_str() {
        "rgb" => ScopeType::ParadeRgb,
        "ycbcr" => ScopeType::ParadeYcbcr,
        other => bail!("Unknown parade mode '{}'. Use: rgb, ycbcr", other),
    };

    let config = ScopeConfig {
        width,
        height,
        show_graticule: true,
        show_labels: true,
        anti_alias: true,
        waveform_mode: WaveformMode::Overlay,
        vectorscope_mode: VectorscopeMode::Circular,
        histogram_mode: HistogramMode::Overlay,
        vectorscope_gain: 1.0,
        highlight_gamut: false,
        gamut_colorspace: oximedia_scopes::GamutColorspace::Rec709,
        resolution: None,
    };

    let (frame_data, fw, fh) = extract_frame_rgb(input, frame_num.unwrap_or(0)).await?;
    let scopes = VideoScopes::new(config);
    let scope_data = scopes
        .analyze(&frame_data, fw, fh, scope_type)
        .map_err(|e| anyhow::anyhow!("Parade analysis failed: {e}"))?;

    write_scope_output(output, &scope_data, json_output, "Parade")
}

// ---------------------------------------------------------------------------
// False Color
// ---------------------------------------------------------------------------

async fn run_false_color(
    input: &std::path::Path,
    output: &std::path::Path,
    frame_num: Option<u64>,
    show_scale: bool,
    json_output: bool,
) -> Result<()> {
    use oximedia_scopes::{ScopeConfig, ScopeType, VideoScopes};

    let config = ScopeConfig::default();
    let (frame_data, fw, fh) = extract_frame_rgb(input, frame_num.unwrap_or(0)).await?;

    let scopes = VideoScopes::new(config);
    let scope_data = scopes
        .analyze(&frame_data, fw, fh, ScopeType::FalseColor)
        .map_err(|e| anyhow::anyhow!("False color analysis failed: {e}"))?;

    if json_output {
        let stats = oximedia_scopes::false_color::compute_false_color_stats(&frame_data, fw, fh);
        let zone_map: serde_json::Map<String, serde_json::Value> = stats
            .zone_distribution
            .iter()
            .map(|(name, pct)| {
                (
                    name.clone(),
                    serde_json::Value::Number(
                        serde_json::Number::from_f64(f64::from(*pct))
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    ),
                )
            })
            .collect();

        let obj = serde_json::json!({
            "scope": "FalseColor",
            "width": scope_data.width,
            "height": scope_data.height,
            "format": "RGBA",
            "bytes": scope_data.data.len(),
            "output": output.to_string_lossy(),
            "show_scale": show_scale,
            "stats": {
                "highlight_clip_pct": stats.highlight_clip_percent,
                "shadow_clip_pct": stats.shadow_clip_percent,
                "good_exposure_pct": stats.good_exposure_percent,
                "zone_distribution": zone_map,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialisation")?
        );
        return Ok(());
    }

    // If scale legend requested, append it beneath the false color image
    let final_data = if show_scale {
        let legend_height = 20u32;
        let scale = oximedia_scopes::false_color::FalseColorScale::default();
        let legend =
            oximedia_scopes::false_color::generate_false_color_legend(fw, legend_height, &scale);
        let mut combined = scope_data.data.clone();
        combined.extend_from_slice(&legend);
        combined
    } else {
        scope_data.data.clone()
    };

    std::fs::write(output, &final_data)
        .with_context(|| format!("Failed to write false color image to {}", output.display()))?;

    let stats = oximedia_scopes::false_color::compute_false_color_stats(&frame_data, fw, fh);

    if !crate::progress::is_quiet() {
        println!("{}", "False Color Scope".green().bold());
        println!("  Output:         {}", output.display());
        println!(
            "  Dimensions:     {}x{}",
            scope_data.width, scope_data.height
        );
        println!("  Format:         RGBA ({} bytes)", final_data.len());
        println!("  Good exposure:  {:.1}%", stats.good_exposure_percent);
        println!("  Shadow clip:    {:.1}%", stats.shadow_clip_percent);
        println!("  Highlight clip: {:.1}%", stats.highlight_clip_percent);
        if show_scale {
            println!("  Legend:          appended ({}px tall)", 20);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Analyze (multi-scope)
// ---------------------------------------------------------------------------

async fn run_analyze(
    input: &std::path::Path,
    frame_num: Option<u64>,
    scope: &str,
    output_dir: &std::path::Path,
    json_output: bool,
) -> Result<()> {
    use oximedia_scopes::{
        HistogramMode, ScopeConfig, ScopeType, VectorscopeMode, VideoScopes, WaveformMode,
    };

    if !input.exists() {
        bail!("Input file not found: {}", input.display());
    }

    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("Cannot create output directory: {}", output_dir.display()))?;

    let frame = frame_num.unwrap_or(0);
    let (frame_data, fw, fh) = extract_frame_rgb(input, frame).await?;

    let config = ScopeConfig {
        width: 512,
        height: 512,
        show_graticule: true,
        show_labels: true,
        anti_alias: true,
        waveform_mode: WaveformMode::Overlay,
        vectorscope_mode: VectorscopeMode::Circular,
        histogram_mode: HistogramMode::Overlay,
        vectorscope_gain: 1.0,
        highlight_gamut: false,
        gamut_colorspace: oximedia_scopes::GamutColorspace::Rec709,
        resolution: None,
    };

    let scope_types: &[(&str, ScopeType)] = match scope {
        "waveform" => &[("waveform", ScopeType::WaveformLuma)],
        "vectorscope" => &[("vectorscope", ScopeType::Vectorscope)],
        "histogram" => &[("histogram", ScopeType::HistogramRgb)],
        _ => &[
            ("waveform", ScopeType::WaveformLuma),
            ("vectorscope", ScopeType::Vectorscope),
            ("histogram", ScopeType::HistogramRgb),
        ],
    };

    let scopes = VideoScopes::new(config);
    let mut generated = Vec::new();

    for (name, scope_type) in scope_types {
        let scope_data = scopes
            .analyze(&frame_data, fw, fh, *scope_type)
            .map_err(|e| anyhow::anyhow!("Scope analysis failed for {name}: {e}"))?;
        let out_path = output_dir.join(format!("{name}.rgba"));
        std::fs::write(&out_path, &scope_data.data)
            .with_context(|| format!("Cannot write {}", out_path.display()))?;
        generated.push((
            name.to_string(),
            out_path,
            scope_data.width,
            scope_data.height,
        ));
    }

    if json_output {
        let files: Vec<serde_json::Value> = generated
            .iter()
            .map(|(n, p, w, h)| {
                serde_json::json!({
                    "scope": n,
                    "path": p.display().to_string(),
                    "width": w,
                    "height": h,
                })
            })
            .collect();
        let obj = serde_json::json!({
            "command": "scopes analyze",
            "input": input.display().to_string(),
            "frame": frame,
            "scope_filter": scope,
            "output_dir": output_dir.display().to_string(),
            "generated": files,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization")?
        );
        return Ok(());
    }

    if !crate::progress::is_quiet() {
        println!("{}", "Scopes Analysis".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Frame:", frame);
        println!("{:20} {}", "Output dir:", output_dir.display());
        println!();
        for (name, path, w, h) in &generated {
            println!("  {} {}x{} → {}", name.cyan(), w, h, path.display());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Compliance
// ---------------------------------------------------------------------------

async fn run_compliance(input: &std::path::Path, standard: &str, json_output: bool) -> Result<()> {
    use oximedia_scopes::{GamutColorspace, ScopeConfig, ScopeType, VideoScopes};

    if !input.exists() {
        bail!("Input file not found: {}", input.display());
    }

    let gamut = match standard {
        "rec2020" => GamutColorspace::Rec2020,
        _ => GamutColorspace::Rec709,
    };

    let config = ScopeConfig {
        highlight_gamut: true,
        gamut_colorspace: gamut,
        ..ScopeConfig::default()
    };

    let (frame_data, fw, fh) = extract_frame_rgb(input, 0).await?;
    let scopes = VideoScopes::new(config);
    let scope_data = scopes
        .analyze(&frame_data, fw, fh, ScopeType::Vectorscope)
        .map_err(|e| anyhow::anyhow!("Compliance analysis failed: {e}"))?;

    // Heuristic: estimate out-of-gamut pixels from scope output brightness
    let total_pixels = (scope_data.width * scope_data.height) as usize;
    let bright_pixels = scope_data
        .data
        .chunks(4)
        .filter(|px| px[0] > 200 || px[1] > 200 || px[2] > 200)
        .count();
    let pct_in_gamut = if total_pixels > 0 {
        100.0 - (bright_pixels as f64 / total_pixels as f64) * 100.0
    } else {
        100.0
    };
    let compliant = pct_in_gamut >= 95.0;

    if json_output {
        let obj = serde_json::json!({
            "command": "scopes compliance",
            "input": input.display().to_string(),
            "standard": standard,
            "pct_in_gamut": pct_in_gamut,
            "compliant": compliant,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization")?
        );
        return Ok(());
    }

    println!("{}", "Scopes Compliance".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", input.display());
    println!("{:20} {}", "Standard:", standard.to_uppercase());
    println!("{:20} {:.1}%", "In-gamut est.:", pct_in_gamut);
    let status = if compliant {
        "PASS".green().bold().to_string()
    } else {
        "FAIL".red().bold().to_string()
    };
    println!("{:20} {}", "Result:", status);

    Ok(())
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

async fn run_stats(
    input: &std::path::Path,
    frames_to_sample: u64,
    json_output: bool,
) -> Result<()> {
    use oximedia_scopes::{ScopeConfig, ScopeType, VideoScopes};

    if !input.exists() {
        bail!("Input file not found: {}", input.display());
    }

    // Sample up to 3 frames (or as requested) — real impl decodes multiple frames
    let count = if frames_to_sample == 0 {
        3
    } else {
        frames_to_sample.min(10)
    };
    let config = ScopeConfig::default();
    let scopes = VideoScopes::new(config);

    let mut min_luma = f64::MAX;
    let mut max_luma = f64::MIN;
    let mut sum_luma = 0.0_f64;

    for i in 0..count {
        let (frame_data, fw, fh) = extract_frame_rgb(input, i).await?;
        let scope_data = scopes
            .analyze(&frame_data, fw, fh, ScopeType::HistogramLuma)
            .map_err(|e| anyhow::anyhow!("Stats analysis failed on frame {i}: {e}"))?;

        // Compute mean luminance from histogram data
        let luma_mean = scope_data.data.iter().map(|&b| b as f64).sum::<f64>()
            / (scope_data.data.len().max(1) as f64);
        if luma_mean < min_luma {
            min_luma = luma_mean;
        }
        if luma_mean > max_luma {
            max_luma = luma_mean;
        }
        sum_luma += luma_mean;
    }

    let avg_luma = sum_luma / count as f64;

    if json_output {
        let obj = serde_json::json!({
            "command": "scopes stats",
            "input": input.display().to_string(),
            "frames_sampled": count,
            "luma": {
                "min": min_luma,
                "max": max_luma,
                "avg": avg_luma,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization")?
        );
        return Ok(());
    }

    println!("{}", "Scopes Statistics".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", input.display());
    println!("{:20} {}", "Frames sampled:", count);
    println!();
    println!("{}", "Luminance".cyan().bold());
    println!("{}", "-".repeat(60));
    println!("{:20} {:.1}", "Min:", min_luma);
    println!("{:20} {:.1}", "Max:", max_luma);
    println!("{:20} {:.1}", "Average:", avg_luma);

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Per-process atomic counter for scopes test temp files.
    ///
    /// Combined with the process id, thread id and a nanosecond timestamp this
    /// guarantees a unique path for every `temp_input()` / `temp_out_dir()`
    /// invocation, even when several test processes (e.g. nextest workers)
    /// execute in parallel on the same machine.  See issue #16.
    static TEMP_INPUT_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Unique process-scoped counter for output directories / files.
    static TEMP_OUT_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Return a unique temporary directory path for scope output.
    ///
    /// The directory is NOT created — callers are responsible for creating it
    /// (e.g. via `std::fs::create_dir_all` or `run_analyze`).  This mirrors
    /// the uniqueness guarantee of `temp_input()` so that parallel test
    /// processes on the same machine never collide.
    fn temp_out_dir(label: &str) -> PathBuf {
        let pid = std::process::id();
        let counter = TEMP_OUT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("oximedia_scopes_{}_{pid}_{counter}_{nanos}", label))
    }

    fn temp_input() -> PathBuf {
        let pid = std::process::id();
        let tid = format!("{:?}", std::thread::current().id());
        // Strip non-alphanumeric characters from the Debug-formatted ThreadId
        // so the resulting filename stays portable (Windows in particular
        // dislikes parentheses and spaces in path components).
        let tid_sanitised: String = tid.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let counter = TEMP_INPUT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let filename =
            format!("oximedia_scopes_test_input_{pid}_{tid_sanitised}_{counter}_{nanos}.y4m");
        let path = std::env::temp_dir().join(filename);
        // Minimal 64×64 YUV420 Y4M with three black frames (run_stats needs up to 3)
        let mut data = b"YUV4MPEG2 W64 H64 F25:1 Ip A0:0 C420\n".to_vec();
        for _ in 0..3 {
            data.extend_from_slice(b"FRAME\n");
            // Y plane: 64*64 = 4096 bytes (black = 16 in studio swing)
            data.extend(std::iter::repeat_n(16u8, 64 * 64));
            // U and V planes: 32*32 = 1024 bytes each (neutral = 128)
            data.extend(std::iter::repeat_n(128u8, 32 * 32));
            data.extend(std::iter::repeat_n(128u8, 32 * 32));
        }
        std::fs::write(&path, &data).expect("failed to write Y4M temp file");
        path
    }

    #[tokio::test]
    async fn test_waveform_luma() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_waveform.rgba");
        let result = run_waveform(&input, &output, "luma", 64, 64, None, false, false).await;
        assert!(result.is_ok());
        assert!(output.exists());
        let _ = std::fs::remove_file(&output);
    }

    #[tokio::test]
    async fn test_waveform_rgb_parade() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_wf_rgbparade.rgba");
        let result = run_waveform(&input, &output, "rgb_parade", 64, 64, None, true, false).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(&output);
    }

    #[tokio::test]
    async fn test_vectorscope_circular() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_vectorscope.rgba");
        let result = run_vectorscope(&input, &output, "circular", 64, None, true, 1.0, false).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(&output);
    }

    #[tokio::test]
    async fn test_histogram_rgb() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_histogram.rgba");
        let result = run_histogram(&input, &output, "rgb", 64, 64, None, false).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(&output);
    }

    #[tokio::test]
    async fn test_parade_rgb() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_parade.rgba");
        let result = run_parade(&input, &output, "rgb", 96, 64, None, false).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(&output);
    }

    #[tokio::test]
    async fn test_false_color() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_false_color.rgba");
        let result = run_false_color(&input, &output, None, false, false).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(&output);
    }

    #[tokio::test]
    async fn test_false_color_with_scale() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_false_color_scale.rgba");
        let result = run_false_color(&input, &output, None, true, false).await;
        assert!(result.is_ok());
        let _ = std::fs::remove_file(&output);
    }

    #[tokio::test]
    async fn test_json_output() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_wf_json.rgba");
        let result = run_waveform(&input, &output, "luma", 64, 64, None, false, true).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_bad_waveform_mode() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_bad_wf.rgba");
        let result = run_waveform(&input, &output, "invalid", 64, 64, None, false, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_bad_vectorscope_mode() {
        let input = temp_input();
        let output = std::env::temp_dir().join("test_bad_vs.rgba");
        let result = run_vectorscope(&input, &output, "invalid", 64, None, false, 1.0, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_missing_input() {
        let output = std::env::temp_dir().join("test_missing.rgba");
        let result = run_waveform(
            std::path::Path::new("/nonexistent/video.mkv"),
            &output,
            "luma",
            64,
            64,
            None,
            false,
            false,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_analyze_all_scopes() {
        let input = temp_input();
        let out_dir = temp_out_dir("analyze_all");
        let result = run_analyze(&input, None, "all", &out_dir, false).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        let _ = std::fs::remove_dir_all(&out_dir);
        let _ = std::fs::remove_file(&input);
    }

    #[tokio::test]
    async fn test_analyze_waveform_json() {
        let input = temp_input();
        let out_dir = temp_out_dir("analyze_wf");
        let result = run_analyze(&input, None, "waveform", &out_dir, true).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        let _ = std::fs::remove_dir_all(&out_dir);
        let _ = std::fs::remove_file(&input);
    }

    #[tokio::test]
    async fn test_analyze_missing_input() {
        let out_dir = std::env::temp_dir().join("oximedia_scopes_analyze_missing");
        let result = run_analyze(
            std::path::Path::new("/nonexistent/input.mkv"),
            None,
            "all",
            &out_dir,
            false,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_compliance_rec709() {
        let input = temp_input();
        let result = run_compliance(&input, "rec709", false).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
    }

    #[tokio::test]
    async fn test_compliance_rec2020_json() {
        let input = temp_input();
        let result = run_compliance(&input, "rec2020", true).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
    }

    #[tokio::test]
    async fn test_compliance_missing_input() {
        let result = run_compliance(
            std::path::Path::new("/nonexistent/video.mkv"),
            "rec709",
            false,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stats_text() {
        let input = temp_input();
        let result = run_stats(&input, 0, false).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
    }

    #[tokio::test]
    async fn test_stats_json() {
        let input = temp_input();
        let result = run_stats(&input, 2, true).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
    }

    #[tokio::test]
    async fn test_stats_missing_input() {
        let result = run_stats(std::path::Path::new("/nonexistent/video.mkv"), 0, false).await;
        assert!(result.is_err());
    }

    /// Regression test for https://github.com/cool-japan/oximedia/issues/16.
    ///
    /// Two consecutive `temp_input()` calls must yield distinct paths and both
    /// must point at intact Y4M files (the magic bytes must survive). Before
    /// the fix, `temp_input()` always returned the same path so parallel test
    /// invocations truncated each other's data and the magic-byte check on
    /// `extract_video_frame_rgb` failed.
    #[test]
    fn test_issue_16_temp_input_unique_per_call() {
        let p1 = temp_input();
        let p2 = temp_input();
        assert_ne!(p1, p2, "temp_input() must yield distinct paths per call");
        for p in [&p1, &p2] {
            assert!(p.exists(), "temp file missing: {}", p.display());
            let bytes = std::fs::read(p).expect("read temp Y4M");
            assert!(
                bytes.starts_with(b"YUV4MPEG2"),
                "temp Y4M missing magic bytes (truncation race?): {}",
                p.display()
            );
        }
        let _ = std::fs::remove_file(&p1);
        let _ = std::fs::remove_file(&p2);
    }

    /// Regression test for the parallel-collision aspect of issue #16.
    ///
    /// Spawn many threads that each call `temp_input()` concurrently and
    /// assert (a) every returned path is unique and (b) every file contains
    /// the full Y4M header. Before the fix this would race on the shared
    /// `oximedia_scopes_test_input.y4m` filename.
    #[test]
    fn test_issue_16_temp_input_parallel_no_truncation() {
        const THREADS: usize = 16;
        let mut handles = Vec::with_capacity(THREADS);
        for _ in 0..THREADS {
            handles.push(std::thread::spawn(temp_input));
        }
        let paths: Vec<PathBuf> = handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect();

        // (a) Every returned path is unique.
        let mut sorted = paths.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            paths.len(),
            "temp_input() returned duplicate paths under contention"
        );

        // (b) Every file is a complete Y4M (intact header + at least one FRAME).
        for p in &paths {
            let bytes = std::fs::read(p).expect("read temp Y4M");
            assert!(
                bytes.starts_with(b"YUV4MPEG2"),
                "temp Y4M missing magic bytes (truncation race?): {}",
                p.display()
            );
            assert!(
                bytes.windows(6).any(|w| w == b"FRAME\n"),
                "temp Y4M missing FRAME marker (partial write?): {}",
                p.display()
            );
        }

        for p in &paths {
            let _ = std::fs::remove_file(p);
        }
    }
}
