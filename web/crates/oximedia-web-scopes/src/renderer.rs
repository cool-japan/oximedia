// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! [`ScopeRenderer`] — the allocation-free orchestrator behind every scope.
//!
//! Everything a frame needs is pre-sized at construction from the fixed scope
//! canvas dimensions and grown at most once when a larger *input* frame arrives
//! (only the `x -> column` map depends on input width). A render call clears
//! its accumulators, scatters the frame into them, normalises, draws the trace
//! onto the caller's output buffer and overlays the graticule — with zero
//! heap traffic on the steady-state path.
//!
//! Matrix conventions match the native `oximedia-scopes`: luma / histogram /
//! false-colour / stats use BT.709; the vectorscope and YCbCr parade use the
//! BT.601 full-range fixed-point kernel (bit-exact with the native SIMD path).

use crate::canvas::CanvasMut;
use crate::error::{Result, ScopeError};
use crate::false_color::{self, Preset};
use crate::graticule;
use crate::histogram;
use crate::vectorscope;
use crate::waveform;
use oximedia_web_core::{
    rgb_to_ycbcr_bt601, rgb_to_ycbcr_bt709, validate_rgba8, FrameDims,
};

// The scope kernels take their RGB -> YCbCr conversion as a generic `Fn`
// parameter and every call below passes one of these two function *items*
// (zero-sized types, statically dispatched and inlined into the pixel
// loops). Storing them in `fn`-pointer constants — the previous shape —
// forced an indirect call per pixel, which cost more than the conversion
// itself.
//
// * luma / histogram / false-colour / stats: BT.709.
// * vectorscope / YCbCr parade: BT.601 full-range (broadcast convention).

/// Waveform display mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveformMode {
    /// Single luma trace.
    Luma,
    /// R | G | B side-by-side parade.
    RgbParade,
    /// Additive R+G+B overlay on one graticule.
    RgbOverlay,
    /// Y | Cb | Cr side-by-side parade.
    Ycbcr,
}

impl WaveformMode {
    /// Maps a wasm-boundary `u32` selector to a mode.
    ///
    /// # Errors
    /// [`ScopeError::InvalidMode`] for an unknown selector.
    pub fn from_u32(v: u32) -> Result<Self> {
        match v {
            0 => Ok(Self::Luma),
            1 => Ok(Self::RgbParade),
            2 => Ok(Self::RgbOverlay),
            3 => Ok(Self::Ycbcr),
            other => Err(ScopeError::InvalidMode(other)),
        }
    }
}

/// Histogram display mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistogramKind {
    /// Single luma histogram.
    Luma,
    /// Per-channel RGB overlay.
    Rgb,
}

impl HistogramKind {
    /// Maps a wasm-boundary `u32` selector to a mode.
    ///
    /// # Errors
    /// [`ScopeError::InvalidMode`] for an unknown selector.
    pub fn from_u32(v: u32) -> Result<Self> {
        match v {
            0 => Ok(Self::Luma),
            1 => Ok(Self::Rgb),
            other => Err(ScopeError::InvalidMode(other)),
        }
    }
}

impl Preset {
    /// Maps a wasm-boundary `u32` selector to a false-colour preset.
    ///
    /// # Errors
    /// [`ScopeError::InvalidPreset`] for an unknown selector.
    pub fn from_u32(v: u32) -> Result<Self> {
        match v {
            0 => Ok(Self::Spectrum),
            1 => Ok(Self::Arri),
            other => Err(ScopeError::InvalidPreset(other)),
        }
    }
}

/// Luma statistics for a frame (all `f32` for a clean wasm boundary).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stats {
    /// Minimum luma seen (0..=255).
    pub min_luma: u8,
    /// Maximum luma seen (0..=255).
    pub max_luma: u8,
    /// Mean luma.
    pub avg_luma: f32,
    /// Luma standard deviation.
    pub std_dev: f32,
    /// Percentage of pixels below legal black (< 16).
    pub black_clip_percent: f32,
    /// Percentage of pixels above legal white (> 235).
    pub white_clip_percent: f32,
}

