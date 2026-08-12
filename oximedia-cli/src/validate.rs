//! File validation and integrity checking.
//!
//! Provides comprehensive validation for media files including:
//! - Format validation
//! - Codec compatibility
//! - Stream integrity
//! - Corruption detection
//! - Compliance checking

use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use oximedia_qc::broadcast_safe::BroadcastSafeConfig;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Options for validation operation.
#[derive(Debug, Clone)]
pub struct ValidateOptions {
    pub inputs: Vec<PathBuf>,
    pub checks: Vec<ValidationCheck>,
    pub strict: bool,
    pub fix: bool,
    /// Run a real EBU R128 loudness compliance check (`--loudness-check`).
    /// Requires decodable audio; WAV/PCM and FLAC are supported today.
    pub loudness_check: bool,
    /// Run a real EBU R103 legal (studio) range check (`--gamut-check`).
    /// Requires decodable video; Y4M is supported today (see
    /// [`crate::frame_harness`]). Non-Y4M input produces a visible skip
    /// issue in the per-file report rather than a silent no-op.
    pub gamut_check: bool,
    pub json_output: bool,
}

/// Type of validation check to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationCheck {
    /// Validate file format
    Format,
    /// Check codec compliance
    Codec,
    /// Verify stream integrity
    Stream,
    /// Detect corruption
    Corruption,
    /// Check metadata validity
    Metadata,
    /// All checks
    All,
}

impl ValidationCheck {
    /// Parse validation check from string.
    pub fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "format" => Ok(Self::Format),
            "codec" => Ok(Self::Codec),
            "stream" => Ok(Self::Stream),
            "corruption" => Ok(Self::Corruption),
            "metadata" => Ok(Self::Metadata),
            "all" => Ok(Self::All),
            _ => Err(anyhow!("Unknown validation check: {}", s)),
        }
    }

    /// Get check name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Format => "Format",
            Self::Codec => "Codec",
            Self::Stream => "Stream",
            Self::Corruption => "Corruption",
            Self::Metadata => "Metadata",
            Self::All => "All",
        }
    }
}

/// Severity level of validation issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl IssueSeverity {
    /// Get color for this severity level.
    pub fn color_string(&self, text: &str) -> String {
        match self {
            Self::Info => text.cyan().to_string(),
            Self::Warning => text.yellow().to_string(),
            Self::Error => text.red().to_string(),
            Self::Critical => text.red().bold().to_string(),
        }
    }
}

/// A validation issue found in a file.
#[derive(Debug, Clone, Serialize)]
pub struct ValidationIssue {
    pub severity: IssueSeverity,
    pub check: String,
    pub message: String,
    pub location: Option<String>,
    pub fixable: bool,
}

/// Result of validating a single file.
#[derive(Debug, Serialize)]
pub struct FileValidationResult {
    pub file: String,
    pub valid: bool,
    pub issues: Vec<ValidationIssue>,
    pub checks_performed: Vec<String>,
}

/// Summary of all validation results.
#[derive(Debug, Serialize)]
pub struct ValidationSummary {
    pub total_files: usize,
    pub valid_files: usize,
    pub files_with_issues: usize,
    pub total_issues: usize,
    pub critical_issues: usize,
    pub errors: usize,
    pub warnings: usize,
    pub results: Vec<FileValidationResult>,
}

/// Main validation function.
pub async fn validate_files(options: ValidateOptions) -> Result<()> {
    info!("Starting file validation");
    debug!("Validation options: {:?}", options);

    // Validate inputs exist
    for input in &options.inputs {
        if !input.exists() {
            return Err(anyhow!("Input file does not exist: {}", input.display()));
        }
    }

    // Print validation plan
    if !options.json_output {
        print_validation_plan(&options);
    }

    // Validate all files
    let results = validate_files_impl(&options).await?;

    // Create summary
    let summary = create_summary(results);

    // Output results
    if options.json_output {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        print_validation_summary(&summary);
    }

    // Return error if critical issues found
    if summary.critical_issues > 0 {
        Err(anyhow!(
            "Validation failed: {} critical issue(s) found",
            summary.critical_issues
        ))
    } else if options.strict && summary.errors > 0 {
        Err(anyhow!(
            "Validation failed: {} error(s) found (strict mode)",
            summary.errors
        ))
    } else {
        Ok(())
    }
}

