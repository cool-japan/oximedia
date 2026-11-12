// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Histogram kernels: luma (single channel) and per-channel RGB overlay.
//!
//! Ported from the native `oximedia-scopes` `histogram.rs`, de-rayon'd (single
//! thread, 256-bin `u32` accumulators owned by the renderer) and adapted to
//! RGBA8. Bars are normalised to the tallest bin and drawn onto an
//! opaque-black canvas.
//!
//! Both kernels bin into **four interleaved bank sets** and merge at the end:
//! neighbouring pixels in real frames (gradients, flat regions) hit the same
//! bin over and over, and a single bank turns that into one serial
//! load-add-store dependency chain per run of equal pixels. Four banks let
//! four consecutive pixels' increments proceed independently (~1.7x on the
//! 1080p RGB histogram natively) and the merged counts are exactly equal.

use crate::canvas::{CanvasMut, Color};
use crate::waveform::luma_row;

const WHITE: Color = [255, 255, 255, 255];
const RED: Color = [255, 0, 0, 192];
const GREEN: Color = [0, 255, 0, 192];
const BLUE: Color = [0, 0, 255, 192];

/// Bank count for the interleaved binning (see the module docs).
const BANKS: usize = 4;

#[inline]
fn max_bin(bins: &[u32; 256]) -> u32 {
    bins.iter().copied().max().unwrap_or(1).max(1)
}

/// Luma histogram: 256 bins of BT.709 (or configured) luma, drawn as white
/// bars. `bins` is cleared and left populated for inspection. `luma_buf` is
/// the reused conversion row buffer (at least `fw` bytes).
pub fn luma<F>(
    canvas: &mut CanvasMut<'_>,
    bins: &mut [u32; 256],
    frame: &[u8],
    fw: u32,
    fh: u32,
    ycbcr: F,
    luma_buf: &mut [u8],
) where
    F: Fn(u8, u8, u8) -> [u8; 3] + Copy,
{
    let mut banks = [[0u32; 256]; BANKS];
    let stride = fw as usize * 4;
    let lbuf = &mut luma_buf[..fw as usize];
    for row in frame.chunks_exact(stride).take(fh as usize) {
        // Vectorisable conversion sweep, then the banked scalar binning.
        luma_row(row, lbuf, ycbcr);
        let mut chunks = lbuf.chunks_exact(BANKS);
        for quad in &mut chunks {
            for (k, bank) in banks.iter_mut().enumerate() {
                let v = quad[k] as usize;
                bank[v] = bank[v].saturating_add(1);
            }
        }
        for &l in chunks.remainder() {
            let v = l as usize;
            banks[0][v] = banks[0][v].saturating_add(1);
        }
    }
    merge_banks(&banks, bins);
    canvas.clear_black();
    draw_bars(canvas, bins, WHITE, max_bin(bins), false);
}

/// Per-channel RGB histogram, drawn overlaid with semi-transparent R/G/B bars.
pub fn rgb(canvas: &mut CanvasMut<'_>, bins: &mut [[u32; 256]; 3], frame: &[u8], fw: u32, fh: u32) {
    let mut banks = [[[0u32; 256]; 3]; BANKS];
    let px_count = fw as usize * fh as usize;
    let mut quads = frame[..px_count * 4].chunks_exact(4 * BANKS);
    for quad in &mut quads {
        for (k, bank) in banks.iter_mut().enumerate() {
            let px = &quad[k * 4..k * 4 + 4];
            for ch in 0..3 {
                let v = px[ch] as usize;
                bank[ch][v] = bank[ch][v].saturating_add(1);
            }
        }
    }
    for px in quads.remainder().chunks_exact(4) {
        for ch in 0..3 {
            let v = px[ch] as usize;
            banks[0][ch][v] = banks[0][ch][v].saturating_add(1);
        }
    }
    for (ch, out) in bins.iter_mut().enumerate() {
        for (v, slot) in out.iter_mut().enumerate() {
            let mut sum = 0u32;
            for bank in &banks {
                sum = sum.saturating_add(bank[ch][v]);
            }
            *slot = sum;
        }
    }
    let max = max_bin(&bins[0]).max(max_bin(&bins[1])).max(max_bin(&bins[2]));
    canvas.clear_black();
    for (ch, color) in [RED, GREEN, BLUE].into_iter().enumerate() {
        draw_bars(canvas, &bins[ch], color, max, true);
    }
}

/// Sums four bank sets into the caller's bins (saturating, order-free).
fn merge_banks(banks: &[[u32; 256]; BANKS], bins: &mut [u32; 256]) {
    for (v, slot) in bins.iter_mut().enumerate() {
        let mut sum = 0u32;
        for bank in banks {
            sum = sum.saturating_add(bank[v]);
        }
        *slot = sum;
    }
}

/// Draws one histogram over the canvas width. When `blend`, the bar colour is
/// alpha-composited (used for the overlapping RGB channels).
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
fn draw_bars(canvas: &mut CanvasMut<'_>, bins: &[u32; 256], color: Color, max: u32, blend: bool) {
    let w = canvas.width();
    let h = canvas.height();
    for (bin, &count) in bins.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let x = (bin as u32 * w / 256).min(w - 1);
        let bar = ((count as f32 / max as f32) * h as f32) as u32;
        let y_start = h.saturating_sub(bar);
        for y in y_start..h {
            if blend {
                canvas.blend_pixel(x, y, color);
            } else {
                canvas.set_pixel(x, y, color);
            }
        }
    }
}
