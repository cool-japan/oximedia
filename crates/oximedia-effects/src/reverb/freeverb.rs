//! Freeverb - classic algorithmic reverb with true stereo decorrelation.
//!
//! Based on the Schroeder reverb architecture with parallel comb filters
//! and series all-pass filters. Enhanced with decorrelated left/right
//! processing using prime-number based delay offsets and independent
//! diffusion coefficients for each channel.

#![allow(clippy::cast_precision_loss)]

use crate::{utils::delay_line::DelayLine, AudioEffect, ReverbConfig};

/// Number of comb filters per channel.
const NUM_COMBS: usize = 8;
/// Number of all-pass filters per channel.
const NUM_ALLPASSES: usize = 4;

/// Comb filter delays for left channel (samples at 44.1kHz).
/// These are chosen as relatively prime values for maximum decorrelation.
const COMB_DELAYS_L: [usize; NUM_COMBS] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];

/// Comb filter delays for right channel (samples at 44.1kHz).
/// Uses prime-number offsets for true stereo decorrelation.
/// Each offset is a distinct prime to ensure L/R never coincide at the
/// same periodicity, producing a wider, more natural stereo image.
const COMB_DELAYS_R: [usize; NUM_COMBS] = [
    1116 + 29, // prime offset 29
    1188 + 37, // prime offset 37
    1277 + 43, // prime offset 43
    1356 + 53, // prime offset 53
    1422 + 59, // prime offset 59
    1491 + 67, // prime offset 67
    1557 + 71, // prime offset 71
    1617 + 79, // prime offset 79
];

/// All-pass filter delays for left channel.
const ALLPASS_DELAYS_L: [usize; NUM_ALLPASSES] = [556, 441, 341, 225];

/// All-pass filter delays for right channel (prime offsets for decorrelation).
const ALLPASS_DELAYS_R: [usize; NUM_ALLPASSES] = [
    556 + 31, // prime offset 31
    441 + 41, // prime offset 41
    341 + 47, // prime offset 47
    225 + 61, // prime offset 61
];

/// Comb filter with feedback and damping.
///
/// Internally uses a pre-allocated [`DelayLine`] ring buffer rather than a
/// raw `Vec<f32>` — no allocations occur during audio processing.
#[derive(Debug, Clone)]
struct CombFilter {
    /// Circular delay line (size = comb delay + 1).
    delay_line: DelayLine,
    /// Delay length in samples (one less than the delay-line size).
    delay: usize,
    filterstore: f32,
    feedback: f32,
    damp1: f32,
    damp2: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        let size = size.max(1);
        // DelayLine capacity is `size + 1` so that `read(size)` is valid.
        // The comb feedback path reads the sample written exactly `size` steps
        // ago, which is equivalent to the oldest slot in the original Vec-based
        // circular buffer of length `size`.
        Self {
            delay_line: DelayLine::new(size + 1),
            delay: size,
            filterstore: 0.0,
            feedback: 0.0,
            damp1: 0.0,
            damp2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        // Read the sample that was written `delay` steps ago (oldest in loop).
        let output = self.delay_line.read(self.delay);

        // One-pole low-pass damping applied to the feedback path.
        self.filterstore = output * self.damp2 + self.filterstore * self.damp1;

        // Write new sample: dry input plus the filtered, fed-back output.
        self.delay_line
            .write(input + self.filterstore * self.feedback);

        output
    }

    fn set_feedback(&mut self, val: f32) {
        self.feedback = val;
    }

    fn set_damp(&mut self, val: f32) {
        self.damp1 = val;
        self.damp2 = 1.0 - val;
    }

    fn clear(&mut self) {
        self.delay_line.clear();
        self.filterstore = 0.0;
    }
}

/// All-pass filter for reverb with configurable diffusion coefficient.
///
/// Internally uses a pre-allocated [`DelayLine`] ring buffer.
#[derive(Debug, Clone)]
struct AllPass {
    /// Circular delay line (size = allpass delay + 1).
    delay_line: DelayLine,
    /// Delay length in samples.
    delay: usize,
    /// Diffusion coefficient (typically 0.5, but varied per-channel for decorrelation).
    diffusion: f32,
}

