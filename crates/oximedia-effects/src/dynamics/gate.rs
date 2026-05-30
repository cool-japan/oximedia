//! Noise gate with threshold and hysteresis.

use crate::{utils::EnvelopeFollower, AudioEffect};

/// Gate configuration.
#[derive(Debug, Clone)]
pub struct GateConfig {
    /// Threshold in dB.
    pub threshold_db: f32,
    /// Attack time in milliseconds.
    pub attack_ms: f32,
    /// Release time in milliseconds.
    pub release_ms: f32,
    /// Hysteresis range in dB (prevents chattering).
    pub hysteresis_db: f32,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            threshold_db: -40.0,
            attack_ms: 1.0,
            release_ms: 100.0,
            hysteresis_db: 6.0,
        }
    }
}

/// Noise gate effect.
pub struct Gate {
    envelope: EnvelopeFollower,
    config: GateConfig,
    is_open: bool,
    current_gain: f32,
    gain_smoother: f32,
}

impl Gate {
    /// Create new gate.
    #[must_use]
    pub fn new(config: GateConfig, sample_rate: f32) -> Self {
        Self {
            envelope: EnvelopeFollower::new(config.attack_ms, config.release_ms, sample_rate),
            config,
            is_open: false,
            current_gain: 0.0,
            gain_smoother: 0.0,
        }
    }

    /// Convert dB to linear.
    fn db_to_linear(db: f32) -> f32 {
        10.0_f32.powf(db / 20.0)
    }
}

impl AudioEffect for Gate {

    const EFFECT_ID: u64 = 6012;

    fn process_sample(&mut self, input: f32) -> f32 {
        // Detect envelope
        let envelope = self.envelope.process(input);

        // Determine if gate should be open or closed
        let threshold = Self::db_to_linear(self.config.threshold_db);

        let target_gain = if self.is_open {
            // Gate is open, check if we should close
            let close_threshold =
                Self::db_to_linear(self.config.threshold_db - self.config.hysteresis_db);
            if envelope < close_threshold {
                self.is_open = false;
                0.0
            } else {
                1.0
            }
        } else {
            // Gate is closed, check if we should open
            if envelope > threshold {
                self.is_open = true;
                1.0
            } else {
                0.0
            }
        };

        // Smooth gain changes
        let smoothing = 0.9;
        self.gain_smoother = target_gain + smoothing * (self.gain_smoother - target_gain);

        input * self.gain_smoother
    }

    fn reset(&mut self) {
        self.envelope.reset();
        self.is_open = false;
        self.current_gain = 0.0;
        self.gain_smoother = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gate() {
        let config = GateConfig::default();
        let mut gate = Gate::new(config, 48000.0);

        // Process loud signal
        let loud_output = gate.process_sample(0.5);
        assert!(loud_output.is_finite());

        // Process quiet signal
        let quiet_output = gate.process_sample(0.001);
        assert!(quiet_output.is_finite());
    }

    #[test]
    fn test_db_to_linear() {
        assert!((Gate::db_to_linear(0.0) - 1.0).abs() < 0.01);
        assert!(Gate::db_to_linear(-40.0) < 0.1);
    }
}
