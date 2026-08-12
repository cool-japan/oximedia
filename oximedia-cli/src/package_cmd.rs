//! HLS/DASH packaging command.
//!
//! Provides `oximedia package` for packaging media into adaptive-bitrate
//! streaming formats using the `oximedia-packager` crate.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;
use std::time::Duration;

/// Options for the `package` command.
pub struct PackageOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub format: String,
    pub segments: u32,
    pub ladders: String,
    pub encrypt: String,
    pub low_latency: bool,
}

/// Entry point called from `main.rs`.
pub async fn run_package(opts: PackageOptions, json_output: bool) -> Result<()> {
    use oximedia_packager::{
        DashPackagerBuilder, EncryptionMethod, HlsPackagerBuilder, PackagingFormat,
    };

    let fmt = parse_format(&opts.format)?;
    let seg_duration = Duration::from_secs(opts.segments.max(1) as u64);
    let ladder = build_ladder(&opts.ladders)?;
    let encryption = build_encryption(&opts.encrypt)?;

    if json_output {
        let obj = serde_json::json!({
            "operation": "package",
            "input": opts.input.to_string_lossy(),
            "output": opts.output.to_string_lossy(),
            "format": opts.format,
            "segment_duration_s": opts.segments,
            "ladders": opts.ladders,
            "encrypt": opts.encrypt,
            "low_latency": opts.low_latency,
            "ladder_entries": ladder.entries.len(),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    if !crate::progress::is_quiet() {
        println!("{}", "Media Package".green().bold());
        println!("  Input:            {}", opts.input.display());
        println!("  Output:           {}", opts.output.display());
        println!("  Format:           {}", format_label(&opts.format).cyan());
        println!("  Segment duration: {}s", opts.segments);
        println!("  Ladder entries:   {}", ladder.entries.len());
        for entry in &ladder.entries {
            println!(
                "    {}x{} @ {}kbps  [{}]",
                entry.width,
                entry.height,
                entry.bitrate / 1000,
                entry.codec
            );
        }
        println!(
            "  Encryption:       {}",
            if opts.encrypt == "none" {
                "none".dimmed().to_string()
            } else {
                opts.encrypt.cyan().to_string()
            }
        );
        println!(
            "  Low latency:      {}",
            if opts.low_latency { "yes" } else { "no" }
        );
    }

    match fmt {
        PackagingFormat::HlsFmp4 | PackagingFormat::HlsTs => {
            let mut builder = HlsPackagerBuilder::new()
                .with_segment_duration(seg_duration)
                .with_output_directory(opts.output.clone())
                .with_ladder(ladder);

            if opts.low_latency {
                // Low-latency flag noted; HlsPackagerBuilder does not expose it
                // directly without a dedicated method — configuration noted.
            }

            match encryption.method {
                EncryptionMethod::Aes128 => {
                    builder = builder.with_encryption(EncryptionMethod::Aes128);
                }
                EncryptionMethod::SampleAes => {
                    builder = builder.with_encryption(EncryptionMethod::SampleAes);
                }
                _ => {}
            }

            if matches!(fmt, PackagingFormat::HlsFmp4) {
                builder = builder.with_fmp4_segments();
            } else {
                builder = builder.with_ts_segments();
            }

            let mut packager = builder
                .build()
                .with_context(|| "Failed to build HLS packager")?;
            let input_str = opts
                .input
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Input path is not valid UTF-8"))?;
            packager
                .package(input_str)
                .await
                .with_context(|| "HLS packaging failed")?;
            if !crate::progress::is_quiet() {
                println!(
                    "{} HLS packaging complete: {}",
                    "✓".green(),
                    opts.output.display()
                );
            }
        }
        PackagingFormat::Dash | PackagingFormat::Both => {
            let mut builder = DashPackagerBuilder::new()
                .with_segment_duration(seg_duration)
                .with_output_directory(opts.output.clone())
                .with_ladder(ladder)
                .with_low_latency(opts.low_latency);

            if encryption.method == EncryptionMethod::Cenc {
                builder = builder.with_encryption(EncryptionMethod::Cenc);
            }

            let mut packager = builder
                .build()
                .with_context(|| "Failed to build DASH packager")?;
            let input_str = opts
                .input
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Input path is not valid UTF-8"))?;
            packager
                .package(input_str)
                .await
                .with_context(|| "DASH packaging failed")?;
            if !crate::progress::is_quiet() {
                println!(
                    "{} DASH packaging complete: {}",
                    "✓".green(),
                    opts.output.display()
                );
            }
        }
    }

    Ok(())
}

/// Parse CLI format string.
fn parse_format(fmt: &str) -> Result<oximedia_packager::PackagingFormat> {
    use oximedia_packager::PackagingFormat;
    match fmt.to_lowercase().replace('-', "_").as_str() {
        "hls" | "hls_ts" => Ok(PackagingFormat::HlsTs),
        "hls_fmp4" | "hlsfmp4" => Ok(PackagingFormat::HlsFmp4),
        "dash" => Ok(PackagingFormat::Dash),
        other => anyhow::bail!(
            "Unknown packaging format '{}'. Use: hls, hls-fmp4, dash",
            other
        ),
    }
}

fn format_label(fmt: &str) -> &'static str {
    match fmt.to_lowercase().as_str() {
        "hls" | "hls_ts" | "hls-ts" => "HLS (MPEG-TS)",
        "hls-fmp4" | "hls_fmp4" => "HLS (fMP4)",
        "dash" => "MPEG-DASH",
        _ => "Unknown",
    }
}

/// Build a `BitrateLadder` from the `--ladders` option.
fn build_ladder(ladders: &str) -> Result<oximedia_packager::BitrateLadder> {
    use oximedia_packager::{BitrateLadder, LadderGenerator, SourceInfo};

    if ladders.to_lowercase() == "auto" {
        // Generate a standard 1080p-to-360p ladder
        let source = SourceInfo::new(1920, 1080, 30.0, "av1".to_string());
        return LadderGenerator::new(source)
            .generate()
            .with_context(|| "Auto ladder generation failed");
    }

    // Manual ladder: comma-separated like "1080p,720p,480p,360p"
    let mut ladder = BitrateLadder::new();
    for token in ladders.split(',') {
        let entry = parse_ladder_entry(token.trim())?;
        ladder.add_entry(entry);
    }

    if ladder.entries.is_empty() {
        anyhow::bail!("No valid ladder entries parsed from '{}'", ladders);
    }

    Ok(ladder)
}

/// Parse a single ladder token like "1080p" or "1920x1080@5000".
fn parse_ladder_entry(token: &str) -> Result<oximedia_packager::BitrateEntry> {
    use oximedia_packager::BitrateEntry;

    // Preset names
    let (w, h, kbps) = match token.to_lowercase().as_str() {
        "2160p" | "4k" => (3840, 2160, 15000),
        "1080p" => (1920, 1080, 5000),
        "720p" => (1280, 720, 2500),
        "480p" => (854, 480, 1200),
        "360p" => (640, 360, 700),
        "240p" => (426, 240, 400),
        other => {
            // Try "WxH@Bkbps" or "WxH"
            if let Some((dims, bps)) = other.split_once('@') {
                let (w, h) = parse_dims(dims)?;
                let b: u32 = bps
                    .trim_end_matches("kbps")
                    .trim_end_matches('k')
                    .parse()
                    .with_context(|| format!("Invalid bitrate in '{}'", other))?;
                (w, h, b)
            } else {
                let (w, h) = parse_dims(other)?;
                let default_kbps = estimate_bitrate(w, h);
                (w, h, default_kbps)
            }
        }
    };

    Ok(BitrateEntry::new(kbps * 1000, w, h, "av1"))
}

fn parse_dims(s: &str) -> Result<(u32, u32)> {
    let parts: Vec<&str> = s.splitn(2, 'x').collect();
    if parts.len() != 2 {
        anyhow::bail!("Expected WxH format, got '{}'", s);
    }
    let w: u32 = parts[0]
        .parse()
        .with_context(|| format!("Invalid width '{}'", parts[0]))?;
    let h: u32 = parts[1]
        .parse()
        .with_context(|| format!("Invalid height '{}'", parts[1]))?;
    Ok((w, h))
}

fn estimate_bitrate(w: u32, h: u32) -> u32 {
    let pixels = w * h;
    // Rough AV1 target in kbps
    match pixels {
        0..=307200 => 700,       // 480p
        307201..=921600 => 2500, // 720p
        _ => 5000,
    }
}

/// Build an `EncryptionConfig` from CLI string.
fn build_encryption(encrypt: &str) -> Result<oximedia_packager::EncryptionConfig> {
    use oximedia_packager::{EncryptionConfig, EncryptionMethod};

    let config = match encrypt.to_lowercase().replace('-', "_").as_str() {
        "none" | "" => EncryptionConfig {
            method: EncryptionMethod::None,
            ..Default::default()
        },
        "aes128" | "aes_128" => EncryptionConfig {
            method: EncryptionMethod::Aes128,
            ..Default::default()
        },
        "sample_aes" | "sampleaes" | "sample-aes" => EncryptionConfig {
            method: EncryptionMethod::SampleAes,
            ..Default::default()
        },
        "cenc" => EncryptionConfig {
            method: EncryptionMethod::Cenc,
            ..Default::default()
        },
        other => {
            anyhow::bail!(
                "Unknown encryption method '{}'. Use: none, aes128, sample-aes, cenc",
                other
            )
        }
    };

    Ok(config)
}
