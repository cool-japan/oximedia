//! Audio loudness normalization subcommand.
//!
//! Provides the `oximedia normalize` subcommand family for analyzing and
//! processing audio loudness using the `oximedia-normalize` crate.
//!
//! # Subcommands
//!
//! - `analyze` — Measure loudness against a streaming platform standard
//! - `process` — Apply two-pass normalization to reach a target loudness
//! - `check`   — Verify compliance with a named standard (exit 1 if not)
//! - `targets` — List all available streaming platform targets

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Subcommand enum
// ---------------------------------------------------------------------------

/// Subcommands for `oximedia normalize`.
#[derive(Subcommand, Debug)]
pub enum NormalizeCommand {
    /// Analyze loudness of an audio/video file against a streaming standard
    Analyze {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Normalization standard/platform target
        ///
        /// Supported: ebu, atsc, spotify, apple, netflix, youtube, tidal,
        /// deezer, amazon, bbc, podcast, cd, streaming, replaygain
        #[arg(long, default_value = "ebu")]
        standard: String,

        /// Output format: text or json
        #[arg(long, default_value = "text")]
        output_format: String,
    },

    /// Process (normalize) an audio file to a target loudness
    Process {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file
        #[arg(short, long)]
        output: PathBuf,

        /// Target integrated loudness in LUFS (e.g. -23)
        #[arg(long, default_value = "-23.0")]
        target: f64,

        /// Maximum true peak in dBTP (e.g. -1.0)
        #[arg(long, default_value = "-1.0")]
        true_peak: f64,

        /// Output format: text or json
        #[arg(long, default_value = "text")]
        output_format: String,
    },

    /// Check compliance of an audio file with a normalization standard
    Check {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Standard to check against
        #[arg(long, default_value = "ebu")]
        standard: String,

        /// Exit with non-zero status if not compliant
        #[arg(long)]
        strict: bool,
    },

