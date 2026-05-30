#![allow(dead_code)]
//! Ring modulation audio effect.
//!
//! Ring modulation multiplies the input signal by a carrier oscillator,
//! producing sum and difference frequencies. This creates metallic, robotic,
//! or bell-like timbres commonly used in sound design, electronic music,
//! and sci-fi audio production.

use std::f32::consts::PI;

/// Carrier oscillator waveform for the ring modulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CarrierWaveform {
    /// Pure sine wave carrier.
    #[default]
    Sine,
    /// Square wave carrier.
    Square,
    /// Triangle wave carrier.
    Triangle,
    /// Sawtooth wave carrier.
    Sawtooth,
}

/// Configuration for the ring modulator effect.
#[derive(Debug, Clone)]
pub struct RingModConfig {
    /// Carrier frequency in Hz.
    pub frequency_hz: f32,
    /// Mix between dry and wet signal (0.0 = dry, 1.0 = full ring mod).
    pub mix: f32,
    /// Carrier waveform shape.
    pub waveform: CarrierWaveform,
    /// Carrier amplitude (0.0 - 1.0).
    pub carrier_level: f32,
    /// Sample rate in Hz.
    pub sample_rate: f32,
    /// Fine-tune frequency offset in Hz.
    pub detune_hz: f32,
}

impl Default for RingModConfig {
    fn default() -> Self {
        Self {
            frequency_hz: 440.0,
            mix: 1.0,
            waveform: CarrierWaveform::Sine,
            carrier_level: 1.0,
            sample_rate: 48000.0,
            detune_hz: 0.0,
        }
    }
}

/// Ring modulator audio effect processor.
#[derive(Debug)]
pub struct RingModulator {
    /// Current configuration.
    config: RingModConfig,
    /// Current carrier phase (0.0 - 1.0).
    phase: f32,
    /// Phase increment per sample.
    phase_inc: f32,
}

impl RingModulator {
    /// Create a new ring modulator with the given configuration.
    #[must_use]
    pub fn new(config: RingModConfig) -> Self {
        let freq = config.frequency_hz + config.detune_hz;
        let phase_inc = freq / config.sample_rate;
        Self {
            config,
            phase: 0.0,
            phase_inc,
        }
    }

    /// Create a ring modulator with default settings.
    #[must_use]
    pub fn default_effect() -> Self {
        Self::new(RingModConfig::default())
    }

    /// Update the carrier frequency.
    pub fn set_frequency(&mut self, frequency_hz: f32) {
        self.config.frequency_hz = frequency_hz.max(0.001);
        self.update_phase_inc();
    }

    /// Update the detune amount.
    pub fn set_detune(&mut self, detune_hz: f32) {
        self.config.detune_hz = detune_hz;
        self.update_phase_inc();
    }

    /// Set the wet/dry mix.
    pub fn set_mix(&mut self, mix: f32) {
        self.config.mix = mix.clamp(0.0, 1.0);
    }

    /// Set the carrier waveform.
    pub fn set_waveform(&mut self, waveform: CarrierWaveform) {
        self.config.waveform = waveform;
    }

