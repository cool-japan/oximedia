// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! [`Resizer`]: the allocation-free separable resampling engine.
//!
//! Ported (not depended on) from the two-pass horizontal/vertical structure
//! of `Resampler::resize_horizontal`/`resize_vertical`/`resize_in_place` in
//! `crates/oximedia-scaling/src/resampler.rs`, restructured around
//! precomputed [`crate::weights::WeightTable`]s (one build per axis, reused
//! for every frame) instead of recomputing filter taps per output pixel on
//! every call.

use oximedia_web_core::{f32_to_u8, rgba8_len, u8_to_f32, validate_rgba8, validate_rgba_f32};

use crate::error::ScaleError;
use crate::filter::Filter;
use crate::weights::WeightTable;

/// Number of interleaved channels per pixel (R, G, B, A).
const CHANNELS: usize = 4;

/// Below this alpha, unpremultiply divides by (near) zero and instead
/// yields `0.0` for the color channels rather than `NaN`/`Inf`.
const UNPREMULTIPLY_EPSILON: f32 = 1e-6;

/// A reusable, allocation-free separable image/video resampler for
/// interleaved RGBA frames.
///
/// Construction ([`Resizer::new`]) is the only place that allocates: it
/// precomputes the horizontal and vertical [`WeightTable`]s and pre-sizes
/// every scratch buffer the resize passes will ever need. [`Self::resize_rgba8`]
/// and [`Self::resize_f32`] then run with zero per-call allocation, so a
/// `Resizer` built once for a video stream's resolution can be reused for
/// every subsequent frame.
///
/// # Pipeline (row-streamed, both passes fused with their conversions)
///
/// 1. Horizontal pass, one source row at a time: convert the row to `f32`
///    (optionally premultiplying RGB by alpha) into a single reused row
///    buffer, then resample it `src_w -> dst_w` into `h_buf`. Fusing the
///    conversion avoids materialising a full-frame `f32` copy of the
///    source (132 MB for a 4K frame — more DRAM traffic than the
///    arithmetic it feeds).
/// 2. Vertical pass, one destination row at a time: accumulate the
///    contributing `h_buf` rows as long contiguous saxpys
///    (`acc += w * row`, the classic autovectorizable form), then
///    optionally unpremultiply and convert back to the output type
///    straight into `dst` — no full-frame vertical output buffer.
#[derive(Debug)]
pub struct Resizer {
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    premultiply: bool,
    h_table: WeightTable,
    v_table: WeightTable,
    /// One source row, converted to `f32` (and premultiplied if
    /// requested): `src_w * 4` elements, reused for every row.
    row_f32: Vec<f32>,
    /// Horizontal-pass output: `dst_w * src_h * 4` elements.
    h_buf: Vec<f32>,
    /// One vertical-pass accumulator row: `dst_w * 4` elements.
    acc_row: Vec<f32>,
}

impl Resizer {
    /// Builds a resizer for a fixed `(src_w, src_h) -> (dst_w, dst_h)`
    /// geometry and [`Filter`] kernel, precomputing weight tables and
    /// pre-sizing every scratch buffer used by [`Self::resize_rgba8`] /
    /// [`Self::resize_f32`].
    ///
    /// When `premultiply` is `true`, RGB is multiplied by alpha before
    /// resampling and divided back out afterwards, which prevents fully or
    /// partially transparent pixels' color from bleeding into opaque
    /// neighbors during downscale ("fringing").
    ///
    /// # Errors
    ///
    /// Returns [`ScaleError::Core`] (wrapping
    /// [`oximedia_web_core::CoreError::ZeroDimension`]) if any of
    /// `src_w`/`src_h`/`dst_w`/`dst_h` is zero, or
    /// [`oximedia_web_core::CoreError::DimensionOverflow`] if a scratch
    /// buffer's element count would overflow `usize`.
    pub fn new(
        src_w: usize,
        src_h: usize,
        dst_w: usize,
        dst_h: usize,
        filter: Filter,
        premultiply: bool,
    ) -> Result<Self, ScaleError> {
        let h_table = WeightTable::build(filter, src_w, dst_w)?;
        let v_table = WeightTable::build(filter, src_h, dst_h)?;

        // Validates the full-frame geometry (dimension-overflow errors)
        // even though only `h_buf` holds a full frame.
        rgba8_len(src_w, src_h)?;
        let h_len = rgba8_len(dst_w, src_h)?;
        rgba8_len(dst_w, dst_h)?;
        let src_row_len = rgba8_len(src_w, 1)?;
        let dst_row_len = rgba8_len(dst_w, 1)?;

        Ok(Self {
            src_w,
            src_h,
            dst_w,
            dst_h,
            premultiply,
            h_table,
            v_table,
            row_f32: vec![0.0f32; src_row_len],
            h_buf: vec![0.0f32; h_len],
            acc_row: vec![0.0f32; dst_row_len],
        })
    }