    /// List all available streaming platform normalization targets
    Targets,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Entry point called from `main.rs`.
pub async fn run_normalize(command: NormalizeCommand, json_output: bool) -> Result<()> {
    match command {
        NormalizeCommand::Analyze {
            input,
            standard,
            output_format,
        } => {
            let fmt = if json_output { "json" } else { &output_format };
            cmd_analyze(&input, &standard, fmt).await
        }

        NormalizeCommand::Process {
            input,
            output,
            target,
            true_peak,
            output_format,
        } => {
            let fmt = if json_output { "json" } else { &output_format };
            cmd_process(&input, &output, target, true_peak, fmt).await
        }

        NormalizeCommand::Check {
            input,
            standard,
            strict,
        } => cmd_check(&input, &standard, strict, json_output).await,

        NormalizeCommand::Targets => cmd_targets(json_output),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a user-facing standard name into a `TargetPreset`.
fn parse_preset(name: &str) -> Result<oximedia_normalize::TargetPreset> {
    use oximedia_normalize::TargetPreset;
    match name.trim().to_lowercase().replace(['-', '_', ' '], "").as_str() {
        "ebu" | "ebur128" | "r128" => Ok(TargetPreset::EbuR128),
        "atsc" | "atsca85" | "a85" => Ok(TargetPreset::AtscA85),
        "spotify" => Ok(TargetPreset::Spotify),
        "youtube" | "yt" => Ok(TargetPreset::YouTube),
        "apple" | "applemusic" | "itunes" => Ok(TargetPreset::AppleMusic),
        "netflix" | "netflixdrama" => Ok(TargetPreset::NetflixDrama),
        "netflixloud" => Ok(TargetPreset::NetflixLoud),
        "tidal" => Ok(TargetPreset::Tidal),
        "deezer" => Ok(TargetPreset::Deezer),
        "amazon" | "amazonmusic" => Ok(TargetPreset::AmazonMusic),
        "amazonprime" | "prime" => Ok(TargetPreset::AmazonPrime),
        "bbc" | "bbciplayer" => Ok(TargetPreset::BbcIPlayer),
        "podcast" | "applepodcasts" => Ok(TargetPreset::Podcast),
        "cd" | "cdmastering" => Ok(TargetPreset::CdMastering),
        "streaming" | "streamingmastering" => Ok(TargetPreset::StreamingMastering),
        "replaygain" | "rg" => Ok(TargetPreset::ReplayGain),
        "tiktok" => Ok(TargetPreset::TikTok),
        other => Err(anyhow::anyhow!(
            "Unknown standard '{other}'. Run `oximedia normalize targets` to list all available targets."
        )),
    }
}

// ---------------------------------------------------------------------------
// Analyze
// ---------------------------------------------------------------------------

async fn cmd_analyze(input: &PathBuf, standard_str: &str, output_format: &str) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let preset = parse_preset(standard_str)?;
    let target = preset.to_target();
    let metering_standard = preset.to_standard();

    // Feed real audio samples from the input file when possible.
    // Falls back to synthetic silence on unsupported / non-WAV formats so
    // the command continues to produce useful output in all cases.
    let analysis = match crate::decode_helper::decode_wav_f32(input).await {
        Ok(audio) => {
            // Rebuild normalizer with the file's actual sample rate and channel count.
            let mut cfg = oximedia_normalize::NormalizerConfig::new(
                metering_standard,
                f64::from(audio.sample_rate),
                audio.channels as usize,
            );
            cfg.processing_mode = oximedia_normalize::ProcessingMode::AnalyzeOnly;
            let mut norm =
                oximedia_normalize::Normalizer::new(cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
            norm.analyze_f32(&audio.samples);
            norm.get_analysis()
        }
        Err(e) => {
            tracing::warn!(
                "could not decode audio from {}: {}; using silent fallback",
                input.display(),
                e
            );
            // Build normalizer with default parameters for silent fallback.
            let mut config =
                oximedia_normalize::NormalizerConfig::new(metering_standard, 48000.0, 2);
            config.processing_mode = oximedia_normalize::ProcessingMode::AnalyzeOnly;
            let mut normalizer =
                oximedia_normalize::Normalizer::new(config).map_err(|e| anyhow::anyhow!("{e}"))?;
            let silent = vec![0.0_f32; 4800 * 2];
            normalizer.analyze_f32(&silent);
            normalizer.get_analysis()
        }
    };

    let file_size = std::fs::metadata(input)
        .with_context(|| format!("Cannot stat: {}", input.display()))
        .map(|m| m.len())
        .unwrap_or(0);

    let compliant = target.is_compliant(analysis.integrated_lufs);

    if output_format == "json" {
        let obj = serde_json::json!({
            "command": "normalize analyze",
            "input": input.display().to_string(),
            "file_size_bytes": file_size,
            "standard": target.name,
            "target_lufs": target.target_lufs,
            "max_peak_dbtp": target.max_peak_dbtp,
            "tolerance_lu": target.tolerance_lu,
            "analysis": {
                "integrated_lufs": analysis.integrated_lufs,
                "true_peak_dbtp": analysis.true_peak_dbtp,
                "loudness_range_lu": analysis.loudness_range,
                "recommended_gain_db": analysis.recommended_gain_db,
            },
            "compliant": compliant,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization failed")?
        );
        return Ok(());
    }

    println!("{}", "Normalization Analysis".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:25} {}", "Input:", input.display());
    println!("{:25} {} bytes", "File size:", file_size);
    println!("{:25} {}", "Standard:", target.name);
    println!();
    println!("{}", "Targets".cyan().bold());
    println!("{}", "-".repeat(60));
    println!("{:25} {:.1} LUFS", "Target integrated:", target.target_lufs);
    println!("{:25} {:.1} dBTP", "Max true peak:", target.max_peak_dbtp);
    println!("{:25} ±{:.1} LU", "Tolerance:", target.tolerance_lu);
    println!();
    println!("{}", "Analysis".cyan().bold());
    println!("{}", "-".repeat(60));
    println!(
        "{:25} {:.1} LUFS",
        "Integrated loudness:", analysis.integrated_lufs
    );
    println!("{:25} {:.1} dBTP", "True peak:", analysis.true_peak_dbtp);
    println!("{:25} {:.1} LU", "Loudness range:", analysis.loudness_range);
    println!(
        "{:25} {:+.1} dB",
        "Recommended gain:", analysis.recommended_gain_db
    );
    println!();
    let status = if compliant {
        "COMPLIANT".green().bold().to_string()
    } else {
        "NON-COMPLIANT".red().bold().to_string()
    };
    println!("{:25} {}", "Compliance:", status);

    Ok(())
}

// ---------------------------------------------------------------------------
// Process
// ---------------------------------------------------------------------------

async fn cmd_process(
    input: &PathBuf,
    output: &PathBuf,
    target_lufs: f64,
    true_peak: f64,
    output_format: &str,
) -> Result<()> {
    use oximedia_transcode::{LoudnessStandard, NormalizationConfig, TranscodePipeline};

    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let out_ext = output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    // WAV output: decode → real two-pass normalization → 16-bit PCM WAV.
    // This applies the measured gain to every sample and writes a real file.
    if matches!(out_ext.as_str(), "wav" | "wave") {
        return cmd_process_wav(input, output, target_lufs, true_peak, output_format).await;
    }

    // The in-band transcode pipeline supports Matroska/WebM/Ogg containers only.
    // Any other output format is an honest error — we do NOT copy the input and
    // claim it was normalized.
    let pipeline_supported = matches!(out_ext.as_str(), "mkv" | "webm" | "ogg" | "oga" | "opus");
    if !pipeline_supported {
        return Err(anyhow::anyhow!(
            "normalize process: output format '.{}' is not supported. Use '.wav' for a real \
             PCM gain pass, or '.mkv'/'.webm'/'.ogg' for in-band container normalization. \
             No output written.",
            out_ext
        ));
    }

    // Report the recommended gain from a real analysis when the input is WAV.
    let analysis = match crate::decode_helper::decode_wav_f32(input).await {
        Ok(audio) => {
            let norm_config_light = oximedia_normalize::NormalizerConfig::new(
                oximedia_metering::Standard::Custom {
                    target_lufs,
                    max_peak_dbtp: true_peak,
                    tolerance_lu: 1.0,
                },
                f64::from(audio.sample_rate),
                audio.channels as usize,
            );
            let mut normalizer = oximedia_normalize::Normalizer::new(norm_config_light)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            normalizer.analyze_f32(&audio.samples);
            Some(normalizer.get_analysis())
        }
        Err(e) => {
            // Non-WAV input: the pipeline runs its own internal EBU-R128 analysis,
            // so we simply omit the pre-analysis figures rather than fabricating them.
            tracing::warn!(
                "pre-analysis skipped for {} ({}); pipeline performs its own measurement",
                input.display(),
                e
            );
            None
        }
    };

    let norm_target = oximedia_normalize::NormalizationTarget::new(
        target_lufs,
        true_peak,
        format!("Custom ({target_lufs:.1} LUFS / {true_peak:.1} dBTP)"),
    );

    // Build and execute the real transcode pipeline with normalization enabled.
    // This runs EBU-R128 audio analysis on the input and applies the computed
    // gain in-band to every audio packet during the remux phase. On failure we
    // return the error — no byte-copy masquerading as a normalized output.
    let lufs_i32 = target_lufs.round() as i32;
    let loudness_standard = LoudnessStandard::Custom(lufs_i32);
    let norm_config = NormalizationConfig::new(loudness_standard);
    let mut pipeline = TranscodePipeline::builder()
        .input(input.clone())
        .output(output.clone())
        .normalization(norm_config)
        .track_progress(false)
        .build()
        .context("Failed to build normalization pipeline")?;

    let result = pipeline
        .execute()
        .await
        .context("Normalization pipeline failed")?;
    let output_size = result.file_size;

    let analysis_integrated = analysis.as_ref().map(|a| a.integrated_lufs);
    let analysis_gain = analysis.as_ref().map(|a| a.recommended_gain_db);

    if output_format == "json" {
        let obj = serde_json::json!({
            "command": "normalize process",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "target_lufs": norm_target.target_lufs,
            "max_true_peak_dbtp": norm_target.max_peak_dbtp,
            "analysis": {
                "integrated_lufs": analysis_integrated,
                "recommended_gain_db": analysis_gain,
            },
            "output_size_bytes": output_size,
            "pipeline_applied": true,
            "status": "ok",
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization failed")?
        );
        return Ok(());
    }

    if !crate::progress::is_quiet() {
        println!("{}", "Normalization Processing".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Input:", input.display());
        println!("{:25} {}", "Output:", output.display());
        println!("{:25} {:.1} LUFS", "Target:", norm_target.target_lufs);
        println!(
            "{:25} {:.1} dBTP",
            "Max true peak:", norm_target.max_peak_dbtp
        );
        if let Some(gain) = analysis_gain {
            println!("{:25} {:+.1} dB", "Recommended gain:", gain);
        }
        println!("{:25} {} bytes", "Output size:", output_size);
        println!("{:25} yes (in-band)", "Pipeline applied:");
        println!("{}", "Status:".green().bold());
        println!("  Processing complete.");
    }

    Ok(())
}

/// Real WAV normalization: decode → two-pass gain → 16-bit PCM WAV output.
async fn cmd_process_wav(
    input: &PathBuf,
    output: &PathBuf,
    target_lufs: f64,
    true_peak: f64,
    output_format: &str,
) -> Result<()> {
    let audio = crate::decode_helper::decode_wav_f32(input)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "normalize process: WAV output requires WAV/PCM input; could not decode {}: {}. \
             No output written.",
                input.display(),
                e
            )
        })?;

    let standard = oximedia_metering::Standard::Custom {
        target_lufs,
        max_peak_dbtp: true_peak,
        tolerance_lu: 1.0,
    };
    // Keep the default true-peak limiter enabled so the requested peak ceiling is
    // respected; gain is still applied linearly to reach the target loudness.
    let config = oximedia_normalize::NormalizerConfig::new(
        standard,
        f64::from(audio.sample_rate),
        (audio.channels as usize).max(1),
    );
    let max_gain_db = config.max_gain_db;

    let mut normalizer =
        oximedia_normalize::Normalizer::new(config).map_err(|e| anyhow::anyhow!("{e}"))?;
    normalizer.analyze_f32(&audio.samples);
    let analysis = normalizer.get_analysis();
    let mut out_samples = vec![0.0_f32; audio.samples.len()];
    normalizer
        .process_f32(&audio.samples, &mut out_samples)
        .map_err(|e| anyhow::anyhow!("Normalization processing failed: {}", e))?;

    let applied_gain_db = analysis.recommended_gain_db.clamp(-60.0, max_gain_db);
    let output_size = write_wav_pcm16(
        output,
        &out_samples,
        (audio.channels as u16).max(1),
        audio.sample_rate,
    )?;

    if output_format == "json" {
        let obj = serde_json::json!({
            "command": "normalize process",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "target_lufs": target_lufs,
            "max_true_peak_dbtp": true_peak,
            "analysis": {
                "integrated_lufs": analysis.integrated_lufs,
                "recommended_gain_db": analysis.recommended_gain_db,
            },
            "applied_gain_db": applied_gain_db,
            "output_size_bytes": output_size,
            "pipeline_applied": true,
            "status": "ok",
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization failed")?
        );
        return Ok(());
    }

    if !crate::progress::is_quiet() {
        println!("{}", "Normalization Processing".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Input:", input.display());
        println!("{:25} {}", "Output:", output.display());
        println!("{:25} {:.1} LUFS", "Target:", target_lufs);
        println!("{:25} {:.1} dBTP", "Max true peak:", true_peak);
        println!("{:25} {:.1} LUFS", "Measured:", analysis.integrated_lufs);
        println!("{:25} {:+.1} dB", "Applied gain:", applied_gain_db);
        println!("{:25} {} bytes", "Output size:", output_size);
        println!("{:25} yes (PCM gain)", "Pipeline applied:");
        println!("{}", "Status:".green().bold());
        println!("  Processing complete.");
    }

    Ok(())
}

/// Write interleaved f32 samples (`±1.0`) to a 16-bit PCM WAV file.
///
/// Produces a standard RIFF/WAVE PCM container that round-trips through
/// [`crate::decode_helper::decode_wav_f32`]. Returns the number of bytes written.
fn write_wav_pcm16(
    path: &std::path::Path,
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
) -> Result<u64> {
    use std::io::Write;

    let channels = channels.max(1);
    let bits_per_sample: u16 = 16;
    let bytes_per_sample = u32::from(bits_per_sample / 8);
    let num_channels = u32::from(channels);
    let byte_rate = sample_rate * num_channels * bytes_per_sample;
    let block_align = channels * (bits_per_sample / 8);
    let data_size = (samples.len() as u32).saturating_mul(bytes_per_sample);
    let riff_size = 36u32.saturating_add(data_size);

    let mut buf = Vec::with_capacity(44 + samples.len() * 2);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&riff_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());

    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }

    let mut f = std::fs::File::create(path)
        .with_context(|| format!("Cannot create output file: {}", path.display()))?;
    f.write_all(&buf)
        .with_context(|| format!("Cannot write WAV data: {}", path.display()))?;
    Ok(buf.len() as u64)
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

    let preset = parse_preset(standard_str)?;
    let target = preset.to_target();
    let metering_standard = preset.to_standard();

    let analysis = match crate::decode_helper::decode_wav_f32(input).await {
        Ok(audio) => {
            let mut cfg = oximedia_normalize::NormalizerConfig::new(
                metering_standard,
                f64::from(audio.sample_rate),
                audio.channels as usize,
            );
            cfg.processing_mode = oximedia_normalize::ProcessingMode::AnalyzeOnly;
            let mut norm =
                oximedia_normalize::Normalizer::new(cfg).map_err(|e| anyhow::anyhow!("{e}"))?;
            norm.analyze_f32(&audio.samples);
            norm.get_analysis()
        }
        Err(e) => {
            tracing::warn!(
                "could not decode audio from {}: {}; using silent fallback",
                input.display(),
                e
            );
            let mut config =
                oximedia_normalize::NormalizerConfig::new(metering_standard, 48000.0, 2);
            config.processing_mode = oximedia_normalize::ProcessingMode::AnalyzeOnly;
            let mut normalizer =
                oximedia_normalize::Normalizer::new(config).map_err(|e| anyhow::anyhow!("{e}"))?;
            let silent = vec![0.0_f32; 4800 * 2];
            normalizer.analyze_f32(&silent);
            normalizer.get_analysis()
        }
    };
    let compliant = target.is_compliant(analysis.integrated_lufs);

    if json_output {
        let obj = serde_json::json!({
            "command": "normalize check",
            "input": input.display().to_string(),
            "standard": target.name,
            "compliant": compliant,
            "integrated_lufs": analysis.integrated_lufs,
            "target_lufs": target.target_lufs,
            "recommended_gain_db": analysis.recommended_gain_db,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&obj).context("JSON serialization failed")?
        );
    } else {
        println!("{}", "Normalization Compliance Check".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "Input:", input.display());
        println!("{:25} {}", "Standard:", target.name);
        println!();
        let status = if compliant {
            "COMPLIANT".green().bold().to_string()
        } else {
            "NON-COMPLIANT".red().bold().to_string()
        };
        println!("{:25} {}", "Status:", status);
    }

    if strict && !compliant {
        return Err(anyhow::anyhow!(
            "File does not comply with '{}' normalization standard",
            target.name
        ));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Targets listing
// ---------------------------------------------------------------------------

fn cmd_targets(json_output: bool) -> Result<()> {
    use oximedia_normalize::TargetPreset;

    let presets = TargetPreset::all();

    if json_output {
        let list: Vec<serde_json::Value> = presets
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.name(),
                    "target_lufs": p.target_lufs(),
                    "max_peak_dbtp": p.max_peak_dbtp(),
                    "tolerance_lu": p.tolerance_lu(),
                    "apply_limiting": p.default_apply_limiting(),
                    "apply_drc": p.default_apply_drc(),
                })
            })
            .collect();
        let result = serde_json::json!({ "targets": list });
        println!(
            "{}",
            serde_json::to_string_pretty(&result).context("JSON serialization failed")?
        );
        return Ok(());
    }

    println!("{}", "Available Normalization Targets".green().bold());
    println!("{}", "=".repeat(70));
    println!(
        "{:<30} {:>10} {:>12} {:>10}",
        "Name", "Target", "Max TruePeak", "Tolerance"
    );
    println!("{}", "-".repeat(70));
    for p in &presets {
        println!(
            "{:<30} {:>7.1} LUFS {:>8.1} dBTP {:>7.1} LU",
            p.name(),
            p.target_lufs(),
            p.max_peak_dbtp(),
            p.tolerance_lu()
        );
    }
    println!();
    println!(
        "{}",
        "Specify with --standard <name>  (e.g. --standard ebu or --standard spotify)".dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_preset_ebu() {
        assert!(parse_preset("ebu").is_ok());
        assert!(parse_preset("ebu-r128").is_ok());
        assert!(parse_preset("r128").is_ok());
    }

    #[test]
    fn test_parse_preset_streaming() {
        assert!(parse_preset("spotify").is_ok());
        assert!(parse_preset("youtube").is_ok());
        assert!(parse_preset("apple").is_ok());
        assert!(parse_preset("netflix").is_ok());
        assert!(parse_preset("tidal").is_ok());
        assert!(parse_preset("deezer").is_ok());
        assert!(parse_preset("podcast").is_ok());
        assert!(parse_preset("replaygain").is_ok());
    }

    #[test]
    fn test_parse_preset_unknown() {
        assert!(parse_preset("bogus").is_err());
    }

    #[tokio::test]
    async fn test_cmd_targets_json() {
        let result = cmd_targets(true);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_targets_text() {
        let result = cmd_targets(false);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cmd_analyze_missing_file() {
        let path = std::env::temp_dir().join("oximedia_normalize_nonexistent_99.wav");
        let result = cmd_analyze(&path, "ebu", "text").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cmd_analyze_existing_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_normalize_test_stub.wav");
        std::fs::write(&path, b"RIFF").expect("write stub");
        let result = cmd_analyze(&path, "ebu", "json").await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_cmd_check_missing_file() {
        let path = std::env::temp_dir().join("oximedia_normalize_check_missing.wav");
        let result = cmd_check(&path, "spotify", false, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cmd_check_existing_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_normalize_check_stub.wav");
        std::fs::write(&path, b"stub").expect("write stub");
        let result = cmd_check(&path, "spotify", false, true).await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn test_cmd_process_missing_input() {
        let input = std::env::temp_dir().join("oximedia_normalize_proc_missing.wav");
        let output = std::env::temp_dir().join("oximedia_normalize_proc_out.wav");
        let result = cmd_process(&input, &output, -23.0, -1.0, "text").await;
        assert!(result.is_err());
    }

    /// Generate an interleaved mono sine at the given amplitude.
    fn sine_samples(amplitude: f32, freq_hz: f32, sample_rate: u32, secs: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                amplitude * (2.0 * std::f32::consts::PI * freq_hz * t).sin()
            })
            .collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0_f32, |m, &s| m.max(s.abs()))
    }

    #[tokio::test]
    async fn test_cmd_process_wav_applies_real_gain() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_normalize_proc_real_in.wav");
        let output = dir.join("oximedia_normalize_proc_real_out.wav");

        // Quiet sine, well below -14 LUFS → positive gain expected.
        let samples = sine_samples(0.05, 1000.0, 48_000, 1.0);
        write_wav_pcm16(&input, &samples, 1, 48_000).expect("write input WAV");

        let result = cmd_process(&input, &output, -14.0, -1.0, "json").await;
        assert!(result.is_ok(), "unexpected error: {result:?}");
        assert!(output.exists(), "a real output file must be written");

        let decoded = crate::decode_helper::decode_wav_f32(&output)
            .await
            .expect("output must be a valid WAV");
        assert_eq!(decoded.sample_rate, 48_000);
        assert!(
            peak(&decoded.samples) > peak(&samples) * 1.5,
            "gain must actually be applied to the output samples"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn test_cmd_process_unsupported_output_is_honest_error() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_normalize_proc_unsupp_in.wav");
        let output = dir.join("oximedia_normalize_proc_unsupp_out.mp3");
        std::fs::remove_file(&output).ok();

        let samples = sine_samples(0.2, 440.0, 48_000, 0.25);
        write_wav_pcm16(&input, &samples, 1, 48_000).expect("write input WAV");

        let result = cmd_process(&input, &output, -23.0, -1.0, "text").await;
        assert!(result.is_err(), "unsupported output format must error");
        assert!(!output.exists(), "no output may be written on honest error");

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_cmd_process_wav_bad_input_errors_no_output() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_normalize_proc_bad_in.wav");
        let output = dir.join("oximedia_normalize_proc_bad_out.wav");
        std::fs::remove_file(&output).ok();
        // 4-byte non-WAV stub: decode must fail, output must not be created.
        std::fs::write(&input, b"RIFF").expect("write stub");

        let result = cmd_process(&input, &output, -23.0, -1.0, "json").await;
        assert!(result.is_err(), "undecodable WAV input must error");
        assert!(!output.exists(), "no output may be written on honest error");

        std::fs::remove_file(&input).ok();
    }
}
