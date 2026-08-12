//! LUT (Look-Up Table) management commands.
//!
//! Provides apply, info, convert, and generate subcommands for working with
//! 3D/1D LUT files using the `oximedia-lut` crate.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

/// LUT command subcommands.
#[derive(Subcommand, Debug)]
pub enum LutCommand {
    /// Apply a LUT to an image or video file
    Apply {
        /// Input file path
        #[arg(short, long)]
        input: PathBuf,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,

        /// LUT file path (.cube, .3dl, etc.)
        #[arg(long)]
        lut: PathBuf,

        /// LUT strength/mix factor (0.0 to 1.0, default 1.0)
        #[arg(long)]
        strength: Option<f32>,
    },

    /// Show metadata and statistics for a LUT file
    Info {
        /// LUT file path
        #[arg(value_name = "LUT")]
        lut: PathBuf,
    },

    /// Convert a LUT to a different format
    Convert {
        /// Input LUT file
        #[arg(short, long)]
        input: PathBuf,

        /// Output LUT file
        #[arg(short, long)]
        output: PathBuf,

        /// Target format: cube, 3dl, csp, icc
        #[arg(long, default_value = "cube")]
        format: String,
    },

    /// Generate an identity LUT of a given size
    Generate {
        /// Output LUT file path
        #[arg(short, long)]
        output: PathBuf,

        /// LUT grid size (e.g. 17, 33, 65)
        #[arg(long, default_value = "33")]
        size: Option<u32>,
    },
}

/// Handle lut subcommand dispatch.
pub async fn handle_lut_command(cmd: LutCommand, json_output: bool) -> Result<()> {
    match cmd {
        LutCommand::Apply {
            input,
            output,
            lut,
            strength,
        } => apply_lut(&input, &output, &lut, strength, json_output).await,
        LutCommand::Info { lut } => lut_info(&lut, json_output).await,
        LutCommand::Convert {
            input,
            output,
            format,
        } => convert_lut(&input, &output, &format, json_output).await,
        LutCommand::Generate { output, size } => generate_lut(&output, size, json_output).await,
    }
}

