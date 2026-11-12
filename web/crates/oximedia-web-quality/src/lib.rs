// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `oximedia-web-quality` — PSNR and (real, windowed) SSIM video quality
//! metrics compiled to WebAssembly.
//!
//! # Milestone
//!
//! M5b: PSNR (`psnr_rgb`, `psnr_luma`) and a separable-Gaussian windowed
//! SSIM, wrapped for JS as the [`Quality`] class.
//!
//! **VMAF is explicitly deferred and will not be added to this crate as-is.**
//! The upstream native codebase carries three mutually non-interoperating,
//! never-validated VMAF approximations; porting any of them here would ship
//! a number that looks authoritative but has not been checked against the
//! reference `libvmaf` on a single real corpus. See `web/TODO.md` M5.
//!
//! # What lives here
//!
//! - [`psnr_rgb`] / [`psnr_luma`] — PSNR (dB) over RGB channels (alpha
//!   ignored) and over BT.709 full-range luma. `f64` accumulators
//!   internally (a single scalar result), `u8` buffers throughout;
//!   bit-identical inputs return `f64::INFINITY` (not a clamped "very high"
//!   placeholder like the native `oximedia-quality` port uses).
//! - [`SsimKernel`] — real windowed SSIM (`11x11` Gaussian, `sigma = 1.5`,
//!   `K1 = 0.01`, `K2 = 0.03`, `L = 1.0` on normalized `f32` luma) evaluated
//!   as two separable 1-D passes (`O(width*height*window)`) instead of the
//!   naive dense `11x11` evaluation (`O(width*height*window^2)`) used by
//!   the native port; see the `ssim` module docs for the exact
//!   edge-handling semantics ported from
//!   `crates/oximedia-quality/src/ssim.rs`, and its `#[cfg(test)]` module
//!   for the naive-reference cross-validation.
//! - [`QualityAnalyzer`] / [`Metrics`] — the preallocated,
//!   zero-per-frame-allocation comparator both the native tests and the
//!   `wasm-bindgen` glue are built on.
//! - [`Quality`] / [`QualityResult`] — the `#[wasm_bindgen]` surface: typed
//!   getters, never JSON, on the per-frame path.
//!
//! # Data-plane discipline
//!
//! Never `Float64Array` across the wasm boundary: RGBA8 in as `&[u8]`,
//! metrics out as `f64` scalars via typed getters, the SSIM heatmap out as
//! `&mut [u8]` (RGBA8). [`QualityAnalyzer::new`] preallocates every
//! luma/SSIM scratch buffer so `compare_rgba8` / `ssim_map_into` never
//! allocate.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod analyzer;
mod error;
mod psnr;
mod ssim;
mod wasm;

pub use analyzer::{Metrics, QualityAnalyzer};
pub use error::{QualityError, Result};
pub use psnr::{psnr_luma, psnr_rgb};
pub use ssim::{SsimKernel, WINDOW_SIZE};
pub use wasm::{Quality, QualityResult};

use wasm_bindgen::prelude::*;

/// Returns the crate version as declared in `Cargo.toml`.
///
/// Used to verify the wasm-bindgen build pipeline (`web/scripts/build.sh`)
/// end-to-end.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    oximedia_web_core::version().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn public_reexports_are_reachable() {
        let mut analyzer = QualityAnalyzer::new(16, 16).expect("analyzer");
        let frame = vec![128u8; 16 * 16 * 4];
        let metrics: Metrics = analyzer.compare_rgba8(&frame, &frame).expect("compare");
        assert_eq!(metrics.ssim, 1.0);

        let kernel = SsimKernel::new(WINDOW_SIZE, WINDOW_SIZE).expect("kernel");
        assert_eq!(kernel.width(), WINDOW_SIZE);

        assert_eq!(
            psnr_rgb(&frame, &frame, 16, 16).expect("psnr_rgb"),
            f64::INFINITY
        );
        let luma = vec![0u8; 16 * 16];
        assert_eq!(psnr_luma(&luma, &luma).expect("psnr_luma"), f64::INFINITY);
    }
}
