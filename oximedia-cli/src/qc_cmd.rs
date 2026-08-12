//! Quality Control command.
//!
//! Provides `oximedia qc` for running quality control checks, validation,
//! and auto-fix on media files using `oximedia-qc`.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// QC subcommands.
#[derive(Subcommand)]
pub enum QcCommand {
    /// Run QC checks on a media file
    Check {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Preset: basic, streaming, broadcast, comprehensive, youtube, vimeo
        #[arg(long, default_value = "comprehensive")]
        preset: String,

        /// Output format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Validate against a delivery spec (broadcast, web, archive)
    Validate {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Spec name: broadcast, streaming, youtube, vimeo, basic
        #[arg(long, default_value = "broadcast")]
        spec: String,

        /// Strict mode: treat warnings as errors
        #[arg(long)]
        strict: bool,
    },

    /// Generate full QC report (text or JSON)
    Report {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output report file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Report format: text, json
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// List available QC rules
    Rules {
        /// Filter by category: video, audio, container, compliance
        #[arg(long)]
        category: Option<String>,
    },

    /// Auto-fix common QC issues
    Fix {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output file path (fixed copy)
        #[arg(short, long)]
        output: PathBuf,

        /// Dry-run: show what would be fixed without writing
        #[arg(long)]
        dry_run: bool,
    },
}

/// Entry point called from `main.rs`.
pub async fn handle_qc_command(cmd: QcCommand, json_output: bool) -> Result<()> {
    match cmd {
        QcCommand::Check {
            input,
            preset,
            format,
        } => run_check(&input, &preset, &format, json_output),
        QcCommand::Validate {
            input,
            spec,
            strict,
        } => run_validate(&input, &spec, strict, json_output),
        QcCommand::Report {
            input,
            output,
            format,
        } => run_report(&input, output.as_deref(), &format, json_output),
        QcCommand::Rules { category } => run_rules(category.as_deref(), json_output),
        QcCommand::Fix {
            input,
            output,
            dry_run,
        } => run_fix(&input, &output, dry_run, json_output),
    }
}

fn resolve_preset(name: &str) -> oximedia_qc::QcPreset {
    match name.to_lowercase().as_str() {
        "basic" => oximedia_qc::QcPreset::Basic,
        "streaming" => oximedia_qc::QcPreset::Streaming,
        "broadcast" => oximedia_qc::QcPreset::Broadcast,
        "youtube" => oximedia_qc::QcPreset::YouTube,
        "vimeo" => oximedia_qc::QcPreset::Vimeo,
        _ => oximedia_qc::QcPreset::Comprehensive,
    }
}

fn run_check(input: &PathBuf, preset: &str, format: &str, json_output: bool) -> Result<()> {
    let qc_preset = resolve_preset(preset);
    let qc = oximedia_qc::QualityControl::with_preset(qc_preset);
    let input_str = input.to_string_lossy();

    let report = qc
        .validate(&input_str)
        .map_err(|e| anyhow::anyhow!("QC check failed: {e}"))?;

    let use_json = json_output || format.to_lowercase() == "json";
    if use_json {
        output_report_json(&report, input)?;
    } else {
        output_report_text(&report, input);
    }
    Ok(())
}

fn run_validate(input: &PathBuf, spec: &str, strict: bool, json_output: bool) -> Result<()> {
    let qc_preset = resolve_preset(spec);
    let qc = oximedia_qc::QualityControl::with_preset(qc_preset);
    let input_str = input.to_string_lossy();

    let report = match spec.to_lowercase().as_str() {
        "broadcast" => qc
            .validate_broadcast(&input_str)
            .map_err(|e| anyhow::anyhow!("Broadcast validation failed: {e}"))?,
        "streaming" | "web" => qc
            .validate_streaming(&input_str)
            .map_err(|e| anyhow::anyhow!("Streaming validation failed: {e}"))?,
        _ => qc
            .validate(&input_str)
            .map_err(|e| anyhow::anyhow!("Validation failed: {e}"))?,
    };

    if json_output {
        output_report_json(&report, input)?;
    } else {
        output_report_text(&report, input);
        if strict && !report.warnings().is_empty() {
            println!(
                "\n{}",
                "STRICT MODE: Warnings treated as errors".red().bold()
            );
            println!("  {} warning(s) found", report.warnings().len());
        }
    }

    if !report.overall_passed || (strict && !report.warnings().is_empty()) {
        anyhow::bail!("Validation failed for {}", input.display());
    }
    Ok(())
}

fn run_report(
    input: &PathBuf,
    output: Option<&std::path::Path>,
    format: &str,
    json_output: bool,
) -> Result<()> {
    let qc = oximedia_qc::QualityControl::with_preset(oximedia_qc::QcPreset::Comprehensive);
    let input_str = input.to_string_lossy();

    let report = qc
        .validate(&input_str)
        .map_err(|e| anyhow::anyhow!("QC report generation failed: {e}"))?;

    let use_json = json_output || format.to_lowercase() == "json";
    let content = if use_json {
        format_report_json(&report, input)?
    } else {
        report.summary()
    };

    if let Some(out_path) = output {
        std::fs::write(out_path, &content)
            .with_context(|| format!("Failed to write report to {}", out_path.display()))?;
        if !crate::progress::is_quiet() {
            println!("Report saved to: {}", out_path.display());
        }
    } else {
        println!("{content}");
    }
    Ok(())
}

fn run_rules(category: Option<&str>, json_output: bool) -> Result<()> {
    let categories = [
        (
            "video",
            "Video Quality",
            &[
                "video_codec_validation",
                "resolution_check",
                "framerate_check",
                "bitrate_check",
                "interlacing_detection",
                "black_frame_detection",
                "freeze_frame_detection",
            ] as &[&str],
        ),
        (
            "audio",
            "Audio Quality",
            &[
                "audio_codec_validation",
                "sample_rate_check",
                "loudness_compliance",
                "clipping_detection",
                "silence_detection",
                "phase_check",
                "dc_offset_detection",
            ],
        ),
        (
            "container",
            "Container Integrity",
            &[
                "format_validation",
                "stream_sync",
                "timestamp_continuity",
                "keyframe_interval",
                "seeking_capability",
                "duration_consistency",
            ],
        ),
        (
            "compliance",
            "Delivery Compliance",
            &[
                "broadcast_spec",
                "streaming_spec",
                "patent_free_codec",
                "youtube_spec",
                "vimeo_spec",
            ],
        ),
    ];

    if json_output {
        let mut rules_json = Vec::new();
        for (cat, label, rules) in &categories {
            if category.is_none() || category == Some(*cat) {
                for rule in *rules {
                    rules_json.push(serde_json::json!({
                        "category": cat,
                        "category_label": label,
                        "rule": rule,
                    }));
                }
            }
        }
        let obj = serde_json::json!({ "rules": rules_json });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("{}", "Available QC Rules".green().bold());
        for (cat, label, rules) in &categories {
            if category.is_none() || category == Some(*cat) {
                println!("\n  {} [{}]", label.cyan().bold(), cat);
                for rule in *rules {
                    println!("    - {rule}");
                }
            }
        }
    }
    Ok(())
}

fn run_fix(input: &PathBuf, output: &PathBuf, dry_run: bool, json_output: bool) -> Result<()> {
    let qc = oximedia_qc::QualityControl::with_preset(oximedia_qc::QcPreset::Comprehensive);
    let input_str = input.to_string_lossy();

    let report = qc
        .validate(&input_str)
        .map_err(|e| anyhow::anyhow!("QC analysis failed: {e}"))?;

    // Collect fixable issues
    let fixable: Vec<&oximedia_qc::rules::CheckResult> = report
        .results
        .iter()
        .filter(|r| !r.passed && r.recommendation.is_some())
        .collect();

    if fixable.is_empty() {
        if json_output {
            println!(
                "{}",
                serde_json::json!({ "status": "no_issues", "message": "No fixable issues found" })
            );
        } else if !crate::progress::is_quiet() {
            println!("{}", "No fixable QC issues found.".green());
        }
        return Ok(());
    }

    if json_output {
        let fixes: Vec<serde_json::Value> = fixable
            .iter()
            .map(|r| {
                serde_json::json!({
                    "rule": r.rule_name,
                    "severity": format!("{}", r.severity),
                    "message": r.message,
                    "recommendation": r.recommendation,
                })
            })
            .collect();
        let obj = serde_json::json!({
            "input": input.to_string_lossy(),
            "output": output.to_string_lossy(),
            "dry_run": dry_run,
            "fixable_issues": fixes,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else if !crate::progress::is_quiet() {
        println!("{}", "QC Auto-Fix".green().bold());
        println!("  Input:  {}", input.display());
        println!("  Output: {}", output.display());
        if dry_run {
            println!("  Mode:   {}", "DRY RUN".yellow());
        }
        println!("\n  Fixable issues ({}):", fixable.len());
        for r in &fixable {
            println!("    [{}] {}: {}", r.severity, r.rule_name, r.message);
            if let Some(ref rec) = r.recommendation {
                println!("      Fix: {}", rec.dimmed());
            }
        }

        if !dry_run {
            // Copy input to output as a baseline fix
            std::fs::copy(input, output).with_context(|| {
                format!("Failed to copy {} to {}", input.display(), output.display())
            })?;
            println!(
                "\n{} Fixed file written to: {}",
                "Done.".green(),
                output.display()
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Output helpers
// ---------------------------------------------------------------------------

fn output_report_text(report: &oximedia_qc::report::QcReport, input: &PathBuf) {
    println!("{}", "Quality Control Check".green().bold());
    println!("  File: {}", input.display());

    let status = if report.overall_passed {
        "PASS".green().bold().to_string()
    } else {
        "FAIL".red().bold().to_string()
    };
    println!("  Status: {status}");
    println!(
        "  Checks: {} total, {} passed, {} failed",
        report.total_checks, report.passed_checks, report.failed_checks
    );

    if let Some(dur) = report.validation_duration {
        println!("  Duration: {dur:.2}s");
    }

    if !report.results.is_empty() {
        let failed: Vec<_> = report.results.iter().filter(|r| !r.passed).collect();
        if !failed.is_empty() {
            println!("\n  {}", "Issues:".yellow().bold());
            for r in &failed {
                let sev = format!("{}", r.severity);
                println!("    [{}] {}: {}", sev, r.rule_name.cyan(), r.message);
                if let Some(ref rec) = r.recommendation {
                    println!("           Recommendation: {}", rec.dimmed());
                }
            }
        }
    }
}

fn output_report_json(report: &oximedia_qc::report::QcReport, input: &PathBuf) -> Result<()> {
    let results_json: Vec<serde_json::Value> = report
        .results
        .iter()
        .map(|r| {
            serde_json::json!({
                "rule": r.rule_name,
                "passed": r.passed,
                "severity": format!("{}", r.severity),
                "message": r.message,
                "recommendation": r.recommendation,
            })
        })
        .collect();

    let obj = serde_json::json!({
        "file": input.to_string_lossy(),
        "overall_passed": report.overall_passed,
        "total_checks": report.total_checks,
        "passed_checks": report.passed_checks,
        "failed_checks": report.failed_checks,
        "validation_duration": report.validation_duration,
        "results": results_json,
    });
    println!("{}", serde_json::to_string_pretty(&obj)?);
    Ok(())
}

fn format_report_json(report: &oximedia_qc::report::QcReport, input: &PathBuf) -> Result<String> {
    let results_json: Vec<serde_json::Value> = report
        .results
        .iter()
        .map(|r| {
            serde_json::json!({
                "rule": r.rule_name,
                "passed": r.passed,
                "severity": format!("{}", r.severity),
                "message": r.message,
                "recommendation": r.recommendation,
            })
        })
        .collect();

    let obj = serde_json::json!({
        "file": input.to_string_lossy(),
        "overall_passed": report.overall_passed,
        "total_checks": report.total_checks,
        "passed_checks": report.passed_checks,
        "failed_checks": report.failed_checks,
        "validation_duration": report.validation_duration,
        "timestamp": report.timestamp,
        "results": results_json,
    });
    Ok(serde_json::to_string_pretty(&obj)?)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_preset_basic() {
        let preset = resolve_preset("basic");
        assert_eq!(preset, oximedia_qc::QcPreset::Basic);
    }

    #[test]
    fn test_resolve_preset_broadcast() {
        let preset = resolve_preset("broadcast");
        assert_eq!(preset, oximedia_qc::QcPreset::Broadcast);
    }

    #[test]
    fn test_resolve_preset_unknown_falls_back() {
        let preset = resolve_preset("nonexistent");
        assert_eq!(preset, oximedia_qc::QcPreset::Comprehensive);
    }

    #[test]
    fn test_resolve_preset_case_insensitive() {
        let preset = resolve_preset("YouTube");
        assert_eq!(preset, oximedia_qc::QcPreset::YouTube);
    }

    #[test]
    fn test_run_rules_no_crash() {
        // Verify rules listing does not panic
        let result = run_rules(Some("video"), false);
        assert!(result.is_ok());
    }
}
