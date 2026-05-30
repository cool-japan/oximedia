//! Channel vocoder implementation.
//!
//! Supports 4–64 analysis/synthesis bands for high-resolution vocoding.
//! Bands are logarithmically spaced from 80 Hz to min(18 kHz, Nyquist*0.9).
//! The bandpass resonance (Q) scales with band count so that adjacent bands
//! maintain good spectral separation even at 64 bands.

use crate::{
    filter::{FilterMode, StateVariableConfig, StateVariableFilter},
    utils::EnvelopeFollower,
    AudioEffect,
};

/// Vocoder configuration.
#[derive(Debug, Clone)]
pub struct VocoderConfig {
    /// Number of frequency bands (clamped to `[4, 64]`).
    ///
    /// Common values:
    /// - 16 — traditional vocoder
    /// - 32 — high-resolution vocoding (good intelligibility)
    /// - 64 — studio-quality formant accuracy
    pub bands: usize,
    /// Envelope follower attack time in milliseconds.
    pub attack_ms: f32,
    /// Envelope follower release time in milliseconds.
    pub release_ms: f32,
    /// Minimum analysis frequency in Hz (default: `80.0`).
    pub min_freq: f32,
    /// Maximum analysis frequency in Hz (default: `18000.0`, capped at Nyquist·0.9).
    pub max_freq: f32,
}

impl Default for VocoderConfig {
    fn default() -> Self {
        Self {
            bands: 32,
            attack_ms: 5.0,
            release_ms: 50.0,
            min_freq: 80.0,
            max_freq: 18_000.0,
        }
    }
}

/// A single analysis/synthesis band.
struct VocoderBand {
    /// Bandpass filter applied to the modulator signal.
    modulator_filter: StateVariableFilter,
    /// Bandpass filter applied to the carrier signal (same frequency).
    carrier_filter: StateVariableFilter,
    /// Envelope follower that tracks modulator band energy.
    envelope: EnvelopeFollower,
}

/// Channel vocoder effect.
///
/// Imposes the spectral envelope of one signal (the *modulator*, typically
/// a voice) onto another (the *carrier*, e.g. a synthesizer pad).
///
/// ## Band count guidance
///
/// | Bands | Use-case |
/// |------:|----------|
/// |  4–8  | Robotic / heavy processing |
/// | 12–16 | Classic vocoder sound |
/// | 24–32 | High intelligibility speech vocoding |
/// | 48–64 | Studio-quality formant preservation |
///
/// ## Example
///
/// ```ignore
/// use oximedia_effects::vocoder::{Vocoder, VocoderConfig};
///
/// let config = VocoderConfig { bands: 32, ..Default::default() };
/// let mut vocoder = Vocoder::new(config, 48000.0);
/// let out = vocoder.process(voice_sample, synth_sample);
/// ```
pub struct Vocoder {
    bands: Vec<VocoderBand>,
    #[allow(dead_code)]
    config: VocoderConfig,
}

impl Vocoder {
    /// Create a new vocoder.
    ///
    /// Band count is clamped to `[4, 64]`. Bandpass Q is automatically scaled
    /// to maintain spectral separation:
    ///
    /// ```text
    /// Q = clamp(1.5 + num_bands / 16, 1.5, 12.0)
    /// ```
    #[must_use]
    pub fn new(config: VocoderConfig, sample_rate: f32) -> Self {
        let num_bands = config.bands.clamp(4, 64);

        let min_freq = config.min_freq.clamp(20.0, 2000.0);
        let nyquist_safe = sample_rate * 0.45;
        let max_freq = config.max_freq.min(nyquist_safe).max(min_freq * 2.0);

        // Scale resonance (Q) with band count for good separation.
        #[allow(clippy::cast_precision_loss)]
        let resonance = (1.5_f32 + num_bands as f32 / 16.0).min(12.0);

        let bands: Vec<VocoderBand> = (0..num_bands)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let ratio = if num_bands > 1 {
                    i as f32 / (num_bands - 1) as f32
                } else {
                    0.5
                };

                // Logarithmic frequency spacing.
                let frequency = min_freq * (max_freq / min_freq).powf(ratio);

                let filter_config = StateVariableConfig {
                    frequency,
                    resonance,
                    mode: FilterMode::BandPass,
                };

                VocoderBand {
                    modulator_filter: StateVariableFilter::new(filter_config.clone(), sample_rate),
                    carrier_filter: StateVariableFilter::new(filter_config, sample_rate),
                    envelope: EnvelopeFollower::new(
                        config.attack_ms,
                        config.release_ms,
                        sample_rate,
                    ),
                }
            })
            .collect();

        Self { bands, config }
    }

    /// Return the actual number of analysis/synthesis bands in use.
    #[must_use]
    pub fn num_bands(&self) -> usize {
        self.bands.len()
    }

    /// Process one sample pair: `modulator` (e.g. voice) and `carrier` (e.g. synth).
    ///
    /// The modulator's spectral envelope is extracted per band and applied to
    /// the carrier. Output is normalized by band count so amplitude is stable
    /// regardless of how many bands are active.
    pub fn process(&mut self, modulator: f32, carrier: f32) -> f32 {
        let mut output = 0.0_f32;

        for band in &mut self.bands {
            // 1. Filter modulator through analysis bandpass.
            let mod_filtered = band.modulator_filter.process_sample(modulator);

            // 2. Track modulator band amplitude with the envelope follower.
            let envelope = band.envelope.process(mod_filtered);

            // 3. Filter carrier through synthesis bandpass (same frequency).
            let car_filtered = band.carrier_filter.process_sample(carrier);

            // 4. Scale carrier by modulator envelope → spectrally shaped output.
            output += car_filtered * envelope;
        }

        // Normalize by band count to keep output level independent of num_bands.
        #[allow(clippy::cast_precision_loss)]
        let scale = 1.0 / self.bands.len() as f32;
        output * scale
    }
}

