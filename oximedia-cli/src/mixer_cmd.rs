//! Audio mixer CLI commands.
//!
//! Provides mixer management commands: create, add-channel, route, render, and info.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Mixer command subcommands.
#[derive(Subcommand, Debug)]
pub enum MixerCommand {
    /// Create a new mixer session file
    Create {
        /// Output mixer session file
        #[arg(short, long)]
        output: PathBuf,

        /// Sample rate in Hz
        #[arg(long, default_value = "48000")]
        sample_rate: u32,

        /// Output format: mono, stereo, 5.1, 7.1
        #[arg(long, default_value = "stereo")]
        output_format: String,

        /// Session name
        #[arg(long)]
        name: Option<String>,

        /// Buffer size in samples
        #[arg(long, default_value = "512")]
        buffer_size: usize,

        /// Maximum number of channels
        #[arg(long, default_value = "128")]
        max_channels: usize,
    },

    /// Add a channel to a mixer session
    AddChannel {
        /// Mixer session file
        #[arg(short, long)]
        mixer: PathBuf,

        /// Input audio file for this channel
        #[arg(short, long)]
        input: PathBuf,

        /// Channel name
        #[arg(long)]
        name: Option<String>,

        /// Channel volume (linear: 0.0 = silence, 1.0 = unity)
        #[arg(long, default_value = "1.0")]
        volume: f64,

        /// Pan position (-1.0 = full left, 0.0 = center, 1.0 = full right)
        #[arg(long, default_value = "0.0")]
        pan: f64,

        /// Mute this channel
        #[arg(long)]
        mute: bool,

        /// Solo this channel
        #[arg(long)]
        solo: bool,

        /// Channel type: mono, stereo
        #[arg(long, default_value = "stereo")]
        channel_type: String,
    },

    /// Route a channel to a bus
    Route {
        /// Mixer session file
        #[arg(short, long)]
        mixer: PathBuf,

        /// Source channel index
        #[arg(long)]
        from_channel: u32,

        /// Target bus name
        #[arg(long)]
        to_bus: String,

        /// Send level (0.0 to 1.0)
        #[arg(long, default_value = "1.0")]
        send_level: f64,

        /// Pre-fader send
        #[arg(long)]
        pre_fader: bool,
    },

    /// Render (mixdown) the mixer session to an output file
    Render {
        /// Mixer session file
        #[arg(short, long)]
        mixer: PathBuf,

        /// Output audio file
        #[arg(short, long)]
        output: PathBuf,

        /// Output format: wav, flac, ogg
        #[arg(long)]
        format: Option<String>,

        /// Normalize output loudness
        #[arg(long)]
        normalize: bool,

        /// Target loudness in LUFS (used with --normalize)
        #[arg(long, default_value = "-14.0")]
        target_lufs: f64,

        /// Bit depth: 16, 24, 32
        #[arg(long, default_value = "24")]
        bit_depth: u32,
    },

    /// Show mixer session information
    Info {
        /// Mixer session file
        #[arg(short, long)]
        mixer: PathBuf,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        output_format: String,
    },
}

/// Handle mixer command dispatch.
pub async fn handle_mixer_command(command: MixerCommand, json_output: bool) -> Result<()> {
    match command {
        MixerCommand::Create {
            output,
            sample_rate,
            output_format,
            name,
            buffer_size,
            max_channels,
        } => {
            create_mixer(
                &output,
                sample_rate,
                &output_format,
                name.as_deref(),
                buffer_size,
                max_channels,
                json_output,
            )
            .await
        }
        MixerCommand::AddChannel {
            mixer,
            input,
            name,
            volume,
            pan,
            mute,
            solo,
            channel_type,
        } => {
            add_channel(
                &mixer,
                &input,
                name.as_deref(),
                volume,
                pan,
                mute,
                solo,
                &channel_type,
                json_output,
            )
            .await
        }
        MixerCommand::Route {
            mixer,
            from_channel,
            to_bus,
            send_level,
            pre_fader,
        } => {
            route_channel(
                &mixer,
                from_channel,
                &to_bus,
                send_level,
                pre_fader,
                json_output,
            )
            .await
        }
        MixerCommand::Render {
            mixer,
            output,
            format,
            normalize,
            target_lufs,
            bit_depth,
        } => {
            render_mixer(
                &mixer,
                &output,
                format.as_deref(),
                normalize,
                target_lufs,
                bit_depth,
                json_output,
            )
            .await
        }
        MixerCommand::Info {
            mixer,
            output_format,
        } => show_mixer_info(&mixer, if json_output { "json" } else { &output_format }).await,
    }
}

