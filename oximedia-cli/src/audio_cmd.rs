//! Audio loudness metering, normalization, spectrum analysis, and beat detection.
//!
//! Provides audio-related commands using `oximedia-metering`, `oximedia-normalize`,
//! and `oximedia-audio-analysis` crates.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Audio command subcommands.
#[derive(Subcommand, Debug)]
pub enum AudioCommand {
    /// Measure audio loudness (ITU-R BS.1770-4)
    Loudness {
        /// Input audio/video file
        #[arg(short, long)]
        input: PathBuf,

        /// Loudness standard: ebu-r128, atsc-a85, spotify, youtube, apple-music, netflix
        #[arg(long, default_value = "ebu-r128")]
        standard: String,

        /// Sample rate override (Hz)
        #[arg(long)]
        sample_rate: Option<f64>,

        /// Number of channels override
        #[arg(long)]
        channels: Option<usize>,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        output_format: String,
    },

    /// Normalize audio loudness to a target standard
    Normalize {
        /// Input audio/video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// Target loudness in LUFS (overrides standard default)
        #[arg(long)]
        target: Option<f64>,

        /// Loudness standard: ebu-r128, atsc-a85, spotify, youtube, apple-music
        #[arg(long, default_value = "spotify")]
        standard: String,

        /// Enable true peak limiter
        #[arg(long)]
        limiter: bool,

        /// Enable dynamic range compression
        #[arg(long)]
        drc: bool,
    },

    /// Analyze audio frequency spectrum
    Spectrum {
        /// Input audio/video file
        #[arg(short, long)]
        input: PathBuf,

        /// FFT size (power of 2)
        #[arg(long, default_value = "2048")]
        fft_size: usize,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        output_format: String,
    },

    /// Detect beats and tempo in audio
    Beats {
        /// Input audio/video file
        #[arg(short, long)]
        input: PathBuf,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        output_format: String,
    },
}

/// Handle audio command dispatch.
pub async fn handle_audio_command(command: AudioCommand, json_output: bool) -> Result<()> {
    match command {
        AudioCommand::Loudness {
            input,
            standard,
            sample_rate,
            channels,
            output_format,
        } => {
            measure_loudness(
                &input,
                &standard,
                sample_rate,
                channels,
                if json_output { "json" } else { &output_format },
            )
            .await
        }
        AudioCommand::Normalize {
            input,
            output,
            target,
            standard,
            limiter,
            drc,
        } => normalize_audio(&input, &output, target, &standard, limiter, drc).await,
        AudioCommand::Spectrum {
            input,
            fft_size,
            output_format,
        } => {
            analyze_spectrum(
                &input,
                fft_size,
                if json_output { "json" } else { &output_format },
            )
            .await
        }
        AudioCommand::Beats {
            input,
            output_format,
        } => detect_beats(&input, if json_output { "json" } else { &output_format }).await,
    }
}

/// Parse a standard name string into `oximedia_metering::Standard`.
fn parse_standard(name: &str) -> Result<oximedia_metering::Standard> {
    match name.trim().to_lowercase().as_str() {
        "ebu-r128" | "ebu_r128" | "ebur128" | "r128" => Ok(oximedia_metering::Standard::EbuR128),
        "atsc-a85" | "atsc_a85" | "atsca85" | "a85" => Ok(oximedia_metering::Standard::AtscA85),
        "spotify" => Ok(oximedia_metering::Standard::Spotify),
        "youtube" => Ok(oximedia_metering::Standard::YouTube),
        "apple-music" | "apple_music" | "applemusic" => {
            Ok(oximedia_metering::Standard::AppleMusic)
        }
        "netflix" => Ok(oximedia_metering::Standard::Netflix),
        "amazon" | "amazon-prime" | "prime" => Ok(oximedia_metering::Standard::AmazonPrime),
        other => Err(anyhow::anyhow!(
            "Unknown standard '{}'. Available: ebu-r128, atsc-a85, spotify, youtube, apple-music, netflix, amazon-prime",
            other
        )),
    }
}

