//! Thumbnail generation from video files.
//!
//! Provides functionality to generate thumbnails and preview strips from videos,
//! with support for multiple extraction strategies and layouts.

use crate::progress::TranscodeProgress;
use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use oximedia_image::{
    png::{PngColorType, PngImage},
    ColorSpace, ImageData, ImageFrame, PixelType,
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Options for thumbnail generation.
#[derive(Debug, Clone)]
pub struct ThumbnailOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub mode: ThumbnailMode,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub quality: u8,
    pub format: ThumbnailFormat,
    pub json_output: bool,
}

/// Thumbnail generation mode.
#[derive(Debug, Clone, PartialEq)]
pub enum ThumbnailMode {
    /// Single thumbnail at specific timestamp
    Single { timestamp: f64 },
    /// Multiple thumbnails at intervals
    Multiple { count: usize },
    /// Grid/strip layout with multiple thumbnails
    Grid { rows: usize, cols: usize },
    /// Automatically detect best frame (least motion blur, etc.)
    Auto,
}

/// Output format for thumbnails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailFormat {
    Png,
    Jpeg,
    Webp,
}

impl ThumbnailFormat {
    /// Parse format from string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "webp" => Ok(Self::Webp),
            _ => Err(anyhow!("Unsupported thumbnail format: {}", s)),
        }
    }

    /// Get format name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
        }
    }

    /// Get file extension.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }
}

