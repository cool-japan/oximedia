// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Primaries-aware gamut conversion (BT.709 / BT.2020 / Display-P3) with
//! hue-preserving out-of-gamut handling.
//!
//! Ported from `crates/oximedia-hdr/src/gamut.rs` (the canonical
//! `GamutConversionMatrix` + soft-clip implementation; the
//! `oximedia-colormgmt` `GamutMapper` is intentionally **not** used — it
//! ignores its colorspace argument). The conversion matrix is derived once at
//! configuration time via CIE XYZ with a Bradford CAT (all three supported
//! gamuts share the D65 white point, so the CAT is an identity in practice)
//! and stored as a single-precision 3×3 for the hot loop.

use crate::error::ColorError;

// ── Primaries ─────────────────────────────────────────────────────────────────

/// Supported RGB primary sets. All use the D65 white point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primaries {
    /// ITU-R BT.709 / sRGB (HD).
    Bt709,
    /// ITU-R BT.2020 / Rec.2100 (UHD / HDR).
    Bt2020,
    /// Display P3 (P3 primaries, D65 white).
    DisplayP3,
}

impl Primaries {
    /// Parses a primaries name.
    ///
    /// Accepted (ASCII case-insensitive): `"bt709"` / `"rec709"` / `"709"` /
    /// `"srgb"`, `"bt2020"` / `"rec2020"` / `"2020"`, `"display-p3"` /
    /// `"p3"` / `"p3-d65"` / `"p3d65"`.
    ///
    /// # Errors
    /// Returns [`ColorError::UnknownName`] for anything else.
    pub fn parse(name: &str) -> Result<Self, ColorError> {
        match name.to_ascii_lowercase().as_str() {
            "bt709" | "rec709" | "709" | "srgb" => Ok(Self::Bt709),
            "bt2020" | "rec2020" | "2020" => Ok(Self::Bt2020),
            "display-p3" | "p3" | "p3-d65" | "p3d65" => Ok(Self::DisplayP3),
            _ => Err(ColorError::UnknownName {
                kind: "primaries",
                name: name.to_string(),
            }),
        }
    }

    /// Canonical lowercase name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bt709 => "bt709",
            Self::Bt2020 => "bt2020",
            Self::DisplayP3 => "display-p3",
        }
    }

    /// CIE 1931 xy chromaticities: `[[rx, ry], [gx, gy], [bx, by], [wx, wy]]`.
    const fn chromaticities(self) -> [[f64; 2]; 4] {
        const D65: [f64; 2] = [0.3127, 0.3290];
        match self {
            Self::Bt709 => [[0.64, 0.33], [0.30, 0.60], [0.15, 0.06], D65],
            Self::Bt2020 => [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046], D65],
            Self::DisplayP3 => [[0.680, 0.320], [0.265, 0.690], [0.150, 0.060], D65],
        }
    }
}

// ── f64 matrix helpers (configuration time only — never on the data plane) ───

type Mat3 = [[f64; 3]; 3];

/// CIE xy → XYZ tristimulus with Y = 1.
fn xy_to_xyz(x: f64, y: f64) -> [f64; 3] {
    [(x / y), 1.0, ((1.0 - x - y) / y)]
}

fn mat3_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [[0.0f64; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            for k in 0..3 {
                *cell += a[i][k] * b[k][j];
            }
        }
    }
    out
}

fn mat3_mul_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// 3×3 inverse; returns `None` for a singular matrix.
fn mat3_inverse(m: &Mat3) -> Option<Mat3> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ])
}

/// RGB → XYZ matrix for the given primaries (white normalised to Y = 1).
fn rgb_to_xyz_matrix(p: Primaries) -> Option<Mat3> {
    let c = p.chromaticities();
    let xr = xy_to_xyz(c[0][0], c[0][1]);
    let xg = xy_to_xyz(c[1][0], c[1][1]);
    let xb = xy_to_xyz(c[2][0], c[2][1]);
    let xw = xy_to_xyz(c[3][0], c[3][1]);

    let m: Mat3 = [
        [xr[0], xg[0], xb[0]],
        [xr[1], xg[1], xb[1]],
        [xr[2], xg[2], xb[2]],
    ];
    let mi = mat3_inverse(&m)?;
    let s = mat3_mul_vec(&mi, xw);
    Some([
        [m[0][0] * s[0], m[0][1] * s[1], m[0][2] * s[2]],
        [m[1][0] * s[0], m[1][1] * s[1], m[1][2] * s[2]],
        [m[2][0] * s[0], m[2][1] * s[1], m[2][2] * s[2]],
    ])
}

