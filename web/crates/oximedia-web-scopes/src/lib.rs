// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! `oximedia-web-scopes` — broadcast-grade video scopes (waveform,
//! vectorscope, histogram and false-colour exposure) for tightly packed RGBA8
//! frames, compiled to WebAssembly.
//!
//! # Architecture
//!
//! The numeric kernels live in [`mod@renderer`] and its per-scope sibling
//! modules and are **natively testable** — they operate on plain slices and
//! never touch `wasm-bindgen` or the DOM, so `cargo test -p
//! oximedia-web-scopes` exercises them on the host. A thin [`Scopes`] class
//! wraps [`ScopeRenderer`] with `#[wasm_bindgen]` glue: it copies the caller's
//! frame in via `&[u8]` and writes the rendered scope back via a `&mut [u8]`
//! out-parameter (one copy in, one copy out). Nothing allocates per frame once
//! the renderer is warm — the accumulators are sized from the fixed scope
//! dimensions at construction and the input-column map grows at most once.
//!
//! # Ported, not depended
//!
//! The algorithms are adapted from the native `oximedia-scopes`
//! (`waveform.rs`, `vectorscope.rs`, `histogram.rs`, `false_color.rs`,
//! `render.rs`), fixing three upstream bugs in the process: the label font now
//! covers the letters it draws (not just digits), false colour renders at the
//! configured scope size (not the input size), and per-call allocation is
//! eliminated. The per-pixel BT.601/BT.709 conversions come from
//! [`oximedia_web_core`] so a browser trace is bit-exact with a native one.
#![forbid(unsafe_code)]

mod canvas;
mod error;
mod false_color;
mod font;
mod graticule;
mod histogram;
mod renderer;
mod vectorscope;
mod waveform;

pub use error::{Result as ScopeResult, ScopeError};
pub use false_color::Preset as FalseColorPreset;
pub use renderer::{HistogramKind, ScopeRenderer, Stats, WaveformMode};
pub use vectorscope::{hue, is_skin_tone, saturation};

use wasm_bindgen::prelude::*;

/// Returns the crate version as declared in `Cargo.toml`.
#[wasm_bindgen]
#[must_use]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// Converts any crate error into a JS exception message.
#[cfg(target_arch = "wasm32")]
fn js_err(e: ScopeError) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// Native fallback: `JsValue` string construction requires a JS heap, so on
/// non-wasm targets (unit tests, native clippy) the error carries no
/// message. Only the `Err`-ness is observable natively.
#[cfg(not(target_arch = "wasm32"))]
fn js_err(e: ScopeError) -> JsValue {
    let _ = e;
    JsValue::NULL
}

/// The "no frame loaded" exception for the `*_current` methods.
#[cfg(target_arch = "wasm32")]
fn no_frame_err() -> JsValue {
    JsValue::from_str("Scopes: no frame loaded — call load_frame(frame, width, height) first")
}

/// Native fallback: `JsValue` string construction requires a JS heap, so on
/// non-wasm targets (unit tests, native clippy) the error carries no
/// message. Only the `Err`-ness is observable natively.
#[cfg(not(target_arch = "wasm32"))]
fn no_frame_err() -> JsValue {
    JsValue::NULL
}

/// WebAssembly-facing scope renderer.
///
/// Construct once per scope panel with the output canvas size; call the render
/// methods every frame, passing the tightly packed RGBA8 frame and a reusable
/// output `Uint8Array` of exactly `scope_w * scope_h * 4` bytes. `wasm-bindgen`
/// copies the frame in and the result out; no per-frame allocation occurs
/// inside wasm.
///
/// Mode selectors are small integers to avoid per-frame string marshalling:
/// waveform `0=luma, 1=rgb-parade, 2=rgb-overlay, 3=ycbcr`; histogram
/// `0=luma, 1=rgb`; false colour `0=spectrum, 1=arri`.
#[wasm_bindgen]
pub struct Scopes {
    inner: ScopeRenderer,
    graticule: bool,
    /// Resident copy of the most recently [`Scopes::load_frame`]d frame
    /// (grow-once; reused across loads).
    frame: Vec<u8>,
    frame_w: u32,
    frame_h: u32,
}

#[wasm_bindgen]
impl Scopes {
    /// Creates a renderer for a `scope_w x scope_h` output canvas.
    ///
    /// `graticule` is the default overlay preference echoed back by
    /// [`Scopes::default_graticule`]; each render call still takes an explicit
    /// flag so it can be toggled per frame.
    ///
    /// # Errors
    /// Throws if either dimension is zero.
    #[wasm_bindgen(constructor)]
    pub fn new(scope_w: u32, scope_h: u32, graticule: bool) -> std::result::Result<Scopes, JsValue> {
        let inner = ScopeRenderer::new(scope_w, scope_h).map_err(js_err)?;
        Ok(Self {
            inner,
            graticule,
            frame: Vec::new(),
            frame_w: 0,
            frame_h: 0,
        })
    }