/// Result of thumbnail generation (for JSON output).
#[derive(Debug, Serialize)]
pub struct ThumbnailResult {
    pub success: bool,
    pub output_files: Vec<String>,
    pub thumbnail_count: usize,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

/// Main thumbnail generation function.
pub async fn generate_thumbnails(options: ThumbnailOptions) -> Result<()> {
    info!("Starting thumbnail generation");
    debug!("Thumbnail options: {:?}", options);

    // Validate input
    validate_input(&options.input).await?;

    // Validate quality
    if options.quality > 100 {
        return Err(anyhow!("Quality must be between 0 and 100"));
    }

    // Print thumbnail plan (status output; suppressed by --quiet)
    if !options.json_output && !crate::progress::is_quiet() {
        print_thumbnail_plan(&options);
    }

    // Generate thumbnails
    let output_files = generate_impl(&options).await?;

    // Print or output result
    if options.json_output {
        let result = create_result(&output_files, &options)?;
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if !crate::progress::is_quiet() {
        print_thumbnail_summary(&output_files, &options);
    }

    Ok(())
}

/// Validate input file exists and is readable.
async fn validate_input(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(anyhow!("Input file does not exist: {}", path.display()));
    }

    if !path.is_file() {
        return Err(anyhow!("Input path is not a file: {}", path.display()));
    }

    let metadata = tokio::fs::metadata(path)
        .await
        .context("Failed to read input file metadata")?;

    if metadata.len() == 0 {
        return Err(anyhow!("Input file is empty"));
    }

    Ok(())
}

/// Print thumbnail generation plan.
fn print_thumbnail_plan(options: &ThumbnailOptions) {
    println!("{}", "Thumbnail Generation Plan".cyan().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", options.input.display());
    println!("{:20} {}", "Output:", options.output.display());
    println!("{:20} {}", "Format:", options.format.name());

    match &options.mode {
        ThumbnailMode::Single { timestamp } => {
            println!("{:20} Single at {:.2}s", "Mode:", timestamp);
        }
        ThumbnailMode::Multiple { count } => {
            println!("{:20} Multiple ({} thumbnails)", "Mode:", count);
        }
        ThumbnailMode::Grid { rows, cols } => {
            println!("{:20} Grid ({}x{})", "Mode:", rows, cols);
        }
        ThumbnailMode::Auto => {
            println!("{:20} Auto-detect best frame", "Mode:");
        }
    }

    if let Some(w) = options.width {
        println!("{:20} {}", "Width:", w);
    }

    if let Some(h) = options.height {
        println!("{:20} {}", "Height:", h);
    }

    if options.format == ThumbnailFormat::Jpeg {
        println!("{:20} {}", "Quality:", options.quality);
    }

    println!("{}", "=".repeat(60));
    println!();
}

/// Perform the actual thumbnail generation.
async fn generate_impl(options: &ThumbnailOptions) -> Result<Vec<PathBuf>> {
    match &options.mode {
        ThumbnailMode::Single { timestamp } => generate_single(options, *timestamp).await,
        ThumbnailMode::Multiple { count } => generate_multiple(options, *count).await,
        ThumbnailMode::Grid { rows, cols } => generate_grid(options, *rows, *cols).await,
        ThumbnailMode::Auto => generate_auto(options).await,
    }
}

/// Generate a single thumbnail at specified timestamp.
///
/// The frame number is derived from the video's actual frame rate when the
/// source is Y4M; for other formats a descriptive error is returned.
async fn generate_single(options: &ThumbnailOptions, timestamp: f64) -> Result<Vec<PathBuf>> {
    info!("Generating single thumbnail at {:.2}s", timestamp);

    // Determine fps from the Y4M header if possible; fall back to 25 fps.
    let fps = probe_y4m_fps(&options.input).unwrap_or(25.0);
    let frame_num = (timestamp * fps).round() as u64;

    debug!(
        "Extracting frame {} (fps={:.3}, ts={:.2}s) from {}",
        frame_num,
        fps,
        timestamp,
        options.input.display()
    );

    let (rgb, width, height) =
        crate::frame_extract::extract_video_frame_rgb(&options.input, frame_num)
            .await
            .context("Frame extraction failed")?;

    let rgb = maybe_scale(rgb, width, height, options.width, options.height);
    let (out_w, out_h) = scaled_dimensions(width, height, options.width, options.height);

    write_thumbnail_image(
        &rgb,
        out_w,
        out_h,
        &options.output,
        options.format,
        options.quality,
    )?;

    info!("Thumbnail written to {}", options.output.display());
    Ok(vec![options.output.clone()])
}

/// Generate multiple thumbnails distributed evenly across the video.
///
/// All frames are extracted in a single sequential pass through the file.
async fn generate_multiple(options: &ThumbnailOptions, count: usize) -> Result<Vec<PathBuf>> {
    info!("Generating {} thumbnails", count);

    let fps = probe_y4m_fps(&options.input).unwrap_or(25.0);
    // Probe total frame count; if unknown, assume 5-second spacing at fps.
    let total_frames =
        probe_y4m_frame_count(&options.input).unwrap_or(count as u64 * 5 * fps as u64);

    // Build evenly-spaced frame indices.
    let indices: Vec<u64> = if count <= 1 {
        vec![0]
    } else {
        (0..count)
            .map(|i| {
                let frac = i as f64 / (count - 1) as f64;
                ((frac * (total_frames.saturating_sub(1)) as f64).round() as u64)
                    .min(total_frames.saturating_sub(1))
            })
            .collect()
    };

    debug!("Extracting frame indices: {:?}", indices);

    let frames = crate::frame_extract::extract_video_frames_rgb(&options.input, &indices)
        .await
        .context("Multi-frame extraction failed")?;

    let mut output_files = Vec::with_capacity(frames.len());
    let mut progress = TranscodeProgress::new(frames.len() as u64);

    for (i, (rgb, width, height)) in frames.into_iter().enumerate() {
        let output_path = generate_output_path(&options.output, i, count, &options.format);
        debug!(
            "Writing thumbnail {}/{}: {}",
            i + 1,
            count,
            output_path.display()
        );

        let rgb = maybe_scale(rgb, width, height, options.width, options.height);
        let (out_w, out_h) = scaled_dimensions(width, height, options.width, options.height);

        write_thumbnail_image(
            &rgb,
            out_w,
            out_h,
            &output_path,
            options.format,
            options.quality,
        )?;

        progress.update(i as u64 + 1);
        output_files.push(output_path);
    }

    progress.finish();
    Ok(output_files)
}

/// Generate a contact-sheet grid of thumbnails assembled into one image.
///
/// Each cell is extracted, scaled to fit, then composited into a single
/// wide-gamut RGB canvas that is written as one output file.
async fn generate_grid(
    options: &ThumbnailOptions,
    rows: usize,
    cols: usize,
) -> Result<Vec<PathBuf>> {
    let total_thumbs = rows * cols;
    info!(
        "Generating {}x{} grid ({} thumbnails)",
        rows, cols, total_thumbs
    );

    // Desired cell dimensions (fallback to 160×90 if not specified).
    let cell_w = options.width.unwrap_or(160);
    let cell_h = options.height.unwrap_or(90);

    // Derive evenly-spaced frame indices.
    let fps = probe_y4m_fps(&options.input).unwrap_or(25.0);
    let total_frames =
        probe_y4m_frame_count(&options.input).unwrap_or(total_thumbs as u64 * 5 * fps as u64);

    let indices: Vec<u64> = (0..total_thumbs)
        .map(|i| {
            let frac = if total_thumbs <= 1 {
                0.0
            } else {
                i as f64 / (total_thumbs - 1) as f64
            };
            ((frac * total_frames.saturating_sub(1) as f64).round() as u64)
                .min(total_frames.saturating_sub(1))
        })
        .collect();

    let frames = crate::frame_extract::extract_video_frames_rgb(&options.input, &indices)
        .await
        .context("Grid frame extraction failed")?;

    // Composite into a single canvas: rows × cols cells.
    let canvas_w = (cell_w * cols as u32) as usize;
    let canvas_h = (cell_h * rows as u32) as usize;
    let mut canvas = vec![0u8; canvas_w * canvas_h * 3];

    let mut progress = TranscodeProgress::new(total_thumbs as u64);

    for (idx, (src_rgb, src_w, src_h)) in frames.into_iter().enumerate() {
        let row = idx / cols;
        let col = idx % cols;

        // Scale the cell.
        let cell_rgb = scale_rgb_nearest(&src_rgb, src_w, src_h, cell_w, cell_h);

        // Blit into canvas.
        let x_off = col * cell_w as usize;
        let y_off = row * cell_h as usize;
        for cy in 0..cell_h as usize {
            let src_row_start = cy * cell_w as usize * 3;
            let dst_row_start = ((y_off + cy) * canvas_w + x_off) * 3;
            let len = cell_w as usize * 3;
            if src_row_start + len <= cell_rgb.len() && dst_row_start + len <= canvas.len() {
                canvas[dst_row_start..dst_row_start + len]
                    .copy_from_slice(&cell_rgb[src_row_start..src_row_start + len]);
            }
        }

        progress.update(idx as u64 + 1);
    }

    progress.finish();

    write_thumbnail_image(
        &canvas,
        canvas_w as u32,
        canvas_h as u32,
        &options.output,
        options.format,
        options.quality,
    )?;

    info!("Grid written to {}", options.output.display());
    Ok(vec![options.output.clone()])
}

/// Auto-select the best frame by choosing the one with the highest average
/// luma (brightness), which tends to be more visually representative.
///
/// Samples up to 16 evenly-spaced frames to keep probe time low.
async fn generate_auto(options: &ThumbnailOptions) -> Result<Vec<PathBuf>> {
    info!("Auto-detecting best frame for thumbnail");

    let fps = probe_y4m_fps(&options.input).unwrap_or(25.0);
    let total_frames = probe_y4m_frame_count(&options.input).unwrap_or((fps * 10.0) as u64);

    // Sample up to 16 evenly-distributed frames.
    let sample_count = 16.min(total_frames as usize).max(1);
    let indices: Vec<u64> = (0..sample_count)
        .map(|i| {
            let frac = if sample_count <= 1 {
                0.0
            } else {
                i as f64 / (sample_count - 1) as f64
            };
            ((frac * total_frames.saturating_sub(1) as f64).round() as u64)
                .min(total_frames.saturating_sub(1))
        })
        .collect();

    let frames = crate::frame_extract::extract_video_frames_rgb(&options.input, &indices)
        .await
        .context("Auto frame extraction failed")?;

    // Pick the frame with the highest mean luma.
    let best_rgb = frames
        .into_iter()
        .max_by_key(|(rgb, _, _)| {
            let luma_sum: u64 = rgb
                .chunks_exact(3)
                .map(|p| {
                    // BT.601 luma approximation: 0.299R + 0.587G + 0.114B, scaled ×1000.
                    (p[0] as u64 * 299 + p[1] as u64 * 587 + p[2] as u64 * 114) / 1000
                })
                .sum();
            luma_sum
        })
        .ok_or_else(|| anyhow!("No frames could be extracted for auto-detection"))?;

    let (rgb, width, height) = best_rgb;
    let rgb = maybe_scale(rgb, width, height, options.width, options.height);
    let (out_w, out_h) = scaled_dimensions(width, height, options.width, options.height);

    write_thumbnail_image(
        &rgb,
        out_w,
        out_h,
        &options.output,
        options.format,
        options.quality,
    )?;

    info!("Auto thumbnail written to {}", options.output.display());
    Ok(vec![options.output.clone()])
}

// ---------------------------------------------------------------------------
// Image writing helpers
// ---------------------------------------------------------------------------

/// Write RGB24 pixel data to `output` using the requested [`ThumbnailFormat`].
///
/// Uses `oximedia-image` encoders for PNG, JPEG, and WebP.
fn write_thumbnail_image(
    rgb: &[u8],
    width: u32,
    height: u32,
    output: &Path,
    format: ThumbnailFormat,
    quality: u8,
) -> Result<()> {
    // Ensure parent directory exists.
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).context("Failed to create output directory")?;
        }
    }

    match format {
        ThumbnailFormat::Png => {
            let image = PngImage {
                width,
                height,
                bit_depth: 8,
                color_type: PngColorType::Rgb,
                pixels: rgb.to_vec(),
                metadata: std::collections::HashMap::new(),
            };
            oximedia_image::png::write_png(output, &image)
                .with_context(|| format!("Failed to write PNG to {}", output.display()))?;
        }
        ThumbnailFormat::Jpeg => {
            let frame = build_image_frame(rgb, width, height);
            oximedia_image::jpeg::write_jpeg(output, &frame, quality)
                .with_context(|| format!("Failed to write JPEG to {}", output.display()))?;
        }
        ThumbnailFormat::Webp => {
            let frame = build_image_frame(rgb, width, height);
            oximedia_image::webp::write_webp(output, &frame)
                .with_context(|| format!("Failed to write WebP to {}", output.display()))?;
        }
    }

    Ok(())
}