/// Bradford cone-response matrix.
const BRADFORD: Mat3 = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Inverse Bradford matrix.
const BRADFORD_INV: Mat3 = [
    [0.9869929, -0.1470543, 0.1599627],
    [0.4323053, 0.5183603, 0.0492912],
    [-0.0085287, 0.0400428, 0.9684867],
];

/// Bradford chromatic adaptation from `src` white to `dst` white.
fn bradford_cat(src_w: [f64; 2], dst_w: [f64; 2]) -> Option<Mat3> {
    let s_cone = mat3_mul_vec(&BRADFORD, xy_to_xyz(src_w[0], src_w[1]));
    let d_cone = mat3_mul_vec(&BRADFORD, xy_to_xyz(dst_w[0], dst_w[1]));
    if s_cone.iter().any(|&v| v.abs() < 1e-12) {
        return None;
    }
    let scale: Mat3 = [
        [d_cone[0] / s_cone[0], 0.0, 0.0],
        [0.0, d_cone[1] / s_cone[1], 0.0],
        [0.0, 0.0, d_cone[2] / s_cone[2]],
    ];
    Some(mat3_mul(&BRADFORD_INV, &mat3_mul(&scale, &BRADFORD)))
}

/// Computes the `src` → `dst` RGB conversion matrix via XYZ (+ Bradford CAT
/// when the white points differ — never for the three built-in D65 gamuts).
fn compute_matrix(src: Primaries, dst: Primaries) -> Result<[[f32; 3]; 3], ColorError> {
    if src == dst {
        return Ok([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    }
    let singular = || ColorError::OutOfRange {
        what: "gamut primaries (singular matrix)",
    };
    let src_to_xyz = rgb_to_xyz_matrix(src).ok_or_else(singular)?;
    let dst_to_xyz = rgb_to_xyz_matrix(dst).ok_or_else(singular)?;
    let xyz_to_dst = mat3_inverse(&dst_to_xyz).ok_or_else(singular)?;

    let src_w = src.chromaticities()[3];
    let dst_w = dst.chromaticities()[3];
    let adapted = if src_w == dst_w {
        src_to_xyz
    } else {
        let cat = bradford_cat(src_w, dst_w).ok_or_else(singular)?;
        mat3_mul(&cat, &src_to_xyz)
    };
    let m64 = mat3_mul(&xyz_to_dst, &adapted);

    let mut m = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = m64[i][j] as f32;
        }
    }
    Ok(m)
}

// ── Out-of-gamut handling (ported soft-clip pipeline) ────────────────────────

/// BT.2100 luminance of a linear RGB triplet.
#[inline]
fn bt2100_luma(r: f32, g: f32, b: f32) -> f32 {
    0.2627 * r + 0.6780 * g + 0.0593 * b
}

/// Desaturates channels toward `luma` until no channel is negative
/// (hue-preserving fix for out-of-gamut colours after a matrix conversion).
#[inline]
#[must_use]
pub fn desaturate_negatives(r: f32, g: f32, b: f32, luma: f32) -> (f32, f32, f32) {
    if r >= 0.0 && g >= 0.0 && b >= 0.0 {
        return (r, g, b);
    }
    if luma <= 0.0 {
        return (r.max(0.0), g.max(0.0), b.max(0.0));
    }
    let mut t = 1.0f32;
    for c in [r, g, b] {
        if c < 0.0 {
            let max_t = luma / (luma - c).max(1e-10);
            t = t.min(max_t);
        }
    }
    t = t.clamp(0.0, 1.0);
    (
        luma + t * (r - luma),
        luma + t * (g - luma),
        luma + t * (b - luma),
    )
}

/// Desaturates toward the achromatic axis when any channel exceeds 1.0,
/// preserving hue while reducing chroma just enough to fit `[0, 1]`.
#[inline]
#[must_use]
pub fn desaturate_highlights(r: f32, g: f32, b: f32, luma: f32) -> (f32, f32, f32) {
    let max_ch = r.max(g).max(b);
    if max_ch <= 1.0 {
        return (r, g, b);
    }
    let mut t = 1.0f32;
    for c in [r, g, b] {
        if c > 1.0 {
            let denom = (c - luma).max(1e-10);
            let max_t = (1.0 - luma).max(0.0) / denom;
            t = t.min(max_t);
        }
    }
    t = t.clamp(0.0, 1.0);
    (
        luma + t * (r - luma),
        luma + t * (g - luma),
        luma + t * (b - luma),
    )
}