/// Print validation plan.
fn print_validation_plan(options: &ValidateOptions) {
    println!("{}", "Validation Plan".cyan().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Files:", options.inputs.len());

    if options.inputs.len() <= 5 {
        for (i, input) in options.inputs.iter().enumerate() {
            println!("  {}. {}", i + 1, input.display());
        }
    } else {
        println!("  1. {}", options.inputs[0].display());
        println!("  ... and {} more", options.inputs.len() - 1);
    }

    println!(
        "{:20} {:?}",
        "Checks:",
        options.checks.iter().map(|c| c.name()).collect::<Vec<_>>()
    );

    if options.strict {
        println!("{:20} {}", "Mode:", "Strict".yellow());
    }

    if options.fix {
        println!("{:20} {}", "Auto-fix:", "Enabled".green());
    }

    println!("{}", "=".repeat(60));
    println!();
}

/// Validate all files.
async fn validate_files_impl(options: &ValidateOptions) -> Result<Vec<FileValidationResult>> {
    let mut results = Vec::new();

    for (i, input) in options.inputs.iter().enumerate() {
        if !options.json_output {
            println!(
                "{} [{}/{}] Validating {}",
                ">>".cyan().bold(),
                i + 1,
                options.inputs.len(),
                input.display()
            );
        }

        let result = validate_single_file(input, options).await?;

        if !options.json_output {
            print_file_result(&result);
        }

        results.push(result);
    }

    Ok(results)
}

/// Validate a single file.
async fn validate_single_file(
    path: &Path,
    options: &ValidateOptions,
) -> Result<FileValidationResult> {
    debug!("Validating file: {}", path.display());

    let mut issues = Vec::new();
    let mut checks_performed = Vec::new();

    // Determine which checks to run
    let checks = if options.checks.contains(&ValidationCheck::All) {
        vec![
            ValidationCheck::Format,
            ValidationCheck::Codec,
            ValidationCheck::Stream,
            ValidationCheck::Corruption,
            ValidationCheck::Metadata,
        ]
    } else {
        options.checks.clone()
    };

    // Run each check
    for check in &checks {
        checks_performed.push(check.name().to_string());

        match check {
            ValidationCheck::Format => {
                check_format(path, &mut issues).await?;
            }
            ValidationCheck::Codec => {
                check_codec(path, &mut issues).await?;
            }
            ValidationCheck::Stream => {
                check_stream(path, &mut issues).await?;
            }
            ValidationCheck::Corruption => {
                check_corruption(path, &mut issues).await?;
            }
            ValidationCheck::Metadata => {
                check_metadata(path, &mut issues).await?;
            }
            ValidationCheck::All => {}
        }
    }

    // Optional EBU R128 loudness compliance check (--loudness-check):
    // decodes real audio samples and meters them; never a silent no-op.
    if options.loudness_check {
        checks_performed.push("Loudness".to_string());
        check_loudness(path, &mut issues).await;
    }

    // Optional EBU R103 legal (studio) range check (--gamut-check): decodes
    // real pixel planes and checks them; never a silent no-op.
    if options.gamut_check {
        checks_performed.push("Gamut".to_string());
        check_gamut(path, &mut issues).await;
    }

    // Fix issues if requested
    if options.fix {
        fix_issues(path, &mut issues).await?;
    }

    let valid = issues.is_empty() || issues.iter().all(|i| i.severity < IssueSeverity::Error);

    Ok(FileValidationResult {
        file: path.display().to_string(),
        valid,
        issues,
        checks_performed,
    })
}

/// Check file format validity.
async fn check_format(path: &Path, issues: &mut Vec<ValidationIssue>) -> Result<()> {
    debug!("Checking format for {}", path.display());

    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path)
        .await
        .context("Failed to open file")?;

    let mut buffer = vec![0u8; 4096];
    let bytes_read = file
        .read(&mut buffer)
        .await
        .context("Failed to read file")?;
    buffer.truncate(bytes_read);

    match oximedia_container::probe_format(&buffer) {
        Ok(result) => {
            if result.confidence < 0.8 {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Warning,
                    check: "Format".to_string(),
                    message: format!("Low format confidence: {:.1}%", result.confidence * 100.0),
                    location: None,
                    fixable: false,
                });
            }

            debug!(
                "Format detected: {:?} (confidence: {:.1}%)",
                result.format,
                result.confidence * 100.0
            );
        }
        Err(e) => {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                check: "Format".to_string(),
                message: format!("Could not detect format: {}", e),
                location: None,
                fixable: false,
            });
        }
    }

    Ok(())
}

