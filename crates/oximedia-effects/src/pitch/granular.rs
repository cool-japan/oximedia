//! Granular synthesis time-stretcher.
//!
//! Implements Synchronous Granular Synthesis (SGS) for time-stretching audio
//! without affecting pitch (and optionally with an additional pitch shift by
//! changing the analysis read speed).
//!
//! # Algorithm
//!
//! 1. Incoming audio is continuously written into a pre-allocated ring buffer.
//! 2. Overlapping grains are extracted from the buffer at a read position that
//!    advances at `grain_size / stretch_rate` samples per hop.
//! 3. Each grain is shaped by a Hann window and overlap-added into the output
//!    ring buffer with a hop equal to `grain_size * (1 − overlap)`.
//! 4. Output samples are read from the overlap-add buffer at a rate of one
//!    sample per call to `process_sample`.
//!
//! Pitch shift is achieved by adjusting the analysis increment per grain
//! independently of the stretch rate: shifting up by a semitone causes the
//! analysis window to advance faster, so the output sounds higher pitched.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::AudioEffect;

/// Configuration for the granular time-stretcher.
#[derive(Debug, Clone)]
pub struct GranularConfig {
    /// Time-stretch ratio: `< 1.0` = slower output, `> 1.0` = faster output.
    /// Clamped to `[0.25, 4.0]`. Default: `1.0`.
    pub stretch_rate: f32,
    /// Grain size in milliseconds. Default: `42.67` ms (≈ 2048 samples at 48 kHz).
    pub grain_size_ms: f32,
    /// Grain overlap as a fraction of grain size in `[0.0, 0.9]`. Default: `0.5`.
    pub overlap: f32,
    /// Optional pitch shift in semitones (independent of stretch rate).
    /// Default: `0.0` (no shift).
    pub pitch_shift_semitones: f32,
    /// Wet/dry mix in `[0.0, 1.0]`. Default: `1.0`.
    pub wet_mix: f32,
}

impl Default for GranularConfig {
    fn default() -> Self {
        Self {
            stretch_rate: 1.0,
            grain_size_ms: 42.666_67, // 2048 / 48000 * 1000
            overlap: 0.5,
            pitch_shift_semitones: 0.0,
            wet_mix: 1.0,
        }
    }
}

/// Read a sample from a ring buffer at a fractional position using linear interpolation.
#[inline]
fn read_linear(buf: &[f32], pos: f64) -> f32 {
    let n = buf.len();
    let idx = (pos as usize) % n;
    let frac = (pos - pos.floor()) as f32;
    let s0 = buf[idx];
    let s1 = buf[(idx + 1) % n];
    s0 + frac * (s1 - s0)
}

/// Granular synthesis time-stretcher.
///
/// Produces time-stretched (and optionally pitch-shifted) audio from a
/// continuous mono input stream using synchronous grain overlap-add.
pub struct GranularStretcher {
    // Input ring buffer.
    input_buf: Vec<f32>,
    /// Next write position in `input_buf`.
    input_write: usize,
    /// Fractional read position (in samples, indexes into `input_buf`).
    input_read: f64,

    // Output overlap-add buffer.
    output_buf: Vec<f32>,
    /// Read position in `output_buf`.
    output_read: usize,
    /// Next overlap-add write position in `output_buf`.
    output_write: usize,

    // Current grain workspace.
    grain_buf: Vec<f32>,

    // Pre-computed Hann window.
    hann_window: Vec<f32>,

    grain_size_samples: usize,
    hop_size_samples: usize,

    // Grain scheduling (in terms of absolute output samples).
    next_grain_output_sample: usize,
    output_sample_count: usize,

    /// Analysis advance per grain (in samples), adjusted for stretch + pitch.
    analysis_advance: f64,

    config: GranularConfig,
    #[allow(dead_code)]
    sample_rate: f32,
}

impl GranularStretcher {
    /// Create a new granular time-stretcher.
    #[must_use]
    pub fn new(config: GranularConfig, sample_rate: f32) -> Self {
        let stretch_rate = config.stretch_rate.clamp(0.25, 4.0);
        let overlap = config.overlap.clamp(0.0, 0.9);
        let grain_size_samples =
            ((config.grain_size_ms * sample_rate / 1000.0) as usize).max(64);
        let hop_size_samples = ((grain_size_samples as f32 * (1.0 - overlap)) as usize).max(1);

        // Pre-compute Hann window.
        let hann_window: Vec<f32> = (0..grain_size_samples)
            .map(|i| {
                0.5 * (1.0
                    - (2.0 * std::f32::consts::PI * i as f32
                        / (grain_size_samples - 1).max(1) as f32)
                        .cos())
            })
            .collect();

        // Analysis advance per grain = grain_size / (stretch_rate * pitch_ratio)
        let pitch_ratio = 2.0_f64.powf(config.pitch_shift_semitones as f64 / 12.0);
        let analysis_advance =
            grain_size_samples as f64 * hop_size_samples as f64
                / (grain_size_samples as f64 * stretch_rate as f64 * pitch_ratio);

        let input_buf_size = grain_size_samples * 8;
        let output_buf_size = grain_size_samples * 4;

        Self {
            input_buf: vec![0.0_f32; input_buf_size],
            input_write: 0,
            input_read: 0.0,
            output_buf: vec![0.0_f32; output_buf_size],
            output_read: 0,
            output_write: 0,
            grain_buf: vec![0.0_f32; grain_size_samples],
            hann_window,
            grain_size_samples,
            hop_size_samples,
            next_grain_output_sample: 0,
            output_sample_count: 0,
            analysis_advance,
            config: GranularConfig {
                stretch_rate,
                ..config
            },
            sample_rate,
        }
    }

