// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! [`QualityAnalyzer`]: the preallocated, zero-per-frame-allocation PSNR +
//! SSIM comparator that both the native `#[cfg(test)]` suite and the
//! `wasm-bindgen` glue in [`crate::wasm`] are built on.

use oximedia_web_core::frame::{validate_rgba8, FrameDims};
use oximedia_web_core::matrix::ColorMatrix;
use oximedia_web_core::normalize::u8_to_f32_into;
use oximedia_web_core::yuv::rgba8_to_luma_into;

use crate::error::Result;
use crate::psnr;
use crate::ssim::SsimKernel;

/// The color matrix used to derive luma from RGBA8 for both `psnr_luma` and
/// SSIM: BT.709 primaries, full `[0, 255]` range (this crate compares
/// arbitrary decoded pixels, not broadcast-legal studio-swing video, so full
/// range is the more useful default — see [`crate`] docs).
const LUMA_MATRIX: ColorMatrix = ColorMatrix::Bt709Full;

/// PSNR + SSIM result for one `compare_rgba8` call.
///
/// `psnr_rgb` / `psnr_luma` are in dB and are `f64::INFINITY` for
/// bit-identical inputs; `ssim` is the mean windowed SSIM (`1.0` for
/// bit-identical inputs, generally in `[0.0, 1.0]` for real distortions,
/// but can dip slightly negative for strongly anti-correlated structure).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// PSNR (dB) over the RGB channels of the RGBA8 frames (alpha ignored).
    pub psnr_rgb: f64,
    /// PSNR (dB) over the BT.709 full-range luma plane.
    pub psnr_luma: f64,
    /// Mean windowed SSIM over the BT.709 full-range luma plane.
    pub ssim: f64,
}

/// PSNR + SSIM analyzer bound to a fixed `width x height` RGBA8 frame size.
///
/// [`QualityAnalyzer::new`] preallocates every working buffer (luma planes,
/// the SSIM separable-Gaussian scratch); [`QualityAnalyzer::compare_rgba8`]
/// and [`QualityAnalyzer::ssim_map_into`] never allocate.
#[derive(Debug)]
pub struct QualityAnalyzer {
    dims: FrameDims,
    luma_a_u8: Vec<u8>,
    luma_b_u8: Vec<u8>,
    luma_a_f32: Vec<f32>,
    luma_b_f32: Vec<f32>,
    ssim: SsimKernel,
}

impl QualityAnalyzer {
    /// Creates an analyzer for `width x height` RGBA8 frames.
    ///
    /// # Errors
    ///
    /// - [`crate::QualityError::Core`] if `width`/`height` is zero.
    /// - [`crate::QualityError::WindowTooLarge`] if `width` or `height` is
    ///   smaller than the `11`-pixel SSIM window.
    pub fn new(width: usize, height: usize) -> Result<Self> {
        let dims = FrameDims::new(width, height)?;
        let luma_len = dims.luma_len()?;
        let ssim = SsimKernel::new(width, height)?;
        Ok(Self {
            dims,
            luma_a_u8: vec![0u8; luma_len],
            luma_b_u8: vec![0u8; luma_len],
            luma_a_f32: vec![0.0f32; luma_len],
            luma_b_f32: vec![0.0f32; luma_len],
            ssim,
        })
    }

