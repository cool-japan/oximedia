// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Plugin health monitoring.
//!
//! [`PluginHealthMonitor`] records per-call success/failure outcomes and
//! duration measurements for a named plugin.  A [`PluginHealthMonitor::health_score`] (0.0–1.0)
//! is computed on demand from the recorded history.
//!
//! The health score is a weighted combination of:
//!
//! - **Success rate** — proportion of successful calls (weight: 0.7).
//! - **Latency penalty** — a sigmoid-shaped penalty that grows when the
//!   exponential-moving-average latency exceeds a configurable threshold
//!   (weight: 0.3).
//!
//! A brand-new monitor with no calls returns a score of `1.0` (assumed healthy).
//!
//! # Example
//!
//! ```rust
//! use oximedia_plugin::health_monitor::PluginHealthMonitor;
//!
//! let mut monitor = PluginHealthMonitor::new("my-codec");
//! monitor.record_call(true, 5);
//! monitor.record_call(true, 8);
//! monitor.record_call(false, 200);
//!
//! let score = monitor.health_score();
//! assert!(score > 0.0 && score <= 1.0);
//! ```

// ---------------------------------------------------------------------------
// PluginHealthMonitor
// ---------------------------------------------------------------------------

/// Records call history and computes a health score for a plugin.
pub struct PluginHealthMonitor {
    /// Human-readable plugin name (for diagnostics).
    name: String,
    /// Total calls recorded.
    total_calls: u64,
    /// Number of successful calls.
    success_calls: u64,
    /// Exponential moving average of call duration in milliseconds.
    ema_latency_ms: f64,
    /// EMA smoothing factor α ∈ (0, 1].  Higher → more weight on recent samples.
    ema_alpha: f64,
    /// Latency threshold in ms above which the health score starts to degrade.
    latency_threshold_ms: f64,
    /// Maximum recorded duration (for reporting).
    max_latency_ms: u64,
    /// Minimum recorded duration (for reporting).
    min_latency_ms: u64,
}