    /// Set the sample rate and recalculate phase increment.
    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        self.config.sample_rate = sample_rate.max(1.0);
        self.update_phase_inc();
    }

    /// Reset the oscillator phase.
    pub fn reset(&mut self) {
        self.phase = 0.0;
    }

    /// Recalculate the phase increment from current settings.
    fn update_phase_inc(&mut self) {
        let freq = self.config.frequency_hz + self.config.detune_hz;
        self.phase_inc = freq / self.config.sample_rate;
    }

    /// Get the current carrier oscillator value.
    fn carrier_value(&self) -> f32 {
        let p = self.phase;
        let raw = match self.config.waveform {
            CarrierWaveform::Sine => (2.0 * PI * p).sin(),
            CarrierWaveform::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            CarrierWaveform::Triangle => {
                if p < 0.5 {
                    4.0 * p - 1.0
                } else {
                    3.0 - 4.0 * p
                }
            }
            CarrierWaveform::Sawtooth => 2.0 * p - 1.0,
        };
        raw * self.config.carrier_level
    }

    /// Advance the oscillator by one sample.
    fn advance(&mut self) {
        self.phase += self.phase_inc;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        while self.phase < 0.0 {
            self.phase += 1.0;
        }
    }

    /// Process a single mono sample.
    pub fn process_sample(&mut self, input: f32) -> f32 {
        let carrier = self.carrier_value();
        self.advance();

        let wet = input * carrier;
        input * (1.0 - self.config.mix) + wet * self.config.mix
    }

    /// Process a mono buffer in-place.
    pub fn process(&mut self, buffer: &mut [f32]) {
        for sample in buffer.iter_mut() {
            *sample = self.process_sample(*sample);
        }
    }

    /// Process stereo buffers in-place.
    pub fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        for i in 0..len {
            let carrier = self.carrier_value();
            self.advance();

            let wet_l = left[i] * carrier;
            let wet_r = right[i] * carrier;
            left[i] = left[i] * (1.0 - self.config.mix) + wet_l * self.config.mix;
            right[i] = right[i] * (1.0 - self.config.mix) + wet_r * self.config.mix;
        }
    }

    /// Get the current carrier frequency including detune.
    #[must_use]
    pub fn effective_frequency(&self) -> f32 {
        self.config.frequency_hz + self.config.detune_hz
    }

    /// Compute the expected output frequencies for a given input frequency.
    #[must_use]
    pub fn output_frequencies(&self, input_freq: f32) -> (f32, f32) {
        let carrier_freq = self.effective_frequency();
        let sum = input_freq + carrier_freq;
        let diff = (input_freq - carrier_freq).abs();
        (sum, diff)
    }
}

// ─── Enhanced ring modulator (AudioEffect-compliant) ─────────────────────────

use crate::AudioEffect;

/// Configuration for [`RingModEffect`], the `AudioEffect`-implementing ring
/// modulator.
///
/// This configuration mirrors the interface described in the module spec and
/// uses field names that make the role of each parameter unambiguous.
#[derive(Debug, Clone)]
pub struct RingModulatorConfig {
    /// Carrier oscillator frequency in Hz.
    ///
    /// The carrier is a continuous-phase oscillator whose waveform is set by
    /// `carrier_waveform`.  Typical ranges:
    /// - Sub-audio (0.1 – 20 Hz): tremolo-like amplitude modulation.
    /// - Audio (20 – 20 000 Hz): classic ring-mod metallic timbre.
    pub carrier_freq_hz: f32,

    /// Carrier oscillator waveform.
    pub carrier_waveform: CarrierWaveform,

    /// Modulation depth in `[0.0, 1.0]`.
    ///
    /// `0.0` = no modulation (carrier is ignored, output equals dry signal).
    /// `1.0` = full-depth ring modulation.
    pub mod_depth: f32,

    /// Wet / dry ratio in `[0.0, 1.0]`.
    ///
    /// `0.0` = fully dry (input passes unchanged).
    /// `1.0` = fully wet (only the ring-modulated signal is output).
    pub wet_dry: f32,
}

impl Default for RingModulatorConfig {
    fn default() -> Self {
        Self {
            carrier_freq_hz: 440.0,
            carrier_waveform: CarrierWaveform::Sine,
            mod_depth: 1.0,
            wet_dry: 1.0,
        }
    }
}

/// Ring modulator implementing the [`AudioEffect`] trait.
///
/// # Signal flow
///
/// ```text
/// input ──► × carrier(phase) ──► mod_depth scale ──► wet/dry mix ──► output
///               ↑
///         continuous-phase oscillator (freq / sample_rate per sample)
/// ```
///
/// The carrier oscillator is advanced by `phase_increment` after each output
/// sample; `phase` is maintained in `[0.0, 1.0)`.
#[derive(Debug)]
pub struct RingModEffect {
    config: RingModulatorConfig,
    /// Oscillator phase in `[0.0, 1.0)`.
    phase: f32,
    /// Sample rate used to derive `phase_increment`.
    sample_rate: f32,
    /// Phase advance per input sample (`carrier_freq_hz / sample_rate`).
    phase_increment: f32,
}