    /// Loads a tightly packed RGBA8 frame into wasm-side memory for the
    /// `*_current` render methods.
    ///
    /// Rendering several scopes of the **same** frame (the standard
    /// multi-scope dashboard layout) through the plain render methods pays
    /// the JS→wasm frame copy once *per scope*; loading the frame once and
    /// calling `waveform_current` / `vectorscope_current` / … pays it once
    /// per frame. The resident buffer grows once and is reused.
    ///
    /// # Errors
    /// Throws on zero dimensions or a wrong-length frame.
    pub fn load_frame(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
    ) -> std::result::Result<(), JsValue> {
        oximedia_web_core::validate_rgba8(frame, fw as usize, fh as usize)
            .map_err(|e| js_err(ScopeError::Core(e)))?;
        self.frame.clear();
        self.frame.extend_from_slice(frame);
        self.frame_w = fw;
        self.frame_h = fh;
        Ok(())
    }

    /// Whether a frame is resident (see [`Scopes::load_frame`]).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn has_frame(&self) -> bool {
        self.frame_w != 0 && self.frame_h != 0
    }

    fn ensure_frame(&self) -> std::result::Result<(), JsValue> {
        if self.frame_w == 0 || self.frame_h == 0 {
            return Err(no_frame_err());
        }
        Ok(())
    }

    /// [`Scopes::waveform`] over the [`Scopes::load_frame`]-resident frame.
    ///
    /// # Errors
    /// Throws if no frame is loaded, plus the plain method's errors.
    pub fn waveform_current(
        &mut self,
        mode: u32,
        graticule: bool,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        self.ensure_frame()?;
        let mode = WaveformMode::from_u32(mode).map_err(js_err)?;
        self.inner
            .waveform(&self.frame, self.frame_w, self.frame_h, mode, graticule, out)
            .map_err(js_err)
    }

    /// [`Scopes::vectorscope`] over the [`Scopes::load_frame`]-resident frame.
    ///
    /// # Errors
    /// Throws if no frame is loaded, plus the plain method's errors.
    pub fn vectorscope_current(
        &mut self,
        gain: f32,
        skin_tone: bool,
        graticule: bool,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        self.ensure_frame()?;
        self.inner
            .vectorscope(
                &self.frame,
                self.frame_w,
                self.frame_h,
                gain,
                skin_tone,
                graticule,
                out,
            )
            .map_err(js_err)
    }

    /// [`Scopes::histogram`] over the [`Scopes::load_frame`]-resident frame.
    ///
    /// # Errors
    /// Throws if no frame is loaded, plus the plain method's errors.
    pub fn histogram_current(
        &mut self,
        mode: u32,
        graticule: bool,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        self.ensure_frame()?;
        let kind = HistogramKind::from_u32(mode).map_err(js_err)?;
        self.inner
            .histogram(&self.frame, self.frame_w, self.frame_h, kind, graticule, out)
            .map_err(js_err)
    }

    /// [`Scopes::false_color`] over the [`Scopes::load_frame`]-resident frame.
    ///
    /// # Errors
    /// Throws if no frame is loaded, plus the plain method's errors.
    pub fn false_color_current(
        &mut self,
        preset: u32,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        self.ensure_frame()?;
        let preset = FalseColorPreset::from_u32(preset).map_err(js_err)?;
        self.inner
            .false_color(&self.frame, self.frame_w, self.frame_h, preset, out)
            .map_err(js_err)
    }

    /// [`Scopes::stats`] over the [`Scopes::load_frame`]-resident frame.
    ///
    /// # Errors
    /// Throws if no frame is loaded, plus the plain method's errors.
    pub fn stats_current(&self) -> std::result::Result<ScopeStats, JsValue> {
        self.ensure_frame()?;
        let stats = self
            .inner
            .compute_stats(&self.frame, self.frame_w, self.frame_h)
            .map_err(js_err)?;
        Ok(ScopeStats { inner: stats })
    }

    /// The scope canvas width.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn width(&self) -> u32 {
        self.inner.width()
    }

    /// The scope canvas height.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn height(&self) -> u32 {
        self.inner.height()
    }

    /// The default graticule preference passed at construction.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn default_graticule(&self) -> bool {
        self.graticule
    }

    /// Renders a waveform (`mode`: 0=luma, 1=rgb-parade, 2=rgb-overlay,
    /// 3=ycbcr) into `out`.
    ///
    /// # Errors
    /// Throws on an unknown mode, a wrong-length frame, or a wrong-length `out`.
    pub fn waveform(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        mode: u32,
        graticule: bool,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        let mode = WaveformMode::from_u32(mode).map_err(js_err)?;
        self.inner
            .waveform(frame, fw, fh, mode, graticule, out)
            .map_err(js_err)
    }

    /// Renders the vectorscope into `out`.
    ///
    /// # Errors
    /// Throws on a wrong-length frame or `out`.
    #[allow(clippy::too_many_arguments)]
    pub fn vectorscope(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        gain: f32,
        skin_tone: bool,
        graticule: bool,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        self.inner
            .vectorscope(frame, fw, fh, gain, skin_tone, graticule, out)
            .map_err(js_err)
    }

    /// Renders a histogram (`mode`: 0=luma, 1=rgb) into `out`.
    ///
    /// # Errors
    /// Throws on an unknown mode, a wrong-length frame, or a wrong-length `out`.
    pub fn histogram(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        mode: u32,
        graticule: bool,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        let kind = HistogramKind::from_u32(mode).map_err(js_err)?;
        self.inner
            .histogram(frame, fw, fh, kind, graticule, out)
            .map_err(js_err)
    }

    /// Renders a false-colour exposure map (`preset`: 0=spectrum, 1=arri) into
    /// `out`, always at exactly the configured scope size.
    ///
    /// # Errors
    /// Throws on an unknown preset, a wrong-length frame, or a wrong-length
    /// `out`.
    pub fn false_color(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        preset: u32,
        out: &mut [u8],
    ) -> std::result::Result<(), JsValue> {
        let preset = FalseColorPreset::from_u32(preset).map_err(js_err)?;
        self.inner
            .false_color(frame, fw, fh, preset, out)
            .map_err(js_err)
    }

    /// Computes luma statistics for a frame.
    ///
    /// # Errors
    /// Throws on a wrong-length frame.
    pub fn stats(
        &self,
        frame: &[u8],
        fw: u32,
        fh: u32,
    ) -> std::result::Result<ScopeStats, JsValue> {
        let stats = self.inner.compute_stats(frame, fw, fh).map_err(js_err)?;
        Ok(ScopeStats { inner: stats })
    }
}

