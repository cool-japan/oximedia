// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Spatial filters: Gaussian blur, unsharp-mask sharpening.

use super::{PixelBuffer, ProcessingError};

// ============================================================================
// Blur
// ============================================================================

/// Gaussian blur with given sigma (radius).
///
/// Uses a separable 2-pass approach (horizontal then vertical) for O(n*k)
/// complexity. Kernel size = `ceil(sigma * 3) * 2 + 1`.
pub fn gaussian_blur(buffer: &PixelBuffer, sigma: f64) -> Result<PixelBuffer, ProcessingError> {
    apply_blur(buffer.clone(), sigma)
}

/// Internal blur implementation.
pub(super) fn apply_blur(buffer: PixelBuffer, sigma: f64) -> Result<PixelBuffer, ProcessingError> {
    if sigma <= 0.0 || buffer.width == 0 || buffer.height == 0 {
        return Ok(buffer);
    }

    let kernel = build_gaussian_kernel(sigma);
    let half = kernel.len() / 2;
    let ch = buffer.channels as usize;
    let color_ch = if ch >= 4 { 3 } else { ch };

    // Horizontal pass
    let mut h_blur = PixelBuffer::new(buffer.width, buffer.height, buffer.channels);
    for y in 0..buffer.height {
        for x in 0..buffer.width {
            let mut accum = [0.0f64; 4];
            let mut weight_sum = 0.0f64;

            for (ki, &kw) in kernel.iter().enumerate() {
                let sx = x as i64 + ki as i64 - half as i64;
                let sx = sx.clamp(0, buffer.width as i64 - 1) as u32;
                let pixel = buffer.get_pixel_rgba(sx, y);
                weight_sum += kw;
                for c in 0..color_ch {
                    accum[c] += pixel[c] as f64 * kw;
                }
                if ch >= 4 {
                    accum[3] += pixel[3] as f64 * kw;
                }
            }

            if weight_sum.abs() > f64::EPSILON {
                let inv = 1.0 / weight_sum;
                let mut out_pixel = [0u8; 4];
                for c in 0..color_ch {
                    out_pixel[c] = (accum[c] * inv).round().clamp(0.0, 255.0) as u8;
                }
                out_pixel[3] = if ch >= 4 {
                    (accum[3] * inv).round().clamp(0.0, 255.0) as u8
                } else {
                    255
                };
                h_blur.set_pixel(x, y, &out_pixel[..ch]);
            }
        }
    }

    // Vertical pass
    let mut output = PixelBuffer::new(buffer.width, buffer.height, buffer.channels);
    for y in 0..buffer.height {
        for x in 0..buffer.width {
            let mut accum = [0.0f64; 4];
            let mut weight_sum = 0.0f64;

            for (ki, &kw) in kernel.iter().enumerate() {
                let sy = y as i64 + ki as i64 - half as i64;
                let sy = sy.clamp(0, buffer.height as i64 - 1) as u32;
                let pixel = h_blur.get_pixel_rgba(x, sy);
                weight_sum += kw;
                for c in 0..color_ch {
                    accum[c] += pixel[c] as f64 * kw;
                }
                if ch >= 4 {
                    accum[3] += pixel[3] as f64 * kw;
                }
            }

            if weight_sum.abs() > f64::EPSILON {
                let inv = 1.0 / weight_sum;
                let mut out_pixel = [0u8; 4];
                for c in 0..color_ch {
                    out_pixel[c] = (accum[c] * inv).round().clamp(0.0, 255.0) as u8;
                }
                out_pixel[3] = if ch >= 4 {
                    (accum[3] * inv).round().clamp(0.0, 255.0) as u8
                } else {
                    255
                };
                output.set_pixel(x, y, &out_pixel[..ch]);
            }
        }
    }

    Ok(output)
}

// ============================================================================
// Sharpen
// ============================================================================

/// Unsharp mask sharpening.
///
/// 1. Blur a copy with a small fixed radius (sigma=1.0).
/// 2. Compute detail: `detail = original - blurred`.
/// 3. Blend: `sharpened = original + amount * detail`.
pub fn unsharp_mask(buffer: &PixelBuffer, amount: f64) -> Result<PixelBuffer, ProcessingError> {
    apply_sharpen(buffer.clone(), amount)
}

/// Internal sharpen implementation.
pub(super) fn apply_sharpen(
    buffer: PixelBuffer,
    amount: f64,
) -> Result<PixelBuffer, ProcessingError> {
    if amount <= 0.0 || buffer.width == 0 || buffer.height == 0 {
        return Ok(buffer);
    }

    let blurred = apply_blur(buffer.clone(), 1.0)?;

    let ch = buffer.channels as usize;
    let color_ch = if ch >= 4 { 3 } else { ch };
    let mut output = buffer.clone();

    for (i, chunk) in output.data.chunks_exact_mut(ch).enumerate() {
        let base_idx = i * ch;
        for c in 0..color_ch {
            let original = buffer.data[base_idx + c] as f64;
            let blur_val = blurred.data[base_idx + c] as f64;
            let sharpened = original + (original - blur_val) * amount;
            chunk[c] = sharpened.round().clamp(0.0, 255.0) as u8;
        }
    }

    Ok(output)
}

// ============================================================================
// Gaussian kernel builder
// ============================================================================

/// Build a 1-D Gaussian kernel for the given sigma.
pub(super) fn build_gaussian_kernel(sigma: f64) -> Vec<f64> {
    let radius = (sigma * 3.0).ceil() as usize;
    let size = radius * 2 + 1;
    let two_sigma_sq = 2.0 * sigma * sigma;

    let mut kernel = Vec::with_capacity(size);
    let mut sum = 0.0;
    for i in 0..size {
        let x = i as f64 - radius as f64;
        let val = (-x * x / two_sigma_sq).exp();
        kernel.push(val);
        sum += val;
    }

    if sum.abs() > f64::EPSILON {
        for v in &mut kernel {
            *v /= sum;
        }
    }
    kernel
}