/// The reusable scope renderer.
pub struct ScopeRenderer {
    dims: FrameDims,
    acc0: Vec<u32>,
    acc1: Vec<u32>,
    acc2: Vec<u32>,
    col_map: Vec<u32>,
    row_lut: [u32; 256],
    hist_rgb: [[u32; 256]; 3],
    hist_luma: [u32; 256],
    /// Reused per-row conversion planes (luma or Y/Cb/Cr), grown once to
    /// `3 * fw` bytes — lets each kernel run a vectorisable conversion
    /// sweep before its scalar scatter.
    row_buf: Vec<u8>,
    /// Reused scope-column → source-column table for the false-colour
    /// nearest resample; fixed at `scope_w` entries.
    fx_map: Vec<u32>,
}

impl ScopeRenderer {
    /// Creates a renderer for a `scope_w x scope_h` output canvas, pre-sizing
    /// all accumulators.
    ///
    /// # Errors
    /// [`ScopeError::Core`] if either dimension is zero or the canvas area
    /// overflows `usize`.
    pub fn new(scope_w: u32, scope_h: u32) -> Result<Self> {
        let dims = FrameDims::new(scope_w as usize, scope_h as usize)?;
        let area = dims.pixel_count()?;
        let mut row_lut = [0u32; 256];
        let h = scope_h;
        for (v, slot) in row_lut.iter_mut().enumerate() {
            let mapped = (v as u32 * h / 255).min(h - 1);
            *slot = h - 1 - mapped;
        }
        Ok(Self {
            dims,
            acc0: vec![0; area],
            acc1: vec![0; area],
            acc2: vec![0; area],
            col_map: Vec::new(),
            row_lut,
            hist_rgb: [[0; 256]; 3],
            hist_luma: [0; 256],
            row_buf: Vec::new(),
            fx_map: vec![0; scope_w as usize],
        })
    }

