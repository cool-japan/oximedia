// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Color matrix selection and fixed-point YCbCr <-> RGB coefficient tables.
//!
//! The public surface is the [`ColorMatrix`] enum. The coefficient structs are
//! internal: the conversion kernels in [`crate::yuv`] build them once per call
//! (a handful of integer multiplies, negligible next to a full-frame loop) and
//! apply them per pixel with `i32` fixed-point arithmetic.
//!
//! All coefficients are scaled by `2^14` (see [`SCALE_BITS`]); a rounding bias
//! of `2^13` is added before the right shift.

/// Number of fractional bits in the fixed-point coefficients.
pub(crate) const SCALE_BITS: u32 = 14;
/// Fixed-point scale factor (`2^SCALE_BITS`).
pub(crate) const SCALE: i32 = 1 << SCALE_BITS;
/// Rounding bias added before the final right shift (`2^(SCALE_BITS-1)`).
pub(crate) const ROUND: i32 = 1 << (SCALE_BITS - 1);

/// ITU-R color matrix / range combination for YCbCr <-> RGB conversion.
///
/// The `Limited` variants use studio-swing quantization (Y in `[16, 235]`,
/// Cb/Cr in `[16, 240]`); the `Full` variants use the full `[0, 255]` range
/// (JFIF-style), which is what `VideoFrame`s decoded from JPEG and many camera
/// pipelines deliver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMatrix {
    /// BT.601 (SD) primaries, limited/studio range.
    Bt601Limited,
    /// BT.601 (SD) primaries, full range (JFIF).
    Bt601Full,
    /// BT.709 (HD) primaries, limited/studio range.
    Bt709Limited,
    /// BT.709 (HD) primaries, full range.
    Bt709Full,
    /// BT.2020 (UHD / wide gamut) primaries, limited/studio range.
    Bt2020Limited,
}

impl ColorMatrix {
    /// Returns `true` for the studio-swing (limited-range) variants.
    #[must_use]
    pub const fn is_limited_range(self) -> bool {
        matches!(
            self,
            Self::Bt601Limited | Self::Bt709Limited | Self::Bt2020Limited
        )
    }

    /// Luma primaries `(kr, kb)` for this matrix (`kg = 1 - kr - kb`).
    fn primaries(self) -> (f64, f64) {
        match self {
            Self::Bt601Limited | Self::Bt601Full => (0.299, 0.114),
            Self::Bt709Limited | Self::Bt709Full => (0.2126, 0.0722),
            Self::Bt2020Limited => (0.2627, 0.0593),
        }
    }
}

/// Rounds an `f64` coefficient to a `2^14`-scaled `i32`.
fn fx(value: f64) -> i32 {
    (value * f64::from(SCALE)).round() as i32
}

/// Fixed-point YCbCr -> RGB coefficients.
///
/// ```text
/// c = y_coef * (Y - y_offset)
/// R = c + cr_to_r * (Cr - 128)
/// G = c + cb_to_g * (Cb - 128) + cr_to_g * (Cr - 128)
/// B = c + cb_to_b * (Cb - 128)
/// ```
pub(crate) struct YuvToRgb {
    y_coef: i32,
    y_offset: i32,
    cr_to_r: i32,
    cb_to_g: i32,
    cr_to_g: i32,
    cb_to_b: i32,
}

impl YuvToRgb {
    pub(crate) fn for_matrix(matrix: ColorMatrix) -> Self {
        let (kr, kb) = matrix.primaries();
        let kg = 1.0 - kr - kb;

        let (y_coef, chroma_scale, y_offset) = if matrix.is_limited_range() {
            // Y' spans 219 codes over 16..235, chroma spans 224 over 16..240.
            (255.0 / 219.0, 255.0 / 224.0, 16)
        } else {
            (1.0, 1.0, 0)
        };

        Self {
            y_coef: fx(y_coef),
            y_offset,
            cr_to_r: fx(chroma_scale * 2.0 * (1.0 - kr)),
            cb_to_g: fx(-chroma_scale * 2.0 * kb * (1.0 - kb) / kg),
            cr_to_g: fx(-chroma_scale * 2.0 * kr * (1.0 - kr) / kg),
            cb_to_b: fx(chroma_scale * 2.0 * (1.0 - kb)),
        }
    }

    /// Converts one `(Y, Cb, Cr)` triplet to `(R, G, B)`, clamped to `[0, 255]`.
    #[inline]
    pub(crate) fn apply(&self, y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
        let c = (i32::from(y) - self.y_offset) * self.y_coef;
        let cb = i32::from(cb) - 128;
        let cr = i32::from(cr) - 128;

        let r = (c + self.cr_to_r * cr + ROUND) >> SCALE_BITS;
        let g = (c + self.cb_to_g * cb + self.cr_to_g * cr + ROUND) >> SCALE_BITS;
        let b = (c + self.cb_to_b * cb + ROUND) >> SCALE_BITS;

        (
            r.clamp(0, 255) as u8,
            g.clamp(0, 255) as u8,
            b.clamp(0, 255) as u8,
        )
    }
}

