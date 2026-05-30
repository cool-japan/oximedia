//! Basic delay effect with feedback and filtering.
//!
//! Includes analog-style feedback saturation modeling for warm, characterful
//! repeats. The saturation stage is applied to the feedback path only, so the
//! dry signal remains uncolored.
//!
//! ## Saturation modes
//!
//! | Mode | Character |
//! |------|-----------|
//! | `None` | Clean digital delay (no coloring) |
//! | `Tape` | Soft asymmetric tanh saturation (tape head squash) |
//! | `Tube` | Asymmetric triode-style warmth (even harmonics) |
//! | `Diode` | Hard-knee diode clipping (bright edge) |

use crate::{
    utils::{DelayLine, ParameterSmoother},
    AudioEffect,
};

// ── Saturation helpers ────────────────────────────────────────────────────────

/// Feedback saturation mode for analog delay emulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackSaturationMode {
    /// No saturation (clean digital delay).
    None,
    /// Tape-style soft saturation using hyperbolic tangent shaping.
    Tape,
    /// Tube/triode-style even-harmonic saturation.
    Tube,
    /// Diode-clipping hard-knee distortion for aggressive character.
    Diode,
}

impl Default for FeedbackSaturationMode {
    fn default() -> Self {
        Self::None
    }
}

impl FeedbackSaturationMode {
    /// Apply the saturation nonlinearity to the feedback signal.
    ///
    /// `drive` controls the input level to the nonlinearity (1.0 = unity).
    /// Output is post-normalized to stay roughly within `[-1, 1]`.
    #[inline]
    pub fn apply(self, x: f32, drive: f32) -> f32 {
        match self {
            Self::None => x,
            Self::Tape => {
                // Soft tanh saturation — classic tape head squash.
                let driven = x * drive;
                driven.tanh()
            }
            Self::Tube => {
                // Asymmetric triode-style: boost positive half slightly.
                let driven = x * drive;
                let pos = (driven + 0.1).tanh();
                let neg = (driven - 0.1).tanh();
                // Even-harmonic mix: slightly more positive than negative.
                0.5 * (pos + neg) + 0.05 * (pos - neg)
            }
            Self::Diode => {
                // Hard-knee diode clipping: linear until knee, then hard clip.
                let driven = x * drive;
                let knee = 0.7_f32;
                if driven.abs() <= knee {
                    driven
                } else {
                    driven.signum() * (knee + (driven.abs() - knee).tanh() * (1.0 - knee))
                }
            }
        }
    }
}

// ── DelayConfig ───────────────────────────────────────────────────────────────

/// Configuration for delay effect.
#[derive(Debug, Clone)]
pub struct DelayConfig {
    /// Delay time in milliseconds.
    pub delay_ms: f32,
    /// Feedback amount (0.0 - 1.0).
    pub feedback: f32,
    /// Wet signal level (0.0 - 1.0).
    pub wet: f32,
    /// Dry signal level (0.0 - 1.0).
    pub dry: f32,
    /// Low-pass filter cutoff for feedback (0.0 = no filtering, 1.0 = maximum filtering).
    pub tone: f32,
    /// Feedback saturation mode (default: `None` = clean digital).
    pub saturation: FeedbackSaturationMode,
    /// Saturation drive level (1.0 = unity, higher = more harmonic content).
    pub saturation_drive: f32,
}

impl Default for DelayConfig {
    fn default() -> Self {
        Self {
            delay_ms: 500.0,
            feedback: 0.4,
            wet: 0.5,
            dry: 0.5,
            tone: 0.0,
            saturation: FeedbackSaturationMode::None,
            saturation_drive: 1.0,
        }
    }
}

impl DelayConfig {
    /// Create a new delay configuration.
    #[must_use]
    pub fn new(delay_ms: f32, feedback: f32, wet: f32) -> Self {
        Self {
            delay_ms: delay_ms.max(0.0),
            feedback: feedback.clamp(0.0, 0.99),
            wet: wet.clamp(0.0, 1.0),
            dry: (1.0 - wet).clamp(0.0, 1.0),
            tone: 0.0,
            saturation: FeedbackSaturationMode::None,
            saturation_drive: 1.0,
        }
    }

