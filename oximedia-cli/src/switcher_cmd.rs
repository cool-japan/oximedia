//! Video switcher CLI commands.
//!
//! Provides subcommands for creating and controlling live production switchers,
//! managing sources, performing transitions, and running switching macros.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;

/// Switcher command subcommands.
#[derive(Subcommand, Debug)]
pub enum SwitcherCommand {
    /// Create a new switcher session
    Create {
        /// Number of M/E rows (1-4)
        #[arg(long, default_value = "1")]
        me_rows: usize,

        /// Number of inputs (2-40)
        #[arg(long, default_value = "8")]
        inputs: usize,

        /// Number of aux outputs
        #[arg(long, default_value = "2")]
        aux: usize,

        /// Preset configuration: basic, professional, broadcast
        #[arg(long)]
        preset: Option<String>,

        /// Frame rate: 25, 29.97, 30, 50, 59.94, 60
        #[arg(long, default_value = "25")]
        fps: f64,
    },

    /// Add a video source input
    AddSource {
        /// Source name / label
        #[arg(short, long)]
        name: String,

        /// Source type: sdi, ndi, file, test_pattern, media_player
        #[arg(long, default_value = "sdi")]
        source_type: String,

        /// Source URI or path
        #[arg(long)]
        uri: Option<String>,

        /// Input slot index (auto-assigned if omitted)
        #[arg(long)]
        slot: Option<usize>,
    },

    /// Switch to a source (cut or auto-transition)
    Switch {
        /// Target input index
        #[arg(short, long)]
        input: usize,

        /// M/E row (0-based, default: 0)
        #[arg(long, default_value = "0")]
        me_row: usize,

        /// Transition type: cut, mix, wipe, dve
        #[arg(long, default_value = "cut")]
        transition: String,

        /// Transition duration in frames (for non-cut transitions)
        #[arg(long, default_value = "30")]
        duration: u32,
    },

    /// Preview a source on the preview bus
    Preview {
        /// Input index to preview
        #[arg(short, long)]
        input: usize,

        /// M/E row (0-based)
        #[arg(long, default_value = "0")]
        me_row: usize,
    },

    /// Start or stop recording the program output
    Record {
        /// Action: start, stop
        #[arg(value_name = "ACTION")]
        action: String,

        /// Output file path (required for start)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,

        /// Video codec: av1, vp9, vp8 (encoder not real in this build --
        /// config validates, `record start` then reports an honest error),
        /// ffv1 (encoder is real; still blocked on the missing live capture
        /// source -- see `handle_record`'s doc comment)
        #[arg(long, default_value = "av1")]
        codec: String,
    },

    /// Run a switching macro (automated transition sequence)
    Macro {
        /// Macro action: run, list, record, stop-record
        #[arg(value_name = "ACTION")]
        action: String,

        /// Macro ID (for run)
        #[arg(long)]
        id: Option<usize>,

        /// Macro name (for record)
        #[arg(long)]
        name: Option<String>,
    },
}

/// Handle switcher command dispatch.
pub async fn handle_switcher_command(command: SwitcherCommand, json_output: bool) -> Result<()> {
    match command {
        SwitcherCommand::Create {
            me_rows,
            inputs,
            aux,
            preset,
            fps,
        } => handle_create(me_rows, inputs, aux, preset.as_deref(), fps, json_output).await,
        SwitcherCommand::AddSource {
            name,
            source_type,
            uri,
            slot,
        } => handle_add_source(&name, &source_type, uri.as_deref(), slot, json_output).await,
        SwitcherCommand::Switch {
            input,
            me_row,
            transition,
            duration,
        } => handle_switch(input, me_row, &transition, duration, json_output).await,
        SwitcherCommand::Preview { input, me_row } => {
            handle_preview(input, me_row, json_output).await
        }
        SwitcherCommand::Record {
            action,
            output,
            codec,
        } => handle_record(&action, output.as_deref(), &codec, json_output).await,
        SwitcherCommand::Macro { action, id, name } => {
            handle_macro(&action, id, name.as_deref(), json_output).await
        }
    }
}

// ---------------------------------------------------------------------------
// Handler: Create
// ---------------------------------------------------------------------------