/// Measure audio loudness against a broadcast/streaming standard.
///
/// Decodes the input (WAV/PCM) into interleaved f32 samples, feeds them through
/// a real [`oximedia_metering::LoudnessMeter`], and reports the measured
/// integrated / momentary / short-term / LRA / true-peak values plus
/// compliance. Formats that the decode stack cannot yet read return an honest
/// error rather than a fabricated "pending" report.
async fn measure_loudness(
    input: &PathBuf,
    standard_name: &str,
    sample_rate: Option<f64>,
    channels: Option<usize>,
    output_format: &str,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let standard = parse_standard(standard_name)?;

    // Decode real audio. Non-WAV/compressed inputs are an honest error here —
    // we do not emit fabricated null metrics with a success exit code.
    let audio = crate::decode_helper::decode_wav_f32(input)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
            "audio loudness metering currently supports WAV/PCM input; could not decode {}: {}. \
             (Compressed-container audio decoding is planned for 0.2.x.)",
            input.display(),
            e
        )
        })?;

    // Honour explicit overrides; otherwise use the decoded stream's real values.
    let sr = sample_rate.unwrap_or_else(|| f64::from(audio.sample_rate));
    let ch = channels.unwrap_or(audio.channels as usize).max(1);

    let config = oximedia_metering::MeterConfig::new(standard, sr, ch);
    let mut meter = oximedia_metering::LoudnessMeter::new(config)
        .map_err(|e| anyhow::anyhow!("Failed to create loudness meter: {}", e))?;
    meter.process_f32(&audio.samples);
    let metrics = meter.metrics();
    let compliance = meter.check_compliance();
    let duration_s = meter.duration_seconds();

    match output_format {
        "json" => {
            let result = serde_json::json!({
                "input": input.display().to_string(),
                "standard": standard.name(),
                "target_lufs": standard.target_lufs(),
                "max_true_peak_dbtp": standard.max_true_peak_dbtp(),
                "tolerance_lu": standard.tolerance_lu(),
                "sample_rate": sr,
                "channels": ch,
                "duration_seconds": duration_s,
                "status": "ok",
                "metrics": {
                    "integrated_lufs": finite_or_null(metrics.integrated_lufs),
                    "momentary_lufs": finite_or_null(metrics.max_momentary),
                    "short_term_lufs": finite_or_null(metrics.max_short_term),
                    "loudness_range_lu": metrics.loudness_range,
                    "true_peak_dbtp": metrics.true_peak_dbtp,
                },
                "compliance": {
                    "compliant": compliance.is_compliant(),
                    "loudness_compliant": compliance.loudness_compliant,
                    "peak_compliant": compliance.peak_compliant,
                    "deviation_lu": compliance.deviation_lu,
                    "recommended_gain_db": compliance.recommended_gain_db(),
                },
            });
            let json_str =
                serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
            println!("{}", json_str);
        }
        _ => {
            println!("{}", "Audio Loudness Metering".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:20} {}", "Input:", input.display());
            println!("{:20} {}", "Standard:", standard.name());
            println!("{:20} {:.1} LUFS", "Target:", standard.target_lufs());
            println!(
                "{:20} {:.1} dBTP",
                "Max True Peak:",
                standard.max_true_peak_dbtp()
            );
            println!("{:20} {:.1} LU", "Tolerance:", standard.tolerance_lu());
            println!("{:20} {} Hz", "Sample rate:", sr);
            println!("{:20} {}", "Channels:", ch);
            println!("{:20} {:.2} s", "Duration:", duration_s);
            println!();

            println!("{}", "Measurements".cyan().bold());
            println!("{}", "-".repeat(60));
            println!("  Integrated LUFS:  {}", fmt_lufs(metrics.integrated_lufs));
            println!("  Momentary LUFS:   {}", fmt_lufs(metrics.max_momentary));
            println!("  Short-term LUFS:  {}", fmt_lufs(metrics.max_short_term));
            println!("  Loudness Range:   {:.1} LU", metrics.loudness_range);
            println!("  True Peak:        {:.1} dBTP", metrics.true_peak_dbtp);
            println!();

            let status = if compliance.is_compliant() {
                "COMPLIANT".green().bold().to_string()
            } else {
                "NON-COMPLIANT".red().bold().to_string()
            };
            println!("{:20} {}", "Compliance:", status);
            println!("{:20} {:+.1} LU", "Deviation:", compliance.deviation_lu);
            println!(
                "{:20} {:+.1} dB",
                "Recommended gain:",
                compliance.recommended_gain_db()
            );
        }
    }

    Ok(())
}

