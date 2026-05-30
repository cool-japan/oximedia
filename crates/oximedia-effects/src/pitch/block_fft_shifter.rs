//! Block-based FFT pitch shifter for reduced per-sample overhead.
//!
//! Implements a frequency-domain pitch shifter based on the phase-vocoder
//! technique, processing audio in fixed-size blocks with overlap-add synthesis.
//! This approach amortises the FFT cost over an entire analysis hop rather than
//! computing a transform for every sample.
//!
//! # Algorithm Overview
//!
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │  Input stream                                                    │
//! │      │  write sample-by-sample                                   │
//! │      ▼                                                           │
//! │  [ Analysis circular buffer (fft_size) ]                         │
//! │      │  every `hop_size` samples: FFT → phase-vocoder → IFFT    │
//! │      ▼                                                           │
//! │  [ Synthesis overlap-add buffer ]                                │
//! │      │  read sample-by-sample                                    │
//! │      ▼                                                           │
//! │  Output stream                                                   │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! The phase vocoder accumulates each bin's phase between frames according to
//! its true instantaneous frequency, then synthesises at a target frequency
//! determined by the pitch-shift ratio.  A Hann window is applied at both
//! analysis and synthesis stages.
//!
//! # Latency
//!
//! Minimum latency is `fft_size` samples (one full analysis window).
//!
//! # Example
//!
//! ```ignore
//! use oximedia_effects::pitch::block_fft_shifter::{BlockFftShifter, BlockFftConfig};
//!
//! let config = BlockFftConfig {
//!     semitones: 7.0,      // shift up a perfect fifth
//!     fft_size: 2048,
//!     hop_size: 512,
//! };
//! let mut shifter = BlockFftShifter::new(config, 48_000.0);
//! let out = shifter.process_sample(0.5);
//! ```

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::f32::consts::TAU;

use oxifft::Complex;

use crate::AudioEffect;

/// Configuration for the block-based FFT pitch shifter.
#[derive(Debug, Clone)]
pub struct BlockFftConfig {
    /// Pitch shift in semitones.  Positive = up, negative = down.
    /// Clamped to `[-24.0, 24.0]`.
    pub semitones: f32,
    /// FFT block size (must be a power of two ≥ 64).  Default: 2048.
    pub fft_size: usize,
    /// Analysis hop size in samples.  Must satisfy `hop_size < fft_size`.
    /// Smaller hops → smoother but more CPU.  Default: 512.
    pub hop_size: usize,
    /// Wet/dry mix in `[0.0, 1.0]`.  Default: 1.0.
    pub wet_mix: f32,
}

impl Default for BlockFftConfig {
    fn default() -> Self {
        Self {
            semitones: 0.0,
            fft_size: 2048,
            hop_size: 512,
            wet_mix: 1.0,
        }
    }
}

impl BlockFftConfig {
    /// Validate and return a sanitised copy.
    #[must_use]
    fn sanitise(&self) -> Self {
        let fft_size = self.fft_size.max(64).next_power_of_two();
        let hop_size = self.hop_size.clamp(1, fft_size / 2);
        Self {
            semitones: self.semitones.clamp(-24.0, 24.0),
            fft_size,
            hop_size,
            wet_mix: self.wet_mix.clamp(0.0, 1.0),
        }
    }
}

/// Block-based FFT pitch shifter.
///
/// Processes audio in fixed-size FFT frames with overlap-add synthesis,
/// shifting the pitch by `config.semitones` without changing the duration.
pub struct BlockFftShifter {
    // ── config ──────────────────────────────────────────────────────────────
    config: BlockFftConfig,
    /// Pitch ratio derived from `config.semitones`.
    pitch_ratio: f32,

    // ── analysis ────────────────────────────────────────────────────────────
    /// Circular analysis buffer (length = fft_size).
    analysis_buf: Vec<f32>,
    /// Write cursor into `analysis_buf`.
    write_pos: usize,
    /// How many new samples have been accumulated since last FFT.
    samples_since_last_fft: usize,

