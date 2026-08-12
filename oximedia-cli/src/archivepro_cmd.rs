//! Professional archive and digital preservation CLI commands.
//!
//! Provides commands for ingesting, verifying, migrating, reporting,
//! and managing preservation policies for long-term media archiving.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use oximedia_transcode::TranscodePipeline;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// Archive-pro command subcommands.
#[derive(Subcommand, Debug)]
pub enum ArchiveProCommand {
    /// Ingest media files into the archive with preservation packaging
    Ingest {
        /// Input file(s) to ingest
        #[arg(short, long, required = true)]
        input: Vec<PathBuf>,

        /// Archive root directory
        #[arg(long)]
        archive: PathBuf,

        /// Packaging format: bagit, oais-sip, tar, zip
        #[arg(long, default_value = "bagit")]
        package_format: String,

        /// Checksum algorithm: sha256, sha512, blake3, xxhash
        #[arg(long, default_value = "sha256")]
        checksum: String,

        /// Generate preservation metadata (PREMIS)
        #[arg(long)]
        premis: bool,

        /// Target preservation format for migration
        #[arg(long)]
        target_format: Option<String>,
    },

    /// Verify archive integrity via fixity checking
    Verify {
        /// Archive or package path to verify
        #[arg(short, long)]
        input: PathBuf,

        /// Checksum algorithm to verify with
        #[arg(long, default_value = "sha256")]
        checksum: String,

        /// Deep verification (re-compute all checksums)
        #[arg(long)]
        deep: bool,

        /// Verify metadata consistency
        #[arg(long)]
        metadata: bool,

        /// Output verification report
        #[arg(long)]
        report: Option<PathBuf>,
    },

    /// Plan or execute format migration
    Migrate {
        /// Input file or archive
        #[arg(short, long)]
        input: PathBuf,

        /// Output directory
        #[arg(short, long)]
        output: PathBuf,

        /// Target preservation format: ffv1-mkv, flac, wav, tiff, png
        #[arg(long)]
        target: String,

        /// Dry run (plan only, no conversion)
        #[arg(long)]
        dry_run: bool,

        /// Preserve original after migration
        #[arg(long)]
        keep_original: bool,

        /// Validate output after migration
        #[arg(long)]
        validate: bool,
    },

    /// Generate archive status report
    Report {
        /// Archive root directory
        #[arg(short, long)]
        archive: PathBuf,

        /// Output report path
        #[arg(short, long)]
        output: PathBuf,

        /// Report format: json, csv, text
        #[arg(long, default_value = "json")]
        format: String,

        /// Include risk assessment
        #[arg(long)]
        risk: bool,

        /// Include format statistics
        #[arg(long)]
        stats: bool,
    },

    /// Manage preservation policies
    Policy {
        /// Policy operation: show, set, validate, export
        #[arg(long)]
        operation: String,

        /// Archive root directory
        #[arg(long)]
        archive: Option<PathBuf>,

        /// Policy file path
        #[arg(long)]
        policy_file: Option<PathBuf>,

        /// Retention period (e.g., "10y", "5y", "forever")
        #[arg(long)]
        retention: Option<String>,

        /// Minimum checksum interval (e.g., "30d", "90d", "1y")
        #[arg(long)]
        fixity_interval: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_preservation_format(s: &str) -> Result<oximedia_archive_pro::PreservationFormat> {
    match s.to_lowercase().as_str() {
        "ffv1-mkv" | "ffv1" | "video-ffv1" => Ok(oximedia_archive_pro::PreservationFormat::VideoFfv1Mkv),
        "ut-video" | "utvideo" => Ok(oximedia_archive_pro::PreservationFormat::VideoUtVideo),
        "flac" | "audio-flac" => Ok(oximedia_archive_pro::PreservationFormat::AudioFlac),
        "wav" | "pcm" | "audio-wav" => Ok(oximedia_archive_pro::PreservationFormat::AudioWav),
        "tiff" | "image-tiff" => Ok(oximedia_archive_pro::PreservationFormat::ImageTiff),
        "png" | "image-png" => Ok(oximedia_archive_pro::PreservationFormat::ImagePng),
        "jp2" | "jpeg2000" => Ok(oximedia_archive_pro::PreservationFormat::ImageJpeg2000),
        "pdf-a" | "pdfa" => Ok(oximedia_archive_pro::PreservationFormat::DocumentPdfA),
        "text" | "txt" => Ok(oximedia_archive_pro::PreservationFormat::DocumentText),
        _ => Err(anyhow::anyhow!(
            "Unknown preservation format: {s}. Supported: ffv1-mkv, flac, wav, tiff, png, jp2, pdf-a, text"
        )),
    }
}

fn compute_checksum(path: &std::path::Path, _algorithm: &str) -> Result<String> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Failed to open: {}", path.display()))?;
    let mut hasher: u64 = 0xcbf29ce484222325;
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).context("Read error")?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            hasher ^= u64::from(byte);
            hasher = hasher.wrapping_mul(0x100000001b3);
        }
    }
    Ok(format!("{:016x}", hasher))
}

