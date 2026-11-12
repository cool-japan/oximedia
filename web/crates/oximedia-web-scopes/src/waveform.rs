// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Waveform monitor kernels: luma, RGB parade, RGB overlay and YCbCr parade.
//!
//! Ported from the native `oximedia-scopes` `waveform.rs`, adapted from RGB24
//! to tightly packed **RGBA8** input and reworked so nothing allocates per
//! frame: the caller supplies pre-sized `u32` accumulators, a per-frame column
//! map (`x -> scope column`), a 256-entry value→row LUT and a row-sized
//! conversion scratch buffer, all owned and reused by
//! [`crate::renderer::ScopeRenderer`].
//!
//! # Hot-loop shape
//!
//! The RGB→YCbCr conversion is taken as a **generic** `Fn` parameter (a
//! zero-sized function item at every call site), not a `fn` pointer: an
//! indirect call per pixel cannot be inlined and costs more than the whole
//! fixed-point dot product it wraps. Luma-driven kernels additionally split
//! each row into two passes — a straight-line RGBA→luma sweep into a reused
//! row buffer (auto-vectorises under `+simd128`) and the inherently scalar
//! scatter — instead of interleaving conversion and scatter per pixel.
//!
//! The trace is normalised with the same `sqrt(count / max)` perceptual scaling
//! as upstream, and accumulated onto an opaque-black canvas. The normalise
//! passes write whole canvas rows through [`CanvasMut::row_mut`] with no
//! per-pixel branch: the canvas is already cleared to opaque black, so
//! unconditionally writing a zero-intensity pixel is identical to skipping it,
//! and the branch-free loop (one `sqrt` per cell) auto-vectorises.

use crate::canvas::CanvasMut;

/// Converts one RGBA row (`fw * 4` bytes) into luma bytes (`fw` entries).
///
/// `ycbcr` is generic so it inlines; the loop then auto-vectorises.
#[inline]
pub fn luma_row<F>(row: &[u8], out: &mut [u8], ycbcr: F)
where
    F: Fn(u8, u8, u8) -> [u8; 3],
{
    for (d, px) in out.iter_mut().zip(row.chunks_exact(4)) {
        *d = ycbcr(px[0], px[1], px[2])[0];
    }
}

#[inline]
fn max_u32(acc: &[u32]) -> u32 {
    acc.iter().copied().max().unwrap_or(1).max(1)
}

/// `sqrt(count / max)` perceptual intensity, branch-free (`count == 0`
/// naturally yields 0 through the same expression).
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
#[inline]
fn normalized(count: u32, max: u32) -> u8 {
    ((count as f32 / max as f32).sqrt() * 255.0) as u8
}

/// Iterates the RGBA8 frame, calling `f(x, r, g, b)` for every pixel.
#[inline]
fn for_each_pixel(frame: &[u8], fw: u32, fh: u32, mut f: impl FnMut(usize, u8, u8, u8)) {
    let stride = fw as usize * 4;
    for y in 0..fh as usize {
        let base = y * stride;
        let row = &frame[base..base + stride];
        for (x, px) in row.chunks_exact(4).enumerate() {
            f(x, px[0], px[1], px[2]);
        }
    }
}

/// One channel's scatter state for run-collapsed accumulation.
///
/// Flat regions and smooth gradients send consecutive pixels to the *same*
/// accumulator cell (same value row, same mapped column), and incrementing
/// that cell per pixel forms one serial load-add-store dependency chain.
/// Counting the run locally and flushing once breaks the chain. Bit-exact
/// versus per-pixel increments: per-frame counts cannot approach
/// `u32::MAX`, so `n` saturating adds of 1 equal one saturating add of `n`.
struct RunAcc {
    idx: usize,
    len: u32,
}

impl RunAcc {
    #[inline]
    fn new() -> Self {
        Self {
            idx: usize::MAX,
            len: 0,
        }
    }

    /// Records one hit on `idx`, flushing the previous run if it ended.
    #[inline]
    fn push(&mut self, acc: &mut [u32], idx: usize) {
        if idx == self.idx {
            self.len += 1;
        } else {
            if self.len > 0 {
                acc[self.idx] = acc[self.idx].saturating_add(self.len);
            }
            self.idx = idx;
            self.len = 1;
        }
    }

    /// Flushes the final run.
    #[inline]
    fn finish(self, acc: &mut [u32]) {
        if self.len > 0 {
            acc[self.idx] = acc[self.idx].saturating_add(self.len);
        }
    }
}

