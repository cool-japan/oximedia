//! Overdrive distortion - soft clipping.

use crate::AudioEffect;

/// Overdrive configuration.
#[derive(Debug, Clone)]
pub struct OverdriveConfig {
    /// Drive amount (1.0 - 100.0).
    pub drive: f32,
    /// Tone control (0.0 - 1.0, higher = brighter).
    pub tone: f32,
    /// Output level (0.0 - 2.0).
    pub level: f32,
}

impl Default for OverdriveConfig {
    fn default() -> Self {
        Self {
            drive: 5.0,
            tone: 0.5,
            level: 0.5,
        }
    }
}

/// Overdrive effect with soft clipping and wet/dry mix.
pub struct Overdrive {
    config: OverdriveConfig,
    tone_filter: f32,
    /// Wet/dry mix ratio: 0.0 = fully dry, 1.0 = fully wet.
    wet_mix: f32,
}

impl Overdrive {
    /// Create new overdrive effect.
    #[must_use]
    pub fn new(config: OverdriveConfig) -> Self {
        Self {
            config,
            tone_filter: 0.0,
            wet_mix: 1.0,
        }
    }

    /// Soft clipping function (tanh-like).
    fn soft_clip(x: f32) -> f32 {
        if x > 1.0 {
            2.0 / 3.0
        } else if x < -1.0 {
            -2.0 / 3.0
        } else {
            x - (x * x * x) / 3.0
        }
    }
}

impl AudioEffect for Overdrive {

    const EFFECT_ID: u64 = 6010;

    fn process_sample(&mut self, input: f32) -> f32 {
        // Apply drive
        let driven = input * self.config.drive;

        // Soft clip
        let clipped = Self::soft_clip(driven);

        // Apply tone control (simple one-pole lowpass)
        let tone_coeff = 1.0 - self.config.tone;
        self.tone_filter = clipped * (1.0 - tone_coeff) + self.tone_filter * tone_coeff;

        // Apply output level
        let wet_out = self.tone_filter * self.config.level;

        // Wet/dry mix
        wet_out * self.wet_mix + input * (1.0 - self.wet_mix)
    }

    fn reset(&mut self) {
        self.tone_filter = 0.0;
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
    fn test_overdrive() {
        let config = OverdriveConfig::default();
        let mut overdrive = Overdrive::new(config);
        let output = overdrive.process_sample(0.5);
        assert!(output.is_finite());
    }

    #[test]
    fn test_soft_clip() {
        assert!(Overdrive::soft_clip(0.0).abs() < 0.01);
        assert!(Overdrive::soft_clip(0.5).abs() < 1.0);
        assert!(Overdrive::soft_clip(2.0) <= 1.0);
    }

    #[test]
    fn test_overdrive_wet_dry_default_is_one() {
        let od = Overdrive::new(OverdriveConfig::default());
        assert!((od.wet_dry() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_overdrive_set_wet_dry_stores_value() {
        let mut od = Overdrive::new(OverdriveConfig::default());
        od.set_wet_dry(0.4);
        assert!((od.wet_dry() - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_overdrive_dry_only_passes_input() {
        let mut od = Overdrive::new(OverdriveConfig::default());
        od.set_wet_dry(0.0);
        // With wet=0 the output is purely the dry (input) signal.
        let out = od.process_sample(0.3);
        assert!((out - 0.3).abs() < 1e-5, "dry-only output={out}, want 0.3");
    }
}
