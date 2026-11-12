// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `wasm-bindgen` glue exposing [`QualityAnalyzer`] to JavaScript as
//! [`Quality`]. No JSON on the per-frame path: [`QualityResult`] exposes its
//! fields as typed getters, and [`Quality::ssim_map`] writes its heatmap
//! into a caller-provided `&mut [u8]` out-param.

use wasm_bindgen::prelude::*;

use crate::analyzer::{Metrics, QualityAnalyzer};

/// Converts any [`crate::QualityError`] into a `wasm-bindgen` `JsError`.
fn to_js_error(err: crate::QualityError) -> JsError {
    JsError::new(&err.to_string())
}

/// PSNR/SSIM comparison result. Exposed to JS via typed getters (`number`,
/// never JSON) so a steady-state comparison loop allocates no intermediate
/// objects beyond this one small wrapper.
#[wasm_bindgen]
#[derive(Clone, Copy, Debug)]
pub struct QualityResult {
    metrics: Metrics,
}

#[wasm_bindgen]
impl QualityResult {
    /// PSNR (dB) over the RGB channels of the two RGBA8 frames (alpha
    /// ignored). `Infinity` for bit-identical inputs.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn psnr_rgb(&self) -> f64 {
        self.metrics.psnr_rgb
    }

    /// PSNR (dB) over the BT.709 full-range luma plane. `Infinity` for
    /// bit-identical inputs.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn psnr_luma(&self) -> f64 {
        self.metrics.psnr_luma
    }

    /// Mean windowed SSIM (`1.0` for bit-identical inputs).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn ssim(&self) -> f64 {
        self.metrics.ssim
    }
}

/// PSNR + SSIM analyzer bound to a fixed `width x height` RGBA8 frame size.
///
/// Every working buffer is preallocated in the constructor; `compare` and
/// `ssim_map` never allocate beyond the one small [`QualityResult`] the
/// former returns.
#[wasm_bindgen]
pub struct Quality {
    inner: QualityAnalyzer,
}

#[wasm_bindgen]
impl Quality {
    /// Creates an analyzer for `width x height` RGBA8 frames.
    ///
    /// # Errors
    ///
    /// Throws if `width`/`height` is zero, or smaller than the `11`-pixel
    /// SSIM window.
    #[wasm_bindgen(constructor)]
    pub fn new(width: u32, height: u32) -> Result<Quality, JsError> {
        let inner =
            QualityAnalyzer::new(width as usize, height as usize).map_err(to_js_error)?;
        Ok(Self { inner })
    }

    /// Computes PSNR (RGB + luma) and mean SSIM between two RGBA8 frames.
    ///
    /// `reference` and `distorted` must each be exactly
    /// `width * height * 4` bytes.
    ///
    /// # Errors
    ///
    /// Throws if either buffer has the wrong length.
    pub fn compare(&mut self, reference: &[u8], distorted: &[u8]) -> Result<QualityResult, JsError> {
        let metrics = self
            .inner
            .compare_rgba8(reference, distorted)
            .map_err(to_js_error)?;
        Ok(QualityResult { metrics })
    }

    /// Renders a per-pixel SSIM heatmap (RGBA8, red = dissimilar, green =
    /// similar) between two RGBA8 frames into `out` and returns the mean
    /// SSIM. `reference`, `distorted`, and `out` must each be exactly
    /// `width * height * 4` bytes.
    ///
    /// # Errors
    ///
    /// Throws if any buffer has the wrong length.
    pub fn ssim_map(
        &mut self,
        reference: &[u8],
        distorted: &[u8],
        out: &mut [u8],
    ) -> Result<f64, JsError> {
        self.inner
            .ssim_map_into(reference, distorted, out)
            .map_err(to_js_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_web_core::frame::RGBA_CHANNELS;

    fn solid(width: usize, height: usize, value: u8) -> Vec<u8> {
        let mut buf = vec![255u8; width * height * RGBA_CHANNELS];
        for px in buf.chunks_exact_mut(RGBA_CHANNELS) {
            px[0] = value;
            px[1] = value;
            px[2] = value;
        }
        buf
    }

    // `Quality::new`/`compare`/`ssim_map` map their `Err` path through
    // `JsError::new`, which calls a real `wasm-bindgen` JS import
    // (`Error::new`) — that import panics ("cannot call wasm-bindgen
    // imported functions on non-wasm targets") when actually invoked under
    // plain `cargo test` on a native target. The underlying error paths
    // (zero/too-small dimensions, mismatched buffer lengths) are already
    // covered exhaustively against [`QualityAnalyzer`] and [`SsimKernel`]
    // directly in `analyzer.rs` / `ssim.rs` / `psnr.rs`; only the
    // success-path JS surface is exercised here. `web/scripts/build.sh`
    // plus a browser/`wasm-bindgen-test` run is the place to exercise the
    // `JsError` conversion itself.

    #[test]
    fn quality_compare_round_trip() {
        let mut q = Quality::new(16, 16).unwrap();
        let a = solid(16, 16, 50);
        let b = solid(16, 16, 60);
        let result = q.compare(&a, &b).unwrap();
        assert!(result.psnr_rgb() > 0.0);
        assert!(result.psnr_luma() > 0.0);
        assert!((0.0..=1.0).contains(&result.ssim()));
    }

    #[test]
    fn quality_compare_identical_is_infinite_and_unit() {
        let mut q = Quality::new(16, 16).unwrap();
        let a = solid(16, 16, 128);
        let result = q.compare(&a, &a).unwrap();
        assert_eq!(result.psnr_rgb(), f64::INFINITY);
        assert_eq!(result.psnr_luma(), f64::INFINITY);
        assert_eq!(result.ssim(), 1.0);
    }

    #[test]
    fn quality_ssim_map_round_trip() {
        let mut q = Quality::new(16, 16).unwrap();
        let a = solid(16, 16, 40);
        let b = solid(16, 16, 45);
        let mut out = vec![0u8; 16 * 16 * RGBA_CHANNELS];
        let mean = q.ssim_map(&a, &b, &mut out).unwrap();
        assert!((0.0..=1.0).contains(&mean));
        assert!(out.chunks_exact(RGBA_CHANNELS).all(|px| px[3] == 255));
    }
}