/// Luma (Y) waveform. `col_map` has length `fw`, mapping input columns to the
/// full-width scope; `row_lut` maps a 0..=255 value to a scope row.
/// `luma_buf` is the reused conversion row buffer (at least `fw` bytes).
#[allow(clippy::too_many_arguments)]
pub fn luma<F>(
    canvas: &mut CanvasMut<'_>,
    accum: &mut [u32],
    col_map: &[u32],
    row_lut: &[u32; 256],
    frame: &[u8],
    fw: u32,
    fh: u32,
    ycbcr: F,
    luma_buf: &mut [u8],
) where
    F: Fn(u8, u8, u8) -> [u8; 3] + Copy,
{
    let w = canvas.width();
    let n = (w * canvas.height()) as usize;
    accum[..n].fill(0);
    let stride = fw as usize * 4;
    let lbuf = &mut luma_buf[..fw as usize];
    let mut run = RunAcc::new();
    for row in frame.chunks_exact(stride).take(fh as usize) {
        luma_row(row, lbuf, ycbcr);
        for (&l, &col) in lbuf.iter().zip(col_map.iter()) {
            run.push(accum, (row_lut[l as usize] * w + col) as usize);
        }
    }
    run.finish(accum);
    canvas.clear_black();
    draw_grayscale(canvas, &accum[..n], w, 0);
}

/// RGB parade (R|G|B). `col_map` maps columns into a *section* of width
/// `section_w = scope_w / 3`; the three accumulators are section-sized.
#[allow(clippy::too_many_arguments)]
pub fn rgb_parade(
    canvas: &mut CanvasMut<'_>,
    acc_r: &mut [u32],
    acc_g: &mut [u32],
    acc_b: &mut [u32],
    col_map: &[u32],
    row_lut: &[u32; 256],
    frame: &[u8],
    fw: u32,
    fh: u32,
    section_w: u32,
) {
    let h = canvas.height();
    let sn = (section_w * h) as usize;
    acc_r[..sn].fill(0);
    acc_g[..sn].fill(0);
    acc_b[..sn].fill(0);
    let mut run_r = RunAcc::new();
    let mut run_g = RunAcc::new();
    let mut run_b = RunAcc::new();
    for_each_pixel(frame, fw, fh, |x, r, g, b| {
        let sx = col_map[x];
        run_r.push(acc_r, (row_lut[r as usize] * section_w + sx) as usize);
        run_g.push(acc_g, (row_lut[g as usize] * section_w + sx) as usize);
        run_b.push(acc_b, (row_lut[b as usize] * section_w + sx) as usize);
    });
    run_r.finish(acc_r);
    run_g.finish(acc_g);
    run_b.finish(acc_b);
    let max = max_u32(&acc_r[..sn])
        .max(max_u32(&acc_g[..sn]))
        .max(max_u32(&acc_b[..sn]));
    canvas.clear_black();
    // Each channel writes `[v, 0, 0]`-style tinted pixels into its own pane;
    // panes don't overlap, so whole-row slices per pane keep this branch-free.
    for (ch, acc) in [&acc_r[..sn], &acc_g[..sn], &acc_b[..sn]].iter().enumerate() {
        let offset = (section_w * ch as u32) as usize * 4;
        for y in 0..h {
            let Some(row) = canvas.row_mut(y) else { break };
            let pane = &mut row[offset..offset + section_w as usize * 4];
            let acc_row = &acc[(y * section_w) as usize..((y + 1) * section_w) as usize];
            for (px, &count) in pane.chunks_exact_mut(4).zip(acc_row.iter()) {
                let v = normalized(count, max);
                px[ch] = v;
                px[3] = 255;
            }
        }
    }
}

/// RGB overlay: all three channels composited additively on one full-width
/// scope. Each accumulator is full-canvas sized.
#[allow(clippy::too_many_arguments)]
pub fn rgb_overlay(
    canvas: &mut CanvasMut<'_>,
    acc_r: &mut [u32],
    acc_g: &mut [u32],
    acc_b: &mut [u32],
    col_map: &[u32],
    row_lut: &[u32; 256],
    frame: &[u8],
    fw: u32,
    fh: u32,
) {
    let w = canvas.width();
    let h = canvas.height();
    let n = (w * h) as usize;
    acc_r[..n].fill(0);
    acc_g[..n].fill(0);
    acc_b[..n].fill(0);
    let mut run_r = RunAcc::new();
    let mut run_g = RunAcc::new();
    let mut run_b = RunAcc::new();
    for_each_pixel(frame, fw, fh, |x, r, g, b| {
        let sx = col_map[x];
        run_r.push(acc_r, (row_lut[r as usize] * w + sx) as usize);
        run_g.push(acc_g, (row_lut[g as usize] * w + sx) as usize);
        run_b.push(acc_b, (row_lut[b as usize] * w + sx) as usize);
    });
    run_r.finish(acc_r);
    run_g.finish(acc_g);
    run_b.finish(acc_b);
    let max = max_u32(&acc_r[..n])
        .max(max_u32(&acc_g[..n]))
        .max(max_u32(&acc_b[..n]));
    canvas.clear_black();
    for y in 0..h {
        let Some(row) = canvas.row_mut(y) else { break };
        let base = (y * w) as usize;
        for (x, px) in row.chunks_exact_mut(4).enumerate() {
            let idx = base + x;
            px[0] = normalized(acc_r[idx], max);
            px[1] = normalized(acc_g[idx], max);
            px[2] = normalized(acc_b[idx], max);
            px[3] = 255;
        }
    }
}