    /// Source `(width, height)` this resizer was built for.
    #[inline]
    #[must_use]
    pub fn src_dims(&self) -> (usize, usize) {
        (self.src_w, self.src_h)
    }

    /// Destination `(width, height)` this resizer was built for.
    #[inline]
    #[must_use]
    pub fn dst_dims(&self) -> (usize, usize) {
        (self.dst_w, self.dst_h)
    }

    /// Resamples a tightly packed RGBA8 `src` frame into `dst`, both sized
    /// exactly for this resizer's constructed geometry.
    ///
    /// Allocation-free: every buffer this touches was sized in
    /// [`Self::new`].
    ///
    /// # Errors
    ///
    /// Returns [`ScaleError::Core`] if `src` or `dst` is not exactly
    /// `width * height * 4` bytes for this resizer's `src`/`dst` dimensions.
    pub fn resize_rgba8(&mut self, src: &[u8], dst: &mut [u8]) -> Result<(), ScaleError> {
        validate_rgba8(src, self.src_w, self.src_h)?;
        validate_rgba8(dst, self.dst_w, self.dst_h)?;

        let dst_w = self.dst_w;
        let premultiply = self.premultiply;
        let src_row_len = self.src_w * CHANNELS;
        let dst_row_len = dst_w * CHANNELS;

        // Fields are destructured into disjoint local bindings so the
        // borrow checker can see the scratch/table fields are accessed
        // independently, without any `unsafe` aliasing tricks.
        let Self {
            h_table,
            v_table,
            row_f32,
            h_buf,
            acc_row,
            ..
        } = self;

        // Horizontal pass, fused with the u8 -> f32 (+ premultiply)
        // conversion one row at a time. `premultiply` is loop-invariant;
        // branching on it per row keeps the per-pixel loops branch-free.
        //
        // Opaque fast path: a row whose every alpha byte is 255 converts
        // *identically* with or without premultiply (`u8_to_f32(255)` is
        // exactly 1.0), so such rows take the cheaper plain conversion —
        // and when the whole frame is opaque the vertical pass skips the
        // unpremultiply divides too. The latter substitutes division by
        // the resampled alpha (1.0 up to weight-normalisation rounding)
        // with no division, which can move an output code by at most one
        // ULP-at-the-rounding-edge — both are valid renderings of an
        // opaque frame, and real video frames are opaque, making this the
        // common case.
        let mut all_opaque = true;
        for (src_row, h_row) in src
            .chunks_exact(src_row_len)
            .zip(h_buf.chunks_exact_mut(dst_row_len))
        {
            let frow = &mut row_f32[..src_row_len];
            let row_opaque = !premultiply
                || src_row
                    .chunks_exact(CHANNELS)
                    .fold(0xFFu8, |m, px| m & px[3])
                    == 0xFF;
            if premultiply && !row_opaque {
                all_opaque = false;
                for (chunk_out, chunk_in) in
                    frow.chunks_exact_mut(CHANNELS).zip(src_row.chunks_exact(CHANNELS))
                {
                    let a = u8_to_f32(chunk_in[3]);
                    chunk_out[0] = u8_to_f32(chunk_in[0]) * a;
                    chunk_out[1] = u8_to_f32(chunk_in[1]) * a;
                    chunk_out[2] = u8_to_f32(chunk_in[2]) * a;
                    chunk_out[3] = a;
                }
            } else {
                for (f, s) in frow.iter_mut().zip(src_row.iter()) {
                    *f = u8_to_f32(*s);
                }
            }
            h_pass_row(h_table, dst_w, frow, h_row);
        }

        // Vertical pass fused with the f32 -> u8 (+ unpremultiply)
        // conversion one destination row at a time.
        let unpremultiply = premultiply && !all_opaque;
        for (y, dst_row) in dst.chunks_exact_mut(dst_row_len).enumerate() {
            let acc = &mut acc_row[..dst_row_len];
            v_pass_row(v_table, y, h_buf, dst_row_len, acc);
            if unpremultiply {
                for (chunk_out, chunk_in) in
                    dst_row.chunks_exact_mut(CHANNELS).zip(acc.chunks_exact(CHANNELS))
                {
                    let a = chunk_in[3];
                    let (r, g, b) = unpremultiply_on(chunk_in[0], chunk_in[1], chunk_in[2], a);
                    chunk_out[0] = f32_to_u8(r);
                    chunk_out[1] = f32_to_u8(g);
                    chunk_out[2] = f32_to_u8(b);
                    chunk_out[3] = f32_to_u8(a);
                }
            } else {
                for (d, s) in dst_row.iter_mut().zip(acc.iter()) {
                    *d = f32_to_u8(*s);
                }
            }
        }

        Ok(())
    }