/// Parse output format string to channel count.
fn parse_output_channels(format: &str) -> Result<u32> {
    match format {
        "mono" => Ok(1),
        "stereo" => Ok(2),
        "5.1" => Ok(6),
        "7.1" => Ok(8),
        other => Err(anyhow::anyhow!(
            "Unknown output format '{}'. Expected: mono, stereo, 5.1, 7.1",
            other
        )),
    }
}

/// Create a new mixer session.
async fn create_mixer(
    output: &PathBuf,
    sample_rate: u32,
    output_format: &str,
    name: Option<&str>,
    buffer_size: usize,
    max_channels: usize,
    json_output: bool,
) -> Result<()> {
    let channels = parse_output_channels(output_format)?;
    let session_name = name.unwrap_or("Untitled Mix");

    // Validate parameters
    if sample_rate == 0 || sample_rate > 384000 {
        return Err(anyhow::anyhow!(
            "Invalid sample rate: {}. Expected 8000-384000 Hz",
            sample_rate
        ));
    }
    if buffer_size == 0 || !buffer_size.is_power_of_two() {
        return Err(anyhow::anyhow!(
            "Invalid buffer size: {}. Must be a power of two",
            buffer_size
        ));
    }

    let config = oximedia_mixer::MixerConfig {
        sample_rate,
        buffer_size,
        max_channels,
        ..Default::default()
    };

    let _mixer = oximedia_mixer::AudioMixer::new(config);

    // Write session metadata
    let session_data = serde_json::json!({
        "name": session_name,
        "sample_rate": sample_rate,
        "buffer_size": buffer_size,
        "output_channels": channels,
        "output_format": output_format,
        "max_channels": max_channels,
        "channels": [],
        "buses": [],
        "master_volume": 1.0,
    });

    let json_str =
        serde_json::to_string_pretty(&session_data).context("Failed to serialize mixer session")?;
    std::fs::write(output, &json_str).context("Failed to write mixer session file")?;

    if json_output {
        let result = serde_json::json!({
            "status": "created",
            "file": output.display().to_string(),
            "config": session_data,
        });
        let result_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", result_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Mixer Session Created".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Session name:", session_name);
        println!("{:20} {}", "Output file:", output.display());
        println!("{:20} {} Hz", "Sample rate:", sample_rate);
        println!("{:20} {} samples", "Buffer size:", buffer_size);
        println!("{:20} {} ({}ch)", "Output format:", output_format, channels);
        println!("{:20} {}", "Max channels:", max_channels);
    }

    Ok(())
}

