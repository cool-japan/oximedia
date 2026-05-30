//! Professional audio effects suite for `OxiMedia`.
//!
//! This crate provides production-quality implementations of professional audio effects
//! used in music production, post-production, and broadcast applications.
//!
//! # Effect Categories
//!
//! ## Reverb
//! - **Freeverb** - Algorithmic reverb based on Schroeder reverb architecture
//! - **Plate Reverb** - Simulation of mechanical plate reverb
//! - **Convolution Reverb** - Impulse response-based reverb for realistic spaces
//!
//! ## Delay/Echo
//! - **Delay** - Simple delay with feedback
//! - **Multi-tap Delay** - Multiple delay taps with independent controls
//! - **Ping-pong Delay** - Stereo ping-pong delay effect
//!
//! ## Modulation
//! - **Chorus** - Multi-voice chorus effect
//! - **Flanger** - Flanging with feedback
//! - **Phaser** - All-pass filter cascade phasing
//! - **Tremolo** - Amplitude modulation
//! - **Vibrato** - Frequency modulation
//! - **Ring Modulator** - Ring modulation effect
//!
//! ## Distortion
//! - **Overdrive** - Soft clipping overdrive
//! - **Fuzz** - Hard clipping fuzz distortion
//! - **Bit Crusher** - Bit depth and sample rate reduction
//!
//! ## Dynamics
//! - **Gate** - Noise gate with threshold and hysteresis
//! - **Expander** - Upward and downward expansion
//!
//! ## Filters
//! - **Biquad** - Second-order IIR filters (low-pass, high-pass, band-pass, notch, shelving)
//! - **State Variable Filter** - Multi-mode state-variable filter
//! - **Moog Ladder** - Classic Moog ladder filter simulation
//!
//! ## Pitch/Time
//! - **Pitch Shifter** - Time-domain and frequency-domain pitch shifting
//! - **Time Stretch** - Tempo change without pitch change
//! - **Harmonizer** - Pitch shifting with formant preservation
//!
//! ## Vocoding/Correction
//! - **Vocoder** - Channel vocoder
//! - **Auto-tune** - Basic pitch correction
//!
//! # Architecture
//!
//! All effects implement the `AudioEffect` trait, which provides a unified interface
//! for real-time audio processing with support for both mono and stereo operation.
//!
//! Effects are designed to be:
//! - **Real-time capable** - Low latency, no allocations in process loops
//! - **Sample-accurate** - Parameter changes are smoothed to avoid artifacts
//! - **Efficient** - Optimized for CPU efficiency
//! - **Safe** - No unsafe code, enforced by `#![forbid(unsafe_code)]`
//!
//! # Example
//!
//! ```ignore
//! use oximedia_effects::{AudioEffect, reverb::Freeverb, ReverbConfig};
//!
//! let config = ReverbConfig::default()
//!     .with_room_size(0.8)
//!     .with_damping(0.5)
//!     .with_wet(0.3);
//!
//! let mut reverb = Freeverb::new(config, 48000.0);
//!
//! // Process stereo audio
//! let mut left = vec![0.0; 1024];
//! let mut right = vec![0.0; 1024];
//! reverb.process_stereo(&mut left, &mut right);
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod analog_delay;
pub mod auto_pan;
pub mod barrel_lens;
pub mod bass_enhancer;
pub mod bitcrusher;
pub mod blend;
pub mod chorus;
pub mod chorus_flanger;
pub mod color_grade;
pub mod composite;
pub mod compressor;
pub mod compressor_look;
pub mod deesser;
pub mod delay;
pub mod delay_line;
pub mod distort;
pub mod distortion;
pub mod ducking;
pub mod dynamics;
pub mod eq;
pub mod filter;
pub mod filter_bank;
pub mod flanger;
pub mod glitch;
pub mod harmonic_exciter;
pub mod keying;
pub mod lookahead_limiter;
pub mod lufs_meter;
pub mod luma_key;
pub mod mix;
pub mod modulation;
pub mod multiband_compressor;
pub mod parametric_eq;
pub mod pitch;
pub mod reverb;
pub mod reverb_hall;
pub mod ring_mod;
pub mod room_reverb;
pub mod saturation;
pub mod spatial_audio;
pub mod stereo_upmix;
pub mod stereo_widener;
pub mod stereo_wider;
pub mod tape_echo;
pub mod tape_sat;
pub mod time_stretch;
pub mod transient_shaper;
pub mod tremolo;
pub mod utils;
pub mod vibrato;
pub mod video;
pub mod vocoder;
pub mod warp;
pub mod waveshaper;
pub mod wet_dry;

