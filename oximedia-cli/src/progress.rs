//! Progress reporting for media processing operations.
//!
//! Provides progress bars with ETA, FPS counters, bitrate display,
//! and frame counting for transcoding operations.

use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Output format for progress reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum ProgressFormat {
    /// Human-readable progress bar (default).
    #[default]
    Plain,
    /// NDJSON records emitted to stderr (one per tick).
    Json,
}

/// Process-wide `--quiet` state.
///
/// When set, status/banner printers across most subcommand handlers (plans,
/// completion summaries, and other decoration around action commands —
/// transcode plan/summary, extract plan, image status, archive-pro
/// ingest/migrate/report, cloud upload/download, collab create/join/.../
/// export, distributed start-coordinator/start-worker/submit/cancel, drm
/// encrypt/decrypt/keys-generate, edl export, and more) suppress their
/// human-oriented stdout text. Command *results* — probe data, analysis
/// values, query/report output (info/status/list/verify/validate/...),
/// `--json` / `--ndjson` payloads — are never suppressed, and errors always
/// go to stderr.
static QUIET: AtomicBool = AtomicBool::new(false);

/// Record the global `--quiet` flag. Called once from each binary's `main`.
pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

/// Whether `--quiet` was passed; status/banner printers consult this.
///
/// SLICE 4F (0.2.x) swept most subcommand handlers' action-command banners
/// (create/ingest/migrate/upload/export/submit/... — anything whose real
/// deliverable is a side effect, not the printed text) onto this mechanism.
/// Query/report commands (info/status/list/verify/validate/...)
/// deliberately print unconditionally: their text *is* the requested
/// result, not decoration, so `--quiet` must not touch it (see the module
/// doc above). Nine frame-harness-territory handlers were left for a later
/// pass: `scaling_cmd.rs`, `denoise_cmd.rs`, `stabilize_cmd.rs`,
/// `multicam_cmd.rs`, `subtitle_cmd.rs`, `timecode_cmd.rs`,
/// `captions_cmd.rs`, `restore_cmd.rs`, `transcode.rs`.
pub fn is_quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// Progress tracker for transcoding operations.
///
/// Displays a progress bar with:
/// - Current frame / total frames
/// - Processing speed (FPS)
/// - Estimated time remaining (ETA)
/// - Current bitrate
/// - File size
pub struct TranscodeProgress {
    bar: ProgressBar,
    start_time: Instant,
    frames_total: u64,
    frames_done: u64,
    bytes_written: u64,
    last_update: Instant,
    update_interval: Duration,
    /// Output format for progress ticks.
    pub format: ProgressFormat,
}

impl TranscodeProgress {
    /// Create a new transcode progress tracker.
    ///
    /// # Arguments
    ///
    /// * `total_frames` - Total number of frames to process
    pub fn new(total_frames: u64) -> Self {
        let bar = ProgressBar::new(total_frames);

        let style = ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} frames ({percent}%) {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-");

        bar.set_style(style);

        Self {
            bar,
            start_time: Instant::now(),
            frames_total: total_frames,
            frames_done: 0,
            bytes_written: 0,
            last_update: Instant::now(),
            update_interval: Duration::from_millis(100),
            format: ProgressFormat::Plain,
        }
    }

