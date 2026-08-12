//! Batch processing mode for audio meters.
//!
//! This module provides simultaneous multi-channel meter processing, enabling efficient
//! analysis of many audio channels in a single pass without per-frame overhead for each
//! meter individually.
//!
//! # Design
//!
//! A [`BatchMeterProcessor`] holds one full set of meters per *lane* (channel group) and
//! processes them together. This is useful in DAW-style mixing consoles where dozens of
//! channels need metering at the same time:
//!
//! - **Peak** (digital dBFS, per-channel, sample-accurate)
//! - **RMS** (root-mean-square level with configurable window)
//! - **True-peak** (instantaneous absolute maximum across all samples)
//!
//! All processing is O(n) in the number of samples and allocation-free in the hot path.
//!
//! # Example
//!
//! ```rust
//! use oximedia_audio::meters::batch::{BatchMeterConfig, BatchMeterProcessor};
//!
//! // Process 4 stereo channels (8 mono lanes) simultaneously.
//! let config = BatchMeterConfig {
//!     channels: 8,
//!     sample_rate: 48_000.0,
//!     rms_window_ms: 300.0,
//!     peak_hold_ms: 2000.0,
//!     overload_threshold_db: -0.1,
//! };
//! let mut proc = BatchMeterProcessor::new(config);
//!
//! // Feed interleaved samples (8 channels, 64 frames).
//! let samples: Vec<f32> = vec![0.5_f32; 8 * 64];
//! proc.process_interleaved(&samples);
//!
//! let reading = proc.reading();
//! assert_eq!(reading.peak_dbfs.len(), 8);
//! ```

#![forbid(unsafe_code)]

use crate::{AudioError, AudioResult};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Minimum dBFS value used as "silence" sentinel.
const SILENCE_DB: f64 = -144.0;

// ─────────────────────────────────────────────────────────────────────────────
// BatchMeterConfig
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration for a [`BatchMeterProcessor`].
#[derive(Debug, Clone)]
pub struct BatchMeterConfig {
    /// Number of channels to meter simultaneously.
    pub channels: usize,
    /// Sample rate in Hz.
    pub sample_rate: f64,
    /// RMS integration window in milliseconds.
    pub rms_window_ms: f64,
    /// Peak hold duration in milliseconds.
    pub peak_hold_ms: f64,
    /// Level (dBFS) above which overload is signalled.
    pub overload_threshold_db: f64,
}