    /// Slapback delay preset (short, single echo).
    #[must_use]
    pub fn slapback() -> Self {
        Self::new(100.0, 0.0, 0.3)
    }

    /// Dotted eighth note delay preset (at 120 BPM).
    #[must_use]
    pub fn dotted_eighth() -> Self {
        Self::new(375.0, 0.35, 0.4)
    }

    /// Long ambient delay preset.
    #[must_use]
    pub fn ambient() -> Self {
        Self::new(750.0, 0.6, 0.5).with_tone(0.4)
    }

    /// Analog tape delay preset with tape saturation.
    #[must_use]
    pub fn tape() -> Self {
        Self {
            saturation: FeedbackSaturationMode::Tape,
            saturation_drive: 1.5,
            ..Self::new(380.0, 0.5, 0.45).with_tone(0.3)
        }
    }

    /// Vintage tube delay with triode character.
    #[must_use]
    pub fn tube() -> Self {
        Self {
            saturation: FeedbackSaturationMode::Tube,
            saturation_drive: 1.2,
            ..Self::new(420.0, 0.4, 0.4).with_tone(0.2)
        }
    }

    /// Set tone control.
    #[must_use]
    pub fn with_tone(mut self, tone: f32) -> Self {
        self.tone = tone.clamp(0.0, 1.0);
        self
    }

    /// Set saturation mode.
    #[must_use]
    pub fn with_saturation(mut self, mode: FeedbackSaturationMode, drive: f32) -> Self {
        self.saturation = mode;
        self.saturation_drive = drive.clamp(0.1, 10.0);
        self
    }
}

// ── MonoDelay ─────────────────────────────────────────────────────────────────

/// Simple mono delay effect.
///
/// The delay line is a pre-allocated circular buffer sized to accommodate
/// up to 2 seconds at the given sample rate — no allocations occur during
/// audio processing.
///
/// The feedback path optionally passes through a saturation stage before
/// being written back into the delay line, modeling the nonlinear character
/// of analog delay hardware (tape machines, bucket-brigade devices, tube circuits).
pub struct MonoDelay {
    /// Pre-allocated circular buffer (ring buffer).
    delay_line: DelayLine,
    delay_samples: usize,
    config: DelayConfig,
    /// One-pole low-pass state for tone/brightness control in the feedback path.
    tone_filter: f32,
    tone_smoother: ParameterSmoother,
    sample_rate: f32,
}

impl MonoDelay {
    /// Create a new mono delay.
    ///
    /// Allocates a circular ring buffer large enough for a 2-second delay at
    /// the given sample rate. No further allocations happen during processing.
    #[must_use]
    pub fn new(config: DelayConfig, sample_rate: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_delay_samples = ((2000.0 * sample_rate) / 1000.0) as usize; // 2 second max

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let delay_samples = ((config.delay_ms * sample_rate) / 1000.0) as usize;

        Self {
            delay_line: DelayLine::new(max_delay_samples),
            delay_samples,
            config,
            tone_filter: 0.0,
            tone_smoother: ParameterSmoother::new(10.0, sample_rate),
            sample_rate,
        }
    }

    /// Set delay time in milliseconds.
    pub fn set_delay_ms(&mut self, delay_ms: f32) {
        self.config.delay_ms = delay_ms.max(0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let delay_samp = ((delay_ms * self.sample_rate) / 1000.0) as usize;
        self.delay_samples = delay_samp.min(self.delay_line.max_delay());
    }

    /// Set feedback amount.
    pub fn set_feedback(&mut self, feedback: f32) {
        self.config.feedback = feedback.clamp(0.0, 0.99);
    }

    /// Set wet level.
    pub fn set_wet(&mut self, wet: f32) {
        self.config.wet = wet.clamp(0.0, 1.0);
    }

    /// Set dry level.
    pub fn set_dry(&mut self, dry: f32) {
        self.config.dry = dry.clamp(0.0, 1.0);
    }

    /// Set tone (low-pass filter for feedback).
    pub fn set_tone(&mut self, tone: f32) {
        self.config.tone = tone.clamp(0.0, 1.0);
        self.tone_smoother.set_target(self.config.tone);
    }

    /// Set the feedback saturation mode.
    pub fn set_saturation(&mut self, mode: FeedbackSaturationMode, drive: f32) {
        self.config.saturation = mode;
        self.config.saturation_drive = drive.clamp(0.1, 10.0);
    }
}

impl AudioEffect for MonoDelay {
    const EFFECT_ID: u64 = 6005;