impl RingModEffect {
    /// Create a new [`RingModEffect`] with the supplied configuration and
    /// sample rate.
    ///
    /// `carrier_freq_hz` is clamped to `(0.0, sample_rate / 2]` (Nyquist).
    /// `mod_depth` and `wet_dry` are clamped to `[0.0, 1.0]`.
    #[must_use]
    pub fn new(config: RingModulatorConfig, sample_rate: f32) -> Self {
        let sample_rate = sample_rate.max(1.0);
        let nyquist = sample_rate * 0.5;
        let freq = config.carrier_freq_hz.clamp(0.001, nyquist);
        let phase_increment = freq / sample_rate;
        let config = RingModulatorConfig {
            carrier_freq_hz: freq,
            mod_depth: config.mod_depth.clamp(0.0, 1.0),
            wet_dry: config.wet_dry.clamp(0.0, 1.0),
            ..config
        };
        Self {
            config,
            phase: 0.0,
            sample_rate,
            phase_increment,
        }
    }

    /// Update the carrier frequency and recompute the phase increment.
    ///
    /// The new frequency is clamped to `(0.0, sample_rate / 2]`.
    pub fn set_carrier_freq(&mut self, freq_hz: f32) {
        let nyquist = self.sample_rate * 0.5;
        self.config.carrier_freq_hz = freq_hz.clamp(0.001, nyquist);
        self.phase_increment = self.config.carrier_freq_hz / self.sample_rate;
    }

    /// Set the modulation depth (clamped to `[0.0, 1.0]`).
    pub fn set_mod_depth(&mut self, depth: f32) {
        self.config.mod_depth = depth.clamp(0.0, 1.0);
    }

    /// Set the carrier waveform.
    pub fn set_carrier_waveform(&mut self, waveform: CarrierWaveform) {
        self.config.carrier_waveform = waveform;
    }

    /// Return the current carrier frequency in Hz.
    #[must_use]
    pub fn carrier_freq_hz(&self) -> f32 {
        self.config.carrier_freq_hz
    }

    /// Return the current oscillator phase in `[0.0, 1.0)`.
    #[must_use]
    pub fn phase(&self) -> f32 {
        self.phase
    }

    // ── Internal ──────────────────────────────────────────────────────────

    /// Evaluate the carrier oscillator at the given `phase` in `[0.0, 1.0)`.
    fn generate_carrier(&self, phase: f32) -> f32 {
        match self.config.carrier_waveform {
            CarrierWaveform::Sine => (2.0 * PI * phase).sin(),
            CarrierWaveform::Square => {
                if phase < 0.5 {
                    1.0_f32
                } else {
                    -1.0_f32
                }
            }
            CarrierWaveform::Triangle => {
                // Rises from -1 at phase=0 to +1 at phase=0.5, falls back to -1 at phase=1.
                // formula: 4 * |phase - 0.5| - 1
                4.0 * (phase - 0.5).abs() - 1.0
            }
            CarrierWaveform::Sawtooth => 2.0 * phase - 1.0,
        }
    }

    /// Advance the oscillator phase by one sample.
    fn advance_phase(&mut self) {
        self.phase += self.phase_increment;
        // Keep phase strictly in [0, 1).
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        // Guard against negative phase (e.g. after a frequency update).
        if self.phase < 0.0 {
            self.phase += 1.0;
        }
    }
}

impl AudioEffect for RingModEffect {

    /// Process a single mono sample.
    ///
    /// ```text
    /// carrier    = generate_carrier(phase)
    /// modulated  = input * carrier * mod_depth
    /// output     = dry * input + wet * modulated
    /// ```
    fn process_sample(&mut self, input: f32) -> f32 {
        let carrier = self.generate_carrier(self.phase);
        self.advance_phase();

        let modulated = input * carrier * self.config.mod_depth;
        let wet = self.config.wet_dry;
        let dry = 1.0 - wet;
        dry * input + wet * modulated
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate.max(1.0);
        // Re-clamp frequency against new Nyquist and recompute increment.
        let nyquist = self.sample_rate * 0.5;
        self.config.carrier_freq_hz = self.config.carrier_freq_hz.min(nyquist);
        self.phase_increment = self.config.carrier_freq_hz / self.sample_rate;
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.config.wet_dry = wet.clamp(0.0, 1.0);
    }

