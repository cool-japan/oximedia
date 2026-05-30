//! Time stretching effect.

use crate::{
    utils::{FractionalDelayLine, InterpolationMode},
    AudioEffect,
};

/// Time stretcher configuration.
#[derive(Debug, Clone)]
pub struct TimeStretchConfig {
    /// Time stretch factor (0.5 = half speed, 2.0 = double speed).
    pub rate: f32,
}

impl Default for TimeStretchConfig {
    fn default() -> Self {
        Self { rate: 1.0 }
    }
}

/// Simple time stretcher using overlap-add.
pub struct TimeStretcher {
    delay: FractionalDelayLine,
    read_pos: f32,
    config: TimeStretchConfig,
}

impl TimeStretcher {
    /// Create new time stretcher.
    #[must_use]
    pub fn new(config: TimeStretchConfig, sample_rate: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let delay_size = (sample_rate as usize) * 2; // 2 second buffer

        Self {
            delay: FractionalDelayLine::new(delay_size, InterpolationMode::Linear),
            read_pos: 0.0,
            config,
        }
    }

    /// Set time stretch rate.
    pub fn set_rate(&mut self, rate: f32) {
        self.config.rate = rate.clamp(0.25, 4.0);
    }
}

impl AudioEffect for TimeStretcher {
    const EFFECT_ID: u64 = 6025;

    fn process_sample(&mut self, input: f32) -> f32 {
        self.delay.write(input);

        let output = self.delay.read(self.read_pos);

        self.read_pos += self.config.rate;
        if self.read_pos >= 10000.0 {
            self.read_pos -= 10000.0;
        }

        output
    }

    fn reset(&mut self) {
        self.delay.clear();
        self.read_pos = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_stretcher() {
        let config = TimeStretchConfig::default();
        let mut stretcher = TimeStretcher::new(config, 48000.0);
        let output = stretcher.process_sample(0.5);
        assert!(output.is_finite());
    }
}