    /// Resamples a tightly packed `f32` RGBA `src` frame into `dst`, both
    /// sized exactly for this resizer's constructed geometry.
    ///
    /// Unlike [`Self::resize_rgba8`], values are never clamped to `[0, 1]`:
    /// this is the HDR/linear-light path, where samples may legitimately
    /// exceed `1.0`. Allocation-free: every buffer this touches was sized in
    /// [`Self::new`].
    ///
    /// # Errors
    ///
    /// Returns [`ScaleError::Core`] if `src` or `dst` is not exactly
    /// `width * height * 4` elements for this resizer's `src`/`dst`
    /// dimensions.
    pub fn resize_f32(&mut self, src: &[f32], dst: &mut [f32]) -> Result<(), ScaleError> {
        validate_rgba_f32(src, self.src_w, self.src_h)?;
        validate_rgba_f32(dst, self.dst_w, self.dst_h)?;

        let dst_w = self.dst_w;
        let premultiply = self.premultiply;
        let src_row_len = self.src_w * CHANNELS;
        let dst_row_len = dst_w * CHANNELS;

        let Self {
            h_table,
            v_table,
            row_f32,
            h_buf,
            acc_row,
            ..
        } = self;

        // Horizontal pass. Without premultiply the source row is consumed
        // in place (no copy at all); with it, one reused row buffer holds
        // the premultiplied pixels.
        for (src_row, h_row) in src
            .chunks_exact(src_row_len)
            .zip(h_buf.chunks_exact_mut(dst_row_len))
        {
            if premultiply {
                let frow = &mut row_f32[..src_row_len];
                for (chunk_out, chunk_in) in
                    frow.chunks_exact_mut(CHANNELS).zip(src_row.chunks_exact(CHANNELS))
                {
                    let a = chunk_in[3];
                    chunk_out[0] = chunk_in[0] * a;
                    chunk_out[1] = chunk_in[1] * a;
                    chunk_out[2] = chunk_in[2] * a;
                    chunk_out[3] = a;
                }
                h_pass_row(h_table, dst_w, frow, h_row);
            } else {
                h_pass_row(h_table, dst_w, src_row, h_row);
            }
        }

        // Vertical pass. Without premultiply each output row is
        // accumulated directly into `dst` (no copy-back).
        for (y, dst_row) in dst.chunks_exact_mut(dst_row_len).enumerate() {
            if premultiply {
                let acc = &mut acc_row[..dst_row_len];
                v_pass_row(v_table, y, h_buf, dst_row_len, acc);
                for (chunk_out, chunk_in) in
                    dst_row.chunks_exact_mut(CHANNELS).zip(acc.chunks_exact(CHANNELS))
                {
                    let a = chunk_in[3];
                    let (r, g, b) = unpremultiply_on(chunk_in[0], chunk_in[1], chunk_in[2], a);
                    chunk_out[0] = r;
                    chunk_out[1] = g;
                    chunk_out[2] = b;
                    chunk_out[3] = a;
                }
            } else {
                v_pass_row(v_table, y, h_buf, dst_row_len, dst_row);
            }
        }

        Ok(())
    }
}

/// One horizontal-pass row: resamples a `f32` source row (`src_w` pixels)
/// into `dst_row` (`dst_w` pixels) under `h_table`.
///
/// Dispatches **once per row** to a kernel monomorphised for the table's
/// tap count. The span is a runtime value, and a runtime-trip-count tap
/// loop costs ~2x on this pass (measured 43 ms -> 21 ms native for the
/// 4K -> 1080p Lanczos3 frame): with a compile-time span LLVM fully
/// unrolls the tap loop into straight-line 128-bit FMAs with no loop
/// carried bookkeeping. Spans are always even (`2*ceil(support*scale)+2`),
/// so the match covers every span the four shipped filters produce for
/// scale factors up to ~3x, with a runtime-length fallback for exotic
/// ratios.
fn h_pass_row(h_table: &WeightTable, dst_w: usize, src_row: &[f32], dst_row: &mut [f32]) {
    match h_table.span() {
        4 => h_pass_row_const::<4>(h_table, dst_w, src_row, dst_row),
        6 => h_pass_row_const::<6>(h_table, dst_w, src_row, dst_row),
        8 => h_pass_row_const::<8>(h_table, dst_w, src_row, dst_row),
        10 => h_pass_row_const::<10>(h_table, dst_w, src_row, dst_row),
        12 => h_pass_row_const::<12>(h_table, dst_w, src_row, dst_row),
        14 => h_pass_row_const::<14>(h_table, dst_w, src_row, dst_row),
        16 => h_pass_row_const::<16>(h_table, dst_w, src_row, dst_row),
        20 => h_pass_row_const::<20>(h_table, dst_w, src_row, dst_row),
        _ => h_pass_row_generic(h_table, dst_w, src_row, dst_row),
    }
}

