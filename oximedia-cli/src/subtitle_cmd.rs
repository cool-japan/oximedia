//! Subtitle conversion, extraction, burn-in, and synchronization.
//!
//! Provides subtitle-related commands using the `oximedia-subtitle` crate.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Subtitle command subcommands.
#[derive(Subcommand, Debug)]
pub enum SubtitleCommand {
    /// Convert subtitles between formats (SRT, WebVTT, ASS)
    Convert {
        /// Input subtitle file
        #[arg(short, long)]
        input: PathBuf,

        /// Output subtitle file
        #[arg(short, long)]
        output: PathBuf,

        /// Input format: srt, vtt, ass (auto-detected if omitted)
        #[arg(long)]
        from: Option<String>,

        /// Output format: srt, vtt, ass
        #[arg(long)]
        to: Option<String>,

        /// Apply timing offset in milliseconds
        #[arg(long, default_value = "0")]
        offset: i64,
    },

    /// Extract subtitles from a container (MKV, WebM)
    Extract {
        /// Input video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output subtitle file
        #[arg(short, long)]
        output: PathBuf,

        /// Subtitle track index
        #[arg(long, default_value = "0")]
        track: usize,

        /// Output format: srt, vtt, ass
        #[arg(long, default_value = "srt")]
        format: String,
    },

    /// Burn subtitles into an uncompressed Y4M clip (requires --font)
    Burn {
        /// Input video file (uncompressed YUV4MPEG2 / .y4m)
        #[arg(short, long)]
        input: PathBuf,

        /// Subtitle file to burn
        #[arg(long)]
        subtitle: PathBuf,

        /// Output video file (YUV4MPEG2 / .y4m)
        #[arg(short, long)]
        output: PathBuf,

        /// Font size in points
        #[arg(long, default_value = "24")]
        font_size: u32,

        /// Subtitle format: srt, vtt, ass (auto-detected if omitted)
        #[arg(long)]
        format: Option<String>,

        /// TrueType/OpenType font used to rasterise cues (required;
        /// OxiMedia ships no font and never picks a system one)
        #[arg(long, value_name = "PATH")]
        font: Option<PathBuf>,
    },

    /// Adjust subtitle timing (sync offset)
    Sync {
        /// Input subtitle file
        #[arg(short, long)]
        input: PathBuf,

        /// Output subtitle file (defaults to overwriting input)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Timing offset in milliseconds (positive = later, negative = earlier)
        #[arg(long)]
        offset: i64,

        /// Scale timing by factor (e.g., 1.001 for 23.976->24fps correction)
        #[arg(long)]
        scale: Option<f64>,
    },
}

/// Handle subtitle command dispatch.
pub async fn handle_subtitle_command(command: SubtitleCommand, json_output: bool) -> Result<()> {
    match command {
        SubtitleCommand::Convert {
            input,
            output,
            from,
            to,
            offset,
        } => convert_subtitles(&input, &output, from.as_deref(), to.as_deref(), offset).await,
        SubtitleCommand::Extract {
            input,
            output,
            track,
            format,
        } => extract_subtitles(&input, &output, track, &format, json_output).await,
        SubtitleCommand::Burn {
            input,
            subtitle,
            output,
            font_size,
            format,
            font,
        } => {
            burn_subtitles(
                &input,
                &subtitle,
                &output,
                font_size,
                format.as_deref(),
                font,
                json_output,
            )
            .await
        }
        SubtitleCommand::Sync {
            input,
            output,
            offset,
            scale,
        } => sync_subtitles(&input, output.as_ref(), offset, scale).await,
    }
}

/// Detect subtitle format from file extension.
fn detect_format(path: &PathBuf) -> Option<&str> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| match ext.to_lowercase().as_str() {
            "srt" => "srt",
            "vtt" | "webvtt" => "vtt",
            "ass" | "ssa" => "ass",
            _ => "unknown",
        })
}