    fn wet_dry(&self) -> f32 {
        self.config.wet_dry
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = RingModConfig::default();
        assert!((cfg.frequency_hz - 440.0).abs() < 1e-6);
        assert!((cfg.mix - 1.0).abs() < 1e-6);
        assert_eq!(cfg.waveform, CarrierWaveform::Sine);
    }

    #[test]
    fn test_carrier_waveform_default() {
        assert_eq!(CarrierWaveform::default(), CarrierWaveform::Sine);
    }

    #[test]
    fn test_ring_mod_creation() {
        let rm = RingModulator::default_effect();
        assert!((rm.phase - 0.0).abs() < 1e-9);
        assert!(rm.phase_inc > 0.0);
    }

    #[test]
    fn test_set_frequency() {
        let mut rm = RingModulator::default_effect();
        rm.set_frequency(1000.0);
        assert!((rm.config.frequency_hz - 1000.0).abs() < 1e-6);
        let expected_inc = 1000.0 / 48000.0;
        assert!((rm.phase_inc - expected_inc).abs() < 1e-9);
    }

    #[test]
    fn test_set_mix() {
        let mut rm = RingModulator::default_effect();
        rm.set_mix(0.5);
        assert!((rm.config.mix - 0.5).abs() < 1e-6);
        rm.set_mix(2.0);
        assert!((rm.config.mix - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_dry_mix_passthrough() {
        let mut rm = RingModulator::new(RingModConfig {
            mix: 0.0,
            ..Default::default()
        });
        let input = 0.75f32;
        let output = rm.process_sample(input);
        assert!((output - input).abs() < 1e-6);
    }

    #[test]
    fn test_silence_input() {
        let mut rm = RingModulator::default_effect();
        let mut buffer = vec![0.0f32; 256];
        rm.process(&mut buffer);
        for &s in &buffer {
            assert!(s.abs() < 1e-9, "Ring mod of silence should be silence");
        }
    }

    #[test]
    fn test_carrier_sine_range() {
        let mut rm = RingModulator::new(RingModConfig {
            frequency_hz: 100.0,
            sample_rate: 1000.0,
            ..Default::default()
        });
        for _ in 0..1000 {
            let v = rm.carrier_value();
            assert!(v >= -1.0 && v <= 1.0);
            rm.advance();
        }
    }

    #[test]
    fn test_carrier_square() {
        let rm = RingModulator::new(RingModConfig {
            waveform: CarrierWaveform::Square,
            frequency_hz: 100.0,
            sample_rate: 1000.0,
            ..Default::default()
        });
        let v = rm.carrier_value();
        assert!(v.abs() > 0.99, "Square wave should be at extremes");
    }

    #[test]
    fn test_process_buffer() {
        let mut rm = RingModulator::default_effect();
        let mut buffer = vec![0.5f32; 128];
        rm.process(&mut buffer);
        // Output should not be all identical (carrier modulates)
        let all_same = buffer.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9);
        assert!(!all_same, "Ring mod should produce varying output");
    }

    #[test]
    fn test_process_stereo() {
        let mut rm = RingModulator::default_effect();
        let mut left = vec![0.5f32; 64];
        let mut right = vec![0.5f32; 64];
        rm.process_stereo(&mut left, &mut right);
        // Both channels should be affected
        assert!(left.iter().any(|&s| (s - 0.5).abs() > 0.01));
    }

    #[test]
    fn test_output_frequencies() {
        let rm = RingModulator::new(RingModConfig {
            frequency_hz: 1000.0,
            ..Default::default()
        });
        let (sum, diff) = rm.output_frequencies(440.0);
        assert!((sum - 1440.0).abs() < 1e-3);
        assert!((diff - 560.0).abs() < 1e-3);
    }

    #[test]
    fn test_detune() {
        let mut rm = RingModulator::default_effect();
        rm.set_detune(5.0);
        assert!((rm.effective_frequency() - 445.0).abs() < 1e-6);
    }

    #[test]
    fn test_reset() {
        let mut rm = RingModulator::default_effect();
        for _ in 0..1000 {
            rm.advance();
        }
        assert!(rm.phase > 0.0);
        rm.reset();
        assert!((rm.phase - 0.0).abs() < 1e-9);
    }

    // ── RingModEffect (AudioEffect-compliant) tests ───────────────────────

    fn make_effect(freq: f32, waveform: CarrierWaveform, depth: f32, wet: f32) -> RingModEffect {
        RingModEffect::new(
            RingModulatorConfig {
                carrier_freq_hz: freq,
                carrier_waveform: waveform,
                mod_depth: depth,
                wet_dry: wet,
            },
            48000.0,
        )
    }

    #[test]
    fn test_ring_mod_effect_output_is_finite() {
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 1.0);
        for i in 0..256 {
            let input = (i as f32 / 128.0) - 1.0;
            let out = fx.process_sample(input);
            assert!(out.is_finite(), "output must be finite");
        }
    }

    #[test]
    fn test_ring_mod_effect_silence_is_silence() {
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 1.0);
        for _ in 0..128 {
            let out = fx.process_sample(0.0);
            assert!(out.abs() < 1e-9, "ring mod of silence should be silence");
        }
    }

