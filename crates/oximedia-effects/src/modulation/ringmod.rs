//! Ring modulation effect.

use crate::{
    utils::{Lfo, LfoWaveform},
    AudioEffect,
};
use std::f32::consts::TAU;

/// Ring modulator configuration.
#[derive(Debug, Clone)]
pub struct RingModConfig {
    /// Carrier frequency in Hz.
    pub frequency: f32,
    /// Modulation depth (0.0 - 1.0).
    pub depth: f32,
    /// Wet/dry mix (0.0 - 1.0).
    pub mix: f32,
    /// Use LFO for carrier (if false, uses sine oscillator).
    pub use_lfo: bool,
    /// LFO waveform (if `use_lfo` is true).
    pub waveform: LfoWaveform,
}

impl Default for RingModConfig {
    fn default() -> Self {
        Self {
            frequency: 100.0,
            depth: 1.0,
            mix: 0.5,
            use_lfo: false,
            waveform: LfoWaveform::Sine,
        }
    }
}

/// Ring modulator effect.
pub struct RingModulator {
    lfo: Option<Lfo>,
    phase: f32,
    phase_inc: f32,
    config: RingModConfig,
    sample_rate: f32,
}

impl RingModulator {
    /// Create new ring modulator.
    #[must_use]
    pub fn new(config: RingModConfig, sample_rate: f32) -> Self {
        let lfo = if config.use_lfo {
            Some(Lfo::new(config.frequency, sample_rate, config.waveform))
        } else {
            None
        };

        let phase_inc = config.frequency / sample_rate;

        Self {
            lfo,
            phase: 0.0,
            phase_inc,
            config,
            sample_rate,
        }
    }

    /// Set carrier frequency.
    pub fn set_frequency(&mut self, frequency: f32) {
        self.config.frequency = frequency.max(0.0);
        self.phase_inc = frequency / self.sample_rate;
        if let Some(lfo) = &mut self.lfo {
            lfo.set_frequency(frequency);
        }
    }

    fn get_carrier(&mut self) -> f32 {
        if let Some(lfo) = &mut self.lfo {
            lfo.next()
        } else {
            let carrier = (self.phase * TAU).sin();
            self.phase += self.phase_inc;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            carrier
        }
    }
}

impl AudioEffect for RingModulator {
    const EFFECT_ID: u64 = 6018;

    fn process_sample(&mut self, input: f32) -> f32 {
        let carrier = self.get_carrier();
        let modulated = input * carrier * self.config.depth;
        modulated * self.config.mix + input * (1.0 - self.config.mix)
    }

    fn reset(&mut self) {
        self.phase = 0.0;
        if let Some(lfo) = &mut self.lfo {
            lfo.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_modulator() {
        let config = RingModConfig::default();
        let mut ringmod = RingModulator::new(config, 48000.0);
        let output = ringmod.process_sample(1.0);
        assert!(output.is_finite());
    }
}
