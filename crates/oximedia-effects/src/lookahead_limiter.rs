//! Lookahead limiter for broadcast loudness compliance.
//!
//! Prevents true-peak overshoot by looking ahead at the input signal and
//! applying gain reduction before peaks arrive at the output. Suitable for
//! EBU R128, ATSC A/85, and ITU-R BS.1770 compliance workflows.
//!
//! # Algorithm
//!
//! 1. Push the input sample into a circular delay buffer.
//! 2. Compute the required gain: `target_gain = ceiling / abs(input)`, clamped to `[0, 1]`.
//! 3. If `target_gain < current_gain`: instantly reduce (attack via lookahead).
//! 4. If `target_gain > current_gain`: exponentially release toward `1.0`.
//! 5. Read the delayed sample from `lookahead_ms` ago.
//! 6. Output = `delayed_sample * gain`.
//!
//! # Example
//!
//! ```
//! use oximedia_effects::lookahead_limiter::{LookaheadLimiter, LimiterConfig};
//! use oximedia_effects::AudioEffect;
//!
//! let config = LimiterConfig::default();
//! let mut limiter = LookaheadLimiter::new(config);
//! let out = limiter.process_sample(0.5);
//! assert!(out.is_finite());
//! ```

#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use crate::AudioEffect;

// ---------------------------------------------------------------------------
// LimiterConfig
// ---------------------------------------------------------------------------

/// Lookahead limiter configuration.
#[derive(Debug, Clone)]
pub struct LimiterConfig {
    /// Ceiling in dB (maximum output level, e.g., -1.0 dBFS).
    pub ceiling_db: f32,
    /// Release time in milliseconds (50-500ms typical, 100ms default).
    pub release_ms: f32,
    /// Lookahead time in milliseconds (must match attack, 5ms default).
    pub lookahead_ms: f32,
    /// Sample rate in Hz.
    pub sample_rate: f32,
}

impl Default for LimiterConfig {
    fn default() -> Self {
        Self {
            ceiling_db: -1.0,
            release_ms: 100.0,
            lookahead_ms: 5.0,
            sample_rate: 48000.0,
        }
    }
}

// ---------------------------------------------------------------------------
// LookaheadLimiter
// ---------------------------------------------------------------------------

/// Broadcast-grade lookahead limiter that prevents true-peak overshoot.
///
/// Introduces latency equal to the configured lookahead time. Use
/// [`latency_samples`](AudioEffect::latency_samples) to query the exact
/// latency in samples.
pub struct LookaheadLimiter {
    config: LimiterConfig,
    /// Delay buffer for lookahead (circular).
    delay_buffer: Vec<f32>,
    delay_write: usize,
    delay_read: usize,
    delay_size: usize,
    /// Current gain (linear, 0.0..=1.0).
    gain: f32,
    /// Release coefficient (exponential release).
    release_coeff: f32,
    /// Ceiling in linear amplitude.
    ceiling_linear: f32,
    /// Peak hold ring buffer for gain computation.
    peak_hold: Vec<f32>,
    peak_write: usize,
}

impl LookaheadLimiter {
    /// Create a new lookahead limiter from the given configuration.
    #[must_use]
    pub fn new(config: LimiterConfig) -> Self {
        let delay_size = Self::compute_delay_size(config.lookahead_ms, config.sample_rate);
        let ceiling_linear = Self::db_to_linear(config.ceiling_db);
        let release_coeff = Self::compute_release_coeff(config.release_ms, config.sample_rate);

        Self {
            delay_buffer: vec![0.0; delay_size],
            delay_write: 0,
            delay_read: 0,
            delay_size,
            gain: 1.0,
            release_coeff,
            ceiling_linear,
            peak_hold: vec![0.0; delay_size],
            peak_write: 0,
            config,
        }
    }

    /// Set the ceiling level in dB.
    pub fn set_ceiling(&mut self, ceiling_db: f32) {
        self.config.ceiling_db = ceiling_db;
        self.ceiling_linear = Self::db_to_linear(ceiling_db);
    }

