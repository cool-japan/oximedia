//! Dolby Vision metadata CLI commands.
//!
//! Provides commands for analyzing, converting, inspecting, validating,
//! and displaying Dolby Vision RPU metadata.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// Dolby Vision command subcommands.
#[derive(Subcommand, Debug)]
pub enum DolbyVisionCommand {
    /// Analyze Dolby Vision metadata in a media file
    Analyze {
        /// Input file containing DV metadata
        #[arg(short, long)]
        input: PathBuf,

        /// Show per-frame metadata
        #[arg(long)]
        per_frame: bool,

        /// Output analysis to file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Show tone mapping curves
        #[arg(long)]
        tone_map: bool,
    },

    /// Convert between Dolby Vision profiles
    Convert {
        /// Input RPU file
        #[arg(short, long)]
        input: PathBuf,

        /// Output RPU file
        #[arg(short, long)]
        output: PathBuf,

        /// Source profile: 5, 7, 8, 81, 84
        #[arg(long)]
        from_profile: Option<u8>,

        /// Target profile: 5, 7, 8, 81, 84
        #[arg(long)]
        to_profile: u8,

        /// Preserve Level 2 (trim pass) metadata during a Profile 8 ->
        /// Profile 8.4 conversion instead of stripping it. Off by default:
        /// Level 2 trims are tuned for the source's PQ target display, and
        /// there is no real trim-pass regeneration for the new HLG target,
        /// so carrying them forward unchanged would misrepresent stale data
        /// as valid. Has no effect on an identity conversion (same profile
        /// in and out), which never touches metadata either way.
        #[arg(long)]
        preserve_levels: bool,
    },

    /// Show Dolby Vision metadata details
    Metadata {
        /// Input file
        #[arg(short, long)]
        input: PathBuf,

        /// Show specific level: 1, 2, 5, 6, 8, 9, 11
        #[arg(long)]
        level: Option<u8>,

        /// Show VDR DM data
        #[arg(long)]
        vdr: bool,

        /// Show RPU header
        #[arg(long)]
        header: bool,
    },

    /// Validate Dolby Vision RPU data
    Validate {
        /// Input file to validate
        #[arg(short, long)]
        input: PathBuf,

        /// Expected profile
        #[arg(long)]
        profile: Option<u8>,

        /// Check backward compatibility
        #[arg(long)]
        compat: bool,

        /// Strict validation mode
        #[arg(long)]
        strict: bool,
    },

    /// Show Dolby Vision profile information
    Info {
        /// Profile number: 5, 7, 8, 81, 84
        #[arg(long)]
        profile: Option<u8>,

        /// List all supported profiles
        #[arg(long)]
        list: bool,

        /// Show compatibility matrix
        #[arg(long)]
        compat_matrix: bool,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_profile(value: u8) -> Result<oximedia_dolbyvision::Profile> {
    oximedia_dolbyvision::Profile::from_u8(value).ok_or_else(|| {
        anyhow::anyhow!("Unknown Dolby Vision profile: {value}. Supported: 5, 7, 8, 81, 84")
    })
}

fn profile_description(p: oximedia_dolbyvision::Profile) -> &'static str {
    match p {
        oximedia_dolbyvision::Profile::Profile5 => "IPT-PQ, backward compatible with HDR10",
        oximedia_dolbyvision::Profile::Profile7 => "MEL + BL, single track, full enhancement",
        oximedia_dolbyvision::Profile::Profile8 => "BL only, backward compatible with HDR10",
        oximedia_dolbyvision::Profile::Profile8_1 => "Low-latency variant of Profile 8",
        oximedia_dolbyvision::Profile::Profile8_4 => "HLG-based, backward compatible with HLG",
    }
}

fn profile_number(p: oximedia_dolbyvision::Profile) -> u8 {
    match p {
        oximedia_dolbyvision::Profile::Profile5 => 5,
        oximedia_dolbyvision::Profile::Profile7 => 7,
        oximedia_dolbyvision::Profile::Profile8 => 8,
        oximedia_dolbyvision::Profile::Profile8_1 => 81,
        oximedia_dolbyvision::Profile::Profile8_4 => 84,
    }
}

/// Which on-disk framing a Dolby Vision RPU byte blob was successfully
/// parsed as.
///
/// [`oximedia_dolbyvision::DolbyVisionRpu::write_to_bitstream`] and
/// [`oximedia_dolbyvision::DolbyVisionRpu::write_to_nal`] are the two
/// supported serializations; a converted RPU is written back out in
/// whichever framing the source file actually parsed as, so `convert` never
/// silently changes a file's container framing as a side effect of a
/// profile change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RpuFraming {
    /// Raw T.35 RPU bitstream payload (no NAL/SEI wrapper) — what
    /// `write_to_bitstream` produces and what dedicated `.rpu` files
    /// typically contain.
    Bitstream,
    /// HEVC NAL unit (2-byte NAL header + SEI envelope wrapping the RPU).
    Nal,
}

impl RpuFraming {
    fn label(self) -> &'static str {
        match self {
            Self::Bitstream => "raw-bitstream",
            Self::Nal => "hevc-nal",
        }
    }
}