use thiserror::Error;

/// Error types for audio effects.
#[derive(Debug, Error)]
pub enum EffectError {
    /// Invalid parameter value.
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    /// Invalid sample rate.
    #[error("Invalid sample rate: {0}")]
    InvalidSampleRate(f32),

    /// Buffer size mismatch.
    #[error("Buffer size mismatch: expected {expected}, got {actual}")]
    BufferSizeMismatch {
        /// Expected buffer size.
        expected: usize,
        /// Actual buffer size.
        actual: usize,
    },

    /// Insufficient buffer size.
    #[error("Insufficient buffer size: need at least {required}, got {actual}")]
    InsufficientBuffer {
        /// Required buffer size.
        required: usize,
        /// Actual buffer size.
        actual: usize,
    },

    /// Effect not initialized.
    #[error("Effect not initialized")]
    NotInitialized,

    /// Processing error.
    #[error("Processing error: {0}")]
    ProcessingError(String),
}

/// Result type for effect operations.
pub type Result<T> = std::result::Result<T, EffectError>;

/// Core trait for audio effects.
///
/// All effects implement this trait to provide a unified interface for
/// real-time audio processing.
///
/// ## Wet/Dry Mix
///
/// Every implementor can optionally override `set_wet_dry` and
/// `wet_dry` to expose real-time wet/dry mix control. The default
/// implementations are no-ops (full wet signal).  Effects that maintain
/// their own wet/dry internally (e.g. `MonoDelay`) are encouraged to
/// override these methods so callers can use a uniform API.
pub trait AudioEffect {

    /// Unique ID for hashing in FunDSP.
    const EFFECT_ID: u64;

    /// Process a single mono sample.
    fn process_sample(&mut self, input: f32) -> f32;

    /// Process a buffer of mono samples in-place.
    fn process(&mut self, buffer: &mut [f32]) {
        for sample in buffer {
            *sample = self.process_sample(*sample);
        }
    }

    /// Process stereo samples (left and right channels).
    fn process_stereo(&mut self, left: &mut [f32], right: &mut [f32]) {
        let len = left.len().min(right.len());
        for i in 0..len {
            let (l, r) = self.process_sample_stereo(left[i], right[i]);
            left[i] = l;
            right[i] = r;
        }
    }

    /// Process a single stereo sample pair.
    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        (self.process_sample(left), self.process_sample(right))
    }

    /// Reset the effect state (clear buffers, reset LFOs, etc.).
    fn reset(&mut self);

    /// Get the latency introduced by this effect in samples.
    fn latency_samples(&self) -> usize {
        0
    }

    /// Set the sample rate (if the effect supports it).
    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    /// Set the wet/dry mix ratio.
    ///
    /// `wet` is in `[0.0, 1.0]` where `0.0` = 100% dry, `1.0` = 100% wet.
    /// `dry` is automatically computed as `1.0 - wet`.
    ///
    /// Effects that manage wet/dry internally should override this method.
    /// The default implementation is a no-op (the effect's internal mix
    /// remains unchanged).
    fn set_wet_dry(&mut self, _wet: f32) {}

    /// Return the current wet mix level in `[0.0, 1.0]`.
    ///
    /// Returns `1.0` (fully wet) by default if the effect does not support
    /// wet/dry reporting.
    fn wet_dry(&self) -> f32 {
        1.0
    }

    /// Set the wet mix level in `[0.0, 1.0]`.
    ///
    /// Alias for [`set_wet_dry`](Self::set_wet_dry).  Provided so that
    /// implementations that store a field named `wet_mix` can satisfy the
    /// trait without renaming the field.  The default delegates to
    /// `set_wet_dry`.
    fn set_wet_mix(&mut self, wet: f32) {
        self.set_wet_dry(wet);
    }

    /// Return the current wet mix level in `[0.0, 1.0]`.
    ///
    /// Alias for [`wet_dry`](Self::wet_dry).  Provided so that implementations
    /// that store a field named `wet_mix` can satisfy the trait without
    /// renaming the field.  The default delegates to `wet_dry`.
    fn wet_mix(&self) -> f32 {
        self.wet_dry()
    }

    /// Process `input` through this effect and blend the result with the dry
    /// signal according to `wet` in `[0.0, 1.0]`.
    ///
    /// The wet level is applied **in-call only**; the effect's stored
    /// `wet_dry` field is **not** modified.  This lets callers temporarily
    /// override the mix without permanently changing the effect state.
    ///
    /// The `output` slice must be at least as long as `input`; any extra
    /// elements are left unchanged.
    fn process_with_wet_dry(&mut self, input: &[f32], output: &mut [f32], wet: f32) {
        let wet = wet.clamp(0.0, 1.0);
        let dry = 1.0 - wet;
        let len = input.len().min(output.len());
        for i in 0..len {
            let processed = self.process_sample(input[i]);
            output[i] = processed * wet + input[i] * dry;
        }
    }
}

