//! Spring reverb simulation using digital waveguide physical modeling.
//!
//! Models the characteristic chirp and wobble of mechanical spring reverb tanks
//! using two misaligned waveguides (tension waves), a helical mode delay, and
//! all-pass dispersion filters that create the frequency-dependent travel time
//! responsible for the spring's characteristic "boing" sound.
//!
//! # Physical Model
//!
//! A helical spring supports two types of waves:
//! - **Tension waves** (longitudinal): modeled by a bidirectional delay line
//!   with loss and dispersion filters at each end.
//! - **Helical (torsional) waves**: modeled by a shorter delay line producing
//!   the bright upper-frequency resonances.
//!
//! Two spring instances with slightly different delay lengths simulate the
//! typical multi-spring tank used in guitar amplifiers.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::{
    utils::{AllPassFilter, DelayLine},
    AudioEffect,
};

/// Spring reverb configuration.
#[derive(Debug, Clone)]
pub struct SpringReverbConfig {
    /// Spring tension factor in `[0.1, 1.0]`. Higher tension = longer spring,
    /// lower fundamental frequency. Default: `0.5`.
    pub tension: f32,
    /// High-frequency damping in `[0.0, 1.0]`. Higher = darker sound.
    /// Default: `0.4`.
    pub damping: f32,
    /// Dispersion amount in `[0.0, 1.0]`. Controls all-pass coefficient
    /// magnitude (higher = more chirp). Default: `0.3`.
    pub dispersion: f32,
    /// Pre-diffuser coefficient in `[0.0, 0.99]`. Default: `0.6`.
    pub diffusion: f32,
    /// Wet/dry mix in `[0.0, 1.0]`. Default: `0.4`.
    pub wet_mix: f32,
}

impl Default for SpringReverbConfig {
    fn default() -> Self {
        Self {
            tension: 0.5,
            damping: 0.4,
            dispersion: 0.3,
            diffusion: 0.6,
            wet_mix: 0.4,
        }
    }
}

/// Number of all-pass dispersion stages per spring.
const DISP_STAGES: usize = 4;

/// Number of input pre-diffuser stages.
const DIFF_STAGES: usize = 2;

/// A single waveguide spring model.
struct SpringWaveguide {
    /// Forward-traveling wave delay line.
    fwd: DelayLine,
    /// Backward-traveling wave delay line (reflection).
    bwd: DelayLine,
    /// All-pass dispersion filters on the forward path.
    dispersion: [AllPassFilter; DISP_STAGES],
    /// One-pole low-pass filter state (loss / HF damping).
    loss_state: f32,
    /// Loss filter coefficient (closer to 1.0 = more damping).
    loss_coeff: f32,
    /// Spring feedback gain (simulates reflection at far termination).
    feedback: f32,
}

impl SpringWaveguide {
    fn new(delay_ms: f32, sample_rate: f32, damping: f32, dispersion: f32, feedback: f32) -> Self {
        let delay_samp = ((delay_ms * sample_rate / 1000.0) as usize).max(4);

        // Dispersion: 4 all-pass filters with staggered coefficients derived from `dispersion`.
        let base_coeff = (dispersion * 0.5).clamp(0.05, 0.49);
        let disp_coeffs = [
            base_coeff,
            base_coeff * 1.1,
            base_coeff * 1.2,
            base_coeff * 1.3,
        ];
        let dispersion = [
            AllPassFilter::new(disp_coeffs[0].clamp(-0.999, 0.999)),
            AllPassFilter::new(disp_coeffs[1].clamp(-0.999, 0.999)),
            AllPassFilter::new(disp_coeffs[2].clamp(-0.999, 0.999)),
            AllPassFilter::new(disp_coeffs[3].clamp(-0.999, 0.999)),
        ];

        // Loss coefficient: higher damping = more HF rolloff per round trip.
        let loss_coeff = (damping * 0.3).clamp(0.0, 0.95);

        Self {
            fwd: DelayLine::new(delay_samp),
            bwd: DelayLine::new(delay_samp),
            dispersion,
            loss_state: 0.0,
            loss_coeff,
            feedback,
        }
    }