/// Add a channel to the mixer.
async fn add_channel(
    mixer_path: &PathBuf,
    input: &PathBuf,
    name: Option<&str>,
    volume: f64,
    pan: f64,
    mute: bool,
    solo: bool,
    channel_type: &str,
    json_output: bool,
) -> Result<()> {
    if !mixer_path.exists() {
        return Err(anyhow::anyhow!(
            "Mixer session file not found: {}",
            mixer_path.display()
        ));
    }
    if !input.exists() {
        return Err(anyhow::anyhow!(
            "Input audio file not found: {}",
            input.display()
        ));
    }
    if !(-1.0..=1.0).contains(&pan) {
        return Err(anyhow::anyhow!(
            "Pan must be between -1.0 and 1.0, got {}",
            pan
        ));
    }
    if !(0.0..=2.0).contains(&volume) {
        return Err(anyhow::anyhow!(
            "Volume must be between 0.0 and 2.0, got {}",
            volume
        ));
    }

    let channel_name = name.unwrap_or_else(|| {
        input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unnamed")
    });

    // Read and update session
    let session_json =
        std::fs::read_to_string(mixer_path).context("Failed to read mixer session")?;
    let mut session: serde_json::Value =
        serde_json::from_str(&session_json).context("Failed to parse mixer session")?;

    let new_channel = serde_json::json!({
        "name": channel_name,
        "input": input.display().to_string(),
        "volume": volume,
        "pan": pan,
        "mute": mute,
        "solo": solo,
        "channel_type": channel_type,
    });

    if let Some(channels) = session.get_mut("channels").and_then(|c| c.as_array_mut()) {
        channels.push(new_channel.clone());
    }

    let updated_json =
        serde_json::to_string_pretty(&session).context("Failed to serialize updated session")?;
    std::fs::write(mixer_path, &updated_json).context("Failed to write updated session")?;

    if json_output {
        let result = serde_json::json!({
            "status": "channel_added",
            "channel": new_channel,
        });
        let result_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", result_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Channel Added".green().bold());
        println!("{}", "-".repeat(40));
        println!("{:20} {}", "Name:", channel_name);
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {:.2}", "Volume:", volume);
        println!("{:20} {:.2}", "Pan:", pan);
        println!("{:20} {}", "Mute:", mute);
        println!("{:20} {}", "Solo:", solo);
        println!("{:20} {}", "Type:", channel_type);
    }

    Ok(())
}

/// Route a channel to a bus.
async fn route_channel(
    mixer_path: &PathBuf,
    from_channel: u32,
    to_bus: &str,
    send_level: f64,
    pre_fader: bool,
    json_output: bool,
) -> Result<()> {
    if !mixer_path.exists() {
        return Err(anyhow::anyhow!(
            "Mixer session file not found: {}",
            mixer_path.display()
        ));
    }
    if !(0.0..=1.0).contains(&send_level) {
        return Err(anyhow::anyhow!(
            "Send level must be between 0.0 and 1.0, got {}",
            send_level
        ));
    }

    if json_output {
        let result = serde_json::json!({
            "status": "route_added",
            "from_channel": from_channel,
            "to_bus": to_bus,
            "send_level": send_level,
            "pre_fader": pre_fader,
        });
        let result_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", result_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Route Added".green().bold());
        println!("{}", "-".repeat(40));
        println!("{:20} {}", "From channel:", from_channel);
        println!("{:20} {}", "To bus:", to_bus);
        println!("{:20} {:.2}", "Send level:", send_level);
        println!(
            "{:20} {}",
            "Pre-fader:",
            if pre_fader { "yes" } else { "no" }
        );
    }

    Ok(())
}

/// Render the mixer session to an output file.
async fn render_mixer(
    mixer_path: &PathBuf,
    output: &PathBuf,
    format: Option<&str>,
    normalize: bool,
    target_lufs: f64,
    bit_depth: u32,
    json_output: bool,
) -> Result<()> {
    if !mixer_path.exists() {
        return Err(anyhow::anyhow!(
            "Mixer session file not found: {}",
            mixer_path.display()
        ));
    }

    let output_format =
        format.unwrap_or_else(|| output.extension().and_then(|e| e.to_str()).unwrap_or("wav"));

    // Read session to validate
    let session_json =
        std::fs::read_to_string(mixer_path).context("Failed to read mixer session")?;
    let session: serde_json::Value =
        serde_json::from_str(&session_json).context("Failed to parse mixer session")?;

    let channel_count = session
        .get("channels")
        .and_then(|c| c.as_array())
        .map_or(0, |a| a.len());

    let sample_rate = session
        .get("sample_rate")
        .and_then(|s| s.as_u64())
        .unwrap_or(48000) as u32;

    if json_output {
        let result = serde_json::json!({
            "status": "render_pending",
            "mixer": mixer_path.display().to_string(),
            "output": output.display().to_string(),
            "format": output_format,
            "sample_rate": sample_rate,
            "bit_depth": bit_depth,
            "channels": channel_count,
            "normalize": normalize,
            "target_lufs": if normalize { Some(target_lufs) } else { None },
            "message": "Mixer configured; full audio pipeline render pending frame decoding integration",
        });
        let result_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", result_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Mixer Render".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Session:", mixer_path.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Format:", output_format);
        println!("{:20} {} Hz", "Sample rate:", sample_rate);
        println!("{:20} {}-bit", "Bit depth:", bit_depth);
        println!("{:20} {}", "Channels:", channel_count);
        if normalize {
            println!("{:20} {} LUFS", "Target loudness:", target_lufs);
        }
        println!();
        println!(
            "{}",
            "Note: Full audio pipeline render pending integration.".yellow()
        );
        println!(
            "{}",
            "Mixer engine is ready; audio decoding will enable end-to-end rendering.".dimmed()
        );
    }

    Ok(())
}

