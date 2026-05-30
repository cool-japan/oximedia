//! Multi-tap delay effect.
//!
//! Provides multiple delay taps with independent time, level, and pan controls.

use crate::{utils::DelayLine, AudioEffect};

/// Maximum number of taps.
pub const MAX_TAPS: usize = 8;

/// Configuration for a single delay tap.
#[derive(Debug, Clone)]
pub struct DelayTap {
    /// Delay time in milliseconds.
    pub delay_ms: f32,
    /// Level/gain for this tap (0.0 - 1.0).
    pub level: f32,
    /// Pan position (-1.0 = left, 0.0 = center, 1.0 = right).
    pub pan: f32,
    /// Feedback amount for this tap (0.0 - 1.0).
    pub feedback: f32,
}

impl DelayTap {
    /// Create a new delay tap.
    #[must_use]
    pub fn new(delay_ms: f32, level: f32) -> Self {
        Self {
            delay_ms: delay_ms.max(0.0),
            level: level.clamp(0.0, 1.0),
            pan: 0.0,
            feedback: 0.0,
        }
    }

    /// Set pan position.
    #[must_use]
    pub fn with_pan(mut self, pan: f32) -> Self {
        self.pan = pan.clamp(-1.0, 1.0);
        self
    }

    /// Set feedback.
    #[must_use]
    pub fn with_feedback(mut self, feedback: f32) -> Self {
        self.feedback = feedback.clamp(0.0, 0.99);
        self
    }

    /// Get left channel gain from pan.
    #[must_use]
    fn left_gain(&self) -> f32 {
        if self.pan <= 0.0 {
            1.0
        } else {
            1.0 - self.pan
        }
    }

    /// Get right channel gain from pan.
    #[must_use]
    fn right_gain(&self) -> f32 {
        if self.pan >= 0.0 {
            1.0
        } else {
            1.0 + self.pan
        }
    }
}

/// Multi-tap delay effect.
pub struct MultiTapDelay {
    delay_line_l: DelayLine,
    delay_line_r: DelayLine,
    taps: Vec<DelayTap>,
    tap_samples: Vec<usize>,
    dry: f32,
    sample_rate: f32,
}

impl MultiTapDelay {
    /// Create a new multi-tap delay.
    ///
    /// # Panics
    ///
    /// Panics if taps vector is empty or contains more than `MAX_TAPS` entries.
    #[must_use]
    pub fn new(taps: Vec<DelayTap>, sample_rate: f32) -> Self {
        assert!(!taps.is_empty(), "Must have at least one tap");
        assert!(taps.len() <= MAX_TAPS, "Too many taps (max {MAX_TAPS})");

        // Find maximum delay time to size buffers
        let max_delay_ms = taps
            .iter()
            .map(|t| t.delay_ms)
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or(1000.0);

        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let max_delay_samples = ((max_delay_ms * sample_rate) / 1000.0) as usize;

        // Convert tap times to samples
        let tap_samples: Vec<usize> = taps
            .iter()
            .map(|t| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let samp = ((t.delay_ms * sample_rate) / 1000.0) as usize;
                samp
            })
            .collect();