/// [`h_pass_row`] body for a compile-time tap count `N`.
fn h_pass_row_const<const N: usize>(
    h_table: &WeightTable,
    dst_w: usize,
    src_row: &[f32],
    dst_row: &mut [f32],
) {
    for x in 0..dst_w {
        let acc = if h_table.is_interior(x) {
            // Fast path: the tap window is `N` *consecutive* source
            // pixels starting at `base`, so this is a plain contiguous
            // weighted sum (no per-tap index load) — the shape LLVM's
            // autovectorizer recognizes as a small FIR/convolution kernel.
            let base = h_table.base(x) as usize * CHANNELS;
            match <&[f32; N]>::try_from(h_table.weights_row(x)) {
                Ok(weights) => {
                    let window = &src_row[base..base + N * CHANNELS];
                    accumulate_const::<N>(weights, window)
                }
                // Unreachable (the table's rows are exactly `span` wide);
                // kept as a correct fallback rather than a panic path.
                Err(_) => {
                    let (weights, indices) = h_table.row(x);
                    accumulate_indexed(weights, indices, src_row)
                }
            }
        } else {
            let (weights, indices) = h_table.row(x);
            accumulate_indexed(weights, indices, src_row)
        };
        dst_row[x * CHANNELS..x * CHANNELS + CHANNELS].copy_from_slice(&acc);
    }
}

/// [`h_pass_row`] fallback for spans without a monomorphised kernel.
fn h_pass_row_generic(h_table: &WeightTable, dst_w: usize, src_row: &[f32], dst_row: &mut [f32]) {
    for x in 0..dst_w {
        let acc = if h_table.is_interior(x) {
            let base = h_table.base(x) as usize * CHANNELS;
            let weights = h_table.weights_row(x);
            let window = &src_row[base..base + weights.len() * CHANNELS];
            accumulate(weights, window)
        } else {
            let (weights, indices) = h_table.row(x);
            accumulate_indexed(weights, indices, src_row)
        };
        dst_row[x * CHANNELS..x * CHANNELS + CHANNELS].copy_from_slice(&acc);
    }
}

/// One vertical-pass row: accumulates output row `y` as a weighted sum of
/// whole `h_buf` rows. Each tap is one long contiguous saxpy
/// (`out += w * row`), the classic shape LLVM's autovectorizer folds into
/// `f32x4` FMAs — unlike a per-pixel strided gather, which it will not.
/// The already-clamped `v_table.row(y)` indices make one code path serve
/// interior and boundary rows alike (per-row cost is a handful of index
/// loads, amortised over `dst_w * 4` floats of saxpy).
///
/// Taps are processed **four per sweep** (`out += w0*r0 + w1*r1 + w2*r2 +
/// w3*r3`): with one tap per sweep the pass re-reads and re-writes the
/// accumulator row once per tap, and that `out` traffic — not the taps'
/// arithmetic — dominates (measured 13.5 ms -> 9 ms native for the 4K ->
/// 1080p frame). The first fused group writes `out` directly, which also
/// replaces the `fill(0.0)` sweep.
fn v_pass_row(v_table: &WeightTable, y: usize, h_buf: &[f32], row_len: usize, out: &mut [f32]) {
    let (weights, indices) = v_table.row(y);
    let row_at = |k: usize| {
        let start = indices[k] as usize * row_len;
        &h_buf[start..start + row_len]
    };
    let n = weights.len();
    let mut k = 0;
    let mut first = true;
    while k + 4 <= n {
        let (w0, w1, w2, w3) = (weights[k], weights[k + 1], weights[k + 2], weights[k + 3]);
        let (r0, r1, r2, r3) = (row_at(k), row_at(k + 1), row_at(k + 2), row_at(k + 3));
        if first {
            for i in 0..row_len {
                out[i] = w0 * r0[i] + w1 * r1[i] + w2 * r2[i] + w3 * r3[i];
            }
            first = false;
        } else {
            for i in 0..row_len {
                out[i] += w0 * r0[i] + w1 * r1[i] + w2 * r2[i] + w3 * r3[i];
            }
        }
        k += 4;
    }
    if first {
        out.fill(0.0);
    }
    while k < n {
        let w = weights[k];
        // Fixed-stride tables pad short tap windows with zero weights;
        // skipping them at row granularity saves whole-row saxpys (and
        // never changes the sum).
        if w != 0.0 {
            let src_row = row_at(k);
            for (d, s) in out.iter_mut().zip(src_row.iter()) {
                *d += w * *s;
            }
        }
        k += 1;
    }
}