    #[test]
    fn test_ring_mod_effect_dry_bypass() {
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 0.0);
        let input = 0.75_f32;
        let out = fx.process_sample(input);
        assert!(
            (out - input).abs() < 1e-6,
            "wet=0.0 should pass input unchanged"
        );
    }

    #[test]
    fn test_ring_mod_effect_modulates_varying_output() {
        let mut fx = make_effect(100.0, CarrierWaveform::Sine, 1.0, 1.0);
        let buf: Vec<f32> = (0..256).map(|_| fx.process_sample(0.5)).collect();
        let all_same = buf.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9);
        assert!(!all_same, "ring mod should produce varying output");
        // process() method (trait default)
        let mut buf2 = vec![0.5_f32; 128];
        fx.process(&mut buf2);
        for &s in &buf2 {
            assert!(s.is_finite());
        }
        let _ = buf.len(); // suppress unused warning
    }

    #[test]
    fn test_ring_mod_effect_all_waveforms() {
        for waveform in [
            CarrierWaveform::Sine,
            CarrierWaveform::Square,
            CarrierWaveform::Triangle,
            CarrierWaveform::Sawtooth,
        ] {
            let mut fx = make_effect(1000.0, waveform, 1.0, 1.0);
            for _ in 0..128 {
                let out = fx.process_sample(0.5);
                assert!(
                    out.is_finite() && out.abs() <= 1.0 + 1e-5,
                    "waveform {waveform:?} output out of range: {out}"
                );
            }
        }
    }

    #[test]
    fn test_ring_mod_effect_triangle_range() {
        // Triangle carrier should be in [-1, +1] at every phase step.
        let fx = make_effect(50.0, CarrierWaveform::Triangle, 1.0, 1.0);
        for i in 0..1000 {
            let phase = i as f32 / 1000.0;
            let v = fx.generate_carrier(phase);
            assert!(
                v >= -1.0 - 1e-6 && v <= 1.0 + 1e-6,
                "triangle out of range at phase {phase}: {v}"
            );
        }
    }

    #[test]
    fn test_ring_mod_effect_square_only_extremes() {
        let fx = make_effect(50.0, CarrierWaveform::Square, 1.0, 1.0);
        for i in 0..1000 {
            let phase = i as f32 / 1000.0;
            let v = fx.generate_carrier(phase);
            let is_extreme = (v - 1.0).abs() < 1e-6 || (v + 1.0).abs() < 1e-6;
            assert!(is_extreme, "square wave must be ±1, got {v}");
        }
    }

    #[test]
    fn test_ring_mod_effect_phase_advances() {
        let mut fx = make_effect(48000.0 * 0.1, CarrierWaveform::Sine, 1.0, 1.0);
        // With freq = 4800 Hz at 48000 Hz, phase_increment = 0.1.
        assert!((fx.phase_increment - 0.1).abs() < 1e-6);
        fx.process_sample(0.5);
        // After one sample the phase should have advanced by ~0.1.
        assert!((fx.phase - 0.1).abs() < 1e-5);
    }

    #[test]
    fn test_ring_mod_effect_phase_wraps() {
        // High frequency = large increment → phase must stay in [0, 1).
        let mut fx = make_effect(10000.0, CarrierWaveform::Sine, 1.0, 1.0);
        for _ in 0..10000 {
            fx.process_sample(0.3);
            assert!(fx.phase >= 0.0 && fx.phase < 1.0, "phase out of range");
        }
    }

    #[test]
    fn test_ring_mod_effect_reset_clears_phase() {
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 1.0);
        for _ in 0..1000 {
            fx.process_sample(0.5);
        }
        assert!(fx.phase > 0.0);
        fx.reset();
        assert!((fx.phase - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_ring_mod_effect_set_carrier_freq() {
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 1.0);
        fx.set_carrier_freq(1000.0);
        assert!((fx.carrier_freq_hz() - 1000.0).abs() < 1e-3);
        let expected_inc = 1000.0 / 48000.0;
        assert!((fx.phase_increment - expected_inc).abs() < 1e-7);
    }

    #[test]
    fn test_ring_mod_effect_set_sample_rate() {
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 1.0);
        fx.set_sample_rate(44100.0);
        let expected_inc = 440.0 / 44100.0;
        assert!(
            (fx.phase_increment - expected_inc).abs() < 1e-7,
            "phase_increment should update after set_sample_rate"
        );
    }

    #[test]
    fn test_ring_mod_effect_wet_dry_trait() {
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 0.5);
        assert!((fx.wet_dry() - 0.5).abs() < 1e-6);
        fx.set_wet_dry(0.8);
        assert!((fx.wet_dry() - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_ring_mod_effect_mod_depth_zero_is_silence() {
        // depth=0 → modulated = input * carrier * 0 = 0; with wet=1 output = 0.
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 0.0, 1.0);
        for _ in 0..128 {
            let out = fx.process_sample(0.7);
            assert!(
                out.abs() < 1e-9,
                "mod_depth=0 should output silence when wet=1"
            );
        }
    }

    #[test]
    fn test_ring_mod_effect_nyquist_clamping() {
        // Requesting a frequency above Nyquist should be clamped.
        let fx = RingModEffect::new(
            RingModulatorConfig {
                carrier_freq_hz: 100_000.0, // way above Nyquist
                ..Default::default()
            },
            48000.0,
        );
        assert!(
            fx.carrier_freq_hz() <= 24000.0 + 1e-3,
            "carrier_freq_hz must be <= Nyquist; got {}",
            fx.carrier_freq_hz()
        );
    }

    #[test]
    fn test_ring_mod_effect_stereo_processing() {
        // The default AudioEffect::process_stereo calls process_sample_stereo,
        // which in turn calls process_sample(left) then process_sample(right),
        // advancing the oscillator phase once per channel.  Therefore L and R
        // use consecutive carrier values and will in general differ — that is
        // acceptable and expected behaviour for a mono-carrier ring modulator
        // exposed through the generic AudioEffect trait.
        let mut fx = make_effect(440.0, CarrierWaveform::Sine, 1.0, 1.0);
        let mut left = vec![0.5_f32; 64];
        let mut right = vec![0.5_f32; 64];
        fx.process_stereo(&mut left, &mut right);
        // All outputs must be finite and within [-1, +1].
        for (&l, &r) in left.iter().zip(right.iter()) {
            assert!(l.is_finite() && l.abs() <= 1.0 + 1e-5);
            assert!(r.is_finite() && r.abs() <= 1.0 + 1e-5);
        }
        // At least some samples should differ from the original 0.5 (modulation applied).
        assert!(
            left.iter().any(|&s| (s - 0.5).abs() > 0.01),
            "left channel should be modulated"
        );
        assert!(
            right.iter().any(|&s| (s - 0.5).abs() > 0.01),
            "right channel should be modulated"
        );
    }
}