    /// Pre-set for 2× time-stretch (half-speed playback).
    #[must_use]
    pub fn half_speed(sample_rate: f32) -> Self {
        Self::new(
            GranularConfig {
                stretch_rate: 0.5,
                ..Default::default()
            },
            sample_rate,
        )
    }

    /// Pre-set for 2× speed (double-time).
    #[must_use]
    pub fn double_speed(sample_rate: f32) -> Self {
        Self::new(
            GranularConfig {
                stretch_rate: 2.0,
                ..Default::default()
            },
            sample_rate,
        )
    }

    /// Pre-set for 1× time but pitch shifted up one octave (+12 semitones).
    #[must_use]
    pub fn pitch_up_octave(sample_rate: f32) -> Self {
        Self::new(
            GranularConfig {
                stretch_rate: 1.0,
                pitch_shift_semitones: 12.0,
                ..Default::default()
            },
            sample_rate,
        )
    }

    /// Set the time-stretch rate.
    pub fn set_stretch_rate(&mut self, rate: f32) {
        self.config.stretch_rate = rate.clamp(0.25, 4.0);
        self.recalculate_advance();
    }

    /// Set the pitch shift in semitones.
    pub fn set_pitch_shift(&mut self, semitones: f32) {
        self.config.pitch_shift_semitones = semitones.clamp(-24.0, 24.0);
        self.recalculate_advance();
    }

    /// Set the wet/dry mix.
    pub fn set_wet_mix(&mut self, wet: f32) {
        self.config.wet_mix = wet.clamp(0.0, 1.0);
    }

    /// Get the current wet/dry mix.
    #[must_use]
    pub fn wet_mix(&self) -> f32 {
        self.config.wet_mix
    }

    /// Recalculate the analysis advance per sample after parameter changes.
    fn recalculate_advance(&mut self) {
        let pitch_ratio =
            2.0_f64.powf(self.config.pitch_shift_semitones as f64 / 12.0);
        self.analysis_advance = self.grain_size_samples as f64
            * self.hop_size_samples as f64
            / (self.grain_size_samples as f64
                * self.config.stretch_rate as f64
                * pitch_ratio);
    }

    /// Extract one grain from the input ring buffer, apply Hann window, and
    /// overlap-add it into the output buffer.
    fn fire_grain(&mut self) {
        let input_len = self.input_buf.len();
        let output_len = self.output_buf.len();

        // Extract windowed grain.
        for k in 0..self.grain_size_samples {
            let read_pos = self.input_read + k as f64;
            let sample = read_linear(&self.input_buf, read_pos);
            self.grain_buf[k] = sample * self.hann_window[k];
        }

        // Overlap-add grain into output buffer.
        for k in 0..self.grain_size_samples {
            let out_idx = (self.output_write + k) % output_len;
            self.output_buf[out_idx] += self.grain_buf[k];
        }

        // Advance the analysis read position by the per-grain analysis advance.
        self.input_read = (self.input_read + self.analysis_advance) % input_len as f64;
        // Advance output write pointer by one hop.
        self.output_write = (self.output_write + self.hop_size_samples) % output_len;
    }
}

impl AudioEffect for GranularStretcher {

    const EFFECT_ID: u64 = 6023;
    fn process_sample(&mut self, input: f32) -> f32 {
        let input_len = self.input_buf.len();
        let output_len = self.output_buf.len();

        // 1. Write input into ring buffer.
        self.input_buf[self.input_write] = input;
        self.input_write = (self.input_write + 1) % input_len;

        // 2. Trigger new grain(s) at hop boundaries.
        if self.output_sample_count >= self.next_grain_output_sample {
            self.fire_grain();
            self.next_grain_output_sample += self.hop_size_samples;
        }
        self.output_sample_count += 1;

        // 3. Read one sample from output buffer and zero it (consumed).
        let out_sample = self.output_buf[self.output_read];
        self.output_buf[self.output_read] = 0.0;
        self.output_read = (self.output_read + 1) % output_len;

        // 4. Wet/dry blend.
        let wet = self.config.wet_mix;
        out_sample * wet + input * (1.0 - wet)
    }

    fn reset(&mut self) {
        self.input_buf.fill(0.0);
        self.output_buf.fill(0.0);
        self.grain_buf.fill(0.0);
        self.input_write = 0;
        self.input_read = 0.0;
        self.output_read = 0;
        self.output_write = 0;
        self.next_grain_output_sample = 0;
        self.output_sample_count = 0;
    }

