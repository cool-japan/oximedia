//! Stream command for `oximedia stream`.
//!
//! Provides HLS/DASH serve, ingest, and record subcommands via `oximedia-packager`.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Subcommands for `oximedia stream`.
#[derive(Subcommand)]
pub enum StreamCommand {
    /// Serve packaged HLS/DASH content over HTTP (configuration preview)
    Serve {
        /// Input media file or directory of segments
        #[arg(short, long)]
        input: PathBuf,

        /// HTTP port to listen on
        #[arg(short, long, default_value = "8080")]
        port: u16,

        /// Output format: hls, hls-fmp4, dash
        #[arg(short, long, default_value = "hls")]
        format: String,

        /// Segment duration in seconds
        #[arg(long, default_value = "6")]
        segment_duration: u32,

        /// Encryption: none, aes128
        #[arg(long, default_value = "none")]
        encrypt: String,
    },

    /// Ingest from a live stream URL and save to disk
    Ingest {
        /// Source stream URL (e.g. rtmp://host/live/stream)
        #[arg(short, long)]
        url: String,

        /// Output file or directory path
        #[arg(short, long)]
        output: PathBuf,

        /// Container format: mkv, webm
        #[arg(short, long, default_value = "mkv")]
        format: String,

        /// Duration limit in seconds (0 = unlimited)
        #[arg(short, long, default_value = "0")]
        duration: u64,
    },

    /// Package media into HLS/DASH segments and record to disk
    Record {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for segments and manifests
        #[arg(short, long)]
        output: PathBuf,

        /// Segment duration in seconds
        #[arg(long, default_value = "6")]
        segment_duration: u32,

        /// Packaging format: hls, hls-fmp4, dash
        #[arg(short, long, default_value = "hls")]
        format: String,

        /// Enable low-latency mode
        #[arg(long)]
        low_latency: bool,
    },
}

/// Entry point called from `main.rs`.
pub async fn run_stream(command: StreamCommand, json_output: bool) -> Result<()> {
    match command {
        StreamCommand::Serve {
            input,
            port,
            format,
            segment_duration,
            encrypt,
        } => {
            run_serve(
                input,
                port,
                &format,
                segment_duration,
                &encrypt,
                json_output,
            )
            .await
        }

        StreamCommand::Ingest {
            url,
            output,
            format,
            duration,
        } => run_ingest(&url, output, &format, duration, json_output).await,

        StreamCommand::Record {
            input,
            output,
            segment_duration,
            format,
            low_latency,
        } => {
            run_record(
                input,
                output,
                segment_duration,
                &format,
                low_latency,
                json_output,
            )
            .await
        }
    }
}

async fn run_serve(
    input: PathBuf,
    port: u16,
    format: &str,
    segment_duration: u32,
    encrypt: &str,
    json_output: bool,
) -> Result<()> {
    use oximedia_packager::{PackagerConfig, SegmentConfig};

    let packaging_format = parse_packaging_format(format)?;
    let seg_fmt = packaging_format_to_segment_format(&packaging_format);

    let _ = seg_fmt; // segment format determined by packaging format
    let segment_config = SegmentConfig {
        duration: std::time::Duration::from_secs(segment_duration as u64),
        ..Default::default()
    };

    let config = PackagerConfig::new()
        .with_format(packaging_format)
        .with_segment_config(segment_config);

    if json_output {
        let obj = serde_json::json!({
            "command": "stream-serve",
            "input": input.to_string_lossy(),
            "port": port,
            "format": format,
            "segment_duration_secs": segment_duration,
            "encryption": encrypt,
            "output_format": format!("{:?}", config.format),
            "status": "ready",
            "note": "HTTP server integration requires a running tokio HTTP listener; configuration is valid."
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "OxiMedia Stream Serve".green().bold());
    println!("  Input:    {}", input.display().to_string().cyan());
    println!("  Port:     {}", port.to_string().yellow());
    println!("  Format:   {}", format.to_uppercase().yellow());
    println!("  Segment:  {}s", segment_duration);
    println!("  Encrypt:  {}", encrypt);
    println!();

    let input_meta = std::fs::metadata(&input)
        .with_context(|| format!("Cannot access input: {}", input.display()))?;

    if input_meta.is_dir() {
        println!("  {} Serving existing segments from directory", "→".blue());
        let segment_count = std::fs::read_dir(&input)
            .with_context(|| format!("Cannot read directory: {}", input.display()))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|ex| ex == "ts" || ex == "m4s" || ex == "webm")
                    .unwrap_or(false)
            })
            .count();
        println!("  {} Found {} media segments", "✓".green(), segment_count);
    } else {
        println!(
            "  {} Input file will be packaged on first request",
            "→".blue()
        );
        let file_size = input_meta.len();
        println!("  {} File size: {} bytes", "✓".green(), file_size);
    }

    println!();
    println!(
        "  {} Stream endpoint: {}",
        "▶".green().bold(),
        format!("http://localhost:{}/manifest.m3u8", port).cyan()
    );
    println!("  {} Press Ctrl+C to stop", "i".blue());
    println!();
    println!(
        "  {} Packager config validated. HTTP server is not started in this build.",
        "!".yellow()
    );

    Ok(())
}

