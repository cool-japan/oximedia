// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `#[wasm_bindgen]` glue: JS classes `ColorPipeline` and `CubeLut`, plus the
//! free function `bake_cube`.
//!
//! Configuration setters take strings (parsed once — this is the config
//! path, and it is JSON-free anyway). The per-frame `apply` / `apply_f32`
//! methods move pixels exclusively as `Uint8Array` / `Float32Array` slices
//! with caller-provided output buffers — one copy in, one copy out, no
//! allocation inside the pipeline after warm-up.

use crate::cube;
use crate::error::ColorError;
use crate::gamut::Primaries;
use crate::lut::LutInterp;
use crate::pipeline;
use crate::tone_map::ToneMapOperator;
use crate::transfer::Transfer;
use oximedia_web_core::validate_rgba8;
use wasm_bindgen::prelude::*;

/// Converts any displayable error into a JS `Error` object.
#[cfg(target_arch = "wasm32")]
fn js_err(e: impl core::fmt::Display) -> JsValue {
    JsValue::from(js_sys::Error::new(&e.to_string()))
}

/// Native fallback: `JsValue` construction requires a JS heap, so on
/// non-wasm targets (unit tests, native clippy) the error carries no
/// message. Only the `Err`-ness is observable natively.
#[cfg(not(target_arch = "wasm32"))]
fn js_err(e: impl core::fmt::Display) -> JsValue {
    let _ = e;
    JsValue::NULL
}

/// Maps a `oximedia-web-core` validation error into a JS `Error`.
fn core_err(e: oximedia_web_core::CoreError) -> JsValue {
    js_err(e)
}

// ── CubeLut ───────────────────────────────────────────────────────────────────

/// A parsed `.cube` 3D LUT, exposed to JS as `CubeLut`.
#[wasm_bindgen(js_name = CubeLut)]
pub struct WasmCubeLut {
    inner: crate::lut::Lut3d,
}

#[wasm_bindgen(js_class = CubeLut)]
impl WasmCubeLut {
    /// Parses `.cube` text. Hostile input is rejected with a descriptive
    /// `Error`; this never panics/aborts the wasm instance.
    ///
    /// # Errors
    /// Throws a JS `Error` describing the parse failure.
    pub fn parse(text: &str) -> Result<WasmCubeLut, JsValue> {
        cube::parse_cube(text)
            .map(|inner| WasmCubeLut { inner })
            .map_err(js_err)
    }

    /// Serialises the LUT back to `.cube` text (R-fastest data order).
    #[must_use]
    pub fn export(&self) -> String {
        cube::export_cube(&self.inner)
    }

    /// Lattice size per axis.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.inner.size() as u32
    }

    /// LUT title, if the file carried one.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        self.inner.title().map(str::to_string)
    }
}

// ── ColorPipeline ─────────────────────────────────────────────────────────────

/// The fused colour pipeline, exposed to JS as `ColorPipeline`.
///
/// See the crate docs for the fixed operator order (decode → exposure →
/// contrast → saturation → tone map → gamut → encode → 3D LUT).
#[wasm_bindgen(js_name = ColorPipeline)]
pub struct WasmColorPipeline {
    inner: pipeline::ColorPipeline,
}