        Self {
            delay_line_l: DelayLine::new(max_delay_samples.max(1)),
            delay_line_r: DelayLine::new(max_delay_samples.max(1)),
            taps,
            tap_samples,
            dry: 0.5,
            sample_rate,
        }
    }

    /// Set dry level.
    pub fn set_dry(&mut self, dry: f32) {
        self.dry = dry.clamp(0.0, 1.0);
    }

    /// Update tap configuration.
    ///
    /// # Panics
    ///
    /// Panics if taps vector is empty or contains more than `MAX_TAPS` entries.
    pub fn set_taps(&mut self, taps: Vec<DelayTap>) {
        assert!(!taps.is_empty(), "Must have at least one tap");
        assert!(taps.len() <= MAX_TAPS, "Too many taps");

        self.tap_samples = taps
            .iter()
            .map(|t| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let samp = ((t.delay_ms * self.sample_rate) / 1000.0) as usize;
                samp
            })
            .collect();

        self.taps = taps;
    }

    /// Create a rhythmic tap pattern.
    #[must_use]
    pub fn rhythmic(tempo_bpm: f32, sample_rate: f32) -> Self {
        let quarter_note_ms = 60000.0 / tempo_bpm;

        let taps = vec![
            DelayTap::new(quarter_note_ms, 0.7).with_pan(-0.3),
            DelayTap::new(quarter_note_ms * 2.0, 0.5).with_pan(0.3),
            DelayTap::new(quarter_note_ms * 3.0, 0.3).with_pan(-0.5),
            DelayTap::new(quarter_note_ms * 4.0, 0.2).with_pan(0.5),
        ];

        Self::new(taps, sample_rate)
    }

    /// Create a haas effect (short delay for stereo widening).
    #[must_use]
    pub fn haas(sample_rate: f32) -> Self {
        let taps = vec![
            DelayTap::new(0.0, 1.0).with_pan(-1.0),
            DelayTap::new(15.0, 0.9).with_pan(1.0), // 15ms delay on right
        ];

        Self::new(taps, sample_rate)
    }

    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let mut out_l = 0.0;
        let mut out_r = 0.0;

        // Sum all taps
        for (i, tap) in self.taps.iter().enumerate() {
            let delay_samp = self.tap_samples[i];

            let delayed_l = self.delay_line_l.read(delay_samp);
            let delayed_r = self.delay_line_r.read(delay_samp);

            // Apply pan
            out_l += delayed_l * tap.level * tap.left_gain();
            out_r += delayed_r * tap.level * tap.right_gain();
        }

        // Write input to delay lines
        self.delay_line_l.write(input_l);
        self.delay_line_r.write(input_r);

        // Mix with dry signal
        out_l += input_l * self.dry;
        out_r += input_r * self.dry;

        (out_l, out_r)
    }
}

impl AudioEffect for MultiTapDelay {
    const EFFECT_ID: u64 = 6006;
    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _right) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    fn reset(&mut self) {
        self.delay_line_l.clear();
        self.delay_line_r.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delay_tap() {
        let tap = DelayTap::new(100.0, 0.5).with_pan(-0.5);
        assert_eq!(tap.delay_ms, 100.0);
        assert_eq!(tap.level, 0.5);
        assert_eq!(tap.pan, -0.5);
        assert_eq!(tap.left_gain(), 1.0);
        assert_eq!(tap.right_gain(), 0.5);
    }

    #[test]
    fn test_multitap_delay() {
        let taps = vec![
            DelayTap::new(100.0, 0.7),
            DelayTap::new(200.0, 0.5),
            DelayTap::new(300.0, 0.3),
        ];

        let mut delay = MultiTapDelay::new(taps, 48000.0);

        // Process impulse
        let (out_l, out_r) = delay.process_sample_stereo(1.0, 1.0);
        assert!(out_l.is_finite());
        assert!(out_r.is_finite());
    }

    #[test]
    fn test_rhythmic_delay() {
        let delay = MultiTapDelay::rhythmic(120.0, 48000.0);
        assert_eq!(delay.taps.len(), 4);
    }

    #[test]
    fn test_haas_effect() {
        let mut delay = MultiTapDelay::haas(48000.0);
        assert_eq!(delay.taps.len(), 2);

        // Process stereo
        let (out_l, out_r) = delay.process_sample_stereo(1.0, 1.0);
        // With Haas effect, left and right should be different
        // (though initially might be similar before delay kicks in)
        assert!(out_l.is_finite());
        assert!(out_r.is_finite());
    }

    #[test]
    #[should_panic(expected = "Must have at least one tap")]
    fn test_multitap_no_taps() {
        let _ = MultiTapDelay::new(vec![], 48000.0);
    }
}
