//! Parametric equaliser using cascaded biquad filters with f64 internal precision.
//!
//! This module provides a [`ParametricEq`] that implements the [`AudioEffect`]
//! trait, enabling drop-in use within OxiMedia effect chains.  Each band is a
//! second-order IIR biquad filter whose coefficients follow the Robert
//! Bristow-Johnson *Audio EQ Cookbook* formulas.
//!
//! Internal processing uses `f64` to maintain numerical stability when many
//! bands are cascaded.
//!
//! # Supported band types
//!
//! | Type | Description |
//! |------|-------------|
//! | [`BandType::LowShelf`]  | Boost/cut below frequency |
//! | [`BandType::HighShelf`] | Boost/cut above frequency |
//! | [`BandType::Peaking`]   | Bell-curve boost/cut around frequency |
//! | [`BandType::Notch`]     | Narrow band-reject |
//! | [`BandType::LowPass`]   | Second-order Butterworth low-pass |
//! | [`BandType::HighPass`]  | Second-order Butterworth high-pass |
//! | [`BandType::BandPass`]  | Constant-skirt-gain band-pass |
//! | [`BandType::AllPass`]   | Unity magnitude, phase-shift only |
//!
//! # Example
//!
//! ```
//! use oximedia_effects::parametric_eq::{ParametricEq, EqBand, BandType};
//! use oximedia_effects::AudioEffect;
//!
//! let mut eq = ParametricEq::new(48_000.0);
//! eq.add_band(EqBand {
//!     band_type: BandType::Peaking,
//!     frequency: 1000.0,
//!     gain_db: 6.0,
//!     q: 1.0,
//!     enabled: true,
//! });
//!
//! let out = eq.process_sample(0.5);
//! assert!(out.is_finite());
//! ```

#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]

use crate::AudioEffect;
use std::f64::consts::PI;

// ═══════════════════════════════════════════════════════════ BandType ══════

/// Band type for parametric EQ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandType {
    /// Boost or cut below the cutoff frequency.
    LowShelf,
    /// Boost or cut above the cutoff frequency.
    HighShelf,
    /// Bell-curve boost or cut around a centre frequency.
    Peaking,
    /// Narrow band-reject filter.
    Notch,
    /// Second-order Butterworth low-pass filter.
    LowPass,
    /// Second-order Butterworth high-pass filter.
    HighPass,
    /// Constant-skirt-gain band-pass filter.
    BandPass,
    /// Unity-gain all-pass filter (phase shift only).
    AllPass,
}

// ═══════════════════════════════════════════════════════════ EqBand ════════

/// A single EQ band configuration.
#[derive(Debug, Clone)]
pub struct EqBand {
    /// Filter shape / type.
    pub band_type: BandType,
    /// Centre or cutoff frequency in Hz.
    pub frequency: f32,
    /// Gain in dB (positive = boost, negative = cut).
    /// Only meaningful for [`BandType::Peaking`], [`BandType::LowShelf`],
    /// and [`BandType::HighShelf`].
    pub gain_db: f32,
    /// Quality factor controlling bandwidth.
    pub q: f32,
    /// Whether this band is active. A disabled band passes the signal
    /// unchanged without consuming CPU on coefficient computation.
    pub enabled: bool,
}

impl EqBand {
    /// Create a peaking (bell) EQ band.
    #[must_use]
    pub fn peaking(frequency: f32, gain_db: f32, q: f32) -> Self {
        Self {
            band_type: BandType::Peaking,
            frequency,
            gain_db,
            q,
            enabled: true,
        }
    }

    /// Create a low-shelf band with Butterworth Q.
    #[must_use]
    pub fn low_shelf(frequency: f32, gain_db: f32) -> Self {
        Self {
            band_type: BandType::LowShelf,
            frequency,
            gain_db,
            q: 0.707,
            enabled: true,
        }
    }

    /// Create a high-shelf band with Butterworth Q.
    #[must_use]
    pub fn high_shelf(frequency: f32, gain_db: f32) -> Self {
        Self {
            band_type: BandType::HighShelf,
            frequency,
            gain_db,
            q: 0.707,
            enabled: true,
        }
    }