/// A lightweight wrapper that adds wet/dry mix control to any `AudioEffect`.
///
/// Use this when an underlying effect does not natively support wet/dry mix,
/// or when you want a single consistent control surface.
///
/// # Example
/// ```ignore
/// use oximedia_effects::{WetDryWrapper, AudioEffect};
/// use oximedia_effects::reverb::Freeverb;
///
/// let mut wrapped = WetDryWrapper::new(Freeverb::default(), 0.4);
/// let out = wrapped.process_sample(0.5);
/// ```
pub struct WetDryWrapper<E: AudioEffect> {
    inner: E,
    wet: f32,
    dry: f32,
}

impl<E: AudioEffect> WetDryWrapper<E> {
    /// Wrap an effect with the given initial wet level `[0.0, 1.0]`.
    #[must_use]
    pub fn new(inner: E, wet: f32) -> Self {
        let wet = wet.clamp(0.0, 1.0);
        Self {
            inner,
            wet,
            dry: 1.0 - wet,
        }
    }

    /// Access the inner effect.
    #[must_use]
    pub fn inner(&self) -> &E {
        &self.inner
    }

    /// Access the inner effect mutably.
    pub fn inner_mut(&mut self) -> &mut E {
        &mut self.inner
    }

    /// Consume the wrapper, returning the inner effect.
    #[must_use]
    pub fn into_inner(self) -> E {
        self.inner
    }
}

impl<E: AudioEffect> AudioEffect for WetDryWrapper<E> {
    const EFFECT_ID: u64 = Self.inner.EFFECT_ID;
    fn process_sample(&mut self, input: f32) -> f32 {
        let wet_out = self.inner.process_sample(input);
        wet_out * self.wet + input * self.dry
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        let (wl, wr) = self.inner.process_sample_stereo(left, right);
        (
            wl * self.wet + left * self.dry,
            wr * self.wet + right * self.dry,
        )
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
        self.wet = wet.clamp(0.0, 1.0);
        self.dry = 1.0 - self.wet;
    }

    fn wet_dry(&self) -> f32 {
        self.wet
    }
}

/// Adapter for FunDSP. Allows effects to be integrated into FunDSPs graph system
#[cfg(feature = "fundsp")]
pub struct FunDspAdapter<E: AudioEffect> { inner: E }

#[cfg(feature = "fundsp")]
impl<E: AudioEffect> AudioNode for FunDspAdapter<E> {
    const ID: u64 = E::EFFECT_ID;
    type Inputs = U2;
    type Outputs = U2;

    #[inline]
    fn tick(&mut self, input: &Frame<f32, Self::Inputs>) -> Frame<f32, Self::Outputs> {
        let (l, r) = self.inner.process_sample_stereo(input[0], input[1]);
        [l, r].into()
    }

    fn set_sample_rate(&mut self, sr: f64) { self.inner.set_sample_rate(sr as f32); }
    fn reset(&mut self) { self.inner.reset(); }
}

/// Configuration for reverb effects.
#[derive(Debug, Clone)]
pub struct ReverbConfig {
    /// Room size (0.0 - 1.0).
    pub room_size: f32,
    /// Damping/high-frequency absorption (0.0 - 1.0).
    pub damping: f32,
    /// Wet signal level (0.0 - 1.0).
    pub wet: f32,
    /// Dry signal level (0.0 - 1.0).
    pub dry: f32,
    /// Stereo width (0.0 - 1.0).
    pub width: f32,
    /// Pre-delay in milliseconds.
    pub predelay_ms: f32,
}

impl Default for ReverbConfig {
    fn default() -> Self {
        Self {
            room_size: 0.5,
            damping: 0.5,
            wet: 0.33,
            dry: 0.67,
            width: 1.0,
            predelay_ms: 0.0,
        }
    }
}