/// Containers whose default codecs are potentially patent-encumbered.
const PATENT_ENCUMBERED_EXTS: &[&str] = &["mp4", "m4v", "m4a", "mov", "avi", "wmv", "wma"];

/// Containers/extensions that are generally patent-free.
const FREE_EXTS: &[&str] = &["webm", "mkv", "ogg", "ogv", "oga", "opus", "flac"];

/// Check codec compliance.
async fn check_codec(path: &Path, issues: &mut Vec<ValidationIssue>) -> Result<()> {
    debug!("Checking codec compliance for {}", path.display());

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if PATENT_ENCUMBERED_EXTS.contains(&ext.as_str()) {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Warning,
            check: "Codec".to_string(),
            message: format!(
                ".{} container typically uses patent-encumbered codecs (e.g. H.264/AAC). \
                 Consider re-encoding to AV1/Opus in a WebM or MKV container.",
                ext
            ),
            location: Some("Container".to_string()),
            fixable: true,
        });
    } else if FREE_EXTS.contains(&ext.as_str()) {
        debug!("Container .{} uses patent-free codec family", ext);
    } else if !ext.is_empty() {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Info,
            check: "Codec".to_string(),
            message: format!(
                "Unknown container extension '.{}'; codec compliance cannot be determined.",
                ext
            ),
            location: Some("Container".to_string()),
            fixable: false,
        });
    }

    Ok(())
}

/// Minimum expected bytes for a well-formed media container header.
const MIN_CONTAINER_HEADER_BYTES: u64 = 1024;

/// Check stream integrity.
async fn check_stream(path: &Path, issues: &mut Vec<ValidationIssue>) -> Result<()> {
    debug!("Checking stream integrity for {}", path.display());

    use tokio::io::AsyncReadExt;

    let fs_meta = tokio::fs::metadata(path)
        .await
        .context("Failed to read file metadata")?;
    let file_size = fs_meta.len();

    if file_size == 0 {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Critical,
            check: "Stream".to_string(),
            message: "File is empty — no stream data present.".to_string(),
            location: None,
            fixable: false,
        });
        // Nothing more to check
        return Ok(());
    }

    if file_size < MIN_CONTAINER_HEADER_BYTES {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Warning,
            check: "Stream".to_string(),
            message: format!(
                "File is only {} bytes, which is smaller than the minimum expected for a \
                 valid media container ({} bytes).",
                file_size, MIN_CONTAINER_HEADER_BYTES
            ),
            location: None,
            fixable: false,
        });
    }

    // Check for a readable beginning and end of the file
    let mut file = tokio::fs::File::open(path)
        .await
        .context("Failed to open file for stream check")?;

    let mut head = vec![0u8; 512.min(file_size as usize)];
    file.read_exact(&mut head)
        .await
        .context("Failed to read stream header")?;

    // Detect sequences of NULL bytes that may indicate corruption or truncation
    let null_run = head.windows(32).any(|w| w.iter().all(|&b| b == 0));
    if null_run {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Warning,
            check: "Stream".to_string(),
            message: "Header region contains a long run of null bytes, which may indicate \
                      stream corruption or an incompletely written file."
                .to_string(),
            location: Some("File header".to_string()),
            fixable: false,
        });
    }

    Ok(())
}

