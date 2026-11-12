// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Separable resampling filter kernels.
//!
//! These are ported (not depended on) from
//! `crates/oximedia-scaling/src/resampler.rs`'s `FilterKernel::{support,
//! evaluate}` for the four kernels this crate supports. One naming
//! correction versus the upstream source: upstream's `FilterKernel::Bicubic`
//! doc comment mislabels `mitchell_netravali(x, B=0.0, C=0.5)` as
//! "Mitchell-Netravali" — that `(B=0, C=0.5)` parameterization is the
//! Catmull-Rom cubic, not Mitchell-Netravali (which is `B=C=1/3`). This
//! crate names the two kernels correctly: [`Filter::CatmullRom`] for
//! `(B=0, C=0.5)` and [`Filter::Mitchell`] for `(B=1/3, C=1/3)`.

use core::f32::consts::PI;

use crate::error::ScaleError;

/// A separable resampling filter kernel.
///
/// Each variant is a 1D kernel evaluated independently on the horizontal and
/// vertical axes ([`crate::weights::WeightTable`]); the 2D result is their
/// tensor product, which is exact for these kernels since none of them are
/// radially symmetric non-separable designs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Linear tent filter, support radius 1. Fast, soft; adequate for small
    /// scale factors or when speed matters more than sharpness.
    Bilinear,
    /// Cubic B-spline family member `(B=0, C=0.5)`, support radius 2.
    /// Interpolating (passes through source samples on upscale), mild
    /// ringing. Correct name for what upstream `oximedia-scaling` calls
    /// `FilterKernel::Bicubic`.
    CatmullRom,
    /// Cubic B-spline family member `(B=1/3, C=1/3)`, support radius 2.
    /// Mitchell & Netravali's "no ringing, no blur, no aliasing please"
    /// compromise; matches `FilterKernel::MitchellNetravali` upstream.
    Mitchell,
    /// Windowed-sinc filter, support radius 3. The sharpest of the four,
    /// with the strongest ringing on hard edges; the default for the JS
    /// wrapper.
    Lanczos3,
}

impl Filter {
    /// Parses a filter selector string as used by the `#[wasm_bindgen]`
    /// `Scaler` constructor and the JS wrapper.
    ///
    /// Accepts exactly `"bilinear"`, `"catmull-rom"`, `"mitchell"` or
    /// `"lanczos3"`.
    ///
    /// # Errors
    ///
    /// Returns [`ScaleError::UnknownFilter`] for any other string.
    pub fn parse(name: &str) -> Result<Self, ScaleError> {
        match name {
            "bilinear" => Ok(Self::Bilinear),
            "catmull-rom" => Ok(Self::CatmullRom),
            "mitchell" => Ok(Self::Mitchell),
            "lanczos3" => Ok(Self::Lanczos3),
            other => Err(ScaleError::UnknownFilter {
                name: other.to_owned(),
            }),
        }
    }

    /// The kernel's native support radius: [`Self::evaluate`] returns `0.0`
    /// for any `|x|` at or beyond this radius.
    #[must_use]
    pub const fn support(self) -> f32 {
        match self {
            Self::Bilinear => 1.0,
            Self::CatmullRom | Self::Mitchell => 2.0,
            Self::Lanczos3 => 3.0,
        }
    }

    /// Evaluates the kernel at `x` (distance from the sample center, in
    /// source-sample units, already divided by any downscale `filter_scale`
    /// factor by the caller — see [`crate::weights::WeightTable::build`]).
    #[must_use]
    pub fn evaluate(self, x: f32) -> f32 {
        match self {
            Self::Bilinear => (1.0 - x.abs()).max(0.0),
            Self::CatmullRom => mitchell_netravali(x, 0.0, 0.5),
            Self::Mitchell => mitchell_netravali(x, 1.0 / 3.0, 1.0 / 3.0),
            Self::Lanczos3 => lanczos3(x),
        }
    }
}

/// Normalized sinc: `sin(pi*x) / (pi*x)`, with the removable singularity at
/// `x=0` filled in as `1.0`.
#[inline]
fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-8 {
        1.0
    } else {
        (PI * x).sin() / (PI * x)
    }
}

/// Lanczos-windowed sinc with a fixed radius-3 window.
#[inline]
fn lanczos3(x: f32) -> f32 {
    let ax = x.abs();
    if ax >= 3.0 {
        0.0
    } else if ax < 1e-8 {
        1.0
    } else {
        sinc(ax) * sinc(ax / 3.0)
    }
}

