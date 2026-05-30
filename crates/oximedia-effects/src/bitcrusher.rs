//! Bitcrusher audio effect — bit-depth and sample-rate reduction with triangular dither.
//!
//! The bitcrusher degrades audio quality in two ways:
//!
//! 1. **Bit-depth reduction** — quantises each sample to a coarser grid, producing
//!    the characteristic lo-fi, digital crunch of early samplers and game consoles.
//! 2. **Sample-rate reduction** — holds each output sample for `sample_rate_reduction`
//!    input samples, creating aliasing artefacts similar to a low-rate ADC/DAC.
//!
//! Optionally, **triangular probability density function (TPDF) dither** can be added
//! before quantisation to break up harmonic distortion patterns at very low bit depths.
//!
//! # Signal flow
//!
//! ```text
//! input ──► [maybe add TPDF dither] ──► [quantise to N bits] ──► [S&H] ──► wet/dry mix ──► output
//! ```
//!
//! # Example
//!
//! ```rust
//! use oximedia_effects::bitcrusher::{Bitcrusher, BitcrusherConfig};
//! use oximedia_effects::AudioEffect;
//!
//! let config = BitcrusherConfig {
//!     bit_depth: 8,
//!     sample_rate_reduction: 4,
//!     dither: true,
//!     wet_dry: 1.0,
//! };
//! let mut fx = Bitcrusher::new(config);
//! let out = fx.process_sample(0.5);
//! assert!(out.is_finite());
//! ```

use crate::AudioEffect;

// ─── Configuration ────────────────────────────────────────────────────────────

/// Configuration for the [`Bitcrusher`] effect.
#[derive(Debug, Clone)]
pub struct BitcrusherConfig {
    /// Bit depth for quantisation (1 – 24).
    ///
    /// Lower values produce a more aggressive lo-fi character.
    /// At 24 bits the quantisation error is inaudible on normal material.
    pub bit_depth: u8,

    /// Sample-rate reduction factor.
    ///
    /// `1` = no reduction (every input sample is quantised independently).
    /// `2` = half-rate (output is held for 2 input samples).
    /// `4` = quarter-rate, and so on.
    pub sample_rate_reduction: u32,

    /// Whether to add TPDF dither before quantisation.
    ///
    /// Dithering replaces correlated harmonic distortion with benign white
    /// noise, which is perceptually less objectionable at low bit depths.
    pub dither: bool,

    /// Wet / dry mix ratio.
    ///
    /// `0.0` = fully dry (bypass), `1.0` = fully wet (processed signal only).
    pub wet_dry: f32,
}

impl Default for BitcrusherConfig {
    fn default() -> Self {
        Self {
            bit_depth: 8,
            sample_rate_reduction: 1,
            dither: false,
            wet_dry: 1.0,
        }
    }
}

// ─── Processor ────────────────────────────────────────────────────────────────

/// Bitcrusher effect processor.
///
/// See the [module-level documentation](self) for a full description of the
/// signal flow and configuration options.
#[derive(Debug)]
pub struct Bitcrusher {
    config: BitcrusherConfig,
    /// The sample value currently being held by the sample-and-hold stage.
    sample_hold: f32,
    /// Number of input samples remaining before the next hold value is computed.
    ///
    /// When this reaches zero the next input sample is quantised and stored.
    hold_counter: u32,
    /// 64-bit state for the splitmix64 PRNG used to generate dither noise.
    dither_state: u64,
}

impl Bitcrusher {
    /// Create a new [`Bitcrusher`] from the supplied configuration.
    ///
    /// The bit depth is clamped to `[1, 24]` and the sample-rate reduction
    /// factor is clamped to `[1, u32::MAX]`.  The wet/dry ratio is clamped
    /// to `[0.0, 1.0]`.
    #[must_use]
    pub fn new(config: BitcrusherConfig) -> Self {
        let config = BitcrusherConfig {
            bit_depth: config.bit_depth.clamp(1, 24),
            sample_rate_reduction: config.sample_rate_reduction.max(1),
            dither: config.dither,
            wet_dry: config.wet_dry.clamp(0.0, 1.0),
        };
        Self {
            config,
            sample_hold: 0.0,
            hold_counter: 0,
            dither_state: 0x9e37_79b9_7f4a_7c15_u64,
        }
    }

    /// Update the bit depth (clamped to `[1, 24]`).
    pub fn set_bit_depth(&mut self, bit_depth: u8) {
        self.config.bit_depth = bit_depth.clamp(1, 24);
    }

    /// Update the sample-rate reduction factor (minimum 1).
    pub fn set_sample_rate_reduction(&mut self, factor: u32) {
        self.config.sample_rate_reduction = factor.max(1);
    }

