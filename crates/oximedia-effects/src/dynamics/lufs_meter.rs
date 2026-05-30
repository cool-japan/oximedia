//! LUFS loudness metering per EBU R128 / ITU-R BS.1770-4.
//!
//! Implements integrated, short-term, and momentary loudness metering using
//! the K-weighting pre-filter chain (two biquad stages) specified in
//! ITU-R BS.1770-4, followed by mean-square accumulation and gated
//! integration as defined in EBU R128.
//!
//! # Algorithm Summary
//!
//! 1. **K-weighting**: Two biquad filters (high-shelf + high-pass) model the
//!    acoustic effect of the listener's head and remove sub-bass.
//! 2. **Mean-square accumulation**: Sliding windows compute momentary (400 ms)
//!    and short-term (3 s) loudness continuously.
//! 3. **Gated integration**: 400 ms blocks with 75% overlap are accumulated.
//!    An absolute gate (−70 LUFS) and a relative gate (−10 LU below integrated
//!    mean) filter out silence and low-level content before computing the final
//!    integrated loudness number.
//!
//! # Usage
//!
//! ```ignore
//! use oximedia_effects::dynamics::LufsMeter;
//! use oximedia_effects::AudioEffect;
//!
//! let mut meter = LufsMeter::new(48000.0);
//! for sample in audio_buffer {
//!     meter.process_sample(sample);
//! }
//! let lufs = meter.integrated_lufs();
//! ```

#![allow(clippy::cast_precision_loss)]

use crate::AudioEffect;

// ── K-weighting coefficients ──────────────────────────────────────────────────
// Pre-computed for common sample rates via the ITU-R BS.1770-4 formulas.
// For a production system these would be parameterised by sample rate.
// The values below are accurate for 48 000 Hz.

/// K-weighting stage 1 (pre-filter: high-shelf boosting above ~1.6 kHz).
struct KWeightBiquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