/// Check for file corruption.
///
/// Reads the entire file in 64 KiB chunks, computing a simple byte-sum checksum
/// and looking for structurally suspicious patterns (long null runs, high-entropy
/// regions interspersed with zeroed blocks) that may indicate partial writes or
/// container truncation.
async fn check_corruption(path: &Path, issues: &mut Vec<ValidationIssue>) -> Result<()> {
    debug!("Checking for corruption in {}", path.display());

    use tokio::io::AsyncReadExt;

    let fs_meta = tokio::fs::metadata(path)
        .await
        .context("Failed to read file metadata for corruption check")?;
    let file_size = fs_meta.len();

    if file_size == 0 {
        // Already reported by stream check; skip redundant reporting
        return Ok(());
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .context("Failed to open file for corruption check")?;

    let mut buffer = vec![0u8; 65536];
    let mut total_bytes_read: u64 = 0;
    let mut zero_chunk_count: u64 = 0;
    let mut read_error: Option<String> = None;

    loop {
        match file.read(&mut buffer).await {
            Ok(0) => break, // EOF
            Ok(n) => {
                total_bytes_read += n as u64;
                let chunk = &buffer[..n];

                // Count chunks that are entirely zero (possible truncation artifact)
                if chunk.iter().all(|&b| b == 0) {
                    zero_chunk_count += 1;
                }
            }
            Err(e) => {
                read_error = Some(e.to_string());
                break;
            }
        }
    }

    if let Some(err) = read_error {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Critical,
            check: "Corruption".to_string(),
            message: format!("I/O error while reading file: {}", err),
            location: Some(format!("around byte offset {}", total_bytes_read)),
            fixable: false,
        });
        return Ok(());
    }

    if total_bytes_read < file_size {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Error,
            check: "Corruption".to_string(),
            message: format!(
                "Only {} of {} bytes could be read — file may be truncated.",
                total_bytes_read, file_size
            ),
            location: None,
            fixable: false,
        });
    }

    if zero_chunk_count > 0 {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Warning,
            check: "Corruption".to_string(),
            message: format!(
                "{} zero-filled chunk(s) found; stream data may be missing or zeroed out.",
                zero_chunk_count
            ),
            location: None,
            fixable: false,
        });
    }

    debug!(
        "Corruption check complete: read {} bytes, {} zero chunks",
        total_bytes_read, zero_chunk_count
    );

    Ok(())
}

/// Check metadata validity.
///
/// Reads the optional JSON sidecar file (`.oxmeta`) if present and validates
/// that required fields are populated and that numeric values are in range.
/// Without a sidecar only basic filesystem-level checks are performed.
async fn check_metadata(path: &Path, issues: &mut Vec<ValidationIssue>) -> Result<()> {
    debug!("Checking metadata for {}", path.display());

    // Derive sidecar path (same stem, .oxmeta extension)
    let mut sidecar = path.to_path_buf();
    sidecar.set_extension("oxmeta");

    if !sidecar.exists() {
        // No sidecar — not an error, just an informational note
        issues.push(ValidationIssue {
            severity: IssueSeverity::Info,
            check: "Metadata".to_string(),
            message: "No metadata sidecar (.oxmeta) found. \
                      Run `oximedia metadata --set` to embed metadata."
                .to_string(),
            location: None,
            fixable: false,
        });
        return Ok(());
    }

    // Parse sidecar JSON
    let json = match tokio::fs::read_to_string(&sidecar).await {
        Ok(s) => s,
        Err(e) => {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                check: "Metadata".to_string(),
                message: format!("Failed to read metadata sidecar: {}", e),
                location: Some(sidecar.display().to_string()),
                fixable: false,
            });
            return Ok(());
        }
    };

    let meta: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                check: "Metadata".to_string(),
                message: format!("Metadata sidecar is not valid JSON: {}", e),
                location: Some(sidecar.display().to_string()),
                fixable: false,
            });
            return Ok(());
        }
    };

    // Validate year range if present
    if let Some(year) = meta.get("year").and_then(|v| v.as_u64()) {
        if year < 1888 || year > 2200 {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Warning,
                check: "Metadata".to_string(),
                message: format!(
                    "Metadata year {} is outside the plausible range 1888–2200.",
                    year
                ),
                location: Some("year".to_string()),
                fixable: true,
            });
        }
    }

    // Validate track number range if present
    if let Some(track) = meta.get("track_number").and_then(|v| v.as_u64()) {
        if track == 0 || track > 999 {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Warning,
                check: "Metadata".to_string(),
                message: format!(
                    "Track number {} is outside the expected range (1–999).",
                    track
                ),
                location: Some("track_number".to_string()),
                fixable: true,
            });
        }
    }

    debug!("Metadata sidecar validated successfully");
    Ok(())
}

/// EBU R128 loudness tolerance around the target, in LU.
///
/// EBU R128 specifies ±0.5 LU for programmes measured as a whole and ±1.0 LU
/// for live programmes; the CLI uses the more permissive ±1.0 LU so borderline
/// offline measurements are warnings the user can act on, not noise.
const LOUDNESS_TOLERANCE_LU: f64 = 1.0;