    /// Enable or disable TPDF dither.
    pub fn set_dither(&mut self, dither: bool) {
        self.config.dither = dither;
    }

    /// Return the current bit depth.
    #[must_use]
    pub fn bit_depth(&self) -> u8 {
        self.config.bit_depth
    }

    /// Return the current sample-rate reduction factor.
    #[must_use]
    pub fn sample_rate_reduction(&self) -> u32 {
        self.config.sample_rate_reduction
    }

    // ── Internals ──────────────────────────────────────────────────────────

    /// Quantise `x` to `config.bit_depth` bits.
    ///
    /// The full-scale range is `[-1.0, +1.0]`.  With `N` bits there are
    /// `2^N` quantisation steps; each step has width `2 / 2^N = 1 / 2^(N-1)`.
    ///
    /// Formula: `round(x * half_steps) / half_steps`
    /// where `half_steps = 2^(N-1)`.
    fn quantize(&self, x: f32) -> f32 {
        // Use f64 arithmetic internally so that large bit depths (e.g. 24)
        // don't accumulate rounding errors.
        let bit_depth = u32::from(self.config.bit_depth);
        // half_steps = 2^(N-1); using pow to avoid integer overflow for N=24.
        let half_steps = (1u32 << (bit_depth - 1)) as f64;
        let quantised = ((x as f64) * half_steps).round() / half_steps;
        // Clamp to prevent floating-point rounding from exceeding [-1, +1].
        quantised.clamp(-1.0, 1.0) as f32
    }

    /// Generate a single TPDF dither sample in `(-1.0/half_steps, +1.0/half_steps)`.
    ///
    /// Two independent uniform random variables are drawn using the splitmix64
    /// PRNG and their sum is halved, producing a triangular distribution centred
    /// on zero with peak-to-peak amplitude equal to one quantisation step.
    fn next_dither(&mut self) -> f32 {
        let u1 = self.splitmix64();
        let u2 = self.splitmix64();

        // Map each 64-bit value to [-1, +1] and average them → triangular PDF.
        let f1 = (u1 as f64) / (u64::MAX as f64) * 2.0 - 1.0;
        let f2 = (u2 as f64) / (u64::MAX as f64) * 2.0 - 1.0;
        let tpdf = ((f1 + f2) * 0.5) as f32;

        // Scale dither amplitude to one quantisation step.
        let bit_depth = u32::from(self.config.bit_depth);
        let half_steps = (1u32 << (bit_depth - 1)) as f32;
        tpdf / half_steps
    }

    /// Advance the splitmix64 PRNG and return a pseudo-random 64-bit value.
    ///
    /// splitmix64 by Sebastiano Vigna — public domain.
    fn splitmix64(&mut self) -> u64 {
        self.dither_state = self.dither_state.wrapping_add(0x9e37_79b9_7f4a_7c15_u64);
        let mut z = self.dither_state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9_u64);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb_u64);
        z ^ (z >> 31)
    }
}

// ─── AudioEffect implementation ───────────────────────────────────────────────

impl AudioEffect for Bitcrusher {

    const EFFECT_ID: u64 = 6001;

    /// Process a single mono sample through the bitcrusher.
    ///
    /// ## Algorithm
    ///
    /// 1. If `hold_counter > 0` the current held value is returned immediately
    ///    and `hold_counter` is decremented (sample-and-hold in hold phase).
    /// 2. Otherwise a new quantised value is computed:
    ///    a. Optionally add TPDF dither.
    ///    b. Quantise to `bit_depth` bits.
    ///    c. Store the result in `sample_hold` and reset `hold_counter` to
    ///       `sample_rate_reduction - 1`.
    /// 3. The wet/dry mix is applied: `dry * input + wet * held_sample`.
    fn process_sample(&mut self, input: f32) -> f32 {
        if self.hold_counter > 0 {
            self.hold_counter -= 1;
        } else {
            // Compute a fresh quantised value.
            let x = if self.config.dither {
                let d = self.next_dither();
                (input + d).clamp(-1.0, 1.0)
            } else {
                input
            };

            self.sample_hold = self.quantize(x);
            // Hold for (sample_rate_reduction - 1) additional samples.
            self.hold_counter = self.config.sample_rate_reduction.saturating_sub(1);
        }

        let wet = self.config.wet_dry;
        let dry = 1.0 - wet;
        dry * input + wet * self.sample_hold
    }

    fn reset(&mut self) {
        self.sample_hold = 0.0;
        self.hold_counter = 0;
    }

    fn set_wet_dry(&mut self, wet: f32) {
        self.config.wet_dry = wet.clamp(0.0, 1.0);
    }

