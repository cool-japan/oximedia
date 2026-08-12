//! Batch processing for converting multiple media files.
//!
//! Provides:
//! - Batch processing engine
//! - Job queue with parallel execution
//! - TOML configuration file parsing
//! - Error recovery and reporting

use crate::progress::{BatchProgress, ProgressFormat};
use crate::transcode::{self, TranscodeOptions};
use anyhow::{Context, Result};
use colored::Colorize;
use rayon::prelude::*;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

/// Options for batch processing.
#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub config: PathBuf,
    pub jobs: usize,
    pub continue_on_error: bool,
    pub dry_run: bool,
    /// Progress output format (plain bar or streaming JSON to stderr).
    pub progress_format: ProgressFormat,
}

/// Batch configuration from TOML file.
#[derive(Debug, Deserialize)]
pub struct BatchConfig {
    /// File patterns to match (e.g., ["*.mkv", "*.mp4"])
    pub patterns: Vec<String>,

    /// File patterns to exclude (optional)
    #[serde(default)]
    pub exclude: Vec<String>,

    /// Video codec to use
    pub video_codec: Option<String>,

    /// Audio codec to use
    pub audio_codec: Option<String>,

    /// Video bitrate
    pub video_bitrate: Option<String>,

    /// Audio bitrate
    pub audio_bitrate: Option<String>,

    /// Scale dimensions
    pub scale: Option<String>,

    /// Video filter chain
    pub video_filter: Option<String>,

    /// Encoder preset
    #[serde(default = "default_preset")]
    pub preset: String,

    /// Enable two-pass encoding
    #[serde(default)]
    pub two_pass: bool,

    /// CRF quality
    pub crf: Option<u32>,

    /// Number of threads per job (0 = auto)
    #[serde(default)]
    pub threads: usize,

    /// Output file extension
    #[serde(default = "default_extension")]
    pub output_extension: String,

    /// Overwrite existing files
    #[serde(default)]
    pub overwrite: bool,

    /// Recursive directory traversal
    #[serde(default = "default_recursive")]
    pub recursive: bool,
}

fn default_preset() -> String {
    "medium".to_string()
}

fn default_extension() -> String {
    "webm".to_string()
}

fn default_recursive() -> bool {
    true
}

/// Represents a single job in the batch queue.
#[derive(Debug, Clone)]
struct BatchJob {
    #[allow(dead_code)]
    input_path: PathBuf,
    #[allow(dead_code)]
    output_path: PathBuf,
    options: TranscodeOptions,
}

/// Result of a batch job.
#[derive(Debug)]
enum JobResult {
    Success {
        #[allow(dead_code)]
        input: PathBuf,
        #[allow(dead_code)]
        output: PathBuf,
    },
    Failed {
        input: PathBuf,
        error: String,
    },
    Skipped {
        input: PathBuf,
        reason: String,
    },
}

/// Main batch processing function.
pub async fn batch_process(options: BatchOptions) -> Result<()> {
    info!("Starting batch processing");
    debug!("Batch options: {:?}", options);

    // Validate directories
    validate_directories(&options)?;

    // Load and parse configuration
    let config = load_config(&options.config).context("Failed to load batch configuration")?;

    debug!("Loaded config: {:?}", config);

    // Discover files to process
    let input_files =
        discover_files(&options.input_dir, &config).context("Failed to discover input files")?;

    if input_files.is_empty() {
        warn!("No files found matching the patterns");
        return Ok(());
    }

    info!("Found {} files to process", input_files.len());

    // Create job queue
    let jobs = create_job_queue(&input_files, &options, &config)?;

    if options.dry_run {
        if !crate::progress::is_quiet() {
            print_dry_run(&jobs);
        }
        return Ok(());
    }

    // Execute jobs
    let results = execute_jobs(jobs, &options, options.progress_format).await?;

    // Print summary
    if !crate::progress::is_quiet() {
        print_batch_summary(&results);
    }

    // Check if any jobs failed
    let failed_count = results
        .iter()
        .filter(|r| matches!(r, JobResult::Failed { .. }))
        .count();

    if failed_count > 0 && !options.continue_on_error {
        Err(anyhow::anyhow!("{} jobs failed", failed_count))
    } else {
        Ok(())
    }
}

/// Validate input and output directories.
fn validate_directories(options: &BatchOptions) -> Result<()> {
    if !options.input_dir.exists() {
        return Err(anyhow::anyhow!(
            "Input directory does not exist: {}",
            options.input_dir.display()
        ));
    }

    if !options.input_dir.is_dir() {
        return Err(anyhow::anyhow!(
            "Input path is not a directory: {}",
            options.input_dir.display()
        ));
    }

    // Create output directory if it doesn't exist
    if !options.output_dir.exists() {
        fs::create_dir_all(&options.output_dir).context("Failed to create output directory")?;
    }

    Ok(())
}

/// Load batch configuration from TOML file.
fn load_config(path: &Path) -> Result<BatchConfig> {
    let content = fs::read_to_string(path).context("Failed to read configuration file")?;

    toml::from_str(&content).context("Failed to parse TOML configuration")
}

