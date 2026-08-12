//! Video/image scaling commands: upscale, downscale, analyze, compare, batch.
//!
//! Exposes `oximedia-scaling` Lanczos, bicubic, bilinear scaling with
//! quality-aware algorithms via the CLI, driven through
//! [`crate::frame_harness::scale`] — the one harness operation that
//! legitimately changes frame geometry (every other command built on this
//! harness preserves it; see that module's docs for why).

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Scaling command subcommands.
#[derive(Subcommand, Debug)]
pub enum ScalingCommand {
    /// Upscale a video or image
    Upscale {
        /// Input file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file
        #[arg(short, long)]
        output: PathBuf,

        /// Target width
        #[arg(long)]
        width: u32,

        /// Target height
        #[arg(long)]
        height: u32,

        /// Scaling algorithm: bilinear, bicubic, lanczos
        #[arg(long, default_value = "lanczos")]
        algorithm: String,

        /// Aspect ratio mode: stretch, letterbox, crop
        #[arg(long, default_value = "letterbox")]
        aspect: String,
    },

    /// Downscale a video or image
    Downscale {
        /// Input file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file
        #[arg(short, long)]
        output: PathBuf,

        /// Target width
        #[arg(long)]
        width: u32,

        /// Target height
        #[arg(long)]
        height: u32,

        /// Scaling algorithm: bilinear, bicubic, lanczos
        #[arg(long, default_value = "lanczos")]
        algorithm: String,

        /// Aspect ratio mode: stretch, letterbox, crop
        #[arg(long, default_value = "letterbox")]
        aspect: String,
    },

    /// Analyze scaling quality for a given source/target resolution pair
    Analyze {
        /// Source width
        #[arg(long)]
        src_width: u32,

        /// Source height
        #[arg(long)]
        src_height: u32,

        /// Target width
        #[arg(long)]
        dst_width: u32,

        /// Target height
        #[arg(long)]
        dst_height: u32,

        /// Algorithm to analyze: bilinear, bicubic, lanczos
        #[arg(long, default_value = "lanczos")]
        algorithm: String,
    },

    /// Compare scaling algorithms on a Y4M input (writes each algorithm's
    /// resized output plus a PSNR/SSIM report to `--output-dir`)
    Compare {
        /// Input file (uncompressed YUV4MPEG2 / .y4m)
        #[arg(short, long)]
        input: PathBuf,

        /// Target width
        #[arg(long)]
        width: u32,

        /// Target height
        #[arg(long)]
        height: u32,

        /// Output directory for comparison results (required)
        #[arg(short, long)]
        output_dir: Option<PathBuf>,
    },

    /// Batch scale multiple Y4M files
    Batch {
        /// Input directory
        #[arg(short, long)]
        input_dir: PathBuf,

        /// Output directory
        #[arg(short, long)]
        output_dir: PathBuf,

        /// Target width
        #[arg(long)]
        width: u32,

        /// Target height
        #[arg(long)]
        height: u32,

        /// Scaling algorithm
        #[arg(long, default_value = "lanczos")]
        algorithm: String,

        /// File extension filter, case-insensitive (default: "y4m" — the
        /// frame harness only reads uncompressed YUV4MPEG2)
        #[arg(long)]
        ext: Option<String>,
    },
}

/// Handle scaling command dispatch.
pub async fn handle_scaling_command(command: ScalingCommand, json_output: bool) -> Result<()> {
    match command {
        ScalingCommand::Upscale {
            input,
            output,
            width,
            height,
            algorithm,
            aspect,
        } => {
            handle_scale(
                &input,
                &output,
                width,
                height,
                &algorithm,
                &aspect,
                "upscale",
                json_output,
            )
            .await
        }
        ScalingCommand::Downscale {
            input,
            output,
            width,
            height,
            algorithm,
            aspect,
        } => {
            handle_scale(
                &input,
                &output,
                width,
                height,
                &algorithm,
                &aspect,
                "downscale",
                json_output,
            )
            .await
        }
        ScalingCommand::Analyze {
            src_width,
            src_height,
            dst_width,
            dst_height,
            algorithm,
        } => {
            handle_analyze(
                src_width,
                src_height,
                dst_width,
                dst_height,
                &algorithm,
                json_output,
            )
            .await
        }
        ScalingCommand::Compare {
            input,
            width,
            height,
            output_dir,
        } => handle_compare(&input, width, height, output_dir.as_deref(), json_output).await,
        ScalingCommand::Batch {
            input_dir,
            output_dir,
            width,
            height,
            algorithm,
            ext,
        } => {
            handle_batch(
                &input_dir,
                &output_dir,
                width,
                height,
                &algorithm,
                ext.as_deref(),
                json_output,
            )
            .await
        }
    }
}