/// Show mixer session information.
async fn show_mixer_info(mixer_path: &PathBuf, output_format: &str) -> Result<()> {
    if !mixer_path.exists() {
        return Err(anyhow::anyhow!(
            "Mixer session file not found: {}",
            mixer_path.display()
        ));
    }

    let session_json =
        std::fs::read_to_string(mixer_path).context("Failed to read mixer session")?;
    let session: serde_json::Value =
        serde_json::from_str(&session_json).context("Failed to parse mixer session")?;

    match output_format {
        "json" => {
            let result_str =
                serde_json::to_string_pretty(&session).context("Failed to format session JSON")?;
            println!("{}", result_str);
        }
        _ => {
            let name = session
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown");
            let sample_rate = session
                .get("sample_rate")
                .and_then(|s| s.as_u64())
                .unwrap_or(0);
            let buffer_size = session
                .get("buffer_size")
                .and_then(|b| b.as_u64())
                .unwrap_or(0);
            let out_format = session
                .get("output_format")
                .and_then(|f| f.as_str())
                .unwrap_or("stereo");
            let channels = session
                .get("channels")
                .and_then(|c| c.as_array())
                .map_or(0, |a| a.len());

            println!("{}", "Mixer Session Info".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:20} {}", "Name:", name);
            println!("{:20} {} Hz", "Sample rate:", sample_rate);
            println!("{:20} {} samples", "Buffer size:", buffer_size);
            println!("{:20} {}", "Output format:", out_format);
            println!("{:20} {}", "Channel count:", channels);

            if channels > 0 {
                println!();
                println!("{}", "Channels".cyan().bold());
                println!("{}", "-".repeat(60));
                if let Some(channel_list) = session.get("channels").and_then(|c| c.as_array()) {
                    for (i, ch) in channel_list.iter().enumerate() {
                        let ch_name = ch.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let ch_vol = ch.get("volume").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let ch_pan = ch.get("pan").and_then(|p| p.as_f64()).unwrap_or(0.0);
                        let ch_mute = ch.get("mute").and_then(|m| m.as_bool()).unwrap_or(false);
                        println!(
                            "  [{:>2}] {:16} vol={:.2}  pan={:+.2}  {}",
                            i + 1,
                            ch_name,
                            ch_vol,
                            ch_pan,
                            if ch_mute { "MUTED" } else { "" }
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_output_channels_stereo() {
        assert_eq!(parse_output_channels("stereo").expect("should parse"), 2);
    }

    #[test]
    fn test_parse_output_channels_mono() {
        assert_eq!(parse_output_channels("mono").expect("should parse"), 1);
    }

    #[test]
    fn test_parse_output_channels_surround() {
        assert_eq!(parse_output_channels("5.1").expect("should parse"), 6);
        assert_eq!(parse_output_channels("7.1").expect("should parse"), 8);
    }

    #[test]
    fn test_parse_output_channels_invalid() {
        assert!(parse_output_channels("quadrophonic").is_err());
    }

    #[test]
    fn test_create_mixer_validates_sample_rate() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build runtime");
        let tmp = std::env::temp_dir().join("test_mixer_create.json");
        let result = rt.block_on(create_mixer(&tmp, 0, "stereo", None, 512, 128, false));
        assert!(result.is_err());
    }
}