/// Discover files matching the patterns in the configuration.
fn discover_files(input_dir: &Path, config: &BatchConfig) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    // Walk directory tree
    if config.recursive {
        walk_dir_recursive(input_dir, &mut files, config)?;
    } else {
        walk_dir_shallow(input_dir, &mut files, config)?;
    }

    // Sort for deterministic processing order
    files.sort();

    Ok(files)
}

/// Walk directory recursively to find matching files.
fn walk_dir_recursive(dir: &Path, files: &mut Vec<PathBuf>, config: &BatchConfig) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            walk_dir_recursive(&path, files, config)?;
        } else if path.is_file() && matches_patterns(&path, config) {
            files.push(path);
        }
    }

    Ok(())
}

/// Walk directory (non-recursive) to find matching files.
fn walk_dir_shallow(dir: &Path, files: &mut Vec<PathBuf>, config: &BatchConfig) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && matches_patterns(&path, config) {
            files.push(path);
        }
    }

    Ok(())
}

/// Check if a file matches the include/exclude patterns.
fn matches_patterns(path: &Path, config: &BatchConfig) -> bool {
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Check include patterns
    let included = config
        .patterns
        .iter()
        .any(|pattern| glob_match(filename, pattern));

    if !included {
        return false;
    }

    // Check exclude patterns
    let excluded = config
        .exclude
        .iter()
        .any(|pattern| glob_match(filename, pattern));

    !excluded
}

/// Simple glob pattern matching (supports * wildcard).
fn glob_match(text: &str, pattern: &str) -> bool {
    // Simple implementation for common cases
    if pattern == "*" {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix('*') {
        text.starts_with(prefix)
    } else if let Some(suffix) = pattern.strip_prefix('*') {
        text.ends_with(suffix)
    } else {
        text == pattern
    }
}

/// Create job queue from discovered files.
fn create_job_queue(
    files: &[PathBuf],
    options: &BatchOptions,
    config: &BatchConfig,
) -> Result<Vec<BatchJob>> {
    let mut jobs = Vec::new();

    for input_path in files {
        let output_path = compute_output_path(input_path, options, config);

        let job_options = TranscodeOptions {
            input: input_path.clone(),
            output: output_path.clone(),
            preset_name: None,
            video_codec: config.video_codec.clone(),
            audio_codec: config.audio_codec.clone(),
            video_bitrate: config.video_bitrate.clone(),
            audio_bitrate: config.audio_bitrate.clone(),
            scale: config.scale.clone(),
            video_filter: config.video_filter.clone(),
            audio_filter: None,
            start_time: None,
            duration: None,
            framerate: None,
            preset: config.preset.clone(),
            two_pass: config.two_pass,
            crf: config.crf,
            threads: config.threads,
            overwrite: config.overwrite,
            // `BatchConfig` has no stream-selection knob; keep every stream.
            map: Vec::new(),
            // `BatchConfig` has no normalization knob today (batch TOML jobs
            // are driven purely by codec/bitrate/scale/preset fields); keep
            // batch transcodes byte-for-byte unchanged from before this flag
            // existed. Out of scope for the `--normalize-audio` CLI fix.
            normalize_audio: false,
            progress_format: ProgressFormat::Plain,
        };

        jobs.push(BatchJob {
            input_path: input_path.clone(),
            output_path,
            options: job_options,
        });
    }

    Ok(jobs)
}

/// Compute output path for an input file.
fn compute_output_path(input_path: &Path, options: &BatchOptions, config: &BatchConfig) -> PathBuf {
    // Get relative path from input directory
    let relative = input_path
        .strip_prefix(&options.input_dir)
        .unwrap_or(input_path);

    // Change extension
    let mut output = options.output_dir.join(relative);
    output.set_extension(&config.output_extension);

    output
}

/// Print dry run information.
fn print_dry_run(jobs: &[BatchJob]) {
    println!("{}", "Batch Dry Run".yellow().bold());
    println!("{}", "=".repeat(60));
    println!("The following operations would be performed:\n");

    for (i, job) in jobs.iter().enumerate() {
        println!("{}. {}", i + 1, "Job".cyan());
        println!("   {:<12} {}", "Input:", job.input_path.display());
        println!("   {:<12} {}", "Output:", job.output_path.display());
        if let Some(ref codec) = job.options.video_codec {
            println!("   {:<12} {}", "Video:", codec);
        }
        if let Some(ref codec) = job.options.audio_codec {
            println!("   {:<12} {}", "Audio:", codec);
        }
        println!();
    }

    println!("{}", "=".repeat(60));
    println!("Total: {} jobs", jobs.len());
}

/// Execute all jobs with parallel processing via rayon inside `spawn_blocking`.
///
/// Using `spawn_blocking` prevents the rayon thread-pool from blocking a tokio
/// worker thread.  Result ordering is preserved because `par_iter().map()` is
/// order-preserving when collected into a `Vec`.
async fn execute_jobs(
    jobs: Vec<BatchJob>,
    options: &BatchOptions,
    progress_format: ProgressFormat,
) -> Result<Vec<JobResult>> {
    let total_jobs = jobs.len();

    // Determine number of parallel threads: 0 means "all CPUs".
    let num_threads = if options.jobs == 0 {
        num_cpus::get()
    } else {
        options.jobs
    };

    info!("Processing with {} parallel jobs", num_threads);

    // Clone what we need to move into the blocking task.
    let progress = Arc::new(Mutex::new({
        let mut bp = BatchProgress::new(total_jobs);
        bp.set_format(progress_format);
        bp
    }));
    let progress_clone = Arc::clone(&progress);

    // Offload rayon work onto a dedicated blocking thread so tokio workers stay
    // free.  The thread pool is built fresh each call to avoid global-pool
    // contention.
    let results: Vec<JobResult> =
        tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<JobResult>> {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(num_threads)
                .build()
                .map_err(|e| anyhow::anyhow!("Failed to build rayon thread pool: {e}"))?;

            // par_iter preserves ordering; each closure executes concurrently.
            let job_results: Vec<JobResult> = pool.install(|| {
                jobs.par_iter()
                    .map(|job| {
                        let result = process_job(job);

                        // Update shared progress counter.
                        let mut guard = progress_clone.lock().unwrap_or_else(|e| e.into_inner());
                        match &result {
                            JobResult::Success { .. } | JobResult::Skipped { .. } => {
                                guard.inc_success();
                            }
                            JobResult::Failed { .. } => guard.inc_failed(),
                        }

                        result
                    })
                    .collect()
            });

            Ok(job_results)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;

    // Finish progress display.
    progress.lock().unwrap_or_else(|e| e.into_inner()).finish();

    Ok(results)
}

/// Process a single job.
fn process_job(job: &BatchJob) -> JobResult {
    // Create output directory if needed
    if let Some(parent) = job.output_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return JobResult::Failed {
                input: job.input_path.clone(),
                error: format!("Failed to create output directory: {}", e),
            };
        }
    }

    // Check if output exists and skip if not overwriting
    if job.output_path.exists() && !job.options.overwrite {
        return JobResult::Skipped {
            input: job.input_path.clone(),
            reason: "Output file already exists".to_string(),
        };
    }

    // Run transcode (blocking call in thread pool)
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            return JobResult::Failed {
                input: job.input_path.clone(),
                error: format!("Failed to create tokio runtime: {e}"),
            };
        }
    };
    match runtime.block_on(transcode::transcode(job.options.clone())) {
        Ok(()) => JobResult::Success {
            input: job.input_path.clone(),
            output: job.output_path.clone(),
        },
        Err(e) => {
            error!("Job failed for {}: {}", job.input_path.display(), e);
            JobResult::Failed {
                input: job.input_path.clone(),
                error: e.to_string(),
            }
        }
    }
}

