//! Analog delay emulation with feedback saturation modeling.
//!
//! Simulates the warm, non-linear character of analog tape-echo machines
//! (e.g., Echoplex EP-3, Roland RE-201) and bucket-brigade device (BBD)
//! delays by inserting a saturation stage in the feedback path.
//!
//! # Signal Flow
//!
//! ```text
//! input ──┬────────────────────────────────────► + ──► output
//!         │                                      │
//!         │   ┌─────────────────────────────┐    │
//!         │   │ delay buffer (ring)          │    │
//!         └──►│ ──► tape/BBD filter ──► sat ├────┘
//!             └─────────────────────────────┘
//!                        feedback ──►
//! ```
//!
//! ## Saturation Modes
//!
//! | Mode | Model | Character |
//! |------|-------|-----------|
//! | `Tape` | Hyperbolic tangent soft-clip | Warm, musical |
//! | `Bbd` | 3rd-order polynomial clip | Brighter, slightly gritty |
//! | `Diode` | Asymmetric rectifier approximation | Vintage, uneven harmonics |
//! | `Hard` | Simple hard clip at ±threshold | Aggressive, brickwall |
//!
//! ## High-Frequency Roll-Off
//!
//! Real analog delay lines degrade treble on each pass through the medium.
//! A first-order IIR low-pass filter in the feedback path models this,
//! with configurable cutoff frequency.
//!
//! # Example
//!
//! ```
//! use oximedia_effects::analog_delay::{AnalogDelay, AnalogDelayConfig, SaturationMode};
//! use oximedia_effects::AudioEffect;
//!
//! let config = AnalogDelayConfig {
//!     delay_ms: 300.0,
//!     feedback: 0.5,
//!     wet_mix: 0.4,
//!     sample_rate: 48_000.0,
//!     saturation_mode: SaturationMode::Tape,
//!     drive: 2.0,
//!     tone_cutoff_hz: 4_000.0,
//! };
//! let mut delay = AnalogDelay::new(config);
//! let out = delay.process_sample(1.0);
//! assert!(out.is_finite());
//! ```

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::AudioEffect;

// ---------------------------------------------------------------------------
// Saturation mode
// ---------------------------------------------------------------------------

/// Saturation algorithm applied in the feedback path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaturationMode {
    /// Hyperbolic tangent soft-clip — warm, symmetric, musical.
    Tape,
    /// 3rd-order polynomial approximation — bright BBD character.
    Bbd,
    /// Asymmetric diode approximation — vintage transistor radio character.
    Diode,
    /// Hard brick-wall clip at ±1.0 — aggressive, transparent to low signals.
    Hard,
}