/// Format a LUFS value, showing `-inf` for digital silence.
fn fmt_lufs(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.1} LUFS")
    } else {
        "-inf LUFS (silence)".to_string()
    }
}

/// Map a non-finite metric (e.g. `-inf` for silence) to JSON `null`.
fn finite_or_null(value: f64) -> serde_json::Value {
    if value.is_finite() {
        serde_json::json!(value)
    } else {
        serde_json::Value::Null
    }
}

/// Normalize audio loudness and write a real output file.
///
/// Decodes the input (WAV/PCM), runs a real two-pass
/// [`oximedia_normalize::Normalizer`] (analyse then apply gain, with optional
/// limiter/DRC), and writes the processed samples as a 16-bit PCM WAV file that
/// round-trips through the decode stack. Inputs the decoder cannot read, and
/// non-WAV output paths, return an honest error — nothing is written on failure.
async fn normalize_audio(
    input: &PathBuf,
    output: &PathBuf,
    target: Option<f64>,
    standard_name: &str,
    limiter: bool,
    drc: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    // Only a WAV/PCM output container is produced by this path; refuse other
    // extensions rather than writing WAV bytes under a misleading name.
    let out_ext = output
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if !matches!(out_ext.as_str(), "wav" | "wave") {
        return Err(anyhow::anyhow!(
            "audio normalize writes a 16-bit PCM WAV file; output '.{}' is not supported \
             (use a .wav output path). Compressed-container muxing is planned for 0.2.x. \
             No output written.",
            out_ext
        ));
    }

    let standard = if let Some(target_lufs) = target {
        oximedia_metering::Standard::Custom {
            target_lufs,
            max_peak_dbtp: -1.0,
            tolerance_lu: 1.0,
        }
    } else {
        parse_standard(standard_name)?
    };

    // Decode real samples; non-WAV input is an honest error.
    let audio = crate::decode_helper::decode_wav_f32(input)
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "audio normalize currently supports WAV/PCM input; could not decode {}: {}. \
             (Compressed-container audio decoding is planned for 0.2.x.) No output written.",
                input.display(),
                e
            )
        })?;

    let mut config = oximedia_normalize::NormalizerConfig::new(
        standard,
        f64::from(audio.sample_rate),
        (audio.channels as usize).max(1),
    );
    config.enable_limiter = limiter;
    config.enable_drc = drc;
    let max_gain_db = config.max_gain_db;

    let mut normalizer = oximedia_normalize::Normalizer::new(config)
        .map_err(|e| anyhow::anyhow!("Failed to create normalizer: {}", e))?;

    // Two-pass: measure, then apply the recommended gain to every sample.
    normalizer.analyze_f32(&audio.samples);
    let analysis = normalizer.get_analysis();
    let mut out_samples = vec![0.0_f32; audio.samples.len()];
    normalizer
        .process_f32(&audio.samples, &mut out_samples)
        .map_err(|e| anyhow::anyhow!("Normalization processing failed: {}", e))?;

    // The processor clamps gain to [-60, max_gain_db]; report the value actually applied.
    let applied_gain_db = analysis.recommended_gain_db.clamp(-60.0, max_gain_db);

    let file_size = write_wav_pcm16(
        output,
        &out_samples,
        (audio.channels as u16).max(1),
        audio.sample_rate,
    )?;

    if !crate::progress::is_quiet() {
        println!("{}", "Audio Normalization".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {:.1} LUFS", "Target:", standard.target_lufs());
        println!("{:20} {:.1} LUFS", "Measured:", analysis.integrated_lufs);
        println!("{:20} {:+.1} dB", "Applied gain:", applied_gain_db);
        println!(
            "{:20} {}",
            "Limiter:",
            if limiter { "enabled" } else { "disabled" }
        );
        println!("{:20} {}", "DRC:", if drc { "enabled" } else { "disabled" });
        println!("{:20} {} bytes", "Output size:", file_size);
        println!();
        println!("{}", "Status: normalization applied and written.".green());
    }

    Ok(())
}

