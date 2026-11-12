// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Resize algorithms: bilinear, Lanczos3, fit-mode helpers.

use crate::transform::{FitMode, Gravity};

use super::{
    geometry::{calculate_crop_rect, crop_region, pad_to_exact_size},
    PixelBuffer, ProcessingError,
};
use crate::transform::Color;

// ============================================================================
// Public resize entry point
// ============================================================================

/// Resize the image, respecting the fit mode and gravity anchor.
pub(super) fn apply_resize(
    buffer: PixelBuffer,
    target_width: u32,
    target_height: u32,
    fit: FitMode,
    gravity: &Gravity,
) -> Result<PixelBuffer, ProcessingError> {
    if buffer.width == 0 || buffer.height == 0 {
        return Ok(buffer);
    }

    // Resolve zero-sentinel dimensions from aspect ratio
    let src_aspect = buffer.width as f64 / buffer.height as f64;
    let (tw, th) = match (target_width, target_height) {
        (0, 0) => return Ok(buffer),
        (0, h) => ((h as f64 * src_aspect).round().max(1.0) as u32, h),
        (w, 0) => (w, (w as f64 / src_aspect).round().max(1.0) as u32),
        (w, h) => (w, h),
    };

    // Early-termination fast-path: when the *target* area is below 4096 pixels
    // (64×64), bilinear is perceptually indistinguishable from Lanczos and
    // dramatically cheaper.  This short-circuit applies for all fit modes that
    // eventually call bilinear_resize for at least one of their resize passes.
    const CHEAP_THRESHOLD_PX: u32 = 64 * 64; // 4096 pixels
    if tw.saturating_mul(th) < CHEAP_THRESHOLD_PX {
        // Dispatch the fit-mode logic but force bilinear for all scale steps.
        return match fit {
            FitMode::ScaleDown => {
                if buffer.width <= tw && buffer.height <= th {
                    return Ok(buffer);
                }
                let (fw, fh) = fit_contain_dims(buffer.width, buffer.height, tw, th);
                Ok(bilinear_resize(&buffer, fw, fh))
            }
            FitMode::Contain => {
                let (fw, fh) = fit_contain_dims(buffer.width, buffer.height, tw, th);
                Ok(bilinear_resize(&buffer, fw, fh))
            }
            FitMode::Cover => {
                let (fw, fh) = fit_cover_dims(buffer.width, buffer.height, tw, th);
                let resized = bilinear_resize(&buffer, fw, fh);
                let (cx, cy, cw, ch) = calculate_crop_rect(fw, fh, tw, th, gravity);
                crop_region(&resized, cx, cy, cw, ch)
            }
            FitMode::Crop => {
                let (cx, cy, cw, ch) = calculate_crop_rect(
                    buffer.width,
                    buffer.height,
                    tw.min(buffer.width),
                    th.min(buffer.height),
                    gravity,
                );
                crop_region(&buffer, cx, cy, cw, ch)
            }
            FitMode::Pad => {
                let (fw, fh) = fit_contain_dims(buffer.width, buffer.height, tw, th);
                let resized = bilinear_resize(&buffer, fw, fh);
                pad_to_exact_size(resized, tw, th, Color::black())
            }
            FitMode::Fill => Ok(bilinear_resize(&buffer, tw, th)),
        };
    }

    match fit {
        FitMode::ScaleDown => {
            if buffer.width <= tw && buffer.height <= th {
                return Ok(buffer);
            }
            let (fw, fh) = fit_contain_dims(buffer.width, buffer.height, tw, th);
            Ok(bilinear_resize(&buffer, fw, fh))
        }
        FitMode::Contain => {
            let (fw, fh) = fit_contain_dims(buffer.width, buffer.height, tw, th);
            Ok(bilinear_resize(&buffer, fw, fh))
        }
        FitMode::Cover => {
            let (fw, fh) = fit_cover_dims(buffer.width, buffer.height, tw, th);
            let resized = bilinear_resize(&buffer, fw, fh);
            let (cx, cy, cw, ch) = calculate_crop_rect(fw, fh, tw, th, gravity);
            crop_region(&resized, cx, cy, cw, ch)
        }
        FitMode::Crop => {
            let (cx, cy, cw, ch) = calculate_crop_rect(
                buffer.width,
                buffer.height,
                tw.min(buffer.width),
                th.min(buffer.height),
                gravity,
            );
            crop_region(&buffer, cx, cy, cw, ch)
        }
        FitMode::Pad => {
            let (fw, fh) = fit_contain_dims(buffer.width, buffer.height, tw, th);
            let resized = bilinear_resize(&buffer, fw, fh);
            pad_to_exact_size(resized, tw, th, Color::black())
        }
        FitMode::Fill => Ok(bilinear_resize(&buffer, tw, th)),
    }
}

// ============================================================================
// Bilinear resize
// ============================================================================

/// Bilinear interpolation resize.
///
/// Maps each destination pixel back to a fractional source coordinate and uses
/// bilinear interpolation to produce a smooth result.
pub fn bilinear_resize(buffer: &PixelBuffer, new_width: u32, new_height: u32) -> PixelBuffer {
    if new_width == 0 || new_height == 0 {
        return PixelBuffer::new(new_width, new_height, buffer.channels);
    }
    if new_width == buffer.width && new_height == buffer.height {
        return buffer.clone();
    }

    let ch = buffer.channels as usize;
    let mut output = PixelBuffer::new(new_width, new_height, buffer.channels);

    let x_ratio = if new_width > 1 {
        (buffer.width as f64 - 1.0) / (new_width as f64 - 1.0)
    } else {
        0.0
    };
    let y_ratio = if new_height > 1 {
        (buffer.height as f64 - 1.0) / (new_height as f64 - 1.0)
    } else {
        0.0
    };

    for dy in 0..new_height {
        for dx in 0..new_width {
            let sx = dx as f64 * x_ratio;
            let sy = dy as f64 * y_ratio;
            let rgba = buffer.sample_bilinear(sx, sy);
            output.set_pixel(dx, dy, &rgba[..ch]);
        }
    }
    output
}