/// Soft-knee compression for a single channel: maps `[0, ∞)` → `[0, 1]` with
/// a C1-continuous tanh roll-off above the knee (`knee = 1 − softness/2`).
///
/// `softness = 0` degenerates to a hard clamp.
#[inline]
#[must_use]
pub fn soft_knee_compress(x: f32, softness: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let knee = 1.0 - 0.5 * softness;
    if x <= knee {
        return x;
    }
    if softness < 1e-6 {
        return x.min(1.0);
    }
    let overshoot = x - knee;
    let headroom = (1.0 - knee).max(1e-7);
    let arg = overshoot / headroom;
    let tanh_val = if arg > 15.0 {
        1.0
    } else {
        let e2x = (2.0 * arg).exp();
        (e2x - 1.0) / (e2x + 1.0)
    };
    knee + headroom * tanh_val
}

/// Full perceptual soft-clip: negative desaturation → highlight desaturation
/// → per-channel soft-knee compression. Output is within `[0, 1]`.
#[must_use]
pub fn soft_clip_gamut_map(r: f32, g: f32, b: f32, softness: f32) -> (f32, f32, f32) {
    let luma = bt2100_luma(r, g, b);
    let (r1, g1, b1) = desaturate_negatives(r, g, b, luma);
    let (r2, g2, b2) = desaturate_highlights(r1, g1, b1, luma.clamp(0.0, 1.0));
    let s = softness.clamp(0.0, 1.0);
    (
        soft_knee_compress(r2, s),
        soft_knee_compress(g2, s),
        soft_knee_compress(b2, s),
    )
}

// ── GamutMap ──────────────────────────────────────────────────────────────────

/// Precomputed primaries conversion with hue-preserving out-of-gamut fix-up.
///
/// With `softness == 0` (the default used by
/// [`ColorPipeline::set_gamut`](crate::pipeline::ColorPipeline::set_gamut))
/// only negative channels are corrected (hue-preserving desaturation) and
/// values above 1.0 pass through untouched, so HDR linear workflows and the
/// identity conversion are exact. With `softness > 0` the full ported
/// soft-clip pipeline runs and output is confined to `[0, 1]`.
#[derive(Clone, Debug)]
pub struct GamutMap {
    src: Primaries,
    dst: Primaries,
    matrix: [[f32; 3]; 3],
    softness: f32,
}

impl GamutMap {
    /// Builds the conversion from `src` to `dst` primaries.
    ///
    /// # Errors
    /// Returns [`ColorError::OutOfRange`] if a primaries matrix is singular
    /// (cannot happen for the built-in gamuts, but kept honest).
    pub fn new(src: Primaries, dst: Primaries) -> Result<Self, ColorError> {
        Ok(Self {
            src,
            dst,
            matrix: compute_matrix(src, dst)?,
            softness: 0.0,
        })
    }

    /// Source primaries.
    #[must_use]
    pub const fn src(&self) -> Primaries {
        self.src
    }

    /// Destination primaries.
    #[must_use]
    pub const fn dst(&self) -> Primaries {
        self.dst
    }

    /// The 3×3 conversion matrix (row-major).
    #[must_use]
    pub const fn matrix(&self) -> &[[f32; 3]; 3] {
        &self.matrix
    }

    /// Sets the soft-clip softness (clamped to `[0, 1]`; `0` disables
    /// highlight compression, see the struct docs).
    ///
    /// # Errors
    /// Returns [`ColorError::NonFinite`] for NaN/infinite input.
    pub fn set_softness(&mut self, softness: f32) -> Result<(), ColorError> {
        if !softness.is_finite() {
            return Err(ColorError::NonFinite { what: "gamut softness" });
        }
        self.softness = softness.clamp(0.0, 1.0);
        Ok(())
    }

    /// Current softness.
    #[must_use]
    pub const fn softness(&self) -> f32 {
        self.softness
    }

    /// Converts one linear RGB triplet.
    #[inline]
    #[must_use]
    pub fn convert(&self, r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        let m = &self.matrix;
        let or = m[0][0] * r + m[0][1] * g + m[0][2] * b;
        let og = m[1][0] * r + m[1][1] * g + m[1][2] * b;
        let ob = m[2][0] * r + m[2][1] * g + m[2][2] * b;
        if self.softness > 0.0 {
            soft_clip_gamut_map(or, og, ob, self.softness)
        } else {
            desaturate_negatives(or, og, ob, bt2100_luma(or, og, ob))
        }
    }

