// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Transfer functions: sRGB, PQ (SMPTE ST 2084), HLG (ARIB STD-B67) and
//! linear, in pure `f32`.
//!
//! Ported from `crates/oximedia-hdr/src/transfer_function.rs` (and its
//! `pq_simd.rs` batch shapes) with the `f64` data plane eliminated. The
//! LUT-accelerated encode path is owned by the pipeline (never a global
//! `Mutex`-guarded cache — see the `oximedia-colormgmt` `get_cached_tf_lut`
//! anti-pattern this crate deliberately avoids).
//!
//! # Normalisation conventions
//!
//! | Transfer | signal domain | linear meaning of 1.0 |
//! |----------|---------------|------------------------|
//! | `Srgb`   | `[0, 1]`      | SDR reference white (~100 nits) |
//! | `Pq`     | `[0, 1]`      | 10 000 nits (SMPTE ST 2084 absolute) |
//! | `Hlg`    | `[0, 1]`      | nominal HLG peak (~1 000 nits) |
//! | `Linear` | unbounded     | caller-defined |
//!
//! When combining a PQ input with the tone-map stage, set
//! `input_peak_nits = 10_000` (or the mastering peak if you pre-scale).
//! HLG decode applies an OOTF-lite (per-channel `x^1.2` system gamma) after
//! the inverse OETF; HLG encode applies the inverse OOTF before the OETF.

use crate::error::ColorError;
use oximedia_web_core::normalize::{srgb_eotf, srgb_oetf};

// ── PQ (SMPTE ST 2084) constants ─────────────────────────────────────────────

/// PQ m1 = 2610 / 16384.
const PQ_M1: f32 = 2610.0 / 16384.0;
/// PQ m2 = 2523 / 4096 × 128.
const PQ_M2: f32 = 2523.0 / 4096.0 * 128.0;
/// PQ c1 = 3424 / 4096.
const PQ_C1: f32 = 3424.0 / 4096.0;
/// PQ c2 = 2413 / 4096 × 32.
const PQ_C2: f32 = 2413.0 / 4096.0 * 32.0;
/// PQ c3 = 2392 / 4096 × 32.
const PQ_C3: f32 = 2392.0 / 4096.0 * 32.0;
/// 1 / m1.
const PQ_INV_M1: f32 = 1.0 / PQ_M1;
/// 1 / m2.
const PQ_INV_M2: f32 = 1.0 / PQ_M2;

// ── HLG (ARIB STD-B67) constants ──────────────────────────────────────────────

/// HLG `a` coefficient.
const HLG_A: f32 = 0.178_832_77;
/// HLG `b` coefficient.
const HLG_B: f32 = 0.284_668_92;
/// HLG `c` coefficient (0.55991073 truncated to `f32` precision).
const HLG_C: f32 = 0.559_910_7;
/// Scene-linear threshold (1/12) below which the OETF is a square root.
const HLG_THRESHOLD: f32 = 1.0 / 12.0;

/// OOTF-lite system gamma applied per channel on HLG decode.
///
/// The full BT.2100 HLG OOTF applies `Y^(γ-1)` scaling on luminance; this
/// module uses the common per-channel approximation (`x^1.2`), which is what
/// "OOTF-lite" means throughout this crate.
pub const HLG_OOTF_GAMMA: f32 = 1.2;

// ── PQ ────────────────────────────────────────────────────────────────────────

/// PQ EOTF: PQ signal `[0, 1]` → display-linear light (1.0 = 10 000 nits).
///
/// Branchless and clamped: NaN and out-of-range inputs are treated as the
/// nearest valid value (NaN → 0).
#[inline]
#[must_use]
pub fn pq_eotf(signal: f32) -> f32 {
    if !signal.is_finite() {
        return 0.0;
    }
    let s = signal.clamp(0.0, 1.0);
    let v = s.powf(PQ_INV_M2);
    let num = (v - PQ_C1).max(0.0);
    let den = (PQ_C2 - PQ_C3 * v).max(1e-7);
    (num / den).powf(PQ_INV_M1)
}

/// PQ OETF: display-linear light (1.0 = 10 000 nits) → PQ signal `[0, 1]`.
///
/// Branchless and clamped: NaN and negative inputs are treated as 0.
#[inline]
#[must_use]
pub fn pq_oetf(linear: f32) -> f32 {
    if !linear.is_finite() {
        return 0.0;
    }
    let lin = linear.clamp(0.0, 1.0);
    let y = lin.powf(PQ_M1);
    let num = PQ_C1 + PQ_C2 * y;
    let den = 1.0 + PQ_C3 * y;
    (num / den).powf(PQ_M2)
}

// ── HLG ───────────────────────────────────────────────────────────────────────

