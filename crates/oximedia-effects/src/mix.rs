//! Wet/dry mix wrapper for any [`AudioEffect`].
//!
//! [`MixEffect`] wraps an arbitrary effect and blends the processed (wet)
//! signal with the original (dry) signal according to a configurable ratio.
//! This is useful when the underlying effect does not expose its own wet/dry
//! control, or when a uniform mix interface is desired across heterogeneous
//! effect chains.
//!
//! # Example
//!
//! ```
//! use oximedia_effects::{AudioEffect, mix::MixEffect};
//! use oximedia_effects::distortion::fuzz::{Fuzz, FuzzConfig};
//!
//! let fuzz = Fuzz::new(FuzzConfig { fuzz: 10.0, level: 0.8 });
//! let mut mixed = MixEffect::new(fuzz, 0.5);
//! // Output is 50% dry + 50% fuzz-processed
//! let _ = mixed.process_sample(0.4);
//! ```

#![allow(clippy::cast_precision_loss)]

use crate::AudioEffect;

/// Wraps any [`AudioEffect`] with wet/dry mix control.
///
/// * `mix = 0.0` -- fully dry (bypass), output equals input.
/// * `mix = 1.0` -- fully wet, output equals processed signal.
/// * Values between 0 and 1 blend linearly.
pub struct MixEffect<E: AudioEffect> {
    inner: E,
    /// Mix ratio in `[0.0, 1.0]`.
    mix: f32,
}

impl<E: AudioEffect> MixEffect<E> {
    /// Create a new `MixEffect` wrapping `effect` with the given mix ratio.
    ///
    /// The mix is clamped to `[0.0, 1.0]`.
    #[must_use]
    pub fn new(effect: E, mix: f32) -> Self {
        Self {
            inner: effect,
            mix: mix.clamp(0.0, 1.0),
        }
    }

    /// Set the wet/dry mix ratio. Values outside `[0.0, 1.0]` are clamped.
    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    /// Return the current mix ratio.
    #[must_use]
    pub fn mix(&self) -> f32 {
        self.mix
    }

    /// Immutable reference to the wrapped effect.
    #[must_use]
    pub fn inner(&self) -> &E {
        &self.inner
    }

    /// Mutable reference to the wrapped effect.
    pub fn inner_mut(&mut self) -> &mut E {
        &mut self.inner
    }

    /// Consume the wrapper, returning the inner effect.
    #[must_use]
    pub fn into_inner(self) -> E {
        self.inner
    }
}

impl<E: AudioEffect> AudioEffect for MixEffect<E> {
    const EFFECT_ID: u64 = Self.inner.EFFECT_ID;

    fn process_sample(&mut self, input: f32) -> f32 {
        let processed = self.inner.process_sample(input);
        input * (1.0 - self.mix) + processed * self.mix
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (pl, pr) = self.inner.process_sample_stereo(left, right);
        let dry = 1.0 - self.mix;
        (left * dry + pl * self.mix, right * dry + pr * self.mix)
    }

    fn process(&mut self, buffer: &mut [f32]) {
        // We need the original dry signal, so copy first.
        let dry: Vec<f32> = buffer.to_vec();
        self.inner.process(buffer);
        let wet = self.mix;
        let dry_gain = 1.0 - wet;
        for (out, &inp) in buffer.iter_mut().zip(dry.iter()) {
            *out = inp * dry_gain + *out * wet;
        }
    }

    fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let dry_l: Vec<f32> = left.to_vec();
        let dry_r: Vec<f32> = right.to_vec();
        self.inner.process_stereo(left, right);
        let wet = self.mix;
        let dry_gain = 1.0 - wet;
        for (out, &inp) in left.iter_mut().zip(dry_l.iter()) {
            *out = inp * dry_gain + *out * wet;
        }
        for (out, &inp) in right.iter_mut().zip(dry_r.iter()) {
            *out = inp * dry_gain + *out * wet;
        }
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn latency_samples(&self) -> usize {
        self.inner.latency_samples()
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.inner.set_sample_rate(sample_rate);
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.set_mix(wet);
    }

    fn wet_dry(&self) -> f32 {
        self.mix
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::distortion::fuzz::{Fuzz, FuzzConfig};

    /// A trivial effect that doubles the signal -- predictable for testing.
    struct DoubleEffect;

    impl AudioEffect for DoubleEffect {
        const EFFECT_ID: u64 = 6003;
        fn process_sample(&mut self, input: f32) -> f32 {
            input * 2.0
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn test_mix_zero_bypass() {
        let mut m = MixEffect::new(DoubleEffect, 0.0);
        let out = m.process_sample(0.5);
        assert!(
            (out - 0.5).abs() < 1e-6,
            "mix=0 should return dry input, got {out}"
        );
    }

    #[test]
    fn test_mix_one_full_wet() {
        let mut m = MixEffect::new(DoubleEffect, 1.0);
        let out = m.process_sample(0.5);
        // Fully wet: output = processed = 0.5 * 2 = 1.0
        assert!(
            (out - 1.0).abs() < 1e-6,
            "mix=1 should return processed signal, got {out}"
        );
    }

    #[test]
    fn test_mix_half_average() {
        let mut m = MixEffect::new(DoubleEffect, 0.5);
        let out = m.process_sample(0.4);
        // dry = 0.4, wet = 0.8, blend = 0.5*0.4 + 0.5*0.8 = 0.6
        let expected = 0.4 * 0.5 + 0.8 * 0.5;
        assert!(
            (out - expected).abs() < 1e-6,
            "mix=0.5 should average dry and wet, got {out}, expected {expected}"
        );
    }

    #[test]
    fn test_mix_set_get() {
        let mut m = MixEffect::new(DoubleEffect, 0.3);
        assert!((m.mix() - 0.3).abs() < 1e-6);
        m.set_mix(0.7);
        assert!((m.mix() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_mix_clamp() {
        let mut m = MixEffect::new(DoubleEffect, 1.5);
        assert!((m.mix() - 1.0).abs() < 1e-6, "constructor should clamp >1");
        m.set_mix(-0.5);
        assert!((m.mix() - 0.0).abs() < 1e-6, "set_mix should clamp <0");
        m.set_mix(2.0);
        assert!((m.mix() - 1.0).abs() < 1e-6, "set_mix should clamp >1");
    }

    #[test]
    fn test_mix_inner_access() {
        let fuzz_cfg = FuzzConfig {
            fuzz: 5.0,
            level: 0.8,
        };
        let m = MixEffect::new(Fuzz::new(fuzz_cfg), 0.5);
        // inner() should return a reference to the wrapped Fuzz
        let _inner: &Fuzz = m.inner();
    }
}
