//! Timecode command for `oximedia timecode`.
//!
//! Provides convert, calculate, validate, burn, to-frames, and from-frames
//! subcommands via `oximedia-timecode`.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use oximedia_timecode::{FrameRate, Timecode, TimecodeError};
use std::path::PathBuf;

/// Subcommands for `oximedia timecode`.
#[derive(Subcommand)]
pub enum TimecodeCommand {
    /// Convert a timecode between two frame rates
    Convert {
        /// Timecode string (HH:MM:SS:FF or HH:MM:SS;FF)
        #[arg(value_name = "TIMECODE")]
        timecode: String,

        /// Source frame rate (23.976, 24, 25, 29.97df, 29.97ndf, 30, 50, 59.94, 60)
        #[arg(long)]
        from_fps: String,

        /// Target frame rate
        #[arg(long)]
        to_fps: String,
    },

    /// Perform arithmetic on a timecode (add or subtract frames/seconds)
    Calculate {
        /// Timecode string (HH:MM:SS:FF)
        #[arg(value_name = "TIMECODE")]
        timecode: String,

        /// Frame rate
        #[arg(long)]
        fps: String,

        /// Operation: add-frames, sub-frames, add-seconds, sub-seconds
        #[arg(long)]
        operation: String,

        /// Value to apply (integer)
        #[arg(value_name = "VALUE")]
        value: i64,
    },

    /// Validate a timecode string
    Validate {
        /// Timecode string (HH:MM:SS:FF or HH:MM:SS;FF)
        #[arg(value_name = "TIMECODE")]
        timecode: String,

        /// Frame rate
        #[arg(long)]
        fps: String,
    },

    /// Convert timecode to total frame count since midnight
    ToFrames {
        /// Timecode string (HH:MM:SS:FF)
        #[arg(value_name = "TIMECODE")]
        timecode: String,

        /// Frame rate
        #[arg(long)]
        fps: String,
    },

    /// Convert a total frame count to timecode
    FromFrames {
        /// Frame number since midnight
        #[arg(value_name = "FRAMES")]
        frames: u64,

        /// Frame rate
        #[arg(long)]
        fps: String,
    },

    /// Burn a timecode overlay into an uncompressed Y4M clip (requires --font)
    Burn {
        /// Input video file (uncompressed YUV4MPEG2 / .y4m)
        #[arg(short, long)]
        input: PathBuf,

        /// Output video file (YUV4MPEG2 / .y4m)
        #[arg(short, long)]
        output: PathBuf,

        /// Starting timecode (HH:MM:SS:FF); defaults to 00:00:00:00
        #[arg(long, default_value = "00:00:00:00")]
        start: String,

        /// Frame rate
        #[arg(long, default_value = "25")]
        fps: String,

        /// Position: top-left, top-right, bottom-left, bottom-right, center
        #[arg(long, default_value = "bottom-right")]
        position: String,

        /// Font size in points
        #[arg(long, default_value = "36")]
        font_size: u32,

        /// TrueType/OpenType font used to rasterise the overlay (required;
        /// OxiMedia ships no font and never picks a system one)
        #[arg(long, value_name = "PATH")]
        font: Option<PathBuf>,
    },
}

/// Entry point called from `main.rs`.
pub async fn run_timecode(command: TimecodeCommand, json_output: bool) -> Result<()> {
    match command {
        TimecodeCommand::Convert {
            timecode,
            from_fps,
            to_fps,
        } => cmd_convert(&timecode, &from_fps, &to_fps, json_output),

        TimecodeCommand::Calculate {
            timecode,
            fps,
            operation,
            value,
        } => cmd_calculate(&timecode, &fps, &operation, value, json_output),

        TimecodeCommand::Validate { timecode, fps } => cmd_validate(&timecode, &fps, json_output),

        TimecodeCommand::ToFrames { timecode, fps } => cmd_to_frames(&timecode, &fps, json_output),

        TimecodeCommand::FromFrames { frames, fps } => cmd_from_frames(frames, &fps, json_output),

        TimecodeCommand::Burn {
            input,
            output,
            start,
            fps,
            position,
            font_size,
            font,
        } => {
            // Burn-in decodes, rasterises and re-encodes every frame: CPU-bound
            // and fully synchronous, so it runs on the blocking pool rather
            // than stalling the async runtime.
            let opts = BurnOptions {
                input,
                output,
                start,
                fps,
                position,
                font_size,
                font,
            };
            tokio::task::spawn_blocking(move || cmd_burn(&opts, json_output))
                .await
                .map_err(|join_err| anyhow::anyhow!("burn-in task panicked: {join_err}"))?
        }
    }
}

