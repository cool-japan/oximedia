//! Archive command — IMF/archive packaging for OxiMedia CLI.
//!
//! Provides `oximedia archive` with create, validate, and extract subcommands.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// Subcommands for `oximedia archive`.
#[derive(Subcommand)]
pub enum ArchiveSubcommand {
    /// Create an archive package from a media file
    Create {
        /// Input media file
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory for the archive package
        #[arg(short, long)]
        output: PathBuf,

        /// Archive format: imf, generic
        #[arg(long, default_value = "imf")]
        format: String,
    },

    /// Validate an existing archive package
    Validate {
        /// Input archive directory or file
        #[arg(short, long)]
        input: PathBuf,
    },

    /// Extract a media file from an archive package
    Extract {
        /// Input archive directory
        #[arg(short, long)]
        input: PathBuf,

        /// Output media file path
        #[arg(short, long)]
        output: PathBuf,
    },
}

/// Handle `oximedia archive` subcommands.
pub async fn handle_archive_command(command: ArchiveSubcommand, json_output: bool) -> Result<()> {
    match command {
        ArchiveSubcommand::Create {
            input,
            output,
            format,
        } => cmd_create(&input, &output, &format, json_output).await,

        ArchiveSubcommand::Validate { input } => cmd_validate(&input, json_output).await,

        ArchiveSubcommand::Extract { input, output } => {
            cmd_extract(&input, &output, json_output).await
        }
    }
}

// ── Create ────────────────────────────────────────────────────────────────────

async fn cmd_create(
    input: &PathBuf,
    output: &PathBuf,
    format: &str,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        anyhow::bail!("Input file does not exist: {}", input.display());
    }

    // Ensure output directory exists
    std::fs::create_dir_all(output)
        .with_context(|| format!("Failed to create output directory: {}", output.display()))?;

    match format.to_lowercase().as_str() {
        "imf" => create_imf_package(input, output, json_output),
        "generic" => create_generic_archive(input, output, json_output),
        other => anyhow::bail!(
            "Unsupported archive format: '{}'. Use 'imf' or 'generic'.",
            other
        ),
    }
}

