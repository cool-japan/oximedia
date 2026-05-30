//! Tremolo effect - amplitude modulation.

use crate::{
    utils::{Lfo, LfoWaveform},
    AudioEffect,
};

/// Tremolo configuration.
#[derive(Debug, Clone)]
pub struct TremoloConfig {
    /// Rate in Hz.
    pub rate: f32,
    /// Depth (0.0 - 1.0).
    pub depth: f32,
    /// Waveform shape.
    pub waveform: LfoWaveform,
}

impl Default for TremoloConfig {
    fn default() -> Self {
        Self {
            rate: 5.0,
            depth: 0.5,
            waveform: LfoWaveform::Sine,
        }
    }
}

/// Stereo tremolo effect.
pub struct StereoTremolo {
    lfo_l: Lfo,
    lfo_r: Lfo,
    config: TremoloConfig,
}

impl StereoTremolo {
    /// Create new tremolo effect.
    #[must_use]
    pub fn new(config: TremoloConfig, sample_rate: f32, stereo_phase: f32) -> Self {
        let lfo_l = Lfo::new(config.rate, sample_rate, config.waveform);
        let mut lfo_r = Lfo::new(config.rate, sample_rate, config.waveform);
        lfo_r.set_phase(stereo_phase);

        Self {
            lfo_l,
            lfo_r,
            config,
        }
    }

    /// Set rate.
    pub fn set_rate(&mut self, rate: f32) {
        self.config.rate = rate;
        self.lfo_l.set_frequency(rate);
        self.lfo_r.set_frequency(rate);
    }

    /// Set depth.
    pub fn set_depth(&mut self, depth: f32) {
        self.config.depth = depth.clamp(0.0, 1.0);
    }

    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let mod_l = self.lfo_l.next_unipolar();
        let mod_r = self.lfo_r.next_unipolar();

        let gain_l = 1.0 - self.config.depth + mod_l * self.config.depth;
        let gain_r = 1.0 - self.config.depth + mod_r * self.config.depth;

        (input_l * gain_l, input_r * gain_r)
    }
}

impl AudioEffect for StereoTremolo {
    const EFFECT_ID: u64 = 6019;

    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    fn reset(&mut self) {
        self.lfo_l.reset();
        self.lfo_r.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tremolo() {
        let config = TremoloConfig::default();
        let mut tremolo = StereoTremolo::new(config, 48000.0, 0.0);

        let output = tremolo.process_sample(1.0);
        assert!(output.is_finite());
    }
}
