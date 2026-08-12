//! Watermark command — digital audio watermarking for OxiMedia CLI.
//!
//! Provides `oximedia watermark` with embed, detect, and verify subcommands.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Subcommands for `oximedia watermark`.
#[derive(Subcommand)]
pub enum WatermarkSubcommand {
    /// Embed a digital watermark into a media file
    Embed {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output watermarked file
        #[arg(short, long)]
        output: PathBuf,

        /// Watermark message to embed
        #[arg(long)]
        message: String,

        /// Embedding strength (0.0 - 1.0)
        #[arg(long, default_value = "0.3")]
        strength: f32,
    },

    /// Detect and extract a watermark from a media file
    Detect {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,
    },

    /// Verify that a watermark matches an expected message
    Verify {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Expected watermark message
        #[arg(long)]
        expected: String,
    },
}

/// Handle `oximedia watermark` subcommands.
pub async fn handle_watermark_command(
    command: WatermarkSubcommand,
    json_output: bool,
) -> Result<()> {
    match command {
        WatermarkSubcommand::Embed {
            input,
            output,
            message,
            strength,
        } => cmd_embed(&input, &output, &message, strength, json_output),

        WatermarkSubcommand::Detect { input } => cmd_detect(&input, json_output),

        WatermarkSubcommand::Verify { input, expected } => {
            cmd_verify(&input, &expected, json_output)
        }
    }
}

// ── Sample rate used for the watermark embedder ───────────────────────────────
const SAMPLE_RATE: u32 = 44100;

// ── Read file bytes and convert to f32 samples ───────────────────────────────

fn read_as_samples(path: &PathBuf) -> Result<Vec<f32>> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read input file: {}", path.display()))?;

    if bytes.is_empty() {
        anyhow::bail!("Input file is empty: {}", path.display());
    }

    // Convert raw bytes to normalised float samples in [-1.0, 1.0]
    let samples: Vec<f32> = bytes.iter().map(|&b| (b as f32 / 128.0) - 1.0).collect();
    Ok(samples)
}

// ── Write f32 samples back as bytes ──────────────────────────────────────────

fn write_samples_as_bytes(path: &PathBuf, samples: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = samples
        .iter()
        .map(|&s| ((s + 1.0) * 128.0).clamp(0.0, 255.0) as u8)
        .collect();
    std::fs::write(path, &bytes)
        .with_context(|| format!("Failed to write output file: {}", path.display()))
}

// ── Build default embedder config ─────────────────────────────────────────────

fn build_config(strength: f32) -> oximedia_watermark::WatermarkConfig {
    oximedia_watermark::WatermarkConfig::default()
        .with_algorithm(oximedia_watermark::Algorithm::SpreadSpectrum)
        .with_strength(strength)
}

// ── Embed ─────────────────────────────────────────────────────────────────────

fn cmd_embed(
    input: &PathBuf,
    output: &PathBuf,
    message: &str,
    strength: f32,
    json_output: bool,
) -> Result<()> {
    use oximedia_watermark::WatermarkEmbedder;

    let samples = read_as_samples(input)?;
    let config = build_config(strength);
    let embedder = WatermarkEmbedder::new(config, SAMPLE_RATE);

    let watermarked = embedder
        .embed(&samples, message.as_bytes())
        .map_err(|e| anyhow::anyhow!("Watermark embedding failed: {}", e))?;

    write_samples_as_bytes(output, &watermarked)?;

    if json_output {
        let json = serde_json::json!({
            "status": "embedded",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "message_length_bytes": message.len(),
            "strength": strength,
            "samples_processed": watermarked.len(),
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !crate::progress::is_quiet() {
        println!(
            "{} Watermark embedded: {} -> {}",
            "OK".green().bold(),
            input.display(),
            output.display()
        );
        println!(
            "   Message: {}  |  Strength: {:.2}  |  Samples: {}",
            message.cyan(),
            strength,
            watermarked.len()
        );
    }

    Ok(())
}

// ── Detect ────────────────────────────────────────────────────────────────────

fn cmd_detect(input: &PathBuf, json_output: bool) -> Result<()> {
    use oximedia_watermark::WatermarkDetector;

    let samples = read_as_samples(input)?;
    let config = build_config(0.3);
    let detector = WatermarkDetector::new(config);

    // Estimate expected bits: try to extract a 32-byte payload (256 bits)
    let expected_bits = 256;
    let extracted = detector
        .detect(&samples, expected_bits)
        .map_err(|e| anyhow::anyhow!("Watermark detection failed: {}", e))?;

    let message = String::from_utf8_lossy(&extracted).into_owned();
    // Trim null bytes that may appear from padding
    let message_trimmed = message.trim_matches('\0').trim().to_string();

    if json_output {
        let json = serde_json::json!({
            "status": "detected",
            "input": input.display().to_string(),
            "message": message_trimmed,
            "raw_bytes": extracted.len(),
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        println!(
            "{} Watermark detected in: {}",
            "OK".green().bold(),
            input.display()
        );
        println!("   Extracted message: {}", message_trimmed.cyan());
        println!("   Raw payload bytes: {}", extracted.len());
    }

    Ok(())
}

// ── Verify ────────────────────────────────────────────────────────────────────

fn cmd_verify(input: &PathBuf, expected: &str, json_output: bool) -> Result<()> {
    use oximedia_watermark::WatermarkDetector;

    let samples = read_as_samples(input)?;
    let config = build_config(0.3);
    let detector = WatermarkDetector::new(config);

    let expected_bits = expected.len() * 8;
    let expected_bits = if expected_bits == 0 {
        256
    } else {
        expected_bits
    };

    let extracted = detector
        .detect(&samples, expected_bits)
        .map_err(|e| anyhow::anyhow!("Watermark detection failed during verify: {}", e))?;

    let message = String::from_utf8_lossy(&extracted).into_owned();
    let message_trimmed = message.trim_matches('\0').trim().to_string();

    // Normalised comparison: trim and compare
    let matched = message_trimmed == expected.trim();

    if json_output {
        let json = serde_json::json!({
            "status": if matched { "verified" } else { "mismatch" },
            "input": input.display().to_string(),
            "expected": expected,
            "detected": message_trimmed,
            "match": matched,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if matched {
        println!(
            "{} Watermark verified: {}",
            "PASS".green().bold(),
            input.display()
        );
        println!("   Message matches: {}", expected.cyan());
    } else {
        println!(
            "{} Watermark mismatch: {}",
            "FAIL".red().bold(),
            input.display()
        );
        println!("   Expected: {}", expected.yellow());
        println!("   Detected: {}", message_trimmed.red());
    }

    if !matched {
        anyhow::bail!("Watermark verification failed: message does not match");
    }

    Ok(())
}