/// Parse subtitles from text using the specified format.
fn parse_subtitles(text: &str, format: &str) -> Result<Vec<oximedia_subtitle::Subtitle>> {
    match format {
        "srt" => oximedia_subtitle::SrtParser::parse(text)
            .map_err(|e| anyhow::anyhow!("Failed to parse SRT: {}", e)),
        "vtt" | "webvtt" => oximedia_subtitle::WebVttParser::parse(text)
            .map_err(|e| anyhow::anyhow!("Failed to parse WebVTT: {}", e)),
        "ass" | "ssa" => oximedia_subtitle::AssParser::parse(text)
            .map_err(|e| anyhow::anyhow!("Failed to parse ASS: {}", e)),
        other => Err(anyhow::anyhow!(
            "Unknown subtitle format '{}'. Supported: srt, vtt, ass",
            other
        )),
    }
}

/// Format a timestamp in milliseconds to SRT format (HH:MM:SS,mmm).
fn format_srt_timestamp(ms: i64) -> String {
    let total_seconds = ms / 1000;
    let millis = ms % 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02},{:03}", hours, minutes, seconds, millis)
}

/// Format a timestamp in milliseconds to WebVTT format (HH:MM:SS.mmm).
fn format_vtt_timestamp(ms: i64) -> String {
    let total_seconds = ms / 1000;
    let millis = ms % 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, seconds, millis)
}

/// Serialize subtitles to a string in the specified format.
fn serialize_subtitles(subs: &[oximedia_subtitle::Subtitle], format: &str) -> Result<String> {
    let mut output = String::new();

    match format {
        "srt" => {
            for (i, sub) in subs.iter().enumerate() {
                output.push_str(&format!("{}\n", i + 1));
                output.push_str(&format!(
                    "{} --> {}\n",
                    format_srt_timestamp(sub.start_time),
                    format_srt_timestamp(sub.end_time)
                ));
                output.push_str(&sub.text);
                output.push_str("\n\n");
            }
        }
        "vtt" | "webvtt" => {
            output.push_str("WEBVTT\n\n");
            for sub in subs {
                if let Some(ref id) = sub.id {
                    output.push_str(id);
                    output.push('\n');
                }
                output.push_str(&format!(
                    "{} --> {}\n",
                    format_vtt_timestamp(sub.start_time),
                    format_vtt_timestamp(sub.end_time)
                ));
                output.push_str(&sub.text);
                output.push_str("\n\n");
            }
        }
        "ass" | "ssa" => {
            output.push_str("[Script Info]\nScriptType: v4.00+\n\n");
            output.push_str("[V4+ Styles]\n");
            output.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
            output.push_str("Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,2,2,10,10,10,1\n\n");
            output.push_str("[Events]\n");
            output.push_str(
                "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
            );
            for sub in subs {
                let start = format_ass_timestamp(sub.start_time);
                let end = format_ass_timestamp(sub.end_time);
                output.push_str(&format!(
                    "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
                    start, end, sub.text
                ));
            }
        }
        other => {
            return Err(anyhow::anyhow!("Unsupported output format: {}", other));
        }
    }

    Ok(output)
}

/// Format a timestamp in milliseconds to ASS format (H:MM:SS.cc).
fn format_ass_timestamp(ms: i64) -> String {
    let total_seconds = ms / 1000;
    let centiseconds = (ms % 1000) / 10;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!(
        "{}:{:02}:{:02}.{:02}",
        hours, minutes, seconds, centiseconds
    )
}