impl Default for SaturationMode {
    fn default() -> Self {
        Self::Tape
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for [`AnalogDelay`].
#[derive(Debug, Clone)]
pub struct AnalogDelayConfig {
    /// Delay time in milliseconds. Range: `[1, 5000]`.
    pub delay_ms: f32,
    /// Feedback level in `[0.0, 0.98]`. Values above ~0.9 approach self-oscillation.
    pub feedback: f32,
    /// Wet/dry ratio in `[0.0, 1.0]`. `1.0` = fully wet.
    pub wet_mix: f32,
    /// Sample rate in Hz.
    pub sample_rate: f32,
    /// Saturation character in the feedback path.
    pub saturation_mode: SaturationMode,
    /// Input drive to the saturation stage in `[0.1, 20.0]`.
    /// Higher values = more harmonic distortion.
    pub drive: f32,
    /// First-order low-pass cutoff in the feedback path (Hz).
    /// Models tape/BBD high-frequency roll-off. Set to `sample_rate / 2` to disable.
    pub tone_cutoff_hz: f32,
}

impl Default for AnalogDelayConfig {
    fn default() -> Self {
        Self {
            delay_ms: 350.0,
            feedback: 0.45,
            wet_mix: 0.35,
            sample_rate: 48_000.0,
            saturation_mode: SaturationMode::Tape,
            drive: 1.5,
            tone_cutoff_hz: 5_000.0,
        }
    }
}

// ---------------------------------------------------------------------------
// First-order IIR tone filter
// ---------------------------------------------------------------------------

/// Simple one-pole low-pass filter for feedback tone shaping.
#[derive(Debug, Clone)]
struct ToneFilter {
    /// IIR coefficient (feedback coefficient `b1` in direct form I).
    coeff: f32,
    /// One sample of state.
    z1: f32,
}

impl ToneFilter {
    fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        // Bilinear 1-pole LP: coeff = 1 - 2π*fc/fs (approximate; clamped)
        let coeff = compute_lp_coeff(cutoff_hz, sample_rate);
        Self { coeff, z1: 0.0 }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.z1 = x * (1.0 - self.coeff) + self.z1 * self.coeff;
        self.z1
    }

    fn reset(&mut self) {
        self.z1 = 0.0;
    }

    fn set_cutoff(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.coeff = compute_lp_coeff(cutoff_hz, sample_rate);
    }
}

fn compute_lp_coeff(cutoff_hz: f32, sample_rate: f32) -> f32 {
    let fc = cutoff_hz.clamp(20.0, sample_rate * 0.499);
    let rc = 1.0 / (2.0 * std::f32::consts::PI * fc);
    let dt = 1.0 / sample_rate;
    rc / (rc + dt)
}

// ---------------------------------------------------------------------------
// Ring buffer
// ---------------------------------------------------------------------------

/// Pre-allocated ring buffer for delay.
#[derive(Debug, Clone)]
struct RingBuf {
    buf: Vec<f32>,
    write_pos: usize,
    capacity: usize,
}

impl RingBuf {
    fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity],
            write_pos: 0,
            capacity,
        }
    }

    fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
    }
}

// ---------------------------------------------------------------------------
// Saturation functions
// ---------------------------------------------------------------------------

/// Apply the chosen saturation algorithm to `x` with drive `d`.
#[inline]
fn saturate(x: f32, mode: SaturationMode, drive: f32) -> f32 {
    let driven = x * drive;
    match mode {
        SaturationMode::Tape => driven.tanh(),
        SaturationMode::Bbd => {
            // 3rd-order Chebyshev-style soft clip
            let v = driven.clamp(-1.5, 1.5);
            v - (v * v * v) / 3.0
        }
        SaturationMode::Diode => {
            // Asymmetric rectifier: harder on positive half, softer on negative
            if driven >= 0.0 {
                1.0 - (-driven).exp()
            } else {
                -(1.0 - driven.exp()) * 0.7
            }
        }
        SaturationMode::Hard => driven.clamp(-1.0, 1.0),
    }
}

// ---------------------------------------------------------------------------
// AnalogDelay
// ---------------------------------------------------------------------------

/// Analog delay with saturation modeling in the feedback path.
///
/// Implements the [`AudioEffect`] trait for both mono and stereo processing.
/// Stereo processing routes each channel through an independent delay line
/// while sharing parameter state.
#[derive(Debug, Clone)]
pub struct AnalogDelay {
    config: AnalogDelayConfig,
    /// Delay ring buffer — left channel (or mono).
    ring_l: RingBuf,
    /// Delay ring buffer — right channel.
    ring_r: RingBuf,
    /// Tone/treble-roll-off filter in feedback — left.
    tone_l: ToneFilter,
    /// Tone/treble-roll-off filter in feedback — right.
    tone_r: ToneFilter,
    /// Delay length in samples (integer, derived from `delay_ms`).
    delay_samples: usize,
}

impl AnalogDelay {
    /// Create a new [`AnalogDelay`] from configuration.
    #[must_use]
    pub fn new(config: AnalogDelayConfig) -> Self {
        let fs = config.sample_rate;
        let max_delay_samples = (fs * 5.0) as usize + 2; // 5 s maximum
        let delay_samples = ms_to_samples(config.delay_ms, fs);

        Self {
            tone_l: ToneFilter::new(config.tone_cutoff_hz, fs),
            tone_r: ToneFilter::new(config.tone_cutoff_hz, fs),
            ring_l: RingBuf::new(max_delay_samples),
            ring_r: RingBuf::new(max_delay_samples),
            delay_samples,
            config,
        }
    }