impl Default for BatchMeterConfig {
    fn default() -> Self {
        Self {
            channels: 2,
            sample_rate: 48_000.0,
            rms_window_ms: 300.0,
            peak_hold_ms: 2_000.0,
            overload_threshold_db: -0.1,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-channel state
// ─────────────────────────────────────────────────────────────────────────────

struct ChannelState {
    /// Circular buffer holding squared samples for RMS computation.
    rms_buf: Vec<f64>,
    /// Write index into `rms_buf`.
    rms_write: usize,
    /// Running sum of squares in `rms_buf`.
    rms_sum: f64,
    /// Current instantaneous peak (linear).
    peak_linear: f64,
    /// Held peak value (dBFS), decays after `peak_hold_samples`.
    peak_hold_db: f64,
    /// Samples remaining until the held peak decays.
    peak_hold_remaining: usize,
    /// Peak hold duration in samples.
    peak_hold_samples: usize,
    /// True-peak (maximum absolute value seen since last reset).
    true_peak_linear: f64,
    /// Overload flag: set when peak exceeds threshold, cleared by [`ChannelState::reset`].
    overload: bool,
    /// Overload threshold (linear).
    overload_threshold_linear: f64,
}

impl ChannelState {
    fn new(
        rms_window_samples: usize,
        peak_hold_samples: usize,
        overload_threshold_db: f64,
    ) -> Self {
        let overload_threshold_linear = db_to_linear(overload_threshold_db);
        Self {
            rms_buf: vec![0.0; rms_window_samples.max(1)],
            rms_write: 0,
            rms_sum: 0.0,
            peak_linear: 0.0,
            peak_hold_db: SILENCE_DB,
            peak_hold_remaining: 0,
            peak_hold_samples,
            true_peak_linear: 0.0,
            overload: false,
            overload_threshold_linear,
        }
    }

    /// Feed one sample (linear amplitude) into this channel's state.
    fn push_sample(&mut self, sample: f64) {
        let abs_sample = sample.abs();

        // True peak.
        if abs_sample > self.true_peak_linear {
            self.true_peak_linear = abs_sample;
        }

        // Overload detection.
        if abs_sample >= self.overload_threshold_linear {
            self.overload = true;
        }

        // Instantaneous peak.
        if abs_sample > self.peak_linear {
            self.peak_linear = abs_sample;
            let db = linear_to_db(abs_sample);
            self.peak_hold_db = db;
            self.peak_hold_remaining = self.peak_hold_samples;
        } else if self.peak_hold_remaining > 0 {
            self.peak_hold_remaining -= 1;
        } else {
            // Decay: pull peak back towards this sample.
            self.peak_linear = (self.peak_linear * 0.999).max(abs_sample);
        }

        // RMS ring-buffer update.
        let squared = sample * sample;
        // Remove the oldest value.
        self.rms_sum -= self.rms_buf[self.rms_write];
        self.rms_buf[self.rms_write] = squared;
        self.rms_sum = (self.rms_sum + squared).max(0.0); // guard fp drift
        self.rms_write = (self.rms_write + 1) % self.rms_buf.len();
    }

    /// Instantaneous peak level in dBFS.
    fn peak_dbfs(&self) -> f64 {
        if self.peak_linear <= 0.0 {
            SILENCE_DB
        } else {
            linear_to_db(self.peak_linear)
        }
    }

    /// RMS level in dBFS.
    fn rms_dbfs(&self) -> f64 {
        let mean_sq = self.rms_sum / self.rms_buf.len() as f64;
        if mean_sq <= 0.0 {
            SILENCE_DB
        } else {
            10.0 * mean_sq.log10()
        }
    }

    /// Held-peak level in dBFS.
    fn peak_hold_dbfs(&self) -> f64 {
        self.peak_hold_db
    }

    /// True-peak (maximum absolute sample ever seen) in dBFS.
    fn true_peak_dbfs(&self) -> f64 {
        if self.true_peak_linear <= 0.0 {
            SILENCE_DB
        } else {
            linear_to_db(self.true_peak_linear)
        }
    }

    fn reset(&mut self) {
        self.rms_buf.fill(0.0);
        self.rms_write = 0;
        self.rms_sum = 0.0;
        self.peak_linear = 0.0;
        self.peak_hold_db = SILENCE_DB;
        self.peak_hold_remaining = 0;
        self.true_peak_linear = 0.0;
        self.overload = false;
    }

    fn reset_peak_hold(&mut self) {
        self.peak_hold_db = SILENCE_DB;
        self.peak_hold_remaining = 0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchMeterReading
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of all meter readings from a [`BatchMeterProcessor`].
#[derive(Debug, Clone)]
pub struct BatchMeterReading {
    /// Instantaneous peak level per channel (dBFS).
    pub peak_dbfs: Vec<f64>,
    /// RMS level per channel (dBFS).
    pub rms_dbfs: Vec<f64>,
    /// Held-peak level per channel (dBFS).
    pub peak_hold_dbfs: Vec<f64>,
    /// True-peak (all-time max absolute sample) per channel (dBFS).
    pub true_peak_dbfs: Vec<f64>,
    /// Overload flag per channel.
    pub overload: Vec<bool>,
    /// Maximum peak level across all channels (dBFS).
    pub max_peak_dbfs: f64,
    /// Maximum RMS level across all channels (dBFS).
    pub max_rms_dbfs: f64,
    /// True if any channel is in overload.
    pub any_overload: bool,
}

impl BatchMeterReading {
    fn from_states(states: &[ChannelState]) -> Self {
        let peak_dbfs: Vec<f64> = states.iter().map(ChannelState::peak_dbfs).collect();
        let rms_dbfs: Vec<f64> = states.iter().map(ChannelState::rms_dbfs).collect();
        let peak_hold_dbfs: Vec<f64> = states.iter().map(ChannelState::peak_hold_dbfs).collect();
        let true_peak_dbfs: Vec<f64> = states.iter().map(ChannelState::true_peak_dbfs).collect();
        let overload: Vec<bool> = states.iter().map(|s| s.overload).collect();

        let max_peak_dbfs = peak_dbfs.iter().copied().fold(SILENCE_DB, f64::max);
        let max_rms_dbfs = rms_dbfs.iter().copied().fold(SILENCE_DB, f64::max);
        let any_overload = overload.iter().any(|&o| o);

        Self {
            peak_dbfs,
            rms_dbfs,
            peak_hold_dbfs,
            true_peak_dbfs,
            overload,
            max_peak_dbfs,
            max_rms_dbfs,
            any_overload,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BatchMeterProcessor
// ─────────────────────────────────────────────────────────────────────────────

/// Batch meter processor: meters all channels simultaneously in a single pass.
///
/// Supported input layouts:
/// - **Interleaved** (`[ch0_s0, ch1_s0, ch2_s0, …, ch0_s1, ch1_s1, …]`)
/// - **Planar** (`[ch0_samples…, ch1_samples…, …]` — one slice per call to
///   [`BatchMeterProcessor::process_channel`])
pub struct BatchMeterProcessor {
    config: BatchMeterConfig,
    states: Vec<ChannelState>,
    /// Total sample frames processed.
    frames_processed: u64,
}

impl BatchMeterProcessor {
    /// Create a new batch meter processor.
    #[must_use]
    pub fn new(config: BatchMeterConfig) -> Self {
        let rms_window_samples =
            ((config.rms_window_ms * config.sample_rate / 1_000.0).round() as usize).max(1);
        let peak_hold_samples =
            ((config.peak_hold_ms * config.sample_rate / 1_000.0).round() as usize).max(1);

        let states = (0..config.channels)
            .map(|_| {
                ChannelState::new(
                    rms_window_samples,
                    peak_hold_samples,
                    config.overload_threshold_db,
                )
            })
            .collect();

        Self {
            config,
            states,
            frames_processed: 0,
        }
    }

    /// Process interleaved samples (all channels packed frame-by-frame).
    ///
    /// `samples.len()` must be a multiple of `config.channels`.
    /// If it is not, trailing partial frames are silently ignored.
    pub fn process_interleaved(&mut self, samples: &[f32]) {
        let ch = self.config.channels;
        if ch == 0 {
            return;
        }
        let frames = samples.len() / ch;
        for frame_idx in 0..frames {
            for (ch_idx, state) in self.states.iter_mut().enumerate() {
                let sample = samples[frame_idx * ch + ch_idx];
                state.push_sample(f64::from(sample));
            }
            self.frames_processed += 1;
        }
    }

    /// Process planar f32 samples for a single channel.
    ///
    /// `channel` must be less than `config.channels`.
    ///
    /// # Errors
    ///
    /// Returns [`AudioError::InvalidParameter`] if `channel` is out of range.
    pub fn process_channel(&mut self, channel: usize, samples: &[f32]) -> AudioResult<()> {
        let channels = self.config.channels;
        let state = self.states.get_mut(channel).ok_or_else(|| {
            AudioError::InvalidParameter(format!(
                "batch meter channel {channel} out of range (have {channels})"
            ))
        })?;
        for &s in samples {
            state.push_sample(f64::from(s));
        }
        Ok(())
    }

    /// Process interleaved f64 samples.
    pub fn process_interleaved_f64(&mut self, samples: &[f64]) {
        let ch = self.config.channels;
        if ch == 0 {
            return;
        }
        let frames = samples.len() / ch;
        for frame_idx in 0..frames {
            for (ch_idx, state) in self.states.iter_mut().enumerate() {
                state.push_sample(samples[frame_idx * ch + ch_idx]);
            }
            self.frames_processed += 1;
        }
    }

    /// Get current meter readings for all channels.
    #[must_use]
    pub fn reading(&self) -> BatchMeterReading {
        BatchMeterReading::from_states(&self.states)
    }

    /// Get instantaneous peak level for one channel (dBFS).
    ///
    /// Returns [`None`] if `channel` is out of range.
    #[must_use]
    pub fn peak_dbfs(&self, channel: usize) -> Option<f64> {
        self.states.get(channel).map(ChannelState::peak_dbfs)
    }

    /// Get RMS level for one channel (dBFS).
    ///
    /// Returns [`None`] if `channel` is out of range.
    #[must_use]
    pub fn rms_dbfs(&self, channel: usize) -> Option<f64> {
        self.states.get(channel).map(ChannelState::rms_dbfs)
    }

    /// Get true-peak for one channel (dBFS).
    ///
    /// Returns [`None`] if `channel` is out of range.
    #[must_use]
    pub fn true_peak_dbfs(&self, channel: usize) -> Option<f64> {
        self.states.get(channel).map(ChannelState::true_peak_dbfs)
    }

    /// Returns `true` if any channel has triggered overload.
    #[must_use]
    pub fn any_overload(&self) -> bool {
        self.states.iter().any(|s| s.overload)
    }

    /// Returns `true` if the specified channel is in overload.
    #[must_use]
    pub fn channel_overload(&self, channel: usize) -> bool {
        self.states.get(channel).map_or(false, |s| s.overload)
    }

    /// Reset all channel state (peaks, RMS history, overload flags).
    pub fn reset(&mut self) {
        for state in &mut self.states {
            state.reset();
        }
        self.frames_processed = 0;
    }

    /// Reset peak holds only (does not clear RMS or overload).
    pub fn reset_peak_holds(&mut self) {
        for state in &mut self.states {
            state.reset_peak_hold();
        }
    }

    /// Total audio frames processed since the last [`BatchMeterProcessor::reset`].
    #[must_use]
    pub fn frames_processed(&self) -> u64 {
        self.frames_processed
    }

    /// Number of channels configured.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.config.channels
    }

    /// Sample rate in Hz.
    #[must_use]
    pub fn sample_rate(&self) -> f64 {
        self.config.sample_rate
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Convert linear amplitude to dBFS.
#[inline]
fn linear_to_db(linear: f64) -> f64 {
    20.0 * linear.log10()
}

/// Convert dBFS to linear amplitude.
#[inline]
fn db_to_linear(db: f64) -> f64 {
    10.0_f64.powf(db / 20.0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(channels: usize) -> BatchMeterConfig {
        BatchMeterConfig {
            channels,
            sample_rate: 48_000.0,
            rms_window_ms: 10.0, // short window for fast test convergence
            peak_hold_ms: 500.0,
            overload_threshold_db: -0.1,
        }
    }

    // 1. Default construction yields correct channel count.
    #[test]
    fn test_channel_count() {
        let proc = BatchMeterProcessor::new(make_config(4));
        assert_eq!(proc.channels(), 4);
    }

    // 2. Silence gives silence-level peak and RMS.
    #[test]
    fn test_silence_readings() {
        let mut proc = BatchMeterProcessor::new(make_config(2));
        let samples = vec![0.0_f32; 2 * 512];
        proc.process_interleaved(&samples);
        let r = proc.reading();
        // All peak readings should be at or below SILENCE_DB
        for &p in &r.peak_dbfs {
            assert!(p <= SILENCE_DB + 1.0, "expected silence, got {p}");
        }
    }

    // 3. Full-scale 0 dBFS sine yields peak near 0 dBFS.
    #[test]
    fn test_full_scale_peak() {
        let mut proc = BatchMeterProcessor::new(make_config(1));
        let samples: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.01).sin()).collect();
        proc.process_interleaved(&samples);
        let peak = proc.peak_dbfs(0).expect("channel exists");
        // Peak of a sine with amplitude ~1 should be close to 0 dBFS
        assert!(peak > -3.0, "expected near-full-scale peak, got {peak}");
    }

    // 4. RMS of a constant signal equals its dBFS value.
    #[test]
    fn test_constant_rms() {
        // A constant 0.1 signal has RMS = 0.1 → −20 dBFS
        let mut proc = BatchMeterProcessor::new(make_config(1));
        // Fill the entire RMS window with the constant value.
        let window_samples = (10.0_f64 * 48_000.0_f64 / 1_000.0_f64).ceil() as usize;
        let samples: Vec<f32> = vec![0.1_f32; window_samples];
        proc.process_interleaved(&samples);
        let rms = proc.rms_dbfs(0).expect("channel exists");
        let expected = 20.0 * (0.1_f64).log10(); // −20 dBFS
        assert!(
            (rms - expected).abs() < 0.5,
            "RMS {rms:.2} not close to expected {expected:.2}"
        );
    }

    // 5. Overload flag set when sample >= threshold.
    #[test]
    fn test_overload_detection() {
        let mut proc = BatchMeterProcessor::new(make_config(2));
        // Ch 0: below threshold; Ch 1: at threshold
        let samples: Vec<f32> = vec![
            0.5, // ch 0 – below -0.1 dBFS
            1.0, // ch 1 – exactly 0 dBFS (above -0.1 dBFS threshold)
        ];
        proc.process_interleaved(&samples);
        assert!(!proc.channel_overload(0), "ch0 should not overload");
        assert!(proc.channel_overload(1), "ch1 should overload");
        assert!(proc.any_overload());
    }

    // 6. Reset clears all state.
    #[test]
    fn test_reset_clears_state() {
        let mut proc = BatchMeterProcessor::new(make_config(2));
        let samples = vec![1.0_f32; 4]; // 2 frames, 2 channels
        proc.process_interleaved(&samples);
        proc.reset();
        let r = proc.reading();
        for &p in &r.peak_dbfs {
            assert!(
                p <= SILENCE_DB + 1.0,
                "after reset, expected silence, got {p}"
            );
        }
        assert!(
            !proc.any_overload(),
            "overload flag should be cleared after reset"
        );
        assert_eq!(proc.frames_processed(), 0);
    }

    // 7. process_channel (planar) feeds the correct lane.
    #[test]
    fn test_process_channel_planar() {
        let mut proc = BatchMeterProcessor::new(make_config(3));
        let samples: Vec<f32> = vec![0.8_f32; 256];
        proc.process_channel(1, &samples).expect("channel 1 ok");
        // Only channel 1 should show a non-silence reading.
        let p0 = proc.peak_dbfs(0).expect("ch 0");
        let p1 = proc.peak_dbfs(1).expect("ch 1");
        let p2 = proc.peak_dbfs(2).expect("ch 2");
        assert!(p0 <= SILENCE_DB + 1.0, "ch0 should be silent, got {p0}");
        assert!(p1 > -5.0, "ch1 should be loud, got {p1}");
        assert!(p2 <= SILENCE_DB + 1.0, "ch2 should be silent, got {p2}");
    }

    // 8. Out-of-range channel returns error or None.
    #[test]
    fn test_out_of_range_channel() {
        let proc = BatchMeterProcessor::new(make_config(2));
        assert!(proc.peak_dbfs(99).is_none());
        assert!(proc.rms_dbfs(99).is_none());
        assert!(proc.true_peak_dbfs(99).is_none());

        let mut proc = BatchMeterProcessor::new(make_config(2));
        assert!(proc.process_channel(99, &[0.0]).is_err());
    }

    // 9. True-peak reflects maximum absolute value seen.
    #[test]
    fn test_true_peak() {
        let mut proc = BatchMeterProcessor::new(make_config(1));
        let samples: Vec<f32> = vec![0.3, -0.9, 0.5, -0.2];
        proc.process_interleaved(&samples);
        let tp = proc.true_peak_dbfs(0).expect("channel exists");
        let expected_db = 20.0 * (0.9_f64).log10();
        assert!(
            (tp - expected_db).abs() < 0.1,
            "true peak {tp:.2} should be near {expected_db:.2}"
        );
    }

    // 10. Frames-processed counter increments correctly.
    #[test]
    fn test_frames_processed_counter() {
        let mut proc = BatchMeterProcessor::new(make_config(2));
        let samples = vec![0.0_f32; 2 * 100]; // 100 frames, 2 channels
        proc.process_interleaved(&samples);
        assert_eq!(proc.frames_processed(), 100);
        proc.reset();
        assert_eq!(proc.frames_processed(), 0);
    }

    // 11. BatchMeterReading summary fields are consistent.
    #[test]
    fn test_reading_summary_fields() {
        let mut proc = BatchMeterProcessor::new(make_config(4));
        // ch0 = 0.1, ch1 = 0.5, ch2 = 0.9 (overload), ch3 = 0.0
        let samples: Vec<f32> = vec![0.1, 0.5, 0.9, 0.0];
        proc.process_interleaved(&samples);
        let r = proc.reading();

        assert!(r.max_peak_dbfs > r.peak_dbfs[0], "max should exceed ch0");
        assert!(r.any_overload || !r.any_overload); // just checking it compiles
                                                    // ch2 at 0.9 is below the -0.1 dBFS threshold (0.9 < ~0.989) → no overload
        assert!(!r.any_overload, "0.9 is below -0.1 dBFS threshold");
    }

    // 12. reset_peak_holds does not clear RMS.
    #[test]
    fn test_reset_peak_holds_preserves_rms() {
        let mut proc = BatchMeterProcessor::new(make_config(1));
        let window = (10.0 * 48_000.0_f64 / 1_000.0).ceil() as usize;
        let samples: Vec<f32> = vec![0.5_f32; window];
        proc.process_interleaved(&samples);

        let rms_before = proc.rms_dbfs(0).expect("ch 0");
        proc.reset_peak_holds();
        let rms_after = proc.rms_dbfs(0).expect("ch 0");

        assert!(
            (rms_before - rms_after).abs() < 0.01,
            "RMS should be preserved after reset_peak_holds"
        );
    }
}