impl Default for WasmColorPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen(js_class = ColorPipeline)]
impl WasmColorPipeline {
    /// Creates an identity pipeline (sRGB in, sRGB out, neutral ops).
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: pipeline::ColorPipeline::new(),
        }
    }

    /// Sets exposure in stops (`gain = 2^stops`, applied in linear light).
    ///
    /// # Errors
    /// Throws on NaN/infinite input or |stops| > 32.
    pub fn set_exposure(&mut self, stops: f32) -> Result<(), JsValue> {
        self.inner.set_exposure(stops).map_err(js_err)
    }

    /// Sets contrast (power law around the 0.18 linear pivot; 1.0 neutral).
    ///
    /// # Errors
    /// Throws for values outside `(0, 10]`.
    pub fn set_contrast(&mut self, contrast: f32) -> Result<(), JsValue> {
        self.inner.set_contrast(contrast).map_err(js_err)
    }

    /// Sets saturation (BT.709 luma blend; 1.0 neutral, 0.0 monochrome).
    ///
    /// # Errors
    /// Throws for values outside `[0, 10]`.
    pub fn set_saturation(&mut self, saturation: f32) -> Result<(), JsValue> {
        self.inner.set_saturation(saturation).map_err(js_err)
    }

    /// Enables tone mapping. `op` is one of `"reinhard"`,
    /// `"reinhard-extended"`, `"hable"`/`"filmic"`, `"aces"` (Narkowicz
    /// fitted curve) or `"aces-odt"` (ACES OT 2.0-shaped RRT — see the
    /// `.d.ts` for the honesty note distinguishing the two).
    ///
    /// # Errors
    /// Throws on an unknown operator or non-positive/non-finite peak nits.
    pub fn set_tone_map(
        &mut self,
        op: &str,
        input_peak_nits: f32,
        output_peak_nits: f32,
    ) -> Result<(), JsValue> {
        let op = ToneMapOperator::parse(op).map_err(js_err)?;
        self.inner
            .set_tone_map(op, input_peak_nits, output_peak_nits)
            .map_err(js_err)
    }

    /// Disables tone mapping.
    pub fn clear_tone_map(&mut self) {
        self.inner.clear_tone_map();
    }

    /// Enables gamut conversion between `"bt709"`, `"bt2020"` and
    /// `"display-p3"` (aliases accepted, see the `.d.ts`).
    ///
    /// # Errors
    /// Throws on unknown primaries names.
    pub fn set_gamut(&mut self, src: &str, dst: &str) -> Result<(), JsValue> {
        let src = Primaries::parse(src).map_err(js_err)?;
        let dst = Primaries::parse(dst).map_err(js_err)?;
        self.inner.set_gamut(src, dst).map_err(js_err)
    }

    /// Sets the soft-clip softness (`[0, 1]`) of the active gamut stage.
    /// 0 (the default) fixes negative channels only and preserves HDR
    /// values above 1.0.
    ///
    /// # Errors
    /// Throws if no gamut stage is configured or the value is non-finite.
    pub fn set_gamut_softness(&mut self, softness: f32) -> Result<(), JsValue> {
        self.inner.set_gamut_softness(softness).map_err(js_err)
    }

    /// Disables gamut conversion.
    pub fn clear_gamut(&mut self) {
        self.inner.clear_gamut();
    }

    /// Enables the 3D-LUT stage. `interp` is `"trilinear"` or
    /// `"tetrahedral"`. The LUT is applied on encoded output values (the
    /// standard creative-LUT convention).
    ///
    /// # Errors
    /// Throws on an unknown interpolation name.
    pub fn set_lut(&mut self, lut: &WasmCubeLut, interp: &str) -> Result<(), JsValue> {
        let interp = LutInterp::parse(interp).map_err(js_err)?;
        self.inner.set_lut(lut.inner.clone(), interp);
        Ok(())
    }

    /// Disables the 3D-LUT stage.
    pub fn clear_lut(&mut self) {
        self.inner.clear_lut();
    }

    /// Sets the input transfer function: `"srgb"`, `"pq"`, `"hlg"` or
    /// `"linear"`.
    ///
    /// # Errors
    /// Throws on an unknown name.
    pub fn set_input_transfer(&mut self, name: &str) -> Result<(), JsValue> {
        let t = Transfer::parse(name).map_err(js_err)?;
        self.inner.set_input_transfer(t);
        Ok(())
    }

    /// Sets the output transfer function: `"srgb"`, `"pq"`, `"hlg"` or
    /// `"linear"`.
    ///
    /// # Errors
    /// Throws on an unknown name.
    pub fn set_output_transfer(&mut self, name: &str) -> Result<(), JsValue> {
        let t = Transfer::parse(name).map_err(js_err)?;
        self.inner.set_output_transfer(t);
        Ok(())
    }

    /// Selects the 8-bit data plane. `false` (the default) runs the baked
    /// fast path: the whole configured chain is sampled into one internal
    /// 33³ lattice whenever the configuration changes, and each frame then
    /// costs a single trilinear interpolation per pixel (may differ from
    /// the exact chain by ~2/255 per channel). `true` runs the exact
    /// per-pixel chain every frame. `apply_f32` is always exact.
    pub fn set_exact(&mut self, exact: bool) {
        self.inner.set_exact(exact);
    }

    /// Applies the pipeline to a tightly packed RGBA8 frame.
    ///
    /// `src` and `dst` must both be `width × height × 4` bytes. Alpha passes
    /// through. Allocation-free per call (wasm-bindgen copies the slices in
    /// and the result back out — one copy each way).
    ///
    /// # Errors
    /// Throws on zero dimensions or buffer-length mismatches.
    pub fn apply(
        &mut self,
        src: &[u8],
        dst: &mut [u8],
        width: u32,
        height: u32,
    ) -> Result<(), JsValue> {
        validate_rgba8(src, width as usize, height as usize).map_err(core_err)?;
        if dst.len() != src.len() {
            return Err(js_err(ColorError::LengthMismatch {
                left: src.len(),
                right: dst.len(),
            }));
        }
        self.inner.apply_rgba8(src, dst).map_err(js_err)
    }

    /// In-place variant of `apply`: `buf` is both source and destination
    /// (alpha bytes are left untouched). Preferred on the per-frame path —
    /// a single buffer halves the JS↔wasm boundary copies.
    ///
    /// # Errors
    /// Throws on zero dimensions or a buffer-length mismatch.
    pub fn apply_in_place(
        &mut self,
        buf: &mut [u8],
        width: u32,
        height: u32,
    ) -> Result<(), JsValue> {
        validate_rgba8(buf, width as usize, height as usize).map_err(core_err)?;
        self.inner.apply_rgba8_in_place(buf).map_err(js_err)
    }

    /// Applies the pipeline to a tightly packed RGBA `f32` frame (HDR path,
    /// exact transfer curves).
    ///
    /// # Errors
    /// Throws on zero dimensions or buffer-length mismatches.
    pub fn apply_f32(
        &mut self,
        src: &[f32],
        dst: &mut [f32],
        width: u32,
        height: u32,
    ) -> Result<(), JsValue> {
        oximedia_web_core::validate_rgba_f32(src, width as usize, height as usize)
            .map_err(core_err)?;
        if dst.len() != src.len() {
            return Err(js_err(ColorError::LengthMismatch {
                left: src.len(),
                right: dst.len(),
            }));
        }
        self.inner.apply_rgba_f32(src, dst).map_err(js_err)
    }

    /// Bakes this pipeline into `.cube` text of the given lattice size
    /// (convenience method mirroring the free function [`bake_cube`]).
    ///
    /// # Errors
    /// Throws if `size` is outside `2..=129`.
    pub fn export_cube(&self, size: u32) -> Result<String, JsValue> {
        let lut = self.inner.bake_lut(size as usize).map_err(js_err)?;
        Ok(cube::export_cube(&lut))
    }
}