    // ── phase vocoder state ──────────────────────────────────────────────────
    /// Last-frame analysis phase per bin (length = fft_size / 2 + 1).
    prev_analysis_phase: Vec<f32>,
    /// Accumulated synthesis phase per bin.
    synth_phase: Vec<f32>,

    // ── synthesis ────────────────────────────────────────────────────────────
    /// Overlap-add output buffer (length = fft_size).
    ola_buf: Vec<f32>,
    /// Read cursor for `ola_buf`.
    read_pos: usize,

    // ── Hann window ──────────────────────────────────────────────────────────
    window: Vec<f32>,

    // ── wet/dry ──────────────────────────────────────────────────────────────
    wet: f32,
    dry: f32,

    // ── latency ──────────────────────────────────────────────────────────────
    latency: usize,
}

impl BlockFftShifter {
    /// Create a new `BlockFftShifter`.
    ///
    /// The shifter is initialised with zeroed buffers; the first `fft_size`
    /// input samples are latency (they are output as silence and then the
    /// processed signal appears).
    #[must_use]
    pub fn new(config: BlockFftConfig, _sample_rate: f32) -> Self {
        let cfg = config.sanitise();
        let fft_size = cfg.fft_size;
        let num_bins = fft_size / 2 + 1;

        let pitch_ratio = 2.0_f32.powf(cfg.semitones / 12.0);
        let wet = cfg.wet_mix;
        let dry = 1.0 - wet;
        let latency = fft_size;

        // Hann window
        let window: Vec<f32> = (0..fft_size)
            .map(|i| 0.5 * (1.0 - (TAU * i as f32 / fft_size as f32).cos()))
            .collect();

        Self {
            config: cfg,
            pitch_ratio,
            analysis_buf: vec![0.0; fft_size],
            write_pos: 0,
            samples_since_last_fft: 0,
            prev_analysis_phase: vec![0.0; num_bins],
            synth_phase: vec![0.0; num_bins],
            ola_buf: vec![0.0; fft_size],
            read_pos: 0,
            window,
            wet,
            dry,
            latency,
        }
    }

    /// Return the pitch ratio (linear, not semitones).
    #[must_use]
    pub fn pitch_ratio(&self) -> f32 {
        self.pitch_ratio
    }

    /// Update the semitone shift at runtime.
    pub fn set_semitones(&mut self, semitones: f32) {
        self.config.semitones = semitones.clamp(-24.0, 24.0);
        self.pitch_ratio = 2.0_f32.powf(self.config.semitones / 12.0);
    }