impl AudioEffect for Vocoder {

    const EFFECT_ID: u64 = 6033;
    /// Process a single mono sample using input as both modulator and carrier.
    fn process_sample(&mut self, input: f32) -> f32 {
        self.process(input, input)
    }

    fn reset(&mut self) {
        for band in &mut self.bands {
            band.modulator_filter.reset();
            band.carrier_filter.reset();
            band.envelope.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vocoder_default_process() {
        let config = VocoderConfig::default();
        let mut vocoder = Vocoder::new(config, 48000.0);
        let output = vocoder.process(0.5, 0.3);
        assert!(output.is_finite(), "output must be finite: {output}");
    }

    #[test]
    fn test_vocoder_default_band_count() {
        let config = VocoderConfig::default();
        let vocoder = Vocoder::new(config, 48000.0);
        assert_eq!(vocoder.num_bands(), 32, "default should be 32 bands");
    }

    #[test]
    fn test_vocoder_32_bands() {
        let config = VocoderConfig {
            bands: 32,
            ..Default::default()
        };
        let vocoder = Vocoder::new(config, 48000.0);
        assert_eq!(vocoder.num_bands(), 32);
    }

    #[test]
    fn test_vocoder_64_bands() {
        let config = VocoderConfig {
            bands: 64,
            ..Default::default()
        };
        let mut vocoder = Vocoder::new(config, 48000.0);
        assert_eq!(vocoder.num_bands(), 64);
        let out = vocoder.process(0.3, 0.7);
        assert!(out.is_finite(), "64-band output must be finite: {out}");
    }

    #[test]
    fn test_vocoder_clamp_max() {
        let config = VocoderConfig {
            bands: 128,
            ..Default::default()
        };
        let vocoder = Vocoder::new(config, 48000.0);
        assert_eq!(vocoder.num_bands(), 64, "bands must clamp at 64");
    }

    #[test]
    fn test_vocoder_clamp_min() {
        let config = VocoderConfig {
            bands: 1,
            ..Default::default()
        };
        let vocoder = Vocoder::new(config, 48000.0);
        assert_eq!(vocoder.num_bands(), 4, "bands must clamp to minimum 4");
    }

    #[test]
    fn test_vocoder_reset() {
        let config = VocoderConfig {
            bands: 16,
            ..Default::default()
        };
        let mut vocoder = Vocoder::new(config, 48000.0);
        for _ in 0..1000 {
            vocoder.process(0.9, 0.9);
        }
        vocoder.reset();
        let out = vocoder.process(0.0, 0.0);
        assert!(
            out.abs() < 1e-6,
            "output after reset on silence must be ~0: {out}"
        );
    }

    #[test]
    fn test_vocoder_mono_process_sample() {
        let config = VocoderConfig::default();
        let mut vocoder = Vocoder::new(config, 48000.0);
        let out = vocoder.process_sample(0.5);
        assert!(out.is_finite());
    }

    #[test]
    fn test_vocoder_output_finite_bulk() {
        let config = VocoderConfig {
            bands: 32,
            ..Default::default()
        };
        let mut vocoder = Vocoder::new(config, 48000.0);
        use std::f32::consts::TAU;
        for i in 0..4800 {
            let mod_s = (i as f32 * TAU * 300.0 / 48000.0).sin() * 0.5;
            let car_s = (i as f32 * TAU * 440.0 / 48000.0).sin() * 0.8;
            let out = vocoder.process(mod_s, car_s);
            assert!(out.is_finite(), "output at sample {i} not finite: {out}");
        }
    }
}