    /// Frame width this analyzer was constructed for.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.dims.width
    }

    /// Frame height this analyzer was constructed for.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.dims.height
    }

    /// Extracts BT.709 full-range luma from `reference`/`distorted` into the
    /// preallocated `u8` and normalized `f32` scratch planes. Both inputs
    /// must already be validated as `width * height * 4` RGBA8 buffers.
    fn extract_luma(&mut self, reference: &[u8], distorted: &[u8]) -> Result<()> {
        let (width, height) = (self.dims.width, self.dims.height);
        rgba8_to_luma_into(reference, width, height, LUMA_MATRIX, &mut self.luma_a_u8)?;
        rgba8_to_luma_into(distorted, width, height, LUMA_MATRIX, &mut self.luma_b_u8)?;
        u8_to_f32_into(&self.luma_a_u8, &mut self.luma_a_f32)?;
        u8_to_f32_into(&self.luma_b_u8, &mut self.luma_b_f32)?;
        Ok(())
    }

    /// Computes PSNR (RGB + luma) and mean SSIM between two RGBA8 frames.
    ///
    /// # Errors
    ///
    /// Returns [`crate::QualityError::Core`] if either buffer's length does
    /// not equal `width * height * 4`.
    pub fn compare_rgba8(&mut self, reference: &[u8], distorted: &[u8]) -> Result<Metrics> {
        let (width, height) = (self.dims.width, self.dims.height);
        validate_rgba8(reference, width, height)?;
        validate_rgba8(distorted, width, height)?;

        let psnr_rgb = psnr::psnr_rgb(reference, distorted, width, height)?;

        self.extract_luma(reference, distorted)?;
        let psnr_luma = psnr::psnr_luma(&self.luma_a_u8, &self.luma_b_u8)?;

        self.ssim.compute(&self.luma_a_f32, &self.luma_b_f32)?;
        let ssim = self.ssim.mean();

        Ok(Metrics {
            psnr_rgb,
            psnr_luma,
            ssim,
        })
    }

    /// Computes the SSIM heatmap between two RGBA8 frames into `out`
    /// (`width * height * 4` RGBA8 bytes; red = dissimilar, green =
    /// similar) and returns the mean SSIM.
    ///
    /// # Errors
    ///
    /// Returns [`crate::QualityError::Core`] if any of `reference`,
    /// `distorted`, or `out` has the wrong length for `width * height * 4`.
    pub fn ssim_map_into(
        &mut self,
        reference: &[u8],
        distorted: &[u8],
        out: &mut [u8],
    ) -> Result<f64> {
        let (width, height) = (self.dims.width, self.dims.height);
        validate_rgba8(reference, width, height)?;
        validate_rgba8(distorted, width, height)?;
        validate_rgba8(out, width, height)?;

        self.extract_luma(reference, distorted)?;
        self.ssim.compute(&self.luma_a_f32, &self.luma_b_f32)?;
        self.ssim.render_heatmap_rgba8(out);
        Ok(self.ssim.mean())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_web_core::frame::RGBA_CHANNELS;

    struct Lcg(u64);
    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn next_u8(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 56) as u8
        }
    }

    fn lcg_rgba(width: usize, height: usize, seed: u64) -> Vec<u8> {
        let mut lcg = Lcg::new(seed);
        (0..width * height * RGBA_CHANNELS)
            .map(|i| {
                if i % RGBA_CHANNELS == 3 {
                    255
                } else {
                    lcg.next_u8()
                }
            })
            .collect()
    }

    #[test]
    fn identical_frames_give_infinite_psnr_and_unit_ssim() {
        let frame = lcg_rgba(32, 32, 0xabc);
        let mut analyzer = QualityAnalyzer::new(32, 32).unwrap();
        let metrics = analyzer.compare_rgba8(&frame, &frame).unwrap();
        assert_eq!(metrics.psnr_rgb, f64::INFINITY);
        assert_eq!(metrics.psnr_luma, f64::INFINITY);
        assert_eq!(metrics.ssim, 1.0);
    }

    #[test]
    fn small_distortion_psnr_matches_expected_range() {
        let a = lcg_rgba(32, 32, 0x1);
        let mut b = a.clone();
        for px in b.chunks_exact_mut(RGBA_CHANNELS) {
            px[0] = px[0].saturating_add(2);
            px[1] = px[1].saturating_add(2);
            px[2] = px[2].saturating_add(2);
        }
        let mut analyzer = QualityAnalyzer::new(32, 32).unwrap();
        let metrics = analyzer.compare_rgba8(&a, &b).unwrap();
        assert!(metrics.psnr_rgb.is_finite());
        assert!(metrics.ssim > 0.0 && metrics.ssim <= 1.0);
    }

    #[test]
    fn mismatched_buffer_length_errors() {
        let a = lcg_rgba(16, 16, 1);
        let b = lcg_rgba(8, 8, 2);
        let mut analyzer = QualityAnalyzer::new(16, 16).unwrap();
        assert!(analyzer.compare_rgba8(&a, &b).is_err());
    }

    #[test]
    fn zero_dimension_construction_errors() {
        assert!(QualityAnalyzer::new(0, 16).is_err());
        assert!(QualityAnalyzer::new(16, 0).is_err());
    }

    #[test]
    fn window_too_small_construction_errors() {
        assert!(QualityAnalyzer::new(4, 4).is_err());
    }

    #[test]
    fn ssim_map_into_matches_compare_mean_and_marks_full_canvas() {
        let a = lcg_rgba(24, 20, 0x10);
        let b = lcg_rgba(24, 20, 0x20);
        let mut analyzer = QualityAnalyzer::new(24, 20).unwrap();
        let metrics = analyzer.compare_rgba8(&a, &b).unwrap();

        let mut heatmap = vec![0u8; 24 * 20 * RGBA_CHANNELS];
        let mean = analyzer.ssim_map_into(&a, &b, &mut heatmap).unwrap();
        assert!((mean - metrics.ssim).abs() < 1e-9);
        for px in heatmap.chunks_exact(RGBA_CHANNELS) {
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn ssim_map_into_rejects_wrong_out_length() {
        let a = lcg_rgba(16, 16, 1);
        let b = lcg_rgba(16, 16, 2);
        let mut analyzer = QualityAnalyzer::new(16, 16).unwrap();
        let mut bad_out = vec![0u8; 4];
        assert!(analyzer.ssim_map_into(&a, &b, &mut bad_out).is_err());
    }

    // ── Perf smoke test (ignored by default; run with --ignored) ───────────

    /// Perf smoke test: `compare_rgba8` on a 1080p pair, printed for
    /// eyeballing plus a generous canary assertion.
    ///
    /// Run with `cargo test --release -- --ignored` for a meaningful
    /// number: the default `dev` profile is 10-50x slower and not
    /// representative of anything.
    ///
    /// The canary is `< 300ms/frame`, not the `< 60ms` a speed-tuned build
    /// would target, because `web/Cargo.toml`'s `[profile.release]` is
    /// `opt-level = "z"` workspace-wide (a deliberate wasm-binary-size
    /// tradeoff shared by every crate under `web/`, out of this crate's
    /// scope to change) — measured locally at ~130ms/frame under that
    /// profile vs. ~45ms/frame with `RUSTFLAGS="-C opt-level=3"`, confirming
    /// the gap is the profile, not an algorithmic regression. The shipped
    /// wasm32 build additionally gets real SIMD lanes via
    /// `-C target-feature=+simd128` (`.cargo/config.toml`) that this
    /// native, non-`target-feature`-enabled baseline cannot exercise
    /// either. 300ms still catches the failure modes a canary exists for —
    /// lost preallocation (reintroduced per-frame `Vec` allocation) or an
    /// accidental quadratic blowup — which would blow past it by 10-100x.
    #[test]
    #[ignore = "perf smoke test; run with --ignored"]
    fn perf_compare_1080p() {
        use std::time::Instant;
        let (w, h) = (1920usize, 1080usize);
        let a = lcg_rgba(w, h, 0x1111);
        let b = lcg_rgba(w, h, 0x2222);
        let mut analyzer = QualityAnalyzer::new(w, h).unwrap();

        // Warm up (also proves the preallocated buffers are already the
        // right size — no allocation happens on this or later calls).
        analyzer.compare_rgba8(&a, &b).unwrap();

        let iters = 10;
        let start = Instant::now();
        for _ in 0..iters {
            analyzer.compare_rgba8(&a, &b).unwrap();
        }
        let elapsed = start.elapsed();
        let per_frame = elapsed / iters;
        println!(
            "compare_rgba8 1920x1080: {per_frame:?}/frame over {iters} iters ({elapsed:?} total)"
        );
        assert!(
            per_frame.as_millis() < 300,
            "canary: expected < 300ms/frame natively (opt-level=z release), got {per_frame:?}"
        );
    }
}