    /// Run one FFT analysis/synthesis frame and overlap-add into `ola_buf`.
    fn process_frame(&mut self) {
        let fft_size = self.config.fft_size;
        let hop_size = self.config.hop_size;
        let num_bins = fft_size / 2 + 1;

        // Build windowed analysis frame from the circular buffer.
        let mut frame: Vec<Complex<f32>> = (0..fft_size)
            .map(|i| {
                let buf_idx = (self.write_pos + i) % fft_size;
                let windowed = self.analysis_buf[buf_idx] * self.window[i];
                Complex::new(windowed, 0.0)
            })
            .collect();

        // Forward FFT.
        let spectrum = oxifft::fft(&frame);

        // Phase vocoder: compute instantaneous frequency and accumulate synth phase.
        let mut synth_spectrum: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); fft_size];

        let hop_f = hop_size as f32;
        let fft_f = fft_size as f32;

        for bin in 0..num_bins {
            let mag = spectrum[bin].norm();
            let phase = spectrum[bin].arg();

            // Expected phase advance from last hop (bin's ideal frequency).
            let expected_advance = TAU * bin as f32 * hop_f / fft_f;
            // True phase deviation from expected.
            let delta = (phase - self.prev_analysis_phase[bin] - expected_advance)
                .rem_euclid(TAU)
                - std::f32::consts::PI;
            // Instantaneous frequency (bins).
            let inst_freq = bin as f32 + delta * fft_f / (TAU * hop_f);

            self.prev_analysis_phase[bin] = phase;

            // Advance synthesis phase by ratio.
            self.synth_phase[bin] += TAU * inst_freq * self.pitch_ratio * hop_f / fft_f;

            let s_phase = self.synth_phase[bin];
            synth_spectrum[bin] = Complex::new(mag * s_phase.cos(), mag * s_phase.sin());

            // Mirror for negative frequencies (conjugate symmetry).
            if bin > 0 && bin < fft_size - num_bins + 1 {
                let mirror = fft_size - bin;
                synth_spectrum[mirror] = Complex::new(mag * s_phase.cos(), -mag * s_phase.sin());
            }
        }

        // Inverse FFT.
        let time_domain = oxifft::ifft(&synth_spectrum);

        // Overlap-add with Hann window, normalised by OLA gain.
        let ola_norm = fft_f / (hop_f * 2.0);
        for (i, sample) in time_domain.iter().enumerate() {
            let windowed = sample.re * self.window[i] / ola_norm.max(1.0);
            let ola_idx = (self.read_pos + i) % fft_size;
            self.ola_buf[ola_idx] += windowed;
        }

        // Suppress unused assignment warning on the local frame variable
        let _ = frame.last_mut();
    }

    /// Clear all internal buffers and reset phase state.
    pub fn clear(&mut self) {
        self.analysis_buf.iter_mut().for_each(|s| *s = 0.0);
        self.ola_buf.iter_mut().for_each(|s| *s = 0.0);
        self.prev_analysis_phase.iter_mut().for_each(|p| *p = 0.0);
        self.synth_phase.iter_mut().for_each(|p| *p = 0.0);
        self.write_pos = 0;
        self.read_pos = 0;
        self.samples_since_last_fft = 0;
    }
}

impl AudioEffect for BlockFftShifter {

    const EFFECT_ID: u64 = 6022;
    fn process_sample(&mut self, input: f32) -> f32 {
        let fft_size = self.config.fft_size;
        let hop_size = self.config.hop_size;

        // Write new sample into analysis buffer.
        self.analysis_buf[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) % fft_size;
        self.samples_since_last_fft += 1;

        // When a full hop has accumulated, process a new FFT frame.
        if self.samples_since_last_fft >= hop_size {
            self.samples_since_last_fft = 0;
            self.process_frame();
        }

        // Read one sample from the OLA output buffer.
        let wet_out = self.ola_buf[self.read_pos];
        self.ola_buf[self.read_pos] = 0.0; // clear after reading
        self.read_pos = (self.read_pos + 1) % fft_size;

        wet_out * self.wet + input * self.dry
    }

    fn reset(&mut self) {
        self.clear();
    }

    fn latency_samples(&self) -> usize {
        self.latency
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.wet = wet.clamp(0.0, 1.0);
        self.dry = 1.0 - self.wet;
        self.config.wet_mix = self.wet;
    }

    fn wet_dry(&self) -> f32 {
        self.wet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const SR: f32 = 48_000.0;

    fn make_shifter(semitones: f32) -> BlockFftShifter {
        BlockFftShifter::new(
            BlockFftConfig {
                semitones,
                fft_size: 1024,
                hop_size: 256,
                wet_mix: 1.0,
            },
            SR,
        )
    }

    fn make_sine(freq: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (TAU * freq * i as f32 / SR).sin() * 0.5)
            .collect()
    }

    // ── construction ─────────────────────────────────────────────────────────