/// Weighted sum of `weights.len()` consecutive `CHANNELS`-wide pixels
/// starting at `window[0]` (the interior fast path: no per-tap index load).
///
/// Written as `chunks_exact` + a fixed `0..CHANNELS` inner loop (rather than
/// four separately-written `acc[c] += ...` statements) because that shape —
/// a compile-time-constant-width reduction over a provably-contiguous
/// slice — is what LLVM's SLP vectorizer reliably folds into a single
/// 128-bit FMA per tap on both `aarch64` (NEON) and `wasm32` (`simd128`),
/// which happen to share the same 4x`f32` lane width as [`CHANNELS`].
#[inline(always)]
fn accumulate(weights: &[f32], window: &[f32]) -> [f32; CHANNELS] {
    let mut acc = [0.0f32; CHANNELS];
    for (&w, px) in weights.iter().zip(window.chunks_exact(CHANNELS)) {
        for c in 0..CHANNELS {
            acc[c] += w * px[c];
        }
    }
    acc
}

/// [`accumulate`] with a compile-time tap count and **two** independent
/// accumulator quads (even/odd taps). A single accumulator makes the tap
/// loop one serial mul-add dependency chain, so the whole output pixel
/// costs `N x` FMA *latency* instead of throughput; splitting even/odd taps
/// halves that chain, and going wider showed no further gain (register
/// pressure) while this shape measured fastest native and wasm. Floating
/// point addition is reassociated versus [`accumulate`] (documented; the
/// difference is <= 1 ULP-scale, see `u8_and_f32_paths_agree_within_one_lsb`).
#[inline(always)]
fn accumulate_const<const N: usize>(weights: &[f32; N], window: &[f32]) -> [f32; CHANNELS] {
    let mut banks = [[0.0f32; CHANNELS]; 2];
    for k in 0..N {
        let px = &window[k * CHANNELS..k * CHANNELS + CHANNELS];
        for c in 0..CHANNELS {
            banks[k & 1][c] += weights[k] * px[c];
        }
    }
    let mut acc = [0.0f32; CHANNELS];
    for c in 0..CHANNELS {
        acc[c] = banks[0][c] + banks[1][c];
    }
    acc
}

/// Weighted sum of `CHANNELS`-wide pixels gathered from `row` via
/// already-clamped `indices` (the always-correct boundary path).
#[inline(always)]
fn accumulate_indexed(weights: &[f32], indices: &[u32], row: &[f32]) -> [f32; CHANNELS] {
    let mut acc = [0.0f32; CHANNELS];
    for (&w, &idx) in weights.iter().zip(indices.iter()) {
        let px = &row[idx as usize * CHANNELS..idx as usize * CHANNELS + CHANNELS];
        for c in 0..CHANNELS {
            acc[c] += w * px[c];
        }
    }
    acc
}

