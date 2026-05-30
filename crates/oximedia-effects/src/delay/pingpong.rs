//! Ping-pong stereo delay effect.
//!
//! Creates a bouncing echo effect that alternates between left and right channels.

use crate::{utils::DelayLine, AudioEffect};

/// Configuration for ping-pong delay.
#[derive(Debug, Clone)]
pub struct PingPongConfig {
    /// Delay time in milliseconds (per bounce).
    pub delay_ms: f32,
    /// Feedback amount (0.0 - 1.0).
    pub feedback: f32,
    /// Wet signal level (0.0 - 1.0).
    pub wet: f32,
    /// Dry signal level (0.0 - 1.0).
    pub dry: f32,
    /// Stereo width (0.0 = mono, 1.0 = full stereo).
    pub width: f32,
}

impl Default for PingPongConfig {
    fn default() -> Self {
        Self {
            delay_ms: 375.0, // Dotted eighth at 120 BPM
            feedback: 0.5,
            wet: 0.5,
            dry: 0.5,
            width: 1.0,
        }
    }
}

impl PingPongConfig {
    /// Create a new ping-pong delay configuration.
    #[must_use]
    pub fn new(delay_ms: f32, feedback: f32, wet: f32) -> Self {
        Self {
            delay_ms: delay_ms.max(0.0),
            feedback: feedback.clamp(0.0, 0.99),
            wet: wet.clamp(0.0, 1.0),
            dry: (1.0 - wet).clamp(0.0, 1.0),
            width: 1.0,
        }
    }

    /// Set stereo width.
    #[must_use]
    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width.clamp(0.0, 1.0);
        self
    }

    /// Fast ping-pong preset.
    #[must_use]
    pub fn fast() -> Self {
        Self::new(200.0, 0.4, 0.4)
    }

    /// Medium ping-pong preset.
    #[must_use]
    pub fn medium() -> Self {
        Self::new(375.0, 0.5, 0.5)
    }

    /// Slow ambient ping-pong preset.
    #[must_use]
    pub fn slow() -> Self {
        Self::new(600.0, 0.6, 0.6)
    }
}

/// Ping-pong delay effect.
///
/// Creates alternating left-right echoes for a stereo bouncing effect.
pub struct PingPongDelay {
    delay_l: DelayLine,
    delay_r: DelayLine,
    delay_samples: usize,
    config: PingPongConfig,
    sample_rate: f32,
}

impl PingPongDelay {
    /// Create a new ping-pong delay.
    #[must_use]
    pub fn new(config: PingPongConfig, sample_rate: f32) -> Self {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_delay_samples = ((2000.0 * sample_rate) / 1000.0) as usize;

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let delay_samples = ((config.delay_ms * sample_rate) / 1000.0) as usize;

        Self {
            delay_l: DelayLine::new(max_delay_samples),
            delay_r: DelayLine::new(max_delay_samples),
            delay_samples,
            config,
            sample_rate,
        }
    }

    /// Set delay time in milliseconds.
    pub fn set_delay_ms(&mut self, delay_ms: f32) {
        self.config.delay_ms = delay_ms.max(0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let delay_samp = ((delay_ms * self.sample_rate) / 1000.0) as usize;
        self.delay_samples = delay_samp.min(self.delay_l.max_delay());
    }

    /// Set feedback amount.
    pub fn set_feedback(&mut self, feedback: f32) {
        self.config.feedback = feedback.clamp(0.0, 0.99);
    }

    /// Set wet level.
    pub fn set_wet(&mut self, wet: f32) {
        self.config.wet = wet.clamp(0.0, 1.0);
    }

    /// Set dry level.
    pub fn set_dry(&mut self, dry: f32) {
        self.config.dry = dry.clamp(0.0, 1.0);
    }

    /// Set stereo width.
    pub fn set_width(&mut self, width: f32) {
        self.config.width = width.clamp(0.0, 1.0);
    }

    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        // Read delayed samples
        let delayed_l = self.delay_l.read(self.delay_samples);
        let delayed_r = self.delay_r.read(self.delay_samples);

        // Ping-pong: left delay receives right feedback and vice versa
        let feedback = self.config.feedback;
        self.delay_l.write(input_l + delayed_r * feedback);
        self.delay_r.write(input_r + delayed_l * feedback);

        // Apply width control
        let width = self.config.width;
        let mid = (delayed_l + delayed_r) * 0.5;
        let side = (delayed_l - delayed_r) * 0.5 * width;

        let wet_l = mid + side;
        let wet_r = mid - side;

        // Mix wet and dry
        let out_l = wet_l * self.config.wet + input_l * self.config.dry;
        let out_r = wet_r * self.config.wet + input_r * self.config.dry;

        (out_l, out_r)
    }
}

impl AudioEffect for PingPongDelay {

    const EFFECT_ID: u64 = 6007;
    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _right) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    fn reset(&mut self) {
        self.delay_l.clear();
        self.delay_r.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pingpong_config() {
        let config = PingPongConfig::default();
        assert_eq!(config.delay_ms, 375.0);
        assert_eq!(config.feedback, 0.5);
    }

    #[test]
    fn test_pingpong_presets() {
        let fast = PingPongConfig::fast();
        assert!(fast.delay_ms < 300.0);

        let slow = PingPongConfig::slow();
        assert!(slow.delay_ms > 500.0);
    }

    #[test]
    fn test_pingpong_delay() {
        let config = PingPongConfig::default();
        let mut delay = PingPongDelay::new(config, 48000.0);

        // Process impulse on left channel only
        let (out_l, out_r) = delay.process_sample_stereo(1.0, 0.0);

        // Initially should get mostly dry left
        assert!(out_l > out_r);

        // Process more samples
        for _ in 0..1000 {
            delay.process_sample_stereo(0.0, 0.0);
        }

        // After delay time, should have ping-pong effect
        let (echo_l, echo_r) = delay.process_sample_stereo(0.0, 0.0);
        assert!(echo_l.is_finite());
        assert!(echo_r.is_finite());
    }

    #[test]
    fn test_pingpong_width() {
        let config = PingPongConfig::default().with_width(0.0);
        let mut delay = PingPongDelay::new(config, 48000.0);

        // With width=0, should be mono
        delay.process_sample_stereo(1.0, 0.0);

        for _ in 0..20000 {
            delay.process_sample_stereo(0.0, 0.0);
        }

        let (out_l, out_r) = delay.process_sample_stereo(0.0, 0.0);
        // With zero width, left and right should be similar
        assert!((out_l - out_r).abs() < 0.1);
    }

    #[test]
    fn test_pingpong_reset() {
        let config = PingPongConfig::default();
        let mut delay = PingPongDelay::new(config, 48000.0);

        // Fill delay lines
        for _ in 0..1000 {
            delay.process_sample_stereo(1.0, 1.0);
        }

        delay.reset();

        // After reset, should be clean
        let (out_l, out_r) = delay.process_sample_stereo(0.0, 0.0);
        assert!(out_l.abs() < 0.01);
        assert!(out_r.abs() < 0.01);
    }
}