/// Construct an [`ImageFrame`] from packed RGB24 data.
fn build_image_frame(rgb: &[u8], width: u32, height: u32) -> ImageFrame {
    ImageFrame::new(
        0,
        width,
        height,
        PixelType::U8,
        3,
        ColorSpace::Srgb,
        ImageData::interleaved(rgb.to_vec()),
    )
}

// ---------------------------------------------------------------------------
// Y4M probing helpers (best-effort — return None on any failure)
// ---------------------------------------------------------------------------

/// Try to read the fps of a Y4M file without reading all frames.
fn probe_y4m_fps(path: &Path) -> Option<f64> {
    let data = std::fs::read(path).ok()?;
    let demuxer =
        oximedia_container::demux::y4m::Y4mDemuxer::new(std::io::Cursor::new(data)).ok()?;
    let (num, den) = demuxer.fps();
    if den == 0 {
        None
    } else {
        Some(num as f64 / den as f64)
    }
}

/// Try to count the total number of frames in a Y4M file.
///
/// This reads through the entire file to count frames, so it is only practical
/// for short clips used in thumbnail generation.
fn probe_y4m_frame_count(path: &Path) -> Option<u64> {
    let data = std::fs::read(path).ok()?;
    let frame_size = {
        let demuxer =
            oximedia_container::demux::y4m::Y4mDemuxer::new(std::io::Cursor::new(data.clone()))
                .ok()?;
        demuxer.frame_size()
    };
    // After the header, each frame is "FRAME\n" (6 bytes) + frame_size bytes.
    // Walk the byte slice to count FRAME tags rather than re-parsing everything.
    let header_end = data.iter().position(|&b| b == b'\n')? + 1;
    let body = &data[header_end..];
    let mut pos = 0usize;
    let mut count = 0u64;
    while pos + 6 <= body.len() {
        if body[pos..].starts_with(b"FRAME") {
            // Skip past the frame-header line then the frame data.
            let frame_hdr_end = body[pos..].iter().position(|&b| b == b'\n')? + 1;
            let next = pos + frame_hdr_end + frame_size;
            if next > body.len() {
                break;
            }
            pos = next;
            count += 1;
        } else {
            break;
        }
    }
    if count == 0 {
        None
    } else {
        Some(count)
    }
}

