// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Vectorscope kernel and chroma helpers.
//!
//! Ported from the native `oximedia-scopes` `vectorscope.rs`. Chroma is derived
//! per pixel with the BT.601 full-range fixed-point kernel from
//! [`oximedia_web_core`] (bit-exact with the native SIMD path), so a browser
//! trace lines up with a natively rendered one. No allocation, no circular mask
//! clipping of the trace (the rings are drawn as a graticule overlay instead).

use crate::canvas::CanvasMut;
use crate::graticule::vectorscope_radius;
use core::f32::consts::PI;

/// Maximum theoretical chroma magnitude (`sqrt(128^2 + 128^2)`), used to
/// normalise [`saturation`] to `0..=1`.
const MAX_CHROMA: f32 = 181.019_34;

/// Renders the vectorscope trace for `frame` into `canvas` (cleared to black
/// first). `accum` must be at least `scope_w * scope_h` `u32`s; `cbcr_buf`
/// is the reused conversion row buffer (at least `2 * fw` bytes, two
/// consecutive `fw`-byte planes).
///
/// # Hot-loop shape
///
/// `Cb`/`Cr` only take 256 values each and the plot coordinate along each
/// axis depends on exactly one of them, so the whole
/// `f32 centre + scale + clip` chain is precomputed into two 256-entry
/// `i32` tables (out-of-range entries hold `-1`, bit-identical to the
/// original per-pixel float path). Each frame row is first converted by a
/// vectorisable sweep into the `Cb`/`Cr` planes (the generic `ycbcr`
/// inlines — a `fn`-pointer call per pixel would dominate the kernel), and
/// the remaining scalar scatter is two table loads, one sign test and one
/// accumulate per pixel.
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_possible_wrap)]
#[allow(clippy::cast_sign_loss)]
#[allow(clippy::too_many_arguments)]
pub fn render<F>(
    canvas: &mut CanvasMut<'_>,
    accum: &mut [u32],
    frame: &[u8],
    fw: u32,
    fh: u32,
    gain: f32,
    ycbcr: F,
    cbcr_buf: &mut [u8],
) where
    F: Fn(u8, u8, u8) -> [u8; 3] + Copy,
{
    let w = canvas.width();
    let h = canvas.height();
    let n = (w * h) as usize;
    accum[..n].fill(0);

    let cx = (w / 2) as f32;
    let cy = (h / 2) as f32;
    let scale = gain * vectorscope_radius(w, h) / 128.0;
    let (wi, hi) = (w as i32, h as i32);

    // cb -> scope x and cr -> scope y, computed with exactly the expression
    // the per-pixel path used; -1 marks off-canvas.
    let mut sx_tab = [-1i32; 256];
    let mut sy_tab = [-1i32; 256];
    for v in 0..256 {
        let sx = (cx + (v as f32 - 128.0) * scale) as i32;
        if sx >= 0 && sx < wi {
            sx_tab[v as usize] = sx;
        }
        let sy = (cy - (v as f32 - 128.0) * scale) as i32;
        if sy >= 0 && sy < hi {
            sy_tab[v as usize] = sy * wi; // pre-multiplied row offset
        }
    }

    // The scatter collapses *runs*: flat regions and smooth gradients map
    // consecutive pixels to the same scope cell, and incrementing that cell
    // per pixel is one serial load-add-store chain. Counting the run and
    // flushing once breaks the chain (bit-exact: per-frame counts cannot
    // approach `u32::MAX`, so `n` adds of 1 equal one add of `n`).
    let stride = fw as usize * 4;
    let (cbbuf, rest) = cbcr_buf.split_at_mut(fw as usize);
    let crbuf = &mut rest[..fw as usize];
    let mut run_idx = usize::MAX;
    let mut run_len = 0u32;
    for row in frame.chunks_exact(stride).take(fh as usize) {
        for ((cb, cr), px) in cbbuf
            .iter_mut()
            .zip(crbuf.iter_mut())
            .zip(row.chunks_exact(4))
        {
            let [_, cbv, crv] = ycbcr(px[0], px[1], px[2]);
            *cb = cbv;
            *cr = crv;
        }
        for (&cb, &cr) in cbbuf.iter().zip(crbuf.iter()) {
            let sx = sx_tab[cb as usize];
            let sy = sy_tab[cr as usize];
            if sx >= 0 && sy >= 0 {
                let idx = (sy + sx) as usize;
                if idx == run_idx {
                    run_len += 1;
                } else {
                    if run_len > 0 {
                        accum[run_idx] = accum[run_idx].saturating_add(run_len);
                    }
                    run_idx = idx;
                    run_len = 1;
                }
            }
        }
    }
    if run_len > 0 {
        accum[run_idx] = accum[run_idx].saturating_add(run_len);
    }

    canvas.clear_black();
    crate::waveform::draw_grayscale(canvas, &accum[..n], w, 0);
}

/// Hue angle in degrees (`0..360`) for a `Cb`/`Cr` pair.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn hue(cb: u8, cr: u8) -> f32 {
    let cb_c = f32::from(cb) - 128.0;
    let cr_c = f32::from(cr) - 128.0;
    let mut deg = cb_c.atan2(cr_c) * 180.0 / PI;
    if deg < 0.0 {
        deg += 360.0;
    }
    deg
}

/// Saturation in `0..=1` for a `Cb`/`Cr` pair.
#[must_use]
pub fn saturation(cb: u8, cr: u8) -> f32 {
    let cb_c = f32::from(cb) - 128.0;
    let cr_c = f32::from(cr) - 128.0;
    ((cb_c * cb_c + cr_c * cr_c).sqrt() / MAX_CHROMA).min(1.0)
}

/// Whether a `Cb`/`Cr` pair falls on the broadcast skin-tone line
/// (~108-138 deg hue, 0.1-0.6 saturation).
#[must_use]
pub fn is_skin_tone(cb: u8, cr: u8) -> bool {
    let hue = hue(cb, cr);
    let sat = saturation(cb, cr);
    (108.0..=138.0).contains(&hue) && (0.1..=0.6).contains(&sat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_zero_saturation() {
        assert!(saturation(128, 128) < 1e-3);
    }

    #[test]
    fn full_red_chroma_is_saturated() {
        // BT.601 pure red is Cb=85, Cr=255.
        assert!(saturation(85, 255) > 0.6);
    }

    #[test]
    fn hue_wraps_to_positive() {
        for cb in [0u8, 64, 128, 200, 255] {
            for cr in [0u8, 64, 128, 200, 255] {
                let d = hue(cb, cr);
                assert!((0.0..360.0).contains(&d), "hue {d} out of range");
            }
        }
    }
}