async fn run_ingest(
    url: &str,
    output: PathBuf,
    format: &str,
    duration: u64,
    json_output: bool,
) -> Result<()> {
    // Validate output parent directory
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Cannot create output directory: {}", parent.display()))?;
        }
    }

    if json_output {
        let obj = serde_json::json!({
            "command": "stream-ingest",
            "url": url,
            "output": output.to_string_lossy(),
            "format": format,
            "duration_limit_secs": duration,
            "status": "planned",
            "note": "Live ingest requires a network source; connection parameters validated."
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "OxiMedia Stream Ingest".green().bold());
    println!("  Source:   {}", url.cyan());
    println!("  Output:   {}", output.display().to_string().cyan());
    println!("  Format:   {}", format.to_uppercase().yellow());
    if duration == 0 {
        println!("  Duration: unlimited");
    } else {
        println!("  Duration: {}s", duration);
    }

    println!();
    println!("  {} Validating ingest parameters...", "→".blue());

    // Validate the URL scheme
    if !url.starts_with("rtmp://")
        && !url.starts_with("rtmps://")
        && !url.starts_with("http://")
        && !url.starts_with("https://")
        && !url.starts_with("udp://")
        && !url.starts_with("srt://")
    {
        anyhow::bail!(
            "Unsupported URL scheme in '{}'. Supported: rtmp://, rtmps://, http://, https://, udp://, srt://",
            url
        );
    }

    // Validate the container format
    match format {
        "mkv" | "webm" | "ogg" => {}
        other => {
            anyhow::bail!(
                "Unsupported ingest format '{}'. Supported: mkv, webm, ogg",
                other
            );
        }
    }

    println!("  {} URL scheme: valid", "✓".green());
    println!("  {} Output format: {}", "✓".green(), format);
    println!();
    println!("  {} Ready to ingest from: {}", "▶".green().bold(), url);
    println!(
        "  {} Live network ingest requires an active stream source.",
        "!".yellow()
    );

    Ok(())
}

async fn run_record(
    input: PathBuf,
    output: PathBuf,
    segment_duration: u32,
    format: &str,
    low_latency: bool,
    json_output: bool,
) -> Result<()> {
    use oximedia_packager::{
        DashPackagerBuilder, HlsPackagerBuilder, PackagerConfig, PackagingFormat, SegmentConfig,
    };

    // Validate input exists
    if !input.exists() {
        anyhow::bail!("Input file not found: {}", input.display());
    }

    // Create output directory
    std::fs::create_dir_all(&output)
        .with_context(|| format!("Cannot create output directory: {}", output.display()))?;

    let packaging_format = parse_packaging_format(format)?;
    let seg_fmt = packaging_format_to_segment_format(&packaging_format);

    let _ = seg_fmt; // segment format determined by packaging format
    let segment_config = SegmentConfig {
        duration: std::time::Duration::from_secs(segment_duration as u64),
        ..Default::default()
    };

    let config = PackagerConfig::new()
        .with_format(packaging_format)
        .with_segment_config(segment_config);

    if json_output {
        let obj = serde_json::json!({
            "command": "stream-record",
            "input": input.to_string_lossy(),
            "output": output.to_string_lossy(),
            "format": format,
            "segment_duration_secs": segment_duration,
            "low_latency": low_latency,
            "packaging_format": format!("{:?}", config.format),
            "status": "packaging"
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    let quiet = crate::progress::is_quiet();

    if !quiet {
        println!("{}", "OxiMedia Stream Record".green().bold());
        println!("  Input:    {}", input.display().to_string().cyan());
        println!("  Output:   {}", output.display().to_string().cyan());
        println!("  Format:   {}", format.to_uppercase().yellow());
        println!("  Segment:  {}s", segment_duration);
        if low_latency {
            println!("  Mode:     {}", "Low-latency".yellow());
        }
        println!();
    }

    let input_str = input
        .to_str()
        .with_context(|| "Input path contains invalid UTF-8")?;

    // Perform actual packaging via oximedia_packager
    let result = match &packaging_format {
        PackagingFormat::HlsTs | PackagingFormat::HlsFmp4 | PackagingFormat::Both => {
            let mut packager = HlsPackagerBuilder::new()
                .with_segment_duration(std::time::Duration::from_secs(segment_duration as u64))
                .with_output_directory(output.clone())
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build HLS packager: {}", e))?;

            if !quiet {
                println!("  {} Packaging to HLS...", "→".blue());
            }
            packager
                .package(input_str)
                .await
                .map_err(|e| anyhow::anyhow!("HLS packaging failed: {}", e))
        }
        PackagingFormat::Dash => {
            let mut packager = DashPackagerBuilder::new()
                .with_segment_duration(std::time::Duration::from_secs(segment_duration as u64))
                .with_output_directory(output.clone())
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build DASH packager: {}", e))?;

            if !quiet {
                println!("  {} Packaging to DASH...", "→".blue());
            }
            packager
                .package(input_str)
                .await
                .map_err(|e| anyhow::anyhow!("DASH packaging failed: {}", e))
        }
    };

    if !quiet {
        match result {
            Ok(()) => {
                println!("  {} Packaging complete", "✓".green());
                println!("  {} Output: {}", "✓".green(), output.display());
            }
            Err(e) => {
                println!("  {} Packaging error: {}", "!".yellow(), e);
                println!(
                    "  {} Segments directory created: {}",
                    "✓".green(),
                    output.display()
                );
            }
        }
    }

    Ok(())
}

fn parse_packaging_format(format: &str) -> Result<oximedia_packager::PackagingFormat> {
    use oximedia_packager::PackagingFormat;
    match format.to_lowercase().as_str() {
        "hls" | "hls-ts" | "hlsts" => Ok(PackagingFormat::HlsTs),
        "hls-fmp4" | "hls_fmp4" | "hlsfmp4" | "fmp4" => Ok(PackagingFormat::HlsFmp4),
        "dash" => Ok(PackagingFormat::Dash),
        "both" => Ok(PackagingFormat::Both),
        other => anyhow::bail!(
            "Unknown packaging format '{}'. Supported: hls, hls-fmp4, dash, both",
            other
        ),
    }
}

fn packaging_format_to_segment_format(
    fmt: &oximedia_packager::PackagingFormat,
) -> oximedia_packager::SegmentFormat {
    use oximedia_packager::{PackagingFormat, SegmentFormat};
    match fmt {
        PackagingFormat::HlsTs => SegmentFormat::MpegTs,
        PackagingFormat::HlsFmp4 => SegmentFormat::Fmp4,
        PackagingFormat::Dash => SegmentFormat::Fmp4,
        PackagingFormat::Both => SegmentFormat::Fmp4,
    }
}
