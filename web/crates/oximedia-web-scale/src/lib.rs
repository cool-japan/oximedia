// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `oximedia-web-scale` — professional separable image/video resampling
//! (Lanczos3, Catmull-Rom, Mitchell-Netravali, bilinear) compiled to
//! WebAssembly.
//!
//! # Implementation strategy: port, not depend
//!
//! This crate does **not** depend on `oximedia-scaling`: that crate's
//! dependency tree pulls in `scirs2-core` (full BLAS/LAPACK) transitively,
//! and its SIMD paths are single/triple-channel only rather than
//! 4-channel-interleaved, neither of which is compatible with the wasm size
//! budget (`web/scripts/size-gate.sh`) or dependency allowlist
//! (`web/scripts/dep-gate.sh`) enforced across `web/`. Instead, the filter
//! kernel math and the separable weight-table technique are **ported** —
//! copied and adapted as dependency-free `f32`/`u8` code — from:
//!
//! - `crates/oximedia-scaling/src/resampler.rs` — `FilterKernel::{support,
//!   evaluate}` for Lanczos3, the two cubic B-spline kernels, and bilinear
//!   ([`filter`]). See that module's doc comment for a naming correction
//!   versus the upstream source (`Bicubic` there is actually Catmull-Rom).
//! - `crates/oximedia-scaling/src/ewa_resample.rs` /
//!   `scale_filter.rs` — the `FilterWeightTable` precomputed-weight-span
//!   technique, adapted to a fixed-stride flat layout ([`weights`]).
//!
//! # Architecture
//!
//! [`Resizer`] is the allocation-free resampling engine, natively testable
//! with plain `cargo test` (no `wasm-bindgen`, no browser). [`Scaler`] is
//! the thin `#[wasm_bindgen]` wrapper the JS side (`web/js/scale.js`) talks
//! to.
//!
//! # Data-plane discipline
//!
//! Per-frame calls ([`Resizer::resize_rgba8`] / [`Resizer::resize_f32`] and
//! their `Scaler` wasm equivalents) never allocate — every scratch buffer
//! and weight table is built once in [`Resizer::new`] / the `Scaler`
//! constructor and reused for the resizer's lifetime. The `f32` path never
//! touches `Float64Array`-shaped data; HDR/linear values above `1.0` pass
//! through unclamped. Errors are always [`ScaleError`] (or a `JsValue`
//! string at the wasm boundary) — malformed input (zero dimension,
//! mismatched buffer length, unknown filter name) is reported, never a
//! panic.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod bindings;
mod error;
mod filter;
mod resizer;
mod weights;

pub use bindings::Scaler;
pub use error::ScaleError;
pub use filter::Filter;
pub use resizer::Resizer;
pub use weights::WeightTable;

use wasm_bindgen::prelude::*;

/// Returns the crate version as declared in `Cargo.toml`.
///
/// Exported to JS as a minimal end-to-end sanity check of the wasm-bindgen
/// build pipeline (`web/scripts/build.sh`), independent of the `Scaler`
/// class itself.
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
    fn public_api_end_to_end() {
        let filter = Filter::parse("lanczos3").unwrap();
        let mut resizer = Resizer::new(4, 4, 2, 2, filter, true).unwrap();
        let src = vec![128u8; 4 * 4 * 4];
        let mut dst = vec![0u8; 2 * 2 * 4];
        resizer.resize_rgba8(&src, &mut dst).unwrap();
        assert_eq!(dst, vec![128u8; 2 * 2 * 4]);
    }
}
