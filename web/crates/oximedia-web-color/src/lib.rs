// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `oximedia-web-color` — the colour-science pipeline for the browser:
//! exposure / contrast / saturation, tone mapping (Reinhard / Filmic-Hable /
//! ACES, with peak-nits control), primaries-aware gamut mapping
//! (BT.709 / BT.2020 / Display-P3), transfer functions (sRGB / PQ / HLG /
//! linear), and 3D LUTs (`.cube` load + trilinear/tetrahedral apply +
//! export), compiled to WebAssembly.
//!
//! # Architecture
//!
//! Everything is `f32` internally; the JS boundary is strictly `Uint8Array`
//! (8-bit SDR) or `Float32Array` (HDR/linear) — never `f64` slices, never
//! JSON on the per-frame path. The per-pixel chain is fully fused (see
//! [`pipeline`] for the fixed operator order) and allocation-free after
//! configuration; the u8 fast path uses pipeline-owned decode/encode tables
//! rebuilt only when a transfer function changes.
//!
//! # Provenance (port, not depend)
//!
//! The algorithms are ports of the canonical native implementations, chosen
//! deliberately over the known-buggy duplicates elsewhere in the workspace:
//!
//! * tone mapping — `crates/oximedia-hdr/src/tone_mapping.rs`; the
//!   `aces-odt` operator additionally ports `AcesOt2` from
//!   `crates/oximedia-colormgmt/src/aces_output_transform.rs`
//!   (see [`tone_map`] for the honest "which ACES is which" note);
//! * gamut — `crates/oximedia-hdr/src/gamut.rs` (**not**
//!   `oximedia-colormgmt`'s `GamutMapper`, which ignores its colorspace
//!   argument);
//! * transfer functions — `crates/oximedia-hdr/src/transfer_function.rs`,
//!   re-derived in pure `f32` (**not** colormgmt's global-mutex LUT cache;
//!   acceleration tables are owned by the pipeline);
//! * LUT — tetrahedral kernel from
//!   `crates/oximedia-colormgmt/src/lut_interp.rs`; `.cube` I/O follows the
//!   R-fastest order of `crates/oximedia-lut/src/formats/cube.rs`
//!   (**not** the B-fastest `GradingLut3D` export).
//!
//! # JS surface
//!
//! `class ColorPipeline` (string-keyed setters + `apply`/`apply_f32`),
//! `class CubeLut` (`parse`/`export`/`size`/`title`) and `bake_cube()` — see
//! `web/js/color.js` for the ergonomic wrapper.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cube;
pub mod error;
pub mod gamut;
pub mod lut;
pub mod pipeline;
pub mod tone_map;
pub mod transfer;
pub mod wasm_api;

pub use cube::{export_cube, parse_cube};
pub use error::ColorError;
pub use gamut::{GamutMap, Primaries};
pub use lut::{Lut3d, LutInterp, MAX_LUT_SIZE, MIN_LUT_SIZE};
pub use pipeline::ColorPipeline;
pub use tone_map::{ToneMap, ToneMapOperator};
pub use transfer::Transfer;

use wasm_bindgen::prelude::*;

/// Returns the crate version as declared in `Cargo.toml`.
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
}