    fn process_sample(&mut self, input: f32) -> f32 {
        // Read delayed sample.
        let delayed = self.delay_line.read(self.delay_samples);

        // Apply one-pole low-pass tone control to the feedback path.
        let tone = self.tone_smoother.next();
        self.tone_filter = delayed * (1.0 - tone) + self.tone_filter * tone;

        // Apply feedback saturation nonlinearity before writing back.
        let saturated = self
            .config
            .saturation
            .apply(self.tone_filter, self.config.saturation_drive);

        // Write input + saturated feedback to the delay line.
        let feedback_signal = saturated * self.config.feedback;
        self.delay_line.write(input + feedback_signal);

        // Mix wet and dry.
        delayed * self.config.wet + input * self.config.dry
    }

    fn reset(&mut self) {
        self.delay_line.clear();
        self.tone_filter = 0.0;
        self.tone_smoother.reset(0.0);
    }

    fn latency_samples(&self) -> usize {
        0 // Zero latency (delay is part of the effect)
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.config.wet = wet.clamp(0.0, 1.0);
        self.config.dry = 1.0 - self.config.wet;
    }

    fn wet_dry(&self) -> f32 {
        self.config.wet
    }
}

// ── StereoDelay ───────────────────────────────────────────────────────────────

/// Stereo delay effect.
pub struct StereoDelay {
    left: MonoDelay,
    right: MonoDelay,
    cross_feedback: f32,
}

impl StereoDelay {
    /// Create a new stereo delay.
    #[must_use]
    pub fn new(config: DelayConfig, sample_rate: f32) -> Self {
        Self {
            left: MonoDelay::new(config.clone(), sample_rate),
            right: MonoDelay::new(config, sample_rate),
            cross_feedback: 0.0,
        }
    }

    /// Create with different delay times for left and right.
    #[must_use]
    pub fn new_dual(left_config: DelayConfig, right_config: DelayConfig, sample_rate: f32) -> Self {
        Self {
            left: MonoDelay::new(left_config, sample_rate),
            right: MonoDelay::new(right_config, sample_rate),
            cross_feedback: 0.0,
        }
    }

    /// Set cross-feedback amount (feedback from left to right and vice versa).
    pub fn set_cross_feedback(&mut self, amount: f32) {
        self.cross_feedback = amount.clamp(0.0, 0.99);
    }

    /// Set delay time for both channels.
    pub fn set_delay_ms(&mut self, delay_ms: f32) {
        self.left.set_delay_ms(delay_ms);
        self.right.set_delay_ms(delay_ms);
    }

    /// Set delay time for left channel.
    pub fn set_left_delay_ms(&mut self, delay_ms: f32) {
        self.left.set_delay_ms(delay_ms);
    }

    /// Set delay time for right channel.
    pub fn set_right_delay_ms(&mut self, delay_ms: f32) {
        self.right.set_delay_ms(delay_ms);
    }

    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let out_l = self.left.process_sample(input_l);
        let out_r = self.right.process_sample(input_r);

        // Apply cross-feedback if enabled.
        if self.cross_feedback > 0.0 {
            let cross_l = out_r * self.cross_feedback;
            let cross_r = out_l * self.cross_feedback;
            (out_l + cross_l, out_r + cross_r)
        } else {
            (out_l, out_r)
        }
    }
}