    /// Configured scope width.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.dims.width as u32
    }

    /// Configured scope height.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.dims.height as u32
    }

    /// Total `u32` capacity across the three trace accumulators (a no-growth
    /// probe for the allocation-discipline tests).
    #[must_use]
    pub fn accumulator_capacity(&self) -> usize {
        self.acc0.capacity()
            + self.acc1.capacity()
            + self.acc2.capacity()
            + self.col_map.capacity()
            + self.row_buf.capacity()
            + self.fx_map.capacity()
    }

    /// Grows the shared conversion row buffer to at least `3 * fw` bytes
    /// (three consecutive `fw`-byte planes), reusing it across frames.
    fn fill_row_buf(&mut self, fw: u32) {
        let n = fw as usize * 3;
        if self.row_buf.len() < n {
            self.row_buf.resize(n, 0);
        }
    }

    /// The luma histogram bins from the most recent [`Self::histogram`] call
    /// with [`HistogramKind::Luma`].
    #[must_use]
    pub fn last_luma_histogram(&self) -> &[u32; 256] {
        &self.hist_luma
    }

    /// Validates frame + output geometry, returning the input [`FrameDims`].
    fn check(&self, frame: &[u8], fw: u32, fh: u32, out: &[u8]) -> Result<FrameDims> {
        let in_dims = FrameDims::new(fw as usize, fh as usize)?;
        validate_rgba8(frame, fw as usize, fh as usize)?;
        let expected = self.dims.rgba8_len()?;
        if out.len() != expected {
            return Err(ScopeError::OutputLength {
                expected,
                actual: out.len(),
            });
        }
        Ok(in_dims)
    }

    /// Fills `col_map[..fw]` with `x -> x * span / fw`, growing once.
    fn fill_col_map(&mut self, fw: u32, span: u32) {
        let n = fw as usize;
        if self.col_map.len() < n {
            self.col_map.resize(n, 0);
        }
        for (x, slot) in self.col_map[..n].iter_mut().enumerate() {
            *slot = x as u32 * span / fw;
        }
    }

    /// Renders a waveform of `mode` into `out`.
    ///
    /// # Errors
    /// Length / geometry errors from [`Self::check`], or
    /// [`ScopeError::ScopeTooSmall`] for a parade in fewer than three columns.
    pub fn waveform(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        mode: WaveformMode,
        graticule_on: bool,
        out: &mut [u8],
    ) -> Result<()> {
        self.check(frame, fw, fh, out)?;
        // Warmed for every mode (not just the luma/YCbCr users) so a single
        // warm-up render of any mode settles the renderer's capacity.
        self.fill_row_buf(fw);
        let (w, h) = (self.width(), self.height());
        match mode {
            WaveformMode::Luma => {
                self.fill_col_map(fw, w);
                let n = fw as usize;
                let Self {
                    acc0,
                    col_map,
                    row_lut,
                    row_buf,
                    ..
                } = self;
                let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
                waveform::luma(
                    &mut canvas,
                    acc0,
                    &col_map[..n],
                    row_lut,
                    frame,
                    fw,
                    fh,
                    rgb_to_ycbcr_bt709,
                    row_buf,
                );
                if graticule_on {
                    graticule::waveform(&mut canvas, true);
                }
            }
            WaveformMode::RgbOverlay => {
                self.fill_col_map(fw, w);
                let n = fw as usize;
                let Self {
                    acc0,
                    acc1,
                    acc2,
                    col_map,
                    row_lut,
                    ..
                } = self;
                let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
                waveform::rgb_overlay(
                    &mut canvas,
                    acc0,
                    acc1,
                    acc2,
                    &col_map[..n],
                    row_lut,
                    frame,
                    fw,
                    fh,
                );
                if graticule_on {
                    graticule::waveform(&mut canvas, true);
                }
            }
            WaveformMode::RgbParade => {
                let section_w = w / 3;
                if section_w == 0 {
                    return Err(ScopeError::ScopeTooSmall);
                }
                self.fill_col_map(fw, section_w);
                let n = fw as usize;
                let Self {
                    acc0,
                    acc1,
                    acc2,
                    col_map,
                    row_lut,
                    ..
                } = self;
                let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
                waveform::rgb_parade(
                    &mut canvas,
                    acc0,
                    acc1,
                    acc2,
                    &col_map[..n],
                    row_lut,
                    frame,
                    fw,
                    fh,
                    section_w,
                );
                if graticule_on {
                    graticule::parade(&mut canvas, 3, &["R", "G", "B"], true);
                }
            }
            WaveformMode::Ycbcr => {
                let section_w = w / 3;
                if section_w == 0 {
                    return Err(ScopeError::ScopeTooSmall);
                }
                self.fill_col_map(fw, section_w);
                let n = fw as usize;
                let Self {
                    acc0,
                    acc1,
                    acc2,
                    col_map,
                    row_lut,
                    row_buf,
                    ..
                } = self;
                let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
                waveform::ycbcr_parade(
                    &mut canvas,
                    acc0,
                    acc1,
                    acc2,
                    &col_map[..n],
                    row_lut,
                    frame,
                    fw,
                    fh,
                    section_w,
                    rgb_to_ycbcr_bt601,
                    row_buf,
                );
                if graticule_on {
                    graticule::parade(&mut canvas, 3, &["Y", "CB", "CR"], true);
                }
            }
        }
        Ok(())
    }

    /// Renders the vectorscope into `out`.
    ///
    /// # Errors
    /// Length / geometry errors from [`Self::check`].
    #[allow(clippy::too_many_arguments)]
    pub fn vectorscope(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        gain: f32,
        skin_tone: bool,
        graticule_on: bool,
        out: &mut [u8],
    ) -> Result<()> {
        self.check(frame, fw, fh, out)?;
        self.fill_row_buf(fw);
        let (w, h) = (self.width(), self.height());
        let Self { acc0, row_buf, .. } = self;
        let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
        vectorscope::render(
            &mut canvas,
            acc0,
            frame,
            fw,
            fh,
            gain,
            rgb_to_ycbcr_bt601,
            row_buf,
        );
        if graticule_on {
            graticule::vectorscope(&mut canvas, skin_tone, true);
        }
        Ok(())
    }

    /// Renders a histogram of `kind` into `out`.
    ///
    /// # Errors
    /// Length / geometry errors from [`Self::check`].
    pub fn histogram(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        kind: HistogramKind,
        graticule_on: bool,
        out: &mut [u8],
    ) -> Result<()> {
        self.check(frame, fw, fh, out)?;
        self.fill_row_buf(fw);
        let (w, h) = (self.width(), self.height());
        match kind {
            HistogramKind::Luma => {
                let Self {
                    hist_luma, row_buf, ..
                } = self;
                let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
                histogram::luma(
                    &mut canvas,
                    hist_luma,
                    frame,
                    fw,
                    fh,
                    rgb_to_ycbcr_bt709,
                    row_buf,
                );
                if graticule_on {
                    graticule::histogram(&mut canvas);
                }
            }
            HistogramKind::Rgb => {
                let Self { hist_rgb, .. } = self;
                let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
                histogram::rgb(&mut canvas, hist_rgb, frame, fw, fh);
                if graticule_on {
                    graticule::histogram(&mut canvas);
                }
            }
        }
        Ok(())
    }

    /// Renders a false-colour exposure map (nearest-sampled into the scope
    /// canvas — always exactly the configured size) into `out`.
    ///
    /// # Errors
    /// Length / geometry errors from [`Self::check`].
    pub fn false_color(
        &mut self,
        frame: &[u8],
        fw: u32,
        fh: u32,
        preset: Preset,
        out: &mut [u8],
    ) -> Result<()> {
        self.check(frame, fw, fh, out)?;
        let (w, h) = (self.width(), self.height());
        let Self { fx_map, .. } = self;
        let mut canvas = CanvasMut::new(out, w, h).ok_or(ScopeError::ScopeTooSmall)?;
        false_color::render(&mut canvas, frame, fw, fh, preset, rgb_to_ycbcr_bt709, fx_map);
        Ok(())
    }

    /// Computes luma [`Stats`] for a frame (BT.709).
    ///
    /// # Errors
    /// [`ScopeError::Core`] for zero dimensions or a wrong-length frame.
    #[allow(clippy::cast_precision_loss)]
    pub fn compute_stats(&self, frame: &[u8], fw: u32, fh: u32) -> Result<Stats> {
        FrameDims::new(fw as usize, fh as usize)?;
        validate_rgba8(frame, fw as usize, fh as usize)?;
        let mut sum = 0u64;
        let mut sum_sq = 0u64;
        let mut min = 255u8;
        let mut max = 0u8;
        let mut black = 0u64;
        let mut white = 0u64;
        for px in frame.chunks_exact(4) {
            let luma = rgb_to_ycbcr_bt709(px[0], px[1], px[2])[0];
            sum += u64::from(luma);
            sum_sq += u64::from(luma) * u64::from(luma);
            min = min.min(luma);
            max = max.max(luma);
            if luma < 16 {
                black += 1;
            }
            if luma > 235 {
                white += 1;
            }
        }
        let n = frame.len() as u64 / 4;
        let count = n.max(1) as f32;
        let avg = sum as f32 / count;
        let variance = (sum_sq as f32 / count) - avg * avg;
        Ok(Stats {
            min_luma: min,
            max_luma: max,
            avg_luma: avg,
            std_dev: variance.max(0.0).sqrt(),
            black_clip_percent: black as f32 / count * 100.0,
            white_clip_percent: white as f32 / count * 100.0,
        })
    }
}
