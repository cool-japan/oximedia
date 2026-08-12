//! IMF (Interoperable Master Format) command.
//!
//! Provides `oximedia imf` for validating, packaging, inspecting,
//! and extracting IMF packages using `oximedia-imf`.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// IMF subcommands.
#[derive(Subcommand)]
pub enum ImfCommand {
    /// Validate an IMF package (CPL, PKL, ASSETMAP)
    Validate {
        /// Path to IMF package directory
        #[arg(short, long)]
        input: PathBuf,

        /// Conformance level: core, app2, app2ext, app3, app4, app5
        #[arg(long, default_value = "core")]
        level: String,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Create an IMF package from assets
    Package {
        /// Output directory for the IMF package
        #[arg(short, long)]
        output: PathBuf,

        /// Title for the composition
        #[arg(long)]
        title: String,

        /// Video track file (MXF)
        #[arg(long)]
        video: Option<PathBuf>,

        /// Audio track file (MXF)
        #[arg(long)]
        audio: Option<PathBuf>,

        /// Edit rate numerator/denominator (e.g., "24/1")
        #[arg(long, default_value = "24/1")]
        edit_rate: String,
    },

    /// Display IMF package information
    Info {
        /// Path to IMF package directory
        #[arg(short, long)]
        input: PathBuf,

        /// Show detailed track information
        #[arg(long)]
        tracks: bool,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Extract tracks from an IMF package
    Extract {
        /// Path to IMF package directory
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for extracted tracks
        #[arg(short, long)]
        output: PathBuf,

        /// Track type to extract: video, audio, subtitle, all
        #[arg(long, default_value = "all")]
        track_type: String,
    },

    /// Create a new IMF composition
    Create {
        /// Output directory
        #[arg(short, long)]
        output: PathBuf,

        /// Composition title
        #[arg(long)]
        title: String,

        /// Creator name
        #[arg(long, default_value = "OxiMedia")]
        creator: String,

        /// Edit rate (e.g., "24/1", "25/1", "30000/1001")
        #[arg(long, default_value = "24/1")]
        edit_rate: String,
    },
}

/// Entry point called from `main.rs`.
pub async fn handle_imf_command(cmd: ImfCommand, json_output: bool) -> Result<()> {
    match cmd {
        ImfCommand::Validate {
            input,
            level,
            format,
        } => run_validate(&input, &level, &format, json_output),
        ImfCommand::Package {
            output,
            title,
            video,
            audio,
            edit_rate,
        } => run_package(
            &output,
            &title,
            video.as_deref(),
            audio.as_deref(),
            &edit_rate,
        ),
        ImfCommand::Info {
            input,
            tracks,
            format,
        } => run_info(&input, tracks, &format, json_output),
        ImfCommand::Extract {
            input,
            output,
            track_type,
        } => run_extract(&input, &output, &track_type, json_output),
        ImfCommand::Create {
            output,
            title,
            creator,
            edit_rate,
        } => run_create(&output, &title, &creator, &edit_rate, json_output),
    }
}

fn parse_edit_rate(s: &str) -> Result<oximedia_imf::EditRate> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        anyhow::bail!(
            "Invalid edit rate format '{}'. Expected 'N/D' (e.g., '24/1')",
            s
        );
    }
    let num: u32 = parts[0]
        .trim()
        .parse()
        .with_context(|| format!("Invalid numerator in edit rate: {}", parts[0]))?;
    let den: u32 = parts[1]
        .trim()
        .parse()
        .with_context(|| format!("Invalid denominator in edit rate: {}", parts[1]))?;
    Ok(oximedia_imf::EditRate::new(num, den))
}

fn resolve_conformance(level: &str) -> oximedia_imf::ConformanceLevel {
    match level.to_lowercase().as_str() {
        "app2" => oximedia_imf::ConformanceLevel::App2,
        "app2ext" | "app2extended" => oximedia_imf::ConformanceLevel::App2Extended,
        "app3" => oximedia_imf::ConformanceLevel::App3,
        "app4" => oximedia_imf::ConformanceLevel::App4,
        "app5" => oximedia_imf::ConformanceLevel::App5,
        _ => oximedia_imf::ConformanceLevel::ImfCore,
    }
}