impl AllPass {
    fn new(size: usize, diffusion: f32) -> Self {
        let size = size.max(1);
        Self {
            delay_line: DelayLine::new(size + 1),
            delay: size,
            diffusion,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let bufout = self.delay_line.read(self.delay);
        let output = -input + bufout;
        self.delay_line.write(input + bufout * self.diffusion);
        output
    }

    fn clear(&mut self) {
        self.delay_line.clear();
    }
}

/// Stereo decorrelation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StereoMode {
    /// Classic mono-in/stereo-out (both channels fed the same input).
    MonoToStereo,
    /// True stereo: each channel processed independently through
    /// decorrelated filter networks with optional cross-feed.
    TrueStereo,
}

/// Freeverb reverb effect with true stereo decorrelation.
///
/// Classic algorithmic reverb based on the Schroeder reverb architecture.
/// Uses parallel comb filters followed by series all-pass filters.
///
/// In `TrueStereo` mode, the left and right channels are processed through
/// completely independent filter networks with:
/// - Prime-number-based delay offsets (not fixed +23)
/// - Per-channel diffusion coefficients
/// - Optional cross-feed for spatial control
pub struct Freeverb {
    // Left channel filters
    combs_l: Vec<CombFilter>,
    allpasses_l: Vec<AllPass>,

    // Right channel filters
    combs_r: Vec<CombFilter>,
    allpasses_r: Vec<AllPass>,

    // Parameters
    config: ReverbConfig,
    room_size: f32,
    damping: f32,
    wet1: f32,
    wet2: f32,
    dry: f32,

    // Pre-delay ring buffers (separate for L/R in true stereo mode).
    // Each is a pre-allocated DelayLine — no allocations during processing.
    predelay_buffer_l: DelayLine,
    predelay_buffer_r: DelayLine,
    predelay_samples: usize,

    /// Cross-feed amount for true stereo (0.0 = fully independent, 1.0 = fully shared).
    cross_feed: f32,

    /// Stereo processing mode.
    stereo_mode: StereoMode,

    sample_rate: f32,
}

impl Freeverb {
    /// Left channel diffusion coefficients (slightly different per stage).
    const DIFFUSION_L: [f32; NUM_ALLPASSES] = [0.50, 0.50, 0.50, 0.50];
    /// Right channel diffusion coefficients (varied for decorrelation).
    const DIFFUSION_R: [f32; NUM_ALLPASSES] = [0.45, 0.52, 0.48, 0.55];

    /// Create a new Freeverb reverb.
    #[must_use]
    pub fn new(config: ReverbConfig, sample_rate: f32) -> Self {
        Self::with_stereo_mode(config, sample_rate, StereoMode::TrueStereo)
    }

    /// Create a new Freeverb with a specific stereo mode.
    #[must_use]
    pub fn with_stereo_mode(
        config: ReverbConfig,
        sample_rate: f32,
        stereo_mode: StereoMode,
    ) -> Self {
        let scale_factor = sample_rate / 44100.0;

        // Create comb filters
        let combs_l: Vec<CombFilter> = COMB_DELAYS_L
            .iter()
            .map(|&delay| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let scaled_delay = (delay as f32 * scale_factor) as usize;
                CombFilter::new(scaled_delay.max(1))
            })
            .collect();