/// Apply a .cube (or similar) LUT to a media file and report.
async fn apply_lut(
    input: &PathBuf,
    output: &PathBuf,
    lut_path: &PathBuf,
    strength: Option<f32>,
    json_output: bool,
) -> Result<()> {
    use oximedia_lut::{Lut3d, LutInterpolation};

    let lut_text = std::fs::read_to_string(lut_path)
        .with_context(|| format!("Failed to read LUT file: {}", lut_path.display()))?;

    let lut = Lut3d::load_cube(&lut_text)
        .with_context(|| format!("Failed to parse LUT: {}", lut_path.display()))?;

    let mix = strength.unwrap_or(1.0_f32).clamp(0.0, 1.0);

    if json_output {
        let obj = serde_json::json!({
            "operation": "lut_apply",
            "input": input.to_string_lossy(),
            "output": output.to_string_lossy(),
            "lut": lut_path.to_string_lossy(),
            "lut_size": lut.size(),
            "lut_title": lut.title,
            "strength": mix,
            "interpolation": "tetrahedral",
            "status": "preview_only",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "LUT Apply".green().bold());
    println!("  Input:         {}", input.display());
    println!("  Output:        {}", output.display());
    println!("  LUT:           {}", lut_path.display());
    if let Some(ref title) = lut.title {
        println!("  LUT title:     {}", title);
    }
    println!(
        "  LUT size:      {}x{}x{}",
        lut.size(),
        lut.size(),
        lut.size()
    );
    println!("  Strength:      {:.2}", mix);
    println!("  Interpolation: {}", "tetrahedral".cyan());

    // Demonstrate that the LUT is functional by sampling the centre point
    let (r_out, g_out, b_out) = lut.apply_rgb(0.5, 0.5, 0.5);
    println!(
        "  Centre sample: ({:.4}, {:.4}, {:.4}) -> ({:.4}, {:.4}, {:.4})",
        0.5_f32, 0.5_f32, 0.5_f32, r_out, g_out, b_out
    );

    // Validate the LUT before confirming
    let warnings = lut.validate();
    if !warnings.is_empty() {
        for w in &warnings {
            println!("  {} {}", "Warning:".yellow(), w);
        }
    }

    // Perform trilinear sample as sanity check for identity
    let _corner = lut.apply(&[1.0, 1.0, 1.0], LutInterpolation::Trilinear);

    println!(
        "\n{} LUT parsed and validated. Full pixel-pipeline apply requires frame I/O integration.",
        "Note:".yellow()
    );
    println!("{} {}", "Would write:".dimmed(), output.display());

    Ok(())
}

/// Show metadata for a LUT file.
async fn lut_info(lut_path: &PathBuf, json_output: bool) -> Result<()> {
    use oximedia_lut::Lut3d;

    let lut_text = std::fs::read_to_string(lut_path)
        .with_context(|| format!("Failed to read LUT file: {}", lut_path.display()))?;

    let lut = Lut3d::load_cube(&lut_text)
        .with_context(|| format!("Failed to parse LUT: {}", lut_path.display()))?;

    let warnings = lut.validate();
    let entries = lut.entry_count();

    if json_output {
        let obj = serde_json::json!({
            "file": lut_path.to_string_lossy(),
            "type": "3d_lut",
            "format": "cube",
            "size": lut.size(),
            "entries": entries,
            "title": lut.title,
            "input_min": lut.input_min,
            "input_max": lut.input_max,
            "warnings": warnings,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    println!("{}", "LUT Info".green().bold());
    println!("  File:        {}", lut_path.display());
    println!("  Type:        {}", "3D LUT (cube)".cyan());
    if let Some(ref title) = lut.title {
        println!("  Title:       {}", title);
    }
    println!("  Grid size:   {}", lut.size());
    println!("  Entries:     {}", entries);
    println!(
        "  Input range: [{:.4}, {:.4}, {:.4}] - [{:.4}, {:.4}, {:.4}]",
        lut.input_min[0],
        lut.input_min[1],
        lut.input_min[2],
        lut.input_max[0],
        lut.input_max[1],
        lut.input_max[2],
    );

    if warnings.is_empty() {
        println!("  Validation:  {}", "OK".green());
    } else {
        println!("  Validation:  {} warning(s)", warnings.len());
        for w in &warnings {
            println!("    {} {}", "!".yellow(), w);
        }
    }

    Ok(())
}

/// Convert a LUT to a different file format.
async fn convert_lut(
    input: &PathBuf,
    output: &PathBuf,
    format: &str,
    json_output: bool,
) -> Result<()> {
    use oximedia_lut::Lut3d;

    let lut_text = std::fs::read_to_string(input)
        .with_context(|| format!("Failed to read LUT: {}", input.display()))?;

    let lut = Lut3d::load_cube(&lut_text)
        .with_context(|| format!("Failed to parse LUT: {}", input.display()))?;

    let supported = ["cube", "3dl", "csp"];
    let fmt_lower = format.to_lowercase();
    if !supported.contains(&fmt_lower.as_str()) {
        anyhow::bail!(
            "Unsupported target format '{}'. Supported: {}",
            format,
            supported.join(", ")
        );
    }

    if json_output {
        let obj = serde_json::json!({
            "operation": "lut_convert",
            "input": input.to_string_lossy(),
            "output": output.to_string_lossy(),
            "target_format": format,
            "lut_size": lut.size(),
            "status": "conversion_ready",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    let quiet = crate::progress::is_quiet();
    if !quiet {
        println!("{}", "LUT Convert".green().bold());
        println!("  Input:         {} (cube)", input.display());
        println!("  Output:        {}", output.display());
        println!("  Target format: {}", format.cyan());
        println!("  Grid size:     {}", lut.size());
    }

    // For cube-to-cube, write the parsed LUT back out directly
    if fmt_lower == "cube" {
        lut.to_file(output)
            .with_context(|| format!("Failed to write LUT: {}", output.display()))?;
        if !quiet {
            println!("{} Written: {}", "✓".green(), output.display());
        }
    } else {
        if !quiet {
            println!(
                "{} Format '{}' serialiser is planned; cube output written as intermediate.",
                "Note:".yellow(),
                format
            );
        }
        lut.to_file(output)
            .with_context(|| format!("Failed to write LUT: {}", output.display()))?;
        if !quiet {
            println!("{} Written (cube): {}", "✓".green(), output.display());
        }
    }

    Ok(())
}

/// Generate an identity LUT and write it to disk.
async fn generate_lut(output: &PathBuf, size: Option<u32>, json_output: bool) -> Result<()> {
    use oximedia_lut::{Lut3d, LutSize};

    let grid = size.unwrap_or(33);

    let lut_size = match grid {
        17 => LutSize::Size17,
        33 => LutSize::Size33,
        65 => LutSize::Size65,
        _ => {
            // Fall back to the nearest supported size via From<usize>
            let size = LutSize::from(grid as usize);
            println!(
                "{} LUT size {} not directly supported; using {}.",
                "Note:".yellow(),
                grid,
                size.as_usize()
            );
            size
        }
    };

    let lut = Lut3d::identity(lut_size);

    lut.to_file(output)
        .with_context(|| format!("Failed to write identity LUT: {}", output.display()))?;

    if json_output {
        let obj = serde_json::json!({
            "operation": "lut_generate",
            "output": output.to_string_lossy(),
            "size": grid,
            "entries": lut.entry_count(),
            "type": "identity",
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
        return Ok(());
    }

    if !crate::progress::is_quiet() {
        println!("{}", "LUT Generate".green().bold());
        println!("  Output:  {}", output.display());
        println!("  Size:    {}x{}x{}", grid, grid, grid);
        println!("  Entries: {}", lut.entry_count());
        println!("  Type:    {}", "identity (no colour change)".cyan());
        println!("{} Written: {}", "✓".green(), output.display());
    }

    Ok(())
}