impl AudioEffect for StereoDelay {
    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _right) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    fn reset(&mut self) {
        self.left.reset();
        self.right.reset();
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.left.set_wet_dry(wet);
        self.right.set_wet_dry(wet);
    }

    fn wet_dry(&self) -> f32 {
        self.left.wet_dry()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delay_config() {
        let config = DelayConfig::default();
        assert_eq!(config.delay_ms, 500.0);
        assert_eq!(config.feedback, 0.4);
    }

    #[test]
    fn test_delay_presets() {
        let slapback = DelayConfig::slapback();
        assert!(slapback.delay_ms < 200.0);

        let ambient = DelayConfig::ambient();
        assert!(ambient.delay_ms > 500.0);
    }

    #[test]
    fn test_tape_preset() {
        let tape = DelayConfig::tape();
        assert_eq!(tape.saturation, FeedbackSaturationMode::Tape);
        assert!(tape.saturation_drive > 1.0);
    }

    #[test]
    fn test_tube_preset() {
        let tube = DelayConfig::tube();
        assert_eq!(tube.saturation, FeedbackSaturationMode::Tube);
    }

    #[test]
    fn test_mono_delay_clean() {
        let config = DelayConfig::new(100.0, 0.5, 0.5);
        let mut delay = MonoDelay::new(config, 48000.0);

        // Process impulse
        let out1 = delay.process_sample(1.0);
        assert!((out1 - 0.5).abs() < 0.01); // Should be mostly dry initially

        // Process silence — should get delayed echo after 100 ms.
        for _ in 0..4799 {
            delay.process_sample(0.0);
        }

        let echo = delay.process_sample(0.0);
        assert!(echo.abs() > 0.1, "Should have echo: {echo}");
    }

    #[test]
    fn test_mono_delay_tape_saturation() {
        let config = DelayConfig::tape();
        let mut delay = MonoDelay::new(config, 48000.0);

        // Process hot signal through tape delay — all outputs must stay finite.
        for _ in 0..2000 {
            let out = delay.process_sample(0.9);
            assert!(out.is_finite(), "tape delay output must be finite");
        }
    }

    #[test]
    fn test_mono_delay_tube_saturation() {
        let config = DelayConfig::tube();
        let mut delay = MonoDelay::new(config, 48000.0);

        for _ in 0..2000 {
            let out = delay.process_sample(0.8);
            assert!(out.is_finite(), "tube delay output must be finite");
        }
    }

    #[test]
    fn test_mono_delay_diode_saturation() {
        let config =
            DelayConfig::new(200.0, 0.5, 0.5).with_saturation(FeedbackSaturationMode::Diode, 2.0);
        let mut delay = MonoDelay::new(config, 48000.0);

        for _ in 0..2000 {
            let out = delay.process_sample(0.7);
            assert!(out.is_finite(), "diode delay output must be finite");
        }
    }

    #[test]
    fn test_saturation_none_is_linear() {
        let x = 0.5_f32;
        assert!((FeedbackSaturationMode::None.apply(x, 1.0) - x).abs() < 1e-6);
    }

    #[test]
    fn test_saturation_tape_bounded() {
        for &x in &[-2.0_f32, -1.0, -0.5, 0.0, 0.5, 1.0, 2.0] {
            let out = FeedbackSaturationMode::Tape.apply(x, 1.5);
            assert!(
                out.abs() <= 1.0 + 1e-5,
                "tape output {out} exceeds unity for input {x}"
            );
        }
    }

    #[test]
    fn test_saturation_tube_finite() {
        for &x in &[-2.0_f32, -1.0, 0.0, 1.0, 2.0] {
            let out = FeedbackSaturationMode::Tube.apply(x, 1.2);
            assert!(out.is_finite(), "tube output must be finite for input {x}");
        }
    }

    #[test]
    fn test_saturation_diode_finite() {
        for &x in &[-2.0_f32, -1.0, -0.7, 0.0, 0.7, 1.0, 2.0] {
            let out = FeedbackSaturationMode::Diode.apply(x, 2.0);
            assert!(out.is_finite(), "diode output must be finite for input {x}");
        }
    }

    #[test]
    fn test_stereo_delay() {
        let config = DelayConfig::default();
        let mut delay = StereoDelay::new(config, 48000.0);

        let (out_l, out_r) = delay.process_sample_stereo(1.0, 0.5);
        assert!(out_l != out_r); // Different inputs should give different outputs
    }

    #[test]
    fn test_delay_reset() {
        let config = DelayConfig::new(100.0, 0.9, 0.5);
        let mut delay = MonoDelay::new(config, 48000.0);

        // Fill delay line
        for _ in 0..1000 {
            delay.process_sample(1.0);
        }

        delay.reset();

        // After reset, delay line should be clear
        let output = delay.process_sample(0.0);
        assert!(output.abs() < 0.01);
    }

    #[test]
    fn test_wet_dry_trait() {
        let config = DelayConfig::new(100.0, 0.5, 0.4);
        let mut delay = MonoDelay::new(config, 48000.0);

        // Initial wet level from config
        assert!((delay.wet_dry() - 0.4).abs() < 1e-5);

        // Update via trait method
        delay.set_wet_dry(0.7);
        assert!((delay.wet_dry() - 0.7).abs() < 1e-5);

        // Clamping
        delay.set_wet_dry(1.5);
        assert!((delay.wet_dry() - 1.0).abs() < 1e-5);
    }

    /// Verify that high feedback + high drive does not cause the signal to diverge.
    ///
    /// The tanh-based saturation in the feedback path bounds the **feedback**
    /// component to `|tanh(x)| ≤ 1`.  The delay effect output also includes the
    /// dry signal, so the total output can exceed 1.0 — but it must remain finite
    /// and bounded even under worst-case sustained input.
    ///
    /// With `wet = 0.5`, `dry = 0.5`, and `feedback = 0.95`:
    ///   - The written value = `input + tanh(delayed * drive) * feedback`
    ///   - `tanh` bounds the feedback loop, preventing runaway divergence
    ///   - At steady state the buffer settles at `tanh(x) * 0.95 + 1.0` which
    ///     is bounded (tanh(x) < 1), so output ≤ dry + wet * (1+1) = 1.5 + margin
    #[test]
    fn delay_feedback_saturation_clips() {
        let config = DelayConfig {
            delay_ms: 10.0,
            feedback: 0.95,
            wet: 0.5,
            dry: 0.5,
            saturation: FeedbackSaturationMode::Tape,
            saturation_drive: 4.0,
            tone: 0.0,
        };
        let mut delay = MonoDelay::new(config, 48000.0);

        for i in 0..10_000usize {
            let out = delay.process_sample(1.0);
            assert!(out.is_finite(), "signal diverged at sample {i}: {out}");
            // The saturation prevents runaway: max steady-state output ≈
            // dry*1.0 + wet*(tanh(buf)*feedback + ...) which is well below 4.0.
            assert!(
                out.abs() < 4.0,
                "signal exceeded reasonable bound at sample {i}: {out}"
            );
        }
    }

    /// Verify that `MonoDelay` with no saturation produces the expected impulse
    /// response: the first echo should appear at exactly `delay_ms` worth of
    /// samples and be scaled by `wet`.
    ///
    /// This acts as a regression test: if the underlying `DelayLine` semantics
    /// ever change (e.g. an off-by-one in read/write ordering), this test will
    /// catch it.
    #[test]
    fn delay_output_matches_reference_after_migration() {
        // Construct a 10-sample delay at 1000 Hz so delay_samples = 10.
        // dry = 1.0, wet = 1.0, feedback = 0.0 → clean, no echo loop.
        let config = DelayConfig {
            delay_ms: 10.0, // 10 ms at 1 000 Hz → 10 samples
            feedback: 0.0,
            wet: 1.0,
            dry: 1.0,
            saturation: FeedbackSaturationMode::None,
            saturation_drive: 1.0,
            tone: 0.0,
        };
        let mut delay = MonoDelay::new(config, 1_000.0);

        // Feed an impulse followed by silence.
        let mut outputs = Vec::with_capacity(20);
        outputs.push(delay.process_sample(1.0));
        for _ in 0..19 {
            outputs.push(delay.process_sample(0.0));
        }

        // Sample 0: dry copy of the impulse (wet component is zero because the
        // delay line was empty → 0 wet output; dry = 1.0).
        assert!(
            (outputs[0] - 1.0).abs() < 0.01,
            "sample 0 expected ~1.0, got {}",
            outputs[0]
        );

        // Samples 1-9: silence (delay line still draining, impulse not yet reached).
        for i in 1..10 {
            assert!(
                outputs[i].abs() < 0.01,
                "sample {i} expected ~0.0, got {}",
                outputs[i]
            );
        }

        // Sample 10: the delayed impulse arrives at wet = 1.0.
        assert!(
            (outputs[10] - 1.0).abs() < 0.01,
            "sample 10 expected ~1.0 (echo), got {}",
            outputs[10]
        );

        // Samples 11-19: silence again.
        for i in 11..20 {
            assert!(
                outputs[i].abs() < 0.01,
                "sample {i} expected ~0.0, got {}",
                outputs[i]
            );
        }
    }
}
