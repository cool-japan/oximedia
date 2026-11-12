// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `u8` <-> `f32` normalization and the sRGB transfer functions.
//!
//! The `_into` slice variants are allocation-free and written as
//! `iter().zip()` loops so LLVM auto-vectorizes them under `+simd128`. The
//! scalar variants are `#[inline]` for use inside per-pixel kernels.

use crate::error::CoreError;

/// Normalizes a `u8` code value to `[0.0, 1.0]`.
#[inline]
#[must_use]
pub fn u8_to_f32(value: u8) -> f32 {
    f32::from(value) / 255.0
}

/// Converts a normalized `f32` back to a `u8` with saturating round-to-nearest.
///
/// Values outside `[0.0, 1.0]` are clamped before rounding; `NaN` maps to `0`.
///
/// Written as `+0.5` + saturating truncation rather than `f32::round`:
/// round-half-away for non-negative inputs is `trunc(v + 0.5)`, and `NaN`
/// propagates through the arithmetic into the saturating cast's `0`. The
/// `round` form lowers to a scalar libm `roundf` call on wasm32 (Rust's
/// half-away-from-zero semantics don't match `f32.nearest`'s
/// half-to-even), which blocks autovectorization of every conversion sweep
/// built on this helper; this form lowers to `i32.trunc_sat_f32_u`.
#[inline]
#[must_use]
pub fn f32_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Normalizes a `u8` slice into a caller-provided `f32` slice.
///
/// # Errors
///
/// [`CoreError::LengthMismatch`] if `src.len() != dst.len()`.
pub fn u8_to_f32_into(src: &[u8], dst: &mut [f32]) -> Result<(), CoreError> {
    if src.len() != dst.len() {
        return Err(CoreError::LengthMismatch {
            left: src.len(),
            right: dst.len(),
        });
    }
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d = f32::from(s) / 255.0;
    }
    Ok(())
}

/// Converts an `f32` slice into a caller-provided `u8` slice, clamping and
/// rounding each element.
///
/// # Errors
///
/// [`CoreError::LengthMismatch`] if `src.len() != dst.len()`.
pub fn f32_to_u8_into(src: &[f32], dst: &mut [u8]) -> Result<(), CoreError> {
    if src.len() != dst.len() {
        return Err(CoreError::LengthMismatch {
            left: src.len(),
            right: dst.len(),
        });
    }
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d = f32_to_u8(s);
    }
    Ok(())
}

/// sRGB EOTF: decodes a gamma-encoded sRGB value to linear light.
///
/// Input and output are in `[0.0, 1.0]`. This is the standard piecewise curve
/// (linear segment below `0.04045`, `2.4`-power segment above).
#[inline]
#[must_use]
pub fn srgb_eotf(value: f32) -> f32 {
    if value <= 0.040_448_237 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB OETF: encodes a linear-light value to gamma-encoded sRGB.
///
/// Input and output are in `[0.0, 1.0]`. This is the inverse of [`srgb_eotf`].
#[inline]
#[must_use]
pub fn srgb_oetf(value: f32) -> f32 {
    if value <= 0.003_130_668_5 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

/// Applies [`srgb_eotf`] element-wise into a caller-provided buffer.
///
/// # Errors
///
/// [`CoreError::LengthMismatch`] if `src.len() != dst.len()`.
pub fn srgb_eotf_into(src: &[f32], dst: &mut [f32]) -> Result<(), CoreError> {
    if src.len() != dst.len() {
        return Err(CoreError::LengthMismatch {
            left: src.len(),
            right: dst.len(),
        });
    }
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d = srgb_eotf(s);
    }
    Ok(())
}

/// Applies [`srgb_oetf`] element-wise into a caller-provided buffer.
///
/// # Errors
///
/// [`CoreError::LengthMismatch`] if `src.len() != dst.len()`.
pub fn srgb_oetf_into(src: &[f32], dst: &mut [f32]) -> Result<(), CoreError> {
    if src.len() != dst.len() {
        return Err(CoreError::LengthMismatch {
            left: src.len(),
            right: dst.len(),
        });
    }
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d = srgb_oetf(s);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_boundaries() {
        assert!((u8_to_f32(0) - 0.0).abs() < 1e-6);
        assert!((u8_to_f32(255) - 1.0).abs() < 1e-6);
        assert_eq!(f32_to_u8(0.0), 0);
        assert_eq!(f32_to_u8(1.0), 255);
        assert_eq!(f32_to_u8(-0.5), 0);
        assert_eq!(f32_to_u8(1.5), 255);
        assert_eq!(f32_to_u8(f32::NAN), 0);
    }

    #[test]
    fn slice_length_mismatch_errors() {
        let src = [0u8; 3];
        let mut dst = [0.0f32; 4];
        assert!(matches!(
            u8_to_f32_into(&src, &mut dst),
            Err(CoreError::LengthMismatch { .. })
        ));
    }

    #[test]
    fn u8_f32_round_trip_within_one() {
        let src: Vec<u8> = (0u16..=255).map(|v| v as u8).collect();
        let mut f = vec![0.0f32; src.len()];
        u8_to_f32_into(&src, &mut f).unwrap();
        let mut back = vec![0u8; src.len()];
        f32_to_u8_into(&f, &mut back).unwrap();
        for (a, b) in src.iter().zip(back.iter()) {
            assert!((i16::from(*a) - i16::from(*b)).abs() <= 1);
        }
    }

    #[test]
    fn srgb_round_trip() {
        for i in 0..=100 {
            let x = i as f32 / 100.0;
            let back = srgb_oetf(srgb_eotf(x));
            assert!((x - back).abs() < 1e-4, "x={x} back={back}");
        }
    }

    #[test]
    fn srgb_known_anchors() {
        assert!((srgb_eotf(0.0)).abs() < 1e-6);
        assert!((srgb_eotf(1.0) - 1.0).abs() < 1e-4);
        assert!((srgb_oetf(0.0)).abs() < 1e-6);
        assert!((srgb_oetf(1.0) - 1.0).abs() < 1e-4);
        // Mid grey: sRGB 0.5 decodes to roughly 0.214 linear.
        assert!((srgb_eotf(0.5) - 0.214).abs() < 0.01);
    }

    #[test]
    fn srgb_into_matches_scalar() {
        let src: Vec<f32> = (0..=50).map(|i| i as f32 / 50.0).collect();
        let mut dst = vec![0.0f32; src.len()];
        srgb_eotf_into(&src, &mut dst).unwrap();
        for (s, d) in src.iter().zip(dst.iter()) {
            assert!((srgb_eotf(*s) - d).abs() < 1e-7);
        }
    }
}