async fn handle_create(
    me_rows: usize,
    inputs: usize,
    aux: usize,
    preset: Option<&str>,
    fps: f64,
    json_output: bool,
) -> Result<()> {
    let (me, inp, ax) = if let Some(p) = preset {
        match p {
            "basic" => (1, 8, 2),
            "professional" => (2, 20, 6),
            "broadcast" => (4, 40, 10),
            other => {
                return Err(anyhow::anyhow!(
                    "Unknown preset '{}'. Valid: basic, professional, broadcast",
                    other
                ));
            }
        }
    } else {
        (me_rows, inputs, aux)
    };

    if me < 1 || me > 4 {
        return Err(anyhow::anyhow!("M/E rows must be 1-4, got {}", me));
    }
    if inp < 2 || inp > 40 {
        return Err(anyhow::anyhow!("Inputs must be 2-40, got {}", inp));
    }

    let config = oximedia_switcher::SwitcherConfig::new(me, inp, ax);
    let _switcher = oximedia_switcher::Switcher::new(config)
        .map_err(|e| anyhow::anyhow!("Failed to create switcher: {}", e))?;

    if json_output {
        let result = serde_json::json!({
            "action": "create",
            "me_rows": me,
            "inputs": inp,
            "aux_outputs": ax,
            "fps": fps,
            "preset": preset,
            "status": "created",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Switcher Created".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "M/E rows:", me);
        println!("{:20} {}", "Inputs:", inp);
        println!("{:20} {}", "Aux outputs:", ax);
        println!("{:20} {}", "Frame rate:", fps);
        if let Some(p) = preset {
            println!("{:20} {}", "Preset:", p);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: AddSource
// ---------------------------------------------------------------------------

async fn handle_add_source(
    name: &str,
    source_type: &str,
    uri: Option<&str>,
    slot: Option<usize>,
    json_output: bool,
) -> Result<()> {
    let valid_types = ["sdi", "ndi", "file", "test_pattern", "media_player"];
    if !valid_types.contains(&source_type) {
        return Err(anyhow::anyhow!(
            "Unknown source type '{}'. Valid: {}",
            source_type,
            valid_types.join(", ")
        ));
    }

    let assigned_slot = slot.unwrap_or(0);

    if json_output {
        let result = serde_json::json!({
            "action": "add_source",
            "name": name,
            "source_type": source_type,
            "uri": uri,
            "slot": assigned_slot,
            "status": "added",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Source Added".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Name:", name);
        println!("{:20} {}", "Type:", source_type);
        println!("{:20} {}", "URI:", uri.unwrap_or("N/A"));
        println!("{:20} {}", "Slot:", assigned_slot);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Switch
// ---------------------------------------------------------------------------

async fn handle_switch(
    input: usize,
    me_row: usize,
    transition: &str,
    duration: u32,
    json_output: bool,
) -> Result<()> {
    let valid_transitions = ["cut", "mix", "wipe", "dve"];
    if !valid_transitions.contains(&transition) {
        return Err(anyhow::anyhow!(
            "Unknown transition '{}'. Valid: {}",
            transition,
            valid_transitions.join(", ")
        ));
    }

    if json_output {
        let result = serde_json::json!({
            "action": "switch",
            "input": input,
            "me_row": me_row,
            "transition": transition,
            "duration_frames": if transition == "cut" { 0 } else { duration },
            "status": "switched",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Source Switched".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Target input:", input);
        println!("{:20} {}", "M/E row:", me_row);
        println!("{:20} {}", "Transition:", transition);
        if transition != "cut" {
            println!("{:20} {} frames", "Duration:", duration);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Preview
// ---------------------------------------------------------------------------

async fn handle_preview(input: usize, me_row: usize, json_output: bool) -> Result<()> {
    if json_output {
        let result = serde_json::json!({
            "action": "preview",
            "input": input,
            "me_row": me_row,
            "status": "previewing",
        });
        let json_str =
            serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
        println!("{}", json_str);
    } else if !crate::progress::is_quiet() {
        println!("{}", "Preview Set".green().bold());
        println!("{:20} {}", "Input:", input);
        println!("{:20} {}", "M/E row:", me_row);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Handler: Record
// ---------------------------------------------------------------------------

/// Whether `codec`'s underlying video encoder produces real, valid output in
/// this build, per the codec-reality note: MJPEG/FFV1 encoders are real
/// (`oximedia-codec`'s `MjpegEncoder`/`Ffv1Encoder`, wired end-to-end in
/// `oximedia-transcode`'s `codec_dispatch`); AV1/VP9/VP8 encode does not yet
/// produce a valid bitstream. Returns `(is_real, caveat)`, where `caveat`
/// explains the honest state for codecs whose `RecordingCodec` mapping is
/// itself imprecise (VP8 has no dedicated variant).
fn encoder_reality(codec: &str) -> (bool, Option<&'static str>) {
    match codec {
        "ffv1" => (true, None),
        "vp8" => (
            false,
            Some(
                "additionally, oximedia_switcher::recording::RecordingCodec has no dedicated \
                 VP8 variant -- the config above was validated using the Vp9Crf preset purely \
                 as a name/path shape check, not as a stand-in encoder",
            ),
        ),
        _ => (false, None),
    }
}

/// Start or stop recording the switcher's program output.
///
/// `oximedia-switcher` provides a real recording *state machine*
/// (`oximedia_switcher::recording::RecordingManager`/`RecordingTrack`) but it
/// is in-process bookkeeping only -- it does not itself open an encoder or
/// write bytes to disk, and `oximedia-cli`'s `switcher` command has no live
/// capture pipeline or long-running switcher daemon (each CLI invocation
/// configures a switcher and exits immediately). So "start" genuinely cannot
/// produce a real recording here, for *any* codec -- the gap this function
/// closes is distinguishing, per codec, whether the encoder itself is real
/// (FFV1: yes, blocked only on the missing capture source) or also fake
/// (AV1/VP9/VP8: blocked on both). Wiring an actual live capture source is
/// separate, materially larger work (see `crates/oximedia-capture`, not yet
/// a dependency of this CLI) tracked outside this function.
///
/// This still does real, useful work before refusing: it validates the
/// codec, validates the track configuration through the real
/// `RecordingTrack::validate()` (name/output-path checks), checks the output
/// directory actually exists, and exercises the real
/// `RecordingManager::add_track`/`start` state transition -- then reports an
/// honest error instead of a fabricated `"status": "recording"` with no file
/// ever created.
async fn handle_record(
    action: &str,
    output: Option<&std::path::Path>,
    codec: &str,
    json_output: bool,
) -> Result<()> {
    match action {
        "start" => {
            let out = output
                .ok_or_else(|| anyhow::anyhow!("Output path is required for 'start' action"))?;

            let recording_codec = match codec {
                "av1" => oximedia_switcher::recording::RecordingCodec::Av1Cbr,
                "vp9" => oximedia_switcher::recording::RecordingCodec::Vp9Crf,
                // `RecordingCodec` has no dedicated VP8 variant; the closest
                // royalty-free long-GOP preset is reused purely for config
                // validation below (name/path checks only -- see
                // `encoder_reality`'s caveat for VP8, surfaced in the error
                // below).
                "vp8" => oximedia_switcher::recording::RecordingCodec::Vp9Crf,
                // FFV1 has a real encoder (unlike AV1/VP9/VP8) and a real,
                // correctly-named `RecordingCodec` variant -- no reuse hack.
                "ffv1" => oximedia_switcher::recording::RecordingCodec::Ffv1Lossless,
                other => {
                    return Err(anyhow::anyhow!(
                        "Unsupported codec '{}'. Use: av1, vp9, vp8, ffv1",
                        other
                    ));
                }
            };
            let (encoder_is_real, encoder_caveat) = encoder_reality(codec);

            let track = oximedia_switcher::recording::RecordingTrack::new("program_out")
                .with_codec(recording_codec)
                .with_output_path(out.display().to_string());
            track
                .validate()
                .map_err(|e| anyhow::anyhow!("Invalid recording configuration: {e}"))?;

            if let Some(parent) = out.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    return Err(anyhow::anyhow!(
                        "Output directory does not exist: {}",
                        parent.display()
                    ));
                }
            }

            let mut manager = oximedia_switcher::recording::RecordingManager::new();
            let id = manager
                .add_track(track)
                .map_err(|e| anyhow::anyhow!("Failed to register recording track: {e}"))?;
            manager
                .start(id)
                .map_err(|e| anyhow::anyhow!("Failed to start recording track: {e}"))?;
            let state_machine_recording = manager.is_recording(id);

            if json_output {
                let diag = serde_json::json!({
                    "action": "record_start",
                    "output": out.display().to_string(),
                    "codec": codec,
                    "status": "error",
                    "config_validated": true,
                    "state_machine_recording": state_machine_recording,
                    "encoder_is_real": encoder_is_real,
                    "error": "no live capture pipeline; refusing to fabricate a recording",
                });
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&diag).unwrap_or_else(|_| diag.to_string())
                );
            }

            let encoder_status = if encoder_is_real {
                format!(
                    "The {codec} encoder itself is real (oximedia-codec); only the missing \
                     capture source blocks this, not a fake codec."
                )
            } else {
                match encoder_caveat {
                    Some(caveat) => {
                        format!("{codec} also has no real encoder in this build ({caveat}).")
                    }
                    None => format!(
                        "{codec} also has no real encoder in this build (it does not yet \
                         produce a valid bitstream)."
                    ),
                }
            };

            Err(anyhow::anyhow!(
                "Switcher recording is not yet implemented: configuration for '{}' (codec: {}) \
                 validated successfully and the in-process recording state machine transitioned \
                 to recording (is_recording={}), but oximedia-cli's `switcher` command has no \
                 live capture pipeline or persistent switcher process to write real frames to \
                 disk. {} Refusing to report \"recording\" with no file ever written.",
                out.display(),
                codec,
                state_machine_recording,
                encoder_status
            ))
        }
        "stop" => {
            if json_output {
                let diag = serde_json::json!({
                    "action": "record_stop",
                    "status": "error",
                    "error": "no recording is active (switcher recording is not implemented)",
                });
                eprintln!(
                    "{}",
                    serde_json::to_string_pretty(&diag).unwrap_or_else(|_| diag.to_string())
                );
            }
            Err(anyhow::anyhow!(
                "Switcher recording is not yet implemented, so no recording can be stopped. \
                 See `oximedia switcher record start` for details on the current limitation."
            ))
        }
        other => Err(anyhow::anyhow!(
            "Unknown record action '{}'. Use: start, stop",
            other
        )),
    }
}

// ---------------------------------------------------------------------------
// Handler: Macro
// ---------------------------------------------------------------------------

async fn handle_macro(
    action: &str,
    id: Option<usize>,
    name: Option<&str>,
    json_output: bool,
) -> Result<()> {
    match action {
        "run" => {
            let macro_id =
                id.ok_or_else(|| anyhow::anyhow!("Macro ID is required for 'run' action (--id)"))?;

            if json_output {
                let result = serde_json::json!({
                    "action": "macro_run",
                    "macro_id": macro_id,
                    "status": "running",
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
                println!("{}", json_str);
            } else if !crate::progress::is_quiet() {
                println!("{}", "Macro Running".green().bold());
                println!("{:20} {}", "Macro ID:", macro_id);
            }
        }
        "list" => {
            if json_output {
                let result = serde_json::json!({
                    "action": "macro_list",
                    "macros": [],
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
                println!("{}", json_str);
            } else {
                println!("{}", "Available Macros".green().bold());
                println!("{}", "=".repeat(60));
                println!("{}", "No macros defined.".dimmed());
            }
        }
        "record" => {
            let macro_name = name.unwrap_or("Untitled Macro");
            if json_output {
                let result = serde_json::json!({
                    "action": "macro_record",
                    "name": macro_name,
                    "status": "recording",
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
                println!("{}", json_str);
            } else if !crate::progress::is_quiet() {
                println!("{}", "Macro Recording Started".green().bold());
                println!("{:20} {}", "Name:", macro_name);
            }
        }
        "stop-record" => {
            if json_output {
                let result = serde_json::json!({
                    "action": "macro_stop_record",
                    "status": "saved",
                });
                let json_str =
                    serde_json::to_string_pretty(&result).context("Failed to serialize result")?;
                println!("{}", json_str);
            } else if !crate::progress::is_quiet() {
                println!("{}", "Macro Recording Stopped".green().bold());
            }
        }
        other => {
            return Err(anyhow::anyhow!(
                "Unknown macro action '{}'. Valid: run, list, record, stop-record",
                other
            ));
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

    #[tokio::test]
    async fn test_handle_create_basic() {
        let result = handle_create(1, 8, 2, None, 25.0, false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_create_preset() {
        let result = handle_create(0, 0, 0, Some("professional"), 25.0, false).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_handle_create_invalid_me_rows() {
        let result = handle_create(5, 8, 2, None, 25.0, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_switch_invalid_transition() {
        let result = handle_switch(1, 0, "invalid", 30, false).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handle_record_start_no_output() {
        let result = handle_record("start", None, "av1", false).await;
        assert!(result.is_err());
    }

    // ── record start/stop: honest-Err, never a fabricated "recording" ───────

    #[tokio::test]
    async fn test_handle_record_start_never_claims_success() {
        let dir = std::env::temp_dir();
        let out = dir.join("oximedia_switcher_test_record_output.mkv");
        let _ = std::fs::remove_file(&out);

        let err = handle_record("start", Some(out.as_path()), "av1", false)
            .await
            .expect_err("switcher recording must not report success");
        let msg = err.to_string();
        assert!(
            msg.contains("not yet implemented"),
            "error should be honest about the limitation, got: {msg}"
        );
        assert!(
            !msg.to_lowercase().contains("\"status\": \"recording\""),
            "must not resemble the old fabricated JSON success payload"
        );
        assert!(
            !out.exists(),
            "no output file should be fabricated for a recording that never started"
        );
    }

    #[tokio::test]
    async fn test_handle_record_start_invalid_output_dir() {
        let missing_parent = std::env::temp_dir()
            .join("oximedia_switcher_test_definitely_missing_dir_xyz")
            .join("out.mkv");

        let err = handle_record("start", Some(missing_parent.as_path()), "av1", false)
            .await
            .expect_err("a nonexistent output directory must fail");
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_handle_record_start_unknown_codec() {
        let dir = std::env::temp_dir();
        let out = dir.join("oximedia_switcher_test_record_bad_codec.mkv");
        let err = handle_record("start", Some(out.as_path()), "h264", false)
            .await
            .expect_err("unsupported codec must fail");
        assert!(err.to_string().contains("Unsupported codec"));
    }

    #[tokio::test]
    async fn test_handle_record_stop_never_claims_success() {
        let err = handle_record("stop", None, "av1", false)
            .await
            .expect_err("stop must be honest when nothing was ever recording");
        assert!(err.to_string().contains("not yet implemented"));
    }

    // ── per-codec honesty: real (FFV1) vs fake (AV1/VP9/VP8) encoders ───────

    #[test]
    fn test_encoder_reality_ffv1_is_real() {
        let (is_real, caveat) = encoder_reality("ffv1");
        assert!(is_real, "FFV1 has a real encoder in oximedia-codec");
        assert!(caveat.is_none());
    }

    #[test]
    fn test_encoder_reality_av1_vp9_vp8_are_fake() {
        for codec in ["av1", "vp9", "vp8"] {
            let (is_real, _) = encoder_reality(codec);
            assert!(
                !is_real,
                "{codec} encode does not yet produce a valid bitstream"
            );
        }
    }

    #[test]
    fn test_encoder_reality_vp8_names_missing_recording_codec_variant() {
        let (_, caveat) = encoder_reality("vp8");
        let caveat = caveat.expect("VP8 must carry a caveat about the RecordingCodec mapping");
        assert!(caveat.contains("no dedicated"), "caveat: {caveat}");
    }

    #[tokio::test]
    async fn test_handle_record_start_ffv1_accepted_and_error_says_encoder_is_real() {
        let dir = std::env::temp_dir();
        let out = dir.join("oximedia_switcher_test_record_ffv1.mkv");
        let _ = std::fs::remove_file(&out);

        let err = handle_record("start", Some(out.as_path()), "ffv1", false)
            .await
            .expect_err("still no live capture pipeline, so this must still error");
        let msg = err.to_string();
        assert!(
            msg.contains("real") && msg.to_lowercase().contains("ffv1"),
            "error must credit FFV1 as a real encoder, got: {msg}"
        );
        assert!(
            !msg.contains("also has no real encoder"),
            "must not claim FFV1's own encoder is fake, got: {msg}"
        );
        assert!(!out.exists(), "no output file may be fabricated");
    }

    #[tokio::test]
    async fn test_handle_record_start_av1_error_says_encoder_is_fake() {
        let dir = std::env::temp_dir();
        let out = dir.join("oximedia_switcher_test_record_av1_fake.mkv");
        let _ = std::fs::remove_file(&out);

        let err = handle_record("start", Some(out.as_path()), "av1", false)
            .await
            .expect_err("must still error (no capture pipeline)");
        let msg = err.to_string();
        assert!(
            msg.contains("also has no real encoder"),
            "AV1's own encoder must be named as fake too, got: {msg}"
        );
    }

    #[tokio::test]
    async fn test_handle_record_start_vp8_error_names_missing_recording_codec_variant() {
        let dir = std::env::temp_dir();
        let out = dir.join("oximedia_switcher_test_record_vp8_caveat.mkv");
        let _ = std::fs::remove_file(&out);

        let err = handle_record("start", Some(out.as_path()), "vp8", false)
            .await
            .expect_err("must still error (no capture pipeline)");
        let msg = err.to_string();
        assert!(
            msg.contains("no dedicated"),
            "VP8's RecordingCodec gap must be disclosed, got: {msg}"
        );
    }
}