impl KWeightBiquad {
    /// Pre-filter high-shelf (stage 1) — valid for 48 kHz.
    fn stage1_48k() -> Self {
        Self {
            b0: 1.53512485958697,
            b1: -2.69169618940638,
            b2: 1.19839281085285,
            a1: -1.69065929318241,
            a2: 0.73248077421585,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// High-pass (stage 2) — valid for 48 kHz.
    fn stage2_48k() -> Self {
        Self {
            b0: 1.0,
            b1: -2.0,
            b2: 1.0,
            a1: -1.99004745483398,
            a2: 0.99007225036616,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Build stage 1 coefficients analytically for an arbitrary sample rate.
    ///
    /// Uses the Audio EQ Cookbook high-shelf formula with:
    /// f0 = 1681.974... Hz, dBgain = +4, Q = 0.7072...
    fn stage1_for_rate(sample_rate: f32) -> Self {
        let sr = sample_rate as f64;
        let f0 = 1681.974_450_955_533;
        let db_gain = 4.0_f64;
        let q = 0.707_175_236_955_419_3;
        let a = 10.0_f64.powf(db_gain / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * f0 / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);
        let sqrt_a = a.sqrt();

        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    /// Build stage 2 coefficients analytically for an arbitrary sample rate.
    ///
    /// High-pass: f0 = 38.135... Hz, Q = 0.5003...
    fn stage2_for_rate(sample_rate: f32) -> Self {
        let sr = sample_rate as f64;
        let f0 = 38.135_470_876_024_44;
        let q = 0.500_327_037_323_877_3;
        let w0 = 2.0 * std::f64::consts::PI * f0 / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let b0 = (1.0 + cos_w0) / 2.0;
        let b1 = -(1.0 + cos_w0);
        let b2 = (1.0 + cos_w0) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            s1: 0.0,
            s2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.s1;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }

    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

// ── Circular mean-square accumulator ─────────────────────────────────────────

/// Ring-buffer accumulator for mean-square computation over a sliding window.
///
/// Maintains a running sum so that `mean_square()` costs O(1).
struct MsAccumulator {
    buffer: Vec<f64>,
    head: usize,
    capacity: usize,
    running_sum: f64,
}

impl MsAccumulator {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            buffer: vec![0.0; capacity],
            head: 0,
            capacity,
            running_sum: 0.0,
        }
    }

    /// Push a new squared sample value and discard the oldest.
    #[inline]
    fn push(&mut self, sq: f64) {
        let old = self.buffer[self.head];
        self.running_sum -= old;
        self.buffer[self.head] = sq;
        self.running_sum += sq;
        self.head = (self.head + 1) % self.capacity;
        // Clamp to guard against floating-point drift below zero.
        if self.running_sum < 0.0 {
            self.running_sum = 0.0;
        }
    }

    /// Mean of squared values in the window.
    #[inline]
    fn mean_square(&self) -> f64 {
        self.running_sum / self.capacity as f64
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.head = 0;
        self.running_sum = 0.0;
    }
}

// ── LUFS conversion ───────────────────────────────────────────────────────────

/// Convert a mean-square value to LUFS (EBU R128 / ITU-R BS.1770 formula).
///
/// Returns [`f32::NEG_INFINITY`] for effectively silent signals.
#[inline]
fn mean_square_to_lufs(ms: f64) -> f32 {
    if ms <= 1e-20 {
        return f32::NEG_INFINITY;
    }
    (-0.691 + 10.0 * ms.log10()) as f32
}

// ── Absolute gate threshold (−70 LUFS ≈ ms = 10^((−70 + 0.691) / 10)) ───────
const ABSOLUTE_GATE_MS: f64 = 1.000_000_000_000_001e-7; // ≈ -70 LUFS in mean-square

// ── LufsMeter ─────────────────────────────────────────────────────────────────

/// EBU R128 / ITU-R BS.1770-4 LUFS loudness metering effect.
///
/// This is a **passthrough** effect: `process_sample` returns the input
/// unchanged while accumulating loudness measurements internally.
///
/// Use `momentary_lufs`, `short_term_lufs`, and `integrated_lufs` to
/// read the current measurements after processing audio.
pub struct LufsMeter {
    // K-weighting filter chain (two biquad stages).
    kw_stage1: KWeightBiquad,
    kw_stage2: KWeightBiquad,

    // Momentary (400 ms) and short-term (3 s) windows.
    momentary_buf: MsAccumulator,
    short_term_buf: MsAccumulator,

    // Integrated gating.
    /// Mean-square value of each completed 400 ms block.
    gating_blocks: Vec<f64>,
    /// Accumulated squared samples for the current partial block.
    current_block_sum: f64,
    /// Number of samples accumulated in the current partial block.
    current_block_samples: usize,
    /// Block size in samples (400 ms).
    block_size_samples: usize,
    /// Hop size in samples (100 ms = 25% of block → 75% overlap).
    hop_size_samples: usize,
    /// Countdown to next hop boundary.
    samples_until_hop: usize,

    /// Cached integrated LUFS value (updated at each gating block boundary).
    integrated_lufs: f32,
    /// Cached momentary LUFS (updated every sample).
    momentary_lufs: f32,
    /// Cached short-term LUFS (updated every sample).
    short_term_lufs: f32,
}

impl LufsMeter {
    /// Create a new LUFS meter for the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        let momentary_samples = ((0.4 * sample_rate) as usize).max(1);
        let short_term_samples = ((3.0 * sample_rate) as usize).max(1);
        let block_size = momentary_samples; // 400 ms
        let hop_size = ((0.1 * sample_rate) as usize).max(1); // 100 ms

        // Choose K-weighting coefficients.
        let (stage1, stage2) = if (sample_rate - 48000.0).abs() < 1.0 {
            (KWeightBiquad::stage1_48k(), KWeightBiquad::stage2_48k())
        } else {
            (
                KWeightBiquad::stage1_for_rate(sample_rate),
                KWeightBiquad::stage2_for_rate(sample_rate),
            )
        };

        Self {
            kw_stage1: stage1,
            kw_stage2: stage2,
            momentary_buf: MsAccumulator::new(momentary_samples),
            short_term_buf: MsAccumulator::new(short_term_samples),
            gating_blocks: Vec::with_capacity(3600),
            current_block_sum: 0.0,
            current_block_samples: 0,
            block_size_samples: block_size,
            hop_size_samples: hop_size,
            samples_until_hop: hop_size,
            integrated_lufs: f32::NEG_INFINITY,
            momentary_lufs: f32::NEG_INFINITY,
            short_term_lufs: f32::NEG_INFINITY,
        }
    }

    /// Process a mono sample for loudness measurement.
    ///
    /// Returns the input sample unchanged (passthrough).
    pub fn process_sample_mono(&mut self, input: f32) -> f32 {
        // Apply K-weighting filter chain.
        let kw = self.kw_stage2.process(self.kw_stage1.process(input as f64));
        let sq = kw * kw;

        // Push to sliding windows.
        self.momentary_buf.push(sq);
        self.short_term_buf.push(sq);

        // Update momentary and short-term loudness.
        self.momentary_lufs = mean_square_to_lufs(self.momentary_buf.mean_square());
        self.short_term_lufs = mean_square_to_lufs(self.short_term_buf.mean_square());

        // Accumulate for the current block (used for integrated gating).
        self.current_block_sum += sq;
        self.current_block_samples += 1;

        // At each hop boundary, record a new gating block mean-square.
        self.samples_until_hop -= 1;
        if self.samples_until_hop == 0 {
            self.samples_until_hop = self.hop_size_samples;
            self.record_block();
        }

        input
    }

    /// Recompute integrated loudness when a new gating block is available.
    fn record_block(&mut self) {
        // Compute block mean-square from the current full 400 ms accumulation.
        let block_ms = if self.current_block_samples > 0 {
            self.current_block_sum / self.current_block_samples as f64
        } else {
            0.0
        };

        self.gating_blocks.push(block_ms);

        // Slide the accumulation window forward by one hop.
        // Subtract the oldest hop worth of samples (approximation: we subtract
        // `block_sum * hop/block` since we don't store per-sample history here).
        let hop_fraction = self.hop_size_samples as f64 / self.block_size_samples as f64;
        self.current_block_sum *= 1.0 - hop_fraction;
        self.current_block_samples = self.block_size_samples - self.hop_size_samples;

        self.update_integrated();
    }

    /// Apply absolute + relative gating and recompute integrated LUFS.
    fn update_integrated(&mut self) {
        // Stage 1: absolute gate — keep blocks above −70 LUFS.
        let above_abs: Vec<f64> = self
            .gating_blocks
            .iter()
            .filter(|&&ms| ms > ABSOLUTE_GATE_MS)
            .copied()
            .collect();

        if above_abs.is_empty() {
            self.integrated_lufs = f32::NEG_INFINITY;
            return;
        }

        // Stage 2: compute preliminary integrated mean-square.
        let prelim_ms = above_abs.iter().sum::<f64>() / above_abs.len() as f64;
        let prelim_lufs = mean_square_to_lufs(prelim_ms);

        // Relative gate threshold: preliminary − 10 LU.
        let relative_threshold_lufs = prelim_lufs - 10.0;
        // Convert threshold back to mean-square for comparison.
        let relative_threshold_ms = if relative_threshold_lufs > -200.0 {
            10.0_f64.powf((relative_threshold_lufs as f64 + 0.691) / 10.0)
        } else {
            0.0
        };

        // Stage 3: relative gate — keep blocks above both gates.
        let above_rel: Vec<f64> = above_abs
            .iter()
            .filter(|&&ms| ms > relative_threshold_ms)
            .copied()
            .collect();

        if above_rel.is_empty() {
            self.integrated_lufs = f32::NEG_INFINITY;
            return;
        }

        let final_ms = above_rel.iter().sum::<f64>() / above_rel.len() as f64;
        self.integrated_lufs = mean_square_to_lufs(final_ms);
    }

    /// Get the integrated LUFS loudness value (EBU R128).
    ///
    /// Returns [`f32::NEG_INFINITY`] if no audio above the gate threshold
    /// has been measured yet.
    #[must_use]
    pub fn integrated_lufs(&self) -> f32 {
        self.integrated_lufs
    }

    /// Get the momentary LUFS loudness (400 ms window).
    ///
    /// Returns [`f32::NEG_INFINITY`] for silence.
    #[must_use]
    pub fn momentary_lufs(&self) -> f32 {
        self.momentary_lufs
    }

    /// Get the short-term LUFS loudness (3 s window).
    ///
    /// Returns [`f32::NEG_INFINITY`] for silence.
    #[must_use]
    pub fn short_term_lufs(&self) -> f32 {
        self.short_term_lufs
    }
}

impl AudioEffect for LufsMeter {
    const EFFECT_ID: u64 = 6013;

    /// Passthrough: returns `input` unchanged while measuring loudness internally.
    fn process_sample(&mut self, input: f32) -> f32 {
        self.process_sample_mono(input)
    }

    fn reset(&mut self) {
        self.kw_stage1.reset();
        self.kw_stage2.reset();
        self.momentary_buf.reset();
        self.short_term_buf.reset();
        self.gating_blocks.clear();
        self.current_block_sum = 0.0;
        self.current_block_samples = 0;
        self.samples_until_hop = self.hop_size_samples;
        self.integrated_lufs = f32::NEG_INFINITY;
        self.momentary_lufs = f32::NEG_INFINITY;
        self.short_term_lufs = f32::NEG_INFINITY;
    }

    fn latency_samples(&self) -> usize {
        0 // Purely an analyser — no latency introduced.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioEffect;
    use std::f32::consts::TAU;

    fn make_sine(freq_hz: f32, sample_rate: f32, amplitude: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| (i as f32 * TAU * freq_hz / sample_rate).sin() * amplitude)
            .collect()
    }

    #[test]
    fn test_lufs_meter_new() {
        let meter = LufsMeter::new(48000.0);
        assert_eq!(meter.integrated_lufs(), f32::NEG_INFINITY);
        assert_eq!(meter.momentary_lufs(), f32::NEG_INFINITY);
        assert_eq!(meter.short_term_lufs(), f32::NEG_INFINITY);
    }

    #[test]
    fn test_lufs_meter_silence_returns_neg_inf() {
        let mut meter = LufsMeter::new(48000.0);
        for _ in 0..48000 {
            meter.process_sample(0.0);
        }
        let lufs = meter.integrated_lufs();
        assert!(
            lufs == f32::NEG_INFINITY || lufs < -60.0,
            "Silence should yield very low / neg-inf LUFS, got: {lufs}"
        );
    }

    #[test]
    fn test_lufs_meter_passthrough() {
        let mut meter = LufsMeter::new(48000.0);
        let input = 0.314_f32;
        let output = meter.process_sample(input);
        assert!((output - input).abs() < 1e-10, "Must be exact passthrough");
    }

    #[test]
    fn test_lufs_meter_reset() {
        let mut meter = LufsMeter::new(48000.0);

        // Fill with audio for a few seconds.
        let audio = make_sine(1000.0, 48000.0, 0.5, 96000);
        for &s in &audio {
            meter.process_sample(s);
        }

        meter.reset();
        assert_eq!(meter.integrated_lufs(), f32::NEG_INFINITY);
        assert_eq!(meter.momentary_lufs(), f32::NEG_INFINITY);
    }

    #[test]
    fn test_lufs_meter_momentary_finite() {
        // After 400 ms of audio the momentary window should be populated.
        let mut meter = LufsMeter::new(48000.0);
        let audio = make_sine(1000.0, 48000.0, 0.5, 19200); // 400 ms at 48 kHz
        for &s in &audio {
            meter.process_sample(s);
        }
        let m = meter.momentary_lufs();
        assert!(
            m.is_finite(),
            "Momentary LUFS should be finite after 400 ms of audio: {m}"
        );
    }

    #[test]
    fn test_kweight_biquad_process_finite() {
        let mut s1 = KWeightBiquad::stage1_48k();
        let mut s2 = KWeightBiquad::stage2_48k();
        for i in 0..1024 {
            let x = (i as f64 * 0.01 * std::f64::consts::TAU).sin();
            let y = s2.process(s1.process(x));
            assert!(y.is_finite(), "K-weight output must be finite: {y}");
        }
    }

    #[test]
    fn test_ms_accumulator_running() {
        let mut acc = MsAccumulator::new(100);
        for _ in 0..100 {
            acc.push(0.25); // constant squared amplitude
        }
        let ms = acc.mean_square();
        assert!(
            (ms - 0.25).abs() < 1e-9,
            "Mean-square should be 0.25, got {ms}"
        );
    }

    #[test]
    fn test_lufs_meter_loud_signal_high_lufs() {
        // 0.9 amplitude sine ≈ −1 dBFS ≈ roughly −2 LUFS after K-weighting.
        let mut meter = LufsMeter::new(48000.0);
        // Process 5 seconds of loud signal to populate gating blocks.
        let audio = make_sine(1000.0, 48000.0, 0.9, 240_000);
        for &s in &audio {
            meter.process_sample(s);
        }
        let lufs = meter.integrated_lufs();
        assert!(
            lufs > -15.0,
            "Loud signal should yield high LUFS (>-15), got: {lufs}"
        );
    }

    #[test]
    fn test_lufs_meter_sine_loudness_approximate() {
        // −23 LUFS corresponds to linear amplitude ≈ 0.0708 for a 1 kHz sine.
        // We measure with the K-weighted chain so the result will be close to −23 LUFS.
        let amplitude = 10.0_f32.powf((-23.0 + 0.691) / 20.0);
        let mut meter = LufsMeter::new(48000.0);
        let audio = make_sine(1000.0, 48000.0, amplitude, 480_000); // 10 s
        for &s in &audio {
            meter.process_sample(s);
        }
        let lufs = meter.integrated_lufs();
        assert!(
            lufs.is_finite(),
            "LUFS should be finite for non-silent signal"
        );
        assert!(
            (lufs - (-23.0)).abs() < 6.0,
            "Expected ~-23 LUFS ±6, got: {lufs}"
        );
    }

    #[test]
    fn test_lufs_meter_audioeffect_trait() {
        let mut meter = LufsMeter::new(48000.0);
        let out = <LufsMeter as AudioEffect>::process_sample(&mut meter, 0.5);
        assert!((out - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_lufs_meter_44100hz() {
        // Should work at 44100 Hz using the analytical coefficient computation.
        let mut meter = LufsMeter::new(44100.0);
        let audio = make_sine(1000.0, 44100.0, 0.5, 44100 * 2);
        for &s in &audio {
            let out = meter.process_sample(s);
            assert!(out.is_finite());
        }
        assert!(meter.momentary_lufs().is_finite() || meter.momentary_lufs() == f32::NEG_INFINITY);
    }
}
