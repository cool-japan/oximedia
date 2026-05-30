//! Expander - dynamics expansion.

use crate::{utils::EnvelopeFollower, AudioEffect};

/// Expander type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpanderType {
    /// Downward expander (reduces quiet signals).
    Downward,
    /// Upward expander (increases loud signals).
    Upward,
}

/// Expander configuration.
#[derive(Debug, Clone)]
pub struct ExpanderConfig {
    /// Threshold in dB.
    pub threshold_db: f32,
    /// Expansion ratio (> 1.0).
    pub ratio: f32,
    /// Attack time in milliseconds.
    pub attack_ms: f32,
    /// Release time in milliseconds.
    pub release_ms: f32,
    /// Expander type.
    pub expander_type: ExpanderType,
}

impl Default for ExpanderConfig {
    fn default() -> Self {
        Self {
            threshold_db: -30.0,
            ratio: 2.0,
            attack_ms: 5.0,
            release_ms: 50.0,
            expander_type: ExpanderType::Downward,
        }
    }
}

/// Expander effect.
pub struct Expander {
    envelope: EnvelopeFollower,
    config: ExpanderConfig,
}

impl Expander {
    /// Create new expander.
    #[must_use]
    pub fn new(config: ExpanderConfig, sample_rate: f32) -> Self {
        Self {
            envelope: EnvelopeFollower::new(config.attack_ms, config.release_ms, sample_rate),
            config,
        }
    }

    fn db_to_linear(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }

    fn linear_to_db(linear: f32) -> f32 {
        20.0 * linear.max(1e-10).log10()
    }

    fn calculate_gain(&self, input_db: f32) -> f32 {
        let threshold = self.config.threshold_db;
        let ratio = self.config.ratio;

        match self.config.expander_type {
            ExpanderType::Downward => {
                if input_db < threshold {
                    // Below threshold: expand downward
                    let diff = threshold - input_db;
                    let expanded_diff = diff * ratio;
                    let gain_db = -expanded_diff + diff;
                    Self::db_to_linear(gain_db)
                } else {
                    1.0
                }
            }
            ExpanderType::Upward => {
                if input_db > threshold {
                    // Above threshold: expand upward
                    let diff = input_db - threshold;
                    let expanded_diff = diff * ratio;
                    let gain_db = expanded_diff - diff;
                    Self::db_to_linear(gain_db)
                } else {
                    1.0
                }
            }
        }
    }
}

impl AudioEffect for Expander {

    const EFFECT_ID: u64 = 6011;

    fn process_sample(&mut self, input: f32) -> f32 {
        let envelope = self.envelope.process(input);
        let input_db = Self::linear_to_db(envelope);
        let gain = self.calculate_gain(input_db);
        input * gain
    }

    fn reset(&mut self) {
        self.envelope.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expander() {
        let config = ExpanderConfig::default();
        let mut expander = Expander::new(config, 48000.0);

        for _ in 0..100 {
            let output = expander.process_sample(0.1);
            assert!(output.is_finite());
        }
    }

    #[test]
    fn test_expander_types() {
        let downward = ExpanderConfig {
            expander_type: ExpanderType::Downward,
            ..Default::default()
        };
        let mut exp_down = Expander::new(downward, 48000.0);

        let upward = ExpanderConfig {
            expander_type: ExpanderType::Upward,
            ..Default::default()
        };
        let mut exp_up = Expander::new(upward, 48000.0);

        let _ = exp_down.process_sample(0.5);
        let _ = exp_up.process_sample(0.5);
    }
}