    /// Process one sample: inject `input` into the forward path, return the
    /// backward-path output (reflection from the far end of the spring).
    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        // Read backward (reflected) wave from far end.
        let bwd_out = self.bwd.read(self.bwd.max_delay());

        // Apply dispersion filters to backward output (spring chirp).
        let mut dispersed = bwd_out;
        for ap in &mut self.dispersion {
            dispersed = ap.process(dispersed);
        }

        // Apply one-pole low-pass (loss / HF absorption).
        self.loss_state = dispersed * (1.0 - self.loss_coeff) + self.loss_state * self.loss_coeff;

        // Inject: input + reflected signal (scaled by feedback) into forward path.
        let fwd_in = input + self.loss_state * self.feedback;
        self.fwd.write(fwd_in);

        // At the far end: forward delayed output becomes backward input (reflection).
        let fwd_out = self.fwd.read(self.fwd.max_delay());
        self.bwd.write(fwd_out * self.feedback);

        // The spring output is the near-end backward wave.
        bwd_out
    }

    fn reset(&mut self) {
        self.fwd.clear();
        self.bwd.clear();
        for ap in &mut self.dispersion {
            ap.reset();
        }
        self.loss_state = 0.0;
    }
}

/// Spring reverb effect using two misaligned waveguide springs and a helical mode.
///
/// Produces the characteristic "boing" and chirp of real spring reverb tanks
/// used in guitar amplifiers and classic recording hardware.
pub struct SpringReverb {
    /// Main spring (longer).
    spring1: SpringWaveguide,
    /// Second spring (slightly detuned for richer sound).
    spring2: SpringWaveguide,

    /// Helical / torsional mode delay line (short, bright resonance).
    helical: DelayLine,
    helical_feedback: f32,
    helical_state: f32,

    /// Input pre-diffuser (2 all-pass stages for initial density).
    diffuser: [AllPassFilter; DIFF_STAGES],

    /// LFO phase for subtle tension wobble (simulates spring sag under signal).
    chirp_phase: f32,
    /// LFO advance per sample (very slow, ~2 Hz).
    chirp_rate: f32,

    config: SpringReverbConfig,
    #[allow(dead_code)]
    sample_rate: f32,
}

impl SpringReverb {
    /// Create a spring reverb with the given configuration.
    #[must_use]
    pub fn new(config: SpringReverbConfig, sample_rate: f32) -> Self {
        let tension = config.tension.clamp(0.1, 1.0);
        let damping = config.damping.clamp(0.0, 1.0);
        let dispersion = config.dispersion.clamp(0.0, 1.0);
        let diffusion = config.diffusion.clamp(0.0, 0.99);

        // Spring delay lengths: longer tension = larger delay times.
        let spring1_ms = 30.0 + tension * 20.0; // 30–50 ms
        let spring2_ms = spring1_ms * 1.06; // ~6% detuned

        let helical_ms = 5.0 + tension * 3.0; // 5–8 ms
        let helical_samp = ((helical_ms * sample_rate / 1000.0) as usize).max(4);

        let spring1 = SpringWaveguide::new(
            spring1_ms,
            sample_rate,
            damping,
            dispersion,
            0.85 - damping * 0.2,
        );
        let spring2 = SpringWaveguide::new(
            spring2_ms,
            sample_rate,
            damping,
            dispersion,
            0.82 - damping * 0.2,
        );

        let diffuser = [
            AllPassFilter::new(diffusion * 0.6),
            AllPassFilter::new(diffusion * 0.5),
        ];

        // Chirp rate: ~2 Hz.
        let chirp_rate = 2.0 * std::f32::consts::TAU / sample_rate;

        Self {
            spring1,
            spring2,
            helical: DelayLine::new(helical_samp),
            helical_feedback: 0.4 + tension * 0.15,
            helical_state: 0.0,
            diffuser,
            chirp_phase: 0.0,
            chirp_rate,
            config,
            sample_rate,
        }
    }

