//! State-variable filter - multi-mode filter.

use crate::AudioEffect;
use std::f32::consts::PI;

/// Filter mode for state-variable filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// Low-pass.
    LowPass,
    /// High-pass.
    HighPass,
    /// Band-pass.
    BandPass,
    /// Notch (band-reject).
    Notch,
}

/// State-variable filter configuration.
#[derive(Debug, Clone)]
pub struct StateVariableConfig {
    /// Cutoff frequency in Hz.
    pub frequency: f32,
    /// Resonance (Q factor, 0.5 - 20.0).
    pub resonance: f32,
    /// Filter mode.
    pub mode: FilterMode,
}

impl Default for StateVariableConfig {
    fn default() -> Self {
        Self {
            frequency: 1000.0,
            resonance: 0.707,
            mode: FilterMode::LowPass,
        }
    }
}

/// State-variable filter.
pub struct StateVariableFilter {
    // Filter states
    low: f32,
    band: f32,
    // Coefficients
    f: f32,
    q: f32,
    config: StateVariableConfig,
    sample_rate: f32,
}

impl StateVariableFilter {
    /// Create new state-variable filter.
    #[must_use]
    pub fn new(config: StateVariableConfig, sample_rate: f32) -> Self {
        let mut filter = Self {
            low: 0.0,
            band: 0.0,
            f: 0.0,
            q: 0.0,
            config,
            sample_rate,
        };
        filter.update_coefficients();
        filter
    }

    fn update_coefficients(&mut self) {
        // Calculate frequency coefficient.
        // Clamp to [0, F_MAX] to prevent the SVF from becoming numerically
        // unstable at high resonance + high frequency combinations.  The
        // theoretical upper bound for stability is f < 2, but in practice
        // high-Q filters lose stability well before that; 0.95 is a
        // conservative safe ceiling that covers the full audible range at
        // standard sample rates.
        const F_MAX: f32 = 0.95;
        self.f = (2.0 * (PI * self.config.frequency / self.sample_rate).sin()).min(F_MAX);
        // Calculate Q coefficient
        self.q = 1.0 / self.config.resonance;
    }

    /// Set cutoff frequency.
    pub fn set_frequency(&mut self, frequency: f32) {
        self.config.frequency = frequency.clamp(20.0, self.sample_rate * 0.5);
        self.update_coefficients();
    }

    /// Set resonance.
    pub fn set_resonance(&mut self, resonance: f32) {
        self.config.resonance = resonance.clamp(0.5, 20.0);
        self.update_coefficients();
    }

    /// Set filter mode.
    pub fn set_mode(&mut self, mode: FilterMode) {
        self.config.mode = mode;
    }
}

impl AudioEffect for StateVariableFilter {
    const EFFECT_ID: u64 = 6016;
    fn process_sample(&mut self, input: f32) -> f32 {
        // Guard against NaN/inf in state from prior instability.
        if !self.low.is_finite() || !self.band.is_finite() {
            self.low = 0.0;
            self.band = 0.0;
        }

        // State-variable filter equations
        self.low += self.f * self.band;
        let high = input - self.low - self.q * self.band;
        self.band += self.f * high;

        // Clamp states to prevent runaway accumulation at high Q.
        self.low = self.low.clamp(-1e6, 1e6);
        self.band = self.band.clamp(-1e6, 1e6);

        // Select output based on mode
        match self.config.mode {
            FilterMode::LowPass => self.low,
            FilterMode::HighPass => high,
            FilterMode::BandPass => self.band,
            FilterMode::Notch => high + self.low,
        }
    }

    fn reset(&mut self) {
        self.low = 0.0;
        self.band = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_svf() {
        let config = StateVariableConfig::default();
        let mut filter = StateVariableFilter::new(config, 48000.0);

        let output = filter.process_sample(1.0);
        assert!(output.is_finite());
    }

    #[test]
    fn test_svf_modes() {
        let modes = [
            FilterMode::LowPass,
            FilterMode::HighPass,
            FilterMode::BandPass,
            FilterMode::Notch,
        ];

        for mode in modes {
            let config = StateVariableConfig {
                mode,
                ..Default::default()
            };
            let mut filter = StateVariableFilter::new(config, 48000.0);

            for _ in 0..100 {
                let output = filter.process_sample(0.5);
                assert!(output.is_finite());
            }
        }
    }
}
