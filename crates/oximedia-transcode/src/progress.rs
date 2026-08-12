//! Progress tracking and estimation for transcode operations.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Progress information for a transcode operation.
#[derive(Debug, Clone)]
pub struct ProgressInfo {
    /// Current frame number being processed.
    pub current_frame: u64,
    /// Total number of frames to process.
    pub total_frames: u64,
    /// Percentage complete (0-100).
    pub percent: f64,
    /// Estimated time remaining.
    pub eta: Option<Duration>,
    /// Current encoding speed (frames per second).
    pub fps: f64,
    /// Current bitrate in bits per second.
    pub bitrate: u64,
    /// Elapsed time since start.
    pub elapsed: Duration,
    /// Current pass number (for multi-pass encoding).
    pub pass: u32,
    /// Total number of passes.
    pub total_passes: u32,
}

/// Callback function for progress updates.
pub type ProgressCallback = Arc<dyn Fn(&ProgressInfo) + Send + Sync>;

/// Progress tracker for transcode operations.
pub struct ProgressTracker {
    start_time: Instant,
    total_frames: u64,
    current_frame: Arc<Mutex<u64>>,
    total_passes: u32,
    current_pass: Arc<Mutex<u32>>,
    callback: Option<ProgressCallback>,
    update_interval: Duration,
    last_update: Arc<Mutex<Instant>>,
    frame_times: Arc<Mutex<Vec<Instant>>>,
}