/// Print batch processing summary.
fn print_batch_summary(results: &[JobResult]) {
    let success_count = results
        .iter()
        .filter(|r| matches!(r, JobResult::Success { .. }))
        .count();

    let failed_count = results
        .iter()
        .filter(|r| matches!(r, JobResult::Failed { .. }))
        .count();

    let skipped_count = results
        .iter()
        .filter(|r| matches!(r, JobResult::Skipped { .. }))
        .count();

    println!();
    println!("{}", "Batch Processing Summary".green().bold());
    println!("{}", "=".repeat(60));
    println!("{:20} {}", "Total jobs:", results.len());
    println!("{:20} {}", "Succeeded:", success_count.to_string().green());
    println!("{:20} {}", "Failed:", failed_count.to_string().red());
    println!("{:20} {}", "Skipped:", skipped_count.to_string().yellow());
    println!("{}", "=".repeat(60));

    // Print failed jobs
    if failed_count > 0 {
        println!();
        println!("{}", "Failed Jobs:".red().bold());
        for result in results {
            if let JobResult::Failed { input, error } = result {
                println!("  {} {}", "✗".red(), input.display());
                println!("    {}", error.dimmed());
            }
        }
    }

    // Print skipped jobs
    if skipped_count > 0 {
        println!();
        println!("{}", "Skipped Jobs:".yellow().bold());
        for result in results {
            if let JobResult::Skipped { input, reason } = result {
                println!("  {} {} - {}", "⊘".yellow(), input.display(), reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        assert!(glob_match("video.mkv", "*.mkv"));
        assert!(glob_match("test.mp4", "*.mp4"));
        assert!(!glob_match("video.mkv", "*.mp4"));
        assert!(glob_match("anything", "*"));
        assert!(glob_match("prefix_test.mkv", "prefix*"));
    }

    #[test]
    fn test_config_defaults() {
        let toml = r#"
            patterns = ["*.mkv"]
            video_codec = "vp9"
        "#;

        let config: BatchConfig = toml::from_str(toml).expect("toml::from_str should succeed");
        assert_eq!(config.preset, "medium");
        assert_eq!(config.output_extension, "webm");
        assert!(config.recursive);
        assert!(!config.two_pass);
    }
}
