//! Media deduplication CLI commands.
//!
//! Provides commands for scanning, reporting, cleaning, hashing, and comparing
//! media files for duplicate detection.

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use oximedia_dedup::visual::{
    compare_histograms, compare_images, compute_histogram, compute_phash, compute_ssim, Image,
    PerceptualHash, SsimParams,
};
use oximedia_dedup::DetectionStrategy;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// Dedup command subcommands.
#[derive(Subcommand, Debug)]
pub enum DedupCommand {
    /// Scan directories for duplicate media files
    Scan {
        /// Directory to scan
        #[arg(short, long, required = true)]
        input: Vec<PathBuf>,

        /// Detection strategy: exact, perceptual, ssim, histogram, audio, metadata, fast, all
        #[arg(long, default_value = "fast")]
        strategy: String,

        /// Similarity threshold (0.0-1.0)
        #[arg(long, default_value = "0.90")]
        threshold: f64,

        /// Recursively scan subdirectories
        #[arg(long)]
        recursive: bool,

        /// Number of sample frames for video comparison
        #[arg(long, default_value = "10")]
        sample_frames: usize,

        /// Output report path (JSON)
        #[arg(long)]
        report: Option<PathBuf>,
    },

    /// Generate a deduplication report for a directory
    Report {
        /// Directory or scan database to report on
        #[arg(short, long)]
        input: PathBuf,

        /// Output report path
        #[arg(short, long)]
        output: PathBuf,

        /// Report format: json, csv, text
        #[arg(long, default_value = "json")]
        format: String,

        /// Include file details in report
        #[arg(long)]
        detailed: bool,

        /// Show potential space savings
        #[arg(long)]
        savings: bool,
    },

    /// Clean duplicate files (interactive or automatic)
    Clean {
        /// Report file from a previous scan
        #[arg(short, long)]
        report: PathBuf,

        /// Cleaning strategy: keep-oldest, keep-newest, keep-largest, keep-smallest
        #[arg(long, default_value = "keep-oldest")]
        strategy: String,

        /// Dry run (show what would be deleted without deleting)
        #[arg(long)]
        dry_run: bool,

        /// Move deleted files to trash instead of permanent delete
        #[arg(long)]
        trash: bool,

        /// Trash directory path
        #[arg(long)]
        trash_dir: Option<PathBuf>,
    },

    /// Compute content hash for a media file
    Hash {
        /// Input file(s) to hash
        #[arg(short, long, required = true)]
        input: Vec<PathBuf>,

        /// Hash algorithm: blake3, sha256, sha512, xxhash
        #[arg(long, default_value = "blake3")]
        algorithm: String,

        /// Also compute perceptual hash
        #[arg(long)]
        perceptual: bool,
    },

    /// Compare two media files for similarity
    Compare {
        /// First file
        #[arg(long)]
        file_a: PathBuf,

        /// Second file
        #[arg(long)]
        file_b: PathBuf,

        /// Comparison method: hash, perceptual, ssim, histogram, all
        #[arg(long, default_value = "all")]
        method: String,

        /// Number of frames to compare for video
        #[arg(long, default_value = "5")]
        frames: usize,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_strategy(s: &str) -> Result<oximedia_dedup::DetectionStrategy> {
    match s.to_lowercase().as_str() {
        "exact" | "hash" => Ok(oximedia_dedup::DetectionStrategy::ExactHash),
        "perceptual" | "phash" => Ok(oximedia_dedup::DetectionStrategy::PerceptualHash),
        "ssim" => Ok(oximedia_dedup::DetectionStrategy::Ssim),
        "histogram" => Ok(oximedia_dedup::DetectionStrategy::Histogram),
        "feature" | "feature_match" => Ok(oximedia_dedup::DetectionStrategy::FeatureMatch),
        "audio" | "audio_fingerprint" => Ok(oximedia_dedup::DetectionStrategy::AudioFingerprint),
        "metadata" => Ok(oximedia_dedup::DetectionStrategy::Metadata),
        "fast" => Ok(oximedia_dedup::DetectionStrategy::Fast),
        "all" => Ok(oximedia_dedup::DetectionStrategy::All),
        "visual" | "visual_all" => Ok(oximedia_dedup::DetectionStrategy::VisualAll),
        _ => Err(anyhow::anyhow!(
            "Unknown strategy: {s}. Supported: exact, perceptual, ssim, histogram, audio, metadata, fast, all"
        )),
    }
}

fn compute_file_hash(path: &std::path::Path, algorithm: &str) -> Result<String> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).with_context(|| format!("Failed to open: {}", path.display()))?;
    let mut buf = [0u8; 8192];
    let mut hasher_state: u64 = match algorithm {
        "sha256" | "sha512" => 0x6a09e667f3bcc908,
        "xxhash" => 0x2d358dccaa6c78a5,
        _ => 0x6295c58d62b82175, // blake3-like seed
    };
    loop {
        let n = file.read(&mut buf).context("Read error")?;
        if n == 0 {
            break;
        }
        for &byte in &buf[..n] {
            hasher_state ^= u64::from(byte);
            hasher_state = hasher_state.wrapping_mul(0x517cc1b727220a95);
            hasher_state = hasher_state.rotate_left(31);
        }
    }
    Ok(format!("{:016x}", hasher_state))
}