/// HLG OETF: scene-linear `[0, 1]` → HLG signal `[0, 1]` (ARIB STD-B67).
///
/// NaN and negative inputs are treated as 0; inputs above 1 are clamped.
#[inline]
#[must_use]
pub fn hlg_oetf(linear: f32) -> f32 {
    if !linear.is_finite() || linear <= 0.0 {
        return 0.0;
    }
    let lin = linear.min(1.0);
    if lin <= HLG_THRESHOLD {
        (3.0 * lin).sqrt()
    } else {
        HLG_A * (12.0 * lin - HLG_B).ln() + HLG_C
    }
}

/// HLG EOTF (inverse OETF): HLG signal `[0, 1]` → scene-linear `[0, 1]`.
///
/// NaN and negative inputs are treated as 0; inputs above 1 are clamped.
#[inline]
#[must_use]
pub fn hlg_eotf(signal: f32) -> f32 {
    if !signal.is_finite() || signal <= 0.0 {
        return 0.0;
    }
    let s = signal.min(1.0);
    // hlg_oetf(1/12) = sqrt(3 * 1/12) = 0.5, so 0.5 is the signal threshold.
    if s <= 0.5 {
        (s * s) / 3.0
    } else {
        (((s - HLG_C) / HLG_A).exp() + HLG_B) / 12.0
    }
}

/// OOTF-lite: HLG scene-linear → display-linear via per-channel `x^1.2`.
#[inline]
#[must_use]
pub fn hlg_ootf_lite(scene: f32) -> f32 {
    if !scene.is_finite() || scene <= 0.0 {
        0.0
    } else {
        scene.powf(HLG_OOTF_GAMMA)
    }
}

/// Inverse OOTF-lite: HLG display-linear → scene-linear via `x^(1/1.2)`.
#[inline]
#[must_use]
pub fn hlg_inverse_ootf_lite(display: f32) -> f32 {
    if !display.is_finite() || display <= 0.0 {
        0.0
    } else {
        display.powf(1.0 / HLG_OOTF_GAMMA)
    }
}

// ── Transfer selection ────────────────────────────────────────────────────────

/// Transfer function selector for the pipeline's input and output stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// sRGB (IEC 61966-2-1) piecewise curve. SDR default.
    Srgb,
    /// SMPTE ST 2084 Perceptual Quantizer. Linear 1.0 = 10 000 nits.
    Pq,
    /// ARIB STD-B67 Hybrid Log-Gamma, with per-channel OOTF-lite (`x^1.2`)
    /// applied on decode and inverted on encode.
    Hlg,
    /// No transfer function: signal is already linear. Values pass through
    /// unclamped (NaN is squashed to 0).
    Linear,
}

impl Transfer {
    /// Parses a transfer-function name.
    ///
    /// Accepted: `"srgb"`, `"pq"` (`"st2084"`, `"smpte2084"`), `"hlg"`
    /// (`"arib-std-b67"`), `"linear"`. Matching is ASCII case-insensitive.
    ///
    /// # Errors
    /// Returns [`ColorError::UnknownName`] for anything else.
    pub fn parse(name: &str) -> Result<Self, ColorError> {
        match name.to_ascii_lowercase().as_str() {
            "srgb" => Ok(Self::Srgb),
            "pq" | "st2084" | "smpte2084" => Ok(Self::Pq),
            "hlg" | "arib-std-b67" => Ok(Self::Hlg),
            "linear" => Ok(Self::Linear),
            _ => Err(ColorError::UnknownName {
                kind: "transfer function",
                name: name.to_string(),
            }),
        }
    }

    /// Canonical lowercase name of this transfer.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::Pq => "pq",
            Self::Hlg => "hlg",
            Self::Linear => "linear",
        }
    }

    /// Decodes an encoded signal value to pipeline-linear light.
    ///
    /// See the module docs for the per-transfer normalisation of 1.0.
    #[inline]
    #[must_use]
    pub fn decode(self, signal: f32) -> f32 {
        match self {
            Self::Srgb => {
                if signal.is_finite() {
                    srgb_eotf(signal.clamp(0.0, 1.0))
                } else {
                    0.0
                }
            }
            Self::Pq => pq_eotf(signal),
            Self::Hlg => hlg_ootf_lite(hlg_eotf(signal)),
            Self::Linear => {
                if signal.is_finite() {
                    signal
                } else {
                    0.0
                }
            }
        }
    }

    /// Encodes pipeline-linear light to an encoded signal value.
    ///
    /// `Linear` passes values through unclamped (HDR floats survive); the
    /// curve transfers clamp to their `[0, 1]` signal domain.
    #[inline]
    #[must_use]
    pub fn encode(self, linear: f32) -> f32 {
        match self {
            Self::Srgb => {
                if linear.is_finite() {
                    srgb_oetf(linear.clamp(0.0, 1.0))
                } else {
                    0.0
                }
            }
            Self::Pq => pq_oetf(linear),
            Self::Hlg => hlg_oetf(hlg_inverse_ootf_lite(linear)),
            Self::Linear => {
                if linear.is_finite() {
                    linear
                } else {
                    0.0
                }
            }
        }
    }
}

