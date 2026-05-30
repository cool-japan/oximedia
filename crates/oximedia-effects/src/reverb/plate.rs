//! Plate reverb simulation.
//!
//! Simulates the sound of a mechanical plate reverb using a network of
//! all-pass and comb filters with modulated delay lines.

use crate::{
    utils::{DelayLine, FractionalDelayLine, InterpolationMode, Lfo, LfoWaveform},
    AudioEffect, ReverbConfig,
};

/// Plate reverb effect.
///
/// Simulates a mechanical plate reverb using a complex network of filters
/// and modulated delay lines to create a dense, smooth reverb tail.
pub struct PlateReverb {
    // Early reflections (comb filters)
    early_delays_l: Vec<DelayLine>,
    early_delays_r: Vec<DelayLine>,

    // Late reverb (modulated all-pass network)
    diffusion_l: Vec<FractionalDelayLine>,
    diffusion_r: Vec<FractionalDelayLine>,

    // Tank delays for density
    tank_delay_l: DelayLine,
    tank_delay_r: DelayLine,

    // Modulation LFOs
    mod_lfo1: Lfo,
    mod_lfo2: Lfo,

    // One-pole damping filters
    damp_l: f32,
    damp_r: f32,
    damping_coeff: f32,

    // Pre-delay
    predelay_buffer: Vec<f32>,
    predelay_write_pos: usize,
    predelay_samples: usize,

    // Parameters
    config: ReverbConfig,
    #[allow(dead_code)]
    sample_rate: f32,
}

impl PlateReverb {
    /// Create a new plate reverb.
    #[must_use]
    pub fn new(config: ReverbConfig, sample_rate: f32) -> Self {
        // Early reflection delays (in ms)
        let early_times_l = [13.0, 19.0, 29.0, 37.0];
        let early_times_r = [17.0, 23.0, 31.0, 41.0];

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let early_delays_l: Vec<DelayLine> = early_times_l
            .iter()
            .map(|&ms| {
                let samples = ((ms * sample_rate) / 1000.0) as usize;
                DelayLine::new(samples.max(1))
            })
            .collect();

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let early_delays_r: Vec<DelayLine> = early_times_r
            .iter()
            .map(|&ms| {
                let samples = ((ms * sample_rate) / 1000.0) as usize;
                DelayLine::new(samples.max(1))
            })
            .collect();

        // Diffusion all-pass network (in ms)
        let diffusion_times_l = [5.0, 7.0, 11.0, 13.0];
        let diffusion_times_r = [6.0, 8.0, 12.0, 14.0];

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let diffusion_l: Vec<FractionalDelayLine> = diffusion_times_l
            .iter()
            .map(|&ms| {
                let samples = ((ms * sample_rate) / 1000.0) as usize;
                FractionalDelayLine::new(samples.max(1), InterpolationMode::Linear)
            })
            .collect();

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let diffusion_r: Vec<FractionalDelayLine> = diffusion_times_r
            .iter()
            .map(|&ms| {
                let samples = ((ms * sample_rate) / 1000.0) as usize;
                FractionalDelayLine::new(samples.max(1), InterpolationMode::Linear)
            })
            .collect();

        // Tank delays for reverb density
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tank_delay_l = DelayLine::new(((47.0 * sample_rate) / 1000.0) as usize);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tank_delay_r = DelayLine::new(((53.0 * sample_rate) / 1000.0) as usize);

        // Modulation LFOs (very slow for smooth plate sound)
        let mod_lfo1 = Lfo::new(0.3, sample_rate, LfoWaveform::Sine);
        let mod_lfo2 = Lfo::new(0.37, sample_rate, LfoWaveform::Triangle);

        // Pre-delay
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let predelay_samples = ((config.predelay_ms * sample_rate) / 1000.0) as usize;
        let predelay_buffer = vec![0.0; predelay_samples.max(1)];

        let mut plate = Self {
            early_delays_l,
            early_delays_r,
            diffusion_l,
            diffusion_r,
            tank_delay_l,
            tank_delay_r,
            mod_lfo1,
            mod_lfo2,
            damp_l: 0.0,
            damp_r: 0.0,
            damping_coeff: 0.0,
            predelay_buffer,
            predelay_write_pos: 0,
            predelay_samples,
            config,
            sample_rate,
        };

        plate.update_parameters();
        plate
    }

    fn update_parameters(&mut self) {
        // Damping coefficient (higher = more high-frequency absorption)
        self.damping_coeff = self.config.damping * 0.5;
    }

    /// Set damping (0.0 - 1.0).
    pub fn set_damping(&mut self, damping: f32) {
        self.config.damping = damping.clamp(0.0, 1.0);
        self.update_parameters();
    }