// ============================================================================
// Lanczos3 resize
// ============================================================================

/// Lanczos3 resize for higher quality output.
///
/// Uses a separable two-pass (horizontal then vertical) approach with a
/// Lanczos3 kernel: `sinc(x) * sinc(x/3)` for `|x| < 3`.
pub fn lanczos_resize(buffer: &PixelBuffer, new_width: u32, new_height: u32) -> PixelBuffer {
    if new_width == 0 || new_height == 0 {
        return PixelBuffer::new(new_width, new_height, buffer.channels);
    }
    if new_width == buffer.width && new_height == buffer.height {
        return buffer.clone();
    }

    let ch = buffer.channels as usize;

    // Pass 1: horizontal resize
    let mut h_resized = PixelBuffer::new(new_width, buffer.height, buffer.channels);
    let x_scale = buffer.width as f64 / new_width as f64;
    let x_support = if x_scale > 1.0 { 3.0 * x_scale } else { 3.0 };

    for y in 0..buffer.height {
        for dx in 0..new_width {
            let center = (dx as f64 + 0.5) * x_scale - 0.5;
            let left = (center - x_support).ceil().max(0.0) as u32;
            let right = (center + x_support).floor().min(buffer.width as f64 - 1.0) as u32;

            let mut accum = [0.0f64; 4];
            let mut weight_sum = 0.0f64;

            for sx in left..=right {
                let dist = (sx as f64 - center) / if x_scale > 1.0 { x_scale } else { 1.0 };
                let w = lanczos3_kernel(dist);
                weight_sum += w;
                let pixel = buffer.get_pixel_rgba(sx, y);
                for c in 0..4 {
                    accum[c] += pixel[c] as f64 * w;
                }
            }

            if weight_sum.abs() > f64::EPSILON {
                let inv = 1.0 / weight_sum;
                let mut pixel = [0u8; 4];
                for c in 0..4 {
                    pixel[c] = (accum[c] * inv).round().clamp(0.0, 255.0) as u8;
                }
                h_resized.set_pixel(dx, y, &pixel[..ch]);
            }
        }
    }

    // Pass 2: vertical resize
    let mut output = PixelBuffer::new(new_width, new_height, buffer.channels);
    let y_scale = buffer.height as f64 / new_height as f64;
    let y_support = if y_scale > 1.0 { 3.0 * y_scale } else { 3.0 };

    for x in 0..new_width {
        for dy in 0..new_height {
            let center = (dy as f64 + 0.5) * y_scale - 0.5;
            let top = (center - y_support).ceil().max(0.0) as u32;
            let bottom = (center + y_support)
                .floor()
                .min(h_resized.height as f64 - 1.0) as u32;

            let mut accum = [0.0f64; 4];
            let mut weight_sum = 0.0f64;

            for sy in top..=bottom {
                let dist = (sy as f64 - center) / if y_scale > 1.0 { y_scale } else { 1.0 };
                let w = lanczos3_kernel(dist);
                weight_sum += w;
                let pixel = h_resized.get_pixel_rgba(x, sy);
                for c in 0..4 {
                    accum[c] += pixel[c] as f64 * w;
                }
            }

            if weight_sum.abs() > f64::EPSILON {
                let inv = 1.0 / weight_sum;
                let mut pixel = [0u8; 4];
                for c in 0..4 {
                    pixel[c] = (accum[c] * inv).round().clamp(0.0, 255.0) as u8;
                }
                output.set_pixel(x, dy, &pixel[..ch]);
            }
        }
    }

    output
}

/// Lanczos3 kernel function: `sinc(x) * sinc(x/3)` for `|x| < 3`, else 0.
pub(super) fn lanczos3_kernel(x: f64) -> f64 {
    let ax = x.abs();
    if ax < f64::EPSILON {
        return 1.0;
    }
    if ax >= 3.0 {
        return 0.0;
    }
    let pi_x = std::f64::consts::PI * x;
    let sinc_x = pi_x.sin() / pi_x;
    let sinc_x3 = (pi_x / 3.0).sin() / (pi_x / 3.0);
    sinc_x * sinc_x3
}

// ============================================================================
// Dimension helpers
// ============================================================================

/// Compute dimensions for "contain" fit: fit within bounds, preserve aspect ratio.
pub(super) fn fit_contain_dims(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (dst_w, dst_h);
    }
    let scale = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
    let w = (src_w as f64 * scale).round().max(1.0) as u32;
    let h = (src_h as f64 * scale).round().max(1.0) as u32;
    (w, h)
}

/// Compute dimensions for "cover" fit: fill bounds, crop excess.
pub(super) fn fit_cover_dims(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (dst_w, dst_h);
    }
    let scale = (dst_w as f64 / src_w as f64).max(dst_h as f64 / src_h as f64);
    let w = (src_w as f64 * scale).round().max(1.0) as u32;
    let h = (src_h as f64 * scale).round().max(1.0) as u32;
    (w, h)
}