    /// Create a notch (band-reject) band.
    #[must_use]
    pub fn notch(frequency: f32, q: f32) -> Self {
        Self {
            band_type: BandType::Notch,
            frequency,
            gain_db: 0.0,
            q,
            enabled: true,
        }
    }

    /// Create a low-pass filter band.
    #[must_use]
    pub fn low_pass(frequency: f32, q: f32) -> Self {
        Self {
            band_type: BandType::LowPass,
            frequency,
            gain_db: 0.0,
            q,
            enabled: true,
        }
    }

    /// Create a high-pass filter band.
    #[must_use]
    pub fn high_pass(frequency: f32, q: f32) -> Self {
        Self {
            band_type: BandType::HighPass,
            frequency,
            gain_db: 0.0,
            q,
            enabled: true,
        }
    }

    /// Create a band-pass filter band.
    #[must_use]
    pub fn band_pass(frequency: f32, q: f32) -> Self {
        Self {
            band_type: BandType::BandPass,
            frequency,
            gain_db: 0.0,
            q,
            enabled: true,
        }
    }

    /// Create an all-pass filter band.
    #[must_use]
    pub fn all_pass(frequency: f32, q: f32) -> Self {
        Self {
            band_type: BandType::AllPass,
            frequency,
            gain_db: 0.0,
            q,
            enabled: true,
        }
    }
}

// ═══════════════════════════════════════════════════════ BiquadCoeffs ═════

/// Biquad filter coefficients (normalised by a0).
///
/// Transfer function:
/// `H(z) = (b0 + b1*z^-1 + b2*z^-2) / (1 + a1*z^-1 + a2*z^-2)`
#[derive(Debug, Clone)]
pub struct BiquadCoeffs {
    /// Feed-forward coefficient b0.
    pub b0: f64,
    /// Feed-forward coefficient b1.
    pub b1: f64,
    /// Feed-forward coefficient b2.
    pub b2: f64,
    /// Feedback coefficient a1.
    pub a1: f64,
    /// Feedback coefficient a2.
    pub a2: f64,
}

impl Default for BiquadCoeffs {
    /// Identity (pass-through) coefficients.
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }
}

// ═══════════════════════════════════════════════════════ BiquadState ══════

/// Biquad filter state (Direct Form II Transposed).
///
/// Uses two delay elements `z1` and `z2`.
#[derive(Debug, Clone)]
pub struct BiquadState {
    /// First delay element.
    pub z1: f64,
    /// Second delay element.
    pub z2: f64,
}

impl Default for BiquadState {
    fn default() -> Self {
        Self { z1: 0.0, z2: 0.0 }
    }
}

// ═══════════════════════════════════════════════════════ ParametricEq ═════

/// Parametric equaliser with configurable bands and f64 internal precision.
///
/// Implements [`AudioEffect`] so it can be used directly in effect chains,
/// wrapped with [`MixEffect`](crate::mix::MixEffect), or composed with other
/// effects.
pub struct ParametricEq {
    bands: Vec<EqBand>,
    coeffs: Vec<BiquadCoeffs>,
    states: Vec<BiquadState>,
    sample_rate: f32,
}

impl ParametricEq {
    /// Create a new empty parametric EQ at the given sample rate.
    #[must_use]
    pub fn new(sample_rate: f32) -> Self {
        Self {
            bands: Vec::new(),
            coeffs: Vec::new(),
            states: Vec::new(),
            sample_rate,
        }
    }

    /// Add a band and compute its biquad coefficients.
    pub fn add_band(&mut self, band: EqBand) {
        let c = Self::compute_coefficients(&band, self.sample_rate);
        self.coeffs.push(c);
        self.states.push(BiquadState::default());
        self.bands.push(band);
    }

    /// Replace a band at `index` and recompute its coefficients.
    ///
    /// Returns `Err` if `index` is out of range.
    pub fn set_band(&mut self, index: usize, band: EqBand) -> Result<(), String> {
        if index >= self.bands.len() {
            return Err(format!(
                "Band index {index} out of range (have {} bands)",
                self.bands.len()
            ));
        }
        let c = Self::compute_coefficients(&band, self.sample_rate);
        self.coeffs[index] = c;
        self.states[index] = BiquadState::default();
        self.bands[index] = band;
        Ok(())
    }

