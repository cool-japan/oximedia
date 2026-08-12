//! Playlist command — playout automation for OxiMedia CLI.
//!
//! Provides `oximedia playlist` with generate, validate, and play subcommands.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;
use std::time::Duration;

/// Subcommands for `oximedia playlist`.
#[derive(Subcommand)]
pub enum PlaylistSubcommand {
    /// Generate a playlist from a directory of media files
    Generate {
        /// Input directory containing media files
        #[arg(short, long)]
        input: PathBuf,

        /// Output M3U8 playlist file path
        #[arg(short, long)]
        output: PathBuf,

        /// Default clip duration in seconds (used when duration cannot be probed)
        #[arg(long, default_value = "30")]
        duration: u64,
    },

    /// Validate an existing M3U8 playlist file
    Validate {
        /// Input playlist file
        #[arg(short, long)]
        input: PathBuf,
    },

    /// Simulate playback of a playlist
    Play {
        /// Input playlist file
        #[arg(short, long)]
        input: PathBuf,

        /// Loop playlist indefinitely (simulation only)
        #[arg(long = "loop")]
        loop_playback: bool,
    },
}

/// Handle `oximedia playlist` subcommands.
pub async fn handle_playlist_command(command: PlaylistSubcommand, json_output: bool) -> Result<()> {
    match command {
        PlaylistSubcommand::Generate {
            input,
            output,
            duration,
        } => cmd_generate(&input, &output, duration, json_output).await,

        PlaylistSubcommand::Validate { input } => cmd_validate(&input, json_output).await,

        PlaylistSubcommand::Play {
            input,
            loop_playback,
        } => cmd_play(&input, loop_playback, json_output).await,
    }
}

// ── Supported video extensions ────────────────────────────────────────────────

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "webm", "avi", "mov", "mxf", "ts", "m2ts", "ogg", "ogv",
];

fn is_video_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

// ── Generate ──────────────────────────────────────────────────────────────────