    /// Set wet level (0.0 - 1.0).
    pub fn set_wet(&mut self, wet: f32) {
        self.config.wet = wet.clamp(0.0, 1.0);
    }

    /// Set dry level (0.0 - 1.0).
    pub fn set_dry(&mut self, dry: f32) {
        self.config.dry = dry.clamp(0.0, 1.0);
    }

    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        // Apply pre-delay
        let (delayed_l, delayed_r) = if self.predelay_samples > 0 {
            let delayed = self.predelay_buffer[self.predelay_write_pos];
            self.predelay_buffer[self.predelay_write_pos] = (input_l + input_r) * 0.5;
            self.predelay_write_pos = (self.predelay_write_pos + 1) % self.predelay_samples;
            (delayed, delayed)
        } else {
            (input_l, input_r)
        };

        // Early reflections
        let mut early_l = 0.0;
        let mut early_r = 0.0;

        for delay in &mut self.early_delays_l {
            early_l += delay.process(delayed_l, delay.max_delay()) * 0.25;
        }

        for delay in &mut self.early_delays_r {
            early_r += delay.process(delayed_r, delay.max_delay()) * 0.25;
        }

        // Modulation for smooth plate sound
        let mod1 = self.mod_lfo1.next_unipolar() * 2.0; // 0-2 samples modulation
        let mod2 = self.mod_lfo2.next_unipolar() * 2.0;

        // Diffusion network with modulation
        let mut diffused_l = delayed_l + early_l;
        let mut diffused_r = delayed_r + early_r;

        for (i, ap) in self.diffusion_l.iter_mut().enumerate() {
            let delay_time = ap.read(1.0) + if i % 2 == 0 { mod1 } else { mod2 };
            let sample = ap.process(diffused_l, delay_time.max(1.0));
            diffused_l = sample;
        }

        for (i, ap) in self.diffusion_r.iter_mut().enumerate() {
            let delay_time = ap.read(1.0) + if i % 2 == 0 { mod2 } else { mod1 };
            let sample = ap.process(diffused_r, delay_time.max(1.0));
            diffused_r = sample;
        }

        // Tank delays for reverb density
        let feedback_gain = 0.5 + self.config.room_size * 0.45; // 0.5 - 0.95

        let tank_l = self
            .tank_delay_l
            .process(diffused_l, self.tank_delay_l.max_delay());
        let tank_r = self
            .tank_delay_r
            .process(diffused_r, self.tank_delay_r.max_delay());

        // Apply damping (one-pole lowpass)
        self.damp_l = tank_l * (1.0 - self.damping_coeff) + self.damp_l * self.damping_coeff;
        self.damp_r = tank_r * (1.0 - self.damping_coeff) + self.damp_r * self.damping_coeff;

        let wet_l = self.damp_l * feedback_gain;
        let wet_r = self.damp_r * feedback_gain;

        // Mix wet and dry
        let out_l = wet_l * self.config.wet + input_l * self.config.dry;
        let out_r = wet_r * self.config.wet + input_r * self.config.dry;

        (out_l, out_r)
    }
}

impl AudioEffect for PlateReverb {
    const EFFECT_ID: u64 = 6030;
    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _right) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    fn reset(&mut self) {
        for delay in &mut self.early_delays_l {
            delay.clear();
        }
        for delay in &mut self.early_delays_r {
            delay.clear();
        }
        for ap in &mut self.diffusion_l {
            ap.clear();
        }
        for ap in &mut self.diffusion_r {
            ap.clear();
        }
        self.tank_delay_l.clear();
        self.tank_delay_r.clear();
        self.mod_lfo1.reset();
        self.mod_lfo2.reset();
        self.damp_l = 0.0;
        self.damp_r = 0.0;
        self.predelay_buffer.fill(0.0);
        self.predelay_write_pos = 0;
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        *self = Self::new(self.config.clone(), sample_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plate_reverb_creation() {
        let config = ReverbConfig::default();
        let reverb = PlateReverb::new(config, 48000.0);
        assert_eq!(reverb.early_delays_l.len(), 4);
    }

    #[test]
    fn test_plate_reverb_process() {
        let config = ReverbConfig::default();
        let mut reverb = PlateReverb::new(config, 48000.0);

        let output = reverb.process_sample(1.0);
        assert!(output.is_finite());

        // Process more samples - verify no crashes
        for _ in 0..1000 {
            let out = reverb.process_sample(0.0);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_plate_reverb_stereo() {
        let config = ReverbConfig::default();
        let mut reverb = PlateReverb::new(config, 48000.0);

        let (out_l, out_r) = reverb.process_sample_stereo(1.0, 0.0);
        assert!(out_l != 0.0 || out_r != 0.0);
    }
}
