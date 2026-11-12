// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Tone-mapping operators with peak-nits control.
//!
//! Ported from `crates/oximedia-hdr/src/tone_mapping.rs` (the canonical `f32`
//! `ToneMapper`) and, for [`ToneMapOperator::AcesOdt`], from
//! `crates/oximedia-colormgmt/src/aces_output_transform.rs` (`AcesOt2`).
//!
//! # Which "ACES" is which (honesty note)
//!
//! * [`ToneMapOperator::Aces`] — the **Narkowicz 2015 fitted approximation**
//!   of the ACES filmic curve (`(x(2.51x+0.03))/(x(2.43x+0.59)+0.14)`),
//!   applied on BT.2100 luminance with hue-preserving ratio scaling. Fast and
//!   the common real-time choice; *not* the reference ACES RRT/ODT.
//! * [`ToneMapOperator::AcesOdt`] — an **ACES Output-Transform-2.0-shaped**
//!   rendering: the `AcesOt2` `rrt_improved` S-curve (toe / mid / peak-nits-
//!   adaptive shoulder) applied **per channel**, followed by the parametric
//!   gamut compression (threshold 0.75, power 1.2). It is a faithful port of
//!   the OxiMedia `AcesOt2` transform, *not* a bit-exact implementation of
//!   the Academy CTL reference. Combine with
//!   [`ColorPipeline::set_gamut`](crate::pipeline::ColorPipeline::set_gamut)
//!   for the display-primaries step of a full ODT.
//!
//! All operators map scene/display-linear input (1.0 = `input_peak_nits`) to
//! display-linear `[0, 1]` (1.0 = `output_peak_nits`). Output encoding
//! (sRGB/PQ/HLG) is the pipeline's transfer stage, not the tone mapper's.

use crate::error::ColorError;

/// Available tone-mapping operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneMapOperator {
    /// Classic Reinhard: `L / (1 + L)` on luminance.
    Reinhard,
    /// Extended Reinhard `L(1 + L/Lw²)/(1 + L)` with the white point `Lw`
    /// anchored at the scaled input peak, so input peak maps exactly to 1.0.
    ReinhardExtended,
    /// Uncharted 2 / Hable filmic curve, normalised by `hable(11.2)`.
    Hable,
    /// ACES — Narkowicz 2015 fitted approximation (see module docs).
    Aces,
    /// ACES OT 2.0-shaped RRT + gamut compression (see module docs).
    AcesOdt,
}

impl ToneMapOperator {
    /// Parses an operator name.
    ///
    /// Accepted (ASCII case-insensitive): `"reinhard"`,
    /// `"reinhard-extended"`, `"hable"` / `"filmic"`, `"aces"`,
    /// `"aces-odt"`.
    ///
    /// # Errors
    /// Returns [`ColorError::UnknownName`] for anything else.
    pub fn parse(name: &str) -> Result<Self, ColorError> {
        match name.to_ascii_lowercase().as_str() {
            "reinhard" => Ok(Self::Reinhard),
            "reinhard-extended" | "reinhard_extended" => Ok(Self::ReinhardExtended),
            "hable" | "filmic" => Ok(Self::Hable),
            "aces" => Ok(Self::Aces),
            "aces-odt" | "aces_odt" => Ok(Self::AcesOdt),
            _ => Err(ColorError::UnknownName {
                kind: "tone-map operator",
                name: name.to_string(),
            }),
        }
    }

    /// Canonical lowercase name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Reinhard => "reinhard",
            Self::ReinhardExtended => "reinhard-extended",
            Self::Hable => "hable",
            Self::Aces => "aces",
            Self::AcesOdt => "aces-odt",
        }
    }
}

// ── Curve primitives (ported) ─────────────────────────────────────────────────

/// Reinhard: `x / (1 + x)`.
#[inline]
fn reinhard(x: f32) -> f32 {
    x / (1.0 + x)
}

/// Extended Reinhard with white point `lw`.
#[inline]
fn reinhard_extended(x: f32, lw: f32) -> f32 {
    x * (1.0 + x / (lw * lw)) / (1.0 + x)
}

/// Hable / Uncharted 2 partial curve (A–F constants).
#[inline]
fn hable_partial(x: f32) -> f32 {
    const A: f32 = 0.15;
    const B: f32 = 0.50;
    const C: f32 = 0.10;
    const D: f32 = 0.20;
    const E: f32 = 0.02;
    const F: f32 = 0.30;
    ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - E / F
}