    /// Return the current ceiling in dBFS.
    #[must_use]
    pub fn ceiling_db(&self) -> f32 {
        self.config.ceiling_db
    }

    /// Return the current gain reduction in dB (positive = reduction).
    #[must_use]
    pub fn gain_reduction_db(&self) -> f32 {
        -(20.0 * self.gain.max(f32::EPSILON).log10())
    }

    /// Process a buffer of mono samples, writing results into `output`.
    ///
    /// `output` must be at least as long as `input`.
    pub fn process_mono(&mut self, input: &[f32], output: &mut [f32]) {
        let len = input.len().min(output.len());
        for i in 0..len {
            output[i] = self.process_one(input[i]);
        }
    }

    /// Reset all internal state (for seeking / discontinuities).
    pub fn reset_state(&mut self) {
        self.delay_buffer.fill(0.0);
        self.delay_write = 0;
        self.delay_read = 0;
        self.peak_hold.fill(0.0);
        self.peak_write = 0;
        self.gain = 1.0;
    }

    // ── internal helpers ──────────────────────────────────────────────────

    /// Process a single sample through the limiter.
    #[inline]
    fn process_one(&mut self, input: f32) -> f32 {
        // 1. Push input absolute value into peak hold ring buffer.
        self.peak_hold[self.peak_write] = input.abs();
        self.peak_write = (self.peak_write + 1) % self.delay_size;

        // 2. Find the peak in the lookahead window.
        let peak = self.peak_hold.iter().copied().fold(0.0_f32, f32::max);

        // 3. Compute the required gain to keep output below ceiling.
        let target_gain = if peak > self.ceiling_linear {
            self.ceiling_linear / peak.max(f32::EPSILON)
        } else {
            1.0
        };

        // 4. Update gain: instant attack, smooth release.
        if target_gain < self.gain {
            // Attack: instantly reduce to required gain.
            self.gain = target_gain;
        } else {
            // Release: exponentially recover toward 1.0.
            self.gain = 1.0 - self.release_coeff * (1.0 - self.gain);
            self.gain = self.gain.min(target_gain).min(1.0);
        }

        // 5. Push input into delay buffer and read the delayed sample.
        let delayed = self.delay_buffer[self.delay_write];
        self.delay_buffer[self.delay_write] = input;
        self.delay_write = (self.delay_write + 1) % self.delay_size;

        // 6. Apply gain to the delayed signal, with a final safety clamp.
        let output = delayed * self.gain;
        if output.abs() > self.ceiling_linear {
            // Safety clamp: ensure output never exceeds ceiling even during
            // edge-case transitions.
            let safe_gain = self.ceiling_linear / delayed.abs().max(f32::EPSILON);
            self.gain = self.gain.min(safe_gain);
            return delayed * self.gain;
        }
        output
    }

    fn compute_delay_size(lookahead_ms: f32, sample_rate: f32) -> usize {
        let samples = (lookahead_ms * sample_rate / 1000.0) as usize;
        samples.max(1)
    }

    fn compute_release_coeff(release_ms: f32, sample_rate: f32) -> f32 {
        let samples = release_ms * sample_rate / 1000.0;
        if samples > 0.0 {
            (-1.0 / samples).exp()
        } else {
            0.0
        }
    }

    fn db_to_linear(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }
}

// ---------------------------------------------------------------------------
// AudioEffect impl
// ---------------------------------------------------------------------------

impl AudioEffect for LookaheadLimiter {
    const EFFECT_ID: u64 = 6002;

    fn process_sample(&mut self, input: f32) -> f32 {
        self.process_one(input)
    }

    fn reset(&mut self) {
        self.reset_state();
    }