/// YCbCr parade (Y|Cb|Cr), each pane grayscale. Section-sized accumulators.
/// `ycbcr_buf` is the reused conversion row buffer (at least `3 * fw` bytes,
/// laid out as three consecutive `fw`-byte planes).
#[allow(clippy::too_many_arguments)]
pub fn ycbcr_parade<F>(
    canvas: &mut CanvasMut<'_>,
    acc_y: &mut [u32],
    acc_cb: &mut [u32],
    acc_cr: &mut [u32],
    col_map: &[u32],
    row_lut: &[u32; 256],
    frame: &[u8],
    fw: u32,
    fh: u32,
    section_w: u32,
    ycbcr: F,
    ycbcr_buf: &mut [u8],
) where
    F: Fn(u8, u8, u8) -> [u8; 3] + Copy,
{
    let h = canvas.height();
    let sn = (section_w * h) as usize;
    acc_y[..sn].fill(0);
    acc_cb[..sn].fill(0);
    acc_cr[..sn].fill(0);
    let stride = fw as usize * 4;
    let (ybuf, rest) = ycbcr_buf.split_at_mut(fw as usize);
    let (cbbuf, rest) = rest.split_at_mut(fw as usize);
    let crbuf = &mut rest[..fw as usize];
    let mut run_y = RunAcc::new();
    let mut run_cb = RunAcc::new();
    let mut run_cr = RunAcc::new();
    for row in frame.chunks_exact(stride).take(fh as usize) {
        // Vectorisable conversion sweep...
        for (((y, cb), cr), px) in ybuf
            .iter_mut()
            .zip(cbbuf.iter_mut())
            .zip(crbuf.iter_mut())
            .zip(row.chunks_exact(4))
        {
            let [yv, cbv, crv] = ycbcr(px[0], px[1], px[2]);
            *y = yv;
            *cb = cbv;
            *cr = crv;
        }
        // ...then the scalar scatter.
        for (((&yv, &cbv), &crv), &sx) in ybuf
            .iter()
            .zip(cbbuf.iter())
            .zip(crbuf.iter())
            .zip(col_map.iter())
        {
            run_y.push(acc_y, (row_lut[yv as usize] * section_w + sx) as usize);
            run_cb.push(acc_cb, (row_lut[cbv as usize] * section_w + sx) as usize);
            run_cr.push(acc_cr, (row_lut[crv as usize] * section_w + sx) as usize);
        }
    }
    run_y.finish(acc_y);
    run_cb.finish(acc_cb);
    run_cr.finish(acc_cr);
    let max = max_u32(&acc_y[..sn])
        .max(max_u32(&acc_cb[..sn]))
        .max(max_u32(&acc_cr[..sn]));
    canvas.clear_black();
    for (ch, acc) in [&acc_y[..sn], &acc_cb[..sn], &acc_cr[..sn]].iter().enumerate() {
        let offset = (section_w * ch as u32) as usize * 4;
        for y in 0..h {
            let Some(row) = canvas.row_mut(y) else { break };
            let pane = &mut row[offset..offset + section_w as usize * 4];
            let acc_row = &acc[(y * section_w) as usize..((y + 1) * section_w) as usize];
            for (px, &count) in pane.chunks_exact_mut(4).zip(acc_row.iter()) {
                let v = normalized(count, max);
                px[0] = v;
                px[1] = v;
                px[2] = v;
                px[3] = 255;
            }
        }
    }
}

/// Draws a full-width grayscale trace from a normalised accumulator.
pub(crate) fn draw_grayscale(canvas: &mut CanvasMut<'_>, accum: &[u32], w: u32, x_offset: u32) {
    let max = max_u32(accum);
    let h = canvas.height();
    let offset = x_offset as usize * 4;
    for y in 0..h {
        let Some(row) = canvas.row_mut(y) else { break };
        let span = &mut row[offset..offset + w as usize * 4];
        let acc_row = &accum[(y * w) as usize..((y + 1) * w) as usize];
        for (px, &count) in span.chunks_exact_mut(4).zip(acc_row.iter()) {
            let v = normalized(count, max);
            px[0] = v;
            px[1] = v;
            px[2] = v;
            px[3] = 255;
        }
    }
}