    fn wet_dry(&self) -> f32 {
        self.config.wet_dry
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_crusher(bit_depth: u8, reduction: u32, dither: bool, wet: f32) -> Bitcrusher {
        Bitcrusher::new(BitcrusherConfig {
            bit_depth,
            sample_rate_reduction: reduction,
            dither,
            wet_dry: wet,
        })
    }

    // ── Basic sanity ──────────────────────────────────────────────────────

    #[test]
    fn test_output_is_finite_and_bounded() {
        let mut fx = make_crusher(8, 1, false, 1.0);
        for i in 0..256 {
            let input = (i as f32 / 128.0) - 1.0; // sweep [-1, +1]
            let out = fx.process_sample(input);
            assert!(out.is_finite(), "output must be finite, got {out}");
            assert!(
                out.abs() <= 1.0 + 1e-5,
                "output magnitude must not exceed 1.0, got {out}"
            );
        }
    }

    #[test]
    fn test_silence_passthrough() {
        let mut fx = make_crusher(8, 1, false, 1.0);
        for _ in 0..64 {
            let out = fx.process_sample(0.0);
            assert!(
                out.abs() < 1e-9,
                "silence should remain silence without dither"
            );
        }
    }

    // ── Bit depth ─────────────────────────────────────────────────────────

    #[test]
    fn test_1_bit_only_two_output_levels() {
        let mut fx = make_crusher(1, 1, false, 1.0);
        // With 1-bit depth, half_steps = 1, so outputs must be in {-1.0, 0.0, +1.0}.
        for i in 0..100 {
            let input = (i as f32 / 50.0) - 1.0;
            let out = fx.process_sample(input);
            let valid =
                (out - (-1.0)).abs() < 1e-5 || (out - 0.0).abs() < 1e-5 || (out - 1.0).abs() < 1e-5;
            assert!(valid, "1-bit output {out} not in {{-1, 0, +1}}");
        }
    }

    #[test]
    fn test_high_bit_depth_preserves_input_closely() {
        let mut fx = make_crusher(24, 1, false, 1.0);
        let input = 0.123_456_789_f32;
        let out = fx.process_sample(input);
        // At 24-bit depth the quantisation error is < 2^-23 ≈ 1.2e-7.
        assert!(
            (out - input).abs() < 2e-7,
            "24-bit should preserve input closely; diff = {}",
            (out - input).abs()
        );
    }

    #[test]
    fn test_quantised_output_snaps_to_grid() {
        let mut fx = make_crusher(4, 1, false, 1.0);
        // With 4-bit depth, half_steps = 8, so valid outputs are multiples of 0.125.
        let step = 1.0_f32 / 8.0;
        for i in 0..100 {
            let input = (i as f32 / 50.0) - 1.0;
            let out = fx.process_sample(input);
            let remainder = (out / step).round() * step - out;
            assert!(
                remainder.abs() < 1e-5,
                "output {out} is not a multiple of {step}"
            );
        }
    }

    // ── Sample-rate reduction ─────────────────────────────────────────────

    #[test]
    fn test_sample_hold_holds_for_correct_duration() {
        let mut fx = make_crusher(16, 4, false, 1.0);
        // Feed a value that can be represented exactly.
        let val = 0.5_f32;
        let first = fx.process_sample(val);
        // The next three outputs should equal the first (hold phase).
        for _ in 0..3 {
            let held = fx.process_sample(0.0); // different input, same hold
            assert!(
                (held - first).abs() < 1e-6,
                "hold_counter should keep value constant; held={held}, first={first}"
            );
        }
    }

    #[test]
    fn test_no_reduction_does_not_hold() {
        // reduction=1 means each sample is freshly quantised.
        let mut fx = make_crusher(16, 1, false, 1.0);
        let a = fx.process_sample(0.25);
        let b = fx.process_sample(0.75);
        assert!(
            (a - b).abs() > 1e-3,
            "no hold: consecutive different inputs must give different outputs"
        );
    }

    // ── Wet/dry ───────────────────────────────────────────────────────────

    #[test]
    fn test_dry_mix_bypasses_effect() {
        let mut fx = make_crusher(2, 8, false, 0.0); // fully dry
        let input = 0.333_f32;
        let out = fx.process_sample(input);
        assert!(
            (out - input).abs() < 1e-6,
            "dry=1.0 should pass input unchanged"
        );
    }

    #[test]
    fn test_wet_dry_mix_interpolates() {
        let mut fx_wet = make_crusher(4, 1, false, 1.0);
        let mut fx_dry = make_crusher(4, 1, false, 0.0);
        let mut fx_mix = make_crusher(4, 1, false, 0.5);

        let input = 0.6_f32;
        let wet_out = fx_wet.process_sample(input);
        let dry_out = fx_dry.process_sample(input);
        let mix_out = fx_mix.process_sample(input);

        let expected = 0.5 * wet_out + 0.5 * dry_out;
        assert!(
            (mix_out - expected).abs() < 1e-5,
            "mix=0.5 should equal average of wet and dry; got {mix_out}, expected {expected}"
        );
    }

    #[test]
    fn test_set_wet_dry_trait_method() {
        let mut fx = make_crusher(8, 1, false, 1.0);
        fx.set_wet_dry(0.3);
        assert!((fx.wet_dry() - 0.3).abs() < 1e-6);
    }

    // ── Dither ────────────────────────────────────────────────────────────

    #[test]
    fn test_dither_output_is_finite() {
        let mut fx = make_crusher(4, 1, true, 1.0);
        for i in 0..256 {
            let input = (i as f32 / 128.0) - 1.0;
            let out = fx.process_sample(input);
            assert!(out.is_finite(), "dithered output must be finite");
        }
    }

    #[test]
    fn test_dither_does_not_dramatically_amplify() {
        let mut fx = make_crusher(8, 1, true, 1.0);
        for _ in 0..1000 {
            let out = fx.process_sample(0.0);
            // Dither on silence should stay within one quantisation step of 0.
            let step = 1.0_f32 / 128.0; // 1 / 2^(8-1)
            assert!(
                out.abs() <= step + 1e-6,
                "dither on silence too large: {out}"
            );
        }
    }

    #[test]
    fn test_dither_produces_variation_on_constant_signal() {
        // At very low bit depths a constant sub-threshold signal is either
        // always zero (no dither) or dithered to different quantisation levels.
        let val = 0.03_f32; // below the 4-bit step of 0.125

        let mut no_dither = make_crusher(4, 1, false, 1.0);
        let no_d: Vec<f32> = (0..64).map(|_| no_dither.process_sample(val)).collect();

        let mut with_dither = make_crusher(4, 1, true, 1.0);
        with_dither.dither_state = 0xdead_beef_cafe_f00d_u64; // deterministic seed
        let d: Vec<f32> = (0..64).map(|_| with_dither.process_sample(val)).collect();

        // Without dither: all outputs should be the same quantised level.
        assert!(
            no_d.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9),
            "no-dither constant input should produce constant output"
        );

        // With dither: outputs should vary.
        let all_same = d.windows(2).all(|w| (w[0] - w[1]).abs() < 1e-9);
        assert!(!all_same, "dither should produce varying outputs");
    }