/// A small, JSON-free statistics record with `f32` getters.
#[wasm_bindgen]
pub struct ScopeStats {
    inner: Stats,
}

#[wasm_bindgen]
impl ScopeStats {
    /// Minimum luma (0..=255).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn min_luma(&self) -> f32 {
        f32::from(self.inner.min_luma)
    }

    /// Maximum luma (0..=255).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn max_luma(&self) -> f32 {
        f32::from(self.inner.max_luma)
    }

    /// Mean luma.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn avg_luma(&self) -> f32 {
        self.inner.avg_luma
    }

    /// Luma standard deviation.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn std_dev(&self) -> f32 {
        self.inner.std_dev
    }

    /// Percentage of pixels below legal black (< 16).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn black_clip_percent(&self) -> f32 {
        self.inner.black_clip_percent
    }

    /// Percentage of pixels above legal white (> 235).
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn white_clip_percent(&self) -> f32 {
        self.inner.white_clip_percent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }

    #[test]
    fn resident_frame_renders_match_plain_renders() {
        let (sw, sh) = (96u32, 64u32);
        let mut scopes = Scopes::new(sw, sh, true).expect("scopes");
        let (fw, fh) = (32u32, 16u32);
        let frame: Vec<u8> = (0..fw * fh * 4).map(|i| (i.wrapping_mul(37)) as u8).collect();
        let mut plain = vec![0u8; (sw * sh * 4) as usize];
        let mut resident = vec![0u8; (sw * sh * 4) as usize];

        assert!(!scopes.has_frame());
        assert!(
            scopes.waveform_current(0, true, &mut resident).is_err(),
            "must demand load_frame first"
        );

        scopes.load_frame(&frame, fw, fh).expect("load");
        assert!(scopes.has_frame());

        scopes.waveform(&frame, fw, fh, 1, true, &mut plain).expect("wf");
        scopes.waveform_current(1, true, &mut resident).expect("wf current");
        assert_eq!(plain, resident, "waveform mismatch");

        scopes
            .vectorscope(&frame, fw, fh, 1.0, true, true, &mut plain)
            .expect("vs");
        scopes
            .vectorscope_current(1.0, true, true, &mut resident)
            .expect("vs current");
        assert_eq!(plain, resident, "vectorscope mismatch");

        scopes.histogram(&frame, fw, fh, 1, true, &mut plain).expect("h");
        scopes.histogram_current(1, true, &mut resident).expect("h current");
        assert_eq!(plain, resident, "histogram mismatch");

        scopes.false_color(&frame, fw, fh, 1, &mut plain).expect("fc");
        scopes.false_color_current(1, &mut resident).expect("fc current");
        assert_eq!(plain, resident, "false colour mismatch");

        // Bad geometry is rejected without disturbing the resident frame.
        assert!(scopes.load_frame(&frame, fw + 1, fh).is_err());
        assert!(scopes.has_frame());
    }

    #[test]
    fn mode_selectors_round_trip() {
        assert_eq!(WaveformMode::from_u32(0), Ok(WaveformMode::Luma));
        assert_eq!(WaveformMode::from_u32(3), Ok(WaveformMode::Ycbcr));
        assert!(WaveformMode::from_u32(9).is_err());
        assert_eq!(HistogramKind::from_u32(1), Ok(HistogramKind::Rgb));
        assert_eq!(FalseColorPreset::from_u32(1), Ok(FalseColorPreset::Arri));
        assert!(FalseColorPreset::from_u32(7).is_err());
    }
}