/// ACES filmic — Narkowicz 2015 fitted approximation.
#[inline]
fn aces_narkowicz(x: f32) -> f32 {
    const A: f32 = 2.51;
    const B: f32 = 0.03;
    const C: f32 = 2.43;
    const D: f32 = 0.59;
    const E: f32 = 0.14;
    (x * (A * x + B)) / (x * (C * x + D) + E)
}

/// `AcesOt2::rrt_improved` — toe / mid / adaptive-shoulder blend
/// (free function so the hot slice loop can hoist the operator dispatch).
#[inline]
fn rrt_improved(x: f32, shoulder_width: f32) -> f32 {
    let v = x.max(0.0);
    let toe = 0.04 * v;
    let mid = v / (v + 0.18);
    let highlight = 1.0 - (-v * shoulder_width).exp();
    let blend = v.clamp(0.0, 1.0);
    let result = toe * (1.0 - blend) + (mid * 0.4 + highlight * 0.6) * blend;
    result.clamp(0.0, 1.0)
}

/// `AcesOt2` parametric gamut compression for one channel
/// (threshold 0.75, power 1.2; negatives clamp to 0).
#[inline]
fn aces_odt_compress_channel(v: f32) -> f32 {
    const THRESHOLD: f32 = 0.75;
    const POWER: f32 = 1.2;
    let mut ch = v;
    if ch > THRESHOLD {
        let excess = ch - THRESHOLD;
        let range = 1.0 - THRESHOLD;
        let normalized = excess / range;
        let compressed = normalized / (1.0 + normalized.powf(POWER)).powf(1.0 / POWER);
        ch = THRESHOLD + compressed * range;
    }
    ch.max(0.0)
}

// ── ToneMap ───────────────────────────────────────────────────────────────────

/// BT.2100 luma coefficients used for the luminance-preserving operators.
const LUMA_R: f32 = 0.2627;
const LUMA_G: f32 = 0.6780;
const LUMA_B: f32 = 0.0593;

/// Configured tone-map stage with precomputed constants.
#[derive(Clone, Debug)]
pub struct ToneMap {
    op: ToneMapOperator,
    input_peak_nits: f32,
    output_peak_nits: f32,
    /// `output_peak_nits / input_peak_nits`, applied before the curve.
    scale: f32,
    /// White point for `ReinhardExtended` (the scaled input peak).
    lw: f32,
    /// `1 / hable_partial(11.2)`.
    hable_norm: f32,
    /// Peak-adaptive shoulder width for `AcesOdt` (`rrt_improved`).
    shoulder_width: f32,
}

impl ToneMap {
    /// Creates a tone-map stage.
    ///
    /// `input_peak_nits` is the luminance meant by linear 1.0 on input
    /// (10 000 for PQ-decoded, ~1 000 for HLG, 100 for SDR);
    /// `output_peak_nits` is the target display peak.
    ///
    /// # Errors
    /// Returns [`ColorError::NonFinite`] / [`ColorError::OutOfRange`] if
    /// either peak is not a finite positive number.
    pub fn new(
        op: ToneMapOperator,
        input_peak_nits: f32,
        output_peak_nits: f32,
    ) -> Result<Self, ColorError> {
        for (v, what) in [
            (input_peak_nits, "input peak nits"),
            (output_peak_nits, "output peak nits"),
        ] {
            if !v.is_finite() {
                return Err(ColorError::NonFinite { what });
            }
            if v <= 0.0 {
                return Err(ColorError::OutOfRange { what });
            }
        }
        let scale = output_peak_nits / input_peak_nits;
        // AcesOt2::rrt_improved: shoulder opens up for brighter displays.
        let peak_factor = output_peak_nits / 100.0;
        let shoulder_width = 1.0 + 0.2 * peak_factor.ln().max(0.0);
        Ok(Self {
            op,
            input_peak_nits,
            output_peak_nits,
            scale,
            lw: scale.max(1e-6),
            hable_norm: 1.0 / hable_partial(11.2),
            shoulder_width,
        })
    }

    /// The configured operator.
    #[must_use]
    pub const fn operator(&self) -> ToneMapOperator {
        self.op
    }

    /// Input peak luminance in nits.
    #[must_use]
    pub const fn input_peak_nits(&self) -> f32 {
        self.input_peak_nits
    }

    /// Output peak luminance in nits.
    #[must_use]
    pub const fn output_peak_nits(&self) -> f32 {
        self.output_peak_nits
    }

