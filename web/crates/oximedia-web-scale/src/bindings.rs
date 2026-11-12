// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `#[wasm_bindgen]` glue exposing [`crate::Resizer`] to JavaScript as the
//! `Scaler` class.
//!
//! This module is deliberately thin: all resampling logic lives in
//! [`crate::resizer::Resizer`] (natively testable, no `wasm-bindgen`
//! dependency); this module only translates JS-facing types (`&str` filter
//! names, `u32` dimensions, [`ScaleError`] -> [`JsValue`]) at the boundary.
//! Per the data-plane rules, `resize`/`resize_f32` never touch
//! `Float64Array`, never allocate on the JS<->wasm hot path (the JS side
//! passes persistent `Uint8ClampedArray`/`Float32Array` buffers, and
//! wasm-bindgen copies `&mut [u8]`/`&mut [f32]` results back into them
//! automatically), and never carry JSON.

use wasm_bindgen::prelude::*;

use crate::error::ScaleError;
use crate::filter::Filter;
use crate::resizer::Resizer;

/// Converts a [`ScaleError`] into the [`JsValue`] wasm-bindgen throws as a
/// JavaScript exception.
fn to_js_error(e: ScaleError) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Professional separable image/video resampler for a fixed
/// `(src -> dst)` geometry, exposed to JavaScript.
///
/// Mirrors [`Resizer`] one-to-one; see that type's docs for the resampling
/// pipeline. Construct once per source/destination resolution pair and
/// reuse across every frame — construction is where the weight tables and
/// scratch buffers are built, so repeated construction defeats the whole
/// "zero per-frame allocation" design.
#[wasm_bindgen]
pub struct Scaler {
    inner: Resizer,
}

#[wasm_bindgen]
impl Scaler {
    /// Builds a `Scaler` for a fixed `(src_width, src_height) ->
    /// (dst_width, dst_height)` geometry.
    ///
    /// `filter` must be one of `"bilinear"`, `"catmull-rom"`, `"mitchell"`
    /// or `"lanczos3"`. `premultiply` should be `true` whenever the source
    /// has a meaningful alpha channel (it prevents color fringing from
    /// transparent pixels bleeding into opaque neighbors on downscale).
    ///
    /// # Errors
    ///
    /// Throws (as a `JsValue` string) if any dimension is `0`, if `filter`
    /// is not a recognized name, or if the requested geometry's scratch
    /// buffers would overflow `usize`.
    #[wasm_bindgen(constructor)]
    pub fn new(
        src_width: u32,
        src_height: u32,
        dst_width: u32,
        dst_height: u32,
        filter: &str,
        premultiply: bool,
    ) -> Result<Scaler, JsValue> {
        let filter = Filter::parse(filter).map_err(to_js_error)?;
        let inner = Resizer::new(
            src_width as usize,
            src_height as usize,
            dst_width as usize,
            dst_height as usize,
            filter,
            premultiply,
        )
        .map_err(to_js_error)?;
        Ok(Self { inner })
    }

    /// The source width this `Scaler` was constructed for.
    #[wasm_bindgen(getter, js_name = srcWidth)]
    #[must_use]
    pub fn src_width(&self) -> u32 {
        self.inner.src_dims().0 as u32
    }

    /// The source height this `Scaler` was constructed for.
    #[wasm_bindgen(getter, js_name = srcHeight)]
    #[must_use]
    pub fn src_height(&self) -> u32 {
        self.inner.src_dims().1 as u32
    }

    /// The destination width this `Scaler` was constructed for.
    #[wasm_bindgen(getter, js_name = dstWidth)]
    #[must_use]
    pub fn dst_width(&self) -> u32 {
        self.inner.dst_dims().0 as u32
    }

    /// The destination height this `Scaler` was constructed for.
    #[wasm_bindgen(getter, js_name = dstHeight)]
    #[must_use]
    pub fn dst_height(&self) -> u32 {
        self.inner.dst_dims().1 as u32
    }

    /// Resamples an 8-bit RGBA `src` frame into `dst`.
    ///
    /// `src` must be `srcWidth * srcHeight * 4` bytes and `dst` must be
    /// `dstWidth * dstHeight * 4` bytes (both tightly packed, no row
    /// padding). wasm-bindgen copies `src` in and copies the written `dst`
    /// contents back into the caller's `Uint8Array`/`Uint8ClampedArray`; no
    /// other allocation occurs.
    ///
    /// # Errors
    ///
    /// Throws if either buffer is not exactly the expected length.
    pub fn resize(&mut self, src: &[u8], dst: &mut [u8]) -> Result<(), JsValue> {
        self.inner.resize_rgba8(src, dst).map_err(to_js_error)
    }

    /// Resamples an `f32` RGBA `src` frame (HDR/linear-light; values are
    /// never clamped to `[0, 1]`) into `dst`.
    ///
    /// `src` must be `srcWidth * srcHeight * 4` elements and `dst` must be
    /// `dstWidth * dstHeight * 4` elements. wasm-bindgen copies `src` in and
    /// copies the written `dst` contents back into the caller's
    /// `Float32Array`; no other allocation occurs.
    ///
    /// # Errors
    ///
    /// Throws if either buffer is not exactly the expected length.
    #[wasm_bindgen(js_name = resizeF32)]
    pub fn resize_f32(&mut self, src: &[f32], dst: &mut [f32]) -> Result<(), JsValue> {
        self.inner.resize_f32(src, dst).map_err(to_js_error)
    }
}
