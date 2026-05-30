//! Bit crusher - bit depth and sample rate reduction.

use crate::AudioEffect;

/// Bit crusher configuration.
#[derive(Debug, Clone)]
pub struct BitCrusherConfig {
    /// Bit depth (1 - 16).
    pub bit_depth: u8,
    /// Sample rate reduction factor (1 = no reduction, higher = more reduction).
    pub downsample: u32,
}

impl Default for BitCrusherConfig {
    fn default() -> Self {
        Self {
            bit_depth: 8,
            downsample: 4,
        }
    }
}

/// Bit crusher effect.
pub struct BitCrusher {
    config: BitCrusherConfig,
    hold_sample: f32,
    sample_counter: u32,
}

impl BitCrusher {
    /// Create new bit crusher.
    #[must_use]
    pub fn new(config: BitCrusherConfig) -> Self {
        Self {
            config,
            hold_sample: 0.0,
            sample_counter: 0,
        }
    }

    /// Set bit depth.
    pub fn set_bit_depth(&mut self, bit_depth: u8) {
        self.config.bit_depth = bit_depth.clamp(1, 16);
    }

    /// Set downsample factor.
    pub fn set_downsample(&mut self, downsample: u32) {
        self.config.downsample = downsample.max(1);
    }
}

impl AudioEffect for BitCrusher {
    const EFFECT_ID: u64 = 6008;

    fn process_sample(&mut self, input: f32) -> f32 {
        // Sample rate reduction
        self.sample_counter += 1;
        if self.sample_counter >= self.config.downsample {
            self.sample_counter = 0;

            // Bit depth reduction
            #[allow(clippy::cast_precision_loss)]
            let levels = (1 << self.config.bit_depth) as f32;
            let step = 2.0 / levels;

            // Quantize
            let quantized = (input / step).round() * step;
            self.hold_sample = quantized.clamp(-1.0, 1.0);
        }

        self.hold_sample
    }

    fn reset(&mut self) {
        self.hold_sample = 0.0;
        self.sample_counter = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bitcrusher() {
        let config = BitCrusherConfig::default();
        let mut crusher = BitCrusher::new(config);

        for _ in 0..100 {
            let output = crusher.process_sample(0.5);
            assert!(output.is_finite());
            assert!(output.abs() <= 1.0);
        }
    }

    #[test]
    fn test_bitcrusher_bit_depth() {
        let config = BitCrusherConfig {
            bit_depth: 2,
            downsample: 1,
        };
        let mut crusher = BitCrusher::new(config);

        // With 2-bit depth, should only have 4 levels
        let output = crusher.process_sample(0.75);
        assert!(output.is_finite());
    }
}