    /// Remove a band by index.
    ///
    /// Returns `Err` if `index` is out of range.
    pub fn remove_band(&mut self, index: usize) -> Result<EqBand, String> {
        if index >= self.bands.len() {
            return Err(format!(
                "Band index {index} out of range (have {} bands)",
                self.bands.len()
            ));
        }
        self.coeffs.remove(index);
        self.states.remove(index);
        Ok(self.bands.remove(index))
    }

    /// Return the number of bands.
    #[must_use]
    pub fn band_count(&self) -> usize {
        self.bands.len()
    }

    /// Get an immutable reference to a band by index.
    #[must_use]
    pub fn band(&self, index: usize) -> Option<&EqBand> {
        self.bands.get(index)
    }

    /// Process a buffer of samples through all enabled bands (in-place).
    pub fn process_samples(&mut self, samples: &mut [f32]) {
        for sample in samples.iter_mut() {
            let mut s = f64::from(*sample);
            for (i, band) in self.bands.iter().enumerate() {
                if band.enabled {
                    s = Self::process_biquad(s, &self.coeffs[i], &mut self.states[i]);
                }
            }
            *sample = s as f32;
        }
    }

    /// Reset all filter states (for seek / discontinuity).
    pub fn reset_states(&mut self) {
        for state in &mut self.states {
            state.z1 = 0.0;
            state.z2 = 0.0;
        }
    }

    /// Return the sample rate.
    #[must_use]
    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// Builder helper — append a band and return `self`.
    #[must_use]
    pub fn with_band(mut self, band: EqBand) -> Self {
        self.add_band(band);
        self
    }

    // ────────────────────────────────────── coefficient computation ──────

    /// Compute normalised biquad coefficients for a band.
    ///
    /// Uses the Robert Bristow-Johnson *Audio EQ Cookbook* formulas.
    fn compute_coefficients(band: &EqBand, sample_rate: f32) -> BiquadCoeffs {
        let sr = f64::from(sample_rate);
        let freq = f64::from(band.frequency);
        let gain = f64::from(band.gain_db);
        let q = f64::from(band.q).max(f64::EPSILON);

        let w0 = 2.0 * PI * freq / sr;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q);

        let (b0, b1, b2, a0, a1, a2) = match band.band_type {
            BandType::Peaking => {
                let a = 10.0_f64.powf(gain / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cos_w0,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos_w0,
                    1.0 - alpha / a,
                )
            }
            BandType::LowShelf => {
                let a = 10.0_f64.powf(gain / 40.0);
                let sqrt_a = a.sqrt();
                (
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0),
                    a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha),
                    (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0),
                    (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha,
                )
            }
            BandType::HighShelf => {
                let a = 10.0_f64.powf(gain / 40.0);
                let sqrt_a = a.sqrt();
                (
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0),
                    a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha),
                    (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos_w0),
                    (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha,
                )
            }
            BandType::LowPass => (
                (1.0 - cos_w0) / 2.0,
                1.0 - cos_w0,
                (1.0 - cos_w0) / 2.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            BandType::HighPass => (
                (1.0 + cos_w0) / 2.0,
                -(1.0 + cos_w0),
                (1.0 + cos_w0) / 2.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            BandType::Notch => (
                1.0,
                -2.0 * cos_w0,
                1.0,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
            BandType::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha),
            BandType::AllPass => (
                1.0 - alpha,
                -2.0 * cos_w0,
                1.0 + alpha,
                1.0 + alpha,
                -2.0 * cos_w0,
                1.0 - alpha,
            ),
        };

        // Normalise by a0
        let inv_a0 = if a0.abs() < f64::EPSILON {
            1.0
        } else {
            1.0 / a0
        };

        BiquadCoeffs {
            b0: b0 * inv_a0,
            b1: b1 * inv_a0,
            b2: b2 * inv_a0,
            a1: a1 * inv_a0,
            a2: a2 * inv_a0,
        }
    }

    /// Process a single sample through one biquad stage (Direct Form II Transposed).
    #[inline]
    fn process_biquad(sample: f64, coeffs: &BiquadCoeffs, state: &mut BiquadState) -> f64 {
        let output = coeffs.b0 * sample + state.z1;
        state.z1 = coeffs.b1 * sample - coeffs.a1 * output + state.z2;
        state.z2 = coeffs.b2 * sample - coeffs.a2 * output;
        output
    }
}

