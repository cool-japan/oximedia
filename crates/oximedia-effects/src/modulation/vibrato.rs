//! Vibrato effect - frequency/pitch modulation.

use crate::{
    utils::{FractionalDelayLine, InterpolationMode, Lfo, LfoWaveform},
    AudioEffect,
};

/// Vibrato configuration.
#[derive(Debug, Clone)]
pub struct VibratoConfig {
    /// Rate in Hz.
    pub rate: f32,
    /// Depth in milliseconds.
    pub depth_ms: f32,
    /// Waveform shape.
    pub waveform: LfoWaveform,
}

impl Default for VibratoConfig {
    fn default() -> Self {
        Self {
            rate: 5.0,
            depth_ms: 2.0,
            waveform: LfoWaveform::Sine,
        }
    }
}

/// Stereo vibrato effect.
pub struct StereoVibrato {
    delay_l: FractionalDelayLine,
    delay_r: FractionalDelayLine,
    lfo_l: Lfo,
    lfo_r: Lfo,
    config: VibratoConfig,
    base_delay_ms: f32,
    sample_rate: f32,
}

impl StereoVibrato {
    /// Create new vibrato effect.
    #[must_use]
    pub fn new(config: VibratoConfig, sample_rate: f32) -> Self {
        let base_delay_ms = 5.0;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_delay_samples =
            (((base_delay_ms + config.depth_ms) * sample_rate) / 1000.0) as usize;

        Self {
            delay_l: FractionalDelayLine::new(max_delay_samples.max(1), InterpolationMode::Linear),
            delay_r: FractionalDelayLine::new(max_delay_samples.max(1), InterpolationMode::Linear),
            lfo_l: Lfo::new(config.rate, sample_rate, config.waveform),
            lfo_r: Lfo::new(config.rate, sample_rate, config.waveform),
            config,
            base_delay_ms,
            sample_rate,
        }
    }

    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let mod_l = self.lfo_l.next_unipolar();
        let mod_r = self.lfo_r.next_unipolar();

        let delay_ms_l = self.base_delay_ms + mod_l * self.config.depth_ms;
        let delay_ms_r = self.base_delay_ms + mod_r * self.config.depth_ms;

        let delay_samples_l = (delay_ms_l * self.sample_rate) / 1000.0;
        let delay_samples_r = (delay_ms_r * self.sample_rate) / 1000.0;

        let out_l = self.delay_l.process(input_l, delay_samples_l);
        let out_r = self.delay_r.process(input_r, delay_samples_r);

        (out_l, out_r)
    }
}

impl AudioEffect for StereoVibrato {
    const EFFECT_ID: u64 = 6020;
    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    fn reset(&mut self) {
        self.delay_l.clear();
        self.delay_r.clear();
        self.lfo_l.reset();
        self.lfo_r.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vibrato() {
        let config = VibratoConfig::default();
        let mut vibrato = StereoVibrato::new(config, 48000.0);
        let output = vibrato.process_sample(1.0);
        assert!(output.is_finite());
    }
}
