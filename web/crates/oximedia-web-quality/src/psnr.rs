// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! PSNR (Peak Signal-to-Noise Ratio) kernels.
//!
//! ```text
//! PSNR = 10 * log10(MAX^2 / MSE)
//! ```
//!
//! `MAX` is `255` (8-bit code value); `MSE` is the mean squared error
//! between the reference and distorted signal. Accumulation uses `f64`
//! internally for precision (the result is a single scalar, never a
//! per-frame buffer), but every input/output buffer stays `u8`.
//!
//! Unlike the native `oximedia-quality` port this crate deliberately does
//! **not** clamp a near-zero MSE to a finite "very high" PSNR: bit-identical
//! inputs return `f64::INFINITY` exactly, which `wasm-bindgen` marshals to
//! JavaScript's `Infinity`.
//!
//! Canonical source: `crates/oximedia-quality/src/psnr.rs` (constants and
//! the MSE-to-PSNR formula only; the loop structure and the rayon-parallel
//! plane/ROI machinery are not ported — see `web/TODO.md` M5 for the port
//! strategy).

use oximedia_web_core::frame::{validate_rgba8, RGBA_CHANNELS};

use crate::error::{QualityError, Result};
use oximedia_web_core::CoreError;

/// Peak (maximum) code value for 8-bit content.
const U8_PEAK: f64 = 255.0;

/// Computes PSNR (dB) over the R, G, B channels of two tightly packed RGBA8
/// buffers, ignoring alpha.
///
/// Returns `f64::INFINITY` when the RGB channels are bit-identical.
///
/// # Errors
///
/// Returns [`QualityError::Core`] if either buffer's length does not equal
/// `width * height * 4`.
pub fn psnr_rgb(reference: &[u8], distorted: &[u8], width: usize, height: usize) -> Result<f64> {
    validate_rgba8(reference, width, height)?;
    validate_rgba8(distorted, width, height)?;

    let mut sum_sq = 0.0f64;
    let mut count = 0u64;
    for (ref_px, dist_px) in reference
        .chunks_exact(RGBA_CHANNELS)
        .zip(distorted.chunks_exact(RGBA_CHANNELS))
    {
        for channel in 0..3 {
            let diff = f64::from(ref_px[channel]) - f64::from(dist_px[channel]);
            sum_sq += diff * diff;
            count += 1;
        }
    }
    Ok(mse_to_psnr(sum_sq, count))
}

/// Computes PSNR (dB) between two equal-length 8-bit luma planes.
///
/// Returns `f64::INFINITY` when the two planes are bit-identical.
///
/// This is the zero-allocation building block used by
/// [`crate::QualityAnalyzer`]: the caller is expected to have already
/// extracted the luma planes (e.g. via
/// [`oximedia_web_core::yuv::rgba8_to_luma_into`] with
/// [`oximedia_web_core::ColorMatrix::Bt709Full`]) into preallocated scratch.
///
/// # Errors
///
/// Returns [`QualityError::Core`] ([`CoreError::LengthMismatch`]) if the two
/// planes differ in length.
pub fn psnr_luma(reference: &[u8], distorted: &[u8]) -> Result<f64> {
    if reference.len() != distorted.len() {
        return Err(QualityError::Core(CoreError::LengthMismatch {
            left: reference.len(),
            right: distorted.len(),
        }));
    }

    let mut sum_sq = 0.0f64;
    for (r, d) in reference.iter().zip(distorted.iter()) {
        let diff = f64::from(*r) - f64::from(*d);
        sum_sq += diff * diff;
    }
    Ok(mse_to_psnr(sum_sq, reference.len() as u64))
}

/// Converts an accumulated sum-of-squared-differences and sample count to a
/// PSNR value in dB, returning `f64::INFINITY` for a zero MSE (including the
/// degenerate zero-sample case).
fn mse_to_psnr(sum_sq: f64, count: u64) -> f64 {
    if count == 0 {
        return f64::INFINITY;
    }
    let mse = sum_sq / count as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (U8_PEAK * U8_PEAK / mse).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_rgba(width: usize, height: usize, value: u8) -> Vec<u8> {
        let mut buf = vec![255u8; width * height * RGBA_CHANNELS];
        for px in buf.chunks_exact_mut(RGBA_CHANNELS) {
            px[0] = value;
            px[1] = value;
            px[2] = value;
        }
        buf
    }

    #[test]
    fn identical_rgb_is_infinite() {
        let a = solid_rgba(8, 8, 100);
        let b = a.clone();
        assert_eq!(psnr_rgb(&a, &b, 8, 8).unwrap(), f64::INFINITY);
    }

    #[test]
    fn identical_luma_is_infinite() {
        let a = vec![42u8; 64];
        let b = a.clone();
        assert_eq!(psnr_luma(&a, &b).unwrap(), f64::INFINITY);
    }

    #[test]
    fn known_small_distortion_matches_formula() {
        // Every u8 channel shifted by +2 => constant squared diff of 4.
        // PSNR = 10*log10(255^2/4) ~= 42.1105 dB.
        let a = solid_rgba(16, 16, 100);
        let mut b = a.clone();
        for px in b.chunks_exact_mut(RGBA_CHANNELS) {
            px[0] += 2;
            px[1] += 2;
            px[2] += 2;
        }
        let psnr = psnr_rgb(&a, &b, 16, 16).unwrap();
        assert!(
            (psnr - 42.1105).abs() < 0.2,
            "expected ~42.1 dB, got {psnr}"
        );
    }

    #[test]
    fn known_small_distortion_luma_matches_formula() {
        let a = vec![100u8; 256];
        let b: Vec<u8> = a.iter().map(|v| v + 2).collect();
        let psnr = psnr_luma(&a, &b).unwrap();
        assert!(
            (psnr - 42.1105).abs() < 0.2,
            "expected ~42.1 dB, got {psnr}"
        );
    }

    #[test]
    fn mismatched_rgb_dims_errors() {
        let a = solid_rgba(8, 8, 10);
        let b = solid_rgba(4, 4, 10);
        assert!(matches!(
            psnr_rgb(&a, &b, 8, 8),
            Err(QualityError::Core(CoreError::BufferLength { .. }))
        ));
    }

    #[test]
    fn mismatched_luma_dims_errors() {
        let a = vec![0u8; 16];
        let b = vec![0u8; 20];
        assert!(matches!(
            psnr_luma(&a, &b),
            Err(QualityError::Core(CoreError::LengthMismatch { .. }))
        ));
    }

    #[test]
    fn worst_case_black_vs_white_is_zero_db() {
        let a = solid_rgba(4, 4, 0);
        let b = solid_rgba(4, 4, 255);
        let psnr = psnr_rgb(&a, &b, 4, 4).unwrap();
        assert!(psnr.abs() < 1e-9, "expected 0 dB, got {psnr}");
    }
}