    /// Maps a pre-scaled luminance/channel value through the configured
    /// curve, clamped to `[0, 1]`.
    #[inline]
    fn curve(&self, x: f32) -> f32 {
        let v = match self.op {
            ToneMapOperator::Reinhard => reinhard(x),
            ToneMapOperator::ReinhardExtended => reinhard_extended(x, self.lw),
            ToneMapOperator::Hable => hable_partial(x) * self.hable_norm,
            ToneMapOperator::Aces => aces_narkowicz(x),
            ToneMapOperator::AcesOdt => self.rrt_improved(x),
        };
        v.clamp(0.0, 1.0)
    }

    /// `AcesOt2::rrt_improved` — toe / mid / adaptive-shoulder blend.
    #[inline]
    fn rrt_improved(&self, x: f32) -> f32 {
        rrt_improved(x, self.shoulder_width)
    }

    /// Maps one linear RGB pixel (1.0 = input peak) to display-linear
    /// `[0, 1]` (1.0 = output peak).
    ///
    /// Luminance-curve operators preserve colour ratios (hue) by scaling RGB
    /// with `mapped / luminance`; `AcesOdt` is per-channel + gamut
    /// compression, matching the ported `AcesOt2` behaviour.
    #[inline]
    #[must_use]
    pub fn map_rgb(&self, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        if self.op == ToneMapOperator::AcesOdt {
            let f = |ch: f32| {
                let x = (ch * self.scale).max(0.0);
                aces_odt_compress_channel(self.rrt_improved(x))
            };
            return (f(r), f(g), f(b));
        }
        let lum = LUMA_R * r + LUMA_G * g + LUMA_B * b;
        if lum <= 1e-7 {
            return (0.0, 0.0, 0.0);
        }
        let mapped = self.curve(lum * self.scale);
        let ratio = mapped / lum;
        (r * ratio, g * ratio, b * ratio)
    }

    /// Maps an interleaved RGBA `f32` slice in place (alpha lanes are left
    /// untouched). Bit-identical to calling [`ToneMap::map_rgb`] per pixel,
    /// but the operator dispatch is hoisted out of the loop so each arm is a
    /// tight, monomorphized kernel.
    pub fn map_slice_rgba(&self, buf: &mut [f32]) {
        match self.op {
            ToneMapOperator::Reinhard => lum_ratio_loop(buf, self.scale, reinhard),
            ToneMapOperator::ReinhardExtended => {
                let lw = self.lw;
                lum_ratio_loop(buf, self.scale, move |x| reinhard_extended(x, lw));
            }
            ToneMapOperator::Hable => {
                let norm = self.hable_norm;
                lum_ratio_loop(buf, self.scale, move |x| hable_partial(x) * norm);
            }
            ToneMapOperator::Aces => lum_ratio_loop(buf, self.scale, aces_narkowicz),
            ToneMapOperator::AcesOdt => {
                let sw = self.shoulder_width;
                let scale = self.scale;
                for px in buf.chunks_exact_mut(4) {
                    px[0] = aces_odt_compress_channel(rrt_improved((px[0] * scale).max(0.0), sw));
                    px[1] = aces_odt_compress_channel(rrt_improved((px[1] * scale).max(0.0), sw));
                    px[2] = aces_odt_compress_channel(rrt_improved((px[2] * scale).max(0.0), sw));
                }
            }
        }
    }
}

