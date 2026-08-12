//! Repair command for `oximedia repair`.
//!
//! Provides analyze, fix, and batch subcommands via `oximedia-repair`.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Subcommands for `oximedia repair`.
#[derive(Subcommand)]
pub enum RepairCommand {
    /// Analyze a media file for issues without making changes
    Analyze {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Show detailed findings
        #[arg(long)]
        detail: bool,
    },

    /// Repair a media file, writing fixed output
    Fix {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output path for repaired file (defaults to temp dir)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Repair mode: safe, balanced, aggressive, extract
        #[arg(short, long, default_value = "balanced")]
        mode: String,

        /// Skip creating a backup before repair
        #[arg(long)]
        no_backup: bool,

        /// Skip verification after repair
        #[arg(long)]
        no_verify: bool,

        /// Fix only specific issues (comma-separated): header, index, timestamps, avsync, truncated, packets, metadata, keyframes, frameorder, conversion
        #[arg(long, default_value = "")]
        issues: String,
    },

    /// Repair multiple media files in batch
    Batch {
        /// Input media files
        #[arg(required = true)]
        inputs: Vec<PathBuf>,

        /// Output directory for repaired files
        #[arg(short, long)]
        output_dir: Option<PathBuf>,

        /// Repair mode: safe, balanced, aggressive, extract
        #[arg(short, long, default_value = "balanced")]
        mode: String,

        /// Skip creating backups before repair
        #[arg(long)]
        no_backup: bool,
    },
}

/// Entry point called from `main.rs`.
pub async fn run_repair(command: RepairCommand, json_output: bool) -> Result<()> {
    match command {
        RepairCommand::Analyze { input, detail } => cmd_analyze(input, detail, json_output),
        RepairCommand::Fix {
            input,
            output,
            mode,
            no_backup,
            no_verify,
            issues,
        } => cmd_fix(
            input,
            output,
            &mode,
            no_backup,
            no_verify,
            &issues,
            json_output,
        ),
        RepairCommand::Batch {
            inputs,
            output_dir,
            mode,
            no_backup,
        } => cmd_batch(inputs, output_dir, &mode, no_backup, json_output),
    }
}

// ---------------------------------------------------------------------------
// Subcommand implementations
// ---------------------------------------------------------------------------