    #[test]
    fn test_default_config_sanitises() {
        let cfg = BlockFftConfig::default().sanitise();
        assert!(cfg.fft_size.is_power_of_two());
        assert!(cfg.hop_size < cfg.fft_size);
        assert!((cfg.wet_mix - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_pitch_ratio_unison() {
        let s = make_shifter(0.0);
        assert!((s.pitch_ratio() - 1.0).abs() < 1e-5, "0 semitones → ratio 1");
    }

    #[test]
    fn test_pitch_ratio_octave_up() {
        let s = make_shifter(12.0);
        assert!(
            (s.pitch_ratio() - 2.0).abs() < 1e-4,
            "12 semitones → ratio 2"
        );
    }

    #[test]
    fn test_pitch_ratio_octave_down() {
        let s = make_shifter(-12.0);
        assert!(
            (s.pitch_ratio() - 0.5).abs() < 1e-4,
            "-12 semitones → ratio 0.5"
        );
    }

    // ── output sanity ─────────────────────────────────────────────────────────

    #[test]
    fn test_output_is_finite() {
        let mut s = make_shifter(7.0);
        let input = make_sine(440.0, 8192);
        for &sample in &input {
            let out = s.process_sample(sample);
            assert!(out.is_finite(), "non-finite output: {out}");
        }
    }

    #[test]
    fn test_silence_stays_silent() {
        let mut s = make_shifter(5.0);
        for _ in 0..4096 {
            let out = s.process_sample(0.0);
            assert!(
                out.abs() < 1e-4,
                "silence input should give near-silence output: {out}"
            );
        }
    }

    #[test]
    fn test_latency_equals_fft_size() {
        let s = make_shifter(0.0);
        assert_eq!(
            s.latency_samples(),
            1024,
            "latency should equal fft_size"
        );
    }

    // ── wet/dry ──────────────────────────────────────────────────────────────

    #[test]
    fn test_wet_zero_passes_dry() {
        let mut s = BlockFftShifter::new(
            BlockFftConfig {
                semitones: 12.0,
                fft_size: 512,
                hop_size: 128,
                wet_mix: 0.0,
            },
            SR,
        );
        // With wet=0 the output should exactly equal the input (dry pass-through).
        let input = 0.3_f32;
        let out = s.process_sample(input);
        assert!(
            (out - input).abs() < 1e-5,
            "wet=0: out={out} expected {input}"
        );
    }

    #[test]
    fn test_set_wet_dry_updates() {
        let mut s = make_shifter(5.0);
        s.set_wet_dry(0.4);
        assert!((s.wet_dry() - 0.4).abs() < 1e-5);
    }

    #[test]
    fn test_set_wet_dry_clamps() {
        let mut s = make_shifter(0.0);
        s.set_wet_dry(2.0);
        assert!((s.wet_dry() - 1.0).abs() < f32::EPSILON);
        s.set_wet_dry(-1.0);
        assert!((s.wet_dry() - 0.0).abs() < f32::EPSILON);
    }

    // ── reset ────────────────────────────────────────────────────────────────

    #[test]
    fn test_reset_clears_buffers() {
        let mut s = make_shifter(3.0);
        let input = make_sine(440.0, 4096);
        for &sample in &input {
            s.process_sample(sample);
        }
        s.reset();
        for _ in 0..512 {
            let out = s.process_sample(0.0);
            assert!(
                out.abs() < 1e-3,
                "after reset silence should yield near-silence: {out}"
            );
        }
    }

    // ── set_semitones ─────────────────────────────────────────────────────────

    #[test]
    fn test_set_semitones_runtime() {
        let mut s = make_shifter(0.0);
        s.set_semitones(7.0);
        let expected = 2.0_f32.powf(7.0 / 12.0);
        assert!(
            (s.pitch_ratio() - expected).abs() < 1e-4,
            "runtime semitone change failed"
        );
    }

    #[test]
    fn test_set_semitones_clamped() {
        let mut s = make_shifter(0.0);
        s.set_semitones(100.0);
        let expected = 2.0_f32.powf(24.0 / 12.0);
        assert!(
            (s.pitch_ratio() - expected).abs() < 1e-4,
            "semitones should clamp at +24"
        );
    }
}