    // -----------------------------------------------------------------------
    // Parameter setters (real-time safe)
    // -----------------------------------------------------------------------

    /// Set the delay time in milliseconds (updates `delay_samples`).
    pub fn set_delay_ms(&mut self, delay_ms: f32) {
        self.config.delay_ms = delay_ms.clamp(1.0, 5000.0);
        self.delay_samples = ms_to_samples(self.config.delay_ms, self.config.sample_rate);
    }

    /// Set the feedback level in `[0.0, 0.98]`.
    pub fn set_feedback(&mut self, feedback: f32) {
        self.config.feedback = feedback.clamp(0.0, 0.98);
    }

    /// Set wet/dry mix.
    pub fn set_wet_mix(&mut self, wet: f32) {
        self.config.wet_mix = wet.clamp(0.0, 1.0);
    }

    /// Set the drive level for the saturation stage.
    pub fn set_drive(&mut self, drive: f32) {
        self.config.drive = drive.clamp(0.1, 20.0);
    }

    /// Set the tone filter cutoff frequency.
    pub fn set_tone_cutoff(&mut self, hz: f32) {
        self.config.tone_cutoff_hz = hz;
        let fs = self.config.sample_rate;
        self.tone_l.set_cutoff(hz, fs);
        self.tone_r.set_cutoff(hz, fs);
    }

    /// Set the saturation mode.
    pub fn set_saturation_mode(&mut self, mode: SaturationMode) {
        self.config.saturation_mode = mode;
    }

    /// Access the current configuration.
    #[must_use]
    pub fn config(&self) -> &AnalogDelayConfig {
        &self.config
    }
}

impl AudioEffect for AnalogDelay {
    const EFFECT_ID: u64 = 6000;

    fn process_sample(&mut self, input: f32) -> f32 {
        // We need to split borrows manually
        let delay = self.delay_samples.min(self.ring_l.capacity - 1);
        let read_pos = if self.ring_l.write_pos >= delay {
            self.ring_l.write_pos - delay
        } else {
            self.ring_l.write_pos + self.ring_l.capacity - delay
        };
        let echo = self.ring_l.buf[read_pos];

        let toned = self.tone_l.process(echo);
        let drive = self.config.drive;
        let mode = self.config.saturation_mode;
        let fb = self.config.feedback.clamp(0.0, 0.98);
        let feedback_signal = saturate(toned, mode, drive) * fb;

        self.ring_l.buf[self.ring_l.write_pos] = input + feedback_signal;
        self.ring_l.write_pos = (self.ring_l.write_pos + 1) % self.ring_l.capacity;

        let wet = self.config.wet_mix;
        let dry = 1.0 - wet;
        input * dry + echo * wet
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        // Left channel
        let delay = self.delay_samples.min(self.ring_l.capacity - 1);
        let read_l = if self.ring_l.write_pos >= delay {
            self.ring_l.write_pos - delay
        } else {
            self.ring_l.write_pos + self.ring_l.capacity - delay
        };
        let echo_l = self.ring_l.buf[read_l];
        let toned_l = self.tone_l.process(echo_l);
        let drive = self.config.drive;
        let mode = self.config.saturation_mode;
        let fb = self.config.feedback.clamp(0.0, 0.98);
        let fb_l = saturate(toned_l, mode, drive) * fb;
        self.ring_l.buf[self.ring_l.write_pos] = left + fb_l;
        self.ring_l.write_pos = (self.ring_l.write_pos + 1) % self.ring_l.capacity;

        // Right channel
        let read_r = if self.ring_r.write_pos >= delay {
            self.ring_r.write_pos - delay
        } else {
            self.ring_r.write_pos + self.ring_r.capacity - delay
        };
        let echo_r = self.ring_r.buf[read_r];
        let toned_r = self.tone_r.process(echo_r);
        let fb_r = saturate(toned_r, mode, drive) * fb;
        self.ring_r.buf[self.ring_r.write_pos] = right + fb_r;
        self.ring_r.write_pos = (self.ring_r.write_pos + 1) % self.ring_r.capacity;

        let wet = self.config.wet_mix;
        let dry = 1.0 - wet;
        (left * dry + echo_l * wet, right * dry + echo_r * wet)
    }