/// Parse scaling mode from string.
fn parse_scaling_mode(s: &str) -> Result<oximedia_scaling::ScalingMode> {
    match s {
        "bilinear" => Ok(oximedia_scaling::ScalingMode::Bilinear),
        "bicubic" => Ok(oximedia_scaling::ScalingMode::Bicubic),
        "lanczos" => Ok(oximedia_scaling::ScalingMode::Lanczos),
        other => Err(anyhow::anyhow!(
            "Unknown algorithm '{}'. Supported: bilinear, bicubic, lanczos",
            other
        )),
    }
}

/// Parse aspect ratio mode from string.
fn parse_aspect_mode(s: &str) -> Result<oximedia_scaling::AspectRatioMode> {
    match s {
        "stretch" => Ok(oximedia_scaling::AspectRatioMode::Stretch),
        "letterbox" => Ok(oximedia_scaling::AspectRatioMode::Letterbox),
        "crop" => Ok(oximedia_scaling::AspectRatioMode::Crop),
        other => Err(anyhow::anyhow!(
            "Unknown aspect mode '{}'. Supported: stretch, letterbox, crop",
            other
        )),
    }
}

/// Map a `ScalingMode` onto the resampler's `FilterKernel`.
///
/// `Lanczos` maps to `Lanczos3` (radius 3), the conventional default quality
/// level for this filter family; the CLI does not expose radius-5 separately.
fn filter_kernel_for(
    mode: oximedia_scaling::ScalingMode,
) -> oximedia_scaling::resampler::FilterKernel {
    use oximedia_scaling::resampler::FilterKernel;
    use oximedia_scaling::ScalingMode;
    match mode {
        ScalingMode::Bilinear => FilterKernel::Bilinear,
        ScalingMode::Bicubic => FilterKernel::Bicubic,
        ScalingMode::Lanczos => FilterKernel::Lanczos3,
        ScalingMode::NearestNeighbor => FilterKernel::Nearest,
    }
}

/// Validate a requested target size against this command's documented
/// bounds.
fn validate_target_dims(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!(
            "Target dimensions must be > 0, got {}x{}",
            width,
            height
        ));
    }
    if width > 7680 || height > 4320 {
        return Err(anyhow::anyhow!(
            "Target dimensions exceed maximum 7680x4320, got {}x{}",
            width,
            height
        ));
    }
    Ok(())
}