// ---------------------------------------------------------------------------
// Scaling helpers
// ---------------------------------------------------------------------------

/// Scale `rgb` to `(target_w, target_h)` using nearest-neighbour if the caller
/// specified target dimensions, otherwise return the original unchanged.
fn maybe_scale(
    rgb: Vec<u8>,
    src_w: u32,
    src_h: u32,
    target_w: Option<u32>,
    target_h: Option<u32>,
) -> Vec<u8> {
    let (tw, th) = scaled_dimensions(src_w, src_h, target_w, target_h);
    if tw == src_w && th == src_h {
        rgb
    } else {
        scale_rgb_nearest(&rgb, src_w, src_h, tw, th)
    }
}

/// Compute output dimensions, preserving aspect ratio if only one axis is given.
fn scaled_dimensions(
    src_w: u32,
    src_h: u32,
    target_w: Option<u32>,
    target_h: Option<u32>,
) -> (u32, u32) {
    match (target_w, target_h) {
        (None, None) => (src_w, src_h),
        (Some(w), None) => {
            let h = (src_h as f64 * w as f64 / src_w as f64).round() as u32;
            (w, h.max(1))
        }
        (None, Some(h)) => {
            let w = (src_w as f64 * h as f64 / src_h as f64).round() as u32;
            (w.max(1), h)
        }
        (Some(w), Some(h)) => (w, h),
    }
}

