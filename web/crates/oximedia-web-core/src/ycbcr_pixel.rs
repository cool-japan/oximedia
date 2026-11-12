// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Per-pixel RGB -> YCbCr helpers for the scopes module (waveform, vectorscope,
//! histogram).
//!
//! These are **bit-exact** ports of the fixed-point kernels the native
//! `oximedia-scopes` crate uses for its analysis scopes
//! (`crates/oximedia-scopes/src/simd_convert.rs`): full-range (0..255) Y with a
//! `+128` bias on Cb/Cr, coefficients scaled by `2^15` with a `2^14` rounding
//! bias. Keeping them bit-for-bit identical means a scope rendered in the
//! browser lines up with one rendered natively.
//!
//! Each helper is `#[inline]` and returns a packed `[Y, Cb, Cr]` so the scopes
//! crate can call it in a tight per-pixel loop without allocation.

/// `2^15` fixed-point scale used by all three coefficient sets.
const FIX_ONE: i32 = 1 << 15;
/// Rounding bias (`2^14`) added before the `>> 15` shift.
const FIX_ROUND: i32 = 1 << 14;

#[inline]
fn dot_shift(r: i32, g: i32, b: i32, kr: i32, kg: i32, kb: i32) -> i32 {
    (kr * r + kg * g + kb * b + FIX_ROUND) >> 15
}

/// Converts one RGB pixel to BT.601 full-range YCbCr (`+128` chroma bias).
///
/// This is the coefficient set the native vectorscope/waveform analysis path
/// uses (`convert_batch_bt601_simd`). Neutral grey maps to `Cb = Cr = 128`.
///
/// ```text
/// Y  =  0.299   R + 0.587   G + 0.114   B
/// Cb = -0.16874 R - 0.33126 G + 0.5     B  (+128)
/// Cr =  0.5     R - 0.41869 G - 0.08131 B  (+128)
/// ```
#[inline]
#[must_use]
pub fn rgb_to_ycbcr_bt601(r: u8, g: u8, b: u8) -> [u8; 3] {
    let (ri, gi, bi) = (i32::from(r), i32::from(g), i32::from(b));
    let y = dot_shift(ri, gi, bi, 9798, 19235, 3736);
    let cb = dot_shift(ri, gi, bi, -5529, -10855, FIX_ONE / 2);
    let cr = dot_shift(ri, gi, bi, FIX_ONE / 2, -13717, -2664);
    [
        y.clamp(0, 255) as u8,
        (cb + 128).clamp(0, 255) as u8,
        (cr + 128).clamp(0, 255) as u8,
    ]
}

/// Converts one RGB pixel to BT.709 full-range YCbCr (`+128` chroma bias).
///
/// ```text
/// Y  =  0.2126 R + 0.7152 G + 0.0722 B
/// Cb = -0.1146 R - 0.3854 G + 0.5    B  (+128)
/// Cr =  0.5    R - 0.4542 G - 0.0458 B  (+128)
/// ```
#[inline]
#[must_use]
pub fn rgb_to_ycbcr_bt709(r: u8, g: u8, b: u8) -> [u8; 3] {
    let (ri, gi, bi) = (i32::from(r), i32::from(g), i32::from(b));
    let y = dot_shift(ri, gi, bi, 6967, 23434, 2367);
    let cb = dot_shift(ri, gi, bi, -3755, -12629, FIX_ONE / 2);
    let cr = dot_shift(ri, gi, bi, FIX_ONE / 2, -14882, -1502);
    [
        y.clamp(0, 255) as u8,
        (cb + 128).clamp(0, 255) as u8,
        (cr + 128).clamp(0, 255) as u8,
    ]
}

/// Converts one RGB pixel to BT.2020 full-range YCbCr (`+128` chroma bias).
///
/// Used by the HDR / wide-gamut scopes.
#[inline]
#[must_use]
pub fn rgb_to_ycbcr_bt2020(r: u8, g: u8, b: u8) -> [u8; 3] {
    let (ri, gi, bi) = (i32::from(r), i32::from(g), i32::from(b));
    let y = dot_shift(ri, gi, bi, 8610, 22216, 1943);
    let cb = dot_shift(ri, gi, bi, -4574, -11810, FIX_ONE / 2);
    let cr = dot_shift(ri, gi, bi, FIX_ONE / 2, -15066, -1317);
    [
        y.clamp(0, 255) as u8,
        (cb + 128).clamp(0, 255) as u8,
        (cr + 128).clamp(0, 255) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_is_max_luma_neutral_chroma() {
        for f in [rgb_to_ycbcr_bt601, rgb_to_ycbcr_bt709, rgb_to_ycbcr_bt2020] {
            let [y, cb, cr] = f(255, 255, 255);
            assert_eq!(y, 255);
            assert!((i32::from(cb) - 128).abs() <= 1);
            assert!((i32::from(cr) - 128).abs() <= 1);
        }
    }

    #[test]
    fn black_is_zero_luma_neutral_chroma() {
        for f in [rgb_to_ycbcr_bt601, rgb_to_ycbcr_bt709, rgb_to_ycbcr_bt2020] {
            let [y, cb, cr] = f(0, 0, 0);
            assert_eq!(y, 0);
            assert!((i32::from(cb) - 128).abs() <= 1);
            assert!((i32::from(cr) - 128).abs() <= 1);
        }
    }

    #[test]
    fn neutral_grey_maps_to_128_chroma() {
        let [_, cb, cr] = rgb_to_ycbcr_bt601(128, 128, 128);
        assert_eq!(cb, 128);
        assert_eq!(cr, 128);
    }

    #[test]
    fn primary_red_pushes_cr_high() {
        // Pure red should push Cr well above neutral in every matrix.
        for f in [rgb_to_ycbcr_bt601, rgb_to_ycbcr_bt709, rgb_to_ycbcr_bt2020] {
            let [_, _, cr] = f(255, 0, 0);
            assert!(cr > 200, "cr={cr}");
        }
    }

    #[test]
    fn bt601_matches_reference_constants() {
        // Exact values from the upstream rgb_to_ycbcr_bt601_pixel path.
        assert_eq!(rgb_to_ycbcr_bt601(255, 0, 0), [76, 85, 255]);
    }
}