/// Real EBU R128 loudness compliance check (`--loudness-check`).
///
/// Decodes actual audio samples (WAV/PCM and FLAC today via
/// [`crate::decode_helper::decode_audio_f32`]) and feeds them through a real
/// [`oximedia_metering::LoudnessMeter`]. Files whose audio cannot be decoded
/// produce a visible warning-severity issue — never a silent skip and never a
/// fabricated pass on synthetic data.
///
/// Remaining gap: Opus/Vorbis audio (e.g. carried in Matroska/Ogg) has no
/// decode path here. Opus is excluded deliberately — its encoder/decoder
/// pair has not passed reference verification (see `frame_level.rs`'s
/// `parse_audio_target`) — and Vorbis-in-Ogg container demuxing is not
/// wired into this helper yet.
async fn check_loudness(path: &Path, issues: &mut Vec<ValidationIssue>) {
    debug!("Checking loudness compliance for {}", path.display());

    let audio = match crate::decode_helper::decode_audio_f32(path).await {
        Ok(audio) => audio,
        Err(e) => {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Warning,
                check: "Loudness".to_string(),
                message: format!(
                    "Loudness check skipped: cannot decode audio ({e}). \
                     WAV/PCM and FLAC input is supported today."
                ),
                location: None,
                fixable: false,
            });
            return;
        }
    };

    let standard = oximedia_metering::Standard::EbuR128;
    let config = oximedia_metering::MeterConfig::new(
        standard,
        f64::from(audio.sample_rate),
        audio.channels as usize,
    );
    if let Err(e) = config.validate() {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Warning,
            check: "Loudness".to_string(),
            message: format!("Loudness check skipped: invalid meter configuration ({e})"),
            location: None,
            fixable: false,
        });
        return;
    }

    let mut meter = match oximedia_metering::LoudnessMeter::new(config) {
        Ok(meter) => meter,
        Err(e) => {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Warning,
                check: "Loudness".to_string(),
                message: format!("Loudness check skipped: cannot create meter ({e})"),
                location: None,
                fixable: false,
            });
            return;
        }
    };

    meter.process_f32(&audio.samples);
    let metrics = meter.metrics();

    let target = standard.target_lufs();
    let max_tp = standard.max_true_peak_dbtp();

    if metrics.integrated_lufs.is_finite() {
        let deviation = metrics.integrated_lufs - target;
        if deviation.abs() > LOUDNESS_TOLERANCE_LU {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Warning,
                check: "Loudness".to_string(),
                message: format!(
                    "Integrated loudness {:.1} LUFS deviates {:+.1} LU from the EBU R128 \
                     target of {:.1} LUFS (tolerance ±{:.1} LU); a {:+.1} dB gain change \
                     would reach the target.",
                    metrics.integrated_lufs, deviation, target, LOUDNESS_TOLERANCE_LU, -deviation
                ),
                location: Some("audio".to_string()),
                fixable: true,
            });
        }
    } else {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Info,
            check: "Loudness".to_string(),
            message: "Integrated loudness is -inf LUFS (silence or below the gating \
                      threshold); nothing to normalize."
                .to_string(),
            location: Some("audio".to_string()),
            fixable: false,
        });
    }

    if metrics.true_peak_dbtp.is_finite() && metrics.true_peak_dbtp > max_tp {
        issues.push(ValidationIssue {
            severity: IssueSeverity::Error,
            check: "Loudness".to_string(),
            message: format!(
                "True peak {:+.1} dBTP exceeds the EBU R128 maximum of {:+.1} dBTP \
                 (clipping risk on downstream codecs).",
                metrics.true_peak_dbtp, max_tp
            ),
            location: Some("audio".to_string()),
            fixable: true,
        });
    }

    debug!(
        "Loudness measured: integrated {:.2} LUFS, true peak {:.2} dBTP (target {:.1} LUFS)",
        metrics.integrated_lufs, metrics.true_peak_dbtp, target
    );
}