    /// Vintage spring tank (high tension, prominent dispersion chirp).
    #[must_use]
    pub fn vintage_tank(sample_rate: f32) -> Self {
        Self::new(
            SpringReverbConfig {
                tension: 0.8,
                damping: 0.35,
                dispersion: 0.55,
                diffusion: 0.7,
                wet_mix: 0.45,
            },
            sample_rate,
        )
    }

    /// Guitar amplifier spring (short, twangy, medium feedback).
    #[must_use]
    pub fn guitar_amp(sample_rate: f32) -> Self {
        Self::new(
            SpringReverbConfig {
                tension: 0.45,
                damping: 0.5,
                dispersion: 0.35,
                diffusion: 0.55,
                wet_mix: 0.35,
            },
            sample_rate,
        )
    }

    /// Large studio spring tank (long decay, smooth density).
    #[must_use]
    pub fn large_tank(sample_rate: f32) -> Self {
        Self::new(
            SpringReverbConfig {
                tension: 0.9,
                damping: 0.25,
                dispersion: 0.45,
                diffusion: 0.75,
                wet_mix: 0.5,
            },
            sample_rate,
        )
    }

    /// Set spring tension (updates delay lengths dynamically is not fully supported in
    /// a ring-buffer context — config is stored for reset/rebuild purposes).
    pub fn set_tension(&mut self, tension: f32) {
        self.config.tension = tension.clamp(0.1, 1.0);
    }

    /// Set HF damping.
    pub fn set_damping(&mut self, damping: f32) {
        self.config.damping = damping.clamp(0.0, 1.0);
    }

    /// Set wet/dry mix.
    pub fn set_wet_mix(&mut self, wet: f32) {
        self.config.wet_mix = wet.clamp(0.0, 1.0);
    }

    /// Get wet/dry mix.
    #[must_use]
    pub fn wet_mix(&self) -> f32 {
        self.config.wet_mix
    }
}

impl AudioEffect for SpringReverb {
    const EFFECT_ID: u64 = 6031;
    fn process_sample(&mut self, input: f32) -> f32 {
        // 1. Apply input pre-diffusion.
        let mut diffused = input;
        for ap in &mut self.diffuser {
            diffused = ap.process(diffused);
        }

        // 2. Process both springs.
        let s1_out = self.spring1.process(diffused * 0.6);
        let s2_out = self.spring2.process(diffused * 0.5);

        // 3. Helical mode: short delay with mild feedback for upper-frequency resonance.
        let helical_in = diffused * 0.15 + self.helical_state * self.helical_feedback;
        let helical_delayed = self.helical.read(self.helical.max_delay());
        self.helical.write(helical_in);
        self.helical_state = helical_delayed;

        // 4. Subtle tension chirp (±1% amplitude wobble at ~2 Hz).
        let chirp_mod = 1.0 + 0.01 * self.chirp_phase.sin();
        self.chirp_phase += self.chirp_rate;
        if self.chirp_phase > std::f32::consts::TAU {
            self.chirp_phase -= std::f32::consts::TAU;
        }

        // 5. Sum and normalize spring outputs.
        let wet = (s1_out + s2_out + helical_delayed * 0.3) * (chirp_mod / 3.0);

        // 6. Wet/dry blend.
        wet * self.config.wet_mix + input * (1.0 - self.config.wet_mix)
    }