impl ProgressTracker {
    /// Creates a new progress tracker.
    ///
    /// # Arguments
    ///
    /// * `total_frames` - Total number of frames to process
    /// * `total_passes` - Total number of encoding passes
    #[must_use]
    pub fn new(total_frames: u64, total_passes: u32) -> Self {
        Self {
            start_time: Instant::now(),
            total_frames,
            current_frame: Arc::new(Mutex::new(0)),
            total_passes,
            current_pass: Arc::new(Mutex::new(1)),
            callback: None,
            update_interval: Duration::from_millis(500),
            last_update: Arc::new(Mutex::new(Instant::now())),
            frame_times: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Sets the progress callback function.
    pub fn set_callback(&mut self, callback: ProgressCallback) {
        self.callback = Some(callback);
    }

    /// Sets the update interval for callbacks.
    pub fn set_update_interval(&mut self, interval: Duration) {
        self.update_interval = interval;
    }

    /// Updates the current frame number.
    pub fn update_frame(&self, frame: u64) {
        if let Ok(mut current) = self.current_frame.lock() {
            *current = frame;

            // Record frame time for FPS calculation
            if let Ok(mut times) = self.frame_times.lock() {
                times.push(Instant::now());
                // Keep only last 30 frames for moving average
                if times.len() > 30 {
                    times.remove(0);
                }
            }
        }

        self.maybe_trigger_callback();
    }

    /// Increments the current frame by one.
    pub fn increment_frame(&self) {
        if let Ok(mut current) = self.current_frame.lock() {
            *current += 1;
            let frame = *current;
            drop(current);

            // Record frame time
            if let Ok(mut times) = self.frame_times.lock() {
                times.push(Instant::now());
                if times.len() > 30 {
                    times.remove(0);
                }
            }

            // Check if we should trigger callback
            if frame % 10 == 0 {
                self.maybe_trigger_callback();
            }
        }
    }

    /// Sets the current pass number.
    pub fn set_pass(&self, pass: u32) {
        if let Ok(mut current_pass) = self.current_pass.lock() {
            *current_pass = pass;
        }
        // Reset frame counter when starting a new pass
        if let Ok(mut current_frame) = self.current_frame.lock() {
            *current_frame = 0;
        }
        self.maybe_trigger_callback();
    }

    /// Gets the current progress information.
    #[must_use]
    pub fn get_info(&self) -> ProgressInfo {
        let current_frame = self.current_frame.lock().map_or(0, |f| *f);
        let current_pass = self.current_pass.lock().map_or(1, |p| *p);
        let elapsed = self.start_time.elapsed();

        // Calculate percentage
        let frames_per_pass = self.total_frames;
        let total_work = frames_per_pass * u64::from(self.total_passes);
        let completed_work = frames_per_pass * u64::from(current_pass - 1) + current_frame;
        let percent = if total_work > 0 {
            (completed_work as f64 / total_work as f64) * 100.0
        } else {
            0.0
        };

        // Calculate FPS
        let fps = self.calculate_fps();

        // Calculate ETA
        let eta = if fps > 0.0 && total_work > completed_work {
            let remaining_frames = total_work - completed_work;
            let remaining_seconds = remaining_frames as f64 / fps;
            Some(Duration::from_secs_f64(remaining_seconds))
        } else {
            None
        };

        ProgressInfo {
            current_frame,
            total_frames: self.total_frames,
            percent,
            eta,
            fps,
            bitrate: 0, // Will be updated by encoder
            elapsed,
            pass: current_pass,
            total_passes: self.total_passes,
        }
    }

    /// Resets the tracker for a new pass.
    pub fn reset_for_pass(&self, pass: u32) {
        if let Ok(mut current_frame) = self.current_frame.lock() {
            *current_frame = 0;
        }
        if let Ok(mut current_pass) = self.current_pass.lock() {
            *current_pass = pass;
        }
        if let Ok(mut times) = self.frame_times.lock() {
            times.clear();
        }
    }

    fn calculate_fps(&self) -> f64 {
        if let Ok(times) = self.frame_times.lock() {
            if times.len() < 2 {
                return 0.0;
            }

            let first = times[0];
            let last = times.last().copied().unwrap_or(first);
            let duration = last.duration_since(first);

            if duration.as_secs_f64() > 0.0 {
                (times.len() - 1) as f64 / duration.as_secs_f64()
            } else {
                0.0
            }
        } else {
            0.0
        }
    }

    fn maybe_trigger_callback(&self) {
        if let Some(callback) = &self.callback {
            if let Ok(mut last_update) = self.last_update.lock() {
                if last_update.elapsed() >= self.update_interval {
                    *last_update = Instant::now();
                    let info = self.get_info();
                    callback(&info);
                }
            }
        }
    }
}

/// Builder for creating a progress tracker with custom configuration.
pub struct ProgressTrackerBuilder {
    #[allow(dead_code)]
    total_frames: u64,
    #[allow(dead_code)]
    total_passes: u32,
    #[allow(dead_code)]
    callback: Option<ProgressCallback>,
    #[allow(dead_code)]
    update_interval: Duration,
}
impl ProgressTrackerBuilder {
    #[allow(dead_code)]
    /// Creates a new progress tracker builder.
    #[must_use]
    pub fn new(total_frames: u64) -> Self {
        Self {
            total_frames,
            total_passes: 1,
            callback: None,
            update_interval: Duration::from_millis(500),
        }
    }

    /// Sets the number of encoding passes.
    #[must_use]
    #[allow(dead_code)]
    pub fn passes(mut self, passes: u32) -> Self {
        self.total_passes = passes;
        self
    }

    /// Sets the progress callback.
    #[must_use]
    #[allow(dead_code)]
    pub fn callback(mut self, callback: ProgressCallback) -> Self {
        self.callback = Some(callback);
        self
    }

    /// Sets the update interval.
    #[must_use]
    #[allow(dead_code)]
    pub fn update_interval(mut self, interval: Duration) -> Self {
        self.update_interval = interval;
        self
    }

    /// Builds the progress tracker.
    #[must_use]
    #[allow(dead_code)]
    pub fn build(self) -> ProgressTracker {
        let mut tracker = ProgressTracker::new(self.total_frames, self.total_passes);
        if let Some(callback) = self.callback {
            tracker.set_callback(callback);
        }
        tracker.set_update_interval(self.update_interval);
        tracker
    }
}

impl ProgressInfo {
    /// Formats the ETA as a human-readable string.
    #[must_use]
    pub fn format_eta(&self) -> String {
        if let Some(eta) = self.eta {
            let total_secs = eta.as_secs();
            let hours = total_secs / 3600;
            let minutes = (total_secs % 3600) / 60;
            let seconds = total_secs % 60;

            if hours > 0 {
                format!("{hours}h {minutes}m {seconds}s")
            } else if minutes > 0 {
                format!("{minutes}m {seconds}s")
            } else {
                format!("{seconds}s")
            }
        } else {
            "Unknown".to_string()
        }
    }

    /// Formats the elapsed time as a human-readable string.
    #[must_use]
    pub fn format_elapsed(&self) -> String {
        let total_secs = self.elapsed.as_secs();
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        let seconds = total_secs % 60;

        if hours > 0 {
            format!("{hours}h {minutes}m {seconds}s")
        } else if minutes > 0 {
            format!("{minutes}m {seconds}s")
        } else {
            format!("{seconds}s")
        }
    }

    /// Formats the bitrate as a human-readable string.
    #[must_use]
    pub fn format_bitrate(&self) -> String {
        let kbps = self.bitrate / 1000;
        if kbps > 1000 {
            format!("{:.2} Mbps", kbps as f64 / 1000.0)
        } else {
            format!("{kbps} kbps")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_tracker_creation() {
        let tracker = ProgressTracker::new(1000, 1);
        let info = tracker.get_info();

        assert_eq!(info.current_frame, 0);
        assert_eq!(info.total_frames, 1000);
        assert_eq!(info.percent, 0.0);
        assert_eq!(info.pass, 1);
        assert_eq!(info.total_passes, 1);
    }

    #[test]
    fn test_progress_update() {
        let tracker = ProgressTracker::new(1000, 1);
        tracker.update_frame(500);

        let info = tracker.get_info();
        assert_eq!(info.current_frame, 500);
        assert!((info.percent - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_progress_increment() {
        let tracker = ProgressTracker::new(1000, 1);

        for _ in 0..100 {
            tracker.increment_frame();
        }

        let info = tracker.get_info();
        assert_eq!(info.current_frame, 100);
        assert!((info.percent - 10.0).abs() < 0.1);
    }

    #[test]
    fn test_multipass_progress() {
        let tracker = ProgressTracker::new(1000, 2);
        tracker.update_frame(1000);
        tracker.set_pass(2);

        let info = tracker.get_info();
        assert_eq!(info.pass, 2);
        // After first pass complete, we're at 50%
        assert!((info.percent - 50.0).abs() < 0.1);
    }

    #[test]
    fn test_progress_reset() {
        let tracker = ProgressTracker::new(1000, 2);
        tracker.update_frame(500);
        tracker.reset_for_pass(2);

        let info = tracker.get_info();
        assert_eq!(info.current_frame, 0);
        assert_eq!(info.pass, 2);
    }

    #[test]
    fn test_progress_builder() {
        let tracker = ProgressTrackerBuilder::new(1000)
            .passes(2)
            .update_interval(Duration::from_secs(1))
            .build();

        let info = tracker.get_info();
        assert_eq!(info.total_frames, 1000);
        assert_eq!(info.total_passes, 2);
    }

    #[test]
    fn test_format_eta() {
        let info = ProgressInfo {
            current_frame: 500,
            total_frames: 1000,
            percent: 50.0,
            eta: Some(Duration::from_secs(3725)), // 1h 2m 5s
            fps: 30.0,
            bitrate: 5_000_000,
            elapsed: Duration::from_secs(60),
            pass: 1,
            total_passes: 1,
        };

        assert_eq!(info.format_eta(), "1h 2m 5s");
    }

    #[test]
    fn test_format_elapsed() {
        let info = ProgressInfo {
            current_frame: 500,
            total_frames: 1000,
            percent: 50.0,
            eta: None,
            fps: 30.0,
            bitrate: 5_000_000,
            elapsed: Duration::from_secs(125), // 2m 5s
            pass: 1,
            total_passes: 1,
        };

        assert_eq!(info.format_elapsed(), "2m 5s");
    }

    #[test]
    fn test_format_bitrate() {
        let info = ProgressInfo {
            current_frame: 500,
            total_frames: 1000,
            percent: 50.0,
            eta: None,
            fps: 30.0,
            bitrate: 5_500_000,
            elapsed: Duration::from_secs(60),
            pass: 1,
            total_passes: 1,
        };

        assert_eq!(info.format_bitrate(), "5.50 Mbps");
    }

    #[test]
    fn test_format_bitrate_kbps() {
        let info = ProgressInfo {
            current_frame: 500,
            total_frames: 1000,
            percent: 50.0,
            eta: None,
            fps: 30.0,
            bitrate: 500_000,
            elapsed: Duration::from_secs(60),
            pass: 1,
            total_passes: 1,
        };

        assert_eq!(info.format_bitrate(), "500 kbps");
    }
}
