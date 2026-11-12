// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! False-colour exposure overlays.
//!
//! Two presets are provided:
//!
//! - [`Preset::Spectrum`] — the classic IRE spectrum (dark-blue crushed blacks
//!   through cyan/green/yellow to red/magenta clipping), ported from the native
//!   `false_color.rs` `default_ire_zones`.
//! - [`Preset::Arri`] — an ARRI-style exposure-band map using the native
//!   cinema-standard thresholds (blue shadows, pink low-mids, grey mids, green
//!   high-mids, yellow highlights, red clipping, purple crush).
//!
//! **Upstream bug fixed:** the native `generate_false_color` renders at the
//! *input* frame size, ignoring the configured scope dimensions. This port
//! nearest-samples the input into the caller-sized scope canvas, so the output
//! is always exactly `scope_w x scope_h`.

use crate::canvas::CanvasMut;

/// A false-colour palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// IRE spectrum (blue → red → magenta).
    Spectrum,
    /// ARRI-style exposure bands.
    Arri,
}

/// IRE spectrum zones: `(min_ire, max_ire, rgb)`, scanned in order.
const SPECTRUM: [(f32, f32, [u8; 3]); 12] = [
    (0.0, 5.0, [0, 0, 128]),
    (5.0, 10.0, [0, 0, 255]),
    (10.0, 20.0, [0, 128, 255]),
    (20.0, 35.0, [0, 255, 255]),
    (35.0, 45.0, [0, 255, 0]),
    (45.0, 55.0, [128, 255, 0]),
    (55.0, 65.0, [255, 255, 0]),
    (65.0, 75.0, [255, 200, 0]),
    (75.0, 85.0, [255, 128, 0]),
    (85.0, 95.0, [255, 0, 0]),
    (95.0, 100.0, [255, 0, 128]),
    (100.0, 110.0, [255, 0, 255]),
];

#[inline]
fn spectrum_color(luma: u8) -> [u8; 4] {
    let ire = (f32::from(luma) / 255.0) * 100.0;
    for &(min, max, rgb) in &SPECTRUM {
        if ire >= min && ire < max {
            return [rgb[0], rgb[1], rgb[2], 255];
        }
    }
    [0, 0, 0, 255]
}

// ARRI cinema-standard thresholds (from the native `FalseColorConfig`).
const ARRI_OVEREXPOSED: f32 = 0.97;
const ARRI_HIGHLIGHT: f32 = 0.88;
const ARRI_MIDTONE_HIGH: f32 = 0.60;
const ARRI_MIDTONE_LOW: f32 = 0.38;
const ARRI_SHADOW: f32 = 0.12;
const ARRI_UNDEREXPOSED: f32 = 0.01;

#[inline]
fn arri_color(luma: u8) -> [u8; 4] {
    let l = f32::from(luma) / 255.0;
    let rgb = if l >= ARRI_OVEREXPOSED {
        [255, 0, 0] // clipping
    } else if l >= ARRI_HIGHLIGHT {
        [255, 255, 0] // highlights
    } else if l >= ARRI_MIDTONE_HIGH {
        [0, 200, 0] // upper mids
    } else if l >= ARRI_MIDTONE_LOW {
        [128, 128, 128] // mids
    } else if l >= ARRI_SHADOW {
        [255, 105, 180] // lower mids
    } else if l >= ARRI_UNDEREXPOSED {
        [0, 0, 255] // shadows
    } else {
        [128, 0, 128] // crushed
    };
    [rgb[0], rgb[1], rgb[2], 255]
}

/// Returns the false-colour RGBA for a luma value under `preset` (test hook).
#[must_use]
pub fn color_for(preset: Preset, luma: u8) -> [u8; 4] {
    match preset {
        Preset::Spectrum => spectrum_color(luma),
        Preset::Arri => arri_color(luma),
    }
}

/// Nearest-samples the input frame into the (caller-sized) scope canvas and
/// maps every pixel's luma through `preset`.
///
/// Per call this precomputes the 256-entry luma→colour palette (replacing
/// the per-pixel IRE zone scan) and takes the caller's reused `fx_map`
/// scope-column → source-column table (values recomputed here, storage
/// owned by the renderer); the per-pixel work is then one gather, one
/// inlined luma dot product and one palette copy, written through whole
/// canvas-row slices instead of bounds-checked `set_pixel` calls.
pub fn render<F>(
    canvas: &mut CanvasMut<'_>,
    frame: &[u8],
    fw: u32,
    fh: u32,
    preset: Preset,
    ycbcr: F,
    fx_map: &mut [u32],
) where
    F: Fn(u8, u8, u8) -> [u8; 3] + Copy,
{
    let w = canvas.width();
    let h = canvas.height();
    let fstride = fw as usize * 4;

    let mut palette = [[0u8; 4]; 256];
    for (luma, color) in palette.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        {
            *color = color_for(preset, luma as u8);
        }
    }

    let cols = &mut fx_map[..w as usize];
    for (sx, slot) in cols.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        {
            *slot = (sx as u32 * fw / w).min(fw - 1);
        }
    }

    for sy in 0..h {
        // Nearest-neighbour source row.
        let fy = (sy * fh / h).min(fh - 1) as usize;
        let src_row = &frame[fy * fstride..(fy + 1) * fstride];
        let Some(out_row) = canvas.row_mut(sy) else { break };
        for (px, &fx) in out_row.chunks_exact_mut(4).zip(cols.iter()) {
            let p = fx as usize * 4;
            let luma = ycbcr(src_row[p], src_row[p + 1], src_row[p + 2])[0];
            px.copy_from_slice(&palette[luma as usize]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_black_is_dark_blue() {
        assert_eq!(color_for(Preset::Spectrum, 0), [0, 0, 128, 255]);
    }

    #[test]
    fn spectrum_white_is_magenta() {
        // 255 → 100 IRE → the (100,110) bright-magenta band.
        assert_eq!(color_for(Preset::Spectrum, 255), [255, 0, 255, 255]);
    }

    #[test]
    fn arri_clipping_is_red() {
        assert_eq!(color_for(Preset::Arri, 255), [255, 0, 0, 255]);
    }

    #[test]
    fn arri_crushed_is_purple() {
        assert_eq!(color_for(Preset::Arri, 0), [128, 0, 128, 255]);
    }

    #[test]
    fn arri_midtone_is_grey() {
        // 128/255 ≈ 0.50 → mids band.
        assert_eq!(color_for(Preset::Arri, 128), [128, 128, 128, 255]);
    }
}