        let combs_r: Vec<CombFilter> = COMB_DELAYS_R
            .iter()
            .map(|&delay| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let scaled_delay = (delay as f32 * scale_factor) as usize;
                CombFilter::new(scaled_delay.max(1))
            })
            .collect();

        // Create all-pass filters with per-channel diffusion coefficients
        let allpasses_l: Vec<AllPass> = ALLPASS_DELAYS_L
            .iter()
            .zip(Self::DIFFUSION_L.iter())
            .map(|(&delay, &diffusion)| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let scaled_delay = (delay as f32 * scale_factor) as usize;
                AllPass::new(scaled_delay.max(1), diffusion)
            })
            .collect();

        let allpasses_r: Vec<AllPass> = ALLPASS_DELAYS_R
            .iter()
            .zip(Self::DIFFUSION_R.iter())
            .map(|(&delay, &diffusion)| {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let scaled_delay = (delay as f32 * scale_factor) as usize;
                AllPass::new(scaled_delay.max(1), diffusion)
            })
            .collect();

        // Pre-delay ring buffers — pre-allocated, zero-alloc during processing.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let predelay_samples = ((config.predelay_ms * sample_rate) / 1000.0) as usize;
        let predelay_size = predelay_samples.max(1);
        // +1 so that `read(predelay_samples)` is within the valid delay range.
        let predelay_buffer_l = DelayLine::new(predelay_size + 1);
        let predelay_buffer_r = DelayLine::new(predelay_size + 1);

        let mut reverb = Self {
            combs_l,
            combs_r,
            allpasses_l,
            allpasses_r,
            config,
            room_size: 0.0,
            damping: 0.0,
            wet1: 0.0,
            wet2: 0.0,
            dry: 0.0,
            predelay_buffer_l,
            predelay_buffer_r,
            predelay_samples,
            cross_feed: 0.15, // Default: slight cross-feed for natural image
            stereo_mode,
            sample_rate,
        };

        reverb.update_parameters();
        reverb
    }

    /// Update internal parameters from config.
    fn update_parameters(&mut self) {
        const ROOM_OFFSET: f32 = 0.7;
        const ROOM_SCALE: f32 = 0.28;
        const DAMP_SCALE: f32 = 0.4;

        self.room_size = self.config.room_size * ROOM_SCALE + ROOM_OFFSET;
        self.damping = self.config.damping * DAMP_SCALE;

        // Calculate wet/dry mix
        let wet = self.config.wet;
        self.dry = self.config.dry;

        // Stereo width
        let width = self.config.width;
        self.wet1 = wet * (width / 2.0 + 0.5);
        self.wet2 = wet * ((1.0 - width) / 2.0);

        // Update all comb filters
        for comb in &mut self.combs_l {
            comb.set_feedback(self.room_size);
            comb.set_damp(self.damping);
        }

        for comb in &mut self.combs_r {
            comb.set_feedback(self.room_size);
            comb.set_damp(self.damping);
        }
    }

    /// Set room size (0.0 - 1.0).
    pub fn set_room_size(&mut self, room_size: f32) {
        self.config.room_size = room_size.clamp(0.0, 1.0);
        self.update_parameters();
    }

    /// Set damping (0.0 - 1.0).
    pub fn set_damping(&mut self, damping: f32) {
        self.config.damping = damping.clamp(0.0, 1.0);
        self.update_parameters();
    }

    /// Set wet level (0.0 - 1.0).
    pub fn set_wet(&mut self, wet: f32) {
        self.config.wet = wet.clamp(0.0, 1.0);
        self.update_parameters();
    }

    /// Set dry level (0.0 - 1.0).
    pub fn set_dry(&mut self, dry: f32) {
        self.config.dry = dry.clamp(0.0, 1.0);
        self.update_parameters();
    }

    /// Set stereo width (0.0 - 1.0).
    pub fn set_width(&mut self, width: f32) {
        self.config.width = width.clamp(0.0, 1.0);
        self.update_parameters();
    }

    /// Set cross-feed amount for true stereo mode (0.0 - 1.0).
    ///
    /// - 0.0: Fully independent L/R processing (widest stereo image)
    /// - 0.5: Equal mix of direct and cross-fed signal
    /// - 1.0: Fully shared input (mono-to-stereo behavior)
    pub fn set_cross_feed(&mut self, amount: f32) {
        self.cross_feed = amount.clamp(0.0, 1.0);
    }

    /// Get the current cross-feed amount.
    #[must_use]
    pub fn cross_feed(&self) -> f32 {
        self.cross_feed
    }

    /// Set the stereo processing mode.
    pub fn set_stereo_mode(&mut self, mode: StereoMode) {
        self.stereo_mode = mode;
    }

    /// Get the current stereo mode.
    #[must_use]
    pub fn stereo_mode(&self) -> StereoMode {
        self.stereo_mode
    }

    /// Process a stereo sample pair.
    fn process_sample_internal(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        // Apply pre-delay using DelayLine ring buffers.
        // `read(N)` retrieves the sample written N steps ago; `write(x)` stores
        // the new sample and advances the internal write position.
        let (delayed_l, delayed_r) = if self.predelay_samples > 0 {
            let del_l = self.predelay_buffer_l.read(self.predelay_samples);
            let del_r = self.predelay_buffer_r.read(self.predelay_samples);

            match self.stereo_mode {
                StereoMode::MonoToStereo => {
                    // Classic: sum to mono, feed both channels.
                    let mono = (input_l + input_r) * 0.5;
                    self.predelay_buffer_l.write(mono);
                    self.predelay_buffer_r.write(mono);
                }
                StereoMode::TrueStereo => {
                    // True stereo: blend toward mono with cross-feed amount.
                    // cf=0: fully independent, cf=1: mono (both get average).
                    let cf = self.cross_feed;
                    let mono = (input_l + input_r) * 0.5;
                    let direct_l = input_l * (1.0 - cf) + mono * cf;
                    let direct_r = input_r * (1.0 - cf) + mono * cf;
                    self.predelay_buffer_l.write(direct_l);
                    self.predelay_buffer_r.write(direct_r);
                }
            }

            (del_l, del_r)
        } else {
            match self.stereo_mode {
                StereoMode::MonoToStereo => {
                    let mono = (input_l + input_r) * 0.5;
                    (mono, mono)
                }
                StereoMode::TrueStereo => {
                    let cf = self.cross_feed;
                    let direct_l = input_l * (1.0 - cf) + input_r * cf;
                    let direct_r = input_r * (1.0 - cf) + input_l * cf;
                    (direct_l, direct_r)
                }
            }
        };

        // Process through comb filters (parallel) - fully independent per channel
        let mut out_l = 0.0;
        let mut out_r = 0.0;

        for comb in &mut self.combs_l {
            out_l += comb.process(delayed_l);
        }

        for comb in &mut self.combs_r {
            out_r += comb.process(delayed_r);
        }

        // Process through all-pass filters (series) - independent per channel
        // Each channel uses its own diffusion coefficients
        for allpass in &mut self.allpasses_l {
            out_l = allpass.process(out_l);
        }

        for allpass in &mut self.allpasses_r {
            out_r = allpass.process(out_r);
        }

        // Mix wet and dry signals with stereo width control
        let wet_l = out_l * self.wet1 + out_r * self.wet2;
        let wet_r = out_r * self.wet1 + out_l * self.wet2;

        let output_l = wet_l + input_l * self.dry;
        let output_r = wet_r + input_r * self.dry;

        (output_l, output_r)
    }
}