// ── LUT-accelerated encode (owned by the pipeline, never global) ─────────────

/// Number of intervals in the encode LUT (table has `N + 1` entries).
const ENCODE_LUT_INTERVALS: usize = 4096;

/// Square-root-domain encode LUT: linear `[0, 1]` → encoded signal.
///
/// The table is indexed by `sqrt(linear)` so the steep toe of PQ/HLG near
/// zero is sampled densely; worst-case error is well below half an 8-bit
/// code for all three curve transfers. Built once per
/// [`Transfer`](crate::transfer::Transfer) change and owned by the pipeline
/// struct — never behind a global lock.
#[derive(Clone, Debug)]
pub(crate) struct EncodeLut {
    table: Vec<f32>,
}

impl EncodeLut {
    /// Builds the LUT for `transfer`, or `None` for [`Transfer::Linear`]
    /// (which must pass unbounded HDR values through exactly).
    pub(crate) fn build(transfer: Transfer) -> Option<Self> {
        if transfer == Transfer::Linear {
            return None;
        }
        let n = ENCODE_LUT_INTERVALS;
        let mut table = Vec::with_capacity(n + 1);
        for i in 0..=n {
            let u = i as f32 / n as f32;
            table.push(transfer.encode(u * u));
        }
        Some(Self { table })
    }

    /// Evaluates the encode curve with linear interpolation.
    ///
    /// (Pure table lookup — despite the name, nothing is executed.)
    #[inline]
    pub(crate) fn eval(&self, linear: f32) -> f32 {
        if !linear.is_finite() {
            return 0.0;
        }
        let u = linear.clamp(0.0, 1.0).sqrt();
        let pos = u * ENCODE_LUT_INTERVALS as f32;
        let i0 = (pos as usize).min(ENCODE_LUT_INTERVALS - 1);
        let frac = pos - i0 as f32;
        let a = self.table[i0];
        let b = self.table[i0 + 1];
        a + frac * (b - a)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_web_core::normalize::u8_to_f32;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    // ── PQ anchors ───────────────────────────────────────────────────────────

    #[test]
    fn pq_eotf_zero_is_zero() {
        assert!(approx(pq_eotf(0.0), 0.0, 1e-6));
    }

    #[test]
    fn pq_eotf_one_is_one() {
        // Signal 1.0 = 10 000 nits = normalised linear 1.0.
        assert!(approx(pq_eotf(1.0), 1.0, 1e-5), "got {}", pq_eotf(1.0));
    }

    #[test]
    fn pq_round_trip_within_1e_4() {
        for i in 0..=1000 {
            let lin = i as f32 / 1000.0;
            let rt = pq_eotf(pq_oetf(lin));
            assert!(
                approx(lin, rt, 1e-4),
                "PQ round-trip at {lin}: got {rt}"
            );
        }
    }

    #[test]
    fn pq_signal_round_trip_within_1e_4() {
        for i in 0..=1000 {
            let sig = i as f32 / 1000.0;
            let rt = pq_oetf(pq_eotf(sig));
            assert!(
                approx(sig, rt, 1e-4),
                "PQ signal round-trip at {sig}: got {rt}"
            );
        }
    }

    #[test]
    fn pq_signal_058_is_about_203_nits() {
        // BT.2408 reference white: 203 cd/m² ≈ 58% PQ signal.
        let nits = pq_eotf(0.58) * 10_000.0;
        assert!(
            (nits - 203.0).abs() < 5.0,
            "PQ(0.58) should be ~203 nits, got {nits}"
        );
    }

    #[test]
    fn pq_oetf_100_nits_is_about_0_5081() {
        // Published ST 2084 reference: 100 nits (0.01 linear) → ~0.5081.
        let sig = pq_oetf(0.01);
        assert!(approx(sig, 0.5081, 1e-3), "PQ OETF(0.01) = {sig}");
    }

    #[test]
    fn pq_hostile_inputs_do_not_panic() {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -5.0, 5.0] {
            let a = pq_eotf(v);
            let b = pq_oetf(v);
            assert!(a.is_finite() && b.is_finite());
        }
    }

    // ── HLG ──────────────────────────────────────────────────────────────────

    #[test]
    fn hlg_oetf_threshold_is_half() {
        assert!(approx(hlg_oetf(1.0 / 12.0), 0.5, 1e-6));
    }