impl PluginHealthMonitor {
    /// Create a new health monitor for the plugin named `name`.
    ///
    /// Defaults:
    /// - EMA alpha: `0.2` (last 5 calls weighted most)
    /// - Latency threshold: `100 ms`
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            total_calls: 0,
            success_calls: 0,
            ema_latency_ms: 0.0,
            ema_alpha: 0.2,
            latency_threshold_ms: 100.0,
            max_latency_ms: 0,
            min_latency_ms: u64::MAX,
        }
    }

    /// Create a monitor with custom EMA alpha and latency threshold.
    pub fn with_config(name: impl Into<String>, ema_alpha: f64, latency_threshold_ms: f64) -> Self {
        let alpha = ema_alpha.clamp(0.01, 1.0);
        let threshold = latency_threshold_ms.max(1.0);
        Self {
            name: name.into(),
            total_calls: 0,
            success_calls: 0,
            ema_latency_ms: 0.0,
            ema_alpha: alpha,
            latency_threshold_ms: threshold,
            max_latency_ms: 0,
            min_latency_ms: u64::MAX,
        }
    }

    /// Record the outcome of a single plugin call.
    ///
    /// # Arguments
    ///
    /// * `success`  — `true` if the call completed without error.
    /// * `dur_ms`   — call duration in milliseconds.
    pub fn record_call(&mut self, success: bool, dur_ms: u64) {
        self.total_calls += 1;
        if success {
            self.success_calls += 1;
        }

        // Update EMA latency.
        let sample = dur_ms as f64;
        if self.total_calls == 1 {
            // Initialise EMA to first sample.
            self.ema_latency_ms = sample;
        } else {
            self.ema_latency_ms =
                self.ema_alpha * sample + (1.0 - self.ema_alpha) * self.ema_latency_ms;
        }

        // Update min/max.
        if dur_ms > self.max_latency_ms {
            self.max_latency_ms = dur_ms;
        }
        if dur_ms < self.min_latency_ms {
            self.min_latency_ms = dur_ms;
        }
    }

    /// Compute the current health score in the range `[0.0, 1.0]`.
    ///
    /// Returns `1.0` when no calls have been recorded (benefit of the doubt).
    #[must_use]
    pub fn health_score(&self) -> f32 {
        if self.total_calls == 0 {
            return 1.0;
        }

        // Success rate component (weight 0.7).
        let success_rate = self.success_calls as f64 / self.total_calls as f64;
        let success_component = success_rate * 0.7;

        // Latency penalty component (weight 0.3).
        // penalty grows from 0 when latency == threshold to ~1 when latency >> threshold.
        let ratio = self.ema_latency_ms / self.latency_threshold_ms;
        // Sigmoid-like: penalty = ratio² / (1 + ratio²)
        let penalty = (ratio * ratio) / (1.0 + ratio * ratio);
        let latency_component = (1.0 - penalty) * 0.3;

        let score = (success_component + latency_component).clamp(0.0, 1.0);
        score as f32
    }

    /// Return the plugin name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the total number of recorded calls.
    #[must_use]
    pub fn total_calls(&self) -> u64 {
        self.total_calls
    }

    /// Return the number of successful calls.
    #[must_use]
    pub fn success_calls(&self) -> u64 {
        self.success_calls
    }

    /// Return the current exponential moving average latency in milliseconds.
    #[must_use]
    pub fn ema_latency_ms(&self) -> f64 {
        self.ema_latency_ms
    }

    /// Return the maximum observed call duration in milliseconds.
    ///
    /// Returns `0` if no calls have been recorded.
    #[must_use]
    pub fn max_latency_ms(&self) -> u64 {
        if self.total_calls == 0 {
            0
        } else {
            self.max_latency_ms
        }
    }

    /// Return the minimum observed call duration in milliseconds.
    ///
    /// Returns `0` if no calls have been recorded.
    #[must_use]
    pub fn min_latency_ms(&self) -> u64 {
        if self.total_calls == 0 {
            0
        } else {
            self.min_latency_ms
        }
    }

    /// Returns `true` if the plugin is considered healthy (score ≥ 0.5).
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.health_score() >= 0.5
    }

    /// Reset all recorded statistics.
    pub fn reset(&mut self) {
        self.total_calls = 0;
        self.success_calls = 0;
        self.ema_latency_ms = 0.0;
        self.max_latency_ms = 0;
        self.min_latency_ms = u64::MAX;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_calls_score_is_one() {
        let monitor = PluginHealthMonitor::new("test-plugin");
        assert!((monitor.health_score() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_all_successes_fast_calls_score_near_one() {
        let mut monitor = PluginHealthMonitor::new("fast-plugin");
        for _ in 0..10 {
            monitor.record_call(true, 5); // 5 ms << 100 ms threshold
        }
        let score = monitor.health_score();
        // expect score close to 1.0 (latency well under threshold)
        assert!(score > 0.9, "score={score}");
    }

    #[test]
    fn test_all_failures_score_near_zero_point_three() {
        let mut monitor = PluginHealthMonitor::new("broken-plugin");
        for _ in 0..10 {
            monitor.record_call(false, 5);
        }
        let score = monitor.health_score();
        // 0 success_rate → success_component=0, latency_component≈0.3 (fast calls)
        assert!(score < 0.4, "score={score}");
    }

    #[test]
    fn test_high_latency_reduces_score() {
        let mut monitor = PluginHealthMonitor::new("slow-plugin");
        for _ in 0..20 {
            monitor.record_call(true, 10_000); // 100× threshold
        }
        let score = monitor.health_score();
        // latency penalty ≈ 1 → latency_component ≈ 0
        assert!(score < 0.75, "score={score}");
    }

    #[test]
    fn test_record_count() {
        let mut m = PluginHealthMonitor::new("x");
        m.record_call(true, 10);
        m.record_call(false, 20);
        assert_eq!(m.total_calls(), 2);
        assert_eq!(m.success_calls(), 1);
    }

    #[test]
    fn test_min_max_latency() {
        let mut m = PluginHealthMonitor::new("x");
        m.record_call(true, 50);
        m.record_call(true, 10);
        m.record_call(true, 200);
        assert_eq!(m.min_latency_ms(), 10);
        assert_eq!(m.max_latency_ms(), 200);
    }

    #[test]
    fn test_min_max_before_any_calls() {
        let m = PluginHealthMonitor::new("x");
        assert_eq!(m.min_latency_ms(), 0);
        assert_eq!(m.max_latency_ms(), 0);
    }

    #[test]
    fn test_is_healthy_all_success() {
        let mut m = PluginHealthMonitor::new("x");
        for _ in 0..5 {
            m.record_call(true, 1);
        }
        assert!(m.is_healthy());
    }

    #[test]
    fn test_is_not_healthy_all_failures() {
        let mut m = PluginHealthMonitor::new("x");
        for _ in 0..5 {
            m.record_call(false, 1);
        }
        assert!(!m.is_healthy());
    }

    #[test]
    fn test_reset_clears_stats() {
        let mut m = PluginHealthMonitor::new("x");
        m.record_call(true, 100);
        m.record_call(false, 200);
        m.reset();
        assert_eq!(m.total_calls(), 0);
        assert_eq!(m.success_calls(), 0);
        assert_eq!(m.max_latency_ms(), 0);
        assert!((m.health_score() - 1.0_f32).abs() < f32::EPSILON);
    }

    #[test]
    fn test_ema_latency_initialised_on_first_call() {
        let mut m = PluginHealthMonitor::new("x");
        m.record_call(true, 42);
        assert!((m.ema_latency_ms() - 42.0).abs() < 1e-9);
    }

    #[test]
    fn test_name() {
        let m = PluginHealthMonitor::new("my-plugin");
        assert_eq!(m.name(), "my-plugin");
    }

    #[test]
    fn test_score_is_in_zero_one_range() {
        let mut m = PluginHealthMonitor::with_config("x", 0.5, 50.0);
        for i in 0..20_u64 {
            m.record_call(i % 3 != 0, i * 30);
        }
        let score = m.health_score();
        assert!((0.0..=1.0).contains(&score), "score {score} out of range");
    }
}