    // ── Reset ─────────────────────────────────────────────────────────────

    #[test]
    fn test_reset_clears_state() {
        let mut fx = make_crusher(8, 4, false, 1.0);
        fx.process_sample(0.9);
        fx.process_sample(0.9);
        fx.reset();
        // After reset sample_hold = 0, so silence passes as silence.
        let out = fx.process_sample(0.0);
        // hold_counter is now 0 (reset), so the input 0.0 is freshly quantised.
        assert!(out.abs() < 1e-6, "reset should clear held sample");
    }

    // ── Config clamping ───────────────────────────────────────────────────

    #[test]
    fn test_config_clamping() {
        let fx = Bitcrusher::new(BitcrusherConfig {
            bit_depth: 0,             // below minimum → clamped to 1
            sample_rate_reduction: 0, // below minimum → clamped to 1
            dither: false,
            wet_dry: 2.5, // above maximum → clamped to 1.0
        });
        assert_eq!(fx.config.bit_depth, 1);
        assert_eq!(fx.config.sample_rate_reduction, 1);
        assert!((fx.config.wet_dry - 1.0).abs() < 1e-6);
    }

    // ── Buffer processing ─────────────────────────────────────────────────

    #[test]
    fn test_process_buffer_via_trait() {
        let config = BitcrusherConfig::default();
        let mut fx = Bitcrusher::new(config);
        let mut buf: Vec<f32> = (0..128).map(|i| (i as f32 / 64.0) - 1.0).collect();
        fx.process(&mut buf);
        for &s in &buf {
            assert!(s.is_finite());
        }
    }

    // ── PRNG uniqueness ───────────────────────────────────────────────────

    #[test]
    fn test_splitmix64_is_not_constant() {
        let mut fx = make_crusher(8, 1, false, 1.0);
        let a = fx.splitmix64();
        let b = fx.splitmix64();
        let c = fx.splitmix64();
        assert_ne!(a, b);
        assert_ne!(b, c);
    }
}
