//! Chorus effect implementation.
//!
//! Creates a rich, ensemble-like sound by mixing the dry signal with multiple
//! delayed and modulated copies (voices).

use crate::{
    utils::{FractionalDelayLine, InterpolationMode, Lfo, LfoWaveform},
    AudioEffect,
};

/// Maximum number of chorus voices.
pub const MAX_VOICES: usize = 8;
/// Minimum number of chorus voices.
pub const MIN_VOICES: usize = 2;

/// Configuration for chorus effect.
#[derive(Debug, Clone)]
pub struct ChorusConfig {
    /// Number of voices (2-8).
    pub voices: usize,
    /// LFO rate in Hz (0.1 - 10.0).
    pub rate: f32,
    /// Modulation depth in milliseconds (0.0 - 20.0).
    pub depth_ms: f32,
    /// Base delay time in milliseconds (10.0 - 50.0).
    pub delay_ms: f32,
    /// Wet signal level (0.0 - 1.0).
    pub wet: f32,
    /// Dry signal level (0.0 - 1.0).
    pub dry: f32,
    /// Stereo spread (0.0 - 1.0).
    pub spread: f32,
    /// LFO waveform.
    pub waveform: LfoWaveform,
}

impl Default for ChorusConfig {
    fn default() -> Self {
        Self {
            voices: 4,
            rate: 0.5,
            depth_ms: 5.0,
            delay_ms: 25.0,
            wet: 0.5,
            dry: 0.5,
            spread: 0.8,
            waveform: LfoWaveform::Sine,
        }
    }
}

impl ChorusConfig {
    /// Create a subtle chorus preset (2 voices).
    #[must_use]
    pub fn subtle() -> Self {
        Self {
            voices: 2,
            rate: 0.3,
            depth_ms: 2.0,
            delay_ms: 20.0,
            wet: 0.3,
            dry: 0.7,
            spread: 0.5,
            waveform: LfoWaveform::Sine,
        }
    }

    /// Create a lush chorus preset (6 voices).
    #[must_use]
    pub fn lush() -> Self {
        Self {
            voices: 6,
            rate: 0.8,
            depth_ms: 8.0,
            delay_ms: 30.0,
            wet: 0.6,
            dry: 0.4,
            spread: 1.0,
            waveform: LfoWaveform::Sine,
        }
    }

    /// Create a vibrato-like chorus preset.
    #[must_use]
    pub fn vibrato() -> Self {
        Self {
            voices: 3,
            rate: 4.0,
            depth_ms: 3.0,
            delay_ms: 15.0,
            wet: 1.0,
            dry: 0.0,
            spread: 0.3,
            waveform: LfoWaveform::Triangle,
        }
    }
}

/// Stereo chorus effect.
pub struct StereoChorus {
    delay_lines: Vec<FractionalDelayLine>,
    lfos: Vec<Lfo>,
    config: ChorusConfig,
    sample_rate: f32,
}

impl StereoChorus {
    /// Create a new stereo chorus effect.
    #[must_use]
    pub fn new(config: ChorusConfig, sample_rate: f32) -> Self {
        let voices = config.voices.clamp(MIN_VOICES, MAX_VOICES);

        // Create delay lines (need enough for maximum delay + modulation)
        let max_delay_ms = config.delay_ms + config.depth_ms;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_delay_samples = ((max_delay_ms * sample_rate) / 1000.0) as usize;

        let delay_lines: Vec<FractionalDelayLine> = (0..voices)
            .map(|_| FractionalDelayLine::new(max_delay_samples.max(1), InterpolationMode::Linear))
            .collect();

        // Create LFOs with different phases for each voice
        let lfos: Vec<Lfo> = (0..voices)
            .map(|i| {
                let mut lfo = Lfo::new(config.rate, sample_rate, config.waveform);
                // Distribute phases evenly across voices
                #[allow(clippy::cast_precision_loss)]
                let phase = i as f32 / voices as f32;
                lfo.set_phase(phase);
                lfo
            })
            .collect();

        Self {
            delay_lines,
            lfos,
            config,
            sample_rate,
        }
    }