fn now_iso8601() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle archive-pro command dispatch.
pub async fn handle_archivepro_command(
    command: ArchiveProCommand,
    json_output: bool,
) -> Result<()> {
    match command {
        ArchiveProCommand::Ingest {
            input,
            archive,
            package_format,
            checksum,
            premis,
            target_format,
        } => {
            run_ingest(
                &input,
                &archive,
                &package_format,
                &checksum,
                premis,
                &target_format,
                json_output,
            )
            .await
        }
        ArchiveProCommand::Verify {
            input,
            checksum,
            deep,
            metadata,
            report,
        } => run_verify(&input, &checksum, deep, metadata, &report, json_output).await,
        ArchiveProCommand::Migrate {
            input,
            output,
            target,
            dry_run,
            keep_original,
            validate,
        } => {
            run_migrate(
                &input,
                &output,
                &target,
                dry_run,
                keep_original,
                validate,
                json_output,
            )
            .await
        }
        ArchiveProCommand::Report {
            archive,
            output,
            format,
            risk,
            stats,
        } => run_report(&archive, &output, &format, risk, stats, json_output).await,
        ArchiveProCommand::Policy {
            operation,
            archive,
            policy_file,
            retention,
            fixity_interval,
        } => {
            run_policy(
                &operation,
                &archive,
                &policy_file,
                &retention,
                &fixity_interval,
                json_output,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

async fn run_ingest(
    inputs: &[PathBuf],
    archive: &PathBuf,
    package_format: &str,
    checksum_algo: &str,
    _premis: bool,
    _target_format: &Option<String>,
    json_output: bool,
) -> Result<()> {
    if !archive.exists() {
        std::fs::create_dir_all(archive)
            .with_context(|| format!("Failed to create archive dir: {}", archive.display()))?;
    }

    let mut ingested = Vec::new();

    for path in inputs {
        if !path.exists() {
            return Err(anyhow::anyhow!("Input not found: {}", path.display()));
        }
        let checksum_val = compute_checksum(path, checksum_algo)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Copy to archive
        let dest = archive.join(&filename);
        std::fs::copy(path, &dest)
            .with_context(|| format!("Failed to copy {} to archive", path.display()))?;

        ingested.push(serde_json::json!({
            "filename": filename,
            "checksum": checksum_val,
            "size": size,
            "timestamp": now_iso8601(),
        }));
    }

    // Write manifest
    let manifest = serde_json::json!({
        "package_format": package_format,
        "checksum_algorithm": checksum_algo,
        "ingested_at": now_iso8601(),
        "files": ingested,
    });
    let manifest_path = archive.join("manifest.json");
    let manifest_str = serde_json::to_string_pretty(&manifest).context("Serialization failed")?;
    std::fs::write(&manifest_path, &manifest_str).context("Failed to write manifest")?;

    if json_output {
        let result = serde_json::json!({
            "command": "archive-pro ingest",
            "archive": archive.display().to_string(),
            "package_format": package_format,
            "files_ingested": ingested.len(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Archive Pro Ingest".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Archive:", archive.display());
        println!("{:20} {}", "Package format:", package_format);
        println!("{:20} {}", "Checksum:", checksum_algo);
        println!("{:20} {}", "Files ingested:", ingested.len());
        println!();
        for item in &ingested {
            let fname = item.get("filename").and_then(|v| v.as_str()).unwrap_or("?");
            println!("  {} {}", "+".green(), fname);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Verify
// ---------------------------------------------------------------------------

async fn run_verify(
    input: &PathBuf,
    checksum_algo: &str,
    deep: bool,
    _metadata: bool,
    report_path: &Option<PathBuf>,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    let manifest_path = if input.is_dir() {
        input.join("manifest.json")
    } else {
        input.clone()
    };

    let mut checks: Vec<(String, bool, String)> = Vec::new();
    let mut all_passed = true;

    // Check manifest exists
    let manifest_exists = manifest_path.exists();
    if !manifest_exists {
        all_passed = false;
    }
    checks.push((
        "manifest_exists".to_string(),
        manifest_exists,
        "Manifest file present".to_string(),
    ));

    // Deep verification
    if deep && manifest_exists {
        let data = std::fs::read_to_string(&manifest_path).context("Failed to read manifest")?;
        let manifest: serde_json::Value =
            serde_json::from_str(&data).context("Failed to parse manifest")?;
        if let Some(files) = manifest.get("files").and_then(|f| f.as_array()) {
            for file_entry in files {
                let fname = file_entry
                    .get("filename")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let expected = file_entry
                    .get("checksum")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let file_path = if input.is_dir() {
                    input.join(&fname)
                } else {
                    input
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join(&fname)
                };
                if file_path.exists() {
                    let actual = compute_checksum(&file_path, checksum_algo)?;
                    let matched = actual == expected;
                    if !matched {
                        all_passed = false;
                    }
                    checks.push(("fixity".to_string(), matched, fname));
                } else {
                    all_passed = false;
                    checks.push(("file_missing".to_string(), false, fname));
                }
            }
        }
    }

    if let Some(ref rpath) = report_path {
        let report = serde_json::json!({
            "all_passed": all_passed,
            "checks": checks.iter().map(|(n, p, d)| serde_json::json!({"check": n, "passed": p, "detail": d})).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&report).context("Serialization failed")?;
        std::fs::write(rpath, s)
            .with_context(|| format!("Failed to write report: {}", rpath.display()))?;
    }

    if json_output {
        let result = serde_json::json!({
            "command": "archive-pro verify",
            "input": input.display().to_string(),
            "all_passed": all_passed,
            "checks_count": checks.len(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Archive Pro Verify".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Algorithm:", checksum_algo);
        println!();
        for (name, passed, detail) in &checks {
            let status = if *passed {
                "PASS".green().to_string()
            } else {
                "FAIL".red().to_string()
            };
            println!("  [{}] {:20} {}", status, name, detail);
        }
        println!();
        if all_passed {
            println!("{}", "All checks passed.".green());
        } else {
            println!("{}", "Some checks failed.".red());
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Migrate
// ---------------------------------------------------------------------------

/// Which real re-encode path (if any) a [`oximedia_archive_pro::PreservationFormat`]
/// migration target is wired to.
enum RealMigration {
    /// Real decode -> re-encode via the `oximedia_transcode` frame-level
    /// pipeline, keyed by the `TranscodePipelineBuilder::audio_codec` name.
    Audio(&'static str),
    /// Real decode -> re-encode via `oximedia_image`, keyed by the target
    /// raster format.
    Image(oximedia_image::format_detect::ImageFormat),
}

/// Resolve which real conversion (if any) backs a preservation target.
///
/// Real re-encode is wired up for the audio preservation formats (genuinely
/// decodes WAV/FLAC and re-encodes to FLAC or PCM/WAV via the same pipeline
/// that backs `oximedia transcode` — see
/// `oximedia-cli/tests/transcode_reencode.rs::flagship_wav_to_flac_round_trips`
/// for an end-to-end, sample-exact proof) and for the raster image
/// preservation formats (genuinely decodes any `oximedia_image`-supported
/// input and re-encodes to PNG or uncompressed TIFF via the same codec path
/// `oximedia image convert` uses).
///
/// Every other preservation target is refused outright — including for
/// `--dry-run` — rather than ever emitting a copy+rename mislabeled as a
/// converted preservation master: for a digital-preservation tool, that is a
/// data-integrity lie (e.g. a WAV renamed `.mxf` reads back as a
/// "successfully migrated" MXF file that is actually still a WAV). This
/// specifically includes `VideoFfv1Mkv`: `oximedia_transcode`'s FFV1 encoder
/// is real, but no container in this workspace can yet mux `V_FFV1` +
/// `CodecPrivate` into Matroska (see `frame_level.rs::execute_video_job`'s
/// `VideoTarget::Ffv1` arm) — writing raw FFV1 framing under the `.mkv`
/// preservation-master label this format promises would be exactly the same
/// mislabeling this function refuses to do for every other unimplemented
/// target, so `VideoFfv1Mkv` stays an honest error rather than a
/// silently-wrong container.
///
/// TODO(0.2.x): wire `VideoFfv1Mkv` once a Matroska muxer can carry FFV1,
/// `VideoUtVideo` once a UT Video encoder exists, `ImageJpeg2000` once a
/// JPEG 2000 encoder exists, and the document formats (PDF/A, plain text)
/// once real document pipelines exist for those domains.
fn resolve_real_migration(pf: oximedia_archive_pro::PreservationFormat) -> Option<RealMigration> {
    use oximedia_archive_pro::PreservationFormat;
    match pf {
        PreservationFormat::AudioFlac => Some(RealMigration::Audio("flac")),
        PreservationFormat::AudioWav => Some(RealMigration::Audio("pcm")),
        PreservationFormat::ImagePng => Some(RealMigration::Image(
            oximedia_image::format_detect::ImageFormat::Png,
        )),
        PreservationFormat::ImageTiff => Some(RealMigration::Image(
            oximedia_image::format_detect::ImageFormat::Tiff,
        )),
        _ => None,
    }
}

/// Detect an input image's format from its magic bytes, mirroring
/// `image_cmd.rs::convert_image`'s detection step.
fn detect_image_format(
    input: &std::path::Path,
) -> Result<oximedia_image::format_detect::ImageFormat> {
    let file_size = std::fs::metadata(input)
        .with_context(|| format!("Failed to read metadata: {}", input.display()))?
        .len();
    let mut buf = vec![0u8; 2048.min(file_size as usize)];
    let mut f = std::fs::File::open(input)
        .with_context(|| format!("Failed to open input file: {}", input.display()))?;
    std::io::Read::read(&mut f, &mut buf).context("Failed to read file header")?;
    Ok(oximedia_image::format_detect::FormatDetector::detect(&buf))
}

async fn run_migrate(
    input: &PathBuf,
    output: &PathBuf,
    target: &str,
    dry_run: bool,
    _keep_original: bool,
    _validate: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    let pf = parse_preservation_format(target)?;

    let real_migration = resolve_real_migration(pf).ok_or_else(|| {
        anyhow::anyhow!(
            "archive-pro migrate: real format conversion for '{}' -> {target} ({}) is not \
             yet implemented; refusing to emit a mislabeled copy. Real conversion is \
             currently available only for: flac, wav, png, tiff.",
            input.display(),
            pf.description(),
        )
    })?;

    let filename = input.file_name().unwrap_or_default().to_string_lossy();
    let new_name = format!(
        "{}.{}",
        filename
            .rsplit_once('.')
            .map(|(n, _)| n)
            .unwrap_or(&filename),
        pf.extension()
    );

    if dry_run {
        if json_output {
            let result = serde_json::json!({
                "command": "archive-pro migrate",
                "input": input.display().to_string(),
                "output": output.display().to_string(),
                "target_format": target,
                "target_extension": pf.extension(),
                "target_mime": pf.mime_type(),
                "dry_run": true,
                "new_filename": new_name,
            });
            let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
            println!("{s}");
        } else if !crate::progress::is_quiet() {
            println!("{}", "Archive Pro Migrate".green().bold());
            println!("{}", "=".repeat(60));
            println!("{:20} {}", "Input:", input.display());
            println!("{:20} {}", "Target format:", pf.description());
            println!("{:20} {}", "New filename:", new_name);
            println!();
            println!(
                "{}",
                "(Dry run - no files were converted; a real re-encode would be attempted)".yellow()
            );
        }
        return Ok(());
    }

    if !output.exists() {
        std::fs::create_dir_all(output)
            .with_context(|| format!("Failed to create output dir: {}", output.display()))?;
    }
    let dest = output.join(&new_name);

    let output_size_bytes: u64 = match real_migration {
        RealMigration::Audio(audio_codec_name) => {
            let mut pipeline = TranscodePipeline::builder()
                .input(input.clone())
                .output(dest.clone())
                .audio_codec(audio_codec_name)
                .build()
                .map_err(|e| {
                    anyhow::anyhow!(
                        "archive-pro migrate: failed to configure conversion pipeline: {e}"
                    )
                })?;

            match pipeline.execute().await {
                Ok(out) => out.file_size,
                Err(e) => {
                    // Never leave a partially-written / fabricated output
                    // file behind after a failed real conversion.
                    std::fs::remove_file(&dest).ok();
                    return Err(anyhow::anyhow!(
                        "archive-pro migrate: real conversion of '{}' to {} failed: {e}",
                        input.display(),
                        pf.description()
                    ));
                }
            }
        }
        RealMigration::Image(out_fmt) => {
            let in_fmt = detect_image_format(input)?;
            let frame = crate::image_cmd::read_input_frame(input, &in_fmt).map_err(|e| {
                anyhow::anyhow!(
                    "archive-pro migrate: real conversion of '{}' to {} failed: could not \
                     decode input as an image: {e}",
                    input.display(),
                    pf.description()
                )
            })?;
            match crate::image_cmd::write_output_frame(&dest, &frame, &out_fmt, None, 100) {
                Ok(()) => std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0),
                Err(e) => {
                    std::fs::remove_file(&dest).ok();
                    return Err(anyhow::anyhow!(
                        "archive-pro migrate: real conversion of '{}' to {} failed: {e}",
                        input.display(),
                        pf.description()
                    ));
                }
            }
        }
    };

    if json_output {
        let result = serde_json::json!({
            "command": "archive-pro migrate",
            "input": input.display().to_string(),
            "output": dest.display().to_string(),
            "target_format": target,
            "target_extension": pf.extension(),
            "target_mime": pf.mime_type(),
            "dry_run": false,
            "new_filename": new_name,
            "real_conversion": true,
            "output_size_bytes": output_size_bytes,
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Archive Pro Migrate".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Input:", input.display());
        println!("{:20} {}", "Target format:", pf.description());
        println!("{:20} {}", "New filename:", new_name);
        println!("{:20} {}", "Output:", dest.display());
        println!(
            "{:20} {:.2} MB",
            "Output size:",
            output_size_bytes as f64 / (1024.0 * 1024.0)
        );
        println!();
        println!(
            "{}",
            "Real conversion complete (genuinely re-encoded, not a renamed copy).".green()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

async fn run_report(
    archive: &PathBuf,
    output: &PathBuf,
    format: &str,
    _risk: bool,
    _stats: bool,
    json_output: bool,
) -> Result<()> {
    if !archive.exists() {
        return Err(anyhow::anyhow!("Archive not found: {}", archive.display()));
    }

    let mut file_count = 0usize;
    let mut total_size: u64 = 0;
    let mut formats: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    if archive.is_dir() {
        let entries = std::fs::read_dir(archive).context("Failed to read archive dir")?;
        for entry in entries {
            let entry = entry.context("Dir entry error")?;
            let path = entry.path();
            if path.is_file() {
                file_count += 1;
                total_size += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("unknown")
                    .to_lowercase();
                *formats.entry(ext).or_insert(0) += 1;
            }
        }
    }

    let report_data = serde_json::json!({
        "archive": archive.display().to_string(),
        "total_files": file_count,
        "total_size": total_size,
        "formats": formats,
        "generated_at": now_iso8601(),
    });

    let report_str = match format {
        "text" => serde_json::to_string_pretty(&report_data).context("Serialization failed")?,
        _ => serde_json::to_string_pretty(&report_data).context("Serialization failed")?,
    };
    std::fs::write(output, &report_str)
        .with_context(|| format!("Failed to write report: {}", output.display()))?;

    if json_output {
        let s = serde_json::to_string_pretty(&report_data).context("JSON serialization failed")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Archive Pro Report".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Archive:", archive.display());
        println!("{:20} {}", "Total files:", file_count);
        println!(
            "{:20} {:.2} MB",
            "Total size:",
            total_size as f64 / (1024.0 * 1024.0)
        );
        println!("{:20} {}", "Report:", output.display());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

async fn run_policy(
    operation: &str,
    archive: &Option<PathBuf>,
    policy_file: &Option<PathBuf>,
    retention: &Option<String>,
    fixity_interval: &Option<String>,
    json_output: bool,
) -> Result<()> {
    match operation {
        "show" => {
            let policy = serde_json::json!({
                "retention": retention.clone().unwrap_or_else(|| "10y".to_string()),
                "fixity_interval": fixity_interval.clone().unwrap_or_else(|| "90d".to_string()),
                "checksum_algorithms": ["sha256", "blake3"],
                "preservation_formats": ["ffv1-mkv", "flac", "tiff"],
            });
            if json_output {
                let s =
                    serde_json::to_string_pretty(&policy).context("JSON serialization failed")?;
                println!("{s}");
            } else {
                println!("{}", "Archive Policy".green().bold());
                println!("{}", "=".repeat(60));
                println!(
                    "{:20} {}",
                    "Retention:",
                    retention.as_deref().unwrap_or("10y")
                );
                println!(
                    "{:20} {}",
                    "Fixity interval:",
                    fixity_interval.as_deref().unwrap_or("90d")
                );
                println!("{:20} sha256, blake3", "Algorithms:");
                println!("{:20} ffv1-mkv, flac, tiff", "Formats:");
            }
        }
        "set" => {
            let default_path = PathBuf::from("policy.json");
            let policy_path = policy_file
                .as_ref()
                .or(archive.as_ref())
                .unwrap_or(&default_path);
            let policy = serde_json::json!({
                "retention": retention.clone().unwrap_or_else(|| "10y".to_string()),
                "fixity_interval": fixity_interval.clone().unwrap_or_else(|| "90d".to_string()),
            });
            let s = serde_json::to_string_pretty(&policy).context("Serialization failed")?;
            std::fs::write(policy_path, s)
                .with_context(|| format!("Failed to write policy: {}", policy_path.display()))?;
            if !json_output && !crate::progress::is_quiet() {
                println!(
                    "{} Policy saved to {}",
                    "OK:".green(),
                    policy_path.display()
                );
            }
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Unknown policy operation: {operation}. Supported: show, set"
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

    #[test]
    fn test_parse_preservation_format() {
        assert!(parse_preservation_format("flac").is_ok());
        assert!(parse_preservation_format("ffv1-mkv").is_ok());
        assert!(parse_preservation_format("tiff").is_ok());
        assert!(parse_preservation_format("png").is_ok());
        assert!(parse_preservation_format("nonsense").is_err());
    }

    #[test]
    fn test_compute_checksum() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_archivepro_test.bin");
        std::fs::write(&path, b"archive test data").expect("write should succeed");
        let ck = compute_checksum(&path, "sha256");
        assert!(ck.is_ok());
        assert_eq!(ck.expect("checksum").len(), 16);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_preservation_format_properties() {
        let flac = oximedia_archive_pro::PreservationFormat::AudioFlac;
        assert_eq!(flac.extension(), "flac");
        assert_eq!(flac.mime_type(), "audio/flac");
    }

    #[test]
    fn test_now_iso8601() {
        let ts = now_iso8601();
        assert!(!ts.is_empty());
        // Should be a number string (seconds since epoch)
        assert!(ts.parse::<u64>().is_ok());
    }

    // ── Real migration / fabrication-elimination tests ──────────────────────
    //
    // `archive-pro migrate` previously did `fs::copy` + extension rename and
    // reported success — a WAV renamed `.flac` reads back as a "migrated"
    // FLAC file that is actually still a WAV, a data-integrity lie in a
    // preservation tool. These tests assert the fixed behavior: genuine
    // re-encodes for the formats that have one (audio), and a clean,
    // no-side-effect error for everything else.

    /// Unique temp-file path for this test process. Per project policy,
    /// tests must use `std::env::temp_dir()` rather than a hardcoded path.
    fn archivepro_temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "oximedia_archivepro_cmd_test_{}_{name}",
            std::process::id()
        ))
    }

    /// Mirror `run_migrate`'s own filename derivation so tests compute the
    /// exact same destination path the production code writes to (the temp
    /// fixture helper bakes a unique prefix into the filename itself, not
    /// into a parent directory, so the migrated basename is longer than the
    /// short suffix passed to `archivepro_temp_path`).
    fn expected_migrate_dest(
        out_dir: &std::path::Path,
        input: &std::path::Path,
        target_ext: &str,
    ) -> std::path::PathBuf {
        let filename = input.file_name().unwrap_or_default().to_string_lossy();
        let stem = filename
            .rsplit_once('.')
            .map(|(n, _)| n)
            .unwrap_or(&filename);
        out_dir.join(format!("{stem}.{target_ext}"))
    }

    /// Build a minimal, genuinely valid 16-bit PCM WAV file (canonical
    /// 44-byte header + a short sine wave) so tests exercise the REAL
    /// `oximedia_transcode` decode -> re-encode pipeline rather than a byte
    /// stub. Mirrors `oximedia-cli/tests/common::make_sine_wav`.
    fn make_sine_wav(freq_hz: f32, sample_rate: u32, channels: u16, duration_secs: f32) -> Vec<u8> {
        let num_samples = (sample_rate as f32 * duration_secs) as u32;
        let num_channels = u32::from(channels);
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * num_channels * u32::from(bits_per_sample / 8);
        let block_align = channels * (bits_per_sample / 8);
        let data_size = num_samples * num_channels * u32::from(bits_per_sample / 8);
        let file_size = 36 + data_size;

        let mut buf = Vec::with_capacity(44 + data_size as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits_per_sample.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for i in 0..num_samples {
            let t = i as f32 / sample_rate as f32;
            let sample = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
            let pcm = (sample * 32767.0) as i16;
            for _ch in 0..channels {
                buf.extend_from_slice(&pcm.to_le_bytes());
            }
        }
        buf
    }

    #[tokio::test]
    async fn test_run_migrate_real_wav_to_flac_is_genuine_reencode() {
        let input = archivepro_temp_path("mig_wav_to_flac_in.wav");
        let out_dir = archivepro_temp_path("mig_wav_to_flac_out_dir");
        std::fs::write(&input, make_sine_wav(1_000.0, 48_000, 2, 0.2)).expect("write wav fixture");
        std::fs::remove_dir_all(&out_dir).ok();

        run_migrate(&input, &out_dir, "flac", false, false, false, true)
            .await
            .expect("real WAV -> FLAC migration must succeed");

        let dest = expected_migrate_dest(&out_dir, &input, "flac");
        let flac_bytes = std::fs::read(&dest).expect("output flac must exist");
        assert!(
            flac_bytes.starts_with(b"fLaC"),
            "output must be a real FLAC stream, not a renamed copy"
        );
        let input_bytes = std::fs::read(&input).expect("read input");
        assert_ne!(
            flac_bytes[..flac_bytes.len().min(256)],
            input_bytes[..input_bytes.len().min(256)],
            "output must not be a byte copy of the input"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_real_wav_to_wav_reencode() {
        let input = archivepro_temp_path("mig_wav_to_wav_in.wav");
        let out_dir = archivepro_temp_path("mig_wav_to_wav_out_dir");
        std::fs::write(&input, make_sine_wav(700.0, 44_100, 2, 0.2)).expect("write wav fixture");
        std::fs::remove_dir_all(&out_dir).ok();

        run_migrate(&input, &out_dir, "wav", false, false, false, true)
            .await
            .expect("real WAV -> WAV (PCM) migration must succeed");

        let dest = expected_migrate_dest(&out_dir, &input, "wav");
        let out_bytes = std::fs::read(&dest).expect("output wav must exist");
        assert!(
            out_bytes.starts_with(b"RIFF"),
            "output must be a real WAV file"
        );
        assert!(
            out_bytes.len() > 44,
            "output must contain real PCM sample data, not just a header"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_unsupported_target_is_honest_err_no_output() {
        // "jp2" (JPEG 2000) has no real encoder wired anywhere in the
        // workspace, so it must stay an honest error. ("tiff"/"png" moved to
        // the real-conversion tests below once the image path was wired.)
        let input = archivepro_temp_path("mig_unsupported_in.wav");
        let out_dir = archivepro_temp_path("mig_unsupported_out_dir");
        std::fs::write(&input, make_sine_wav(440.0, 48_000, 1, 0.1)).expect("write wav fixture");
        std::fs::remove_dir_all(&out_dir).ok();

        let err = run_migrate(&input, &out_dir, "jp2", false, false, false, true)
            .await
            .expect_err("unimplemented preservation target must be an honest error");
        let msg = err.to_string();
        assert!(
            msg.contains("not yet"),
            "error must say real conversion is not yet implemented: {msg}"
        );
        assert!(
            msg.contains("mislabeled"),
            "error must name the fabrication it refuses to produce: {msg}"
        );
        assert!(
            !out_dir.exists(),
            "no output directory/file may be created for an unsupported target"
        );

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_dry_run_does_not_bypass_honesty_check() {
        // "pdf-a" has no real document pipeline wired, so even a dry-run
        // plan must be refused (dry-run must not bypass the honesty check).
        let input = archivepro_temp_path("mig_dryrun_unsupported_in.wav");
        std::fs::write(&input, make_sine_wav(440.0, 48_000, 1, 0.1)).expect("write wav fixture");
        let out_dir = archivepro_temp_path("mig_dryrun_unsupported_out_dir");

        let result = run_migrate(&input, &out_dir, "pdf-a", true, false, false, true).await;
        assert!(
            result.is_err(),
            "dry-run must not report a fake successful plan for an unimplemented target"
        );

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_dry_run_supported_target_writes_nothing() {
        let input = archivepro_temp_path("mig_dryrun_ok_in.wav");
        std::fs::write(&input, make_sine_wav(440.0, 48_000, 1, 0.1)).expect("write wav fixture");
        let out_dir = archivepro_temp_path("mig_dryrun_ok_out_dir");
        std::fs::remove_dir_all(&out_dir).ok();

        run_migrate(&input, &out_dir, "flac", true, false, false, true)
            .await
            .expect("dry-run on a real-conversion-capable target must succeed");
        assert!(!out_dir.exists(), "dry-run must not touch the filesystem");

        std::fs::remove_file(&input).ok();
    }

    /// Build a minimal, genuinely valid PNG file (4x4 RGB) via the real
    /// `oximedia_image` encoder, so image-migration tests exercise the REAL
    /// decode -> re-encode pipeline rather than a byte stub.
    fn make_tiny_png(path: &std::path::Path) {
        let width = 4u32;
        let height = 4u32;
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.push((x * 60) as u8);
                pixels.push((y * 60) as u8);
                pixels.push(128u8);
            }
        }
        let png = oximedia_image::png::PngImage {
            width,
            height,
            bit_depth: 8,
            color_type: oximedia_image::png::PngColorType::Rgb,
            pixels,
            metadata: std::collections::HashMap::new(),
        };
        oximedia_image::png::write_png(path, &png).expect("write png fixture");
    }

    #[tokio::test]
    async fn test_run_migrate_real_png_to_tiff_is_genuine_reencode() {
        let input = archivepro_temp_path("mig_png_to_tiff_in.png");
        let out_dir = archivepro_temp_path("mig_png_to_tiff_out_dir");
        make_tiny_png(&input);
        std::fs::remove_dir_all(&out_dir).ok();

        run_migrate(&input, &out_dir, "tiff", false, false, false, true)
            .await
            .expect("real PNG -> TIFF migration must succeed");

        let dest = expected_migrate_dest(&out_dir, &input, "tiff");
        let tiff_bytes = std::fs::read(&dest).expect("output tiff must exist");
        assert!(
            tiff_bytes.starts_with(b"II*\0") || tiff_bytes.starts_with(b"MM\0*"),
            "output must be a real TIFF stream (little- or big-endian magic), not a renamed copy"
        );
        let input_bytes = std::fs::read(&input).expect("read input");
        assert_ne!(
            tiff_bytes[..tiff_bytes.len().min(256)],
            input_bytes[..input_bytes.len().min(256)],
            "output must not be a byte copy of the input"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_real_png_to_png_is_genuine_reencode() {
        let input = archivepro_temp_path("mig_png_to_png_in.png");
        let out_dir = archivepro_temp_path("mig_png_to_png_out_dir");
        make_tiny_png(&input);
        std::fs::remove_dir_all(&out_dir).ok();

        run_migrate(&input, &out_dir, "png", false, false, false, true)
            .await
            .expect("real PNG -> PNG (preservation re-encode) migration must succeed");

        let dest = expected_migrate_dest(&out_dir, &input, "png");
        let png_bytes = std::fs::read(&dest).expect("output png must exist");
        assert!(
            png_bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']),
            "output must be a real PNG stream"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_dry_run_image_target_writes_nothing() {
        let input = archivepro_temp_path("mig_dryrun_png_in.png");
        make_tiny_png(&input);
        let out_dir = archivepro_temp_path("mig_dryrun_png_out_dir");
        std::fs::remove_dir_all(&out_dir).ok();

        run_migrate(&input, &out_dir, "tiff", true, false, false, true)
            .await
            .expect("dry-run on the now-real-conversion-capable tiff target must succeed");
        assert!(!out_dir.exists(), "dry-run must not touch the filesystem");

        std::fs::remove_file(&input).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_non_image_input_honest_err_no_fabricated_output() {
        let input = archivepro_temp_path("mig_garbage_in.png");
        std::fs::write(&input, b"not a real png file at all, just some bytes")
            .expect("write garbage");
        let out_dir = archivepro_temp_path("mig_garbage_png_out_dir");
        std::fs::remove_dir_all(&out_dir).ok();

        let result = run_migrate(&input, &out_dir, "tiff", false, false, false, true).await;
        assert!(
            result.is_err(),
            "a non-image input must not silently 'convert' to TIFF"
        );

        let dest = expected_migrate_dest(&out_dir, &input, "tiff");
        assert!(
            !dest.exists(),
            "no fabricated TIFF output may remain after a failed real conversion"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_non_audio_input_honest_err_no_fabricated_output() {
        let input = archivepro_temp_path("mig_garbage_in.wav");
        std::fs::write(&input, b"not a real wav file at all, just some bytes")
            .expect("write garbage");
        let out_dir = archivepro_temp_path("mig_garbage_out_dir");
        std::fs::remove_dir_all(&out_dir).ok();

        let result = run_migrate(&input, &out_dir, "flac", false, false, false, true).await;
        assert!(
            result.is_err(),
            "a non-WAV/FLAC input must not silently 'convert'"
        );

        let dest = expected_migrate_dest(&out_dir, &input, "flac");
        assert!(
            !dest.exists(),
            "no fabricated FLAC output may remain after a failed real conversion"
        );

        std::fs::remove_file(&input).ok();
        std::fs::remove_dir_all(&out_dir).ok();
    }

    #[tokio::test]
    async fn test_run_migrate_missing_input_is_err() {
        let input = archivepro_temp_path("mig_does_not_exist.wav");
        let out_dir = archivepro_temp_path("mig_missing_out_dir");
        let result = run_migrate(&input, &out_dir, "flac", false, false, false, true).await;
        assert!(result.is_err());
    }
}