    /// Create a progress tracker with unknown total.
    ///
    /// Useful when the total frame count is not known in advance.
    pub fn new_spinner() -> Self {
        let bar = ProgressBar::new_spinner();

        let style = ProgressStyle::default_spinner()
            .template("{spinner:.green} [{elapsed_precise}] {pos} frames {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner());

        bar.set_style(style);

        Self {
            bar,
            start_time: Instant::now(),
            frames_total: 0,
            frames_done: 0,
            bytes_written: 0,
            last_update: Instant::now(),
            update_interval: Duration::from_millis(100),
            format: ProgressFormat::Plain,
        }
    }

    /// Create a new transcode progress tracker with an explicit output format.
    ///
    /// When `format` is [`ProgressFormat::Json`], the indicatif bar is hidden
    /// so that NDJSON records written to stderr are not interleaved with bar
    /// control codes.
    pub fn new_with_format(total_frames: u64, fmt: ProgressFormat) -> Self {
        let mut this = Self::new(total_frames);
        this.set_format(fmt);
        this
    }

    /// Set the progress output format.
    pub fn set_format(&mut self, fmt: ProgressFormat) {
        self.format = fmt;
        if fmt == ProgressFormat::Json {
            // Hide the indicatif bar when using JSON output.
            self.bar
                .set_draw_target(indicatif::ProgressDrawTarget::hidden());
        }
    }

    /// Update progress with the number of frames processed.
    ///
    /// # Arguments
    ///
    /// * `frames` - Number of frames completed so far
    pub fn update(&mut self, frames: u64) {
        self.frames_done = frames;

        // Throttle updates to avoid excessive CPU usage
        let now = Instant::now();
        if now.duration_since(self.last_update) < self.update_interval {
            return;
        }
        self.last_update = now;

        let fps = self.fps();
        let eta = self.eta();
        let bitrate = self.bitrate();

        match self.format {
            ProgressFormat::Plain => {
                self.bar.set_position(frames);
                let msg = format!(
                    "{:.1} fps | {} | {}",
                    fps,
                    format_eta(eta),
                    format_bitrate(bitrate)
                );
                self.bar.set_message(msg);
            }
            ProgressFormat::Json => {
                let elapsed = self.start_time.elapsed().as_secs_f64();
                let eta_secs = eta.as_secs_f64();
                let record = serde_json::json!({
                    "kind": "progress",
                    "frames_done": frames,
                    "frames_total": self.frames_total,
                    "fps": fps,
                    "bitrate_bps": bitrate,
                    "eta_seconds": eta_secs,
                    "elapsed_seconds": elapsed
                });
                eprintln!("{record}");
            }
        }
    }

    /// Update the number of bytes written to the output file.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Total bytes written so far
    pub fn set_bytes_written(&mut self, bytes: u64) {
        self.bytes_written = bytes;
    }

    /// Set a status message on the progress bar.
    ///
    /// # Arguments
    ///
    /// * `status` - Status message to display
    #[allow(dead_code)]
    pub fn set_status(&self, status: &str) {
        self.bar.set_message(status.to_string());
    }

    /// Mark the progress as complete and show final statistics.
    pub fn finish(&self) {
        let elapsed = self.start_time.elapsed();
        let avg_fps = if elapsed.as_secs_f64() > 0.0 {
            self.frames_done as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        };

        match self.format {
            ProgressFormat::Plain => {
                let final_msg = format!(
                    "{} | Avg {:.1} fps | {}",
                    "Complete".green().bold(),
                    avg_fps,
                    format_size(self.bytes_written)
                );
                self.bar.finish_with_message(final_msg);
            }
            ProgressFormat::Json => {
                let record = serde_json::json!({
                    "kind": "done",
                    "frames_done": self.frames_done,
                    "frames_total": self.frames_total,
                    "avg_fps": avg_fps,
                    "bytes_written": self.bytes_written,
                    "elapsed_seconds": elapsed.as_secs_f64()
                });
                eprintln!("{record}");
            }
        }
    }

    /// Mark the progress as failed with an error message.
    ///
    /// # Arguments
    ///
    /// * `error` - Error message to display
    #[allow(dead_code)]
    pub fn finish_with_error(&self, error: &str) {
        let msg = format!("{} {}", "Failed:".red().bold(), error);
        self.bar.finish_with_message(msg);
    }

    /// Calculate current processing speed in frames per second.
    pub fn fps(&self) -> f64 {
        let elapsed = self.start_time.elapsed();
        if elapsed.as_secs_f64() > 0.0 {
            self.frames_done as f64 / elapsed.as_secs_f64()
        } else {
            0.0
        }
    }

    /// Calculate estimated time remaining.
    pub fn eta(&self) -> Duration {
        if self.frames_total == 0 || self.frames_done == 0 {
            return Duration::from_secs(0);
        }

        let elapsed = self.start_time.elapsed();
        let frames_remaining = self.frames_total.saturating_sub(self.frames_done);

        if self.frames_done > 0 {
            let time_per_frame = elapsed.as_secs_f64() / self.frames_done as f64;
            let eta_secs = time_per_frame * frames_remaining as f64;
            Duration::from_secs_f64(eta_secs)
        } else {
            Duration::from_secs(0)
        }
    }

    /// Calculate current bitrate in bits per second.
    pub fn bitrate(&self) -> f64 {
        let elapsed = self.start_time.elapsed();
        if elapsed.as_secs_f64() > 0.0 {
            (self.bytes_written as f64 * 8.0) / elapsed.as_secs_f64()
        } else {
            0.0
        }
    }

    /// Get the total number of frames.
    #[allow(dead_code)]
    pub fn total_frames(&self) -> u64 {
        self.frames_total
    }

    /// Get the number of frames completed.
    #[allow(dead_code)]
    pub fn frames_completed(&self) -> u64 {
        self.frames_done
    }

    /// Get the total elapsed time.
    #[allow(dead_code)]
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }
}

/// Simple progress tracker for batch operations.
pub struct BatchProgress {
    bar: ProgressBar,
    start_time: Instant,
    #[allow(dead_code)]
    total_files: usize,
    completed: usize,
    failed: usize,
    /// Output format for progress events.
    pub format: ProgressFormat,
}

impl BatchProgress {
    /// Create a new batch progress tracker.
    ///
    /// # Arguments
    ///
    /// * `total_files` - Total number of files to process
    pub fn new(total_files: usize) -> Self {
        let bar = ProgressBar::new(total_files as u64);

        let style = ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} files ({percent}%) {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-");

        bar.set_style(style);

        Self {
            bar,
            start_time: Instant::now(),
            total_files,
            completed: 0,
            failed: 0,
            format: ProgressFormat::Plain,
        }
    }