/// Fixed-point RGB -> YCbCr coefficients (the inverse of [`YuvToRgb`]).
pub(crate) struct RgbToYuv {
    y_r: i32,
    y_g: i32,
    y_b: i32,
    cb_r: i32,
    cb_g: i32,
    cb_b: i32,
    cr_r: i32,
    cr_g: i32,
    cr_b: i32,
    y_offset: i32,
    y_min: i32,
    y_max: i32,
    c_min: i32,
    c_max: i32,
}

impl RgbToYuv {
    pub(crate) fn for_matrix(matrix: ColorMatrix) -> Self {
        let (kr, kb) = matrix.primaries();
        let kg = 1.0 - kr - kb;

        let (y_scale, c_scale, y_offset, y_min, y_max, c_min, c_max) = if matrix.is_limited_range() {
            (219.0 / 255.0, 224.0 / 255.0, 16, 16, 235, 16, 240)
        } else {
            (1.0, 1.0, 0, 0, 255, 0, 255)
        };

        Self {
            y_r: fx(y_scale * kr),
            y_g: fx(y_scale * kg),
            y_b: fx(y_scale * kb),
            cb_r: fx(c_scale * -0.5 * kr / (1.0 - kb)),
            cb_g: fx(c_scale * -0.5 * kg / (1.0 - kb)),
            cb_b: fx(c_scale * 0.5),
            cr_r: fx(c_scale * 0.5),
            cr_g: fx(c_scale * -0.5 * kg / (1.0 - kr)),
            cr_b: fx(c_scale * -0.5 * kb / (1.0 - kr)),
            y_offset,
            y_min,
            y_max,
            c_min,
            c_max,
        }
    }

    /// Converts one `(R, G, B)` triplet to `(Y, Cb, Cr)` with range clamping.
    #[inline]
    pub(crate) fn apply(&self, r: u8, g: u8, b: u8) -> (u8, u8, u8) {
        let ri = i32::from(r);
        let gi = i32::from(g);
        let bi = i32::from(b);

        let y = ((self.y_r * ri + self.y_g * gi + self.y_b * bi + ROUND) >> SCALE_BITS)
            + self.y_offset;
        let cb =
            ((self.cb_r * ri + self.cb_g * gi + self.cb_b * bi + ROUND) >> SCALE_BITS) + 128;
        let cr =
            ((self.cr_r * ri + self.cr_g * gi + self.cr_b * bi + ROUND) >> SCALE_BITS) + 128;

        (
            y.clamp(self.y_min, self.y_max) as u8,
            cb.clamp(self.c_min, self.c_max) as u8,
            cr.clamp(self.c_min, self.c_max) as u8,
        )
    }

    /// Converts one `(R, G, B)` triplet to just the luma `Y`, range-clamped.
    #[inline]
    pub(crate) fn luma(&self, r: u8, g: u8, b: u8) -> u8 {
        let ri = i32::from(r);
        let gi = i32::from(g);
        let bi = i32::from(b);
        let y = ((self.y_r * ri + self.y_g * gi + self.y_b * bi + ROUND) >> SCALE_BITS)
            + self.y_offset;
        y.clamp(self.y_min, self.y_max) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATRICES: [ColorMatrix; 5] = [
        ColorMatrix::Bt601Limited,
        ColorMatrix::Bt601Full,
        ColorMatrix::Bt709Limited,
        ColorMatrix::Bt709Full,
        ColorMatrix::Bt2020Limited,
    ];

    #[test]
    fn is_limited_range_flags() {
        assert!(ColorMatrix::Bt601Limited.is_limited_range());
        assert!(!ColorMatrix::Bt601Full.is_limited_range());
        assert!(ColorMatrix::Bt709Limited.is_limited_range());
        assert!(!ColorMatrix::Bt709Full.is_limited_range());
        assert!(ColorMatrix::Bt2020Limited.is_limited_range());
    }

    #[test]
    fn full_range_bt601_matches_jfif_coefficients() {
        // JFIF forward: Cb from R = -0.168736, Cr from B = -0.081312.
        let f = RgbToYuv::for_matrix(ColorMatrix::Bt601Full);
        assert_eq!(f.cb_r, fx(-0.168_736));
        assert_eq!(f.cr_b, fx(-0.081_312));
        assert_eq!(f.cb_b, fx(0.5));
        assert_eq!(f.cr_r, fx(0.5));
    }

    #[test]
    fn white_black_round_trip_all_matrices() {
        for m in MATRICES {
            let fwd = RgbToYuv::for_matrix(m);
            let inv = YuvToRgb::for_matrix(m);

            let (y, cb, cr) = fwd.apply(255, 255, 255);
            let (r, g, b) = inv.apply(y, cb, cr);
            assert!(r >= 253 && g >= 253 && b >= 253, "{m:?} white -> {r},{g},{b}");

            let (y, cb, cr) = fwd.apply(0, 0, 0);
            let (r, g, b) = inv.apply(y, cb, cr);
            assert!(r <= 2 && g <= 2 && b <= 2, "{m:?} black -> {r},{g},{b}");
        }
    }

    #[test]
    fn limited_range_clamps_luma() {
        let fwd = RgbToYuv::for_matrix(ColorMatrix::Bt709Limited);
        let (y_white, _, _) = fwd.apply(255, 255, 255);
        let (y_black, _, _) = fwd.apply(0, 0, 0);
        assert_eq!(y_black, 16);
        assert_eq!(y_white, 235);
    }
}