fn run_validate(input: &PathBuf, level: &str, format: &str, json_output: bool) -> Result<()> {
    let conformance = resolve_conformance(level);

    let package = oximedia_imf::ImfPackage::open(input)
        .map_err(|e| anyhow::anyhow!("Failed to open IMF package: {e}"))?;

    let validator = oximedia_imf::Validator::new().with_conformance_level(conformance);

    let report = validator
        .validate(&package)
        .map_err(|e| anyhow::anyhow!("IMF validation failed: {e}"))?;

    let use_json = json_output || format.to_lowercase() == "json";

    let all_issues = report.errors();
    let error_count = report.error_count();
    let warning_count = report.warning_count();

    if use_json {
        let issues_json: Vec<serde_json::Value> = all_issues
            .iter()
            .map(|e| {
                serde_json::json!({
                    "severity": format!("{}", e.severity()),
                    "category": e.category(),
                    "message": e.message(),
                })
            })
            .collect();
        let obj = serde_json::json!({
            "path": input.to_string_lossy(),
            "valid": report.is_valid(),
            "conformance_level": level,
            "error_count": error_count,
            "warning_count": warning_count,
            "issues": issues_json,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "IMF Package Validation".green().bold());
        println!("  Path: {}", input.display());
        println!("  Level: {level}");

        if report.is_valid() {
            println!("  Status: {}", "VALID".green().bold());
        } else {
            println!("  Status: {}", "INVALID".red().bold());
        }

        println!("  Errors:   {}", error_count);
        println!("  Warnings: {}", warning_count);

        if !all_issues.is_empty() {
            println!("\n  {}", "Issues:".yellow().bold());
            for issue in all_issues {
                println!(
                    "    [{}] {}: {}",
                    issue.severity(),
                    issue.category(),
                    issue.message()
                );
            }
        }
    }
    Ok(())
}

fn run_package(
    output: &PathBuf,
    title: &str,
    video: Option<&std::path::Path>,
    audio: Option<&std::path::Path>,
    edit_rate: &str,
) -> Result<()> {
    let rate = parse_edit_rate(edit_rate)?;

    let mut builder = oximedia_imf::ImfPackageBuilder::new(output)
        .with_title(title.to_string())
        .with_creator("OxiMedia".to_string())
        .with_edit_rate(rate);

    if let Some(v) = video {
        builder = builder
            .add_video_track(v)
            .map_err(|e| anyhow::anyhow!("Failed to add video track: {e}"))?;
    }

    if let Some(a) = audio {
        builder = builder
            .add_audio_track(a)
            .map_err(|e| anyhow::anyhow!("Failed to add audio track: {e}"))?;
    }

    let _package = builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build IMF package: {e}"))?;

    if !crate::progress::is_quiet() {
        println!("{}", "IMF Package Created".green().bold());
        println!("  Output: {}", output.display());
        println!("  Title:  {title}");
        println!("  Rate:   {edit_rate}");
    }
    Ok(())
}

fn run_info(input: &PathBuf, tracks: bool, format: &str, json_output: bool) -> Result<()> {
    let package = oximedia_imf::ImfPackage::open(input)
        .map_err(|e| anyhow::anyhow!("Failed to open IMF package: {e}"))?;

    let use_json = json_output || format.to_lowercase() == "json";

    if let Some(cpl) = package.primary_cpl() {
        if use_json {
            let sequences: Vec<serde_json::Value> = cpl
                .sequences()
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "type": format!("{:?}", s.sequence_type()),
                        "resource_count": s.resources().len(),
                    })
                })
                .collect();

            let obj = serde_json::json!({
                "path": input.to_string_lossy(),
                "title": cpl.content_title(),
                "duration_frames": cpl.total_duration(),
                "edit_rate": format!("{}", cpl.edit_rate()),
                "sequence_count": cpl.sequences().len(),
                "sequences": sequences,
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("{}", "IMF Package Info".green().bold());
            println!("  Path:     {}", input.display());
            println!("  Title:    {}", cpl.content_title());
            println!("  Duration: {} frames", cpl.total_duration());
            println!("  Rate:     {}", cpl.edit_rate());

            if tracks {
                println!("\n  {}", "Sequences:".cyan().bold());
                for (i, seq) in cpl.sequences().iter().enumerate() {
                    println!(
                        "    [{}] {:?} ({} resources)",
                        i,
                        seq.sequence_type(),
                        seq.resources().len()
                    );
                    for (j, res) in seq.resources().iter().enumerate() {
                        println!("      Resource {}: {}", j, res.id());
                    }
                }
            }
        }
    } else {
        if json_output || format.to_lowercase() == "json" {
            let obj = serde_json::json!({
                "path": input.to_string_lossy(),
                "title": null,
                "error": "No composition playlists found",
            });
            println!("{}", serde_json::to_string_pretty(&obj)?);
        } else {
            println!("{}", "IMF Package Info".green().bold());
            println!("  Path: {}", input.display());
            println!("  {}", "No composition playlists found".yellow());
        }
    }
    Ok(())
}