    /// Set the progress output format.
    pub fn set_format(&mut self, fmt: ProgressFormat) {
        self.format = fmt;
        if fmt == ProgressFormat::Json {
            self.bar
                .set_draw_target(indicatif::ProgressDrawTarget::hidden());
        }
    }

    /// Mark a file as successfully completed.
    pub fn inc_success(&mut self) {
        self.completed += 1;
        self.bar.inc(1);
        self.emit_tick();
    }

    /// Mark a file as failed.
    pub fn inc_failed(&mut self) {
        self.failed += 1;
        self.bar.inc(1);
        self.emit_tick();
    }

    /// Emit a progress tick (plain bar or JSON).
    fn emit_tick(&self) {
        match self.format {
            ProgressFormat::Plain => {
                let msg = if self.failed > 0 {
                    format!(
                        "{} succeeded, {} failed",
                        self.completed.to_string().green(),
                        self.failed.to_string().red()
                    )
                } else {
                    format!("{} succeeded", self.completed.to_string().green())
                };
                self.bar.set_message(msg);
            }
            ProgressFormat::Json => {
                let elapsed = self.start_time.elapsed().as_secs_f64();
                let record = serde_json::json!({
                    "kind": "batch_progress",
                    "completed": self.completed,
                    "failed": self.failed,
                    "total": self.total_files,
                    "elapsed_seconds": elapsed
                });
                eprintln!("{record}");
            }
        }
    }

    /// Finish the progress display.
    pub fn finish(&self) {
        let elapsed = self.start_time.elapsed();
        match self.format {
            ProgressFormat::Plain => {
                let msg = format!(
                    "{} | {} succeeded, {} failed | Took {}",
                    "Complete".green().bold(),
                    self.completed,
                    self.failed,
                    format_duration(elapsed)
                );
                self.bar.finish_with_message(msg);
            }
            ProgressFormat::Json => {
                let record = serde_json::json!({
                    "kind": "batch_done",
                    "completed": self.completed,
                    "failed": self.failed,
                    "total": self.total_files,
                    "elapsed_seconds": elapsed.as_secs_f64()
                });
                eprintln!("{record}");
            }
        }
    }
}

/// Format a duration as a human-readable string (e.g., "1h 23m 45s").
fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// Format ETA with appropriate color coding.
fn format_eta(eta: Duration) -> String {
    let eta_str = format!("ETA {}", format_duration(eta));

    if eta.as_secs() > 3600 {
        eta_str.red().to_string()
    } else if eta.as_secs() > 600 {
        eta_str.yellow().to_string()
    } else {
        eta_str.green().to_string()
    }
}

/// Format bitrate in human-readable format (e.g., "2.5 Mbps").
fn format_bitrate(bitrate: f64) -> String {
    if bitrate >= 1_000_000.0 {
        format!("{:.2} Mbps", bitrate / 1_000_000.0)
    } else if bitrate >= 1_000.0 {
        format!("{:.1} kbps", bitrate / 1_000.0)
    } else {
        format!("{:.0} bps", bitrate)
    }
}

/// Format file size in human-readable format (e.g., "1.5 GB").
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
    }

    #[test]
    fn test_format_bitrate() {
        assert_eq!(format_bitrate(500.0), "500 bps");
        assert_eq!(format_bitrate(1500.0), "1.5 kbps");
        assert_eq!(format_bitrate(2_500_000.0), "2.50 Mbps");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(500), "500 B");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(2_097_152), "2.00 MB");
        assert_eq!(format_size(1_610_612_736), "1.50 GB");
    }

    #[test]
    fn test_progress_fps() {
        let mut progress = TranscodeProgress::new(100);
        std::thread::sleep(Duration::from_millis(100));
        progress.update(10);

        let fps = progress.fps();
        assert!(fps > 0.0);
    }

    #[test]
    fn test_progress_eta() {
        let mut progress = TranscodeProgress::new(100);
        std::thread::sleep(Duration::from_millis(100));
        progress.update(10);

        let eta = progress.eta();
        let _ = eta.as_secs(); // ETA is a Duration (always non-negative)
    }

    #[test]
    fn test_set_format_json_does_not_panic() {
        let mut progress = TranscodeProgress::new(100);
        progress.set_format(ProgressFormat::Json);
        assert_eq!(progress.format, ProgressFormat::Json);
        // JSON update should write to stderr, not panic
        progress.update(5);
    }

    #[test]
    fn test_set_format_plain_roundtrip() {
        let mut progress = TranscodeProgress::new_spinner();
        progress.set_format(ProgressFormat::Plain);
        assert_eq!(progress.format, ProgressFormat::Plain);
    }

    #[test]
    fn test_batch_progress_json_emit() {
        let mut bp = BatchProgress::new(3);
        bp.set_format(ProgressFormat::Json);
        bp.inc_success();
        bp.inc_failed();
        // finish should emit JSON to stderr without panicking
        bp.finish();
    }
}