/// Reverses [`Resizer`]'s premultiply-by-alpha step for one pixel, guarding
/// against division by (near) zero alpha.
///
/// Callers only reach this when `self.premultiply` is `true` (see
/// `resize_rgba8`/`resize_f32`, which branch on it once outside their
/// per-pixel loops rather than passing it down here per pixel).
#[inline]
fn unpremultiply_on(r: f32, g: f32, b: f32, a: f32) -> (f32, f32, f32) {
    if a > UNPREMULTIPLY_EPSILON {
        (r / a, g / a, b / a)
    } else {
        (0.0, 0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkerboard_rgba8(w: usize, h: usize) -> Vec<u8> {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let on = (x + y) % 2 == 0;
                let v = if on { 255 } else { 0 };
                let off = (y * w + x) * 4;
                buf[off] = v;
                buf[off + 1] = v;
                buf[off + 2] = v;
                buf[off + 3] = 255;
            }
        }
        buf
    }

    #[test]
    fn zero_dimension_is_rejected_never_panics() {
        assert!(Resizer::new(0, 4, 4, 4, Filter::Bilinear, false).is_err());
        assert!(Resizer::new(4, 4, 0, 4, Filter::Bilinear, false).is_err());
        assert!(Resizer::new(4, 4, 4, 4, Filter::Bilinear, false).is_ok());
    }

    #[test]
    fn mismatched_buffer_length_errors_never_panics() {
        let mut resizer = Resizer::new(4, 4, 2, 2, Filter::Bilinear, false).unwrap();
        let src_bad = vec![0u8; 4]; // way too short
        let mut dst = vec![0u8; 2 * 2 * 4];
        assert!(resizer.resize_rgba8(&src_bad, &mut dst).is_err());

        let src = vec![0u8; 4 * 4 * 4];
        let mut dst_bad = vec![0u8; 3]; // too short
        assert!(resizer.resize_rgba8(&src, &mut dst_bad).is_err());
    }

    #[test]
    fn identity_resize_is_byte_identical_for_interpolating_filters() {
        // Bilinear, Catmull-Rom and Lanczos3 are *interpolating* kernels:
        // `evaluate(n) == 0` for every nonzero integer `n`, so at identity
        // scale (taps land exactly on integer offsets) only the tap at
        // offset 0 contributes and the source is reproduced exactly.
        // Mitchell-Netravali (B=C=1/3) is deliberately *not* interpolating
        // (see `filter::tests::mitchell_center_matches_closed_form`'s
        // neighbor, `Filter::evaluate(1.0) != 0`), so it is excluded here
        // and covered instead by `constant_color_image_stays_constant_dc_preservation`.
        for filter in [Filter::Bilinear, Filter::CatmullRom, Filter::Lanczos3] {
            let w = 17usize;
            let h = 11usize;
            let src: Vec<u8> = (0..(w * h * 4))
                .map(|i| ((i * 37 + 5) % 256) as u8)
                .collect();
            let mut resizer = Resizer::new(w, h, w, h, filter, false).unwrap();
            let mut dst = vec![0u8; w * h * 4];
            resizer.resize_rgba8(&src, &mut dst).unwrap();
            assert_eq!(src, dst, "{filter:?} identity resize must be byte-identical");
        }
    }

    #[test]
    fn constant_color_image_stays_constant_dc_preservation() {
        for filter in [
            Filter::Bilinear,
            Filter::CatmullRom,
            Filter::Mitchell,
            Filter::Lanczos3,
        ] {
            let (r, g, b, a) = (200u8, 90u8, 10u8, 255u8);
            let src_w = 33;
            let src_h = 29;
            let mut src = vec![0u8; src_w * src_h * 4];
            for px in src.chunks_exact_mut(4) {
                px[0] = r;
                px[1] = g;
                px[2] = b;
                px[3] = a;
            }
            for (dst_w, dst_h) in [(16, 13), (64, 57), (33, 29)] {
                let mut resizer =
                    Resizer::new(src_w, src_h, dst_w, dst_h, filter, false).unwrap();
                let mut dst = vec![0u8; dst_w * dst_h * 4];
                resizer.resize_rgba8(&src, &mut dst).unwrap();
                for px in dst.chunks_exact(4) {
                    assert!((i32::from(px[0]) - i32::from(r)).abs() <= 1, "{filter:?}");
                    assert!((i32::from(px[1]) - i32::from(g)).abs() <= 1, "{filter:?}");
                    assert!((i32::from(px[2]) - i32::from(b)).abs() <= 1, "{filter:?}");
                    assert!((i32::from(px[3]) - i32::from(a)).abs() <= 1, "{filter:?}");
                }
            }
        }
    }

    #[test]
    fn impulse_response_is_symmetric() {
        // A single bright pixel exactly centered in an otherwise black
        // source, resized at *identity* scale so the output-to-source
        // sample mapping `(i + 0.5) * scale - 0.5` lands the point of
        // symmetry exactly on an integer output index (source index `c`,
        // odd dimension `2c+1`) rather than between two pixels — a
        // non-identity scale factor shifts that alignment by a fractional
        // pixel and makes a strict left/right index comparison invalid even
        // though the underlying kernel is perfectly symmetric.
        let size = 17usize;
        let center = size / 2;
        let mut src = vec![0u8; size * size * 4];
        let off = (center * size + center) * 4;
        src[off] = 255;
        src[off + 1] = 255;
        src[off + 2] = 255;
        src[off + 3] = 255;

        let mut resizer =
            Resizer::new(size, size, size, size, Filter::Lanczos3, false).unwrap();
        let mut dst = vec![0u8; size * size * 4];
        resizer.resize_rgba8(&src, &mut dst).unwrap();

        // Compare the row through the response peak reflected around its
        // own center: sample values at +k and -k from center should match.
        let row = &dst[center * size * 4..(center + 1) * size * 4];
        for k in 1..=center {
            let left = row[(center - k) * 4] as i32;
            let right = row[(center + k) * 4] as i32;
            assert!((left - right).abs() <= 1, "k={k} left={left} right={right}");
        }
    }

    #[test]
    fn checkerboard_4k_to_1080p_mean_is_preserved() {
        let src_w = 3840;
        let src_h = 2160;
        let src = checkerboard_rgba8(src_w, src_h);
        let dst_w = 1920;
        let dst_h = 1080;
        let mut resizer =
            Resizer::new(src_w, src_h, dst_w, dst_h, Filter::Lanczos3, false).unwrap();
        let mut dst = vec![0u8; dst_w * dst_h * 4];
        resizer.resize_rgba8(&src, &mut dst).unwrap();

        let src_mean: f64 = src
            .chunks_exact(4)
            .map(|px| f64::from(px[0]))
            .sum::<f64>()
            / (src_w * src_h) as f64;
        let dst_mean: f64 = dst
            .chunks_exact(4)
            .map(|px| f64::from(px[0]))
            .sum::<f64>()
            / (dst_w * dst_h) as f64;
        assert!(
            (src_mean - dst_mean).abs() <= 1.0,
            "src_mean={src_mean} dst_mean={dst_mean}"
        );
    }

    #[test]
    fn premultiply_prevents_red_fringing_next_to_opaque_green() {
        // Left half: fully transparent red. Right half: fully opaque green.
        // Downscaling across the boundary without premultiplying blends the
        // *unweighted* red into the result (visible red fringe on the
        // opaque side); premultiplying suppresses the transparent pixel's
        // color contribution in proportion to its (zero) alpha.
        let w = 16;
        let h = 4;
        let mut src = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let off = (y * w + x) * 4;
                if x < w / 2 {
                    // Transparent red.
                    src[off] = 255;
                    src[off + 1] = 0;
                    src[off + 2] = 0;
                    src[off + 3] = 0;
                } else {
                    // Opaque green.
                    src[off] = 0;
                    src[off + 1] = 255;
                    src[off + 2] = 0;
                    src[off + 3] = 255;
                }
            }
        }

        let dst_w = 4;
        let dst_h = 1;

        let mut premul = Resizer::new(w, h, dst_w, dst_h, Filter::Bilinear, true).unwrap();
        let mut dst_premul = vec![0u8; dst_w * dst_h * 4];
        premul.resize_rgba8(&src, &mut dst_premul).unwrap();

        let mut naive = Resizer::new(w, h, dst_w, dst_h, Filter::Bilinear, false).unwrap();
        let mut dst_naive = vec![0u8; dst_w * dst_h * 4];
        naive.resize_rgba8(&src, &mut dst_naive).unwrap();

        // The output pixel(s) that straddle the red/green boundary must
        // carry noticeably less red bleed with premultiply on than off, and
        // must not be near-opaque-red the way the naive average pulls it.
        let boundary_px = &dst_premul[(dst_w / 2 - 1) * 4..];
        let naive_px = &dst_naive[(dst_w / 2 - 1) * 4..];
        assert!(
            boundary_px[0] <= naive_px[0],
            "premultiplied red={} should be <= naive red={}",
            boundary_px[0],
            naive_px[0]
        );
    }

    #[test]
    fn odd_prime_dimensions_resize_without_panicking() {
        let src_w = 641;
        let src_h = 359;
        let dst_w = 123;
        let dst_h = 77;
        let src: Vec<u8> = (0..(src_w * src_h * 4))
            .map(|i| ((i * 91 + 13) % 256) as u8)
            .collect();
        for filter in [
            Filter::Bilinear,
            Filter::CatmullRom,
            Filter::Mitchell,
            Filter::Lanczos3,
        ] {
            let mut resizer =
                Resizer::new(src_w, src_h, dst_w, dst_h, filter, true).unwrap();
            let mut dst = vec![0u8; dst_w * dst_h * 4];
            resizer.resize_rgba8(&src, &mut dst).unwrap();
        }
    }

    #[test]
    fn one_pixel_source_and_destination_do_not_panic() {
        let src = [10u8, 20, 30, 255];
        let mut dst = [0u8; 4];
        let mut resizer = Resizer::new(1, 1, 1, 1, Filter::Lanczos3, false).unwrap();
        resizer.resize_rgba8(&src, &mut dst).unwrap();
        for (a, b) in src.iter().zip(dst.iter()) {
            assert!((i32::from(*a) - i32::from(*b)).abs() <= 1);
        }

        let mut resizer = Resizer::new(1, 1, 5, 5, Filter::Mitchell, false).unwrap();
        let mut dst2 = vec![0u8; 5 * 5 * 4];
        resizer.resize_rgba8(&src, &mut dst2).unwrap();
        for px in dst2.chunks_exact(4) {
            assert_eq!(px, &src);
        }
    }

    #[test]
    fn opaque_frame_premultiply_matches_plain_path_exactly() {
        // Every alpha byte is 255, so premultiply multiplies by exactly 1.0
        // and the opaque fast path must produce byte-identical output to a
        // premultiply-disabled resizer.
        let (src_w, src_h, dst_w, dst_h) = (64usize, 48usize, 31usize, 17usize);
        let src: Vec<u8> = (0..src_w * src_h * 4)
            .map(|i| if i % 4 == 3 { 255 } else { ((i * 41 + 3) % 256) as u8 })
            .collect();
        for filter in [Filter::Bilinear, Filter::Mitchell, Filter::Lanczos3] {
            let mut with_premul =
                Resizer::new(src_w, src_h, dst_w, dst_h, filter, true).unwrap();
            let mut without =
                Resizer::new(src_w, src_h, dst_w, dst_h, filter, false).unwrap();
            let mut a = vec![0u8; dst_w * dst_h * 4];
            let mut b = vec![0u8; dst_w * dst_h * 4];
            with_premul.resize_rgba8(&src, &mut a).unwrap();
            without.resize_rgba8(&src, &mut b).unwrap();
            assert_eq!(a, b, "{filter:?}: opaque premul fast path must equal plain path");
        }

        // One transparent pixel anywhere disables the fast path (the real
        // premultiplied result must differ from the naive one at the
        // red/green boundary — covered by the fringing test; here we just
        // pin that the mixed-alpha path still runs).
        let mut src_mixed = src;
        src_mixed[3] = 0;
        let mut with_premul =
            Resizer::new(src_w, src_h, dst_w, dst_h, Filter::Lanczos3, true).unwrap();
        let mut out = vec![0u8; dst_w * dst_h * 4];
        with_premul.resize_rgba8(&src_mixed, &mut out).unwrap();
    }

    #[test]
    fn u8_and_f32_paths_agree_within_one_lsb() {
        let src_w = 40;
        let src_h = 30;
        let dst_w = 17;
        let dst_h = 13;
        let src_u8: Vec<u8> = (0..(src_w * src_h * 4))
            .map(|i| ((i * 53 + 7) % 256) as u8)
            .collect();
        let mut src_f32 = vec![0.0f32; src_u8.len()];
        oximedia_web_core::u8_to_f32_into(&src_u8, &mut src_f32).unwrap();

        for filter in [Filter::Bilinear, Filter::CatmullRom, Filter::Mitchell, Filter::Lanczos3] {
            let mut r8 = Resizer::new(src_w, src_h, dst_w, dst_h, filter, true).unwrap();
            let mut dst_u8 = vec![0u8; dst_w * dst_h * 4];
            r8.resize_rgba8(&src_u8, &mut dst_u8).unwrap();

            let mut rf = Resizer::new(src_w, src_h, dst_w, dst_h, filter, true).unwrap();
            let mut dst_f32 = vec![0.0f32; dst_w * dst_h * 4];
            rf.resize_f32(&src_f32, &mut dst_f32).unwrap();
            let mut dst_f32_as_u8 = vec![0u8; dst_f32.len()];
            oximedia_web_core::f32_to_u8_into(&dst_f32, &mut dst_f32_as_u8).unwrap();

            for (a, b) in dst_u8.iter().zip(dst_f32_as_u8.iter()) {
                assert!(
                    (i32::from(*a) - i32::from(*b)).abs() <= 1,
                    "{filter:?} u8={a} f32-as-u8={b}"
                );
            }
        }
    }

    #[test]
    fn resize_f32_does_not_clamp_hdr_values_above_one() {
        let src = [2.0f32, 3.0, 0.5, 1.0, 2.0, 3.0, 0.5, 1.0];
        let mut resizer = Resizer::new(2, 1, 4, 1, Filter::Bilinear, false).unwrap();
        let mut dst = vec![0.0f32; 4 * 4];
        resizer.resize_f32(&src, &mut dst).unwrap();
        for px in dst.chunks_exact(4) {
            assert!((px[0] - 2.0).abs() < 1e-4);
            assert!((px[1] - 3.0).abs() < 1e-4);
        }
    }

    /// Perf smoke test / regression canary — deliberately **not** part of
    /// the mandatory verify gate (`cargo test` without `--ignored`, and
    /// `web/scripts/*.sh`, never run it). Wall-clock assertions are
    /// inherently sensitive to two things outside this crate's control:
    ///
    /// - **Build profile.** The shared `web/Cargo.toml` workspace
    ///   `[profile.release]` uses `opt-level = "z"` (correct for the actual
    ///   wasm gzip-size budget this crate ships under — see
    ///   `web/scripts/size-gate.sh`), which trades a meaningful amount of
    ///   native throughput for code size versus `opt-level = 3`. Run this
    ///   test with `CARGO_PROFILE_RELEASE_OPT_LEVEL=3 cargo test -p
    ///   oximedia-web-scale --manifest-path web/Cargo.toml --release --
    ///   --ignored --nocapture` for a throughput-representative number.
    /// - **System load.** On a busy dev machine (other builds/tests
    ///   competing for cores) this number can be 2-3x its quiet-machine
    ///   value; treat a single over-budget run as inconclusive, not a
    ///   regression, unless it reproduces on an otherwise-idle machine.
    #[test]
    #[ignore = "perf smoke test: run explicitly with `cargo test -- --ignored`"]
    fn perf_smoke_4k_to_1080p_lanczos3() {
        let src_w = 3840;
        let src_h = 2160;
        let dst_w = 1920;
        let dst_h = 1080;
        let src = checkerboard_rgba8(src_w, src_h);
        let mut resizer =
            Resizer::new(src_w, src_h, dst_w, dst_h, Filter::Lanczos3, true).unwrap();
        let mut dst = vec![0u8; dst_w * dst_h * 4];

        let start = std::time::Instant::now();
        resizer.resize_rgba8(&src, &mut dst).unwrap();
        let elapsed = start.elapsed();
        let ms = elapsed.as_secs_f64() * 1000.0;
        println!("4K->1080p lanczos3 native resize: {ms:.2} ms");
        assert!(ms < 80.0, "native perf canary exceeded: {ms:.2} ms");
    }
}