    #[test]
    fn hlg_round_trip() {
        for i in 0..=1000 {
            let lin = i as f32 / 1000.0;
            let rt = hlg_eotf(hlg_oetf(lin));
            assert!(
                approx(lin, rt, 1e-5),
                "HLG round-trip at {lin}: got {rt}"
            );
        }
    }

    #[test]
    fn hlg_full_decode_encode_round_trip() {
        // decode (EOTF + OOTF) then encode (inverse OOTF + OETF).
        for i in 0..=100 {
            let sig = i as f32 / 100.0;
            let rt = Transfer::Hlg.encode(Transfer::Hlg.decode(sig));
            assert!(
                approx(sig, rt, 1e-4),
                "HLG signal round-trip at {sig}: got {rt}"
            );
        }
    }

    #[test]
    fn hlg_ootf_lite_round_trip() {
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            let rt = hlg_inverse_ootf_lite(hlg_ootf_lite(x));
            assert!(approx(x, rt, 1e-5));
        }
    }

    #[test]
    fn hlg_hostile_inputs_do_not_panic() {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0] {
            assert!(hlg_oetf(v).is_finite());
            assert!(hlg_eotf(v).is_finite());
            assert!(hlg_ootf_lite(v).is_finite());
            assert!(hlg_inverse_ootf_lite(v).is_finite());
        }
    }

    // ── sRGB anchors (via oximedia-web-core) ─────────────────────────────────

    #[test]
    fn srgb_anchor_0_5_decodes_to_0_2140() {
        let lin = Transfer::Srgb.decode(0.5);
        assert!(approx(lin, 0.2140, 5e-4), "sRGB EOTF(0.5) = {lin}");
    }

    #[test]
    fn srgb_round_trip() {
        for i in 0..=255u32 {
            let sig = u8_to_f32(i as u8);
            let rt = Transfer::Srgb.encode(Transfer::Srgb.decode(sig));
            assert!(approx(sig, rt, 1e-5), "sRGB round-trip at {sig}: {rt}");
        }
    }

    // ── Transfer enum ────────────────────────────────────────────────────────

    #[test]
    fn parse_accepts_known_names() {
        assert_eq!(Transfer::parse("srgb"), Ok(Transfer::Srgb));
        assert_eq!(Transfer::parse("SRGB"), Ok(Transfer::Srgb));
        assert_eq!(Transfer::parse("pq"), Ok(Transfer::Pq));
        assert_eq!(Transfer::parse("st2084"), Ok(Transfer::Pq));
        assert_eq!(Transfer::parse("hlg"), Ok(Transfer::Hlg));
        assert_eq!(Transfer::parse("linear"), Ok(Transfer::Linear));
    }

    #[test]
    fn parse_rejects_unknown_names() {
        assert!(Transfer::parse("gamma22").is_err());
        assert!(Transfer::parse("").is_err());
    }

    #[test]
    fn linear_transfer_preserves_hdr_values() {
        assert!(approx(Transfer::Linear.encode(37.5), 37.5, 0.0));
        assert!(approx(Transfer::Linear.decode(-0.25), -0.25, 0.0));
        assert!(approx(Transfer::Linear.encode(f32::NAN), 0.0, 0.0));
    }

    #[test]
    fn names_round_trip_through_parse() {
        for t in [Transfer::Srgb, Transfer::Pq, Transfer::Hlg, Transfer::Linear] {
            assert_eq!(Transfer::parse(t.name()), Ok(t));
        }
    }

    // ── EncodeLut accuracy ───────────────────────────────────────────────────

    #[test]
    fn encode_lut_matches_exact_curve_within_half_u8_code() {
        for transfer in [Transfer::Srgb, Transfer::Pq, Transfer::Hlg] {
            let lut = match EncodeLut::build(transfer) {
                Some(l) => l,
                None => panic!("curve transfer must build a LUT"),
            };
            let mut max_err = 0.0f32;
            for i in 0..=100_000 {
                let lin = i as f32 / 100_000.0;
                let err = (lut.eval(lin) - transfer.encode(lin)).abs();
                max_err = max_err.max(err);
            }
            assert!(
                max_err < 0.5 / 255.0,
                "{}: encode LUT max error {max_err} exceeds half a u8 code",
                transfer.name()
            );
        }
    }

    #[test]
    fn encode_lut_linear_is_none() {
        assert!(EncodeLut::build(Transfer::Linear).is_none());
    }

    #[test]
    fn encode_lut_hostile_inputs() {
        let lut = match EncodeLut::build(Transfer::Pq) {
            Some(l) => l,
            None => panic!("PQ must build a LUT"),
        };
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 2.0] {
            assert!(lut.eval(v).is_finite());
        }
    }
}