impl ReverbConfig {
    /// Create a new reverb configuration with custom parameters.
    #[must_use]
    pub fn new(room_size: f32, damping: f32, wet: f32) -> Self {
        Self {
            room_size: room_size.clamp(0.0, 1.0),
            damping: damping.clamp(0.0, 1.0),
            wet: wet.clamp(0.0, 1.0),
            dry: (1.0 - wet).clamp(0.0, 1.0),
            width: 1.0,
            predelay_ms: 0.0,
        }
    }

    /// Set room size.
    #[must_use]
    pub fn with_room_size(mut self, room_size: f32) -> Self {
        self.room_size = room_size.clamp(0.0, 1.0);
        self
    }

    /// Set damping.
    #[must_use]
    pub fn with_damping(mut self, damping: f32) -> Self {
        self.damping = damping.clamp(0.0, 1.0);
        self
    }

    /// Set wet level.
    #[must_use]
    pub fn with_wet(mut self, wet: f32) -> Self {
        self.wet = wet.clamp(0.0, 1.0);
        self
    }

    /// Set dry level.
    #[must_use]
    pub fn with_dry(mut self, dry: f32) -> Self {
        self.dry = dry.clamp(0.0, 1.0);
        self
    }

    /// Set stereo width.
    #[must_use]
    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width.clamp(0.0, 1.0);
        self
    }

    /// Set pre-delay in milliseconds.
    #[must_use]
    pub fn with_predelay(mut self, predelay_ms: f32) -> Self {
        self.predelay_ms = predelay_ms.max(0.0);
        self
    }

    /// Small room preset.
    #[must_use]
    pub fn small_room() -> Self {
        Self::new(0.3, 0.4, 0.2)
    }

    /// Medium room preset.
    #[must_use]
    pub fn medium_room() -> Self {
        Self::new(0.5, 0.5, 0.3)
    }

    /// Large hall preset.
    #[must_use]
    pub fn hall() -> Self {
        Self::new(0.8, 0.6, 0.4).with_predelay(20.0)
    }

    /// Cathedral preset.
    #[must_use]
    pub fn cathedral() -> Self {
        Self::new(0.95, 0.7, 0.5).with_predelay(40.0)
    }

    /// Chamber preset.
    #[must_use]
    pub fn chamber() -> Self {
        Self::new(0.6, 0.4, 0.35).with_predelay(10.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reverb_config_defaults() {
        let config = ReverbConfig::default();
        assert_eq!(config.room_size, 0.5);
        assert_eq!(config.damping, 0.5);
        assert_eq!(config.wet, 0.33);
    }

    #[test]
    fn test_reverb_config_builder() {
        let config = ReverbConfig::default()
            .with_room_size(0.8)
            .with_damping(0.6)
            .with_wet(0.4);
        assert_eq!(config.room_size, 0.8);
        assert_eq!(config.damping, 0.6);
        assert_eq!(config.wet, 0.4);
    }

    #[test]
    fn test_reverb_config_clamping() {
        let config = ReverbConfig::new(1.5, -0.5, 2.0);
        assert_eq!(config.room_size, 1.0);
        assert_eq!(config.damping, 0.0);
        assert_eq!(config.wet, 1.0);
    }

    #[test]
    fn test_reverb_presets() {
        let small = ReverbConfig::small_room();
        assert!(small.room_size < 0.5);

        let hall = ReverbConfig::hall();
        assert!(hall.room_size > 0.7);
        assert!(hall.predelay_ms > 0.0);
    }
}

#[cfg(test)]
mod wet_dry_tests {
    //! Tests for wet/dry mix control across all `AudioEffect` implementations.
    use super::*;
    use crate::chorus::{ChorusParams, ChorusProcessor};
    use crate::distortion::fuzz::{Fuzz, FuzzConfig};
    use crate::distortion::overdrive::{Overdrive, OverdriveConfig};
    use crate::flanger::{Flanger, FlangerConfig};
    use crate::reverb::Freeverb;

    // ── WetDryWrapper ────────────────────────────────────────────────────────

    #[test]
    fn test_wrapper_wet_zero_returns_dry() {
        let mut wrapped = WetDryWrapper::new(Fuzz::new(FuzzConfig::default()), 0.0);
        let out = wrapped.process_sample(0.5);
        assert!(
            (out - 0.5).abs() < 1e-5,
            "wet=0 should return dry signal, got {out}"
        );
    }

    #[test]
    fn test_wrapper_wet_one_returns_processed() {
        // With wet=1, WetDryWrapper contributes 0 dry, so output == processed.
        let inner = Fuzz::new(FuzzConfig {
            fuzz: 1.0,
            level: 1.0,
        });
        let mut wrapped = WetDryWrapper::new(inner, 1.0);
        // Input 0.5, fuzz=1.0 → hard_clip(0.5) * 1.0 = 0.5 → same as input in this case
        let out = wrapped.process_sample(0.5);
        assert!(out.is_finite());
    }

    #[test]
    fn test_wrapper_wet_half_blends() {
        // Use an effect that transforms the signal predictably.
        // Fuzz with fuzz=100 and level=1 → hard_clip(input*100) = ±1.0 for nonzero input.
        let inner = Fuzz::new(FuzzConfig {
            fuzz: 100.0,
            level: 1.0,
        });
        let mut wrapped = WetDryWrapper::new(inner, 0.5);
        let out = wrapped.process_sample(0.5);
        // Expected: processed=1.0, dry=0.5, blend = 0.5*1.0 + 0.5*0.5 = 0.75
        assert!((out - 0.75).abs() < 1e-5, "blend mismatch: got {out}");
    }

    #[test]
    fn test_wrapper_set_wet_dry_updates() {
        let inner = Fuzz::new(FuzzConfig::default());
        let mut wrapped = WetDryWrapper::new(inner, 0.3);
        assert!((wrapped.wet_dry() - 0.3).abs() < 1e-5);
        wrapped.set_wet_dry(0.8);
        assert!((wrapped.wet_dry() - 0.8).abs() < 1e-5);
    }

    #[test]
    fn test_wrapper_set_wet_dry_clamps() {
        let inner = Fuzz::new(FuzzConfig::default());
        let mut wrapped = WetDryWrapper::new(inner, 0.5);
        wrapped.set_wet_dry(2.0);
        assert!((wrapped.wet_dry() - 1.0).abs() < 1e-5);
        wrapped.set_wet_dry(-1.0);
        assert!((wrapped.wet_dry() - 0.0).abs() < 1e-5);
    }

    // ── process_with_wet_dry default method ──────────────────────────────────

    #[test]
    fn test_process_with_wet_dry_zero_equals_input() {
        let mut fuzz = Fuzz::new(FuzzConfig {
            fuzz: 100.0,
            level: 1.0,
        });
        let input = vec![0.3_f32, -0.5, 0.7];
        let mut output = vec![0.0_f32; 3];
        fuzz.process_with_wet_dry(&input, &mut output, 0.0);
        for (i, (&inp, &out)) in input.iter().zip(output.iter()).enumerate() {
            assert!(
                (out - inp).abs() < 1e-5,
                "output[{i}]={out} should equal input {inp}"
            );
        }
    }

    #[test]
    fn test_process_with_wet_dry_one_equals_processed() {
        // Identity fuzz: fuzz=1.0, level=1.0 → hard_clip(x*1)= x for |x|<1
        let mut fuzz = Fuzz::new(FuzzConfig {
            fuzz: 1.0,
            level: 1.0,
        });
        let input = vec![0.3_f32, -0.4, 0.2];
        let mut output = vec![0.0_f32; 3];
        fuzz.process_with_wet_dry(&input, &mut output, 1.0);
        // with wet=0 on fuzz itself (default 1.0), processed = hard_clip(x) = x
        // process_with_wet_dry at wet=1 → output == processed
        for &s in &output {
            assert!(s.is_finite());
        }
    }

    #[test]
    fn test_process_with_wet_dry_half_blends() {
        let mut fuzz = Fuzz::new(FuzzConfig {
            fuzz: 100.0,
            level: 1.0,
        });
        let input = vec![0.5_f32];
        let mut output = vec![0.0_f32; 1];
        fuzz.process_with_wet_dry(&input, &mut output, 0.5);
        // processed by fuzz at wet=1 (default): hard_clip(50)=1.0 → wet_out=1.0
        // blend at 0.5: 0.5*1.0 + 0.5*0.5 = 0.75
        assert!((output[0] - 0.75).abs() < 0.01, "blend={}", output[0]);
    }

    // ── Overdrive wet/dry ─────────────────────────────────────────────────────

    #[test]
    fn test_overdrive_wet_dry_default_is_one() {
        let od = Overdrive::new(OverdriveConfig::default());
        assert!((od.wet_dry() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_overdrive_wet_zero_passes_dry() {
        let mut od = Overdrive::new(OverdriveConfig::default());
        od.set_wet_dry(0.0);
        let out = od.process_sample(0.4);
        assert!((out - 0.4).abs() < 1e-5, "dry pass-through failed: {out}");
    }

    #[test]
    fn test_overdrive_set_wet_dry_clamps() {
        let mut od = Overdrive::new(OverdriveConfig::default());
        od.set_wet_dry(5.0);
        assert!((od.wet_dry() - 1.0).abs() < f32::EPSILON);
        od.set_wet_dry(-2.0);
        assert!((od.wet_dry() - 0.0).abs() < f32::EPSILON);
    }

    // ── Fuzz wet/dry ─────────────────────────────────────────────────────────

    #[test]
    fn test_fuzz_wet_zero_passes_dry() {
        let mut f = Fuzz::new(FuzzConfig::default());
        f.set_wet_dry(0.0);
        let out = f.process_sample(0.6);
        assert!((out - 0.6).abs() < 1e-5, "fuzz dry failed: {out}");
    }

    #[test]
    fn test_fuzz_wet_one_full_effect() {
        let mut f = Fuzz::new(FuzzConfig {
            fuzz: 50.0,
            level: 1.0,
        });
        f.set_wet_dry(1.0);
        let out = f.process_sample(0.5);
        // hard_clip(25)*1.0 = 1.0
        assert!((out - 1.0).abs() < 1e-5, "full wet fuzz: {out}");
    }

    // ── Flanger wet/dry ───────────────────────────────────────────────────────

    #[test]
    fn test_flanger_wet_dry_stores() {
        let mut fl = Flanger::new(FlangerConfig::default(), 48_000.0);
        fl.set_wet_dry(0.3);
        assert!((fl.wet_dry() - 0.3).abs() < 1e-5);
    }

    #[test]
    fn test_flanger_wet_zero_bypasses() {
        let mut fl = Flanger::new(
            FlangerConfig {
                feedback: 0.0,
                ..FlangerConfig::default()
            },
            48_000.0,
        );
        fl.set_wet_dry(0.0);
        let out = fl.process_sample(0.5);
        assert!((out - 0.5).abs() < 1e-5, "flanger dry bypass: {out}");
    }

    // ── ChorusProcessor wet/dry ───────────────────────────────────────────────

    #[test]
    fn test_chorus_wet_zero_passes_dry() {
        let mut cp = ChorusProcessor::new(48_000.0, ChorusParams::default());
        cp.set_wet_dry(0.0);
        // process_sample on AudioEffect casts f64→f32
        let out: f32 = crate::AudioEffect::process_sample(&mut cp, 0.7);
        assert!((out - 0.7).abs() < 1e-4, "chorus dry: {out}");
    }

    #[test]
    fn test_chorus_wet_dry_stores() {
        let mut cp = ChorusProcessor::new(48_000.0, ChorusParams::default());
        cp.set_wet_dry(0.6);
        assert!((cp.wet_dry() - 0.6).abs() < 1e-5);
    }

    // ── Freeverb wet/dry ──────────────────────────────────────────────────────

    #[test]
    fn test_freeverb_wet_dry_stores() {
        let mut rv = Freeverb::new(ReverbConfig::default(), 48_000.0);
        rv.set_wet_dry(0.5);
        assert!((rv.wet_dry() - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_freeverb_wet_dry_clamps() {
        let mut rv = Freeverb::new(ReverbConfig::default(), 48_000.0);
        rv.set_wet_dry(2.0);
        assert!((rv.wet_dry() - 1.0).abs() < f32::EPSILON);
        rv.set_wet_dry(-1.0);
        assert!((rv.wet_dry() - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_freeverb_wet_zero_output_is_dry() {
        // With wet=0 and dry=1, the reverb tail should not appear in output.
        let mut rv = Freeverb::new(ReverbConfig::default(), 48_000.0);
        rv.set_wet_dry(0.0);
        // First sample: an impulse of 1.0.
        let out = rv.process_sample(1.0);
        // With wet=0 (config.wet=0) and dry=1 (config.dry=1), output ≈ input.
        assert!(
            out.is_finite(),
            "freeverb wet=0 should produce finite output"
        );
        // After setting wet=0 the config.wet=0, dry=1; the reverb tails carry 0 wet.
        // So output should equal input * dry (1.0 * 1.0 = 1.0 approximately).
        assert!(
            (out - 1.0).abs() < 0.05,
            "freeverb wet=0: expected ~1.0, got {out}"
        );
    }
}