    fn latency_samples(&self) -> usize {
        self.delay_size
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioEffect;

    #[test]
    fn test_limiter_below_ceiling_unchanged() {
        // A quiet signal (well below ceiling) should pass through unchanged
        // once the delay buffer has been filled.
        let config = LimiterConfig {
            ceiling_db: 0.0, // 0 dBFS = 1.0 linear
            sample_rate: 48000.0,
            ..LimiterConfig::default()
        };
        let mut limiter = LookaheadLimiter::new(config);

        // Feed constant quiet value through the delay.
        let quiet = 0.1_f32;
        let total = limiter.delay_size + 256;
        let mut last = 0.0_f32;
        for _ in 0..total {
            last = limiter.process_sample(quiet);
        }
        // After the delay buffer has filled, output should equal the quiet input.
        assert!(
            (last - quiet).abs() < 1e-4,
            "Quiet signal should pass through unchanged, got {last}"
        );
    }

    #[test]
    fn test_limiter_above_ceiling_limited() {
        let config = LimiterConfig {
            ceiling_db: -6.0,
            lookahead_ms: 5.0,
            release_ms: 100.0,
            sample_rate: 48000.0,
        };
        let ceiling_linear = 10.0_f32.powf(-6.0 / 20.0);
        let mut limiter = LookaheadLimiter::new(config);

        // Feed a loud signal for enough samples to fill the delay and settle.
        let loud = 1.0_f32;
        let n = limiter.delay_size * 4;
        let mut outputs = Vec::with_capacity(n);
        for _ in 0..n {
            outputs.push(limiter.process_sample(loud));
        }

        // After settling, output should not exceed ceiling.
        let settle = limiter.delay_size;
        for (i, &y) in outputs[settle..].iter().enumerate() {
            assert!(
                y.abs() <= ceiling_linear + 1e-3,
                "Sample {} exceeded ceiling: {} > {}",
                i + settle,
                y.abs(),
                ceiling_linear
            );
        }
    }

    #[test]
    fn test_limiter_output_never_exceeds_ceiling() {
        // Stress test with an impulse.
        let config = LimiterConfig {
            ceiling_db: -1.0,
            lookahead_ms: 5.0,
            release_ms: 50.0,
            sample_rate: 48000.0,
        };
        let ceiling_linear = 10.0_f32.powf(-1.0 / 20.0);
        let mut limiter = LookaheadLimiter::new(config);

        // Warm up with zeros.
        for _ in 0..limiter.delay_size * 2 {
            limiter.process_sample(0.0);
        }

        // Sudden impulse of 10.0.
        let impulse_len = 100;
        let mut outputs = Vec::with_capacity(impulse_len + limiter.delay_size * 2);
        for _ in 0..impulse_len {
            outputs.push(limiter.process_sample(10.0));
        }
        // Trail with silence to flush.
        for _ in 0..limiter.delay_size * 2 {
            outputs.push(limiter.process_sample(0.0));
        }

        for (i, &y) in outputs.iter().enumerate() {
            assert!(
                y.abs() <= ceiling_linear + 1e-3,
                "Output[{i}] = {} exceeds ceiling {}",
                y.abs(),
                ceiling_linear,
            );
        }
    }

    #[test]
    fn test_limiter_reset() {
        let config = LimiterConfig::default();
        let mut limiter = LookaheadLimiter::new(config);

        // Drive the limiter with loud signal.
        for _ in 0..1024 {
            limiter.process_sample(2.0);
        }
        assert!(
            limiter.gain < 1.0,
            "Gain should be reduced after loud input"
        );

        limiter.reset();
        assert!(
            (limiter.gain - 1.0).abs() < 1e-6,
            "After reset, gain should be 1.0"
        );
    }

    #[test]
    fn test_limiter_gain_reduction_db() {
        let config = LimiterConfig::default();
        let mut limiter = LookaheadLimiter::new(config);

        // No input yet: no gain reduction.
        assert!(limiter.gain_reduction_db().abs() < 0.01);

        // Drive with loud signal.
        for _ in 0..1024 {
            limiter.process_sample(2.0);
        }
        let gr = limiter.gain_reduction_db();
        assert!(
            gr > 0.0,
            "Gain reduction should be positive after loud input, got {gr}"
        );
    }

    #[test]
    fn test_limiter_latency() {
        let config = LimiterConfig {
            lookahead_ms: 5.0,
            sample_rate: 48000.0,
            ..LimiterConfig::default()
        };
        let limiter = LookaheadLimiter::new(config);
        let expected = (5.0 * 48000.0 / 1000.0) as usize;
        assert_eq!(
            limiter.latency_samples(),
            expected,
            "Latency should equal lookahead samples"
        );
    }

    #[test]
    fn test_limiter_default_config() {
        let config = LimiterConfig::default();
        assert_eq!(
            config.ceiling_db, -1.0,
            "Default ceiling should be -1.0 dBFS (broadcast standard)"
        );
        assert_eq!(config.release_ms, 100.0, "Default release should be 100ms");
        assert_eq!(config.lookahead_ms, 5.0, "Default lookahead should be 5ms");
        assert_eq!(
            config.sample_rate, 48000.0,
            "Default sample rate should be 48 kHz"
        );
    }

    #[test]
    fn test_limiter_set_ceiling() {
        let config = LimiterConfig::default();
        let mut limiter = LookaheadLimiter::new(config);
        assert_eq!(limiter.ceiling_db(), -1.0);

        limiter.set_ceiling(-3.0);
        assert_eq!(limiter.ceiling_db(), -3.0);

        // Verify the linear ceiling was also updated.
        let expected_linear = 10.0_f32.powf(-3.0 / 20.0);
        assert!(
            (limiter.ceiling_linear - expected_linear).abs() < 1e-6,
            "Linear ceiling should match: {} vs {}",
            limiter.ceiling_linear,
            expected_linear,
        );
    }

    // ── New tests for TODO item: lookahead limiter ────────────────────────

    #[test]
    fn test_limiter_output_finite_for_sine_input() {
        // Feed a sinusoidal signal; all outputs must be finite.
        let config = LimiterConfig {
            ceiling_db: -3.0,
            lookahead_ms: 5.0,
            release_ms: 80.0,
            sample_rate: 48_000.0,
        };
        let mut limiter = LookaheadLimiter::new(config);
        let n = 4800;
        for i in 0..n {
            let x = (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            let out = limiter.process_sample(x);
            assert!(out.is_finite(), "sample {i} not finite: {out}");
        }
    }

    #[test]
    fn test_limiter_ceiling_1_dbfs_strict() {
        // Strict test: verify no sample exceeds -1 dBFS ceiling.
        let config = LimiterConfig {
            ceiling_db: -1.0,
            lookahead_ms: 5.0,
            release_ms: 100.0,
            sample_rate: 48_000.0,
        };
        let ceiling = 10.0_f32.powf(-1.0 / 20.0);
        let mut limiter = LookaheadLimiter::new(config);

        // Sweep amplitude from 0 to 2× ceiling to stress test.
        for i in 0..2000 {
            let amp = (i as f32 / 500.0).min(2.0);
            let out = limiter.process_sample(amp);
            assert!(
                out.abs() <= ceiling + 1e-3,
                "Sample {i}: |{out}| > ceiling {ceiling}"
            );
        }
    }

    #[test]
    fn test_limiter_process_mono_output_length() {
        let config = LimiterConfig::default();
        let mut limiter = LookaheadLimiter::new(config);
        let input = vec![0.3_f32; 1024];
        let mut output = vec![0.0_f32; 1024];
        limiter.process_mono(&input, &mut output);
        assert_eq!(output.len(), 1024);
        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn test_limiter_process_mono_shorter_output() {
        // process_mono handles output shorter than input gracefully.
        let config = LimiterConfig::default();
        let mut limiter = LookaheadLimiter::new(config);
        let input = vec![0.5_f32; 512];
        let mut output = vec![0.0_f32; 256]; // shorter
        limiter.process_mono(&input, &mut output);
        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn test_limiter_release_recovers_gain() {
        // After a loud burst ends, gain should recover toward 1.0 over time.
        let config = LimiterConfig {
            ceiling_db: -6.0,
            lookahead_ms: 5.0,
            release_ms: 50.0,
            sample_rate: 48_000.0,
        };
        let mut limiter = LookaheadLimiter::new(config);

        // Drive hard for 200 ms.
        let drive_samples = (0.2 * 48_000.0) as usize;
        for _ in 0..drive_samples {
            limiter.process_sample(2.0);
        }
        let gain_after_drive = limiter.gain;

        // Then feed silence for 500 ms (10× release time).
        let release_samples = (0.5 * 48_000.0) as usize;
        for _ in 0..release_samples {
            limiter.process_sample(0.0);
        }
        let gain_after_release = limiter.gain;

        assert!(
            gain_after_release > gain_after_drive,
            "gain should recover after silence: {gain_after_release} vs {gain_after_drive}"
        );
    }

    #[test]
    fn test_limiter_multiple_ceiling_levels() {
        // Test several ceiling settings to ensure the limit is always respected.
        for ceiling_db in [-1.0_f32, -3.0, -6.0, -12.0] {
            let ceiling_lin = 10.0_f32.powf(ceiling_db / 20.0);
            let config = LimiterConfig {
                ceiling_db,
                lookahead_ms: 5.0,
                release_ms: 100.0,
                sample_rate: 48_000.0,
            };
            let mut limiter = LookaheadLimiter::new(config);

            // Fill delay + extra.
            let n = limiter.delay_size * 3;
            for _ in 0..n {
                let out = limiter.process_sample(1.0);
                assert!(
                    out.abs() <= ceiling_lin + 2e-3,
                    "ceiling={ceiling_db}: |{out}| > {ceiling_lin}"
                );
            }
        }
    }

    #[test]
    fn test_limiter_no_dc_offset_at_unity_ceiling() {
        // With ceiling=0 dBFS and a very quiet signal, output should match input.
        let config = LimiterConfig {
            ceiling_db: 0.0, // 1.0 linear
            lookahead_ms: 5.0,
            release_ms: 100.0,
            sample_rate: 48_000.0,
        };
        let mut limiter = LookaheadLimiter::new(config);

        let quiet = 0.01_f32;
        let total = limiter.delay_size + 512;
        let mut last = 0.0_f32;
        for _ in 0..total {
            last = limiter.process_sample(quiet);
        }
        assert!(
            (last - quiet).abs() < 1e-3,
            "quiet signal should pass unchanged at 0 dBFS ceiling, got {last}"
        );
    }

    #[test]
    fn test_limiter_impulse_train_all_finite() {
        // Alternating impulses exercise the attack / release cycle continuously.
        let config = LimiterConfig {
            ceiling_db: -3.0,
            lookahead_ms: 5.0,
            release_ms: 20.0,
            sample_rate: 48_000.0,
        };
        let mut limiter = LookaheadLimiter::new(config);
        let ceiling = 10.0_f32.powf(-3.0 / 20.0);

        // Alternating loud / quiet.
        for i in 0..2000 {
            let x = if i % 100 == 0 { 2.0_f32 } else { 0.05 };
            let out = limiter.process_sample(x);
            assert!(out.is_finite(), "sample {i}: not finite");
            assert!(out.abs() <= ceiling + 2e-3, "sample {i}: {out} > ceiling");
        }
    }

    #[test]
    fn test_limiter_lookahead_provides_anticipatory_gain() {
        // A limiter with lookahead should reduce gain BEFORE the peak arrives.
        // We check that at the sample just before the peak output the gain has
        // already been reduced from 1.0.
        let config = LimiterConfig {
            ceiling_db: -6.0,
            lookahead_ms: 10.0, // 480 samples at 48 kHz
            release_ms: 200.0,
            sample_rate: 48_000.0,
        };
        let mut limiter = LookaheadLimiter::new(config);

        // Prime with silence to fill delay buffer.
        for _ in 0..limiter.delay_size {
            limiter.process_sample(0.0);
        }

        // Feed a single loud sample followed by many silent ones.
        limiter.process_sample(2.0); // this peak is now in the lookahead window

        // Immediately after injecting the loud sample, the gain should be reduced.
        assert!(
            limiter.gain < 1.0,
            "gain should be reduced in anticipation of the peak: {}",
            limiter.gain
        );
    }
}