/// The two-piece cubic B-spline family (Mitchell & Netravali 1988),
/// parameterized by `b` and `c`. `(B=0, C=0.5)` is Catmull-Rom; `(B=C=1/3)`
/// is the "Mitchell-Netravali" filter as usually cited.
#[inline]
fn mitchell_netravali(x: f32, b: f32, c: f32) -> f32 {
    let ax = x.abs();
    if ax < 1.0 {
        ((12.0 - 9.0 * b - 6.0 * c) * ax.powi(3)
            + (-18.0 + 12.0 * b + 6.0 * c) * ax.powi(2)
            + (6.0 - 2.0 * b))
            / 6.0
    } else if ax < 2.0 {
        ((-b - 6.0 * c) * ax.powi(3)
            + (6.0 * b + 30.0 * c) * ax.powi(2)
            + (-12.0 * b - 48.0 * c) * ax
            + (8.0 * b + 24.0 * c))
            / 6.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_all_four_names() {
        assert_eq!(Filter::parse("bilinear"), Ok(Filter::Bilinear));
        assert_eq!(Filter::parse("catmull-rom"), Ok(Filter::CatmullRom));
        assert_eq!(Filter::parse("mitchell"), Ok(Filter::Mitchell));
        assert_eq!(Filter::parse("lanczos3"), Ok(Filter::Lanczos3));
    }

    #[test]
    fn parse_rejects_unknown_name() {
        let err = Filter::parse("bogus").unwrap_err();
        assert_eq!(
            err,
            ScaleError::UnknownFilter {
                name: "bogus".to_owned()
            }
        );
    }

    #[test]
    fn support_radii_match_upstream() {
        assert!((Filter::Bilinear.support() - 1.0).abs() < f32::EPSILON);
        assert!((Filter::CatmullRom.support() - 2.0).abs() < f32::EPSILON);
        assert!((Filter::Mitchell.support() - 2.0).abs() < f32::EPSILON);
        assert!((Filter::Lanczos3.support() - 3.0).abs() < f32::EPSILON);
    }

    #[test]
    fn bilinear_center_and_edge() {
        assert!((Filter::Bilinear.evaluate(0.0) - 1.0).abs() < f32::EPSILON);
        assert!((Filter::Bilinear.evaluate(1.0)).abs() < f32::EPSILON);
        assert!((Filter::Bilinear.evaluate(2.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn lanczos3_center_and_outside_support() {
        assert!((Filter::Lanczos3.evaluate(0.0) - 1.0).abs() < 1e-5);
        assert!((Filter::Lanczos3.evaluate(3.5)).abs() < f32::EPSILON);
        assert!((Filter::Lanczos3.evaluate(-3.5)).abs() < f32::EPSILON);
    }

    #[test]
    fn lanczos3_is_symmetric() {
        for i in 0..30 {
            let x = i as f32 * 0.1;
            assert!((Filter::Lanczos3.evaluate(x) - Filter::Lanczos3.evaluate(-x)).abs() < 1e-6);
        }
    }

    #[test]
    fn mitchell_center_matches_closed_form() {
        // MN with B=1/3, C=1/3: peak = (6 - 2B)/6 = (6 - 2/3)/6.
        let expected = (6.0 - 2.0 / 3.0) / 6.0;
        assert!((Filter::Mitchell.evaluate(0.0) - expected).abs() < 0.001);
    }

    #[test]
    fn catmull_rom_is_interpolating_at_integer_offsets() {
        // Catmull-Rom (B=0, C=0.5) is interpolating: it passes exactly
        // through 0 at x = +/-1 and +/-2 (unit sample spacing) other than
        // the center, which is exactly 1.
        assert!((Filter::CatmullRom.evaluate(0.0) - 1.0).abs() < 1e-5);
        assert!((Filter::CatmullRom.evaluate(1.0)).abs() < 1e-5);
        assert!((Filter::CatmullRom.evaluate(2.0)).abs() < 1e-5);
    }

    #[test]
    fn all_kernels_are_symmetric_and_bounded_by_support() {
        for filter in [
            Filter::Bilinear,
            Filter::CatmullRom,
            Filter::Mitchell,
            Filter::Lanczos3,
        ] {
            let support = filter.support();
            assert!((filter.evaluate(support)).abs() < 1e-4, "{filter:?}");
            assert!((filter.evaluate(support + 0.1)).abs() < 1e-6, "{filter:?}");
            for i in 0..20 {
                let x = i as f32 * support / 20.0;
                assert!(
                    (filter.evaluate(x) - filter.evaluate(-x)).abs() < 1e-5,
                    "{filter:?} not symmetric at x={x}"
                );
            }
        }
    }
}