/// Nearest-neighbour RGB24 scaling.
fn scale_rgb_nearest(src: &[u8], src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> Vec<u8> {
    let sw = src_w as usize;
    let sh = src_h as usize;
    let dw = dst_w as usize;
    let dh = dst_h as usize;
    let mut dst = vec![0u8; dw * dh * 3];

    for dy in 0..dh {
        let sy = (dy * sh / dh).min(sh - 1);
        for dx in 0..dw {
            let sx = (dx * sw / dw).min(sw - 1);
            let src_idx = (sy * sw + sx) * 3;
            let dst_idx = (dy * dw + dx) * 3;
            dst[dst_idx] = src[src_idx];
            dst[dst_idx + 1] = src[src_idx + 1];
            dst[dst_idx + 2] = src[src_idx + 2];
        }
    }
    dst
}

/// Generate output path for multiple thumbnails.
fn generate_output_path(
    base_path: &Path,
    index: usize,
    total: usize,
    format: &ThumbnailFormat,
) -> PathBuf {
    let parent = base_path.parent().unwrap_or(Path::new(""));
    let stem = base_path.file_stem().unwrap_or_default().to_string_lossy();

    let filename = if total > 1 {
        format!("{}_{:03}.{}", stem, index + 1, format.extension())
    } else {
        format!("{}.{}", stem, format.extension())
    };

    parent.join(filename)
}

/// Create result structure for JSON output.
fn create_result(output_files: &[PathBuf], options: &ThumbnailOptions) -> Result<ThumbnailResult> {
    Ok(ThumbnailResult {
        success: true,
        output_files: output_files
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        thumbnail_count: output_files.len(),
        format: options.format.name().to_string(),
        width: options.width.unwrap_or(320),
        height: options.height.unwrap_or(240),
    })
}

/// Print thumbnail generation summary.
fn print_thumbnail_summary(output_files: &[PathBuf], options: &ThumbnailOptions) {
    println!();
    println!("{}", "Thumbnail Generation Complete".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Thumbnails Created:", output_files.len());
    println!("{:20} {}", "Format:", options.format.name());

    if output_files.len() <= 10 {
        println!("\n{}", "Output Files:".cyan());
        for (i, path) in output_files.iter().enumerate() {
            println!("  {}. {}", i + 1, path.display());
        }
    } else {
        println!("{:20} {}", "First Output:", output_files[0].display());
        println!("{:20} ... and {} more", "", output_files.len() - 1);
    }

    println!("{}", "=".repeat(60));
}

/// Parse time string to seconds.
pub fn parse_timestamp(s: &str) -> Result<f64> {
    // Try parsing as seconds first
    if let Ok(seconds) = s.parse::<f64>() {
        return Ok(seconds);
    }

    // Try parsing as HH:MM:SS or MM:SS
    let parts: Vec<&str> = s.split(':').collect();

    match parts.len() {
        1 => parts[0].parse().context("Invalid time format"),
        2 => {
            let minutes: f64 = parts[0].parse().context("Invalid minutes")?;
            let seconds: f64 = parts[1].parse().context("Invalid seconds")?;
            Ok(minutes * 60.0 + seconds)
        }
        3 => {
            let hours: f64 = parts[0].parse().context("Invalid hours")?;
            let minutes: f64 = parts[1].parse().context("Invalid minutes")?;
            let seconds: f64 = parts[2].parse().context("Invalid seconds")?;
            Ok(hours * 3600.0 + minutes * 60.0 + seconds)
        }
        _ => Err(anyhow!("Invalid time format: {}", s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thumbnail_format_parsing() {
        assert_eq!(
            ThumbnailFormat::from_str("png").expect("ThumbnailFormat::from_str should succeed"),
            ThumbnailFormat::Png
        );
        assert_eq!(
            ThumbnailFormat::from_str("jpeg").expect("ThumbnailFormat::from_str should succeed"),
            ThumbnailFormat::Jpeg
        );
        assert_eq!(
            ThumbnailFormat::from_str("webp").expect("ThumbnailFormat::from_str should succeed"),
            ThumbnailFormat::Webp
        );
        assert!(ThumbnailFormat::from_str("bmp").is_err());
    }

    #[test]
    fn test_parse_timestamp() {
        assert_eq!(parse_timestamp("30").expect("parse should succeed"), 30.0);
        assert_eq!(parse_timestamp("1:30").expect("parse should succeed"), 90.0);
        assert_eq!(
            parse_timestamp("1:01:30").expect("parse should succeed"),
            3690.0
        );
        assert_eq!(
            parse_timestamp("0:05:00").expect("parse should succeed"),
            300.0
        );
    }

    #[test]
    fn test_generate_output_path() {
        let base = PathBuf::from("output.png");
        let format = ThumbnailFormat::Png;

        let path = generate_output_path(&base, 0, 5, &format);
        assert_eq!(path, PathBuf::from("output_001.png"));

        let path = generate_output_path(&base, 4, 5, &format);
        assert_eq!(path, PathBuf::from("output_005.png"));
    }
}