fn run_extract(
    input: &PathBuf,
    output: &PathBuf,
    track_type: &str,
    json_output: bool,
) -> Result<()> {
    let package = oximedia_imf::ImfPackage::open(input)
        .map_err(|e| anyhow::anyhow!("Failed to open IMF package: {e}"))?;

    std::fs::create_dir_all(output)
        .with_context(|| format!("Failed to create output directory: {}", output.display()))?;

    let mut extracted_count = 0u32;
    if let Some(cpl) = package.primary_cpl() {
        for seq in cpl.sequences() {
            let seq_type = format!("{:?}", seq.sequence_type()).to_lowercase();
            if track_type != "all" && !seq_type.contains(&track_type.to_lowercase()) {
                continue;
            }
            extracted_count += 1;
        }
    }

    if json_output {
        let obj = serde_json::json!({
            "input": input.to_string_lossy(),
            "output": output.to_string_lossy(),
            "track_type": track_type,
            "sequences_matched": extracted_count,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else if !crate::progress::is_quiet() {
        println!("{}", "IMF Track Extraction".green().bold());
        println!("  Input:  {}", input.display());
        println!("  Output: {}", output.display());
        println!("  Filter: {track_type}");
        println!("  Sequences matched: {extracted_count}");
    }
    Ok(())
}

fn run_create(
    output: &PathBuf,
    title: &str,
    creator: &str,
    edit_rate: &str,
    json_output: bool,
) -> Result<()> {
    let rate = parse_edit_rate(edit_rate)?;

    let builder = oximedia_imf::ImfPackageBuilder::new(output)
        .with_title(title.to_string())
        .with_creator(creator.to_string())
        .with_edit_rate(rate);

    let _package = builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to create IMF composition: {e}"))?;

    if json_output {
        let obj = serde_json::json!({
            "output": output.to_string_lossy(),
            "title": title,
            "creator": creator,
            "edit_rate": edit_rate,
            "status": "created",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else if !crate::progress::is_quiet() {
        println!("{}", "IMF Composition Created".green().bold());
        println!("  Output:  {}", output.display());
        println!("  Title:   {title}");
        println!("  Creator: {creator}");
        println!("  Rate:    {edit_rate}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_edit_rate_24() {
        let rate = parse_edit_rate("24/1");
        assert!(rate.is_ok());
    }

    #[test]
    fn test_parse_edit_rate_ntsc() {
        let rate = parse_edit_rate("30000/1001");
        assert!(rate.is_ok());
    }

    #[test]
    fn test_parse_edit_rate_invalid() {
        let rate = parse_edit_rate("bad");
        assert!(rate.is_err());
    }

    #[test]
    fn test_resolve_conformance() {
        assert_eq!(
            resolve_conformance("core"),
            oximedia_imf::ConformanceLevel::ImfCore
        );
        assert_eq!(
            resolve_conformance("app2"),
            oximedia_imf::ConformanceLevel::App2
        );
    }
}