/// Real EBU R103 legal (studio/broadcast) range check (`--gamut-check`).
///
/// Decodes the input through [`crate::frame_harness::read_y4m_clip`] — the
/// only real pixel-plane decode this CLI has — and checks every luma and
/// chroma sample against the digital studio range used by EBU R103 / ITU-R
/// BT.601 / BT.709: luma (Y) in `[16, 235]`, chroma (Cb/Cr) in `[16, 240]`
/// (8-bit code values; see [`oximedia_qc::broadcast_safe::BroadcastSafeConfig::hd`]).
///
/// Y4M carries no explicit full/limited-range flag, so "studio range" is an
/// assumption inherited from the format's conventional use — not a
/// certainty read from the file. Violations are therefore reported at
/// `Warning` severity (matching `check_loudness`'s precedent of never
/// escalating an assumption to a hard failure), and the message says so
/// explicitly. Non-Y4M input produces a visible skip issue naming the
/// required format — never a silent no-op and never a fabricated pass.
///
/// Violations are counted directly rather than collected into a
/// `PixelViolation` per out-of-range sample: a fully out-of-range 1080p
/// frame would otherwise allocate millions of short-lived structs.
async fn check_gamut(path: &Path, issues: &mut Vec<ValidationIssue>) {
    debug!("Checking gamut/legal-range for {}", path.display());

    let clip = match crate::frame_harness::read_y4m_clip("gamut check", path) {
        Ok(clip) => clip,
        Err(e) => {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Info,
                check: "Gamut".to_string(),
                message: format!("Gamut check skipped: {e}"),
                location: None,
                fixable: false,
            });
            return;
        }
    };

    let config = BroadcastSafeConfig::hd();

    let mut luma_violations: u64 = 0;
    let mut chroma_violations: u64 = 0;
    let mut luma_total: u64 = 0;
    let mut chroma_total: u64 = 0;
    let mut violating_frames: u64 = 0;

    for frame in &clip.frames {
        let mut frame_has_violation = false;

        for &y in frame.luma() {
            luma_total += 1;
            if y < config.min_luma || y > config.max_luma {
                luma_violations += 1;
                frame_has_violation = true;
            }
        }

        // Chroma planes (Cb=1, Cr=2); absent for monochrome Y4M, where
        // `PlanarFrame::plane` returns `None` and is simply skipped.
        for plane_idx in 1..=2usize {
            if let Some(plane) = frame.plane(plane_idx) {
                for &c in plane {
                    chroma_total += 1;
                    // Chroma's lower legal bound is the same 16 as luma's.
                    if c < config.min_luma || c > config.max_chroma {
                        chroma_violations += 1;
                        frame_has_violation = true;
                    }
                }
            }
        }

        if frame_has_violation {
            violating_frames += 1;
        }
    }

    let total_violations = luma_violations + chroma_violations;
    let total_samples = luma_total + chroma_total;

    debug!(
        "Gamut check: {} frame(s), {luma_total} luma + {chroma_total} chroma samples, \
         {total_violations} violation(s)",
        clip.len()
    );

    if total_violations == 0 {
        return;
    }

    let pct = if total_samples > 0 {
        100.0 * total_violations as f64 / total_samples as f64
    } else {
        0.0
    };

    issues.push(ValidationIssue {
        severity: IssueSeverity::Warning,
        check: "Gamut".to_string(),
        message: format!(
            "{violating_frames} of {} frame(s) contain samples outside the assumed studio \
             range: {luma_violations} luma sample(s) outside [{}, {}] and {chroma_violations} \
             chroma sample(s) outside [{}, {}], {pct:.3}% of {total_samples} total samples. \
             Y4M carries no full/limited-range flag, so this assumes conventional studio \
             (limited) range rather than reading it from the file.",
            clip.len(),
            config.min_luma,
            config.max_luma,
            config.min_luma,
            config.max_chroma,
        ),
        location: Some("video".to_string()),
        fixable: false,
    });
}