async fn cmd_generate(
    input: &PathBuf,
    output: &PathBuf,
    default_duration_secs: u64,
    json_output: bool,
) -> Result<()> {
    use oximedia_playlist::playlist::{Playlist, PlaylistItem, PlaylistType};

    if !input.is_dir() {
        anyhow::bail!("Input path is not a directory: {}", input.display());
    }

    let mut playlist = Playlist::new("generated", PlaylistType::Linear);
    let mut media_files: Vec<PathBuf> = Vec::new();

    let entries = std::fs::read_dir(input)
        .with_context(|| format!("Failed to read directory: {}", input.display()))?;

    for entry in entries {
        let entry = entry.with_context(|| "Failed to read directory entry")?;
        let path = entry.path();
        if path.is_file() && is_video_file(&path) {
            media_files.push(path);
        }
    }

    // Sort for deterministic ordering
    media_files.sort();

    if media_files.is_empty() {
        anyhow::bail!(
            "No supported video files found in directory: {}",
            input.display()
        );
    }

    let default_duration = Duration::from_secs(default_duration_secs);

    let mut m3u8_lines: Vec<String> = Vec::new();
    m3u8_lines.push("#EXTM3U".to_string());
    m3u8_lines.push(format!("#EXT-X-VERSION:3"));
    m3u8_lines.push(String::new());

    for path in &media_files {
        let item =
            PlaylistItem::new(path.to_string_lossy().as_ref()).with_duration(default_duration);

        let display_name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");

        m3u8_lines.push(format!(
            "#EXTINF:{},{}\n{}",
            default_duration_secs,
            display_name,
            path.display()
        ));

        playlist.add_item(item);
    }

    let content = m3u8_lines.join("\n");
    std::fs::write(output, &content)
        .with_context(|| format!("Failed to write playlist: {}", output.display()))?;

    if json_output {
        let json = serde_json::json!({
            "status": "ok",
            "output": output.display().to_string(),
            "files_added": media_files.len(),
            "default_duration_secs": default_duration_secs,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !crate::progress::is_quiet() {
        println!(
            "{} Playlist generated: {}",
            "OK".green().bold(),
            output.display()
        );
        println!(
            "   {} media files added (default duration: {}s each)",
            media_files.len(),
            default_duration_secs
        );
        for f in &media_files {
            println!("   + {}", f.display());
        }
    }

    Ok(())
}

// ── Validate ──────────────────────────────────────────────────────────────────

async fn cmd_validate(input: &PathBuf, json_output: bool) -> Result<()> {
    let content = std::fs::read_to_string(input)
        .with_context(|| format!("Failed to read playlist: {}", input.display()))?;

    let mut issues: Vec<String> = Vec::new();
    let mut entry_count = 0usize;
    let mut extinf_count = 0usize;
    let mut uri_count = 0usize;

    if !content.starts_with("#EXTM3U") {
        issues.push("Missing #EXTM3U header".to_string());
    }

    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            if trimmed.starts_with("#EXTINF") {
                extinf_count += 1;
            }
            continue;
        }
        // Non-comment, non-empty lines are URIs
        uri_count += 1;
        entry_count += 1;

        // Check that each URI has a preceding EXTINF
        if extinf_count < uri_count {
            issues.push(format!(
                "Line {}: URI '{}' lacks preceding #EXTINF tag",
                line_no + 1,
                trimmed
            ));
        }
    }

    if extinf_count != uri_count {
        issues.push(format!(
            "Mismatched EXTINF ({}) and URI ({}) counts",
            extinf_count, uri_count
        ));
    }

    let valid = issues.is_empty();

    if json_output {
        let json = serde_json::json!({
            "valid": valid,
            "entry_count": entry_count,
            "issues": issues,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if valid {
        println!(
            "{} Playlist is valid: {} entries",
            "PASS".green().bold(),
            entry_count
        );
    } else {
        println!("{} Playlist has issues:", "FAIL".red().bold());
        for issue in &issues {
            println!("   - {}", issue.yellow());
        }
    }

    if !valid {
        anyhow::bail!("Playlist validation failed with {} issue(s)", issues.len());
    }

    Ok(())
}

// ── Play ──────────────────────────────────────────────────────────────────────

async fn cmd_play(input: &PathBuf, loop_playback: bool, json_output: bool) -> Result<()> {
    let content = std::fs::read_to_string(input)
        .with_context(|| format!("Failed to read playlist: {}", input.display()))?;

    // Parse the M3U8 into (title, duration_secs, uri) tuples
    let mut entries: Vec<(String, u64, String)> = Vec::new();
    let mut current_extinf: Option<(String, u64)> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("#EXTINF:") {
            // #EXTINF:<duration>,<title>
            let (dur_str, title) = rest.split_once(',').unwrap_or((rest, "Unknown"));
            let dur_secs: u64 = dur_str
                .trim()
                .parse::<f64>()
                .map(|f| f as u64)
                .unwrap_or(30);
            current_extinf = Some((title.trim().to_string(), dur_secs));
        } else if !trimmed.starts_with('#') {
            // URI line
            let (title, dur) = current_extinf
                .take()
                .unwrap_or_else(|| ("Unknown".to_string(), 30));
            entries.push((title, dur, trimmed.to_string()));
        }
    }

    if entries.is_empty() {
        anyhow::bail!("Playlist contains no playable entries");
    }

    if json_output {
        let json_entries: Vec<_> = entries
            .iter()
            .map(|(title, dur, uri)| {
                serde_json::json!({
                    "title": title,
                    "duration_secs": dur,
                    "uri": uri,
                })
            })
            .collect();
        let json = serde_json::json!({
            "playlist": input.display().to_string(),
            "entry_count": entries.len(),
            "loop": loop_playback,
            "entries": json_entries,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
        return Ok(());
    }

    println!(
        "{} Simulating playback of: {}",
        "PLAY".green().bold(),
        input.display()
    );
    if loop_playback {
        println!("     (loop mode — showing 1 pass)");
    }
    println!();

    let mut offset_secs: u64 = 0;
    for (idx, (title, dur, uri)) in entries.iter().enumerate() {
        let hh = offset_secs / 3600;
        let mm = (offset_secs % 3600) / 60;
        let ss = offset_secs % 60;
        println!(
            "  [{:02}:{:02}:{:02}] #{:03}  {}  ({:>4}s)",
            hh,
            mm,
            ss,
            idx + 1,
            title.cyan(),
            dur
        );
        println!("              => {}", uri.dimmed());
        offset_secs += dur;
    }
    println!();
    let total_hh = offset_secs / 3600;
    let total_mm = (offset_secs % 3600) / 60;
    let total_ss = offset_secs % 60;
    println!(
        "   Total runtime: {:02}:{:02}:{:02}",
        total_hh, total_mm, total_ss
    );

    Ok(())
}