// ---------------------------------------------------------------------------
// Individual subcommand implementations
// ---------------------------------------------------------------------------

fn cmd_convert(tc_str: &str, from_fps: &str, to_fps: &str, json_output: bool) -> Result<()> {
    let src_rate = parse_frame_rate(from_fps)?;
    let dst_rate = parse_frame_rate(to_fps)?;
    let tc = parse_timecode(tc_str, src_rate)?;

    // Convert via total frame count, adjusting for frame rate ratio
    let src_frames = tc.to_frames();
    let src_fps_float = src_rate.as_float();
    let dst_fps_float = dst_rate.as_float();

    // Scale frame count to target fps
    let dst_frame_count = (src_frames as f64 * dst_fps_float / src_fps_float).round() as u64;
    let converted = Timecode::from_frames(dst_frame_count, dst_rate)
        .map_err(|e| anyhow::anyhow!("Timecode conversion failed: {}", e))?;

    if json_output {
        let obj = serde_json::json!({
            "input": tc_str,
            "from_fps": from_fps,
            "to_fps": to_fps,
            "source_frames": src_frames,
            "output_frames": dst_frame_count,
            "output": converted.to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Timecode Conversion".green().bold());
    println!(
        "  {} {} @ {}",
        "Input:".cyan(),
        tc.to_string().yellow(),
        from_fps
    );
    println!(
        "  {} {} @ {}",
        "Output:".cyan(),
        converted.to_string().yellow(),
        to_fps
    );
    println!(
        "  {} {} → {} frames",
        "Frames:".cyan(),
        src_frames,
        dst_frame_count
    );

    Ok(())
}

fn cmd_calculate(
    tc_str: &str,
    fps_str: &str,
    operation: &str,
    value: i64,
    json_output: bool,
) -> Result<()> {
    let fps = parse_frame_rate(fps_str)?;
    let tc = parse_timecode(tc_str, fps)?;
    let initial_frames = tc.to_frames() as i64;

    let delta_frames: i64 = match operation {
        "add-frames" => value,
        "sub-frames" => -value,
        "add-seconds" => {
            let fps_val = fps.frames_per_second() as i64;
            value * fps_val
        }
        "sub-seconds" => {
            let fps_val = fps.frames_per_second() as i64;
            -value * fps_val
        }
        other => anyhow::bail!(
            "Unknown operation '{}'. Supported: add-frames, sub-frames, add-seconds, sub-seconds",
            other
        ),
    };

    let result_frames = (initial_frames + delta_frames).max(0) as u64;
    let result_tc = Timecode::from_frames(result_frames, fps)
        .map_err(|e| anyhow::anyhow!("Timecode calculation failed: {}", e))?;

    if json_output {
        let obj = serde_json::json!({
            "input": tc_str,
            "fps": fps_str,
            "operation": operation,
            "value": value,
            "initial_frames": initial_frames,
            "delta_frames": delta_frames,
            "result_frames": result_frames,
            "result": result_tc.to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Timecode Calculate".green().bold());
    println!(
        "  {} {} @ {}",
        "Input:".cyan(),
        tc.to_string().yellow(),
        fps_str
    );
    println!(
        "  {} {} ({:+})",
        "Operation:".cyan(),
        operation,
        delta_frames
    );
    println!(
        "  {} {}",
        "Result:".cyan(),
        result_tc.to_string().yellow().bold()
    );

    Ok(())
}

fn cmd_validate(tc_str: &str, fps_str: &str, json_output: bool) -> Result<()> {
    let fps = parse_frame_rate(fps_str)?;
    let parse_result = parse_timecode(tc_str, fps);

    let (valid, reason) = match &parse_result {
        Ok(_) => (true, "Valid SMPTE timecode".to_string()),
        Err(e) => (false, e.to_string()),
    };

    if json_output {
        let obj = serde_json::json!({
            "input": tc_str,
            "fps": fps_str,
            "valid": valid,
            "reason": reason,
            "parsed": parse_result.as_ref().map(|tc| tc.to_string()).ok(),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Timecode Validation".green().bold());
    println!("  {} {}", "Input:".cyan(), tc_str);
    println!("  {} {}", "FPS:".cyan(), fps_str);
    if valid {
        println!("  {} {}", "Status:".cyan(), "VALID".green().bold());
        if let Ok(tc) = parse_result {
            println!("  {} {}", "Parsed:".cyan(), tc.to_string().yellow());
        }
    } else {
        println!("  {} {}", "Status:".cyan(), "INVALID".red().bold());
        println!("  {} {}", "Reason:".cyan(), reason.red());
    }

    Ok(())
}

fn cmd_to_frames(tc_str: &str, fps_str: &str, json_output: bool) -> Result<()> {
    let fps = parse_frame_rate(fps_str)?;
    let tc = parse_timecode(tc_str, fps)?;
    let total_frames = tc.to_frames();

    // Also compute wall-clock time
    let fps_f = fps.as_float();
    let seconds_total = total_frames as f64 / fps_f;
    let hours = (seconds_total / 3600.0) as u64;
    let minutes = ((seconds_total % 3600.0) / 60.0) as u64;
    let seconds = (seconds_total % 60.0) as u64;

    if json_output {
        let obj = serde_json::json!({
            "input": tc_str,
            "fps": fps_str,
            "total_frames": total_frames,
            "wall_clock_seconds": seconds_total,
            "wall_clock": format!("{:02}:{:02}:{:02}", hours, minutes, seconds),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Timecode → Frames".green().bold());
    println!(
        "  {} {} @ {}",
        "Input:".cyan(),
        tc.to_string().yellow(),
        fps_str
    );
    println!(
        "  {} {}",
        "Total frames:".cyan(),
        total_frames.to_string().yellow().bold()
    );
    println!(
        "  {} {:.3}s  ({:02}:{:02}:{:02})",
        "Wall clock:".cyan(),
        seconds_total,
        hours,
        minutes,
        seconds
    );

    Ok(())
}

fn cmd_from_frames(frames: u64, fps_str: &str, json_output: bool) -> Result<()> {
    let fps = parse_frame_rate(fps_str)?;
    let tc = Timecode::from_frames(frames, fps)
        .map_err(|e| anyhow::anyhow!("Failed to build timecode: {}", e))?;

    let fps_f = fps.as_float();
    let seconds_total = frames as f64 / fps_f;

    if json_output {
        let obj = serde_json::json!({
            "input_frames": frames,
            "fps": fps_str,
            "timecode": tc.to_string(),
            "drop_frame": tc.frame_rate.drop_frame,
            "wall_clock_seconds": seconds_total,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Frames → Timecode".green().bold());
    println!(
        "  {} {} @ {}",
        "Input:".cyan(),
        frames.to_string().yellow(),
        fps_str
    );
    println!(
        "  {} {}",
        "Timecode:".cyan(),
        tc.to_string().yellow().bold()
    );
    println!("  {} {:.3}s", "Wall clock:".cyan(), seconds_total);

    Ok(())
}

/// Operation label used in the frame harness's error messages.
const BURN_OP: &str = "timecode burn";

/// Parameters of the `timecode burn` subcommand.
#[derive(Debug, Clone)]
struct BurnOptions {
    /// Input clip (uncompressed YUV4MPEG2).
    input: PathBuf,
    /// Output clip (YUV4MPEG2).
    output: PathBuf,
    /// Timecode shown on the first frame (`HH:MM:SS:FF`).
    start: String,
    /// Frame rate the timecode is counted at.
    fps: String,
    /// Overlay anchor position.
    position: String,
    /// Overlay font size in points.
    font_size: u32,
    /// TrueType/OpenType font to rasterise with; there is no default.
    font: Option<PathBuf>,
}

/// Burn a timecode overlay into every frame of a Y4M clip.
///
/// This is a real decode → composite → encode pass:
///
/// 1. [`crate::frame_harness::process_frames`] demuxes the input Y4M one frame
///    at a time.
/// 2. Each packed planar frame is bridged to an
///    [`oximedia_codec::VideoFrame`] and wrapped as
///    [`oximedia_graph::frame::FilterFrame::Video`].
/// 3. [`oximedia_graph::filters::video::TimecodeFilter`] — the real `fontdue`
///    glyph rasteriser — composites the frame's timecode string through its
///    public [`oximedia_graph::node::Node::process`] implementation.
/// 4. The composited frame is packed back and muxed into the output Y4M.
///
/// The per-frame timecode is `start + frame_index` at `fps`, so `--start` is
/// honoured (the filter's own counter always begins at zero, so the string is
/// supplied per frame as a custom overlay field).
///
/// # Errors
///
/// Returns an error if a parameter is invalid, if the input is not Y4M, if
/// `--font` is missing or unreadable, or if any stage of the pass fails. No
/// output file is written unless the whole clip succeeds.
fn cmd_burn(opts: &BurnOptions, json_output: bool) -> Result<()> {
    use oximedia_graph::filters::video::{
        FrameContext, MetadataField, OverlayElement, TextStyle, TimecodeConfig, TimecodeFilter,
    };
    use oximedia_graph::frame::FilterFrame;
    use oximedia_graph::node::{Node, NodeId};

    let BurnOptions {
        input,
        output,
        start,
        fps: fps_str,
        position,
        font_size,
        font,
    } = opts;
    let font_size = *font_size;
    let font_path = font.as_deref();

    // Validate input exists
    if !input.exists() {
        anyhow::bail!("Input file not found: {}", input.display());
    }

    let fps = parse_frame_rate(fps_str)?;
    let start_tc =
        parse_timecode(start, fps).with_context(|| format!("Invalid start timecode: {}", start))?;
    let overlay_position = parse_position(position)?;

    if font_size == 0 {
        anyhow::bail!("--font-size must be greater than zero");
    }

    // Input contract: Y4M in / Y4M out, checked before the font so a wrong
    // input format is reported before a missing font.
    let (_header, layout) = crate::frame_harness::peek_y4m_header(BURN_OP, input)?;
    crate::frame_harness::adapt::require_compositable(BURN_OP, &layout)?;

    let font_data = crate::frame_harness::font::load_font(font_path)?;

    let (fps_num, fps_den) = fps.as_rational();
    let start_frames = start_tc.to_frames();

    let style = TextStyle {
        font_size: font_size as f32,
        ..TextStyle::default()
    };
    // A single `Custom` element: the default config carries a Timecode and a
    // FrameNumber element whose text comes from the filter's own frame
    // counter, which cannot honour `--start`.
    let config = TimecodeConfig {
        elements: vec![OverlayElement::new(
            MetadataField::Custom(start_tc.to_string()),
            overlay_position,
        )
        .with_style(style)],
        context: FrameContext {
            framerate: oximedia_core::Rational::new(
                i64::from(fps_num.max(1)),
                i64::from(fps_den.max(1)),
            ),
            filename: input.file_name().map_or_else(
                || input.display().to_string(),
                |n| n.to_string_lossy().into(),
            ),
            ..FrameContext::default()
        },
        ..TimecodeConfig::default()
    };

    let mut filter = TimecodeFilter::new(NodeId(0), "timecode-burn", config.clone(), font_data)
        .map_err(|e| anyhow::anyhow!("Failed to build the timecode overlay filter: {e}"))?;

    let stats = crate::frame_harness::process_frames(BURN_OP, input, output, |index, frame| {
        // Frame N shows `start + N`, which is what a burn-in is for.
        let tc = Timecode::from_frames(start_frames + index as u64, fps)
            .map_err(|e| anyhow::anyhow!("Failed to advance the timecode: {e}"))?;
        let mut frame_config = config.clone();
        if let Some(element) = frame_config.elements.first_mut() {
            element.field = MetadataField::Custom(tc.to_string());
        }
        filter.set_config(frame_config);

        let video = crate::frame_harness::adapt::planar_to_video_frame(
            frame,
            index,
            fps_num.max(1),
            fps_den.max(1),
        )?;
        let processed = filter
            .process(Some(FilterFrame::Video(video)))
            .map_err(|e| anyhow::anyhow!("Timecode overlay filter failed: {e}"))?;
        let Some(FilterFrame::Video(video)) = processed else {
            anyhow::bail!("Timecode overlay filter returned no video frame");
        };
        *frame = crate::frame_harness::adapt::video_frame_to_planar(&video, frame.layout)?;
        Ok(())
    })?;

    if stats.bytes_changed == 0 {
        // The pass "succeeded" without touching a pixel, so the output is a
        // copy dressed up as a burn-in. Remove it and say so.
        let _ = std::fs::remove_file(output);
        anyhow::bail!(
            "timecode burn-in changed no pixels, so no output was written to {}: the overlay \
             rasterised to nothing (font '{}' may have no glyphs for the timecode digits, or \
             --font-size {font_size} may be too small for a {}x{} frame). Refusing to report a \
             successful burn-in.",
            output.display(),
            font_path.map_or_else(|| "<none>".to_string(), |p| p.display().to_string()),
            layout.luma_w,
            layout.luma_h
        );
    }

    let end_tc = Timecode::from_frames(
        start_frames + stats.frame_count.saturating_sub(1) as u64,
        fps,
    )
    .map_err(|e| anyhow::anyhow!("Failed to compute the final timecode: {e}"))?;

    if json_output {
        let obj = serde_json::json!({
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "operation": "timecode-burn",
            "input_format": "y4m",
            "output_format": "y4m",
            "width": layout.luma_w,
            "height": layout.luma_h,
            "fps": fps_str,
            "start_timecode": start_tc.to_string(),
            "end_timecode": end_tc.to_string(),
            "position": position,
            "font": font_path.map(|p| p.display().to_string()),
            "font_size": font_size,
            "frame_count": stats.frame_count,
            "input_size_bytes": stats.bytes_in,
            "output_size_bytes": stats.bytes_out,
            "bytes_changed": stats.bytes_changed,
            "note": "Overlay rasterised with the fontdue-backed \
                     oximedia_graph TimecodeFilter and composited onto every decoded frame.",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Timecode Burn-In Complete".green().bold());
    println!("  {} {}", "Input:".cyan(), input.display());
    println!("  {} {}", "Output:".cyan(), output.display());
    println!(
        "  {} {}x{} @ {}",
        "Video:".cyan(),
        layout.luma_w,
        layout.luma_h,
        fps_str
    );
    println!(
        "  {} {} \u{2192} {} ({} frames)",
        "Timecode:".cyan(),
        start_tc.to_string().yellow(),
        end_tc.to_string().yellow(),
        stats.frame_count
    );
    println!("  {} {} @ {}pt", "Overlay:".cyan(), position, font_size);
    println!(
        "  {} {} bytes ({} bytes changed)",
        "Output size:".cyan(),
        stats.bytes_out,
        stats.bytes_changed
    );

    Ok(())
}

/// Map a CLI position name onto the overlay filter's `Position`.
fn parse_position(position: &str) -> Result<oximedia_graph::filters::video::Position> {
    use oximedia_graph::filters::video::Position;
    match position {
        "top-left" => Ok(Position::TopLeft),
        "top-right" => Ok(Position::TopRight),
        "bottom-left" => Ok(Position::BottomLeft),
        "bottom-right" => Ok(Position::BottomRight),
        "center" => Ok(Position::Center),
        other => anyhow::bail!(
            "Invalid position '{}'. Supported: top-left, top-right, bottom-left, \
             bottom-right, center",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a frame-rate string into a `FrameRate` enum variant.
fn parse_frame_rate(s: &str) -> Result<FrameRate> {
    match s.to_lowercase().replace(' ', "").as_str() {
        "23.976" | "23976" | "fps23976" | "23.98" => Ok(FrameRate::Fps23976),
        "24" | "fps24" => Ok(FrameRate::Fps24),
        "25" | "fps25" => Ok(FrameRate::Fps25),
        "29.97df" | "2997df" | "29.97" | "29.97dropframe" | "ntsc" => Ok(FrameRate::Fps2997DF),
        "29.97ndf" | "2997ndf" | "29.97nondropframe" => Ok(FrameRate::Fps2997NDF),
        "30" | "fps30" => Ok(FrameRate::Fps30),
        "50" | "fps50" => Ok(FrameRate::Fps50),
        "59.94" | "5994" | "fps5994" => Ok(FrameRate::Fps5994),
        "60" | "fps60" => Ok(FrameRate::Fps60),
        other => anyhow::bail!(
            "Unknown frame rate '{}'. Supported: 23.976, 24, 25, 29.97df, 29.97ndf, 30, 50, 59.94, 60",
            other
        ),
    }
}

/// Parse a timecode string `HH:MM:SS:FF` or `HH:MM:SS;FF` into a `Timecode`.
fn parse_timecode(s: &str, fps: FrameRate) -> Result<Timecode> {
    // Accept both `:` and `;` as frame separator
    let s_norm = s.replace(';', ":");
    let parts: Vec<&str> = s_norm.splitn(4, ':').collect();
    if parts.len() != 4 {
        anyhow::bail!(
            "Invalid timecode format '{}'. Expected HH:MM:SS:FF or HH:MM:SS;FF",
            s
        );
    }

    let hours: u8 = parts[0]
        .parse()
        .with_context(|| format!("Invalid hours in timecode '{}'", s))?;
    let minutes: u8 = parts[1]
        .parse()
        .with_context(|| format!("Invalid minutes in timecode '{}'", s))?;
    let seconds: u8 = parts[2]
        .parse()
        .with_context(|| format!("Invalid seconds in timecode '{}'", s))?;
    let frames: u8 = parts[3]
        .parse()
        .with_context(|| format!("Invalid frames in timecode '{}'", s))?;

    Timecode::new(hours, minutes, seconds, frames, fps)
        .map_err(|e: TimecodeError| anyhow::anyhow!("Invalid timecode '{}': {}", s, e))
}