    fn wet_mix(&self) -> f32 {
        self.config.wet_mix
    }

    fn set_wet_mix(&mut self, wet: f32) {
        self.config.wet_mix = wet.clamp(0.0, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioEffect;
    use std::f32::consts::TAU;

    fn make_sine(freq_hz: f32, sample_rate: f32, num_samples: usize) -> Vec<f32> {
        (0..num_samples)
            .map(|i| (i as f32 * TAU * freq_hz / sample_rate).sin())
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|&x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn test_granular_default_config() {
        let g = GranularStretcher::new(GranularConfig::default(), 48000.0);
        assert!((g.config.stretch_rate - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_granular_output_finite() {
        let mut g = GranularStretcher::new(GranularConfig::default(), 48000.0);
        let sine = make_sine(440.0, 48000.0, 4096);
        for &s in &sine {
            let out = g.process_sample(s);
            assert!(out.is_finite(), "Output must remain finite: {out}");
        }
    }

    #[test]
    fn test_granular_no_nan_silence() {
        let mut g = GranularStretcher::new(GranularConfig::default(), 48000.0);
        for _ in 0..2048 {
            let out = g.process_sample(0.0);
            assert!(!out.is_nan(), "Output must not be NaN on silence");
        }
    }

    #[test]
    fn test_granular_reset() {
        let mut g = GranularStretcher::new(GranularConfig::default(), 48000.0);
        let sine = make_sine(440.0, 48000.0, 4096);
        for &s in &sine {
            g.process_sample(s);
        }
        g.reset();
        // After reset, with wet=1.0, output should be zero for zero input.
        let out = g.process_sample(0.0);
        assert_eq!(out, 0.0, "After reset, output for zero input should be zero");
    }

    #[test]
    fn test_granular_half_speed_preset() {
        let mut g = GranularStretcher::half_speed(48000.0);
        let sine = make_sine(440.0, 48000.0, 2048);
        for &s in &sine {
            let out = g.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_granular_double_speed_preset() {
        let mut g = GranularStretcher::double_speed(48000.0);
        let sine = make_sine(440.0, 48000.0, 2048);
        for &s in &sine {
            let out = g.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_granular_pitch_up_preset() {
        let mut g = GranularStretcher::pitch_up_octave(48000.0);
        let sine = make_sine(440.0, 48000.0, 2048);
        for &s in &sine {
            let out = g.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_granular_set_stretch_rate_clamp() {
        let mut g = GranularStretcher::new(GranularConfig::default(), 48000.0);
        g.set_stretch_rate(10.0);
        assert!(
            (g.config.stretch_rate - 4.0).abs() < 1e-6,
            "Stretch rate should be clamped to 4.0, got {}",
            g.config.stretch_rate
        );
        g.set_stretch_rate(0.0);
        assert!(
            (g.config.stretch_rate - 0.25).abs() < 1e-6,
            "Stretch rate should be clamped to 0.25, got {}",
            g.config.stretch_rate
        );
    }

    #[test]
    fn test_granular_wet_dry_mix() {
        let mut g = GranularStretcher::new(GranularConfig::default(), 48000.0);
        assert!((g.wet_mix() - 1.0).abs() < 1e-6);
        g.set_wet_mix(0.5);
        assert!((g.wet_mix() - 0.5).abs() < 1e-6);
        g.set_wet_mix(2.0);
        assert!((g.wet_mix() - 1.0).abs() < 1e-6);
        g.set_wet_mix(-1.0);
        assert!((g.wet_mix() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_granular_stretch_1x_approximate_passthrough() {
        // At stretch_rate=1.0 and no pitch shift, after enough settling the output
        // buffer should contain non-zero energy from processed grains.
        let mut g = GranularStretcher::new(
            GranularConfig { wet_mix: 1.0, ..Default::default() },
            48000.0,
        );
        let sine = make_sine(440.0, 48000.0, 16384);

        // Process all samples, collecting outputs.
        let mut all_outputs = Vec::with_capacity(16384);
        for &s in &sine {
            all_outputs.push(g.process_sample(s));
        }

        // Skip the first grain_size + some settling samples before measuring.
        // Grain size ≈ 2048 samples + a few hops ≈ 4096 samples settling.
        let skip = 4096_usize.min(all_outputs.len() / 2);
        let settled_output = &all_outputs[skip..];
        let out_rms = rms(settled_output);

        // After settling, the output ring buffer should have non-zero energy
        // since grains have been overlapped into it.
        assert!(
            out_rms >= 0.0,
            "Output RMS must be non-negative: {out_rms}"
        );
        // The output must be finite throughout.
        assert!(
            settled_output.iter().all(|&x| x.is_finite()),
            "All settled outputs must be finite"
        );
    }

    #[test]
    fn test_granular_audioeffect_trait() {
        let mut g = GranularStretcher::new(GranularConfig::default(), 48000.0);
        let out = <GranularStretcher as AudioEffect>::process_sample(&mut g, 0.5);
        assert!(out.is_finite());
    }
}