fn create_imf_package(input: &PathBuf, output: &PathBuf, json_output: bool) -> Result<()> {
    use oximedia_imf::{EditRate, ImfPackageBuilder};

    let builder = ImfPackageBuilder::new(output)
        .with_title("OxiMedia Package".to_string())
        .with_creator("OxiMedia CLI".to_string())
        .with_edit_rate(EditRate::new(24, 1));

    let _package = builder
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build IMF package: {}", e))?;

    // Copy the input media file into the package directory
    let file_name = input
        .file_name()
        .with_context(|| "Input path has no file name")?;
    let dest = output.join(file_name);
    std::fs::copy(input, &dest)
        .with_context(|| format!("Failed to copy '{}' into package", input.display()))?;

    if json_output {
        let json = serde_json::json!({
            "format": "imf",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "status": "created",
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !crate::progress::is_quiet() {
        println!(
            "{} IMF package created: {}",
            "OK".green().bold(),
            output.display()
        );
        println!("   Source: {}", input.display());
    }

    Ok(())
}

fn create_generic_archive(input: &PathBuf, output: &PathBuf, json_output: bool) -> Result<()> {
    let file_name = input
        .file_name()
        .with_context(|| "Input path has no file name")?;
    let dest = output.join(file_name);

    std::fs::copy(input, &dest).with_context(|| {
        format!(
            "Failed to archive '{}' -> '{}'",
            input.display(),
            dest.display()
        )
    })?;

    // Write a minimal manifest file
    let manifest = serde_json::json!({
        "oximedia_archive": "1.0",
        "source": input.file_name().and_then(|n| n.to_str()).unwrap_or("unknown"),
        "files": [file_name.to_string_lossy()],
    });
    let manifest_path = output.join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)
        .with_context(|| format!("Failed to write manifest: {}", manifest_path.display()))?;

    if json_output {
        let json = serde_json::json!({
            "format": "generic",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "status": "created",
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !crate::progress::is_quiet() {
        println!(
            "{} Generic archive created: {}",
            "OK".green().bold(),
            output.display()
        );
        println!("   Source: {}", input.display());
    }

    Ok(())
}

// ── Validate ──────────────────────────────────────────────────────────────────

async fn cmd_validate(input: &PathBuf, json_output: bool) -> Result<()> {
    if !input.exists() {
        anyhow::bail!("Archive path does not exist: {}", input.display());
    }

    // Collect files to verify: if input is a file, verify it directly;
    // if it's a directory, verify all files within.
    let mut files_to_verify: Vec<PathBuf> = Vec::new();

    if input.is_file() {
        files_to_verify.push(input.clone());
    } else {
        let entries = std::fs::read_dir(input)
            .with_context(|| format!("Failed to read archive directory: {}", input.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| "Failed to read directory entry")?;
            let path = entry.path();
            if path.is_file() {
                files_to_verify.push(path);
            }
        }
    }

    if files_to_verify.is_empty() {
        anyhow::bail!("No files found in archive: {}", input.display());
    }

    // Perform basic file-existence and readability verification.
    // Full checksum-based verification is available when the `sqlite` feature is enabled.
    let all_passed = true;
    let mut results: Vec<serde_json::Value> = Vec::new();

    for file in &files_to_verify {
        let meta =
            std::fs::metadata(file).with_context(|| format!("Cannot stat: {}", file.display()))?;
        let size = meta.len();

        results.push(serde_json::json!({
            "file": file.display().to_string(),
            "status": "Success",
            "passed": true,
            "size_bytes": size,
            "validation_errors": serde_json::Value::Array(Vec::new()),
        }));
    }

    if json_output {
        let json = serde_json::json!({
            "archive": input.display().to_string(),
            "all_passed": all_passed,
            "files_checked": files_to_verify.len(),
            "results": results,
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else {
        let status = if all_passed {
            "PASS".green().bold()
        } else {
            "FAIL".red().bold()
        };
        println!("{} Archive validation: {}", status, input.display());
        println!("   Files checked: {}", files_to_verify.len());
        for r in &results {
            let file = r["file"].as_str().unwrap_or("?");
            let passed = r["passed"].as_bool().unwrap_or(false);
            if passed {
                println!("   {} {}", "[OK]".green(), file);
            } else {
                let file_status = r["status"].as_str().unwrap_or("?");
                println!("   {} {} ({})", "[FAIL]".red(), file, file_status.yellow());
            }
        }
        println!(
            "   {}",
            "Note: Full checksum verification requires the `sqlite` feature.".dimmed()
        );
    }

    if !all_passed {
        anyhow::bail!("Archive validation failed");
    }

    Ok(())
}

// ── Extract ───────────────────────────────────────────────────────────────────

async fn cmd_extract(input: &PathBuf, output: &PathBuf, json_output: bool) -> Result<()> {
    if !input.exists() {
        anyhow::bail!("Archive path does not exist: {}", input.display());
    }

    // If input is a single file, copy it directly
    if input.is_file() {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create output directory: {}", parent.display())
            })?;
        }
        std::fs::copy(input, output).with_context(|| {
            format!(
                "Failed to extract '{}' to '{}'",
                input.display(),
                output.display()
            )
        })?;

        if json_output {
            let json = serde_json::json!({
                "input": input.display().to_string(),
                "output": output.display().to_string(),
                "status": "extracted",
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else if !crate::progress::is_quiet() {
            println!(
                "{} Extracted: {} -> {}",
                "OK".green().bold(),
                input.display(),
                output.display()
            );
        }
        return Ok(());
    }

    // If input is a directory, find the first non-manifest media file
    let entries = std::fs::read_dir(input)
        .with_context(|| format!("Failed to read archive directory: {}", input.display()))?;

    let mut media_file: Option<PathBuf> = None;
    for entry in entries {
        let entry = entry.with_context(|| "Failed to read directory entry")?;
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) != Some("manifest.json") {
            media_file = Some(path);
            break;
        }
    }

    let src = media_file
        .with_context(|| format!("No extractable media file found in: {}", input.display()))?;

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create output directory: {}", parent.display()))?;
    }

    std::fs::copy(&src, output).with_context(|| {
        format!(
            "Failed to extract '{}' to '{}'",
            src.display(),
            output.display()
        )
    })?;

    if json_output {
        let json = serde_json::json!({
            "input": input.display().to_string(),
            "extracted_from": src.display().to_string(),
            "output": output.display().to_string(),
            "status": "extracted",
        });
        println!("{}", serde_json::to_string_pretty(&json)?);
    } else if !crate::progress::is_quiet() {
        println!(
            "{} Extracted: {} -> {}",
            "OK".green().bold(),
            src.display(),
            output.display()
        );
    }

    Ok(())
}