    /// Converts an interleaved RGBA `f32` slice in place (alpha lanes are
    /// left untouched). Bit-identical to calling [`GamutMap::convert`] per
    /// pixel, with the soft-clip dispatch hoisted out of the loop.
    pub fn convert_slice_rgba(&self, buf: &mut [f32]) {
        let m = self.matrix;
        if self.softness > 0.0 {
            let s = self.softness;
            for px in buf.chunks_exact_mut(4) {
                let (r, g, b) = (px[0], px[1], px[2]);
                let or = m[0][0] * r + m[0][1] * g + m[0][2] * b;
                let og = m[1][0] * r + m[1][1] * g + m[1][2] * b;
                let ob = m[2][0] * r + m[2][1] * g + m[2][2] * b;
                let (cr, cg, cb) = soft_clip_gamut_map(or, og, ob, s);
                px[0] = cr;
                px[1] = cg;
                px[2] = cb;
            }
        } else {
            for px in buf.chunks_exact_mut(4) {
                let (r, g, b) = (px[0], px[1], px[2]);
                let or = m[0][0] * r + m[0][1] * g + m[0][2] * b;
                let og = m[1][0] * r + m[1][1] * g + m[1][2] * b;
                let ob = m[2][0] * r + m[2][1] * g + m[2][2] * b;
                let (cr, cg, cb) =
                    desaturate_negatives(or, og, ob, bt2100_luma(or, og, ob));
                px[0] = cr;
                px[1] = cg;
                px[2] = cb;
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn identity_gamut_is_exact_passthrough() {
        for p in [Primaries::Bt709, Primaries::Bt2020, Primaries::DisplayP3] {
            let m = GamutMap::new(p, p).expect("identity map");
            let (r, g, b) = m.convert(0.5, 0.3, 0.7);
            assert!(approx(r, 0.5, 1e-6), "identity R: {r}");
            assert!(approx(g, 0.3, 1e-6), "identity G: {g}");
            assert!(approx(b, 0.7, 1e-6), "identity B: {b}");
        }
    }

    #[test]
    fn identity_preserves_hdr_values() {
        let m = GamutMap::new(Primaries::Bt709, Primaries::Bt709).expect("map");
        let (r, _, _) = m.convert(12.5, 3.0, 8.0);
        assert!(approx(r, 12.5, 1e-6), "HDR passthrough: {r}");
    }

    #[test]
    fn bt2020_to_bt709_white_maps_to_white() {
        let m = GamutMap::new(Primaries::Bt2020, Primaries::Bt709).expect("map");
        let (r, g, b) = m.convert(1.0, 1.0, 1.0);
        assert!(approx(r, 1.0, 1e-3) && approx(g, 1.0, 1e-3) && approx(b, 1.0, 1e-3));
    }

    #[test]
    fn bt2020_to_bt709_black_maps_to_black() {
        let m = GamutMap::new(Primaries::Bt2020, Primaries::Bt709).expect("map");
        let (r, g, b) = m.convert(0.0, 0.0, 0.0);
        assert!(approx(r, 0.0, 1e-6) && approx(g, 0.0, 1e-6) && approx(b, 0.0, 1e-6));
    }

    #[test]
    fn round_trip_bt2020_bt709() {
        let fwd = GamutMap::new(Primaries::Bt2020, Primaries::Bt709).expect("fwd");
        let inv = GamutMap::new(Primaries::Bt709, Primaries::Bt2020).expect("inv");
        // Pick a colour that stays in-gamut both ways so no clipping applies.
        let (r1, g1, b1) = fwd.convert(0.4, 0.45, 0.5);
        let (r2, g2, b2) = inv.convert(r1, g1, b1);
        assert!(approx(r2, 0.4, 1e-4), "R round-trip: {r2}");
        assert!(approx(g2, 0.45, 1e-4), "G round-trip: {g2}");
        assert!(approx(b2, 0.5, 1e-4), "B round-trip: {b2}");
    }

    #[test]
    fn p3_to_bt709_white_maps_to_white() {
        let m = GamutMap::new(Primaries::DisplayP3, Primaries::Bt709).expect("map");
        let (r, g, b) = m.convert(1.0, 1.0, 1.0);
        assert!(approx(r, 1.0, 1e-3) && approx(g, 1.0, 1e-3) && approx(b, 1.0, 1e-3));
    }

    #[test]
    fn out_of_gamut_bt2020_green_clamps_sensibly() {
        // Pure BT.2020 green is far outside BT.709: the raw matrix output has
        // negative R/B. The default (softness 0) map must return non-negative
        // channels with green still dominant.
        let m = GamutMap::new(Primaries::Bt2020, Primaries::Bt709).expect("map");
        let raw_g = {
            let mm = m.matrix();
            (
                mm[0][0] * 0.0 + mm[0][1] * 1.0,
                mm[1][1] * 1.0,
                mm[2][1] * 1.0,
            )
        };
        assert!(raw_g.0 < 0.0, "raw R should be negative: {}", raw_g.0);
        let (r, g, b) = m.convert(0.0, 1.0, 0.0);
        assert!(r >= 0.0 && g >= 0.0 && b >= 0.0, "({r},{g},{b})");
        assert!(g > r && g > b, "green must stay dominant: ({r},{g},{b})");
    }

    #[test]
    fn soft_clip_output_in_unit_range() {
        let mut m = GamutMap::new(Primaries::Bt2020, Primaries::Bt709).expect("map");
        m.set_softness(0.8).expect("softness");
        for (r, g, b) in [(5.0, 0.0, 0.0), (0.0, 1.0, 0.0), (1.2, 1.2, 1.2), (0.3, 0.4, 0.5)] {
            let (or, og, ob) = m.convert(r, g, b);
            for v in [or, og, ob] {
                assert!((0.0..=1.0 + 1e-3).contains(&v), "out of range: {v}");
            }
        }
    }

    #[test]
    fn softness_rejects_non_finite() {
        let mut m = GamutMap::new(Primaries::Bt709, Primaries::Bt2020).expect("map");
        assert!(m.set_softness(f32::NAN).is_err());
        assert!(m.set_softness(0.5).is_ok());
        assert!(approx(m.softness(), 0.5, 0.0));
    }

    #[test]
    fn parse_accepts_aliases() {
        assert_eq!(Primaries::parse("BT709"), Ok(Primaries::Bt709));
        assert_eq!(Primaries::parse("srgb"), Ok(Primaries::Bt709));
        assert_eq!(Primaries::parse("rec2020"), Ok(Primaries::Bt2020));
        assert_eq!(Primaries::parse("display-p3"), Ok(Primaries::DisplayP3));
        assert_eq!(Primaries::parse("p3"), Ok(Primaries::DisplayP3));
        assert!(Primaries::parse("adobe-rgb").is_err());
    }

    #[test]
    fn names_round_trip() {
        for p in [Primaries::Bt709, Primaries::Bt2020, Primaries::DisplayP3] {
            assert_eq!(Primaries::parse(p.name()), Ok(p));
        }
    }

    // ── Ported soft-clip helper behaviour ────────────────────────────────────

    #[test]
    fn desaturate_negatives_passthrough_when_positive() {
        let (r, g, b) = desaturate_negatives(0.5, 0.3, 0.7, 0.4);
        assert!(approx(r, 0.5, 1e-6) && approx(g, 0.3, 1e-6) && approx(b, 0.7, 1e-6));
    }

    #[test]
    fn desaturate_negatives_fixes_negative_channel() {
        let (r, g, b) = desaturate_negatives(1.5, -0.3, 0.2, 0.5);
        assert!(r >= -1e-6 && g >= -1e-6 && b >= -1e-6, "({r},{g},{b})");
    }

    #[test]
    fn desaturate_highlights_caps_overshoot() {
        let (r, g, b) = desaturate_highlights(1.5, 0.3, 0.2, 0.5);
        assert!(r <= 1.001, "R capped: {r}");
        assert!(g >= 0.0 && b >= 0.0);
    }

    #[test]
    fn soft_knee_monotonic_and_bounded() {
        for &softness in &[0.0f32, 0.1, 0.5, 1.0] {
            let mut prev = 0.0f32;
            for i in 1..=400 {
                let x = i as f32 / 100.0;
                let v = soft_knee_compress(x, softness);
                assert!(v >= prev - 1e-6, "not monotonic at {x} (s={softness})");
                assert!(v <= 1.0, "exceeds 1.0 at {x} (s={softness}): {v}");
                prev = v;
            }
        }
    }

    #[test]
    fn soft_knee_c1_continuity_at_knee() {
        let softness = 0.5;
        let knee = 1.0 - 0.5 * softness;
        let eps = 1e-4;
        let below = soft_knee_compress(knee - eps, softness);
        let above = soft_knee_compress(knee + eps, softness);
        let derivative = (above - below) / (2.0 * eps);
        assert!(approx(derivative, 1.0, 0.05), "derivative at knee: {derivative}");
    }
}