/// Convert subtitles between formats.
async fn convert_subtitles(
    input: &PathBuf,
    output: &PathBuf,
    from: Option<&str>,
    to: Option<&str>,
    offset: i64,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    // Detect input format
    let input_format = from.unwrap_or_else(|| detect_format(input).unwrap_or("srt"));

    // Detect output format
    let output_format = to.unwrap_or_else(|| detect_format(output).unwrap_or("srt"));

    // Read input file
    let text = tokio::fs::read_to_string(input)
        .await
        .context("Failed to read input subtitle file")?;

    // Parse subtitles
    let mut subs = parse_subtitles(&text, input_format)?;

    // Apply offset if non-zero
    if offset != 0 {
        for sub in &mut subs {
            sub.start_time += offset;
            sub.end_time += offset;
        }
    }

    // Serialize to output format
    let output_text = serialize_subtitles(&subs, output_format)?;

    // Write output
    tokio::fs::write(output, &output_text)
        .await
        .context("Failed to write output subtitle file")?;

    println!("{}", "Subtitle Conversion".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", input.display());
    println!("{:20} {}", "Output:", output.display());
    println!("{:20} {}", "From:", input_format);
    println!("{:20} {}", "To:", output_format);
    println!("{:20} {} subtitle(s)", "Converted:", subs.len());
    if offset != 0 {
        println!("{:20} {}ms", "Offset applied:", offset);
    }
    println!();
    println!("{}", "Conversion complete.".green());

    Ok(())
}

/// Extract subtitles from a container.
///
/// Delegates to the proven real Matroska/WebM subtitle demux shared with
/// `oximedia captions extract` (`crate::captions_cmd::run_captions_extract`),
/// which genuinely parses EBML track headers and subtitle block payloads.
/// Containers other than Matroska/WebM fail with an honest, actionable error
/// instead of pretending an extraction happened.
async fn extract_subtitles(
    input: &PathBuf,
    output: &PathBuf,
    track: usize,
    format: &str,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    // Validate the requested output format against this command's documented
    // surface (srt, vtt, ass) before delegating; the shared captions exporter
    // accepts more formats, but `subtitle extract --help` only promises these.
    match format.to_lowercase().as_str() {
        "srt" | "vtt" | "webvtt" | "ass" => {}
        other => {
            return Err(anyhow::anyhow!(
                "Unknown subtitle output format '{}'. Supported: srt, vtt, ass",
                other
            ));
        }
    }

    let opts = crate::captions_cmd::CaptionsExtractOptions {
        input: input.clone(),
        output: output.clone(),
        format: format.to_string(),
        track,
    };
    crate::captions_cmd::run_captions_extract(opts, json_output).await
}

/// Operation label used in the frame harness's error messages.
const SUBTITLE_BURN_OP: &str = "subtitle burn";

/// Parameters of the `subtitle burn` subcommand, bundled so the whole burn
/// pass can run as one `spawn_blocking` closure (subtitle parsing, Y4M
/// decode, glyph rasterisation and re-encode are all synchronous work).
struct BurnSubtitleOptions {
    input: PathBuf,
    subtitle: PathBuf,
    output: PathBuf,
    font_size: u32,
    format: Option<String>,
    font: Option<PathBuf>,
}

/// Burn subtitles into every frame of a Y4M clip whose timestamp falls
/// inside a cue's `[start_time, end_time)` range.
///
/// This is a real decode -> composite -> encode pass: burn-in is CPU-bound
/// and fully synchronous, so — like `timecode burn` — it runs on the
/// blocking pool rather than stalling the async runtime.
///
/// # Errors
///
/// Returns an error if the input/subtitle files don't exist, the subtitle
/// file doesn't parse, the input is not Y4M, `--font` is missing/invalid, no
/// cue overlaps the clip's time range, or the overlay rasterised to nothing.
/// No output file is written unless the whole clip succeeds.
async fn burn_subtitles(
    input: &PathBuf,
    subtitle: &PathBuf,
    output: &PathBuf,
    font_size: u32,
    format: Option<&str>,
    font: Option<PathBuf>,
    json_output: bool,
) -> Result<()> {
    let opts = BurnSubtitleOptions {
        input: input.clone(),
        subtitle: subtitle.clone(),
        output: output.clone(),
        font_size,
        format: format.map(str::to_string),
        font,
    };
    tokio::task::spawn_blocking(move || cmd_burn_subtitles(&opts, json_output))
        .await
        .map_err(|join_err| anyhow::anyhow!("subtitle burn-in task panicked: {join_err}"))?
}

/// Synchronous implementation of `subtitle burn`, run inside `spawn_blocking`
/// by [`burn_subtitles`].
///
/// # Pipeline
///
/// 1. Parse the subtitle file with the existing real `oximedia-subtitle`
///    parsers (already used by `convert`/`sync`), giving `[Subtitle]` cues
///    with millisecond `start_time`/`end_time`.
/// 2. Validate the input contract in the same order `timecode burn` does:
///    Y4M header -> chroma-divisibility (`require_compositable`) -> font.
/// 3. [`crate::frame_harness::process_frames`] demuxes one frame at a time;
///    for each frame's timestamp, every cue whose range covers it is laid
///    out with [`crate::frame_harness::text::TextRenderer`] (real `fontdue`
///    glyphs from the user's `--font`) and alpha-composited at a bottom-
///    centre, safe-area-aware position from
///    `oximedia_subtitle::burn_in::BurnInRenderer::compute_position` (real
///    position math; deliberately *not* that module's `BitmapFont`/
///    `SubtitleBurnIn`, which draws a built-in approximation font rather
///    than the user's real glyphs). Multiple simultaneously-active cues
///    stack upward from the bottom.
fn cmd_burn_subtitles(opts: &BurnSubtitleOptions, json_output: bool) -> Result<()> {
    use crate::frame_harness::text::{TextColor, TextRenderer};
    use oximedia_subtitle::burn_in::{BurnInAlignment, BurnInConfig, BurnInRenderer};

    if !opts.input.exists() {
        anyhow::bail!("Input video not found: {}", opts.input.display());
    }
    if !opts.subtitle.exists() {
        anyhow::bail!("Subtitle file not found: {}", opts.subtitle.display());
    }
    if opts.font_size == 0 {
        anyhow::bail!("--font-size must be greater than zero");
    }

    let sub_format = opts
        .format
        .as_deref()
        .unwrap_or_else(|| detect_format(&opts.subtitle).unwrap_or("srt"));
    let text = std::fs::read_to_string(&opts.subtitle)
        .with_context(|| format!("Failed to read subtitle file: {}", opts.subtitle.display()))?;
    let subs = parse_subtitles(&text, sub_format)?;
    if subs.is_empty() {
        anyhow::bail!(
            "Subtitle file '{}' contains no cues to burn",
            opts.subtitle.display()
        );
    }

    // Input contract: Y4M in / Y4M out, checked before the font so a wrong
    // input format is reported before a missing font (mirrors `timecode
    // burn`'s validation order).
    let (header, layout) = crate::frame_harness::peek_y4m_header(SUBTITLE_BURN_OP, &opts.input)?;
    crate::frame_harness::adapt::require_compositable(SUBTITLE_BURN_OP, &layout)?;
    let font_bytes = crate::frame_harness::font::load_font(opts.font.as_deref())?;

    let fps = f64::from(header.fps_num.max(1)) / f64::from(header.fps_den.max(1));
    let frame_w = layout.luma_w as u32;
    let frame_h = layout.luma_h as u32;

    let mut renderer = TextRenderer::new(font_bytes)?;
    let burn_position = BurnInRenderer::new(BurnInConfig::web());
    let max_text_width = frame_w as f32 * 0.8;

    let mut composited_frames = 0usize;
    let stats = crate::frame_harness::process_frames(
        SUBTITLE_BURN_OP,
        &opts.input,
        &opts.output,
        |index, frame| {
            let t_ms = (index as f64 * 1000.0 / fps).round() as i64;
            let active: Vec<&oximedia_subtitle::Subtitle> = subs
                .iter()
                .filter(|s| s.start_time <= t_ms && t_ms < s.end_time)
                .collect();
            if active.is_empty() {
                return Ok(());
            }

            let mut painted_here = 0usize;
            let mut stack_offset = 0u32;
            for sub in &active {
                let glyphs =
                    renderer.layout(&sub.text, opts.font_size as f32, Some(max_text_width));
                let (tw, th) = TextRenderer::bounds(&glyphs);
                let (x, base_y) = burn_position.compute_position(
                    tw.ceil() as u32,
                    th.ceil() as u32,
                    frame_w,
                    frame_h,
                    &BurnInAlignment::BottomCenter,
                );
                let y = base_y.saturating_sub(stack_offset);
                painted_here += renderer.paint(
                    frame,
                    &glyphs,
                    opts.font_size as f32,
                    x as i32,
                    y as i32,
                    TextColor::WHITE,
                )?;
                stack_offset += th.ceil() as u32 + 4;
            }
            if painted_here > 0 {
                composited_frames += 1;
            }
            Ok(())
        },
    )?;

    if composited_frames == 0 {
        let _ = std::fs::remove_file(&opts.output);
        let clip_duration_s = stats.frame_count as f64 / fps;
        let cue_min_s = subs.iter().map(|s| s.start_time).min().unwrap_or(0) as f64 / 1000.0;
        let cue_max_s = subs.iter().map(|s| s.end_time).max().unwrap_or(0) as f64 / 1000.0;
        anyhow::bail!(
            "subtitle burn-in composited no cues onto any frame: the clip covers 0.00s-{:.2}s \
             ({} frames at {:.3} fps), but the {} cue(s) in '{}' span {:.2}s-{:.2}s, which does \
             not overlap. No output was written to {}.",
            clip_duration_s,
            stats.frame_count,
            fps,
            subs.len(),
            opts.subtitle.display(),
            cue_min_s,
            cue_max_s,
            opts.output.display()
        );
    }
    if stats.bytes_changed == 0 {
        let _ = std::fs::remove_file(&opts.output);
        anyhow::bail!(
            "subtitle burn-in composited cues onto {composited_frames} frame(s) but changed no \
             pixels, so no output was written to {}: the overlay rasterised to nothing (font \
             '{}' may have no glyphs for this text, or --font-size {} may be too small for a \
             {}x{} frame). Refusing to report a successful burn-in.",
            opts.output.display(),
            opts.font
                .as_ref()
                .map_or_else(|| "<none>".to_string(), |p| p.display().to_string()),
            opts.font_size,
            frame_w,
            frame_h
        );
    }

    if json_output {
        let obj = serde_json::json!({
            "input": opts.input.display().to_string(),
            "subtitle": opts.subtitle.display().to_string(),
            "output": opts.output.display().to_string(),
            "operation": "subtitle-burn",
            "input_format": "y4m",
            "output_format": "y4m",
            "subtitle_format": sub_format,
            "width": frame_w,
            "height": frame_h,
            "font": opts.font.as_ref().map(|p| p.display().to_string()),
            "font_size": opts.font_size,
            "cues_total": subs.len(),
            "frames_with_cues": composited_frames,
            "frame_count": stats.frame_count,
            "input_size_bytes": stats.bytes_in,
            "output_size_bytes": stats.bytes_out,
            "bytes_changed": stats.bytes_changed,
            "note": "Real glyphs rasterised with the user-supplied font (fontdue-backed \
                     oximedia_subtitle::font) and alpha-composited onto every frame whose \
                     timestamp falls inside a cue's [start_time, end_time) range.",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Subtitle Burn-In Complete".green().bold());
    println!("  {} {}", "Input:".cyan(), opts.input.display());
    println!("  {} {}", "Subtitle:".cyan(), opts.subtitle.display());
    println!("  {} {}", "Output:".cyan(), opts.output.display());
    println!(
        "  {} {}x{} @ {:.3} fps",
        "Video:".cyan(),
        frame_w,
        frame_h,
        fps
    );
    println!(
        "  {} {} cue(s), {} frame(s) with a cue drawn",
        "Cues:".cyan(),
        subs.len(),
        composited_frames
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

/// Adjust subtitle timing.
async fn sync_subtitles(
    input: &PathBuf,
    output: Option<&PathBuf>,
    offset: i64,
    scale: Option<f64>,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let format = detect_format(input).unwrap_or("srt");

    // Read and parse
    let text = tokio::fs::read_to_string(input)
        .await
        .context("Failed to read subtitle file")?;
    let mut subs = parse_subtitles(&text, format)?;

    // Apply scale factor
    if let Some(factor) = scale {
        for sub in &mut subs {
            sub.start_time = (sub.start_time as f64 * factor) as i64;
            sub.end_time = (sub.end_time as f64 * factor) as i64;
        }
    }

    // Apply offset
    if offset != 0 {
        for sub in &mut subs {
            sub.start_time += offset;
            sub.end_time += offset;
        }
    }

    // Serialize
    let output_text = serialize_subtitles(&subs, format)?;

    // Write to output or input
    let out_path = output.unwrap_or(input);
    tokio::fs::write(out_path, &output_text)
        .await
        .context("Failed to write subtitle file")?;

    println!("{}", "Subtitle Sync".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Input:", input.display());
    println!("{:20} {}", "Output:", out_path.display());
    println!("{:20} {}ms", "Offset:", offset);
    if let Some(factor) = scale {
        println!("{:20} {}", "Scale factor:", factor);
    }
    println!("{:20} {} subtitle(s)", "Processed:", subs.len());
    println!();
    println!("{}", "Sync complete.".green());

    Ok(())
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
            "oximedia_subtitle_cmd_{}_{name}",
            std::process::id()
        ))
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

    fn write_srt(path: &std::path::Path, cue: &str) {
        std::fs::write(path, cue).expect("write srt fixture");
    }

    #[test]
    fn test_detect_format() {
        assert_eq!(detect_format(&PathBuf::from("a.srt")), Some("srt"));
        assert_eq!(detect_format(&PathBuf::from("a.vtt")), Some("vtt"));
        assert_eq!(detect_format(&PathBuf::from("a.ass")), Some("ass"));
        assert_eq!(detect_format(&PathBuf::from("a.bin")), Some("unknown"));
    }

    #[test]
    fn test_parse_subtitles_srt() {
        let subs = parse_subtitles("1\n00:00:00,000 --> 00:00:02,000\nHello\n\n", "srt")
            .expect("must parse");
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].text, "Hello");
    }

    // ── burn_subtitles: honest errors + real burn-in ────────────────────────

    #[tokio::test]
    async fn burn_missing_video_errors() {
        let input = temp_path("burn_missing_video.y4m");
        let subtitle = temp_path("burn_missing_video.srt");
        let output = temp_path("burn_missing_video_out.y4m");
        let _ = std::fs::remove_file(&input);
        write_srt(&subtitle, "1\n00:00:00,000 --> 00:00:02,000\nHello\n\n");
        let _ = std::fs::remove_file(&output);

        let err = burn_subtitles(&input, &subtitle, &output, 24, None, None, false)
            .await
            .expect_err("missing video must fail");
        assert!(err.to_string().contains("Input video not found"));
        assert!(!output.exists());

        std::fs::remove_file(&subtitle).ok();
    }

    #[tokio::test]
    async fn burn_missing_subtitle_errors() {
        let input = temp_path("burn_missing_sub.y4m");
        let subtitle = temp_path("burn_missing_sub.srt");
        let output = temp_path("burn_missing_sub_out.y4m");
        write_flat_y4m(&input, 16, 16, 1, 128);
        let _ = std::fs::remove_file(&subtitle);
        let _ = std::fs::remove_file(&output);

        let err = burn_subtitles(&input, &subtitle, &output, 24, None, None, false)
            .await
            .expect_err("missing subtitle must fail");
        assert!(err.to_string().contains("Subtitle file not found"));
        assert!(!output.exists());

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn burn_requires_y4m_input() {
        let input = temp_path("burn_not_y4m.webm");
        let subtitle = temp_path("burn_not_y4m.srt");
        let output = temp_path("burn_not_y4m_out.y4m");
        std::fs::write(&input, b"\x1aE\xdf\xa3not-a-y4m").expect("write fake webm");
        write_srt(&subtitle, "1\n00:00:00,000 --> 00:00:02,000\nHello\n\n");
        let _ = std::fs::remove_file(&output);

        let err = burn_subtitles(&input, &subtitle, &output, 24, None, None, false)
            .await
            .expect_err("a non-Y4M input must be refused");
        let msg = err.to_string();
        assert!(msg.contains("YUV4MPEG2"), "got: {msg}");
        assert!(msg.contains("oximedia transcode"), "got: {msg}");
        assert!(!output.exists());

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&subtitle).ok();
    }

    /// Without `--font` the command refuses honestly and writes nothing.
    /// This test always runs: it needs no font precisely because the point
    /// is that OxiMedia ships none.
    #[tokio::test]
    async fn burn_without_font_errors_honestly() {
        let input = temp_path("burn_nofont.y4m");
        let subtitle = temp_path("burn_nofont.srt");
        let output = temp_path("burn_nofont_out.y4m");
        write_flat_y4m(&input, 32, 32, 2, 128);
        write_srt(&subtitle, "1\n00:00:00,000 --> 00:00:02,000\nHello\n\n");
        let _ = std::fs::remove_file(&output);

        let err = burn_subtitles(&input, &subtitle, &output, 24, None, None, false)
            .await
            .expect_err("burn-in without a font must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("--font"),
            "error must name the flag, got: {msg}"
        );
        assert!(
            !output.exists(),
            "no output may be fabricated without a font"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&subtitle).ok();
    }

    /// A cue whose range never overlaps the clip must fail honestly rather
    /// than silently writing an unmodified copy.
    #[tokio::test]
    async fn burn_cue_outside_clip_range_errors() {
        let Some(font) = std::env::var_os("OXIMEDIA_TEST_FONT").map(PathBuf::from) else {
            eprintln!(
                "SKIP burn_cue_outside_clip_range_errors: no font available. Re-run with \
                 OXIMEDIA_TEST_FONT=/path/to/font.ttf."
            );
            return;
        };

        let input = temp_path("burn_outside_range.y4m");
        let subtitle = temp_path("burn_outside_range.srt");
        let output = temp_path("burn_outside_range_out.y4m");
        // 2 frames @ 25fps = 0.08s of footage; the cue starts at 10s.
        write_flat_y4m(&input, 32, 32, 2, 128);
        write_srt(&subtitle, "1\n00:00:10,000 --> 00:00:12,000\nHello\n\n");
        let _ = std::fs::remove_file(&output);

        let err = burn_subtitles(&input, &subtitle, &output, 24, None, Some(font), false)
            .await
            .expect_err("a cue outside the clip's time range must fail");
        assert!(err.to_string().contains("does not overlap"), "got: {err}");
        assert!(!output.exists());

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&subtitle).ok();
    }

    /// Real glyph rasterisation: burns a cue into every overlapping frame
    /// and checks the overlay actually landed in the pixels.
    ///
    /// Needs a font: set `OXIMEDIA_TEST_FONT=/path/to/font.ttf`.
    #[tokio::test]
    async fn burn_renders_real_glyphs() {
        let Some(font) = std::env::var_os("OXIMEDIA_TEST_FONT").map(PathBuf::from) else {
            eprintln!(
                "SKIP burn_renders_real_glyphs: no font available. Re-run with \
                 OXIMEDIA_TEST_FONT=/path/to/font.ttf."
            );
            return;
        };

        let input = temp_path("burn_real.y4m");
        let subtitle = temp_path("burn_real.srt");
        let output = temp_path("burn_real_out.y4m");
        write_flat_y4m(&input, 320, 96, 3, 128);
        write_srt(
            &subtitle,
            "1\n00:00:00,000 --> 00:00:02,000\nHello world\n\n",
        );
        let _ = std::fs::remove_file(&output);

        burn_subtitles(&input, &subtitle, &output, 28, None, Some(font), false)
            .await
            .expect("burn-in must succeed with a real font");

        let bytes = std::fs::read(&output).expect("read output");
        assert!(bytes.starts_with(b"YUV4MPEG2 W320 H96"));

        let mut demuxer =
            oximedia_container::demux::y4m::Y4mDemuxer::new(std::io::Cursor::new(bytes.as_slice()))
                .expect("parse output y4m");
        let frames = demuxer.read_all_frames().expect("read output frames");
        assert_eq!(frames.len(), 3, "every frame must be re-encoded");

        let luma_len = 320 * 96;
        for (i, frame) in frames.iter().enumerate() {
            let changed = frame[..luma_len].iter().filter(|&&s| s != 128).count();
            assert!(
                changed > 50,
                "frame {i} must carry a rasterised overlay, but only {changed} luma samples \
                 differ from the flat background"
            );
        }

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&subtitle).ok();
        std::fs::remove_file(&output).ok();
    }
}
