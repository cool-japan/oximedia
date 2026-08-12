//! Top-level `oximedia loudness` subcommand.
//!
//! Provides dedicated loudness analysis with support for multiple broadcast
//! and streaming standards: EBU R128, ATSC A/85, YouTube, Spotify, etc.
//!
//! Uses `oximedia-metering` for ITU-R BS.1770-4 compliant measurement.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Subcommand enum
// ---------------------------------------------------------------------------

/// Subcommands for `oximedia loudness`.
#[derive(Subcommand, Debug)]
pub enum LoudnessCommand {
    /// Analyze loudness of a media file and report LUFS/LRA/true peak
    Analyze {
        /// Input media file (positional)
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,

        /// Input media file to analyze
        #[arg(
            short = 'i',
            long = "input",
            value_name = "FILE",
            conflicts_with = "file"
        )]
        input: Option<PathBuf>,

        /// Broadcast/streaming target standard
        ///
        /// Supported values: ebu-r128, atsc-a85, youtube, spotify,
        /// apple-music, netflix, amazon-prime
        #[arg(long, default_value = "ebu-r128")]
        target: String,

        /// Sample rate override in Hz (default: the decoded file's rate)
        #[arg(long)]
        sample_rate: Option<f64>,

        /// Channel count override (default: the decoded file's channels)
        #[arg(long)]
        channels: Option<usize>,

        /// Show per-channel true peak levels
        #[arg(long)]
        per_channel: bool,

        /// Output format: text or json
        #[arg(long, default_value = "text")]
        output_format: String,
    },

    /// Check compliance of a media file against a loudness standard
    Check {
        /// Input media file (positional)
        #[arg(value_name = "FILE")]
        file: Option<PathBuf>,

        /// Input media file
        #[arg(
            short = 'i',
            long = "input",
            value_name = "FILE",
            conflicts_with = "file"
        )]
        input: Option<PathBuf>,

        /// Standard to check against
        #[arg(long, default_value = "ebu-r128")]
        standard: String,

        /// Exit with non-zero status if file is not compliant
        #[arg(long)]
        strict: bool,
    },

    /// List all supported loudness standards and their targets
    Standards,

    /// Show detailed report for a standard (targets, tolerances, references)
    Info {
        /// Standard name (ebu-r128, atsc-a85, youtube, etc.)
        #[arg(value_name = "STANDARD")]
        standard: String,
    },
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Entry point called from `main.rs`.
pub async fn run_loudness(command: LoudnessCommand, json_output: bool, ndjson: bool) -> Result<()> {
    match command {
        LoudnessCommand::Analyze {
            file,
            input,
            target,
            sample_rate,
            channels,
            per_channel,
            output_format,
        } => {
            let resolved = input.or(file).ok_or_else(|| {
                anyhow::anyhow!("input file required: use -i <FILE> or pass as positional argument")
            })?;
            if ndjson {
                colored::control::set_override(false);
                return cmd_analyze_ndjson(&resolved, &target, sample_rate, channels, per_channel)
                    .await;
            }
            let fmt = if json_output { "json" } else { &output_format };
            cmd_analyze(&resolved, &target, sample_rate, channels, per_channel, fmt).await
        }

        LoudnessCommand::Check {
            file,
            input,
            standard,
            strict,
        } => {
            let resolved = input.or(file).ok_or_else(|| {
                anyhow::anyhow!("input file required: use -i <FILE> or pass as positional argument")
            })?;
            cmd_check(&resolved, &standard, strict, json_output).await
        }

        LoudnessCommand::Standards => cmd_standards(json_output),

        LoudnessCommand::Info { standard } => cmd_info(&standard, json_output),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a user-facing standard string into `oximedia_metering::Standard`.
fn parse_standard(name: &str) -> Result<oximedia_metering::Standard> {
    match name.trim().to_lowercase().replace(['-', '_', ' '], "").as_str() {
        "ebur128" | "ebu" | "r128" => Ok(oximedia_metering::Standard::EbuR128),
        "atsca85" | "atsc" | "a85" => Ok(oximedia_metering::Standard::AtscA85),
        "youtube" | "yt" => Ok(oximedia_metering::Standard::YouTube),
        "spotify" => Ok(oximedia_metering::Standard::Spotify),
        "applemusic" | "apple" => Ok(oximedia_metering::Standard::AppleMusic),
        "netflix" => Ok(oximedia_metering::Standard::Netflix),
        "amazonprime" | "amazon" | "prime" => Ok(oximedia_metering::Standard::AmazonPrime),
        other => Err(anyhow::anyhow!(
            "Unknown standard '{other}'. Use `oximedia loudness standards` to list all supported standards."
        )),
    }
}

/// Format a LUFS/LRA value; returns "-∞" for non-finite values.
fn fmt_lufs(v: f64) -> String {
    if v.is_finite() {
        format!("{v:+.1}")
    } else {
        "-∞".to_string()
    }
}

// ---------------------------------------------------------------------------
// Analyze
// ---------------------------------------------------------------------------

async fn cmd_analyze(
    input: &PathBuf,
    target_str: &str,
    sample_rate: Option<f64>,
    channels: Option<usize>,
    per_channel: bool,
    output_format: &str,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let standard = parse_standard(target_str)?;

    // Decode real audio. Non-WAV/compressed inputs are an honest error —
    // the old behaviour of metering a block of synthetic silence reported
    // fabricated metrics/compliance for the file.
    let audio = crate::decode_helper::decode_wav_f32(input)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "loudness analysis currently supports WAV/PCM input; could not decode {}: {}. \
             (Compressed-container audio decoding is planned for 0.2.x.)",
                input.display(),
                e
            )
        })?;

    // Honour explicit overrides; otherwise use the decoded stream's values.
    let sr = sample_rate.unwrap_or_else(|| f64::from(audio.sample_rate));
    let ch = channels.unwrap_or(audio.channels as usize).max(1);

    // Build and validate meter config
    let config = oximedia_metering::MeterConfig::new(standard, sr, ch);
    config
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid meter configuration: {e}"))?;

    let mut meter = oximedia_metering::LoudnessMeter::new(config)
        .map_err(|e| anyhow::anyhow!("Failed to create loudness meter: {e}"))?;

    // Meter the file's actual samples.
    meter.process_f32(&audio.samples);

    let metrics = meter.metrics();
    let compliance = meter.check_compliance();

    // Determine file size for the report
    let file_size = std::fs::metadata(input)
        .with_context(|| format!("Cannot stat: {}", input.display()))
        .map(|m| m.len())
        .unwrap_or(0);

    if output_format == "json" {
        let per_ch_json: serde_json::Value =
            if per_channel && !metrics.channel_peaks_dbtp.is_empty() {
                serde_json::json!(metrics.channel_peaks_dbtp)
            } else {
                serde_json::Value::Null
            };

        let result = serde_json::json!({
            "command": "loudness analyze",
            "input": input.display().to_string(),
            "file_size_bytes": file_size,
            "standard": standard.name(),
            "sample_rate_hz": sr,
            "channels": ch,
            "metrics": {
                "integrated_lufs": metrics.integrated_lufs,
                "momentary_lufs": metrics.momentary_lufs,
                "short_term_lufs": metrics.short_term_lufs,
                "loudness_range_lu": metrics.loudness_range,
                "true_peak_dbtp": metrics.true_peak_dbtp,
                "max_momentary_lufs": metrics.max_momentary,
                "max_short_term_lufs": metrics.max_short_term,
                "channel_peaks_dbtp": per_ch_json,
            },
            "targets": {
                "target_lufs": standard.target_lufs(),
                "max_true_peak_dbtp": standard.max_true_peak_dbtp(),
                "tolerance_lu": standard.tolerance_lu(),
            },
            "compliance": {
                "is_compliant": compliance.is_compliant(),
                "loudness_compliant": compliance.loudness_compliant,
                "peak_compliant": compliance.peak_compliant,
                "lra_acceptable": compliance.lra_acceptable,
                "deviation_lu": compliance.deviation_lu,
                "recommended_gain_db": compliance.recommended_gain_db(),
            },
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
        return Ok(());
    }

    // Human-readable output
    println!("{}", "Loudness Analysis".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:25} {}", "Input:", input.display());
    println!("{:25} {} bytes", "File size:", file_size);
    println!("{:25} {}", "Standard:", standard.name());
    println!("{:25} {} Hz", "Sample rate:", sr);
    println!("{:25} {}", "Channels:", ch);
    println!();

    println!("{}", "Targets".cyan().bold());
    println!("{}", "-".repeat(60));
    println!(
        "{:25} {} LUFS",
        "Target integrated:",
        standard.target_lufs()
    );
    println!(
        "{:25} {} dBTP",
        "Max true peak:",
        standard.max_true_peak_dbtp()
    );
    println!("{:25} ±{} LU", "Tolerance:", standard.tolerance_lu());
    println!();

    println!("{}", "Measurements".cyan().bold());
    println!("{}", "-".repeat(60));
    println!(
        "{:25} {} LUFS",
        "Integrated:",
        fmt_lufs(metrics.integrated_lufs).yellow()
    );
    println!(
        "{:25} {} LUFS",
        "Momentary (400 ms):",
        fmt_lufs(metrics.momentary_lufs).yellow()
    );
    println!(
        "{:25} {} LUFS",
        "Short-term (3 s):",
        fmt_lufs(metrics.short_term_lufs).yellow()
    );
    println!(
        "{:25} {} LU",
        "Loudness range:",
        if metrics.loudness_range.is_finite() {
            format!("{:.1}", metrics.loudness_range)
                .yellow()
                .to_string()
        } else {
            "-∞".yellow().to_string()
        }
    );
    println!(
        "{:25} {} dBTP",
        "True peak:",
        if metrics.true_peak_dbtp.is_finite() {
            format!("{:+.1}", metrics.true_peak_dbtp)
                .yellow()
                .to_string()
        } else {
            "-∞".yellow().to_string()
        }
    );

    if per_channel && !metrics.channel_peaks_dbtp.is_empty() {
        println!();
        println!("{}", "Per-channel true peaks".cyan().bold());
        println!("{}", "-".repeat(60));
        for (i, &peak) in metrics.channel_peaks_dbtp.iter().enumerate() {
            println!(
                "  Ch {}: {} dBTP",
                i + 1,
                if peak.is_finite() {
                    format!("{peak:+.1}").yellow().to_string()
                } else {
                    "-∞".yellow().to_string()
                }
            );
        }
    }

    println!();
    println!("{}", "Compliance".cyan().bold());
    println!("{}", "-".repeat(60));
    let compliant_label = if compliance.is_compliant() {
        "PASS".green().bold().to_string()
    } else {
        "FAIL".red().bold().to_string()
    };
    println!("{:25} {}", "Overall:", compliant_label);
    let loudness_label = if compliance.loudness_compliant {
        "OK".green().to_string()
    } else {
        "OUT OF RANGE".red().to_string()
    };
    let peak_label = if compliance.peak_compliant {
        "OK".green().to_string()
    } else {
        "EXCEEDED".red().to_string()
    };
    println!("{:25} {}", "Loudness:", loudness_label);
    println!("{:25} {}", "True peak:", peak_label);
    if compliance.deviation_lu.is_finite() {
        println!(
            "{:25} {:+.1} LU",
            "Deviation from target:", compliance.deviation_lu
        );
        let gain = compliance.recommended_gain_db();
        if gain.abs() > 0.1 {
            println!("{:25} {:+.1} dB", "Recommended gain adj.:", gain);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// NDJSON loudness analyze
// ---------------------------------------------------------------------------

/// Emit a single NDJSON record with loudness metrics for `loudness analyze`.
async fn cmd_analyze_ndjson(
    input: &PathBuf,
    target_str: &str,
    sample_rate: Option<f64>,
    channels: Option<usize>,
    per_channel: bool,
) -> Result<()> {
    use anyhow::Context;

    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let standard = parse_standard(target_str)?;

    // Decode real audio — same honest-error path as the text/JSON variant.
    let audio = crate::decode_helper::decode_wav_f32(input)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "loudness analysis currently supports WAV/PCM input; could not decode {}: {}. \
             (Compressed-container audio decoding is planned for 0.2.x.)",
                input.display(),
                e
            )
        })?;

    let sr = sample_rate.unwrap_or_else(|| f64::from(audio.sample_rate));
    let ch = channels.unwrap_or(audio.channels as usize).max(1);

    let config = oximedia_metering::MeterConfig::new(standard, sr, ch);
    config
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid meter configuration: {e}"))?;

    let mut meter = oximedia_metering::LoudnessMeter::new(config)
        .map_err(|e| anyhow::anyhow!("Failed to create loudness meter: {e}"))?;

    meter.process_f32(&audio.samples);

    let metrics = meter.metrics();
    let compliance = meter.check_compliance();
    let file_size = std::fs::metadata(input)
        .with_context(|| format!("Cannot stat: {}", input.display()))
        .map(|m| m.len())
        .unwrap_or(0);

    let per_ch_json: serde_json::Value = if per_channel && !metrics.channel_peaks_dbtp.is_empty() {
        serde_json::json!(metrics.channel_peaks_dbtp)
    } else {
        serde_json::Value::Null
    };

    let record = serde_json::json!({
        "path": input.display().to_string(),
        "file_size_bytes": file_size,
        "standard": standard.name(),
        "sample_rate_hz": sr,
        "channels": ch,
        "integrated_lufs": metrics.integrated_lufs,
        "momentary_lufs": metrics.momentary_lufs,
        "short_term_lufs": metrics.short_term_lufs,
        "loudness_range_lu": metrics.loudness_range,
        "true_peak_dbtp": metrics.true_peak_dbtp,
        "channel_peaks_dbtp": per_ch_json,
        "is_compliant": compliance.is_compliant(),
        "target_lufs": standard.target_lufs(),
        "recommended_gain_db": compliance.recommended_gain_db(),
    });

    let mut writer = crate::output::NdjsonWriter::new(std::io::stdout());
    writer
        .emit(&record)
        .context("Failed to write NDJSON loudness record")
}

// ---------------------------------------------------------------------------
// Check
// ---------------------------------------------------------------------------

async fn cmd_check(
    input: &PathBuf,
    standard_str: &str,
    strict: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let standard = parse_standard(standard_str)?;

    // Decode real audio: a compliance verdict for the file must be measured
    // from the file (the old silence-metering path reported a fabricated
    // verdict for any input).
    let audio = crate::decode_helper::decode_wav_f32(input)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "loudness compliance checking currently supports WAV/PCM input; could not decode \
             {}: {}. (Compressed-container audio decoding is planned for 0.2.x.)",
                input.display(),
                e
            )
        })?;

    let config = oximedia_metering::MeterConfig::new(
        standard,
        f64::from(audio.sample_rate),
        (audio.channels as usize).max(1),
    );
    let mut meter = oximedia_metering::LoudnessMeter::new(config)
        .map_err(|e| anyhow::anyhow!("Failed to create loudness meter: {e}"))?;

    meter.process_f32(&audio.samples);
    let compliance = meter.check_compliance();

    if json_output {
        let result = serde_json::json!({
            "command": "loudness check",
            "input": input.display().to_string(),
            "standard": standard.name(),
            "compliant": compliance.is_compliant(),
            "loudness_compliant": compliance.loudness_compliant,
            "peak_compliant": compliance.peak_compliant,
            "lra_acceptable": compliance.lra_acceptable,
            "integrated_lufs": compliance.integrated_lufs,
            "true_peak_dbtp": compliance.true_peak_dbtp,
            "loudness_range_lu": compliance.loudness_range,
            "target_lufs": compliance.target_lufs,
            "max_peak_dbtp": compliance.max_peak_dbtp,
            "deviation_lu": compliance.deviation_lu,
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Loudness Compliance Check".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Input:", input.display());
        println!("{:25} {}", "Standard:", standard.name());
        println!();

        let status = if compliance.is_compliant() {
            "COMPLIANT".green().bold().to_string()
        } else {
            "NON-COMPLIANT".red().bold().to_string()
        };
        println!("{:25} {}", "Status:", status);
    }

    if strict && !compliance.is_compliant() {
        return Err(anyhow::anyhow!(
            "File does not comply with {} standard",
            standard.name()
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Standards listing
// ---------------------------------------------------------------------------

fn cmd_standards(json_output: bool) -> Result<()> {
    use oximedia_metering::Standard;

    let standards = [
        Standard::EbuR128,
        Standard::AtscA85,
        Standard::YouTube,
        Standard::Spotify,
        Standard::AppleMusic,
        Standard::Netflix,
        Standard::AmazonPrime,
    ];

    if json_output {
        let list: Vec<serde_json::Value> = standards
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name(),
                    "target_lufs": s.target_lufs(),
                    "max_true_peak_dbtp": s.max_true_peak_dbtp(),
                    "tolerance_lu": s.tolerance_lu(),
                })
            })
            .collect();
        let result = serde_json::json!({ "standards": list });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
        return Ok(());
    }

    println!("{}", "Supported Loudness Standards".green().bold());
    println!("{}", "=".repeat(60));
    println!(
        "{:<22} {:>10} {:>12} {:>10}",
        "Standard", "Target", "Max TruePeak", "Tolerance"
    );
    println!("{}", "-".repeat(60));
    for s in &standards {
        println!(
            "{:<22} {:>7.1} LUFS {:>8.1} dBTP {:>7.1} LU",
            s.name(),
            s.target_lufs(),
            s.max_true_peak_dbtp(),
            s.tolerance_lu()
        );
    }
    println!();
    println!(
        "{}",
        "Specify with --target <name>  (e.g. --target ebu-r128)".dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Info
// ---------------------------------------------------------------------------

fn cmd_info(standard_str: &str, json_output: bool) -> Result<()> {
    let standard = parse_standard(standard_str)?;

    if json_output {
        let result = serde_json::json!({
            "standard": standard.name(),
            "target_lufs": standard.target_lufs(),
            "max_true_peak_dbtp": standard.max_true_peak_dbtp(),
            "tolerance_lu": standard.tolerance_lu(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
        return Ok(());
    }

    println!("{}", "Standard Details".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:25} {}", "Standard:", standard.name());
    println!(
        "{:25} {:.1} LUFS",
        "Target integrated:",
        standard.target_lufs()
    );
    println!(
        "{:25} {:.1} dBTP",
        "Max true peak:",
        standard.max_true_peak_dbtp()
    );
    println!("{:25} ±{:.1} LU", "Tolerance:", standard.tolerance_lu());

    // Human-readable notes per standard
    let note = match &standard {
        oximedia_metering::Standard::EbuR128 => {
            "European Broadcasting Union standard. ITU-R BS.1770-4 compliant. \
             Widely used in European broadcast, streaming, and podcast platforms."
        }
        oximedia_metering::Standard::AtscA85 => {
            "Advanced Television Systems Committee standard for US broadcast. \
             Equivalent to LKFS measurement; used by major US networks."
        }
        oximedia_metering::Standard::YouTube => {
            "YouTube normalises uploads to -14 LUFS integrated. Content louder \
             than -14 LUFS is turned down; quieter content is not boosted."
        }
        oximedia_metering::Standard::Spotify => {
            "Spotify targets -14 LUFS. Albums are normalised per-track or per-album \
             depending on user preference."
        }
        oximedia_metering::Standard::AppleMusic => {
            "Apple Music / iTunes Sound Check targets -16 LUFS. More headroom \
             than YouTube/Spotify for dynamic material."
        }
        oximedia_metering::Standard::Netflix => {
            "Netflix requires -27 LUFS for original content. Very conservative target \
             to preserve dialogue intelligibility across devices."
        }
        oximedia_metering::Standard::AmazonPrime => {
            "Amazon Prime Video targets -24 LUFS, aligning closely with ATSC A/85."
        }
        oximedia_metering::Standard::TidalHiFi => {
            "Tidal HiFi targets -14 LUFS integrated with a -1.0 dBTP true-peak ceiling. \
             Matches the loudness normalisation level used by most major streaming services."
        }
        oximedia_metering::Standard::AmazonMusicHd => {
            "Amazon Music HD targets -14 LUFS integrated with a -1.0 dBTP true-peak ceiling. \
             Consistent with the broad streaming consensus for loudness normalisation."
        }
        oximedia_metering::Standard::TikTok => {
            "TikTok normalises short-form video audio to -14 LUFS integrated with a \
             -1.0 dBTP true-peak ceiling, matching the streaming consensus."
        }
        oximedia_metering::Standard::Custom { .. } => "Custom target standard.",
    };

    println!();
    println!("{}", "Notes".cyan().bold());
    println!("{}", "-".repeat(60));

    // Word-wrap at 58 chars
    let mut line_buf = String::new();
    for word in note.split_whitespace() {
        if line_buf.len() + word.len() + 1 > 58 {
            println!("  {line_buf}");
            line_buf = word.to_string();
        } else {
            if !line_buf.is_empty() {
                line_buf.push(' ');
            }
            line_buf.push_str(word);
        }
    }
    if !line_buf.is_empty() {
        println!("  {line_buf}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_standard_variants() {
        assert!(parse_standard("ebu-r128").is_ok());
        assert!(parse_standard("atsc-a85").is_ok());
        assert!(parse_standard("youtube").is_ok());
        assert!(parse_standard("spotify").is_ok());
        assert!(parse_standard("apple-music").is_ok());
        assert!(parse_standard("netflix").is_ok());
        assert!(parse_standard("amazon-prime").is_ok());
        assert!(parse_standard("unknown").is_err());
    }

    #[test]
    fn test_fmt_lufs_finite() {
        let s = fmt_lufs(-23.0);
        assert!(s.contains("-23.0"));
    }

    #[test]
    fn test_fmt_lufs_neg_inf() {
        let s = fmt_lufs(f64::NEG_INFINITY);
        assert_eq!(s, "-∞");
    }

    #[tokio::test]
    async fn test_cmd_standards_no_panic() {
        // Should not panic; json_output path
        let result = cmd_standards(true);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_info_ebu() {
        let result = cmd_info("ebu-r128", true);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_info_unknown() {
        let result = cmd_info("bogus", false);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cmd_analyze_missing_file() {
        let path = std::env::temp_dir().join("oximedia_loudness_nonexistent_12345.wav");
        let result = cmd_analyze(&path, "ebu-r128", None, None, false, "text").await;
        assert!(result.is_err());
    }

    /// Write a real 16-bit PCM mono WAV containing a sine wave.
    fn write_sine_wav(path: &std::path::Path, amplitude: f64, seconds: f64) {
        let sample_rate: u32 = 48_000;
        let n = (f64::from(sample_rate) * seconds) as usize;
        let mut pcm: Vec<u8> = Vec::with_capacity(n * 2);
        for i in 0..n {
            let t = i as f64 / f64::from(sample_rate);
            let v = (t * 1000.0 * std::f64::consts::TAU).sin() * amplitude;
            let s = (v * f64::from(i16::MAX)) as i16;
            pcm.extend_from_slice(&s.to_le_bytes());
        }
        let mut wav: Vec<u8> = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&(sample_rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(&pcm);
        std::fs::write(path, &wav).expect("write wav fixture");
    }

    #[tokio::test]
    async fn test_cmd_analyze_real_wav_json() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_loudness_test_real.wav");
        write_sine_wav(&path, 0.5, 1.0);
        let result = cmd_analyze(&path, "ebu-r128", None, None, false, "json").await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        std::fs::remove_file(&path).ok();
    }

    /// Undecodable input must be an honest error, not a fabricated report
    /// metered from synthetic silence (the pre-0.2.0 behaviour).
    #[tokio::test]
    async fn test_cmd_analyze_undecodable_file_errors() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_loudness_test_garbage.wav");
        std::fs::write(&path, b"RIFF").expect("write should succeed");
        let err = cmd_analyze(&path, "ebu-r128", None, None, false, "json")
            .await
            .expect_err("garbage input must fail honestly");
        assert!(
            err.to_string().contains("WAV/PCM"),
            "error must explain supported input, got: {err}"
        );
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_cmd_check_missing_file() {
        let path = std::env::temp_dir().join("oximedia_loudness_missing_check.wav");
        let result = cmd_check(&path, "ebu-r128", false, false).await;
        assert!(result.is_err());
    }

    /// A loud full-scale tone is genuinely non-compliant with EBU R128; the
    /// strict check must fail — proof that real file samples flow through
    /// the meter (silence would never trip the loudness gate).
    #[tokio::test]
    async fn test_cmd_check_real_measurement_strict() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_loudness_check_loud.wav");
        write_sine_wav(&path, 0.9, 2.0);
        let err = cmd_check(&path, "ebu-r128", true, true)
            .await
            .expect_err("a ~-3 LUFS tone must fail strict EBU R128 compliance");
        assert!(
            err.to_string().contains("comply"),
            "error must be the compliance failure, got: {err}"
        );
        // Non-strict mode reports the same verdict without failing.
        let result = cmd_check(&path, "ebu-r128", false, true).await;
        assert!(result.is_ok(), "non-strict must exit Ok: {result:?}");
        std::fs::remove_file(&path).ok();
    }

    /// Undecodable input to `check` is an honest error as well.
    #[tokio::test]
    async fn test_cmd_check_undecodable_file_errors() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_loudness_check_stub.wav");
        std::fs::write(&path, b"stub").expect("write should succeed");
        let result = cmd_check(&path, "youtube", false, true).await;
        assert!(result.is_err(), "garbage input must fail honestly");
        std::fs::remove_file(&path).ok();
    }
}