    fn reset(&mut self) {
        self.spring1.reset();
        self.spring2.reset();
        self.helical.clear();
        self.helical_state = 0.0;
        for ap in &mut self.diffuser {
            ap.reset();
        }
        self.chirp_phase = 0.0;
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

    fn make_noise(num_samples: usize) -> Vec<f32> {
        // Deterministic pseudo-noise (linear congruential).
        let mut state = 12345_u32;
        (0..num_samples)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn test_spring_reverb_default_config() {
        let _reverb = SpringReverb::new(SpringReverbConfig::default(), 48000.0);
    }

    #[test]
    fn test_spring_reverb_output_finite() {
        let mut reverb = SpringReverb::new(SpringReverbConfig::default(), 48000.0);
        let noise = make_noise(2000);
        for &s in &noise {
            let out = reverb.process_sample(s);
            assert!(out.is_finite(), "Output must remain finite, got: {out}");
        }
    }

    #[test]
    fn test_spring_reverb_silence_input_decays() {
        // Feed some audio to build up reverb, then silence, and verify that
        // the output settles (is bounded and does not grow unboundedly).
        let mut reverb = SpringReverb::new(SpringReverbConfig::default(), 48000.0);

        // Feed 100 ms of a moderate signal.
        for _ in 0..4800 {
            reverb.process_sample(0.5);
        }

        // Now silence for 500 ms and track that the output does not explode.
        let mut all_finite = true;
        for _ in 0..24000 {
            let out = reverb.process_sample(0.0);
            if !out.is_finite() {
                all_finite = false;
                break;
            }
        }
        assert!(
            all_finite,
            "Spring reverb output must remain finite during decay"
        );
    }

    #[test]
    fn test_spring_reverb_preset_vintage_tank() {
        let mut reverb = SpringReverb::vintage_tank(48000.0);
        let noise = make_noise(1024);
        for &s in &noise {
            let out = reverb.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_spring_reverb_preset_guitar_amp() {
        let mut reverb = SpringReverb::guitar_amp(48000.0);
        let noise = make_noise(1024);
        for &s in &noise {
            let out = reverb.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_spring_reverb_preset_large_tank() {
        let mut reverb = SpringReverb::large_tank(48000.0);
        let noise = make_noise(1024);
        for &s in &noise {
            let out = reverb.process_sample(s);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_spring_reverb_wet_dry_mix() {
        let mut reverb = SpringReverb::new(SpringReverbConfig::default(), 48000.0);
        assert!((reverb.wet_mix() - 0.4).abs() < 1e-6);

        reverb.set_wet_mix(0.8);
        assert!((reverb.wet_mix() - 0.8).abs() < 1e-6);

        reverb.set_wet_mix(2.0);
        assert!((reverb.wet_mix() - 1.0).abs() < 1e-6);

        reverb.set_wet_mix(-1.0);
        assert!((reverb.wet_mix() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_spring_reverb_reset() {
        let mut reverb = SpringReverb::new(SpringReverbConfig::default(), 48000.0);

        // Pump audio in.
        for _ in 0..1000 {
            reverb.process_sample(0.9);
        }

        reverb.reset();

        // With wet_mix=0.4 and dry input=0, output contribution from wet should
        // be zero immediately after reset.
        let out = reverb.process_sample(0.0);
        assert!(
            out.abs() < 1e-6,
            "After reset with zero input, output should be zero: {out}"
        );
    }

    #[test]
    fn test_spring_reverb_adds_decay() {
        let mut reverb = SpringReverb::new(
            SpringReverbConfig {
                wet_mix: 1.0,
                ..Default::default()
            },
            48000.0,
        );

        // Send one impulse.
        let _ = reverb.process_sample(1.0);

        // After the impulse, collect the next 100 ms.
        let mut energy = 0.0_f32;
        for _ in 0..4800 {
            let out = reverb.process_sample(0.0);
            energy += out * out;
        }

        // The reverb tail should carry some energy.
        assert!(
            energy > 0.0,
            "Spring reverb should produce a decay tail: energy={energy}"
        );
    }

    #[test]
    fn test_spring_reverb_set_tension() {
        let mut reverb = SpringReverb::new(SpringReverbConfig::default(), 48000.0);
        reverb.set_tension(0.9);
        assert!((reverb.config.tension - 0.9).abs() < 1e-6);

        // Clamp high.
        reverb.set_tension(5.0);
        assert!((reverb.config.tension - 1.0).abs() < 1e-6);

        // Clamp low.
        reverb.set_tension(-1.0);
        assert!((reverb.config.tension - 0.1).abs() < 1e-6);
    }
}
