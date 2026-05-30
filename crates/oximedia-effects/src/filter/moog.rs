//! Moog ladder filter simulation.

use crate::AudioEffect;

/// Moog ladder filter configuration.
#[derive(Debug, Clone)]
pub struct MoogConfig {
    /// Cutoff frequency in Hz.
    pub frequency: f32,
    /// Resonance (0.0 - 1.0).
    pub resonance: f32,
}

impl Default for MoogConfig {
    fn default() -> Self {
        Self {
            frequency: 1000.0,
            resonance: 0.3,
        }
    }
}

/// Moog ladder filter - classic 4-pole low-pass.
pub struct MoogFilter {
    // Four filter stages
    stage: [f32; 4],
    // Delay elements
    delay: [f32; 4],
    // Coefficients
    cutoff: f32,
    resonance: f32,
    config: MoogConfig,
    sample_rate: f32,
}

impl MoogFilter {
    /// Create new Moog filter.
    #[must_use]
    pub fn new(config: MoogConfig, sample_rate: f32) -> Self {
        let mut filter = Self {
            stage: [0.0; 4],
            delay: [0.0; 4],
            cutoff: 0.0,
            resonance: 0.0,
            config,
            sample_rate,
        };
        filter.update_coefficients();
        filter
    }

    fn update_coefficients(&mut self) {
        // Calculate cutoff coefficient
        let fc = self.config.frequency / (self.sample_rate * 0.5);
        self.cutoff = fc * 1.16;

        // Calculate resonance feedback
        self.resonance = self.config.resonance * 4.0;
    }

    /// Set cutoff frequency.
    pub fn set_frequency(&mut self, frequency: f32) {
        self.config.frequency = frequency.clamp(20.0, self.sample_rate * 0.4);
        self.update_coefficients();
    }

    /// Set resonance.
    pub fn set_resonance(&mut self, resonance: f32) {
        self.config.resonance = resonance.clamp(0.0, 1.0);
        self.update_coefficients();
    }

    fn tanh_approx(x: f32) -> f32 {
        if x < -3.0 {
            -1.0
        } else if x > 3.0 {
            1.0
        } else {
            x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
        }
    }
}

impl AudioEffect for MoogFilter {
    const EFFECT_ID: u64 = 6015;

    fn process_sample(&mut self, input: f32) -> f32 {
        // Add resonance feedback
        let input_with_fb = input - self.resonance * self.stage[3];

        // Process through 4 stages
        for i in 0..4 {
            let stage_input = if i == 0 {
                input_with_fb
            } else {
                self.stage[i - 1]
            };

            self.stage[i] = self.delay[i]
                + self.cutoff * (Self::tanh_approx(stage_input) - Self::tanh_approx(self.delay[i]));
            self.delay[i] = self.stage[i];
        }

        self.stage[3]
    }

    fn reset(&mut self) {
        self.stage.fill(0.0);
        self.delay.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_moog_filter() {
        let config = MoogConfig::default();
        let mut filter = MoogFilter::new(config, 48000.0);

        let output = filter.process_sample(1.0);
        assert!(output.is_finite());
    }

    #[test]
    fn test_moog_resonance() {
        let config = MoogConfig {
            frequency: 1000.0,
            resonance: 0.8,
        };
        let mut filter = MoogFilter::new(config, 48000.0);

        for _ in 0..1000 {
            let output = filter.process_sample(0.5);
            assert!(output.is_finite());
            assert!(output.abs() < 10.0); // Should not explode
        }
    }
}