// ── Free functions ────────────────────────────────────────────────────────────

/// Samples `pipeline` into a 3D LUT of the given size and returns `.cube`
/// text (powers the demo's "Export .cube" button).
///
/// # Errors
/// Throws if `size` is outside `2..=129`.
#[wasm_bindgen]
pub fn bake_cube(pipeline: &WasmColorPipeline, size: u32) -> Result<String, JsValue> {
    pipeline.export_cube(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wasm_pipeline_smoke() {
        let mut p = WasmColorPipeline::new();
        p.set_exposure(0.7).expect("exposure");
        p.set_contrast(1.1).expect("contrast");
        p.set_saturation(1.0).expect("saturation");
        p.set_tone_map("aces", 1000.0, 100.0).expect("tone map");
        p.set_gamut("bt2020", "bt709").expect("gamut");
        p.set_input_transfer("hlg").expect("in transfer");
        p.set_output_transfer("srgb").expect("out transfer");

        let src = vec![128u8; 16];
        let mut dst = vec![0u8; 16];
        p.apply(&src, &mut dst, 2, 2).expect("apply");
        assert_eq!(dst[3], 128, "alpha passthrough");
    }

    #[test]
    fn wasm_pipeline_rejects_bad_geometry() {
        let mut p = WasmColorPipeline::new();
        let src = vec![0u8; 16];
        let mut dst = vec![0u8; 12];
        assert!(p.apply(&src, &mut dst, 2, 2).is_err());
        let mut dst16 = vec![0u8; 16];
        assert!(p.apply(&src, &mut dst16, 0, 2).is_err());
        assert!(p.apply(&src, &mut dst16, 3, 2).is_err());
    }

    #[test]
    fn wasm_cube_lut_round_trip() {
        let p = WasmColorPipeline::new();
        let text = bake_cube(&p, 5).expect("bake");
        let lut = WasmCubeLut::parse(&text).expect("parse");
        assert_eq!(lut.size(), 5);
        assert_eq!(lut.title(), Some("OxiMedia ColorPipeline".to_string()));
        let text2 = lut.export();
        let lut2 = WasmCubeLut::parse(&text2).expect("re-parse");
        assert_eq!(lut2.size(), 5);
    }

    #[test]
    fn wasm_setters_reject_unknown_names() {
        let mut p = WasmColorPipeline::new();
        assert!(p.set_tone_map("mobius", 1000.0, 100.0).is_err());
        assert!(p.set_gamut("adobergb", "bt709").is_err());
        assert!(p.set_input_transfer("gamma26").is_err());
        assert!(p.set_output_transfer("").is_err());
        let identity = WasmColorPipeline::new();
        let cube_text = bake_cube(&identity, 3).expect("bake");
        let lut = WasmCubeLut::parse(&cube_text).expect("parse");
        assert!(p.set_lut(&lut, "nearest").is_err());
        assert!(p.set_lut(&lut, "tetrahedral").is_ok());
    }
}