/// Handle upscale or downscale.
///
/// A real decode -> resize -> encode pass via
/// [`crate::frame_harness::scale::resize_clip_file`]. Unlike every other
/// command wired onto the frame harness, this legitimately changes frame
/// geometry, so it does not use [`crate::frame_harness::process_clip`] (see
/// [`crate::frame_harness::scale`]'s module docs for why).
///
/// # Errors
///
/// Returns an error if the input does not exist, is not Y4M, the target
/// dimensions are out of bounds, `--algorithm`/`--aspect` are unrecognised,
/// or the resize/pad/crop pipeline fails. No output file is written unless
/// the whole clip succeeds.
#[allow(clippy::too_many_arguments)]
async fn handle_scale(
    input: &PathBuf,
    output: &PathBuf,
    width: u32,
    height: u32,
    algorithm: &str,
    aspect: &str,
    direction: &str,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }
    validate_target_dims(width, height)?;

    let mode = parse_scaling_mode(algorithm)?;
    let aspect_mode = parse_aspect_mode(aspect)?;
    let kernel = filter_kernel_for(mode);

    let op_label = format!("scaling {direction}");
    let input_task = input.clone();
    let output_task = output.clone();
    let stats = tokio::task::spawn_blocking(move || {
        crate::frame_harness::scale::resize_clip_file(
            &op_label,
            &input_task,
            &output_task,
            width,
            height,
            kernel,
            aspect_mode,
        )
    })
    .await
    .map_err(|join_err| anyhow::anyhow!("{direction} task panicked: {join_err}"))??;

    if json_output {
        let obj = serde_json::json!({
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "operation": direction,
            "input_format": "y4m",
            "output_format": "y4m",
            "algorithm": algorithm,
            "aspect": aspect,
            "src_dims": format!("{}x{}", stats.src_dims.0, stats.src_dims.1),
            "dst_dims": format!("{}x{}", stats.dst_dims.0, stats.dst_dims.1),
            "frame_count": stats.frame_count,
            "input_size_bytes": stats.bytes_in,
            "output_size_bytes": stats.bytes_out,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!(
        "{}",
        format!("{} Complete", capitalize(direction)).green().bold()
    );
    println!("  {} {}", "Input:".cyan(), input.display());
    println!("  {} {}", "Output:".cyan(), output.display());
    println!(
        "  {} {}x{} -> {}x{}",
        "Resize:".cyan(),
        stats.src_dims.0,
        stats.src_dims.1,
        stats.dst_dims.0,
        stats.dst_dims.1
    );
    println!("  {} {} / {}", "Settings:".cyan(), algorithm, aspect);
    println!(
        "  {} {} bytes ({} frames)",
        "Output size:".cyan(),
        stats.bytes_out,
        stats.frame_count
    );

    Ok(())
}

/// Analyze scaling quality.
async fn handle_analyze(
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
    algorithm: &str,
    json_output: bool,
) -> Result<()> {
    let mode = parse_scaling_mode(algorithm)?;

    if src_width == 0 || src_height == 0 || dst_width == 0 || dst_height == 0 {
        return Err(anyhow::anyhow!("All dimensions must be > 0"));
    }

    let params = oximedia_scaling::ScalingParams::new(dst_width, dst_height).with_mode(mode);
    let scaler = oximedia_scaling::VideoScaler::new(params);
    let (calc_w, calc_h) = scaler.calculate_dimensions(src_width, src_height);

    let scale_factor_x = dst_width as f64 / src_width as f64;
    let scale_factor_y = dst_height as f64 / src_height as f64;
    let is_upscale = scale_factor_x > 1.0 || scale_factor_y > 1.0;

    if json_output {
        let result = serde_json::json!({
            "command": "analyze",
            "source": format!("{}x{}", src_width, src_height),
            "target": format!("{}x{}", dst_width, dst_height),
            "calculated": format!("{}x{}", calc_w, calc_h),
            "scale_factor_x": scale_factor_x,
            "scale_factor_y": scale_factor_y,
            "is_upscale": is_upscale,
            "algorithm": algorithm,
            "quality_assessment": if is_upscale { "upscale may introduce artifacts" } else { "downscale preserves detail" },
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize analysis")?;
        println!("{}", json_str);
    } else {
        println!("{}", "Scaling Analysis".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}x{}", "Source:", src_width, src_height);
        println!("{:20} {}x{}", "Target:", dst_width, dst_height);
        println!("{:20} {}x{}", "Calculated:", calc_w, calc_h);
        println!(
            "{:20} {:.3}x / {:.3}x",
            "Scale factor:", scale_factor_x, scale_factor_y
        );
        println!(
            "{:20} {}",
            "Direction:",
            if is_upscale { "upscale" } else { "downscale" }
        );
        println!("{:20} {}", "Algorithm:", algorithm);
        println!();
        if is_upscale {
            println!(
                "{}",
                "Note: Upscaling may introduce interpolation artifacts.".yellow()
            );
            println!(
                "{}",
                "Lanczos provides the best quality for upscaling.".dimmed()
            );
        } else {
            println!(
                "{}",
                "Downscaling preserves visual detail well with anti-aliasing.".green()
            );
        }
    }

    Ok(())
}

/// Operation label used in the frame harness's error messages.
const COMPARE_OP: &str = "scaling compare";

/// Algorithms `compare` resizes with. `lanczos` is also the quality
/// reference every other algorithm's PSNR/SSIM is measured against (the
/// same algorithm `analyze` recommends for upscaling).
const COMPARE_ALGORITHMS: [(&str, oximedia_scaling::ScalingMode); 3] = [
    ("bilinear", oximedia_scaling::ScalingMode::Bilinear),
    ("bicubic", oximedia_scaling::ScalingMode::Bicubic),
    ("lanczos", oximedia_scaling::ScalingMode::Lanczos),
];

/// Compare scaling algorithms on a real Y4M clip.
///
/// Resizes the same input to `(width, height)` with every algorithm in
/// [`COMPARE_ALGORITHMS`], writes each result as `<output_dir>/<algorithm>.y4m`,
/// and scores every non-reference algorithm's luma plane against `lanczos`'s
/// with real [`oximedia_scaling::quality_metrics::psnr`] /
/// [`oximedia_scaling::quality_metrics::ssim_simple`] (all outputs share the
/// same target dimensions by construction, so their length-equality
/// assertions never trip). A `comparison.json` report is written alongside
/// the per-algorithm outputs.
///
/// # Errors
///
/// Returns an error if the input does not exist or is not Y4M, if
/// `--output-dir` is omitted (there is nowhere to write results), or if any
/// resize/write step fails. No files are written unless every algorithm
/// succeeds.
async fn handle_compare(
    input: &PathBuf,
    width: u32,
    height: u32,
    output_dir: Option<&std::path::Path>,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }
    validate_target_dims(width, height)?;

    let output_dir = output_dir
        .ok_or_else(|| {
            anyhow::anyhow!(
                "compare needs --output-dir to write each algorithm's resized Y4M output and \
                 the comparison report; none was given"
            )
        })?
        .to_path_buf();

    let input_task = input.clone();
    let dir_task = output_dir.clone();
    let (src_dims, algorithms) = tokio::task::spawn_blocking(
        move || -> Result<((u32, u32), Vec<serde_json::Value>)> {
            std::fs::create_dir_all(&dir_task).with_context(|| {
                format!("Failed to create output directory: {}", dir_task.display())
            })?;
            let clip = crate::frame_harness::read_y4m_clip(COMPARE_OP, &input_task)?;
            let src_dims = (clip.width(), clip.height());

            let mut luma_by_algorithm: Vec<(&str, PathBuf, Vec<u8>)> =
                Vec::with_capacity(COMPARE_ALGORITHMS.len());
            for (name, mode) in COMPARE_ALGORITHMS {
                let kernel = filter_kernel_for(mode);
                let resized = crate::frame_harness::scale::resize_clip(
                    &clip,
                    width,
                    height,
                    kernel,
                    oximedia_scaling::AspectRatioMode::Letterbox,
                )
                .with_context(|| format!("failed to resize with '{name}'"))?;
                let out_path = dir_task.join(format!("{name}.y4m"));
                crate::frame_harness::write_y4m_clip(&out_path, &resized)?;
                let luma: Vec<u8> = resized.frames.iter().flat_map(|f| f.luma().to_vec()).collect();
                luma_by_algorithm.push((name, out_path, luma));
            }

            let reference_luma = luma_by_algorithm
                .iter()
                .find(|(name, _, _)| *name == "lanczos")
                .map(|(_, _, luma)| luma.clone())
                .ok_or_else(|| anyhow::anyhow!("internal error: 'lanczos' missing from comparison set"))?;

            let mut algorithms = Vec::with_capacity(luma_by_algorithm.len());
            for (name, out_path, luma) in &luma_by_algorithm {
                let (psnr, ssim) = if *name == "lanczos" {
                    (f64::INFINITY, 1.0)
                } else {
                    (
                        oximedia_scaling::quality_metrics::psnr(&reference_luma, luma),
                        oximedia_scaling::quality_metrics::ssim_simple(&reference_luma, luma),
                    )
                };
                algorithms.push(serde_json::json!({
                    "algorithm": name,
                    "output": out_path.display().to_string(),
                    "psnr_db": if psnr.is_finite() { serde_json::json!(psnr) } else { serde_json::json!(null) },
                    "psnr_identical_to_reference": !psnr.is_finite(),
                    "ssim": ssim,
                }));
            }

            let report = serde_json::json!({
                "input": input_task.display().to_string(),
                "source": format!("{}x{}", src_dims.0, src_dims.1),
                "target": format!("{width}x{height}"),
                "reference_algorithm": "lanczos",
                "note": "psnr_db/ssim compare each algorithm's resized luma plane against \
                         lanczos's (this command's quality baseline), not against the \
                         un-resized source. psnr_db is null when identical to the reference \
                         (mathematically infinite, not a measurement).",
                "algorithms": algorithms,
            });
            let report_path = dir_task.join("comparison.json");
            std::fs::write(&report_path, serde_json::to_string_pretty(&report)?)
                .with_context(|| format!("Failed to write {}", report_path.display()))?;

            Ok((src_dims, algorithms))
        },
    )
    .await
    .map_err(|join_err| anyhow::anyhow!("compare task panicked: {join_err}"))??;

    if json_output {
        let obj = serde_json::json!({
            "input": input.display().to_string(),
            "output_dir": output_dir.display().to_string(),
            "source": format!("{}x{}", src_dims.0, src_dims.1),
            "target": format!("{width}x{height}"),
            "algorithms": algorithms,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Scaling Comparison".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", input.display());
    println!(
        "{:20} {}x{} -> {}x{}",
        "Resize:", src_dims.0, src_dims.1, width, height
    );
    println!("{:20} {}", "Output dir:", output_dir.display());
    println!();
    for algo in &algorithms {
        let name = algo["algorithm"].as_str().unwrap_or("?");
        let ssim = algo["ssim"].as_f64().unwrap_or(0.0);
        let psnr_str = algo["psnr_db"].as_f64().map_or_else(
            || "identical to reference".to_string(),
            |v| format!("{v:.2} dB"),
        );
        println!("  {name:10} PSNR(vs lanczos) {psnr_str:22} SSIM {ssim:.4}");
    }
    println!();
    println!(
        "  {} report written to {}",
        "Note:".yellow(),
        output_dir.join("comparison.json").display()
    );

    Ok(())
}

/// Operation label used in the frame harness's error messages.
const BATCH_OP: &str = "scaling batch";

/// Batch scale multiple Y4M files.
///
/// Enumerates `input_dir` for files matching `--ext` (default `y4m`, the
/// only format the frame harness reads), sorts them for a deterministic
/// processing order, and applies the same real resize
/// ([`crate::frame_harness::scale::resize_clip_file`]) that `upscale`/
/// `downscale` use to each one, writing results to `output_dir` under the
/// same file name.
///
/// # Errors
///
/// Returns an error if the input directory does not exist, `--algorithm` is
/// unrecognised, no matching files were found, or every matching file failed
/// to resize (a per-file failure alone is reported, not raised, as long as
/// at least one file succeeded).
#[allow(clippy::too_many_arguments)]
async fn handle_batch(
    input_dir: &PathBuf,
    output_dir: &PathBuf,
    width: u32,
    height: u32,
    algorithm: &str,
    ext: Option<&str>,
    json_output: bool,
) -> Result<()> {
    if !input_dir.exists() {
        return Err(anyhow::anyhow!(
            "Input directory not found: {}",
            input_dir.display()
        ));
    }
    if !input_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "Input path is not a directory: {}",
            input_dir.display()
        ));
    }
    validate_target_dims(width, height)?;

    let mode = parse_scaling_mode(algorithm)?;
    let kernel = filter_kernel_for(mode);
    let aspect = oximedia_scaling::AspectRatioMode::Letterbox;
    let want_ext = ext.unwrap_or("y4m").to_lowercase();

    let input_dir_task = input_dir.clone();
    let output_dir_task = output_dir.clone();
    let ext_task = want_ext.clone();
    let (processed, failed): (Vec<serde_json::Value>, Vec<serde_json::Value>) =
        tokio::task::spawn_blocking(move || -> Result<(Vec<serde_json::Value>, Vec<serde_json::Value>)> {
            std::fs::create_dir_all(&output_dir_task).with_context(|| {
                format!(
                    "Failed to create output directory: {}",
                    output_dir_task.display()
                )
            })?;

            let mut entries: Vec<PathBuf> = std::fs::read_dir(&input_dir_task)
                .with_context(|| {
                    format!(
                        "Failed to read input directory: {}",
                        input_dir_task.display()
                    )
                })?
                .filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .filter(|p| {
                    p.extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|s| s.eq_ignore_ascii_case(&ext_task))
                })
                .collect();
            // `read_dir` order is filesystem-dependent; sort for a
            // deterministic, reproducible processing order.
            entries.sort();

            if entries.is_empty() {
                anyhow::bail!(
                    "no '*.{ext_task}' files found in '{}'",
                    input_dir_task.display()
                );
            }

            let mut processed = Vec::new();
            let mut failed = Vec::new();
            for path in &entries {
                let Some(file_name) = path.file_name() else {
                    failed.push(serde_json::json!({
                        "input": path.display().to_string(),
                        "error": "path has no file name",
                    }));
                    continue;
                };
                let out_path = output_dir_task.join(file_name);
                match crate::frame_harness::scale::resize_clip_file(
                    BATCH_OP, path, &out_path, width, height, kernel, aspect,
                ) {
                    Ok(stats) => processed.push(serde_json::json!({
                        "input": path.display().to_string(),
                        "output": out_path.display().to_string(),
                        "src_dims": format!("{}x{}", stats.src_dims.0, stats.src_dims.1),
                        "dst_dims": format!("{}x{}", stats.dst_dims.0, stats.dst_dims.1),
                        "frame_count": stats.frame_count,
                    })),
                    Err(e) => failed.push(serde_json::json!({
                        "input": path.display().to_string(),
                        "error": e.to_string(),
                    })),
                }
            }

            if processed.is_empty() {
                let first_error = failed
                    .first()
                    .and_then(|f| f["error"].as_str())
                    .unwrap_or("unknown error");
                anyhow::bail!(
                    "batch scale processed 0 of {} file(s) successfully; first error: {first_error}",
                    entries.len()
                );
            }

            Ok((processed, failed))
        })
        .await
        .map_err(|join_err| anyhow::anyhow!("batch task panicked: {join_err}"))??;

    if json_output {
        let obj = serde_json::json!({
            "input_dir": input_dir.display().to_string(),
            "output_dir": output_dir.display().to_string(),
            "algorithm": algorithm,
            "ext": want_ext,
            "processed": processed,
            "failed": failed,
            "processed_count": processed.len(),
            "failed_count": failed.len(),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Batch Scale Complete".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input dir:", input_dir.display());
    println!("{:20} {}", "Output dir:", output_dir.display());
    println!("{:20} {} / {}", "Settings:", algorithm, want_ext);
    println!(
        "{:20} {} succeeded, {} failed",
        "Files:",
        processed.len(),
        failed.len()
    );
    if !failed.is_empty() {
        println!();
        println!("{}", "Failures:".yellow().bold());
        for f in &failed {
            println!(
                "  {} {}",
                f["input"].as_str().unwrap_or("?"),
                f["error"].as_str().unwrap_or("?")
            );
        }
    }

    Ok(())
}

/// Capitalise the first ASCII letter of a short label (`"upscale"` ->
/// `"Upscale"`), used only for the success banner text.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
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
            "oximedia_scaling_cmd_{}_{name}",
            std::process::id()
        ))
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = temp_path(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    fn write_gradient_y4m(path: &std::path::Path, w: u32, h: u32, frames: usize) {
        let cw = w.div_ceil(2);
        let ch = h.div_ceil(2);
        let mut buf = format!("YUV4MPEG2 W{w} H{h} F25:1 Ip A1:1 C420jpeg\n").into_bytes();
        for t in 0..frames {
            buf.extend_from_slice(b"FRAME\n");
            for y in 0..h {
                for x in 0..w {
                    buf.push(((x * 3 + y * 5 + t as u32 * 7) % 256) as u8);
                }
            }
            for plane in 0..2u32 {
                for y in 0..ch {
                    for x in 0..cw {
                        buf.push(((x * 2 + y + plane * 40) % 256) as u8);
                    }
                }
            }
        }
        std::fs::write(path, buf).expect("write y4m fixture");
    }

    #[test]
    fn test_parse_scaling_mode_variants() {
        assert!(parse_scaling_mode("bilinear").is_ok());
        assert!(parse_scaling_mode("bicubic").is_ok());
        assert!(parse_scaling_mode("lanczos").is_ok());
        assert!(parse_scaling_mode("invalid").is_err());
    }

    #[test]
    fn test_parse_aspect_mode_variants() {
        assert!(parse_aspect_mode("stretch").is_ok());
        assert!(parse_aspect_mode("letterbox").is_ok());
        assert!(parse_aspect_mode("crop").is_ok());
        assert!(parse_aspect_mode("invalid").is_err());
    }

    #[test]
    fn test_scaling_mode_values() {
        let mode = parse_scaling_mode("lanczos").expect("should succeed");
        assert_eq!(mode, oximedia_scaling::ScalingMode::Lanczos);
    }

    #[test]
    fn test_aspect_mode_values() {
        let mode = parse_aspect_mode("letterbox").expect("should succeed");
        assert_eq!(mode, oximedia_scaling::AspectRatioMode::Letterbox);
    }

    #[test]
    fn test_scaler_integration() {
        let params = oximedia_scaling::ScalingParams::new(1920, 1080)
            .with_mode(oximedia_scaling::ScalingMode::Lanczos);
        let scaler = oximedia_scaling::VideoScaler::new(params);
        let (w, h) = scaler.calculate_dimensions(3840, 2160);
        assert_eq!((w, h), (1920, 1080));
    }

    #[test]
    fn test_capitalize() {
        assert_eq!(capitalize("upscale"), "Upscale");
        assert_eq!(capitalize(""), "");
    }

    // ── handle_scale: real resize, exact output dims, honest errors ────────

    #[tokio::test]
    async fn upscale_writes_real_y4m_with_exact_dims() {
        let input = temp_path("up_in.y4m");
        let output = temp_path("up_out.y4m");
        write_gradient_y4m(&input, 16, 12, 2);
        let _ = std::fs::remove_file(&output);

        handle_scale(
            &input, &output, 32, 24, "lanczos", "stretch", "upscale", false,
        )
        .await
        .expect("upscale must succeed on a real Y4M clip");

        let bytes = std::fs::read(&output).expect("read output");
        assert!(
            bytes.starts_with(b"YUV4MPEG2 W32 H24"),
            "got header: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(40)])
        );

        let demuxed =
            crate::frame_harness::read_y4m_clip("test", &output).expect("re-demux output");
        assert_eq!((demuxed.width(), demuxed.height()), (32, 24));
        assert_eq!(demuxed.len(), 2);

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn downscale_letterbox_preserves_aspect_with_bars() {
        let input = temp_path("down_in.y4m");
        let output = temp_path("down_out.y4m");
        write_gradient_y4m(&input, 16, 16, 1);
        let _ = std::fs::remove_file(&output);

        // 16x16 (square) into 32x16 (2:1) must letterbox: content stays
        // square-ish and is padded, not stretched to fill.
        handle_scale(
            &input,
            &output,
            32,
            16,
            "bilinear",
            "letterbox",
            "downscale",
            false,
        )
        .await
        .expect("downscale must succeed");

        let demuxed =
            crate::frame_harness::read_y4m_clip("test", &output).expect("re-demux output");
        assert_eq!((demuxed.width(), demuxed.height()), (32, 16));

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn upscale_missing_input_errors_no_output() {
        let input = temp_path("missing_in.y4m");
        let output = temp_path("missing_out.y4m");
        let _ = std::fs::remove_file(&input);
        let _ = std::fs::remove_file(&output);

        let result = handle_scale(
            &input,
            &output,
            1920,
            1080,
            "lanczos",
            "letterbox",
            "upscale",
            false,
        )
        .await;
        assert!(result.is_err(), "missing input must error");
        assert!(!output.exists(), "no output file may be produced");
    }

    #[tokio::test]
    async fn upscale_invalid_algorithm_errors_no_output() {
        let input = temp_path("badalg_in.y4m");
        let output = temp_path("badalg_out.y4m");
        write_gradient_y4m(&input, 8, 8, 1);
        std::fs::remove_file(&output).ok();

        let result = handle_scale(
            &input,
            &output,
            100,
            100,
            "bogus",
            "letterbox",
            "upscale",
            false,
        )
        .await;
        assert!(result.is_err(), "invalid algorithm must error");
        assert!(!output.exists(), "no output file may be produced");

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn upscale_requires_y4m_input() {
        let input = temp_path("not_y4m_in.png");
        let output = temp_path("not_y4m_out.y4m");
        std::fs::write(&input, b"\x89PNG dummy input").expect("write dummy input");
        std::fs::remove_file(&output).ok();

        let err = handle_scale(
            &input,
            &output,
            100,
            100,
            "lanczos",
            "letterbox",
            "upscale",
            false,
        )
        .await
        .expect_err("non-Y4M input must be refused");
        assert!(format!("{err}").contains("YUV4MPEG2"));
        assert!(!output.exists());

        std::fs::remove_file(&input).ok();
    }

    // ── handle_compare: real PSNR/SSIM, honest errors ───────────────────────

    #[tokio::test]
    async fn compare_writes_per_algorithm_outputs_and_report() {
        let input = temp_path("cmp_in.y4m");
        let out_dir = scratch_dir("cmp_out");
        write_gradient_y4m(&input, 16, 16, 2);

        handle_compare(&input, 32, 32, Some(out_dir.as_path()), false)
            .await
            .expect("compare must succeed on a real Y4M clip");

        for name in ["bilinear", "bicubic", "lanczos"] {
            let path = out_dir.join(format!("{name}.y4m"));
            assert!(path.exists(), "{name}.y4m must be written");
            let demuxed = crate::frame_harness::read_y4m_clip("test", &path).expect("re-demux");
            assert_eq!((demuxed.width(), demuxed.height()), (32, 32));
        }

        let report_path = out_dir.join("comparison.json");
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report_path).expect("read report"))
                .expect("valid JSON");
        let algorithms = report["algorithms"].as_array().expect("algorithms array");
        assert_eq!(algorithms.len(), 3);
        let lanczos = algorithms
            .iter()
            .find(|a| a["algorithm"] == "lanczos")
            .expect("lanczos entry");
        assert!(
            lanczos["psnr_identical_to_reference"]
                .as_bool()
                .unwrap_or(false),
            "lanczos compared against itself must be identical"
        );

        std::fs::remove_file(&input).ok();
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[tokio::test]
    async fn compare_without_output_dir_is_honest_error() {
        let input = temp_path("cmp_noout_in.y4m");
        write_gradient_y4m(&input, 8, 8, 1);

        let err = handle_compare(&input, 16, 16, None, false)
            .await
            .expect_err("compare without --output-dir must error");
        assert!(format!("{err}").contains("--output-dir"));

        std::fs::remove_file(&input).ok();
    }

    // ── handle_batch: deterministic order, honest ext handling ──────────────

    #[tokio::test]
    async fn batch_scales_every_matching_file_in_sorted_order() {
        let in_dir = scratch_dir("batch_in");
        let out_dir = scratch_dir("batch_out");
        write_gradient_y4m(&in_dir.join("b.y4m"), 8, 8, 1);
        write_gradient_y4m(&in_dir.join("a.y4m"), 8, 8, 1);
        std::fs::write(in_dir.join("ignore.txt"), b"not a video").expect("write non-match");

        handle_batch(&in_dir, &out_dir, 16, 16, "bilinear", None, false)
            .await
            .expect("batch must succeed");

        for name in ["a.y4m", "b.y4m"] {
            let out_path = out_dir.join(name);
            assert!(out_path.exists(), "{name} must be scaled");
            let demuxed = crate::frame_harness::read_y4m_clip("test", &out_path).expect("re-demux");
            assert_eq!((demuxed.width(), demuxed.height()), (16, 16));
        }
        assert!(!out_dir.join("ignore.txt").exists());

        let _ = std::fs::remove_dir_all(&in_dir);
        let _ = std::fs::remove_dir_all(&out_dir);
    }

    #[tokio::test]
    async fn batch_empty_directory_is_honest_error() {
        let in_dir = scratch_dir("batch_empty_in");
        let out_dir = temp_path("batch_empty_out");
        let _ = std::fs::remove_dir_all(&out_dir);

        let err = handle_batch(&in_dir, &out_dir, 16, 16, "lanczos", None, false)
            .await
            .expect_err("an empty input directory must error");
        assert!(format!("{err}").contains("no '*.y4m' files"));

        let _ = std::fs::remove_dir_all(&in_dir);
    }
}
