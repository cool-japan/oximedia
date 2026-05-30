//! Fuzz distortion - hard clipping.

use crate::AudioEffect;

/// Fuzz configuration.
#[derive(Debug, Clone)]
pub struct FuzzConfig {
    /// Fuzz amount (1.0 - 100.0).
    pub fuzz: f32,
    /// Output level (0.0 - 1.0).
    pub level: f32,
}

impl Default for FuzzConfig {
    fn default() -> Self {
        Self {
            fuzz: 10.0,
            level: 0.3,
        }
    }
}

/// Fuzz distortion effect with wet/dry mix.
pub struct Fuzz {
    config: FuzzConfig,
    /// Wet/dry mix ratio: 0.0 = fully dry, 1.0 = fully wet.
    wet_mix: f32,
}

impl Fuzz {
    /// Create new fuzz effect.
    #[must_use]
    pub fn new(config: FuzzConfig) -> Self {
        Self {
            config,
            wet_mix: 1.0,
        }
    }

    /// Hard clipping function.
    fn hard_clip(x: f32) -> f32 {
        x.clamp(-1.0, 1.0)
    }
}

impl AudioEffect for Fuzz {
    const EFFECT_ID: u64 = 6009;

    fn process_sample(&mut self, input: f32) -> f32 {
        let wet_out = Self::hard_clip(input * self.config.fuzz) * self.config.level;
        wet_out * self.wet_mix + input * (1.0 - self.wet_mix)
    }

    fn reset(&mut self) {
        // No state to reset
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.wet_mix = wet.clamp(0.0, 1.0);
    }

    fn wet_dry(&self) -> f32 {
        self.wet_mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzz() {
        let config = FuzzConfig::default();
        let mut fuzz = Fuzz::new(config);
        let output = fuzz.process_sample(0.5);
        assert!(output.is_finite());
        assert!(output.abs() <= 1.0);
    }

    #[test]
    fn test_fuzz_wet_dry_default_is_one() {
        let f = Fuzz::new(FuzzConfig::default());
        assert!((f.wet_dry() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_fuzz_dry_only_passes_input() {
        let mut f = Fuzz::new(FuzzConfig::default());
        f.set_wet_dry(0.0);
        let out = f.process_sample(0.5);
        assert!((out - 0.5).abs() < 1e-5, "dry-only output={out}, want 0.5");
    }
}