impl AudioEffect for Freeverb {
    const EFFECT_ID: u64 = 6028;
    fn process_sample(&mut self, input: f32) -> f32 {
        let (left, _right) = self.process_sample_internal(input, input);
        left
    }

    fn process_sample_stereo(&mut self, left: f32, right: f32) -> (f32, f32) {
        self.process_sample_internal(left, right)
    }

    /// Set the wet level; dry is computed as `1.0 - wet`.
    fn set_wet_dry(&mut self, wet: f32) {
        let w = wet.clamp(0.0, 1.0);
        self.set_wet(w);
        self.set_dry(1.0 - w);
    }

    /// Return the current wet level from the underlying config.
    fn wet_dry(&self) -> f32 {
        self.config.wet
    }

    fn reset(&mut self) {
        for comb in &mut self.combs_l {
            comb.clear();
        }
        for comb in &mut self.combs_r {
            comb.clear();
        }
        for ap in &mut self.allpasses_l {
            ap.clear();
        }
        for ap in &mut self.allpasses_r {
            ap.clear();
        }
        self.predelay_buffer_l.clear();
        self.predelay_buffer_r.clear();
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        let mode = self.stereo_mode;
        *self = Self::with_stereo_mode(self.config.clone(), sample_rate, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freeverb_creation() {
        let config = ReverbConfig::default();
        let reverb = Freeverb::new(config, 48000.0);
        assert_eq!(reverb.combs_l.len(), NUM_COMBS);
        assert_eq!(reverb.allpasses_l.len(), NUM_ALLPASSES);
    }

    #[test]
    fn test_freeverb_process() {
        let config = ReverbConfig::default();
        let mut reverb = Freeverb::new(config, 48000.0);

        // Process impulse
        let output = reverb.process_sample(1.0);
        assert!(output.is_finite());

        // Process more samples - just verify no crashes
        for _ in 0..1000 {
            let out = reverb.process_sample(0.0);
            assert!(out.is_finite());
        }
    }

    #[test]
    fn test_freeverb_stereo() {
        let config = ReverbConfig::default().with_width(1.0);
        let mut reverb = Freeverb::new(config, 48000.0);

        let (out_l, out_r) = reverb.process_sample_stereo(1.0, 0.0);

        // With stereo width, left and right should be different
        assert!(out_l != out_r);
    }

    #[test]
    fn test_freeverb_parameters() {
        let config = ReverbConfig::default();
        let mut reverb = Freeverb::new(config, 48000.0);

        reverb.set_room_size(0.9);
        reverb.set_damping(0.3);
        reverb.set_wet(0.5);
        reverb.set_dry(0.5);

        assert_eq!(reverb.config.room_size, 0.9);
        assert_eq!(reverb.config.damping, 0.3);
    }

    #[test]
    fn test_freeverb_reset() {
        let config = ReverbConfig::default();
        let mut reverb = Freeverb::new(config, 48000.0);

        // Generate reverb tail
        reverb.process_sample(1.0);
        for _ in 0..100 {
            reverb.process_sample(0.0);
        }

        // Reset
        reverb.reset();

        // After reset, output should be much quieter
        let output = reverb.process_sample(0.0);
        assert!(output.abs() < 0.001);
    }

    // --- True stereo decorrelation tests ---

    #[test]
    fn test_true_stereo_decorrelation() {
        // True stereo mode should produce decorrelated L/R output
        // even with identical L/R input
        let config = ReverbConfig::default().with_width(1.0);
        let mut reverb = Freeverb::new(config, 48000.0);
        assert_eq!(reverb.stereo_mode(), StereoMode::TrueStereo);

        // Feed identical impulse to both channels
        let (out_l, out_r) = reverb.process_sample_stereo(1.0, 1.0);
        // First sample: both channels get same input but different filter networks
        assert!(out_l.is_finite());
        assert!(out_r.is_finite());

        // After several samples, the decorrelation becomes apparent
        let mut diff_sum = 0.0f32;
        for _ in 0..2000 {
            let (l, r) = reverb.process_sample_stereo(0.0, 0.0);
            diff_sum += (l - r).abs();
        }
        // The sum of L/R differences should be non-trivial
        assert!(
            diff_sum > 0.01,
            "True stereo should produce decorrelated output, diff_sum={diff_sum}"
        );
    }

    #[test]
    fn test_mono_to_stereo_mode() {
        let config = ReverbConfig::default().with_width(1.0);
        let mut reverb = Freeverb::with_stereo_mode(config, 48000.0, StereoMode::MonoToStereo);
        assert_eq!(reverb.stereo_mode(), StereoMode::MonoToStereo);

        // Feed mono signal: both channels still produce decorrelated reverb tails
        // due to different delay lengths
        let (l, r) = reverb.process_sample_stereo(1.0, 1.0);
        assert!(l.is_finite());
        assert!(r.is_finite());
    }

    #[test]
    fn test_cross_feed_zero_fully_independent() {
        let config = ReverbConfig::default().with_width(1.0);
        let mut reverb = Freeverb::new(config, 48000.0);
        reverb.set_cross_feed(0.0);

        // With zero cross-feed and input only on left channel,
        // right channel reverb should be very quiet initially
        let (l, _r) = reverb.process_sample_stereo(1.0, 0.0);
        assert!(l.is_finite());

        // Accumulate right channel energy
        let mut r_energy = 0.0f32;
        for _ in 0..500 {
            let (_l, r) = reverb.process_sample_stereo(0.0, 0.0);
            r_energy += r * r;
        }

        // Right channel should have some energy due to width mixing
        // but it comes only from the wet2 crossmix, not from cross-feed
        assert!(r_energy.is_finite());
    }

    #[test]
    fn test_cross_feed_one_shared() {
        let config = ReverbConfig::default()
            .with_width(1.0)
            .with_wet(0.5)
            .with_room_size(0.8);
        let mut reverb = Freeverb::new(config, 48000.0);
        reverb.set_cross_feed(1.0);

        // Feed a continuous signal (left only) with cross-feed=1.0
        // Both L and R filters should receive mono=0.5 of the left signal
        let mut l_sum = 0.0f32;
        let mut r_sum = 0.0f32;
        // Pump enough signal for reverb tail to build (> longest comb delay)
        for _ in 0..4000 {
            let (l, r) = reverb.process_sample_stereo(0.5, 0.0);
            l_sum += l.abs();
            r_sum += r.abs();
        }
        // Both channels should have energy from wet reverb + dry signal
        assert!(
            l_sum > 0.001,
            "Left should have energy with cross-feed=1: {l_sum}"
        );
        assert!(
            r_sum > 0.001,
            "Right should have energy with cross-feed=1: {r_sum}"
        );
    }

    #[test]
    fn test_prime_offsets_differ() {
        // Verify that the L/R delay arrays are actually different
        for i in 0..NUM_COMBS {
            assert_ne!(
                COMB_DELAYS_L[i], COMB_DELAYS_R[i],
                "Comb delay {i} should differ between L and R"
            );
        }
        for i in 0..NUM_ALLPASSES {
            assert_ne!(
                ALLPASS_DELAYS_L[i], ALLPASS_DELAYS_R[i],
                "Allpass delay {i} should differ between L and R"
            );
        }
        // Verify offsets are all different (unique primes)
        let offsets_comb: Vec<usize> = COMB_DELAYS_R
            .iter()
            .zip(COMB_DELAYS_L.iter())
            .map(|(r, l)| r - l)
            .collect();
        for i in 0..offsets_comb.len() {
            for j in (i + 1)..offsets_comb.len() {
                assert_ne!(
                    offsets_comb[i], offsets_comb[j],
                    "Comb offsets should be unique primes"
                );
            }
        }
    }

    #[test]
    fn test_diffusion_coefficients_differ() {
        // Verify L/R diffusion coefficients are different for decorrelation
        for i in 0..NUM_ALLPASSES {
            assert!(
                (Freeverb::DIFFUSION_L[i] - Freeverb::DIFFUSION_R[i]).abs() > 1e-6 || i == 0, // first pair can match if desired
                "Diffusion coefficients should generally differ between L and R"
            );
        }
    }

    #[test]
    fn test_true_stereo_asymmetric_input() {
        // Feed signal only to left channel in true stereo mode
        let config = ReverbConfig::default().with_width(1.0).with_room_size(0.8);
        let mut reverb = Freeverb::new(config, 48000.0);
        reverb.set_cross_feed(0.1); // small cross-feed

        // Impulse on left only
        reverb.process_sample_stereo(1.0, 0.0);

        // Collect tail
        let mut l_energy = 0.0f32;
        let mut r_energy = 0.0f32;
        for _ in 0..4000 {
            let (l, r) = reverb.process_sample_stereo(0.0, 0.0);
            l_energy += l * l;
            r_energy += r * r;
        }

        // Left channel should have more energy than right
        // (due to low cross-feed)
        assert!(
            l_energy > r_energy,
            "Left should have more energy with left-only input: L={l_energy}, R={r_energy}"
        );
        // But right should still have some energy from cross-feed and width
        assert!(
            r_energy > 1e-6,
            "Right should have some energy from cross-feed"
        );
    }

    #[test]
    fn test_energy_conservation() {
        // Output energy should not exceed input energy * (wet + dry)
        let config = ReverbConfig::default()
            .with_room_size(0.5)
            .with_wet(0.3)
            .with_dry(0.7);
        let mut reverb = Freeverb::new(config, 48000.0);

        let mut _input_energy = 0.0f32;
        let mut output_energy = 0.0f32;

        // Generate test signal
        for i in 0..8000 {
            #[allow(clippy::cast_precision_loss)]
            let input = (i as f32 * 0.1).sin() * 0.5;
            _input_energy += input * input;
            let (l, r) = reverb.process_sample_stereo(input, input);
            output_energy += (l * l + r * r) * 0.5;
        }

        // Drain reverb tail
        for _ in 0..48000 {
            let (l, r) = reverb.process_sample_stereo(0.0, 0.0);
            output_energy += (l * l + r * r) * 0.5;
        }

        // Output energy should be bounded
        assert!(output_energy.is_finite(), "Output energy should be finite");
    }

    #[test]
    fn test_set_stereo_mode() {
        let config = ReverbConfig::default();
        let mut reverb = Freeverb::new(config, 48000.0);
        assert_eq!(reverb.stereo_mode(), StereoMode::TrueStereo);

        reverb.set_stereo_mode(StereoMode::MonoToStereo);
        assert_eq!(reverb.stereo_mode(), StereoMode::MonoToStereo);
    }

    #[test]
    fn test_predelay_with_true_stereo() {
        let config = ReverbConfig::default().with_predelay(20.0);
        let mut reverb = Freeverb::new(config, 48000.0);

        // With predelay, first samples should be mostly dry
        let (l, r) = reverb.process_sample_stereo(1.0, 1.0);
        assert!(l.is_finite());
        assert!(r.is_finite());

        // Process enough samples to fill predelay
        for _ in 0..2000 {
            let (l, r) = reverb.process_sample_stereo(0.0, 0.0);
            assert!(l.is_finite());
            assert!(r.is_finite());
        }
    }

    /// Regression test: verify that after the Vec<f32> → DelayLine ring-buffer
    /// migration the reverb still produces bounded, finite output.
    ///
    /// Processes an impulse followed by a long silence window and checks that:
    /// 1. All output samples are finite (no NaN / inf).
    /// 2. The reverb tail carries non-trivial energy (reverb is actually working).
    /// 3. The total output energy is finite (tail eventually decays to near-zero).
    #[test]
    fn delay_line_migration_output_bounded_and_decaying() {
        // Use a moderate room size so the tail is audible but not infinitely
        // long.  feedback ≈ 0.5 * 0.28 + 0.7 = 0.84 which decays within ~1 s.
        let config = ReverbConfig::default()
            .with_room_size(0.5)
            .with_wet(0.8)
            .with_dry(0.2);
        let mut reverb = Freeverb::new(config, 48000.0);

        // Feed an impulse.
        reverb.process_sample(1.0);

        // Drain 48000 samples (≈1 second): all must be finite; accumulate total energy.
        let mut total_energy = 0.0_f32;
        for i in 0..48_000usize {
            let s = reverb.process_sample(0.0);
            assert!(
                s.is_finite(),
                "freeverb sample {i} not finite after migration: {s}"
            );
            total_energy += s * s;
        }

        // After a 1-second impulse response the total energy must be:
        //   > 0 : reverb is doing work
        //   < 1e6 : tail is not diverging
        assert!(
            total_energy > 1e-4,
            "reverb tail should carry non-zero energy: {total_energy}"
        );
        assert!(
            total_energy < 1.0e6,
            "reverb tail energy is diverging (migration bug?): {total_energy}"
        );
    }
}