// ═══════════════════════════════════════════════════ AudioEffect impl ═════

impl AudioEffect for ParametricEq {

    const EFFECT_ID: u64 = 6004;
    fn process_sample(&mut self, input: f32) -> f32 {
        let mut s = f64::from(input);
        for (i, band) in self.bands.iter().enumerate() {
            if band.enabled {
                s = Self::process_biquad(s, &self.coeffs[i], &mut self.states[i]);
            }
        }
        s as f32
    }

    fn process(&mut self, buffer: &mut [f32]) {
        self.process_samples(buffer);
    }

    fn reset(&mut self) {
        self.reset_states();
    }

    fn latency_samples(&self) -> usize {
        0
    }

    fn set_sample_rate(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        // Recompute all coefficients for the new sample rate
        for (i, band) in self.bands.iter().enumerate() {
            self.coeffs[i] = Self::compute_coefficients(band, sample_rate);
        }
        self.reset_states();
    }
}

// ═══════════════════════════════════════════════════════════ tests ════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI as PI32;

    const SR: f32 = 48_000.0;

    fn make_sine(freq: f32, sample_rate: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI32 * freq * i as f32 / sample_rate).sin())
            .collect()
    }

    fn rms(buf: &[f32]) -> f32 {
        if buf.is_empty() {
            return 0.0;
        }
        (buf.iter().map(|&s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    // ── basic structure ──────────────────────────────────────────────────

    #[test]
    fn test_eq_new_no_bands() {
        let eq = ParametricEq::new(SR);
        assert_eq!(eq.band_count(), 0);
    }

    #[test]
    fn test_eq_add_band() {
        let mut eq = ParametricEq::new(SR);
        eq.add_band(EqBand::peaking(1000.0, 6.0, 1.0));
        assert_eq!(eq.band_count(), 1);
        eq.add_band(EqBand::low_shelf(200.0, 3.0));
        assert_eq!(eq.band_count(), 2);
    }

    // ── passthrough / bypass ─────────────────────────────────────────────

    #[test]
    fn test_eq_flat_passthrough() {
        // A peaking band with gain = 0 dB should not modify the signal.
        let mut eq = ParametricEq::new(SR).with_band(EqBand::peaking(1000.0, 0.0, 1.0));
        let input = make_sine(1000.0, SR, 512);
        let output: Vec<f32> = input.iter().map(|&s| eq.process_sample(s)).collect();
        for (i, (&a, &b)) in input.iter().zip(output.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-5,
                "Flat EQ should pass through unchanged, sample {i}: in={a}, out={b}"
            );
        }
    }

    // ── low-pass attenuates high ─────────────────────────────────────────

    #[test]
    fn test_eq_low_pass_attenuates_high() {
        let mut eq = ParametricEq::new(SR).with_band(EqBand::low_pass(1000.0, 0.707));
        // Settle the filter
        let settle = make_sine(10_000.0, SR, 4096);
        for &s in &settle {
            eq.process_sample(s);
        }
        let input = make_sine(10_000.0, SR, 1024);
        let output: Vec<f32> = input.iter().map(|&s| eq.process_sample(s)).collect();
        let in_rms = rms(&input);
        let out_rms = rms(&output);
        assert!(
            out_rms < in_rms * 0.5,
            "LPF at 1 kHz should attenuate 10 kHz: in_rms={in_rms:.4}, out_rms={out_rms:.4}"
        );
    }

    // ── high-pass attenuates low ─────────────────────────────────────────

    #[test]
    fn test_eq_high_pass_attenuates_low() {
        let mut eq = ParametricEq::new(SR).with_band(EqBand::high_pass(1000.0, 0.707));
        // Settle
        let settle = make_sine(100.0, SR, 4096);
        for &s in &settle {
            eq.process_sample(s);
        }
        let input = make_sine(100.0, SR, 1024);
        let output: Vec<f32> = input.iter().map(|&s| eq.process_sample(s)).collect();
        let in_rms = rms(&input);
        let out_rms = rms(&output);
        assert!(
            out_rms < in_rms * 0.5,
            "HPF at 1 kHz should attenuate 100 Hz: in_rms={in_rms:.4}, out_rms={out_rms:.4}"
        );
    }

    // ── peaking boost ────────────────────────────────────────────────────

    #[test]
    fn test_eq_peaking_boost_at_center() {
        let mut eq = ParametricEq::new(SR).with_band(EqBand::peaking(1000.0, 12.0, 1.0));
        // Settle
        let settle = make_sine(1000.0, SR, 4096);
        for &s in &settle {
            eq.process_sample(s);
        }
        let input = make_sine(1000.0, SR, 1024);
        let output: Vec<f32> = input.iter().map(|&s| eq.process_sample(s)).collect();
        assert!(
            rms(&output) > rms(&input),
            "Peaking +12 dB at 1 kHz should boost: in={:.4}, out={:.4}",
            rms(&input),
            rms(&output)
        );
    }

    // ── notch attenuates ─────────────────────────────────────────────────

    #[test]
    fn test_eq_notch_attenuates_center() {
        let mut eq = ParametricEq::new(SR).with_band(EqBand::notch(1000.0, 1.0));
        // Settle with wider Q for deeper attenuation
        let settle = make_sine(1000.0, SR, 8192);
        for &s in &settle {
            eq.process_sample(s);
        }
        let input = make_sine(1000.0, SR, 1024);
        let output: Vec<f32> = input.iter().map(|&s| eq.process_sample(s)).collect();
        assert!(
            rms(&output) < rms(&input) * 0.8,
            "Notch at 1 kHz should reduce RMS: in={:.4}, out={:.4}",
            rms(&input),
            rms(&output)
        );
    }

    // ── reset ────────────────────────────────────────────────────────────

    #[test]
    fn test_eq_reset_clears_state() {
        let mut eq = ParametricEq::new(SR).with_band(EqBand::peaking(1000.0, 6.0, 1.0));
        // Prime with non-zero signal
        for &s in &make_sine(1000.0, SR, 256) {
            eq.process_sample(s);
        }
        eq.reset();
        // After reset, processing silence should give silence
        let out = eq.process_sample(0.0);
        assert!(
            out.abs() < 1e-10,
            "After reset, zero input should produce zero output, got {out}"
        );
    }

    // ── remove band ──────────────────────────────────────────────────────

    #[test]
    fn test_eq_remove_band() {
        let mut eq = ParametricEq::new(SR);
        eq.add_band(EqBand::peaking(500.0, 3.0, 1.0));
        eq.add_band(EqBand::notch(2000.0, 5.0));
        assert_eq!(eq.band_count(), 2);
        let removed = eq.remove_band(0);
        assert!(removed.is_ok());
        assert_eq!(eq.band_count(), 1);
        // Remaining band should be the notch
        assert_eq!(eq.band(0).map(|b| b.band_type), Some(BandType::Notch));
    }

    // ── disabled band bypassed ───────────────────────────────────────────

    #[test]
    fn test_eq_disabled_band_bypassed() {
        let band = EqBand {
            band_type: BandType::Peaking,
            frequency: 1000.0,
            gain_db: 20.0,
            q: 1.0,
            enabled: false,
        };
        let mut eq = ParametricEq::new(SR).with_band(band);
        let input = make_sine(1000.0, SR, 256);
        let output: Vec<f32> = input.iter().map(|&s| eq.process_sample(s)).collect();
        for (i, (&a, &b)) in input.iter().zip(output.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "Disabled band should pass through, sample {i}: in={a}, out={b}"
            );
        }
    }
}