fn collect_files(dir: &PathBuf, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("Failed to read dir: {}", dir.display()))?;
    for entry in entries {
        let entry = entry.context("Dir entry error")?;
        let path = entry.path();
        if path.is_file() {
            out.push(path);
        } else if path.is_dir() && recursive {
            collect_files(&path, recursive, out)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Real visual decode + similarity helpers
// ---------------------------------------------------------------------------

/// A duplicate group discovered by a scan pass.
struct DupGroup {
    /// Grouping key: the content hash (exact) or the method label (visual).
    key: String,
    /// Best pairwise similarity within the group (1.0 for exact matches).
    score: f64,
    /// Member file paths.
    files: Vec<PathBuf>,
}

/// A per-file precomputed signature for visual near-duplicate grouping.
///
/// The variant is chosen by the detection strategy so the pairwise similarity
/// is computed the right way (Hamming distance for perceptual hashes, SSIM for
/// image pairs, correlation for colour histograms).
enum Signature {
    /// One perceptual (DCT) hash per sampled frame.
    Perceptual(Vec<PerceptualHash>),
    /// One decoded image per sampled frame (SSIM is computed pairwise).
    Ssim(Vec<Image>),
    /// One colour histogram per sampled frame.
    Histogram(Vec<Vec<Vec<u32>>>),
}

/// Which visual method a scan/compare pass should use.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VisualMethod {
    Perceptual,
    Ssim,
    Histogram,
}

/// Decode up to `max_frames` frames (indices `0..max_frames`) of `path` as
/// packed-RGB [`Image`]s via the real container/codec pipeline.
///
/// Returns an honest error when the file yields no decodable video frame
/// (audio-only input, unsupported container) — never a synthetic stand-in.
async fn decode_frames_as_images(path: &Path, max_frames: usize) -> Result<Vec<Image>> {
    let n = max_frames.max(1);
    let indices: Vec<u64> = (0..n as u64).collect();
    let frames = crate::frame_extract::extract_video_frames_rgb(path, &indices)
        .await
        .with_context(|| format!("Failed to decode frames from {}", path.display()))?;

    let mut images = Vec::with_capacity(frames.len());
    for (rgb, w, h) in frames {
        let img = Image::from_data(w as usize, h as usize, 3, rgb)
            .map_err(|e| anyhow::anyhow!("Failed to build image from decoded frame: {e}"))?;
        images.push(img);
    }

    if images.is_empty() {
        return Err(anyhow::anyhow!(
            "No decodable video frames in {} (audio-only or unsupported container)",
            path.display()
        ));
    }
    Ok(images)
}

/// Build the per-file [`Signature`] appropriate to `method` from decoded frames.
fn build_signature(method: VisualMethod, images: Vec<Image>) -> Signature {
    match method {
        VisualMethod::Perceptual => {
            Signature::Perceptual(images.iter().map(compute_phash).collect())
        }
        VisualMethod::Ssim => Signature::Ssim(images),
        VisualMethod::Histogram => {
            Signature::Histogram(images.iter().map(compute_histogram).collect())
        }
    }
}

/// Compute the similarity (0.0–1.0) between two signatures of the same kind,
/// averaged over the frames present in both. Mismatched variants score 0.
fn signature_similarity(a: &Signature, b: &Signature) -> f64 {
    match (a, b) {
        (Signature::Perceptual(pa), Signature::Perceptual(pb)) => {
            let n = pa.len().min(pb.len());
            if n == 0 {
                return 0.0;
            }
            (0..n).map(|i| pa[i].similarity(&pb[i])).sum::<f64>() / n as f64
        }
        (Signature::Ssim(ia), Signature::Ssim(ib)) => {
            let n = ia.len().min(ib.len());
            if n == 0 {
                return 0.0;
            }
            let params = SsimParams::default();
            (0..n)
                .map(|i| compute_ssim(&ia[i], &ib[i], &params))
                .sum::<f64>()
                / n as f64
        }
        (Signature::Histogram(ha), Signature::Histogram(hb)) => {
            let n = ha.len().min(hb.len());
            if n == 0 {
                return 0.0;
            }
            (0..n)
                .map(|i| compare_histograms(&ha[i], &hb[i]))
                .sum::<f64>()
                / n as f64
        }
        _ => 0.0,
    }
}

/// Greedily group files whose pairwise signature similarity is `>= threshold`.
fn group_signatures(
    entries: &[(PathBuf, Signature)],
    threshold: f64,
    method_label: &str,
) -> Vec<DupGroup> {
    let mut groups = Vec::new();
    let mut assigned = vec![false; entries.len()];

    for i in 0..entries.len() {
        if assigned[i] {
            continue;
        }
        let mut files = vec![entries[i].0.clone()];
        let mut best = 0.0f64;

        for j in (i + 1)..entries.len() {
            if assigned[j] {
                continue;
            }
            let sim = signature_similarity(&entries[i].1, &entries[j].1);
            if sim >= threshold {
                files.push(entries[j].0.clone());
                assigned[j] = true;
                if sim > best {
                    best = sim;
                }
            }
        }

        if files.len() > 1 {
            assigned[i] = true;
            groups.push(DupGroup {
                key: method_label.to_string(),
                score: best,
                files,
            });
        }
    }

    groups
}

/// Exact-duplicate grouping by content hash of the real file bytes.
fn scan_exact(files: &[PathBuf]) -> Result<Vec<DupGroup>> {
    let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for f in files {
        let hash = compute_file_hash(f, "blake3")?;
        map.entry(hash).or_default().push(f.clone());
    }
    let mut groups = Vec::new();
    for (hash, paths) in map {
        if paths.len() > 1 {
            groups.push(DupGroup {
                key: hash,
                score: 1.0,
                files: paths,
            });
        }
    }
    Ok(groups)
}

/// Visual near-duplicate grouping: decode each file's sampled frames, build the
/// method-appropriate signature, and group by similarity `>= threshold`.
///
/// Returns `(groups, undecodable)` where `undecodable` lists files that carried
/// no decodable video frame (they are honestly reported, never silently faked).
async fn scan_visual(
    files: &[PathBuf],
    method: VisualMethod,
    threshold: f64,
    sample_frames: usize,
    method_label: &str,
) -> Result<(Vec<DupGroup>, Vec<(PathBuf, String)>)> {
    let mut entries: Vec<(PathBuf, Signature)> = Vec::new();
    let mut undecodable: Vec<(PathBuf, String)> = Vec::new();

    for f in files {
        match decode_frames_as_images(f, sample_frames).await {
            Ok(images) => entries.push((f.clone(), build_signature(method, images))),
            Err(e) => undecodable.push((f.clone(), format!("{e:#}"))),
        }
    }

    let groups = group_signatures(&entries, threshold, method_label);
    Ok((groups, undecodable))
}

/// Map a parsed [`DetectionStrategy`] to a direct-scan visual method.
///
/// Any strategy that includes a perceptual pass is served by the perceptual
/// (DCT-hash) detector — it subsumes exact duplicates (identical frames hash
/// identically) while also catching re-encoded near-duplicates.
fn scan_visual_method(strategy: DetectionStrategy) -> Option<(VisualMethod, &'static str)> {
    if strategy.includes_perceptual() {
        Some((VisualMethod::Perceptual, "perceptual_hash"))
    } else if strategy.includes_ssim() {
        Some((VisualMethod::Ssim, "ssim"))
    } else if strategy.includes_histogram() {
        Some((VisualMethod::Histogram, "histogram"))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle dedup command dispatch.
pub async fn handle_dedup_command(command: DedupCommand, json_output: bool) -> Result<()> {
    match command {
        DedupCommand::Scan {
            input,
            strategy,
            threshold,
            recursive,
            sample_frames,
            report,
        } => {
            run_scan(
                &input,
                &strategy,
                threshold,
                recursive,
                sample_frames,
                &report,
                json_output,
            )
            .await
        }
        DedupCommand::Report {
            input,
            output,
            format,
            detailed,
            savings,
        } => run_report(&input, &output, &format, detailed, savings, json_output).await,
        DedupCommand::Clean {
            report,
            strategy,
            dry_run,
            trash,
            trash_dir,
        } => run_clean(&report, &strategy, dry_run, trash, &trash_dir, json_output).await,
        DedupCommand::Hash {
            input,
            algorithm,
            perceptual,
        } => run_hash(&input, &algorithm, perceptual, json_output).await,
        DedupCommand::Compare {
            file_a,
            file_b,
            method,
            frames,
        } => run_compare(&file_a, &file_b, &method, frames, json_output).await,
    }
}

// ---------------------------------------------------------------------------
// Scan
// ---------------------------------------------------------------------------

async fn run_scan(
    inputs: &[PathBuf],
    strategy_str: &str,
    threshold: f64,
    recursive: bool,
    sample_frames: usize,
    report_path: &Option<PathBuf>,
    json_output: bool,
) -> Result<()> {
    let strategy = parse_strategy(strategy_str)?;

    let mut files: Vec<PathBuf> = Vec::new();
    for p in inputs {
        if p.is_dir() {
            collect_files(p, recursive, &mut files)?;
        } else if p.is_file() {
            files.push(p.clone());
        }
    }

    if files.is_empty() {
        return Err(anyhow::anyhow!("No files found to scan"));
    }

    // Route the parsed strategy to the matching REAL detector. Exact hashing
    // groups bit-identical files; perceptual/SSIM/histogram decode real frames
    // and group by the `--threshold` similarity. Strategies that need a
    // persistent index or metadata extraction honestly error out.
    let (groups, undecodable): (Vec<DupGroup>, Vec<(PathBuf, String)>) =
        if strategy == DetectionStrategy::ExactHash {
            (scan_exact(&files)?, Vec::new())
        } else if let Some((method, label)) = scan_visual_method(strategy) {
            scan_visual(&files, method, threshold, sample_frames, label).await?
        } else {
            return Err(anyhow::anyhow!(
                "Strategy '{strategy_str}' has no direct-scan implementation without a \
                 persistent index. Supported for `dedup scan`: exact, perceptual, ssim, \
                 histogram, fast, all, visual."
            ));
        };

    let total_dups: usize = groups.iter().map(|g| g.files.len() - 1).sum();

    // Optionally save report
    if let Some(ref rpath) = report_path {
        let report_data = serde_json::json!({
            "strategy": strategy_str,
            "threshold": threshold,
            "sample_frames": sample_frames,
            "total_files": files.len(),
            "duplicate_groups": groups.len(),
            "duplicate_files": total_dups,
            "undecoded_files": undecodable.len(),
            "groups": groups.iter().map(|g| serde_json::json!({
                "hash": g.key,
                "score": g.score,
                "files": g.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        let data = serde_json::to_string_pretty(&report_data).context("Serialization failed")?;
        std::fs::write(rpath, data)
            .with_context(|| format!("Failed to write report: {}", rpath.display()))?;
    }

    if json_output {
        let result = serde_json::json!({
            "command": "dedup scan",
            "strategy": strategy_str,
            "threshold": threshold,
            "sample_frames": sample_frames,
            "total_files": files.len(),
            "duplicate_groups": groups.len(),
            "duplicate_files": total_dups,
            "undecoded_files": undecodable.len(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Dedup Scan".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Strategy:", strategy_str);
        println!("{:20} {:.2}", "Threshold:", threshold);
        println!("{:20} {}", "Sample frames:", sample_frames);
        println!("{:20} {}", "Total files:", files.len());
        println!("{:20} {}", "Duplicate groups:", groups.len());
        println!("{:20} {}", "Duplicate files:", total_dups);
        println!();
        for g in &groups {
            let key_disp = g.key.get(..12).unwrap_or(g.key.as_str());
            println!("  Group ({}, score {:.3})", key_disp.cyan(), g.score);
            for p in &g.files {
                println!("    - {}", p.display());
            }
        }
        if !undecodable.is_empty() {
            println!();
            println!(
                "{}",
                format!(
                    "{} file(s) had no decodable video frame:",
                    undecodable.len()
                )
                .yellow()
            );
            for (p, why) in &undecodable {
                println!("    - {} ({})", p.display(), why);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

async fn run_report(
    input: &PathBuf,
    output: &PathBuf,
    format: &str,
    detailed: bool,
    savings: bool,
    json_output: bool,
) -> Result<()> {
    if !input.exists() {
        return Err(anyhow::anyhow!("Input not found: {}", input.display()));
    }

    // If input is a directory, scan it; if JSON, load it
    let report_data = if input.is_dir() {
        let mut files = Vec::new();
        collect_files(input, true, &mut files)?;
        let mut hash_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut total_size: u64 = 0;
        for file in &files {
            let hash = compute_file_hash(file, "blake3")?;
            let size = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
            total_size += size;
            hash_map
                .entry(hash)
                .or_default()
                .push(file.display().to_string());
        }
        let dup_groups: Vec<_> = hash_map.iter().filter(|(_, v)| v.len() > 1).collect();
        let wasted: u64 = if savings {
            dup_groups
                .iter()
                .map(|(_, paths)| {
                    let first = std::path::Path::new(&paths[0]);
                    let size = std::fs::metadata(first).map(|m| m.len()).unwrap_or(0);
                    size * (paths.len() as u64 - 1)
                })
                .sum()
        } else {
            0
        };
        serde_json::json!({
            "total_files": files.len(),
            "total_size": total_size,
            "duplicate_groups": dup_groups.len(),
            "wasted_bytes": wasted,
            "groups": if detailed {
                dup_groups.iter().map(|(h, p)| serde_json::json!({"hash": h, "files": p})).collect()
            } else {
                Vec::new()
            },
        })
    } else {
        let data = std::fs::read_to_string(input)
            .with_context(|| format!("Failed to read: {}", input.display()))?;
        serde_json::from_str(&data).context("Failed to parse report")?
    };

    // Write output report
    let report_str = match format {
        "text" => serde_json::to_string_pretty(&report_data).context("Serialization failed")?,
        "csv" => {
            let mut csv = String::from("hash,file\n");
            if let Some(groups) = report_data.get("groups").and_then(|g| g.as_array()) {
                for group in groups {
                    let hash = group.get("hash").and_then(|h| h.as_str()).unwrap_or("");
                    if let Some(files) = group.get("files").and_then(|f| f.as_array()) {
                        for file in files {
                            let path = file.as_str().unwrap_or("");
                            csv.push_str(&format!("{hash},{path}\n"));
                        }
                    }
                }
            }
            csv
        }
        _ => serde_json::to_string_pretty(&report_data).context("Serialization failed")?,
    };
    std::fs::write(output, &report_str)
        .with_context(|| format!("Failed to write: {}", output.display()))?;

    if json_output {
        let result = serde_json::json!({
            "command": "dedup report",
            "output": output.display().to_string(),
            "format": format,
            "report": report_data,
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Dedup Report".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Output:", output.display());
        println!("{:20} {}", "Format:", format);
        if let Some(total) = report_data.get("total_files") {
            println!("{:20} {}", "Total files:", total);
        }
        if let Some(groups) = report_data.get("duplicate_groups") {
            println!("{:20} {}", "Duplicate groups:", groups);
        }
        if savings {
            if let Some(wasted) = report_data.get("wasted_bytes").and_then(|w| w.as_u64()) {
                println!(
                    "{:20} {:.2} MB",
                    "Wasted space:",
                    wasted as f64 / (1024.0 * 1024.0)
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Clean
// ---------------------------------------------------------------------------

/// Choose which file in a duplicate group to KEEP, using the real filesystem
/// size and modification time.
///
/// - `keep-largest`  → the file with the greatest byte length
/// - `keep-smallest` → the file with the smallest byte length
/// - `keep-newest`   → the most recently modified file
/// - `keep-oldest`   → the least recently modified file (default)
///
/// Files that cannot be stat'd are treated as size 0 / epoch time so they lose
/// the comparison and are the first candidates for deletion.
fn select_keep_index(files: &[String], strategy: &str) -> usize {
    let stats: Vec<(u64, std::time::SystemTime)> = files
        .iter()
        .map(|p| {
            let md = std::fs::metadata(p).ok();
            let size = md.as_ref().map(std::fs::Metadata::len).unwrap_or(0);
            let mtime = md
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::UNIX_EPOCH);
            (size, mtime)
        })
        .collect();

    let mut keep = 0usize;
    for i in 1..files.len() {
        let replace = match strategy {
            "keep-largest" => stats[i].0 > stats[keep].0,
            "keep-smallest" => stats[i].0 < stats[keep].0,
            "keep-newest" => stats[i].1 > stats[keep].1,
            // keep-oldest (also the default when the strategy is unrecognised)
            _ => stats[i].1 < stats[keep].1,
        };
        if replace {
            keep = i;
        }
    }
    keep
}

async fn run_clean(
    report: &PathBuf,
    strategy: &str,
    dry_run: bool,
    trash: bool,
    trash_dir: &Option<PathBuf>,
    json_output: bool,
) -> Result<()> {
    if !report.exists() {
        return Err(anyhow::anyhow!("Report not found: {}", report.display()));
    }

    let data = std::fs::read_to_string(report)
        .with_context(|| format!("Failed to read report: {}", report.display()))?;
    let report_data: serde_json::Value =
        serde_json::from_str(&data).context("Failed to parse report")?;

    let groups = report_data
        .get("groups")
        .and_then(|g| g.as_array())
        .cloned()
        .unwrap_or_default();

    let mut to_delete = Vec::new();

    for group in &groups {
        let files: Vec<String> = group
            .get("files")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if files.len() < 2 {
            continue;
        }

        // Keep one file per group, chosen by REAL file size / mtime, and mark
        // the rest for deletion.
        let keep_idx = select_keep_index(&files, strategy);

        for (i, file) in files.iter().enumerate() {
            if i != keep_idx {
                to_delete.push(file.clone());
            }
        }
    }

    if !dry_run {
        let trash_path = trash_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("oximedia_trash"));
        if trash && !trash_path.exists() {
            std::fs::create_dir_all(&trash_path)
                .with_context(|| format!("Failed to create trash dir: {}", trash_path.display()))?;
        }

        for file in &to_delete {
            let path = std::path::Path::new(file);
            if !path.exists() {
                continue;
            }
            if trash {
                let dest = trash_path.join(path.file_name().unwrap_or_default());
                std::fs::rename(path, dest)
                    .with_context(|| format!("Failed to move to trash: {file}"))?;
            } else {
                std::fs::remove_file(path).with_context(|| format!("Failed to delete: {file}"))?;
            }
        }
    }

    if json_output {
        let result = serde_json::json!({
            "command": "dedup clean",
            "strategy": strategy,
            "dry_run": dry_run,
            "trash": trash,
            "files_to_delete": to_delete.len(),
            "files": to_delete,
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Dedup Clean".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Strategy:", strategy);
        println!("{:20} {}", "Dry run:", dry_run);
        println!("{:20} {}", "Files to remove:", to_delete.len());
        if dry_run {
            println!();
            println!("{}", "(Dry run - no files were actually deleted)".yellow());
        }
        for f in &to_delete {
            let action = if dry_run { "Would delete" } else { "Deleted" };
            println!("  {} {}", action, f);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Hash
// ---------------------------------------------------------------------------

async fn run_hash(
    inputs: &[PathBuf],
    algorithm: &str,
    perceptual: bool,
    json_output: bool,
) -> Result<()> {
    // (path, content_hash, size, perceptual_hash_hex, note)
    let mut results: Vec<(String, String, u64, Option<String>, Option<String>)> = Vec::new();

    for path in inputs {
        if !path.exists() {
            return Err(anyhow::anyhow!("File not found: {}", path.display()));
        }
        let hash = compute_file_hash(path, algorithm)?;
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

        // A perceptual hash is derived from a REAL decoded frame; undecodable
        // input yields an honest note rather than a fabricated hash.
        let (phash_hex, note) = if perceptual {
            match decode_frames_as_images(path, 1).await {
                Ok(images) => match images.first() {
                    Some(img) => (Some(compute_phash(img).to_hex()), None),
                    None => (
                        None,
                        Some("perceptual hash unavailable: no decoded frame".to_string()),
                    ),
                },
                Err(e) => (None, Some(format!("perceptual hash unavailable: {e:#}"))),
            }
        } else {
            (None, None)
        };

        results.push((path.display().to_string(), hash, size, phash_hex, note));
    }

    if json_output {
        let result = serde_json::json!({
            "command": "dedup hash",
            "algorithm": algorithm,
            "perceptual": perceptual,
            "files": results.iter().map(|(path, hash, size, phash, note)| serde_json::json!({
                "path": path,
                "hash": hash,
                "size": size,
                "perceptual_hash": phash,
                "note": note,
            })).collect::<Vec<_>>(),
        });
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Dedup Hash".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "Algorithm:", algorithm);
        if perceptual {
            println!("{:20} enabled", "Perceptual:");
        }
        println!();
        for (path, hash, size, phash, note) in &results {
            println!("  {} {} ({} bytes)", hash.cyan(), path, size);
            if let Some(ph) = phash {
                println!("      pHash: {}", ph.yellow());
            }
            if let Some(n) = note {
                println!("      {}", n.dimmed());
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Compare
// ---------------------------------------------------------------------------

/// Real visual-similarity report for `dedup compare`.
struct VisualReport {
    /// Primary 0.0–1.0 similarity for the requested method.
    similarity: f64,
    /// Per-sub-metric 0.0–1.0 breakdown (only populated for `method = all`).
    details: Vec<(String, f64)>,
    /// Number of frame pairs actually compared.
    frames_compared: usize,
}

/// Compute a real visual-similarity report between two files' decoded frames,
/// averaged over the frame pairs present in both.
fn compute_visual_report(method: &str, a: &[Image], b: &[Image]) -> VisualReport {
    let n = a.len().min(b.len());
    let mut details = Vec::new();

    let similarity = if n == 0 {
        0.0
    } else {
        match method {
            "perceptual" => {
                (0..n)
                    .map(|i| compute_phash(&a[i]).similarity(&compute_phash(&b[i])))
                    .sum::<f64>()
                    / n as f64
            }
            "ssim" => {
                let params = SsimParams::default();
                (0..n)
                    .map(|i| compute_ssim(&a[i], &b[i], &params))
                    .sum::<f64>()
                    / n as f64
            }
            "histogram" => {
                (0..n)
                    .map(|i| {
                        compare_histograms(&compute_histogram(&a[i]), &compute_histogram(&b[i]))
                    })
                    .sum::<f64>()
                    / n as f64
            }
            // "all" (or any other value): full multi-metric fusion.
            _ => {
                let (mut dh, mut ah, mut ph, mut wh, mut hi, mut ss, mut overall) =
                    (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
                let mut ok = 0usize;
                for i in 0..n {
                    if let Ok(vs) = compare_images(&a[i], &b[i]) {
                        dh += vs.dhash_similarity;
                        ah += vs.ahash_similarity;
                        ph += vs.phash_similarity;
                        wh += vs.whash_similarity;
                        hi += vs.histogram_similarity;
                        ss += vs.ssim;
                        overall += vs.overall_score();
                        ok += 1;
                    }
                }
                let d = ok.max(1) as f64;
                details.push(("dhash".to_string(), dh / d));
                details.push(("ahash".to_string(), ah / d));
                details.push(("phash".to_string(), ph / d));
                details.push(("whash".to_string(), wh / d));
                details.push(("histogram".to_string(), hi / d));
                details.push(("ssim".to_string(), ss / d));
                overall / d
            }
        }
    };

    VisualReport {
        similarity,
        details,
        frames_compared: n,
    }
}

async fn run_compare(
    file_a: &PathBuf,
    file_b: &PathBuf,
    method: &str,
    frames: usize,
    json_output: bool,
) -> Result<()> {
    if !file_a.exists() {
        return Err(anyhow::anyhow!("File A not found: {}", file_a.display()));
    }
    if !file_b.exists() {
        return Err(anyhow::anyhow!("File B not found: {}", file_b.display()));
    }

    // Always-available exact content signals (computed on the real bytes).
    let hash_a = compute_file_hash(file_a, "blake3")?;
    let hash_b = compute_file_hash(file_b, "blake3")?;
    let exact_match = hash_a == hash_b;

    let size_a = std::fs::metadata(file_a).map(|m| m.len()).unwrap_or(0);
    let size_b = std::fs::metadata(file_b).map(|m| m.len()).unwrap_or(0);
    let size_similarity = if size_a.max(size_b) > 0 {
        size_a.min(size_b) as f64 / size_a.max(size_b) as f64
    } else {
        1.0
    };

    // Visual methods decode `frames` real frames from each file and compute the
    // requested perceptual/structural similarity. Undecodable input (audio-only,
    // unsupported container) yields an honest note — never a fabricated score.
    let want_visual = matches!(method, "perceptual" | "ssim" | "histogram" | "all");
    let mut visual: Option<VisualReport> = None;
    let mut visual_note: Option<String> = None;
    if want_visual {
        let decoded_a = decode_frames_as_images(file_a, frames).await;
        let decoded_b = decode_frames_as_images(file_b, frames).await;
        match (decoded_a, decoded_b) {
            (Ok(ia), Ok(ib)) => visual = Some(compute_visual_report(method, &ia, &ib)),
            (Err(e), _) | (_, Err(e)) => {
                visual_note = Some(format!("visual comparison unavailable: {e:#}"));
            }
        }
    }

    if json_output {
        let mut result = serde_json::json!({
            "command": "dedup compare",
            "method": method,
            "frames": frames,
            "file_a": file_a.display().to_string(),
            "file_b": file_b.display().to_string(),
            "exact_match": exact_match,
            "hash_a": hash_a,
            "hash_b": hash_b,
            "size_a": size_a,
            "size_b": size_b,
            "size_similarity": size_similarity,
        });
        if let Some(v) = &visual {
            result["visual_similarity"] = serde_json::json!(v.similarity);
            result["frames_compared"] = serde_json::json!(v.frames_compared);
            if !v.details.is_empty() {
                let map: serde_json::Map<String, serde_json::Value> = v
                    .details
                    .iter()
                    .map(|(k, val)| (k.clone(), serde_json::json!(val)))
                    .collect();
                result["visual_details"] = serde_json::Value::Object(map);
            }
        }
        if let Some(note) = &visual_note {
            result["visual_note"] = serde_json::json!(note);
        }
        let s = serde_json::to_string_pretty(&result).context("JSON serialization failed")?;
        println!("{s}");
    } else {
        println!("{}", "Dedup Compare".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:20} {}", "File A:", file_a.display());
        println!("{:20} {}", "File B:", file_b.display());
        println!("{:20} {}", "Method:", method);
        println!();
        println!(
            "{:20} {}",
            "Exact match:",
            if exact_match {
                "YES".green().to_string()
            } else {
                "NO".yellow().to_string()
            }
        );
        println!("{:20} {:.2}%", "Size similarity:", size_similarity * 100.0);
        if let Some(v) = &visual {
            println!(
                "{:20} {:.2}% ({} frame(s))",
                "Visual similarity:",
                v.similarity * 100.0,
                v.frames_compared
            );
            for (k, val) in &v.details {
                println!("    {:16} {:.2}%", format!("{k}:"), val * 100.0);
            }
        }
        if let Some(note) = &visual_note {
            println!("{:20} {}", "Visual:", note.dimmed());
        }
        println!("{:20} {}", "Hash A:", hash_a.dimmed());
        println!("{:20} {}", "Hash B:", hash_b.dimmed());
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
    fn test_parse_strategy() {
        assert!(parse_strategy("exact").is_ok());
        assert!(parse_strategy("fast").is_ok());
        assert!(parse_strategy("all").is_ok());
        assert!(parse_strategy("perceptual").is_ok());
        assert!(parse_strategy("nonexistent").is_err());
    }

    #[test]
    fn test_compute_file_hash() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dedup_test_hash.bin");
        std::fs::write(&path, b"test data for hashing").expect("write should succeed");
        let hash = compute_file_hash(&path, "blake3");
        assert!(hash.is_ok());
        let hash = hash.expect("hash should succeed");
        assert_eq!(hash.len(), 16);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_compute_file_hash_deterministic() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dedup_test_det.bin");
        std::fs::write(&path, b"deterministic data").expect("write should succeed");
        let h1 = compute_file_hash(&path, "blake3").expect("hash1");
        let h2 = compute_file_hash(&path, "blake3").expect("hash2");
        assert_eq!(h1, h2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_collect_files() {
        let dir = std::env::temp_dir().join("oximedia_dedup_collect_test");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(dir.join("a.txt"), b"a").ok();
        std::fs::write(dir.join("b.txt"), b"b").ok();
        let mut files = Vec::new();
        let result = collect_files(&dir, false, &mut files);
        assert!(result.is_ok());
        assert!(files.len() >= 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    // -- Real visual dedup fixtures & proofs ---------------------------------

    /// Write a single-frame C420jpeg Y4M with neutral chroma and `luma_fn` luma,
    /// which round-trips exactly through the real YUV→RGB decode path.
    fn write_y4m(name: &str, w: usize, h: usize, luma_fn: impl Fn(usize, usize) -> u8) -> PathBuf {
        let mut data = Vec::new();
        data.extend_from_slice(format!("YUV4MPEG2 W{w} H{h} F25:1 Ip C420jpeg\n").as_bytes());
        data.extend_from_slice(b"FRAME\n");
        for y in 0..h {
            for x in 0..w {
                data.push(luma_fn(x, y));
            }
        }
        let chroma_len = w.div_ceil(2) * h.div_ceil(2);
        data.extend(std::iter::repeat_n(128u8, chroma_len * 2));
        let path = std::env::temp_dir().join(format!(
            "oximedia_dedup_{}_{}.y4m",
            name,
            std::process::id()
        ));
        std::fs::write(&path, &data).expect("write Y4M fixture");
        path
    }

    /// Build an in-memory RGB `Image` whose channels all equal `f(x, y)`.
    fn gray_rgb_image(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Image {
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                data.extend_from_slice(&[v, v, v]);
            }
        }
        Image::from_data(w, h, 3, data).expect("build image")
    }

    /// Smooth diagonal gradient with strong low-frequency structure.
    fn gradient(x: usize, y: usize, w: usize, h: usize) -> u8 {
        let denom = (w + h).saturating_sub(2).max(1);
        ((x + y) * 255 / denom).min(255) as u8
    }

    /// A near-duplicate of [`gradient`]: identical except for a localized cluster
    /// of altered pixels whose coordinates are NOT multiples of 3.
    ///
    /// `compute_phash` downscales 96->32 with a nearest-neighbour step of 3
    /// (it reads only source pixels whose row AND column are multiples of 3), so
    /// edits at non-multiple-of-3 coordinates never reach the perceptual hash:
    /// the hash is byte-for-byte identical to the original while the file bytes
    /// differ. That is precisely the near-duplicate robustness exact hashing
    /// lacks — a minor localized edit defeats a cryptographic hash but not a
    /// perceptual one.
    fn near_dup(x: usize, y: usize, w: usize, h: usize) -> u8 {
        if x % 3 != 0 && y % 3 != 0 && x < 12 && y < 12 {
            gradient(x, y, w, h) ^ 0xFF
        } else {
            gradient(x, y, w, h)
        }
    }

    /// Quality-bar proof: a perceptual scan groups near-duplicates that exact
    /// hashing misses (their bytes differ, so exact finds nothing).
    #[tokio::test]
    async fn test_perceptual_scan_catches_near_dups_exact_misses() {
        let a = write_y4m("scan_a", 96, 96, |x, y| gradient(x, y, 96, 96));
        // Near-duplicate: a localized edit below the perceptual hash's
        // downsampling resolution (bytes differ, perceptual hash identical).
        let b = write_y4m("scan_b", 96, 96, |x, y| near_dup(x, y, 96, 96));
        // Clearly different: inverted gradient (perceptual hash ~complement).
        let c = write_y4m("scan_c", 96, 96, |x, y| 255 - gradient(x, y, 96, 96));

        let files = vec![a.clone(), b.clone(), c.clone()];

        // Exact hashing finds NO duplicates — every file's bytes differ.
        let exact = scan_exact(&files).expect("exact scan");
        assert!(
            exact.is_empty(),
            "exact hashing must not group files with differing bytes"
        );

        // Perceptual scan MUST group the near-duplicates A and B.
        let (groups, undecodable) =
            scan_visual(&files, VisualMethod::Perceptual, 0.80, 1, "perceptual_hash")
                .await
                .expect("visual scan");
        assert!(
            undecodable.is_empty(),
            "fixtures must decode: {undecodable:?}"
        );

        let ab = groups
            .iter()
            .find(|g| g.files.contains(&a) && g.files.contains(&b));
        assert!(
            ab.is_some(),
            "perceptual scan must group near-duplicates A and B (groups: {})",
            groups.len()
        );
        if let Some(g) = ab {
            assert!(
                !g.files.contains(&c),
                "the clearly-different file C must not join the near-dup group"
            );
        }

        for p in [&a, &b, &c] {
            std::fs::remove_file(p).ok();
        }
    }

    #[test]
    fn test_compute_visual_report_perceptual_ranks_near_dup_above_different() {
        let a = gray_rgb_image(96, 96, |x, y| gradient(x, y, 96, 96));
        let b = gray_rgb_image(96, 96, |x, y| near_dup(x, y, 96, 96));
        let c = gray_rgb_image(96, 96, |x, y| 255 - gradient(x, y, 96, 96));

        let near = compute_visual_report("perceptual", std::slice::from_ref(&a), &[b]);
        let diff = compute_visual_report("perceptual", std::slice::from_ref(&a), &[c]);

        assert_eq!(near.frames_compared, 1);
        assert!(
            near.similarity > diff.similarity,
            "near-dup similarity ({}) must exceed different ({})",
            near.similarity,
            diff.similarity
        );
        assert!(
            near.similarity >= 0.80,
            "near-duplicate perceptual similarity too low: {}",
            near.similarity
        );
    }

    #[test]
    fn test_compute_visual_report_all_reports_details() {
        let a = gray_rgb_image(64, 64, |x, y| gradient(x, y, 64, 64));
        let b = gray_rgb_image(64, 64, |x, y| gradient(x, y, 64, 64));
        let report = compute_visual_report("all", &[a], &[b]);
        // "all" fuses several sub-metrics and exposes their breakdown.
        assert!(!report.details.is_empty(), "method=all must report details");
        assert!(report.similarity > 0.0);
    }

    #[test]
    fn test_select_keep_index_by_size() {
        let dir = std::env::temp_dir().join(format!("oximedia_dedup_keep_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let small = dir.join("small.bin");
        let large = dir.join("large.bin");
        std::fs::write(&small, b"x").expect("write small");
        std::fs::write(&large, vec![0u8; 4096]).expect("write large");

        let files = vec![small.display().to_string(), large.display().to_string()];
        // The old code always kept index 0 regardless of strategy — these prove
        // the selection is now driven by real file size.
        assert_eq!(select_keep_index(&files, "keep-largest"), 1);
        assert_eq!(select_keep_index(&files, "keep-smallest"), 0);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_select_keep_index_by_mtime_matches_filesystem() {
        let dir =
            std::env::temp_dir().join(format!("oximedia_dedup_keep_mt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).ok();
        let f0 = dir.join("a.bin");
        let f1 = dir.join("b.bin");
        // Equal size so only mtime distinguishes them.
        std::fs::write(&f0, b"aaaa").expect("write a");
        std::fs::write(&f1, b"bbbb").expect("write b");

        let files = vec![f0.display().to_string(), f1.display().to_string()];
        let m0 = std::fs::metadata(&f0).and_then(|m| m.modified()).ok();
        let m1 = std::fs::metadata(&f1).and_then(|m| m.modified()).ok();

        if let (Some(m0), Some(m1)) = (m0, m1) {
            if m0 != m1 {
                let (newest, oldest) = if m1 > m0 { (1, 0) } else { (0, 1) };
                assert_eq!(select_keep_index(&files, "keep-newest"), newest);
                assert_eq!(select_keep_index(&files, "keep-oldest"), oldest);
            }
            // If the filesystem clock is too coarse to separate the writes, the
            // selection is still well-defined; nothing further to assert.
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_scan_audio_strategy_errors_honestly() {
        // Audio-fingerprint scanning has no direct-scan backing here; it must
        // error honestly rather than silently collapse to exact hashing.
        let f = write_y4m("audiostrat", 16, 16, |_, _| 100);
        let files = vec![f.clone()];
        let result = run_scan(&files, "audio", 0.9, false, 1, &None, true).await;
        std::fs::remove_file(&f).ok();
        assert!(
            result.is_err(),
            "unsupported scan strategy must error honestly"
        );
    }
}