fn cmd_analyze(input: PathBuf, detail: bool, json_output: bool) -> Result<()> {
    use oximedia_repair::RepairEngine;

    if !input.exists() {
        anyhow::bail!("Input file not found: {}", input.display());
    }

    let engine = RepairEngine::new();
    let issues = engine
        .analyze(&input)
        .map_err(|e| anyhow::anyhow!("Analysis failed: {}", e))?;

    if json_output {
        let issues_json: Vec<serde_json::Value> = issues
            .iter()
            .map(|issue| {
                serde_json::json!({
                    "type": format!("{:?}", issue.issue_type),
                    "severity": format!("{:?}", issue.severity),
                    "description": issue.description,
                    "location": issue.location,
                    "fixable": issue.fixable,
                })
            })
            .collect();

        let obj = serde_json::json!({
            "file": input.to_string_lossy(),
            "total_issues": issues.len(),
            "fixable_count": issues.iter().filter(|i| i.fixable).count(),
            "issues": issues_json,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "Media Repair Analysis".green().bold());
    println!("  File:    {}", input.display().to_string().cyan());

    let file_meta = std::fs::metadata(&input)
        .with_context(|| format!("Cannot stat file: {}", input.display()))?;
    println!("  Size:    {} bytes", file_meta.len());
    println!();

    if issues.is_empty() {
        println!(
            "  {} No issues detected — file appears healthy",
            "✓".green().bold()
        );
        return Ok(());
    }

    let fixable = issues.iter().filter(|i| i.fixable).count();
    println!(
        "  {} {} issue(s) found ({} fixable, {} unfixable)",
        "!".yellow(),
        issues.len(),
        fixable,
        issues.len() - fixable
    );
    println!();

    let severity_color = |s: &oximedia_repair::Severity| match s {
        oximedia_repair::Severity::Low => "Low".green().to_string(),
        oximedia_repair::Severity::Medium => "Medium".yellow().to_string(),
        oximedia_repair::Severity::High => "High".red().to_string(),
        oximedia_repair::Severity::Critical => "Critical".red().bold().to_string(),
    };

    println!("  {}", "Issues:".cyan().bold());
    for (idx, issue) in issues.iter().enumerate() {
        let fixable_str = if issue.fixable {
            "fixable".green().to_string()
        } else {
            "unfixable".red().to_string()
        };
        println!(
            "    {}. [{:?}] {}",
            idx + 1,
            issue.issue_type,
            severity_color(&issue.severity)
        );
        println!("       {}", issue.description);
        if detail {
            if let Some(loc) = issue.location {
                println!("       Location: byte offset {}", loc);
            }
            println!("       Status: {}", fixable_str);
        }
    }

    Ok(())
}

fn cmd_fix(
    input: PathBuf,
    output: Option<PathBuf>,
    mode_str: &str,
    no_backup: bool,
    no_verify: bool,
    issues_str: &str,
    json_output: bool,
) -> Result<()> {
    use oximedia_repair::{RepairEngine, RepairOptions};

    if !input.exists() {
        anyhow::bail!("Input file not found: {}", input.display());
    }

    let mode = parse_repair_mode(mode_str)?;
    let fix_issues = parse_issue_types(issues_str)?;

    let options = RepairOptions {
        mode,
        create_backup: !no_backup,
        verify_after_repair: !no_verify,
        output_dir: output
            .as_ref()
            .and_then(|p| p.parent().map(|pa| pa.to_path_buf())),
        fix_issues,
        verbose: !json_output,
        ..Default::default()
    };

    let engine = RepairEngine::new();

    // If a specific output path is given, resolve the output dir
    let output_dir = if let Some(ref out) = output {
        if let Some(parent) = out.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Cannot create output dir: {}", parent.display()))?;
            }
        }
        out.parent().map(|p| p.to_path_buf())
    } else {
        None
    };

    let effective_options = RepairOptions {
        output_dir,
        ..options
    };

    let result = engine
        .repair_file(&input, &effective_options)
        .map_err(|e| anyhow::anyhow!("Repair failed: {}", e))?;

    if json_output {
        let fixed_json: Vec<serde_json::Value> = result
            .fixed_issues
            .iter()
            .map(|i| {
                serde_json::json!({
                    "type": format!("{:?}", i.issue_type),
                    "severity": format!("{:?}", i.severity),
                    "description": i.description,
                })
            })
            .collect();

        let unfixed_json: Vec<serde_json::Value> = result
            .unfixed_issues
            .iter()
            .map(|i| {
                serde_json::json!({
                    "type": format!("{:?}", i.issue_type),
                    "severity": format!("{:?}", i.severity),
                    "description": i.description,
                })
            })
            .collect();

        let obj = serde_json::json!({
            "success": result.success,
            "original": result.original_path.to_string_lossy(),
            "repaired": result.repaired_path.to_string_lossy(),
            "backup": result.backup_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "issues_detected": result.issues_detected,
            "issues_fixed": result.issues_fixed,
            "duration_ms": result.duration.as_millis(),
            "fixed_issues": fixed_json,
            "unfixed_issues": unfixed_json,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    if crate::progress::is_quiet() {
        return Ok(());
    }

    println!("{}", "Media Repair".green().bold());
    println!("  Input:     {}", input.display().to_string().cyan());
    println!(
        "  Output:    {}",
        result.repaired_path.display().to_string().cyan()
    );
    if let Some(ref bak) = result.backup_path {
        println!("  Backup:    {}", bak.display());
    }
    println!("  Mode:      {}", mode_str);
    println!();

    if result.success {
        println!(
            "  {} Repair successful: {} of {} issues fixed",
            "✓".green().bold(),
            result.issues_fixed,
            result.issues_detected
        );
    } else {
        println!(
            "  {} Repair partial: {} of {} issues fixed",
            "!".yellow(),
            result.issues_fixed,
            result.issues_detected
        );
    }

    println!(
        "  {} Completed in {:.2}s",
        "⏱".blue(),
        result.duration.as_secs_f64()
    );

    if !result.unfixed_issues.is_empty() {
        println!();
        println!("  {} Remaining issues:", "!".yellow());
        for issue in &result.unfixed_issues {
            println!("    - [{:?}] {}", issue.issue_type, issue.description);
        }
    }

    Ok(())
}

fn cmd_batch(
    inputs: Vec<PathBuf>,
    output_dir: Option<PathBuf>,
    mode_str: &str,
    no_backup: bool,
    json_output: bool,
) -> Result<()> {
    use oximedia_repair::{RepairEngine, RepairOptions};

    let mode = parse_repair_mode(mode_str)?;

    if let Some(ref dir) = output_dir {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create output directory: {}", dir.display()))?;
    }

    let options = RepairOptions {
        mode,
        create_backup: !no_backup,
        verify_after_repair: true,
        output_dir: output_dir.clone(),
        verbose: !json_output,
        ..Default::default()
    };

    let engine = RepairEngine::new();
    let results = engine
        .repair_batch(&inputs, &options)
        .map_err(|e| anyhow::anyhow!("Batch repair failed: {}", e))?;

    if json_output {
        let results_json: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "original": r.original_path.to_string_lossy(),
                    "repaired": r.repaired_path.to_string_lossy(),
                    "success": r.success,
                    "issues_detected": r.issues_detected,
                    "issues_fixed": r.issues_fixed,
                    "duration_ms": r.duration.as_millis(),
                })
            })
            .collect();

        let total_fixed: usize = results.iter().map(|r| r.issues_fixed).sum();
        let obj = serde_json::json!({
            "total_files": inputs.len(),
            "processed": results.len(),
            "total_issues_fixed": total_fixed,
            "mode": mode_str,
            "results": results_json,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    if crate::progress::is_quiet() {
        return Ok(());
    }

    println!("{}", "Batch Media Repair".green().bold());
    println!("  Files:   {}", inputs.len());
    println!("  Mode:    {}", mode_str);
    if let Some(ref dir) = output_dir {
        println!("  Output:  {}", dir.display());
    }
    println!();

    let total_fixed: usize = results.iter().map(|r| r.issues_fixed).sum();
    let successes = results.iter().filter(|r| r.success).count();

    for result in &results {
        let status = if result.success {
            "✓".green().to_string()
        } else {
            "!".yellow().to_string()
        };
        println!(
            "  {} {} — {} fixed",
            status,
            result
                .original_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| result.original_path.to_string_lossy().into_owned()),
            result.issues_fixed
        );
    }

    println!();
    println!(
        "  {} {}/{} files repaired successfully, {} total issues fixed",
        "✓".green().bold(),
        successes,
        results.len(),
        total_fixed
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_repair_mode(s: &str) -> Result<oximedia_repair::RepairMode> {
    use oximedia_repair::RepairMode;
    match s.to_lowercase().as_str() {
        "safe" => Ok(RepairMode::Safe),
        "balanced" => Ok(RepairMode::Balanced),
        "aggressive" => Ok(RepairMode::Aggressive),
        "extract" => Ok(RepairMode::Extract),
        other => anyhow::bail!(
            "Unknown repair mode '{}'. Supported: safe, balanced, aggressive, extract",
            other
        ),
    }
}

fn parse_issue_types(s: &str) -> Result<Vec<oximedia_repair::IssueType>> {
    use oximedia_repair::IssueType;
    if s.is_empty() {
        return Ok(Vec::new());
    }

    s.split(',')
        .map(|part| {
            match part.trim().to_lowercase().as_str() {
                "header" | "corruptedheader" => Ok(IssueType::CorruptedHeader),
                "index" | "missingindex" => Ok(IssueType::MissingIndex),
                "timestamps" | "invalidtimestamps" => Ok(IssueType::InvalidTimestamps),
                "avsync" | "avdesync" => Ok(IssueType::AVDesync),
                "truncated" => Ok(IssueType::Truncated),
                "packets" | "corruptpackets" => Ok(IssueType::CorruptPackets),
                "metadata" | "corruptmetadata" => Ok(IssueType::CorruptMetadata),
                "keyframes" | "missingkeyframes" => Ok(IssueType::MissingKeyframes),
                "frameorder" | "invalidframeorder" => Ok(IssueType::InvalidFrameOrder),
                "conversion" | "conversionerror" => Ok(IssueType::ConversionError),
                other => anyhow::bail!(
                    "Unknown issue type '{}'. Supported: header, index, timestamps, avsync, truncated, packets, metadata, keyframes, frameorder, conversion",
                    other
                ),
            }
        })
        .collect()
}