    fn reset(&mut self) {
        self.ring_l.reset();
        self.ring_r.reset();
        self.tone_l.reset();
        self.tone_r.reset();
    }

    fn latency_samples(&self) -> usize {
        self.delay_samples
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.config.sample_rate = sample_rate;
        self.delay_samples = ms_to_samples(self.config.delay_ms, sample_rate);
        self.tone_l
            .set_cutoff(self.config.tone_cutoff_hz, sample_rate);
        self.tone_r
            .set_cutoff(self.config.tone_cutoff_hz, sample_rate);
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.config.wet_mix = wet.clamp(0.0, 1.0);
    }

    fn wet_dry(&self) -> f32 {
        self.config.wet_mix
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ms_to_samples(delay_ms: f32, sample_rate: f32) -> usize {
    ((delay_ms * sample_rate / 1000.0) as usize).max(1)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioEffect;

    fn make_delay() -> AnalogDelay {
        AnalogDelay::new(AnalogDelayConfig::default())
    }

    #[test]
    fn test_dry_pass_through_immediate() {
        // With wet=0, output should be the dry signal.
        let mut d = AnalogDelay::new(AnalogDelayConfig {
            wet_mix: 0.0,
            delay_ms: 100.0,
            feedback: 0.0,
            ..AnalogDelayConfig::default()
        });
        let out = d.process_sample(0.7);
        assert!((out - 0.7).abs() < 1e-5, "dry pass-through: {out}");
    }

    #[test]
    fn test_silence_produces_silence_initially() {
        let mut d = make_delay();
        let out = d.process_sample(0.0);
        assert!(
            out.abs() < 1e-5,
            "silence in → silence out initially: {out}"
        );
    }

    #[test]
    fn test_echo_appears_after_delay() {
        let sample_rate = 48_000.0;
        let delay_ms = 10.0;
        let delay_samples = (delay_ms * sample_rate / 1000.0) as usize;
        let mut d = AnalogDelay::new(AnalogDelayConfig {
            delay_ms,
            feedback: 0.0,
            wet_mix: 1.0, // fully wet
            sample_rate,
            saturation_mode: SaturationMode::Hard,
            drive: 1.0,
            tone_cutoff_hz: sample_rate / 2.0,
        });

        // Feed an impulse at sample 0
        let out0 = d.process_sample(1.0);
        // At fully wet with delay_samples > 0, initial output is 0 (buffer empty)
        assert!(out0.abs() < 1e-5, "first sample: {out0}");

        // Advance through the delay
        for _ in 1..delay_samples {
            d.process_sample(0.0);
        }
        // The (delay_samples+1)th sample should carry the echo
        let echo = d.process_sample(0.0);
        assert!(echo.abs() > 0.1, "echo should appear after delay: {echo}");
    }

    #[test]
    fn test_feedback_accumulates() {
        let mut d = AnalogDelay::new(AnalogDelayConfig {
            delay_ms: 5.0,
            feedback: 0.7,
            wet_mix: 0.5,
            sample_rate: 48_000.0,
            saturation_mode: SaturationMode::Tape,
            drive: 1.0,
            tone_cutoff_hz: 20_000.0,
        });
        // After many passes of an impulse with non-zero feedback, the output
        // should stay non-zero (feedback tail is present).
        d.process_sample(1.0);
        let delay_samps = ms_to_samples(5.0, 48_000.0);
        for _ in 0..delay_samps {
            d.process_sample(0.0);
        }
        // Multiple echoes with feedback
        let mut non_zero = false;
        for _ in 0..(delay_samps * 4) {
            let out = d.process_sample(0.0);
            if out.abs() > 1e-4 {
                non_zero = true;
            }
        }
        assert!(non_zero, "feedback should produce multiple echoes");
    }

    #[test]
    fn test_tape_saturation_clips_high_drive() {
        // High drive → saturated output should be bounded by tanh range (-1, 1)
        let val = saturate(10.0, SaturationMode::Tape, 100.0);
        assert!(val.abs() <= 1.001, "tanh should be bounded: {val}");
    }

    #[test]
    fn test_hard_saturation_clips() {
        let val = saturate(5.0, SaturationMode::Hard, 10.0);
        assert!((val - 1.0).abs() < 1e-5, "hard clip: {val}");
        let val_neg = saturate(-5.0, SaturationMode::Hard, 10.0);
        assert!((val_neg + 1.0).abs() < 1e-5, "hard clip neg: {val_neg}");
    }

    #[test]
    fn test_bbd_saturation_bounded() {
        // 3rd-order polynomial has finite range
        let val = saturate(2.0, SaturationMode::Bbd, 1.0);
        assert!(val.is_finite(), "BBD saturation finite: {val}");
    }

    #[test]
    fn test_diode_saturation_asymmetric() {
        let pos = saturate(1.0, SaturationMode::Diode, 1.0);
        let neg = saturate(-1.0, SaturationMode::Diode, 1.0);
        // Asymmetric: positive and negative sides should have different magnitudes
        assert!(
            (pos.abs() - neg.abs()).abs() > 0.05,
            "diode should be asymmetric: pos={pos} neg={neg}"
        );
    }

    #[test]
    fn test_stereo_processing_independent_channels() {
        let mut d = make_delay();
        let (l, r) = d.process_sample_stereo(1.0, -1.0);
        assert!(l.is_finite() && r.is_finite());
    }

    #[test]
    fn test_reset_clears_echo() {
        let mut d = AnalogDelay::new(AnalogDelayConfig {
            delay_ms: 5.0,
            feedback: 0.0,
            wet_mix: 1.0,
            sample_rate: 48_000.0,
            saturation_mode: SaturationMode::Tape,
            drive: 1.0,
            tone_cutoff_hz: 20_000.0,
        });
        d.process_sample(1.0);
        d.reset();
        let out = d.process_sample(0.0);
        assert!(
            out.abs() < 1e-5,
            "after reset, output should be zero: {out}"
        );
    }

    #[test]
    fn test_set_delay_ms_updates_samples() {
        let mut d = make_delay();
        d.set_delay_ms(100.0);
        let expected = ms_to_samples(100.0, 48_000.0);
        assert_eq!(d.delay_samples, expected);
    }

    #[test]
    fn test_wet_dry_trait_methods() {
        let mut d = make_delay();
        d.set_wet_dry(0.6);
        assert!((d.wet_dry() - 0.6).abs() < 1e-5);
    }

    #[test]
    fn test_latency_reports_delay_samples() {
        let d = AnalogDelay::new(AnalogDelayConfig {
            delay_ms: 10.0,
            sample_rate: 48_000.0,
            ..AnalogDelayConfig::default()
        });
        let expected = ms_to_samples(10.0, 48_000.0);
        assert_eq!(d.latency_samples(), expected);
    }

    #[test]
    fn test_all_saturation_modes_produce_finite_output() {
        let modes = [
            SaturationMode::Tape,
            SaturationMode::Bbd,
            SaturationMode::Diode,
            SaturationMode::Hard,
        ];
        for mode in modes {
            let mut d = AnalogDelay::new(AnalogDelayConfig {
                saturation_mode: mode,
                drive: 3.0,
                ..AnalogDelayConfig::default()
            });
            for i in 0..500 {
                let inp = if i == 0 { 0.8 } else { 0.0 };
                let out = d.process_sample(inp);
                assert!(out.is_finite(), "mode {mode:?} produced non-finite: {out}");
            }
        }
    }
}
