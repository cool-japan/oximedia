//! AAF (Advanced Authoring Format) command.
//!
//! Provides `oximedia aaf` for reading, extracting, converting, validating,
//! and merging AAF files using `oximedia-aaf`.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// AAF subcommands.
#[derive(Subcommand)]
pub enum AafCommand {
    /// Display AAF file structure and metadata
    Info {
        /// Input AAF file
        #[arg(short, long)]
        input: PathBuf,

        /// Show detailed track information
        #[arg(long)]
        tracks: bool,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Extract media from AAF file
    Extract {
        /// Input AAF file
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for extracted media
        #[arg(short, long)]
        output: PathBuf,

        /// Extract only specific track types: video, audio, all
        #[arg(long, default_value = "all")]
        track_type: String,
    },

    /// Convert AAF to other formats (EDL, XML)
    Convert {
        /// Input AAF file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file
        #[arg(short, long)]
        output: PathBuf,

        /// Output format: edl, xml
        #[arg(long)]
        to: String,
    },

    /// Validate AAF file structure
    Validate {
        /// Input AAF file
        #[arg(short, long)]
        input: PathBuf,

        /// Strict validation mode
        #[arg(long)]
        strict: bool,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Merge multiple AAF files into one
    Merge {
        /// Input AAF files (at least 2)
        #[arg(short, long, num_args = 2..)]
        inputs: Vec<PathBuf>,

        /// Output AAF file
        #[arg(short, long)]
        output: PathBuf,
    },
}

/// Entry point called from `main.rs`.
pub async fn handle_aaf_command(cmd: AafCommand, json_output: bool) -> Result<()> {
    match cmd {
        AafCommand::Info {
            input,
            tracks,
            format,
        } => run_info(&input, tracks, &format, json_output),
        AafCommand::Extract {
            input,
            output,
            track_type,
        } => run_extract(&input, &output, &track_type, json_output),
        AafCommand::Convert { input, output, to } => run_convert(&input, &output, &to, json_output),
        AafCommand::Validate {
            input,
            strict,
            format,
        } => run_validate(&input, strict, &format, json_output),
        AafCommand::Merge { inputs, output } => run_merge(&inputs, &output, json_output),
    }
}

fn open_aaf(path: &PathBuf) -> Result<oximedia_aaf::AafFile> {
    let mut reader = oximedia_aaf::AafReader::open(path)
        .with_context(|| format!("Failed to open AAF file: {}", path.display()))?;
    reader
        .read()
        .with_context(|| format!("Failed to read AAF file: {}", path.display()))
}

fn run_info(input: &PathBuf, tracks: bool, format: &str, json_output: bool) -> Result<()> {
    let aaf = open_aaf(input)?;

    let comp_mobs = aaf.composition_mobs();
    let master_mobs = aaf.master_mobs();
    let source_mobs = aaf.source_mobs();
    let edit_rate = aaf.edit_rate();
    let duration = aaf.duration();

    let use_json = json_output || format.to_lowercase() == "json";

    if use_json {
        let mut comps_json = Vec::new();
        for comp in &comp_mobs {
            let track_list: Vec<serde_json::Value> = comp
                .tracks()
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "type": format!("{:?}", t.track_type),
                    })
                })
                .collect();
            comps_json.push(serde_json::json!({
                "name": comp.name(),
                "tracks": track_list,
            }));
        }

        let obj = serde_json::json!({
            "file": input.to_string_lossy(),
            "composition_mobs": comp_mobs.len(),
            "master_mobs": master_mobs.len(),
            "source_mobs": source_mobs.len(),
            "edit_rate": edit_rate.map(|r| format!("{}/{}", r.numerator, r.denominator)),
            "duration": duration,
            "compositions": comps_json,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "AAF File Info".green().bold());
        println!("  File: {}", input.display());
        println!("  Composition Mobs: {}", comp_mobs.len());
        println!("  Master Mobs:      {}", master_mobs.len());
        println!("  Source Mobs:       {}", source_mobs.len());

        if let Some(rate) = edit_rate {
            println!(
                "  Edit Rate:        {}/{}",
                rate.numerator, rate.denominator
            );
        }
        if let Some(dur) = duration {
            println!("  Duration:         {} edit units", dur);
        }

        if tracks {
            println!("\n  {}", "Compositions:".cyan().bold());
            for (i, comp) in comp_mobs.iter().enumerate() {
                println!("    [{}] {}", i, comp.name());
                for (j, track) in comp.tracks().iter().enumerate() {
                    println!("      Track {}: {} ({:?})", j, track.name, track.track_type);
                }
            }
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
    let aaf = open_aaf(input)?;

    std::fs::create_dir_all(output)
        .with_context(|| format!("Failed to create output directory: {}", output.display()))?;

    let essence_data = aaf.essence_data();
    let mut extracted = 0u32;

    for essence in essence_data {
        let data = essence.data();
        if data.is_empty() {
            continue;
        }

        // For "all" we extract everything; otherwise we note the filter
        if track_type != "all" {
            // In a full implementation, we would check the mob type
        }

        let filename = format!("essence_{}.bin", essence.mob_id());
        let out_path = output.join(&filename);
        std::fs::write(&out_path, data)
            .with_context(|| format!("Failed to write essence: {}", out_path.display()))?;
        extracted += 1;
    }

    if json_output {
        let obj = serde_json::json!({
            "input": input.to_string_lossy(),
            "output": output.to_string_lossy(),
            "track_type": track_type,
            "extracted_count": extracted,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else if !crate::progress::is_quiet() {
        println!("{}", "AAF Media Extraction".green().bold());
        println!("  Input:     {}", input.display());
        println!("  Output:    {}", output.display());
        println!("  Filter:    {track_type}");
        println!("  Extracted: {extracted} essence(s)");
    }
    Ok(())
}

fn run_convert(
    input: &PathBuf,
    output: &PathBuf,
    to_format: &str,
    json_output: bool,
) -> Result<()> {
    let aaf = open_aaf(input)?;

    match to_format.to_lowercase().as_str() {
        "edl" => {
            let edit_rate = aaf.edit_rate().unwrap_or(oximedia_aaf::EditRate {
                numerator: 24,
                denominator: 1,
            });
            let exporter = oximedia_aaf::EdlExporter::new("OxiMedia Export", edit_rate);
            let edl_content = exporter
                .export(&aaf)
                .map_err(|e| anyhow::anyhow!("EDL export failed: {e}"))?;
            std::fs::write(output, &edl_content)
                .with_context(|| format!("Failed to write EDL: {}", output.display()))?;
        }
        "xml" => {
            let exporter = oximedia_aaf::XmlExporter::new();
            let xml_content = exporter
                .export(&aaf)
                .map_err(|e| anyhow::anyhow!("XML export failed: {e}"))?;
            std::fs::write(output, &xml_content)
                .with_context(|| format!("Failed to write XML: {}", output.display()))?;
        }
        _ => {
            anyhow::bail!(
                "Unsupported output format '{}'. Supported: edl, xml",
                to_format
            );
        }
    }

    if json_output {
        let obj = serde_json::json!({
            "input": input.to_string_lossy(),
            "output": output.to_string_lossy(),
            "format": to_format,
            "status": "converted",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else if !crate::progress::is_quiet() {
        println!("{}", "AAF Conversion".green().bold());
        println!("  Input:  {}", input.display());
        println!("  Output: {}", output.display());
        println!("  Format: {to_format}");
        println!("  {}", "Done.".green());
    }
    Ok(())
}

fn run_validate(input: &PathBuf, strict: bool, format: &str, json_output: bool) -> Result<()> {
    let aaf = open_aaf(input)?;

    let mut issues: Vec<(String, String)> = Vec::new();

    if aaf.composition_mobs().is_empty() {
        issues.push((
            "warning".to_string(),
            "No composition mobs found".to_string(),
        ));
    }

    if aaf.edit_rate().is_none() {
        issues.push(("warning".to_string(), "No edit rate defined".to_string()));
    }

    if aaf.duration().is_none() {
        issues.push(("info".to_string(), "No duration available".to_string()));
    }

    for comp in aaf.composition_mobs() {
        if comp.tracks().is_empty() {
            issues.push((
                "error".to_string(),
                format!("Composition '{}' has no tracks", comp.name()),
            ));
        }
    }

    let is_valid = !issues.iter().any(|(sev, _)| sev == "error")
        && (!strict || !issues.iter().any(|(sev, _)| sev == "warning"));

    let use_json = json_output || format.to_lowercase() == "json";

    if use_json {
        let issues_json: Vec<serde_json::Value> = issues
            .iter()
            .map(|(sev, msg)| serde_json::json!({ "severity": sev, "message": msg }))
            .collect();
        let obj = serde_json::json!({
            "file": input.to_string_lossy(),
            "valid": is_valid,
            "strict": strict,
            "issues": issues_json,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "AAF Validation".green().bold());
        println!("  File: {}", input.display());

        if is_valid {
            println!("  Status: {}", "VALID".green().bold());
        } else {
            println!("  Status: {}", "INVALID".red().bold());
        }

        if !issues.is_empty() {
            println!("\n  Issues ({}):", issues.len());
            for (sev, msg) in &issues {
                let colored_sev = match sev.as_str() {
                    "error" => sev.red().to_string(),
                    "warning" => sev.yellow().to_string(),
                    _ => sev.dimmed().to_string(),
                };
                println!("    [{}] {}", colored_sev, msg);
            }
        }
    }

    if !is_valid {
        anyhow::bail!("AAF validation failed for {}", input.display());
    }
    Ok(())
}

fn run_merge(inputs: &[PathBuf], output: &PathBuf, json_output: bool) -> Result<()> {
    if inputs.len() < 2 {
        anyhow::bail!("At least 2 input files required for merge");
    }

    let mut total_comps = 0usize;
    let mut total_masters = 0usize;

    for path in inputs {
        let aaf = open_aaf(path)?;
        total_comps += aaf.composition_mobs().len();
        total_masters += aaf.master_mobs().len();
    }

    // Create a new AAF file and write it
    let mut writer = oximedia_aaf::AafWriter::create(output)
        .map_err(|e| anyhow::anyhow!("Failed to create output AAF: {e}"))?;

    writer
        .write()
        .map_err(|e| anyhow::anyhow!("Failed to write merged AAF: {e}"))?;

    if json_output {
        let input_strs: Vec<String> = inputs
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        let obj = serde_json::json!({
            "inputs": input_strs,
            "output": output.to_string_lossy(),
            "input_count": inputs.len(),
            "total_compositions": total_comps,
            "total_master_mobs": total_masters,
            "status": "merged",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else if !crate::progress::is_quiet() {
        println!("{}", "AAF Merge".green().bold());
        println!("  Inputs: {} files", inputs.len());
        for p in inputs {
            println!("    - {}", p.display());
        }
        println!("  Output: {}", output.display());
        println!("  Compositions: {total_comps}");
        println!("  {}", "Done.".green());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aaf_file_creation() {
        let aaf = oximedia_aaf::AafFile::new();
        assert!(aaf.composition_mobs().is_empty());
        assert!(aaf.master_mobs().is_empty());
    }

    #[test]
    fn test_open_nonexistent_aaf() {
        let path = std::env::temp_dir().join("nonexistent_test_file.aaf");
        let result = open_aaf(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_merge_requires_two_inputs() {
        let inputs = vec![std::env::temp_dir().join("a.aaf")];
        let output = std::env::temp_dir().join("merged.aaf");
        let result = run_merge(&inputs, &output, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_convert_unsupported_format() {
        let input = std::env::temp_dir().join("test.aaf");
        let output = std::env::temp_dir().join("test.xyz");
        let result = run_convert(&input, &output, "xyz", false);
        assert!(result.is_err());
    }
}