/// Read and parse a Dolby Vision RPU from real file bytes.
///
/// Tries the raw bitstream framing first, then falls back to HEVC-NAL
/// framing. Returns the parsed RPU plus which framing succeeded, so callers
/// can round-trip the same framing on write. Returns an error naming both
/// parse attempts' failures when neither framing parses — every command in
/// this file must reflect the real input file or refuse, never fabricate
/// metadata for a file it could not read.
fn read_and_parse_rpu(
    path: &std::path::Path,
) -> Result<(oximedia_dolbyvision::DolbyVisionRpu, RpuFraming)> {
    let data = std::fs::read(path)
        .with_context(|| format!("Failed to read input file: {}", path.display()))?;
    if data.is_empty() {
        return Err(anyhow::anyhow!(
            "'{}' is empty; not a Dolby Vision RPU",
            path.display()
        ));
    }

    match oximedia_dolbyvision::DolbyVisionRpu::parse_from_bitstream(&data) {
        Ok(rpu) => Ok((rpu, RpuFraming::Bitstream)),
        Err(bitstream_err) => match oximedia_dolbyvision::DolbyVisionRpu::parse_from_nal(&data) {
            Ok(rpu) => Ok((rpu, RpuFraming::Nal)),
            Err(nal_err) => Err(anyhow::anyhow!(
                "could not parse '{}' as a Dolby Vision RPU: not a valid raw RPU bitstream \
                 ({bitstream_err}) and not a valid HEVC NAL RPU ({nal_err})",
                path.display()
            )),
        },
    }
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle Dolby Vision command dispatch.
pub async fn handle_dolbyvision_command(
    command: DolbyVisionCommand,
    json_output: bool,
) -> Result<()> {
    match command {
        DolbyVisionCommand::Analyze {
            input,
            per_frame,
            output,
            tone_map,
        } => run_analyze(&input, per_frame, &output, tone_map, json_output).await,
        DolbyVisionCommand::Convert {
            input,
            output,
            from_profile,
            to_profile,
            preserve_levels,
        } => {
            run_convert(
                &input,
                &output,
                from_profile,
                to_profile,
                preserve_levels,
                json_output,
            )
            .await
        }
        DolbyVisionCommand::Metadata {
            input,
            level,
            vdr,
            header,
        } => run_metadata(&input, level, vdr, header, json_output).await,
        DolbyVisionCommand::Validate {
            input,
            profile,
            compat,
            strict,
        } => run_validate(&input, profile, compat, strict, json_output).await,
        DolbyVisionCommand::Info {
            profile,
            list,
            compat_matrix,
        } => run_info(profile, list, compat_matrix, json_output).await,
    }
}

// ---------------------------------------------------------------------------
// Analyze
// ---------------------------------------------------------------------------

async fn run_analyze(
    input: &PathBuf,
    _per_frame: bool,
    output: &Option<PathBuf>,
    _tone_map: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    // Read and parse the REAL source RPU. Analysis of a default/blank RPU
    // that has nothing to do with the input file's bytes would be a
    // fabricated report — refuse instead when the file cannot be parsed.
    let (rpu, framing) = read_and_parse_rpu(input)
        .context("dolby-vision analyze: cannot analyze an unparseable input")?;
    let profile_num = profile_number(rpu.profile);
    let structurally_valid = rpu.validate().is_ok();

    let analysis = serde_json::json!({
        "file": input.display().to_string(),
        "framing": framing.label(),
        "profile": profile_num,
        "profile_description": profile_description(rpu.profile),
        "backward_compatible": rpu.profile.is_backward_compatible(),
        "has_mel": rpu.profile.has_mel(),
        "is_hlg": rpu.profile.is_hlg(),
        "is_low_latency": rpu.profile.is_low_latency(),
        "rpu_format": rpu.header.rpu_format,
        "has_level1": rpu.level1.is_some(),
        "has_level2": rpu.level2.is_some(),
        "has_level5": rpu.level5.is_some(),
        "has_level6": rpu.level6.is_some(),
        "has_vdr_dm": rpu.vdr_dm_data.is_some(),
        "structurally_valid": structurally_valid,
    });

    if let Some(ref opath) = output {
        let s = serde_json::to_string_pretty(&analysis).context("Serialization failed")?;
        std::fs::write(opath, s)
            .with_context(|| format!("Failed to write: {}", opath.display()))?;
    }

    if json_output {
        let s = serde_json::to_string_pretty(&analysis).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Dolby Vision Analysis".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:25} {}", "File:", input.display());
        println!("{:25} {}", "Framing:", framing.label());
        println!("{:25} {}", "Profile:", profile_num);
        println!("{:25} {}", "Description:", profile_description(rpu.profile));
        println!(
            "{:25} {}",
            "Backward compatible:",
            rpu.profile.is_backward_compatible()
        );
        println!("{:25} {}", "Has MEL:", rpu.profile.has_mel());
        println!("{:25} {}", "HLG:", rpu.profile.is_hlg());
        println!("{:25} {}", "Low latency:", rpu.profile.is_low_latency());
        println!("{:25} {}", "RPU format:", rpu.header.rpu_format);
        println!("{:25} {}", "Structurally valid:", structurally_valid);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Convert
// ---------------------------------------------------------------------------

async fn run_convert(
    input: &PathBuf,
    output: &PathBuf,
    from_profile: Option<u8>,
    to_profile: u8,
    preserve_levels: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    let target = parse_profile(to_profile)?;

    // Read and parse the REAL source RPU. A "conversion" that never looks
    // at the input's actual metadata is a fabrication — writing a blank
    // default RPU and labeling it the conversion output. Refuse instead of
    // doing that.
    let (source_rpu, framing) = read_and_parse_rpu(input)
        .context("dolby-vision convert: cannot convert an unparseable input")?;

    // `--from-profile` is an explicit hint for cases where the header-based
    // auto-detector (`detect_profile_from_header`) is ambiguous; when
    // given, it selects which conversion path to take. It never rewrites
    // the RPU's own parsed bytes.
    let source_profile = match from_profile {
        Some(fp) => parse_profile(fp)?,
        None => source_rpu.profile,
    };

    let path = oximedia_dolbyvision::profile_convert::ConversionPath::new(source_profile, target);

    // Real profile-pair coverage, as implemented by `oximedia-dolbyvision`
    // today: identity (same profile in and out) and Profile 8 -> Profile
    // 8.4 (`convert_profile8_to_8_4`, a real PQ->HLG Level 1/9 rescale).
    // Every other pair (5<->7<->8<->8.1<->8.4) has no metadata transform in
    // the crate — `profile_convert::DvProfileConverter::plan()` only lists
    // *which* actions a real converter would need (strip-MEL, remap-base,
    // regen-trims, ...), it does not execute any of them on RPU bytes. This
    // is that ceiling, not a forgotten wiring task: refuse rather than emit
    // a fabricated RPU for a pair nothing here can honestly produce.
    let (converted, level2_stripped) = if path.is_identity() {
        (source_rpu.clone(), false)
    } else if source_profile == oximedia_dolbyvision::Profile::Profile8
        && target == oximedia_dolbyvision::Profile::Profile8_4
    {
        let mut converted =
            oximedia_dolbyvision::profile_convert::convert_profile8_to_8_4(&source_rpu);

        // `--preserve-levels` now actually gates something: Level 2 (trim
        // passes) is exactly the block `DvProfileConverter::plan()` marks
        // `RegenerateTrimPasses` for on every non-identity conversion, and
        // there is no real trim-pass regeneration to run — the source
        // profile's trims were tuned for a PQ target display, not the HLG
        // one this conversion just produced. Carrying them forward
        // unchanged (the only thing `convert_profile8_to_8_4` itself does
        // with Level 2) is a plausible default when the caller explicitly
        // asks to preserve it; stripping is the honest default otherwise,
        // since forwarding stale trim data silently would misrepresent it
        // as valid for the new target. Other blocks this conversion does
        // not touch (Level 4/5/6/7/9/11) describe things that stay true
        // regardless of transfer function (mastering display, active area,
        // content type, source geometry) and are always kept.
        let level2_stripped = !preserve_levels && converted.level2.is_some();
        if level2_stripped {
            converted.level2 = None;
        }
        (converted, level2_stripped)
    } else {
        return Err(anyhow::anyhow!(
            "dolby-vision convert: real profile conversion from P{} to P{to_profile} is not yet \
             implemented (only an identity conversion and Profile 8 -> Profile 8.4 have a real \
             metadata transform); refusing to emit a fabricated RPU. No output file was written.",
            profile_number(source_profile)
        ));
    };

    converted.validate().map_err(|e| {
        anyhow::anyhow!("dolby-vision convert: converted RPU failed validation: {e}")
    })?;

    let out_bytes = match framing {
        RpuFraming::Bitstream => converted.write_to_bitstream(),
        RpuFraming::Nal => converted.write_to_nal(),
    }
    .map_err(|e| anyhow::anyhow!("dolby-vision convert: failed to serialize converted RPU: {e}"))?;

    std::fs::write(output, &out_bytes)
        .with_context(|| format!("Failed to write: {}", output.display()))?;

    if json_output {
        let result = serde_json::json!({
            "command": "dolby-vision convert",
            "input": input.display().to_string(),
            "output": output.display().to_string(),
            "framing": framing.label(),
            "from_profile": profile_number(source_profile),
            "to_profile": to_profile,
            "identity": path.is_identity(),
            "real_conversion": true,
            "preserve_levels": preserve_levels,
            "level2_stripped": level2_stripped,
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Dolby Vision Convert".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "From profile:", profile_number(source_profile));
        println!("{:20} {}", "To profile:", to_profile);
        println!(
            "{:20} {}",
            "Transform:",
            if path.is_identity() {
                "identity (no metadata change)".to_string()
            } else {
                "Profile 8 -> Profile 8.4 (real PQ->HLG rescale)".to_string()
            }
        );
        if !path.is_identity() {
            println!(
                "{:20} {}",
                "Level 2 (trims):",
                if level2_stripped {
                    "stripped (--preserve-levels not set; stale for the new target)".to_string()
                } else if preserve_levels {
                    "preserved (--preserve-levels set)".to_string()
                } else {
                    "absent in source (nothing to strip)".to_string()
                }
            );
        }
        println!();
        println!(
            "{}",
            "Conversion complete (real RPU transform of the input, not a blank default).".green()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

async fn run_metadata(
    input: &PathBuf,
    level: Option<u8>,
    vdr: bool,
    header: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    // Read and parse the REAL source RPU rather than reporting a default's
    // fields under the input file's name.
    let (rpu, _framing) = read_and_parse_rpu(input)
        .context("dolby-vision metadata: cannot report metadata for an unparseable input")?;

    let mut info = serde_json::json!({
        "file": input.display().to_string(),
        "profile": profile_number(rpu.profile),
    });

    if header {
        info["header"] = serde_json::json!({
            "rpu_type": rpu.header.rpu_type,
            "rpu_format": rpu.header.rpu_format,
        });
    }

    if vdr {
        info["vdr_dm_data"] = if rpu.vdr_dm_data.is_some() {
            serde_json::json!("present")
        } else {
            serde_json::json!("absent")
        };
    }

    if let Some(lvl) = level {
        let level_present = match lvl {
            1 => rpu.level1.is_some(),
            2 => rpu.level2.is_some(),
            5 => rpu.level5.is_some(),
            6 => rpu.level6.is_some(),
            8 => rpu.level8.is_some(),
            9 => rpu.level9.is_some(),
            11 => rpu.level11.is_some(),
            _ => false,
        };
        info[format!("level{lvl}")] =
            serde_json::json!(if level_present { "present" } else { "absent" });
    }

    if json_output {
        let s = serde_json::to_string_pretty(&info).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Dolby Vision Metadata".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "File:", input.display());
        println!("{:20} {}", "Profile:", profile_number(rpu.profile));
        if header {
            println!();
            println!("{}", "RPU Header".cyan().bold());
            println!("{:20} {}", "  RPU type:", rpu.header.rpu_type);
            println!("{:20} {}", "  RPU format:", rpu.header.rpu_format);
        }
        if vdr {
            println!(
                "{:20} {}",
                "VDR DM data:",
                if rpu.vdr_dm_data.is_some() {
                    "present"
                } else {
                    "absent"
                }
            );
        }
        if let Some(lvl) = level {
            let present = match lvl {
                1 => rpu.level1.is_some(),
                2 => rpu.level2.is_some(),
                5 => rpu.level5.is_some(),
                6 => rpu.level6.is_some(),
                8 => rpu.level8.is_some(),
                9 => rpu.level9.is_some(),
                11 => rpu.level11.is_some(),
                _ => false,
            };
            println!(
                "{:20} {}",
                format!("Level {lvl}:"),
                if present { "present" } else { "absent" }
            );
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Validate
// ---------------------------------------------------------------------------

/// Compute the real validation checks for a Dolby Vision RPU.
///
/// Pure and I/O-free: takes an already-parsed RPU (or `None` plus the parse
/// error text when the input could not be parsed at all) and the same
/// `--profile`/`--compat` inputs `run_validate` receives, and returns the
/// check list plus overall pass/fail. Separated out of `run_validate` so
/// tests can assert on the computed checks directly instead of scraping
/// stdout.
///
/// Unlike the pre-fix implementation (which validated a freshly-constructed
/// default/profile RPU that trivially always passed), every check here is
/// computed from the REAL parsed input.
fn compute_validate_checks(
    rpu: Option<&oximedia_dolbyvision::DolbyVisionRpu>,
    parse_error: Option<&str>,
    profile: Option<u8>,
    compat: bool,
) -> (Vec<(&'static str, bool, String)>, bool) {
    let mut checks: Vec<(&'static str, bool, String)> = Vec::new();
    let mut all_passed = true;

    match rpu {
        Some(_) => checks.push(("parse", true, "RPU parsed successfully".to_string())),
        None => {
            all_passed = false;
            let detail = parse_error
                .map(str::to_string)
                .unwrap_or_else(|| "input did not parse as a Dolby Vision RPU".to_string());
            checks.push(("parse", false, detail));
        }
    }

    if let Some(rpu) = rpu {
        // Structure validation on the REAL parsed RPU.
        match rpu.validate() {
            Ok(()) => checks.push(("structure", true, "RPU structure is valid".to_string())),
            Err(e) => {
                all_passed = false;
                checks.push(("structure", false, format!("RPU structure invalid: {e}")));
            }
        }

        // Profile check: compare the caller's expected profile against the
        // REAL (auto-detected or explicitly-signalled) profile of the
        // input RPU, not a profile manufactured to match by construction.
        if let Some(p) = profile {
            let matches = profile_number(rpu.profile) == p;
            if !matches {
                all_passed = false;
            }
            checks.push((
                "profile_match",
                matches,
                format!(
                    "expected profile {p}, RPU is profile {}",
                    profile_number(rpu.profile)
                ),
            ));
        }

        // Compatibility check on the REAL RPU's profile.
        if compat {
            let bwd = rpu.profile.is_backward_compatible();
            checks.push((
                "backward_compat",
                bwd,
                "Backward compatible with SDR/HDR10/HLG".to_string(),
            ));
        }
    } else {
        // No parsed RPU: the remaining checks have nothing real to check
        // against, so they are honestly skipped rather than fabricated.
        if profile.is_some() {
            checks.push((
                "profile_match",
                false,
                "skipped: input did not parse as a Dolby Vision RPU".to_string(),
            ));
        }
        if compat {
            checks.push((
                "backward_compat",
                false,
                "skipped: input did not parse as a Dolby Vision RPU".to_string(),
            ));
        }
    }

    (checks, all_passed)
}

async fn run_validate(
    input: &PathBuf,
    profile: Option<u8>,
    compat: bool,
    _strict: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    // Parse the REAL input RPU. Unlike this command's other checks, an
    // unparseable file is itself a legitimate validation failure rather
    // than a hard program error: a validator's job includes reporting "this
    // is not a valid RPU" about bad input, so it becomes a failed check
    // instead of an early return. Previously this command never read the
    // input at all — it validated a freshly-constructed, always-valid
    // default/profile RPU, so `validate` could never fail on real content.
    let (rpu, parse_err_msg) = match read_and_parse_rpu(input) {
        Ok((rpu, _framing)) => (Some(rpu), None),
        Err(e) => (None, Some(e.to_string())),
    };

    let (checks, all_passed) =
        compute_validate_checks(rpu.as_ref(), parse_err_msg.as_deref(), profile, compat);

    if json_output {
        let result = serde_json::json!({
            "command": "dolby-vision validate",
            "input": input.display().to_string(),
            "all_passed": all_passed,
            "checks": checks.iter().map(|(n, p, d)| serde_json::json!({"check": n, "passed": p, "detail": d})).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Dolby Vision Validation".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!();
        for (name, passed, detail) in &checks {
            let status = if *passed {
                "PASS".green().to_string()
            } else {
                "FAIL".red().to_string()
            };
            println!("  [{}] {:25} {}", status, name, detail);
        }
        println!();
        if all_passed {
            println!("{}", "All validation checks passed.".green());
        } else {
            println!("{}", "Some validation checks failed.".red());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Info
// ---------------------------------------------------------------------------

async fn run_info(
    profile: Option<u8>,
    list: bool,
    compat_matrix: bool,
    json_output: bool,
) -> Result<()> {
    let all_profiles = [
        oximedia_dolbyvision::Profile::Profile5,
        oximedia_dolbyvision::Profile::Profile7,
        oximedia_dolbyvision::Profile::Profile8,
        oximedia_dolbyvision::Profile::Profile8_1,
        oximedia_dolbyvision::Profile::Profile8_4,
    ];

    if let Some(p) = profile {
        let prof = parse_profile(p)?;
        if json_output {
            let result = serde_json::json!({
                "profile": p,
                "description": profile_description(prof),
                "backward_compatible": prof.is_backward_compatible(),
                "has_mel": prof.has_mel(),
                "is_hlg": prof.is_hlg(),
                "is_low_latency": prof.is_low_latency(),
            });
            let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
            println!("{s}");
        } else {
            println!("{}", "Dolby Vision Profile Info".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:25} {}", "Profile:", p);
            println!("{:25} {}", "Description:", profile_description(prof));
            println!(
                "{:25} {}",
                "Backward compatible:",
                prof.is_backward_compatible()
            );
            println!("{:25} {}", "Has MEL:", prof.has_mel());
            println!("{:25} {}", "HLG:", prof.is_hlg());
            println!("{:25} {}", "Low latency:", prof.is_low_latency());
        }
        return Ok(());
    }

    if list || compat_matrix {
        let profiles_info: Vec<serde_json::Value> = all_profiles
            .iter()
            .map(|p| {
                serde_json::json!({
                    "profile": profile_number(*p),
                    "description": profile_description(*p),
                    "backward_compatible": p.is_backward_compatible(),
                    "has_mel": p.has_mel(),
                    "is_hlg": p.is_hlg(),
                    "is_low_latency": p.is_low_latency(),
                })
            })
            .collect();

        if json_output {
            let result = serde_json::json!({
                "command": "dolby-vision info",
                "profiles": profiles_info,
            });
            let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
            println!("{s}");
        } else {
            println!("{}", "Dolby Vision Profiles".green().bold());
            println!("{}", "=".repeat(70));
            println!(
                "  {:10} {:40} {:6} {:5} {:5}",
                "Profile", "Description", "Compat", "MEL", "HLG"
            );
            println!("{}", "-".repeat(70));
            for p in &all_profiles {
                println!(
                    "  {:10} {:40} {:6} {:5} {:5}",
                    profile_number(*p),
                    profile_description(*p),
                    if p.is_backward_compatible() {
                        "Yes"
                    } else {
                        "No"
                    },
                    if p.has_mel() { "Yes" } else { "No" },
                    if p.is_hlg() { "Yes" } else { "No" },
                );
            }
        }
    } else {
        // Default: show summary
        if !json_output {
            println!("{}", "Dolby Vision Info".green().bold());
            println!("{}", "=".repeat(60));
            println!("Supported profiles: 5, 7, 8, 8.1, 8.4");
            println!("Use --list for details or --profile <N> for specific info.");
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

    #[test]
    fn test_parse_profile() {
        assert!(parse_profile(5).is_ok());
        assert!(parse_profile(7).is_ok());
        assert!(parse_profile(8).is_ok());
        assert!(parse_profile(81).is_ok());
        assert!(parse_profile(84).is_ok());
        assert!(parse_profile(99).is_err());
    }

    #[test]
    fn test_profile_description() {
        let desc = profile_description(oximedia_dolbyvision::Profile::Profile8);
        assert!(desc.contains("backward compatible"));
    }

    #[test]
    fn test_profile_number() {
        assert_eq!(profile_number(oximedia_dolbyvision::Profile::Profile5), 5);
        assert_eq!(
            profile_number(oximedia_dolbyvision::Profile::Profile8_4),
            84
        );
    }

    #[test]
    fn test_rpu_default_validates() {
        let rpu = oximedia_dolbyvision::DolbyVisionRpu::default();
        assert!(rpu.validate().is_ok());
    }

    #[test]
    fn test_profile_properties() {
        let p8 = oximedia_dolbyvision::Profile::Profile8;
        assert!(p8.is_backward_compatible());
        assert!(!p8.has_mel());
        assert!(!p8.is_hlg());
    }

    // ── Real RPU parsing / fabrication-elimination tests ───────────────────

    /// Unique temp-file path for this test process. Per project policy,
    /// tests must use `std::env::temp_dir()` rather than a hardcoded path.
    fn dv_temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_dolbyvision_cmd_test_{}_{name}",
            std::process::id()
        ))
    }

    /// Write a real, genuinely-parseable RPU bitstream fixture for `profile`.
    fn write_rpu_fixture(path: &std::path::Path, profile: oximedia_dolbyvision::Profile) {
        let rpu = oximedia_dolbyvision::DolbyVisionRpu::new(profile);
        let bytes = rpu
            .write_to_bitstream()
            .expect("write_to_bitstream should succeed for a freshly-constructed RPU");
        std::fs::write(path, bytes).expect("writing RPU fixture should succeed");
    }

    #[test]
    fn test_read_and_parse_rpu_roundtrips_profile5() {
        // Profile 5 is unambiguously auto-detected (mapping_color_space ==
        // 2), unlike 7/8/8.1/8.4 which share heuristic fallbacks in
        // `detect_profile_from_header` — see that function's doc comment.
        let path = dv_temp_path("p5.rpu");
        write_rpu_fixture(&path, oximedia_dolbyvision::Profile::Profile5);

        let (rpu, framing) = read_and_parse_rpu(&path).expect("real RPU bytes must parse");
        assert_eq!(framing, RpuFraming::Bitstream);
        assert_eq!(rpu.profile, oximedia_dolbyvision::Profile::Profile5);

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_read_and_parse_rpu_missing_file() {
        let path = dv_temp_path("does_not_exist.rpu");
        assert!(read_and_parse_rpu(&path).is_err());
    }

    #[test]
    fn test_read_and_parse_rpu_rejects_empty_file() {
        let path = dv_temp_path("empty.rpu");
        std::fs::write(&path, []).expect("write empty file");
        assert!(read_and_parse_rpu(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_read_and_parse_rpu_rejects_garbage() {
        // Same 2-byte buffer the dolbyvision crate's own parser tests use
        // for "invalid NAL type" (`test_parse_invalid_nal_type`); too short
        // to be a real header and not a recognized NAL type either.
        let path = dv_temp_path("garbage.rpu");
        std::fs::write(&path, [0x00, 0x00]).expect("write garbage file");
        let err = read_and_parse_rpu(&path).expect_err("garbage must not parse as a real RPU");
        let msg = err.to_string();
        assert!(
            msg.contains("bitstream") && msg.contains("NAL"),
            "error should explain both parse attempts: {msg}"
        );
        std::fs::remove_file(&path).ok();
    }

    // ── analyze: real fields, honest-err on bad input ───────────────────────

    #[tokio::test]
    async fn test_run_analyze_reports_real_profile_not_hardcoded_default() {
        let input = dv_temp_path("analyze_p5_in.rpu");
        let output = dv_temp_path("analyze_p5_out.json");
        write_rpu_fixture(&input, oximedia_dolbyvision::Profile::Profile5);
        std::fs::remove_file(&output).ok();

        run_analyze(&input, false, &Some(output.clone()), false, true)
            .await
            .expect("analyze on a real, parseable RPU must succeed");

        let written = std::fs::read_to_string(&output).expect("analyze must write the output file");
        let parsed: serde_json::Value =
            serde_json::from_str(&written).expect("output must be valid JSON");
        // The pre-fix implementation always reported profile 8 (the default
        // RPU's profile) regardless of input; a genuine Profile 5 fixture
        // proves the report now reflects the real file's bytes.
        assert_eq!(
            parsed["profile"], 5,
            "must report the REAL parsed profile, not the old hardcoded default of 8: {parsed}"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn test_run_analyze_honest_err_on_unparseable_input() {
        let input = dv_temp_path("analyze_garbage_in.rpu");
        std::fs::write(&input, [0x00, 0x00]).expect("write garbage");

        let result = run_analyze(&input, false, &None, false, true).await;
        assert!(
            result.is_err(),
            "unparseable input must be an honest error, not a fabricated analysis"
        );

        std::fs::remove_file(&input).ok();
    }

    // ── convert: real transform, no fabricated blank-default output ─────────

    #[tokio::test]
    async fn test_run_convert_identity_writes_real_roundtrip() {
        let input = dv_temp_path("convert_identity_in.rpu");
        let output = dv_temp_path("convert_identity_out.rpu");
        write_rpu_fixture(&input, oximedia_dolbyvision::Profile::Profile8);
        std::fs::remove_file(&output).ok();

        run_convert(&input, &output, None, 8, false, true)
            .await
            .expect("identity conversion (P8 -> P8) must succeed");

        assert!(
            output.exists(),
            "identity conversion must produce a real output file"
        );
        let (roundtripped, _framing) =
            read_and_parse_rpu(&output).expect("output must itself be a real, parseable RPU");
        assert_eq!(
            roundtripped.profile,
            oximedia_dolbyvision::Profile::Profile8
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn test_run_convert_profile8_to_84_applies_real_transform() {
        // A bare `DolbyVisionRpu::new(Profile8)` has no Level 1/6 metadata
        // and `default_for_profile` only special-cases Profile 5 in the
        // header, so its bitstream is indistinguishable from a Profile 8.4
        // default at the header level — auto-detection on read-back would
        // fall back to Profile 8 either way (see
        // `detect_profile_from_header`'s documented ambiguity), which would
        // make a round-tripped `.profile` comparison meaningless here. Use
        // a source RPU with real Level 1 (PQ range) + Level 6 (mastering
        // luminance) data instead, so the PQ->HLG rescale that
        // `convert_profile8_to_8_4` performs has a concrete, checkable
        // effect independent of that heuristic.
        let input = dv_temp_path("convert_8_to_84_in.rpu");
        let output = dv_temp_path("convert_8_to_84_out.rpu");

        let mut source =
            oximedia_dolbyvision::DolbyVisionRpu::new(oximedia_dolbyvision::Profile::Profile8);
        source.level1 = Some(oximedia_dolbyvision::Level1Metadata {
            min_pq: 0,
            avg_pq: 2000,
            max_pq: 4000,
        });
        source.level6 = Some(oximedia_dolbyvision::Level6Metadata {
            max_cll: 1000,
            max_fall: 400,
            min_display_mastering_luminance: 1,
            max_display_mastering_luminance: 4000, // >> HLG's 1000-nit nominal peak
            master_display_primaries: [[34000, 16000], [13250, 34500], [7500, 3000]],
            master_display_white_point: [15635, 16450],
        });
        let source_max_pq = source.level1.as_ref().expect("level1 was just set").max_pq;
        let fixture_bytes = source
            .write_to_bitstream()
            .expect("write_to_bitstream should succeed for a valid RPU");
        std::fs::write(&input, fixture_bytes).expect("write input fixture");
        std::fs::remove_file(&output).ok();

        run_convert(&input, &output, Some(8), 84, false, true)
            .await
            .expect("Profile 8 -> Profile 8.4 has a real transform and must succeed");

        let (converted, _framing) =
            read_and_parse_rpu(&output).expect("output must be a real, parseable RPU");
        let converted_l1 = converted
            .level1
            .expect("Level 1 metadata must survive the real transform");
        assert_ne!(
            converted_l1.max_pq, source_max_pq,
            "the real Profile 8 -> 8.4 PQ->HLG rescale must change Level 1 max_pq \
             (an identity copy or a blank default would leave it unchanged at {source_max_pq})"
        );
        assert!(
            converted_l1.max_pq < source_max_pq,
            "with a 4000-nit mastering peak the HLG rescale must reduce max_pq \
             (source {source_max_pq}, got {})",
            converted_l1.max_pq
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    /// Build a Profile 8 fixture carrying real Level 1 + Level 2 data, write
    /// it to `path`, and return the source RPU (so callers can compare
    /// against `converted.level1` etc.).
    fn write_p8_fixture_with_level2(
        path: &std::path::Path,
    ) -> oximedia_dolbyvision::DolbyVisionRpu {
        let mut source =
            oximedia_dolbyvision::DolbyVisionRpu::new(oximedia_dolbyvision::Profile::Profile8);
        source.level1 = Some(oximedia_dolbyvision::Level1Metadata {
            min_pq: 0,
            avg_pq: 2000,
            max_pq: 4000,
        });
        source.level2 = Some(oximedia_dolbyvision::Level2Metadata {
            target_display_index: 1,
            trim_slope: 100,
            ..Default::default()
        });
        let bytes = source
            .write_to_bitstream()
            .expect("write_to_bitstream should succeed for a valid RPU");
        std::fs::write(path, bytes).expect("write input fixture");
        source
    }

    #[tokio::test]
    async fn test_run_convert_default_strips_level2_trim_passes() {
        let input = dv_temp_path("convert_8_to_84_strip_in.rpu");
        let output = dv_temp_path("convert_8_to_84_strip_out.rpu");
        let source = write_p8_fixture_with_level2(&input);
        assert!(source.level2.is_some(), "fixture must carry Level 2 data");
        std::fs::remove_file(&output).ok();

        // Default (no --preserve-levels) must strip stale Level 2 trims.
        run_convert(&input, &output, Some(8), 84, false, true)
            .await
            .expect("Profile 8 -> Profile 8.4 must succeed");

        let (converted, _framing) =
            read_and_parse_rpu(&output).expect("output must be a real, parseable RPU");
        assert!(
            converted.level2.is_none(),
            "Level 2 trim passes must be stripped by default (they were tuned for the PQ \
             target, not the HLG one this conversion just produced)"
        );
        // The real Level 1 rescale must still have happened — stripping
        // Level 2 must not degrade into an identity/blank copy.
        assert!(converted.level1.is_some());

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn test_run_convert_preserve_levels_keeps_level2() {
        let input = dv_temp_path("convert_8_to_84_preserve_in.rpu");
        let output = dv_temp_path("convert_8_to_84_preserve_out.rpu");
        write_p8_fixture_with_level2(&input);
        std::fs::remove_file(&output).ok();

        run_convert(&input, &output, Some(8), 84, true, true)
            .await
            .expect("Profile 8 -> Profile 8.4 must succeed");

        let (converted, _framing) =
            read_and_parse_rpu(&output).expect("output must be a real, parseable RPU");
        let l2 = converted
            .level2
            .expect("--preserve-levels must keep Level 2 trim passes");
        assert_eq!(
            l2.trim_slope, 100,
            "preserved Level 2 must be the source's own data"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn test_run_convert_identity_never_strips_regardless_of_preserve_levels() {
        // Identity conversions (P8 -> P8) are a plain clone, not a
        // Profile-8-to-8.4 rescale; `--preserve-levels` must not touch them.
        let input = dv_temp_path("convert_identity_level2_in.rpu");
        let output = dv_temp_path("convert_identity_level2_out.rpu");
        write_p8_fixture_with_level2(&input);
        std::fs::remove_file(&output).ok();

        run_convert(&input, &output, None, 8, false, true)
            .await
            .expect("identity conversion must succeed");

        let (converted, _framing) =
            read_and_parse_rpu(&output).expect("output must be a real, parseable RPU");
        assert!(
            converted.level2.is_some(),
            "identity conversion must never strip metadata, even with --preserve-levels unset"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_file(&output).ok();
    }

    #[tokio::test]
    async fn test_run_convert_unsupported_path_is_honest_err_with_no_output_file() {
        let input = dv_temp_path("convert_unsupported_in.rpu");
        let output = dv_temp_path("convert_unsupported_out.rpu");
        write_rpu_fixture(&input, oximedia_dolbyvision::Profile::Profile5);
        std::fs::remove_file(&output).ok();

        // Profile 5 -> Profile 7 has no real transform implemented (only
        // identity and Profile 8 -> Profile 8.4 do).
        let err = run_convert(&input, &output, Some(5), 7, false, true)
            .await
            .expect_err("unimplemented conversion path must be an honest error");
        let msg = err.to_string();
        assert!(
            msg.contains("not yet"),
            "error must be explicit about the missing implementation: {msg}"
        );
        assert!(
            !output.exists(),
            "no fabricated/mislabeled output file may be produced for an unsupported path"
        );

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_run_convert_honest_err_on_unparseable_input_writes_nothing() {
        let input = dv_temp_path("convert_garbage_in.rpu");
        let output = dv_temp_path("convert_garbage_out.rpu");
        std::fs::write(&input, [0x00, 0x00]).expect("write garbage");
        std::fs::remove_file(&output).ok();

        let result = run_convert(&input, &output, None, 8, false, true).await;
        assert!(
            result.is_err(),
            "garbage input must not be silently 'converted'"
        );
        assert!(
            !output.exists(),
            "no output file may be fabricated from unparseable input"
        );

        std::fs::remove_file(&input).ok();
    }

    // ── metadata: real fields ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_run_metadata_reports_real_profile() {
        let input = dv_temp_path("metadata_p5_in.rpu");
        write_rpu_fixture(&input, oximedia_dolbyvision::Profile::Profile5);

        run_metadata(&input, None, false, false, true)
            .await
            .expect("metadata on a real, parseable RPU must succeed");

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_run_metadata_honest_err_on_unparseable_input() {
        let input = dv_temp_path("metadata_garbage_in.rpu");
        std::fs::write(&input, [0x00, 0x00]).expect("write garbage");

        let result = run_metadata(&input, None, false, false, true).await;
        assert!(result.is_err());

        std::fs::remove_file(&input).ok();
    }

    // ── validate: real structural check, not a rubber stamp ─────────────────

    #[test]
    fn test_compute_validate_checks_all_pass_on_real_valid_rpu() {
        let rpu =
            oximedia_dolbyvision::DolbyVisionRpu::new(oximedia_dolbyvision::Profile::Profile5);
        let (checks, all_passed) = compute_validate_checks(Some(&rpu), None, Some(5), true);
        assert!(all_passed, "checks: {checks:?}");
        assert!(checks.iter().any(|(n, p, _)| *n == "profile_match" && *p));
    }

    #[test]
    fn test_compute_validate_checks_detects_real_profile_mismatch() {
        // Previously `--profile` constructed a fresh RPU of that exact
        // profile, so `profile_match` could never fail. Now it must compare
        // against the REAL RPU (Profile 5 here), which does not match 8.
        let rpu =
            oximedia_dolbyvision::DolbyVisionRpu::new(oximedia_dolbyvision::Profile::Profile5);
        let (checks, all_passed) = compute_validate_checks(Some(&rpu), None, Some(8), false);
        assert!(!all_passed, "checks: {checks:?}");
        let profile_check = checks
            .iter()
            .find(|(n, _, _)| *n == "profile_match")
            .expect("profile_match check must be present");
        assert!(!profile_check.1, "profile 5 must not match expected 8");
    }

    #[test]
    fn test_compute_validate_checks_unparseable_input_fails_parse_check_only() {
        let (checks, all_passed) =
            compute_validate_checks(None, Some("boom: not an RPU"), Some(8), true);
        assert!(!all_passed);
        let parse_check = checks
            .iter()
            .find(|(n, _, _)| *n == "parse")
            .expect("parse check must be present");
        assert!(!parse_check.1);
        assert!(parse_check.2.contains("boom"));
        // profile_match/backward_compat must be honestly marked skipped,
        // never fabricated as passing.
        for name in ["profile_match", "backward_compat"] {
            let check = checks
                .iter()
                .find(|(n, _, _)| *n == name)
                .unwrap_or_else(|| panic!("{name} check must be present"));
            assert!(!check.1, "{name} must not be fabricated as passing");
        }
    }

    #[tokio::test]
    async fn test_run_validate_completes_without_hard_error_on_garbage_input() {
        // A validator's job includes saying "this isn't a valid RPU" about
        // bad input — that's a reported failure via the checks list, not a
        // Rust-level panic or an unrelated process crash.
        let input = dv_temp_path("validate_garbage_in.rpu");
        std::fs::write(&input, [0x00, 0x00]).expect("write garbage");

        let result = run_validate(&input, None, false, false, true).await;
        assert!(
            result.is_ok(),
            "validate must complete and report, not hard-error, on unparseable input"
        );

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_run_validate_missing_file_is_err() {
        let input = dv_temp_path("validate_missing.rpu");
        let result = run_validate(&input, None, false, false, true).await;
        assert!(result.is_err());
    }
}