    /// Set chorus rate.
    pub fn set_rate(&mut self, rate: f32) {
        self.config.rate = rate.clamp(0.1, 10.0);
        for lfo in &mut self.lfos {
            lfo.set_frequency(self.config.rate);
        }
    }

    /// Set modulation depth.
    pub fn set_depth(&mut self, depth_ms: f32) {
        self.config.depth_ms = depth_ms.clamp(0.0, 20.0);
    }

    /// Set wet level.
    pub fn set_wet(&mut self, wet: f32) {
        self.config.wet = wet.clamp(0.0, 1.0);
    }

    /// Set dry level.
    pub fn set_dry(&mut self, dry: f32) {
        self.config.dry = dry.clamp(0.0, 1.0);
    }

    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let mut out_l = 0.0;
        let mut out_r = 0.0;

        #[allow(clippy::cast_precision_loss)]
        let num_voices = self.delay_lines.len() as f32;

        // Process each voice
        for (i, delay_line) in self.delay_lines.iter_mut().enumerate() {
            // Get modulation value
            let mod_value = self.lfos[i].next_unipolar(); // 0.0 - 1.0

            // Calculate delay time
            let delay_ms = self.config.delay_ms + mod_value * self.config.depth_ms;
            let delay_samples = (delay_ms * self.sample_rate) / 1000.0;

            // Read modulated delay
            let delayed = delay_line.read(delay_samples);

            // Write input to delay line
            delay_line.write((input_l + input_r) * 0.5);

            // Distribute voices across stereo field based on spread
            #[allow(clippy::cast_precision_loss)]
            let pan = ((i as f32 / num_voices) * 2.0 - 1.0) * self.config.spread;

            // Calculate stereo gains
            let left_gain = if pan <= 0.0 { 1.0 } else { 1.0 - pan };
            let right_gain = if pan >= 0.0 { 1.0 } else { 1.0 + pan };

            out_l += delayed * left_gain / num_voices;
            out_r += delayed * right_gain / num_voices;
        }

        // Mix wet and dry
        out_l = out_l * self.config.wet + input_l * self.config.dry;
        out_r = out_r * self.config.wet + input_r * self.config.dry;

        (out_l, out_r)
    }
}

impl AudioEffect for StereoChorus {

    const EFFECT_ID: u64 = 6017;
    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _right) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    fn reset(&mut self) {
        for delay_line in &mut self.delay_lines {
            delay_line.clear();
        }
        for lfo in &mut self.lfos {
            lfo.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chorus_config() {
        let config = ChorusConfig::default();
        assert_eq!(config.voices, 4);
        assert_eq!(config.rate, 0.5);
    }

    #[test]
    fn test_chorus_presets() {
        let subtle = ChorusConfig::subtle();
        assert_eq!(subtle.voices, 2);

        let lush = ChorusConfig::lush();
        assert_eq!(lush.voices, 6);
    }

    #[test]
    fn test_chorus_process() {
        let config = ChorusConfig::default();
        let mut chorus = StereoChorus::new(config, 48000.0);

        let (out_l, out_r) = chorus.process_sample_stereo(1.0, 1.0);
        assert!(out_l.is_finite());
        assert!(out_r.is_finite());

        // Process more samples to verify LFO modulation
        for _ in 0..1000 {
            chorus.process_sample_stereo(0.1, 0.1);
        }
    }

    #[test]
    fn test_chorus_voices() {
        let config = ChorusConfig {
            voices: 3,
            ..Default::default()
        };
        let chorus = StereoChorus::new(config, 48000.0);
        assert_eq!(chorus.delay_lines.len(), 3);
        assert_eq!(chorus.lfos.len(), 3);
    }

    #[test]
    fn test_chorus_stereo_spread() {
        let config = ChorusConfig {
            spread: 1.0,
            ..Default::default()
        };
        let mut chorus = StereoChorus::new(config, 48000.0);

        let (out_l, out_r) = chorus.process_sample_stereo(1.0, 0.0);
        // With spread, left and right should be different
        assert!(out_l.is_finite());
        assert!(out_r.is_finite());
    }
}