/// Shared kernel for the luminance-preserving operators: computes BT.2100
/// luminance, maps it through `curve`, and scales RGB by the ratio.
#[inline(always)]
fn lum_ratio_loop(buf: &mut [f32], scale: f32, curve: impl Fn(f32) -> f32) {
    for px in buf.chunks_exact_mut(4) {
        let lum = LUMA_R * px[0] + LUMA_G * px[1] + LUMA_B * px[2];
        if lum <= 1e-7 {
            px[0] = 0.0;
            px[1] = 0.0;
            px[2] = 0.0;
            continue;
        }
        let mapped = curve(lum * scale).clamp(0.0, 1.0);
        let ratio = mapped / lum;
        px[0] *= ratio;
        px[1] *= ratio;
        px[2] *= ratio;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn unity(op: ToneMapOperator) -> ToneMap {
        ToneMap::new(op, 100.0, 100.0).expect("unity tone map")
    }

    // ── Reinhard anchors ─────────────────────────────────────────────────────

    #[test]
    fn reinhard_curve_anchors() {
        let tm = unity(ToneMapOperator::Reinhard);
        // x/(1+x): f(0)=0, f(1)=0.5, f(3)=0.75
        assert!(approx(tm.curve(0.0), 0.0, 1e-6));
        assert!(approx(tm.curve(1.0), 0.5, 1e-6));
        assert!(approx(tm.curve(3.0), 0.75, 1e-6));
    }

    #[test]
    fn reinhard_gray_pixel_preserves_neutrality() {
        let tm = unity(ToneMapOperator::Reinhard);
        let (r, g, b) = tm.map_rgb(0.5, 0.5, 0.5);
        // luminance 0.5 → 1/3; ratio 2/3 → all channels 1/3.
        assert!(approx(r, 1.0 / 3.0, 1e-5) && approx(g, r, 1e-6) && approx(b, r, 1e-6));
    }

    #[test]
    fn reinhard_extended_maps_input_peak_to_one() {
        // With 1000 → 100 nits, an input of 1.0 (the input peak) must land on 1.0.
        let tm = ToneMap::new(ToneMapOperator::ReinhardExtended, 1000.0, 100.0)
            .expect("tone map");
        let (r, g, b) = tm.map_rgb(1.0, 1.0, 1.0);
        assert!(approx(r, 1.0, 1e-4), "peak white: {r}");
        assert!(approx(g, 1.0, 1e-4) && approx(b, 1.0, 1e-4));
    }

    // ── Hable anchors (curve values derived from the upstream constants) ─────

    #[test]
    fn hable_anchor_values() {
        let tm = unity(ToneMapOperator::Hable);
        // hable(11.2) normalises to 1.0.
        assert!(approx(tm.curve(11.2), 1.0, 1e-5));
        // hable_partial(1.0)/hable_partial(11.2) ≈ 0.3043.
        assert!(approx(tm.curve(1.0), 0.3043, 1e-3), "got {}", tm.curve(1.0));
        assert!(approx(tm.curve(0.0), 0.0, 1e-6));
    }

    #[test]
    fn hable_output_in_range() {
        let tm = ToneMap::new(ToneMapOperator::Hable, 1000.0, 100.0).expect("tm");
        for v in [0.0f32, 0.1, 0.5, 1.0, 2.0, 10.0] {
            let (r, g, b) = tm.map_rgb(v, v, v);
            for c in [r, g, b] {
                assert!((0.0..=1.0).contains(&c), "Hable({v}) = {c}");
            }
        }
    }

    // ── ACES (Narkowicz fitted) ──────────────────────────────────────────────

    #[test]
    fn aces_fitted_anchor_mid_gray() {
        let tm = unity(ToneMapOperator::Aces);
        // aces(0.18) ≈ 0.2669
        assert!(approx(tm.curve(0.18), 0.2669, 1e-3), "got {}", tm.curve(0.18));
    }

    #[test]
    fn aces_fitted_monotonic_and_bounded() {
        let tm = unity(ToneMapOperator::Aces);
        let mut prev = -1.0f32;
        for i in 0..=4000 {
            let x = i as f32 * 0.01; // [0, 40]
            let y = tm.curve(x);
            assert!((0.0..=1.0).contains(&y), "ACES({x}) = {y} out of [0,1]");
            assert!(y >= prev - 1e-6, "ACES not monotonic at {x}: {y} < {prev}");
            prev = y;
        }
    }

    // ── ACES-ODT (AcesOt2 port) ──────────────────────────────────────────────

    #[test]
    fn aces_odt_gray_ramp_is_sane() {
        let tm = ToneMap::new(ToneMapOperator::AcesOdt, 100.0, 100.0).expect("tm");
        let mut prev = -1.0f32;
        for i in 0..=100 {
            let x = i as f32 / 25.0; // [0, 4]
            let (r, g, b) = tm.map_rgb(x, x, x);
            assert!(approx(r, g, 1e-6) && approx(g, b, 1e-6), "gray stays gray");
            assert!((0.0..=1.0).contains(&r), "AcesOdt({x}) = {r}");
            assert!(r >= prev - 1e-5, "AcesOdt not monotonic at {x}: {r} < {prev}");
            prev = r;
        }
        // Black maps to (near) black; bright input approaches the operator's
        // ceiling. Note: the ported AcesOt2 gamut compression soft-clips the
        // top so peak white lands at ~0.89 pre-OETF (faithful to the source,
        // which reserves highlight headroom), not at exactly 1.0.
        let (black, _, _) = tm.map_rgb(0.0, 0.0, 0.0);
        assert!(black.abs() < 1e-5);
        let (bright, _, _) = tm.map_rgb(50.0, 50.0, 50.0);
        assert!(bright > 0.85, "bright input should approach the ceiling: {bright}");
    }

    #[test]
    fn aces_odt_hdr_target_opens_shoulder() {
        // A brighter target display must render a bright input at or above
        // the SDR rendering (wider shoulder ⇒ more highlight headroom).
        let sdr = ToneMap::new(ToneMapOperator::AcesOdt, 1000.0, 100.0).expect("sdr");
        let hdr = ToneMap::new(ToneMapOperator::AcesOdt, 1000.0, 1000.0).expect("hdr");
        let (s, _, _) = sdr.map_rgb(0.9, 0.9, 0.9);
        let (h, _, _) = hdr.map_rgb(0.9, 0.9, 0.9);
        assert!(h >= s - 1e-6, "HDR target should not be darker: sdr={s} hdr={h}");
    }

    // ── Peak-nits behaviour ──────────────────────────────────────────────────

    #[test]
    fn peak_nits_scale_compresses_hdr_input() {
        // 1000-nit content on a 100-nit display: input 1.0 is scaled to 0.1
        // before the curve, so it must map well below 1.0 with Reinhard.
        let tm = ToneMap::new(ToneMapOperator::Reinhard, 1000.0, 100.0).expect("tm");
        let (r, _, _) = tm.map_rgb(1.0, 1.0, 1.0);
        assert!(approx(r, 0.1 / 1.1, 1e-4), "got {r}");
    }

    #[test]
    fn invalid_peaks_are_rejected() {
        assert!(ToneMap::new(ToneMapOperator::Aces, 0.0, 100.0).is_err());
        assert!(ToneMap::new(ToneMapOperator::Aces, 100.0, -1.0).is_err());
        assert!(ToneMap::new(ToneMapOperator::Aces, f32::NAN, 100.0).is_err());
        assert!(ToneMap::new(ToneMapOperator::Aces, 100.0, f32::INFINITY).is_err());
    }

    #[test]
    fn black_maps_to_black_for_every_operator() {
        for op in [
            ToneMapOperator::Reinhard,
            ToneMapOperator::ReinhardExtended,
            ToneMapOperator::Hable,
            ToneMapOperator::Aces,
            ToneMapOperator::AcesOdt,
        ] {
            let tm = ToneMap::new(op, 1000.0, 100.0).expect("tm");
            let (r, g, b) = tm.map_rgb(0.0, 0.0, 0.0);
            assert!(
                r.abs() < 1e-5 && g.abs() < 1e-5 && b.abs() < 1e-5,
                "{}: black → ({r},{g},{b})",
                op.name()
            );
        }
    }

    #[test]
    fn hue_ratio_preserved_by_luminance_operators() {
        let tm = ToneMap::new(ToneMapOperator::Aces, 1000.0, 100.0).expect("tm");
        let (r, g, b) = tm.map_rgb(0.8, 0.4, 0.2);
        // Channel ratios must match the input ratios (hue preservation).
        assert!(approx(r / g, 2.0, 1e-3), "r/g = {}", r / g);
        assert!(approx(g / b, 2.0, 1e-3), "g/b = {}", g / b);
    }

    #[test]
    fn parse_accepts_known_operators() {
        assert_eq!(ToneMapOperator::parse("reinhard"), Ok(ToneMapOperator::Reinhard));
        assert_eq!(
            ToneMapOperator::parse("reinhard-extended"),
            Ok(ToneMapOperator::ReinhardExtended)
        );
        assert_eq!(ToneMapOperator::parse("hable"), Ok(ToneMapOperator::Hable));
        assert_eq!(ToneMapOperator::parse("filmic"), Ok(ToneMapOperator::Hable));
        assert_eq!(ToneMapOperator::parse("ACES"), Ok(ToneMapOperator::Aces));
        assert_eq!(ToneMapOperator::parse("aces-odt"), Ok(ToneMapOperator::AcesOdt));
        assert!(ToneMapOperator::parse("mobius").is_err());
    }

    #[test]
    fn names_round_trip() {
        for op in [
            ToneMapOperator::Reinhard,
            ToneMapOperator::ReinhardExtended,
            ToneMapOperator::Hable,
            ToneMapOperator::Aces,
            ToneMapOperator::AcesOdt,
        ] {
            assert_eq!(ToneMapOperator::parse(op.name()), Ok(op));
        }
    }
}