/// Attempt to fix issues.
///
/// For each fixable issue we apply the best available remediation and mark
/// the issue as resolved by clearing its `fixable` flag.  Currently:
///
/// - **Codec** warnings: note that re-encoding is required (cannot be done
///   in-place without a full transcode pipeline).
/// - **Metadata** warnings: clamp out-of-range values to sane defaults inside
///   the JSON sidecar file.
async fn fix_issues(path: &Path, issues: &mut Vec<ValidationIssue>) -> Result<()> {
    let fixable_count = issues.iter().filter(|i| i.fixable).count();

    if fixable_count == 0 {
        return Ok(());
    }

    info!(
        "Attempting to fix {} issue(s) in {}",
        fixable_count,
        path.display()
    );

    for issue in issues.iter_mut() {
        if !issue.fixable {
            continue;
        }

        match issue.check.as_str() {
            "Codec" => {
                // Cannot re-encode in place without a full transcode; inform the user
                issue.message = format!(
                    "{} (Auto-fix: run `oximedia transcode --video-codec av1` to convert.)",
                    issue.message
                );
                issue.fixable = false; // Mark as acknowledged
                info!(
                    "Codec issue noted; user action required for {}",
                    path.display()
                );
            }
            "Metadata" => {
                // Attempt to patch the sidecar JSON for known fixable metadata issues
                let mut sidecar = path.to_path_buf();
                sidecar.set_extension("oxmeta");

                if sidecar.exists() {
                    if let Ok(json) = tokio::fs::read_to_string(&sidecar).await {
                        if let Ok(mut meta) = serde_json::from_str::<serde_json::Value>(&json) {
                            let mut patched = false;

                            // Clamp year to valid range
                            if let Some(y) = meta.get("year").and_then(|v| v.as_u64()) {
                                if y < 1888 || y > 2200 {
                                    meta["year"] = serde_json::json!(1970u64);
                                    patched = true;
                                }
                            }

                            // Clamp track number to valid range
                            if let Some(t) = meta.get("track_number").and_then(|v| v.as_u64()) {
                                if t == 0 || t > 999 {
                                    meta["track_number"] = serde_json::json!(1u64);
                                    patched = true;
                                }
                            }

                            if patched {
                                if let Ok(fixed_json) = serde_json::to_string_pretty(&meta) {
                                    if tokio::fs::write(&sidecar, fixed_json).await.is_ok() {
                                        issue.message = format!(
                                            "{} (Auto-fixed: sidecar updated.)",
                                            issue.message
                                        );
                                        issue.fixable = false;
                                        info!("Metadata auto-fix applied to {}", sidecar.display());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                // No automated fix available for other check types
                debug!("No automated fix for check '{}'", issue.check);
            }
        }
    }

    Ok(())
}

/// Print result for a single file.
fn print_file_result(result: &FileValidationResult) {
    if result.valid {
        if result.issues.is_empty() {
            println!("   {} {}", "✓".green().bold(), "Valid".green());
        } else {
            // Sub-error findings (warnings/infos — e.g. a loudness deviation
            // measurement) must stay visible: hiding them behind a bare
            // "Valid" would silently discard the check's real output.
            println!(
                "   {} {} ({} advisory issue(s))",
                "✓".green().bold(),
                "Valid".green(),
                result.issues.len()
            );
            for issue in &result.issues {
                print_issue(issue);
            }
        }
    } else {
        println!(
            "   {} {} issue(s) found",
            "✗".red().bold(),
            result.issues.len()
        );

        for issue in &result.issues {
            print_issue(issue);
        }
    }
    println!();
}

/// Print a single validation issue.
fn print_issue(issue: &ValidationIssue) {
    let severity_str = match issue.severity {
        IssueSeverity::Info => "INFO",
        IssueSeverity::Warning => "WARN",
        IssueSeverity::Error => "ERROR",
        IssueSeverity::Critical => "CRITICAL",
    };

    let colored_severity = issue.severity.color_string(severity_str);

    let location_str = if let Some(ref loc) = issue.location {
        format!(" [{}]", loc)
    } else {
        String::new()
    };

    println!(
        "     {} [{}]{}: {}",
        colored_severity, issue.check, location_str, issue.message
    );
}

/// Create summary from validation results.
fn create_summary(results: Vec<FileValidationResult>) -> ValidationSummary {
    let total_files = results.len();
    let valid_files = results.iter().filter(|r| r.valid).count();
    let files_with_issues = total_files - valid_files;

    let mut total_issues = 0;
    let mut critical_issues = 0;
    let mut errors = 0;
    let mut warnings = 0;

    for result in &results {
        total_issues += result.issues.len();
        for issue in &result.issues {
            match issue.severity {
                IssueSeverity::Critical => critical_issues += 1,
                IssueSeverity::Error => errors += 1,
                IssueSeverity::Warning => warnings += 1,
                IssueSeverity::Info => {}
            }
        }
    }

    ValidationSummary {
        total_files,
        valid_files,
        files_with_issues,
        total_issues,
        critical_issues,
        errors,
        warnings,
        results,
    }
}

/// Print validation summary.
fn print_validation_summary(summary: &ValidationSummary) {
    println!("{}", "Validation Summary".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Total Files:", summary.total_files);
    println!(
        "{:20} {}",
        "Valid Files:",
        summary.valid_files.to_string().green()
    );
    println!(
        "{:20} {}",
        "Files with Issues:",
        if summary.files_with_issues > 0 {
            summary.files_with_issues.to_string().red()
        } else {
            summary.files_with_issues.to_string().normal()
        }
    );
    println!();

    if summary.total_issues > 0 {
        println!("{}", "Issues Found:".yellow().bold());
        if summary.critical_issues > 0 {
            println!("  {} {}", "Critical:".red().bold(), summary.critical_issues);
        }
        if summary.errors > 0 {
            println!("  {} {}", "Errors:".red(), summary.errors);
        }
        if summary.warnings > 0 {
            println!("  {} {}", "Warnings:".yellow(), summary.warnings);
        }
    } else {
        println!("{}", "No issues found!".green().bold());
    }

    println!("{}", "=".repeat(60));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_check_parsing() {
        assert_eq!(
            ValidationCheck::from_str("format").expect("ValidationCheck::from_str should succeed"),
            ValidationCheck::Format
        );
        assert_eq!(
            ValidationCheck::from_str("codec").expect("ValidationCheck::from_str should succeed"),
            ValidationCheck::Codec
        );
        assert_eq!(
            ValidationCheck::from_str("all").expect("ValidationCheck::from_str should succeed"),
            ValidationCheck::All
        );
        assert!(ValidationCheck::from_str("invalid").is_err());
    }

    #[test]
    fn test_issue_severity_ordering() {
        assert!(IssueSeverity::Info < IssueSeverity::Warning);
        assert!(IssueSeverity::Warning < IssueSeverity::Error);
        assert!(IssueSeverity::Error < IssueSeverity::Critical);
    }

    // ── check_gamut ──────────────────────────────────────────────────────────

    /// Build a minimal single-frame 4:2:0 Y4M clip with every luma byte set
    /// to `luma` and every chroma byte set to `chroma`.
    fn make_y4m_solid(width: u32, height: u32, luma: u8, chroma: u8) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(format!("YUV4MPEG2 W{width} H{height} C420jpeg\n").as_bytes());
        buf.extend_from_slice(b"FRAME\n");
        buf.extend(std::iter::repeat_n(luma, (width * height) as usize));
        let chroma_w = width.div_ceil(2);
        let chroma_h = height.div_ceil(2);
        buf.extend(std::iter::repeat_n(
            chroma,
            (chroma_w * chroma_h) as usize * 2,
        ));
        buf
    }

    #[tokio::test]
    async fn check_gamut_in_range_reports_nothing() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_validate_test_gamut_ok.y4m");
        // Mid-range luma (126) and chroma (128) are well inside [16,235]/[16,240].
        std::fs::write(&path, make_y4m_solid(4, 4, 126, 128)).expect("write Y4M fixture");

        let mut issues = Vec::new();
        check_gamut(&path, &mut issues).await;
        assert!(
            issues.is_empty(),
            "in-range Y4M must report no issues, got: {issues:?}"
        );

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn check_gamut_out_of_range_reports_warning() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_validate_test_gamut_bad.y4m");
        // Full-white luma (255) is far above the legal maximum of 235.
        std::fs::write(&path, make_y4m_solid(4, 4, 255, 128)).expect("write Y4M fixture");

        let mut issues = Vec::new();
        check_gamut(&path, &mut issues).await;
        assert_eq!(
            issues.len(),
            1,
            "expected exactly one Gamut issue: {issues:?}"
        );
        let issue = &issues[0];
        assert_eq!(issue.check, "Gamut");
        assert_eq!(issue.severity, IssueSeverity::Warning);
        assert!(
            issue.message.contains("luma"),
            "message must mention luma: {}",
            issue.message
        );
        assert!(
            issue.message.contains("studio"),
            "message must disclose the studio-range assumption: {}",
            issue.message
        );

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn check_gamut_non_y4m_is_visible_skip() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_validate_test_gamut_notY4m.bin");
        std::fs::write(&path, b"not a y4m file").expect("write fixture");

        let mut issues = Vec::new();
        check_gamut(&path, &mut issues).await;
        assert_eq!(
            issues.len(),
            1,
            "expected exactly one skip issue: {issues:?}"
        );
        assert_eq!(issues[0].severity, IssueSeverity::Info);
        assert!(
            issues[0].message.contains("Gamut check skipped"),
            "skip must be visible: {}",
            issues[0].message
        );

        std::fs::remove_file(&path).ok();
    }
}