/// Write interleaved f32 samples (`±1.0`) to a 16-bit PCM WAV file.
///
/// Produces a standard RIFF/WAVE PCM container that round-trips through
/// [`crate::decode_helper::decode_wav_f32`]. Returns the number of bytes
/// written. Samples are clamped to `±1.0` before quantisation.
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

/// Analyze audio frequency spectrum.
async fn analyze_spectrum(input: &PathBuf, fft_size: usize, output_format: &str) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    // Validate FFT size is a power of 2
    if fft_size == 0 || (fft_size & (fft_size - 1)) != 0 {
        return Err(anyhow::anyhow!(
            "FFT size must be a power of 2, got {}",
            fft_size
        ));
    }

    let config = oximedia_audio_analysis::AnalysisConfig {
        fft_size,
        ..oximedia_audio_analysis::AnalysisConfig::default()
    };
    let _analyzer = oximedia_audio_analysis::AudioAnalyzer::new(config);

    match output_format {
        "json" => {
            let result = serde_json::json!({
                "input": input.display().to_string(),
                "fft_size": fft_size,
                "frequency_resolution": 48000.0 / fft_size as f64,
                "status": "pending_audio_decoding",
                "spectral_features": {
                    "centroid": null,
                    "flatness": null,
                    "rolloff": null,
                    "bandwidth": null,
                },
                "message": "Audio analyzer initialized; awaiting audio decoding pipeline integration",
            });
            let json_str =
                serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
            println!("{}", json_str);
        }
        _ => {
            println!("{}", "Spectrum Analysis".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:20} {}", "Input:", input.display());
            println!("{:20} {}", "FFT size:", fft_size);
            println!(
                "{:20} {:.2} Hz",
                "Freq resolution:",
                48000.0 / fft_size as f64
            );
            println!();

            println!("{}", "Spectral Features".cyan().bold());
            println!("{}", "-".repeat(60));
            println!("  Centroid:   (pending audio decoding)");
            println!("  Flatness:   (pending audio decoding)");
            println!("  Rolloff:    (pending audio decoding)");
            println!("  Bandwidth:  (pending audio decoding)");
            println!();

            println!(
                "{}",
                "Note: Audio decoding pipeline not yet integrated.".yellow()
            );
        }
    }

    Ok(())
}

