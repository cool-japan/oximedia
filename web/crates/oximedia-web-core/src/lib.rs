// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `oximedia-web-core` — dependency-free numeric kernels shared by the
//! OxiMedia web wasm modules (`oximedia-web-scopes`, `-color`, `-scale`,
//! `-quality`).
//!
//! This crate has **zero** dependencies (not even `wasm-bindgen`) so its
//! kernels can be exercised with plain `cargo test` on the host target,
//! without a browser or `wasm-pack`. The four wasm-facing crates depend on
//! this crate by path and wrap its kernels with `#[wasm_bindgen]` glue.
//!
//! # What lives here
//!
//! - [`CoreError`] — the crate's single error type (hand-written `Display` +
//!   `std::error::Error`, no `thiserror`).
//! - [`FrameDims`] and the `validate_*` / `*_len` helpers ([`frame`]) — tightly
//!   packed RGBA8 / f32-RGBA buffer geometry with checked (overflow-safe)
//!   arithmetic.
//! - [`ColorMatrix`] and the `*_into` YUV <-> RGBA8 kernels ([`yuv`]) —
//!   allocation-free I420 / NV12 conversion with BT.601 / BT.709 / BT.2020
//!   coefficient sets in limited and full range, written as `chunks_exact`
//!   loops for auto-vectorization under `+simd128`.
//! - Per-pixel [`rgb_to_ycbcr_bt601`] / `_bt709` / `_bt2020` helpers
//!   ([`ycbcr_pixel`]) — bit-exact with the native `oximedia-scopes` analysis
//!   path, for the scopes module's tight loops.
//! - `u8` <-> `f32` normalization and the sRGB transfer functions
//!   ([`normalize`]).
//! - [`Scratch`] ([`scratch`]) — grow-once reusable buffers so module
//!   `apply()`s never allocate per frame.
//!
//! # Data-plane discipline
//!
//! All hot-path conversions take a caller-provided output slice (`*_into`) and
//! never allocate. Fixed-point `i32` arithmetic is used for the 8-bit paths;
//! `f32` (never `f64`) is used for the linear/HDR paths.
//!
//! # Canonical sources
//!
//! The YUV kernels are ported and re-shaped from the native crates:
//! `crates/oximedia-core/src/convert/pixel.rs`,
//! `crates/oximedia-core/src/convert/simd_pixel.rs`, and
//! `crates/oximedia-simd/src/yuv_rgb.rs`. The scope YCbCr helpers are bit-exact
//! ports of `crates/oximedia-scopes/src/simd_convert.rs`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod frame;
pub mod matrix;
pub mod normalize;
pub mod scratch;
pub mod ycbcr_pixel;
pub mod yuv;

pub use error::CoreError;
pub use frame::{
    rgba8_len, rgba_f32_len, validate_rgba8, validate_rgba_f32, FrameDims, RGBA_CHANNELS,
};
pub use matrix::ColorMatrix;
pub use normalize::{
    f32_to_u8, f32_to_u8_into, srgb_eotf, srgb_eotf_into, srgb_oetf, srgb_oetf_into, u8_to_f32,
    u8_to_f32_into,
};
pub use scratch::Scratch;
pub use ycbcr_pixel::{rgb_to_ycbcr_bt2020, rgb_to_ycbcr_bt601, rgb_to_ycbcr_bt709};
pub use yuv::{
    i420_to_rgba8_into, nv12_to_rgba8_into, rgba8_to_i420_into, rgba8_to_luma_into,
};

/// Returns the crate version as declared in `Cargo.toml`.
///
/// Used by the downstream wasm crates to verify workspace wiring compiles
/// end-to-end.
#[must_use]
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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
        // A tiny end-to-end sanity check that the re-exported names resolve
        // and interoperate.
        let dims = FrameDims::new(2, 2).unwrap();
        let mut scratch = Scratch::new();
        let rgba = scratch.bytes(dims.rgba8_len().unwrap());
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 255;
            px[3] = 255;
        }
        assert_eq!(validate_rgba8(rgba, 2, 2), Ok(()));
        let [_, _, cr] = rgb_to_ycbcr_bt709(255, 0, 0);
        assert!(cr > 128);
    }
}