/// Detect beats and tempo in audio.
async fn detect_beats(input: &PathBuf, output_format: &str) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let config = oximedia_audio_analysis::AnalysisConfig::default();
    let _analyzer = oximedia_audio_analysis::AudioAnalyzer::new(config);

    match output_format {
        "json" => {
            let result = serde_json::json!({
                "input": input.display().to_string(),
                "status": "pending_audio_decoding",
                "tempo": {
                    "bpm": null,
                    "confidence": null,
                },
                "beats": [],
                "message": "Beat detector initialized; awaiting audio decoding pipeline integration",
            });
            let json_str =
                serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
            println!("{}", json_str);
        }
        _ => {
            println!("{}", "Beat Detection".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:20} {}", "Input:", input.display());
            println!();

            println!("{}", "Tempo Analysis".cyan().bold());
            println!("{}", "-".repeat(60));
            println!("  BPM:        (pending audio decoding)");
            println!("  Confidence: (pending audio decoding)");
            println!("  Beats:      (pending audio decoding)");
            println!();

            println!(
                "{}",
                "Note: Audio decoding pipeline not yet integrated.".yellow()
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn normalize_audio_writes_louder_real_wav() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_audiocmd_norm_in.wav");
        let output = dir.join("oximedia_audiocmd_norm_out.wav");

        // Quiet 1 kHz sine — well below the -14 LUFS target, so gain is positive.
        let samples = sine_samples(0.05, 1000.0, 48_000, 1.0);
        let in_size = write_wav_pcm16(&input, &samples, 1, 48_000).expect("write input WAV");
        assert!(in_size > 44, "input WAV must have a data payload");

        let result = normalize_audio(&input, &output, Some(-14.0), "spotify", false, false).await;
        assert!(result.is_ok(), "normalize failed: {result:?}");
        assert!(output.exists(), "output file must be created");

        // Decode the produced file and confirm the gain was actually applied.
        let decoded = crate::decode_helper::decode_wav_f32(&output)
            .await
            .expect("output must be a valid WAV");
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.sample_rate, 48_000);
        assert!(!decoded.samples.is_empty(), "output must contain samples");

        let in_peak = peak(&samples);
        let out_peak = peak(&decoded.samples);
        assert!(
            out_peak > in_peak * 1.5,
            "expected normalization to raise level: in_peak={in_peak}, out_peak={out_peak}"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn normalize_audio_rejects_non_wav_output_and_writes_nothing() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_audiocmd_reject_in.wav");
        let output = dir.join("oximedia_audiocmd_reject_out.mp3");
        std::fs::remove_file(&output).ok();

        let samples = sine_samples(0.2, 440.0, 48_000, 0.25);
        write_wav_pcm16(&input, &samples, 1, 48_000).expect("write input WAV");

        let result = normalize_audio(&input, &output, None, "spotify", false, false).await;
        assert!(result.is_err(), "non-WAV output must be an honest error");
        assert!(
            !output.exists(),
            "no output file may be created on honest error"
        );

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn normalize_audio_missing_input_errors() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_audiocmd_missing_in.wav");
        std::fs::remove_file(&input).ok();
        let output = dir.join("oximedia_audiocmd_missing_out.wav");

        let result = normalize_audio(&input, &output, None, "spotify", false, false).await;
        assert!(result.is_err(), "missing input must error");
    }

    #[tokio::test]
    async fn measure_loudness_real_wav_ok() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_audiocmd_meter_in.wav");
        let samples = sine_samples(0.5, 1000.0, 48_000, 1.0);
        write_wav_pcm16(&input, &samples, 1, 48_000).expect("write input WAV");

        let result = measure_loudness(&input, "ebu-r128", None, None, "json").await;
        assert!(result.is_ok(), "loudness measurement failed: {result:?}");

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn measure_loudness_non_wav_errors() {
        let dir = std::env::temp_dir();
        let input = dir.join("oximedia_audiocmd_meter_notwav.mkv");
        std::fs::write(&input, b"\x1a\x45\xdf\xa3fake_mkv").expect("write fake mkv");

        let result = measure_loudness(&input, "ebu-r128", None, None, "text").await;
        assert!(
            result.is_err(),
            "non-WAV loudness input must be honest error"
        );

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn write_wav_pcm16_round_trips() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_audiocmd_roundtrip.wav");
        let samples = sine_samples(0.3, 500.0, 44_100, 0.5);
        let size = write_wav_pcm16(&path, &samples, 1, 44_100).expect("write WAV");
        assert_eq!(size as usize, 44 + samples.len() * 2);

        let decoded = crate::decode_helper::decode_wav_f32(&path)
            .await
            .expect("round-trip decode");
        assert_eq!(decoded.sample_rate, 44_100);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples.len(), samples.len());

        std::fs::remove_file(&path).ok();
    }
}
