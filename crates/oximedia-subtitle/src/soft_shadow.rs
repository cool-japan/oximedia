// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Soft-shadow rendering for subtitle burn-in.
//!
//! Provides Gaussian blur, shadow configuration, and compositing of
//! drop-shadow text onto RGBA frame buffers.

use super::burn_in::BurnInColor;

// ============================================================================
// Soft-shadow rendering
// ============================================================================

/// Configuration for soft-shadow rendering behind subtitle text.
///
/// A rasterised text bitmap used as input to shadow/compositing operations.
///
/// Each byte in `bitmap` represents the alpha coverage of a single pixel
/// (0 = fully transparent, 255 = fully opaque).
#[derive(Debug, Clone)]
pub struct TextBitmap {
    /// Alpha-coverage bitmap data, row-major, `width * height` bytes.
    pub bitmap: Vec<u8>,
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
}

/// A Gaussian-blurred shadow is rendered before the main text, giving a
/// natural drop-shadow appearance that remains legible on any background.
#[derive(Debug, Clone)]
pub struct SoftShadowConfig {
    /// Shadow offset in pixels (horizontal).
    pub offset_x: i32,
    /// Shadow offset in pixels (vertical).
    pub offset_y: i32,
    /// Gaussian blur radius in pixels (0 = hard shadow, higher = softer).
    pub blur_radius: f32,
    /// Shadow colour and opacity.
    pub color: BurnInColor,
}

impl Default for SoftShadowConfig {
    fn default() -> Self {
        Self {
            offset_x: 2,
            offset_y: 2,
            blur_radius: 3.0,
            color: BurnInColor::black_with_alpha(180),
        }
    }
}

impl SoftShadowConfig {
    /// Create a subtle soft shadow suitable for web/streaming.
    #[must_use]
    pub fn web() -> Self {
        Self::default()
    }

    /// Create a heavier broadcast shadow.
    #[must_use]
    pub fn broadcast() -> Self {
        Self {
            offset_x: 3,
            offset_y: 3,
            blur_radius: 5.0,
            color: BurnInColor::black_with_alpha(220),
        }
    }
}

/// Apply a separable Gaussian blur to a single-channel alpha bitmap.
///
/// Returns a new blurred bitmap of the same dimensions.
/// `radius` controls the standard deviation of the Gaussian kernel (in pixels).
///
/// # Panics
///
/// Panics if `width == 0 || height == 0` (no valid bitmap to blur).
#[must_use]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn gaussian_blur_alpha(bitmap: &[u8], width: u32, height: u32, radius: f32) -> Vec<u8> {
    if width == 0 || height == 0 || bitmap.is_empty() {
        return bitmap.to_vec();
    }

    let sigma = radius.max(0.5);
    let kernel_radius = (sigma * 3.0).ceil() as usize;
    let kernel_size = 2 * kernel_radius + 1;

    // Build Gaussian kernel (normalised).
    let mut kernel = Vec::with_capacity(kernel_size);
    let mut kernel_sum = 0.0_f32;
    for i in 0..kernel_size {
        let x = i as f32 - kernel_radius as f32;
        let val = (-0.5 * (x / sigma) * (x / sigma)).exp();
        kernel.push(val);
        kernel_sum += val;
    }
    for k in &mut kernel {
        *k /= kernel_sum;
    }

    let w = width as usize;
    let h = height as usize;
    let mut tmp = vec![0.0_f32; w * h];
    let mut out = vec![0u8; w * h];

    // Horizontal pass
    for row in 0..h {
        for col in 0..w {
            let mut acc = 0.0_f32;
            for (ki, &kval) in kernel.iter().enumerate() {
                let src_col = col as isize + ki as isize - kernel_radius as isize;
                let src_col = src_col.clamp(0, w as isize - 1) as usize;
                acc += bitmap[row * w + src_col] as f32 * kval;
            }
            tmp[row * w + col] = acc;
        }
    }

    // Vertical pass
    for row in 0..h {
        for col in 0..w {
            let mut acc = 0.0_f32;
            for (ki, &kval) in kernel.iter().enumerate() {
                let src_row = row as isize + ki as isize - kernel_radius as isize;
                let src_row = src_row.clamp(0, h as isize - 1) as usize;
                acc += tmp[src_row * w + col] * kval;
            }
            out[row * w + col] = acc.clamp(0.0, 255.0) as u8;
        }
    }

    out
}

/// Render soft-shadow text onto an RGBA pixel buffer.
///
/// The shadow is rendered first (at an offset with Gaussian blur), then the
/// main opaque text is composited on top.  The blur radius is taken from
/// `shadow.blur_radius`.
///
/// # Errors
///
/// Returns `Err` if the buffer is smaller than `frame_w * frame_h * 4` bytes.
#[allow(
    clippy::too_many_arguments,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
pub fn render_soft_shadow_rgba(
    buffer: &mut [u8],
    frame_w: u32,
    frame_h: u32,
    text_bitmap: &TextBitmap,
    text_color: BurnInColor,
    text_x: u32,
    text_y: u32,
    shadow: &SoftShadowConfig,
) -> Result<(), String> {
    let expected = (frame_w as usize) * (frame_h as usize) * 4;
    if buffer.len() < expected {
        return Err(format!("Buffer too small: {} < {expected}", buffer.len()));
    }
    if text_bitmap.width == 0 || text_bitmap.height == 0 {
        return Ok(());
    }

    // ----- 1. Render blurred shadow -----
    let blurred = gaussian_blur_alpha(
        &text_bitmap.bitmap,
        text_bitmap.width,
        text_bitmap.height,
        shadow.blur_radius,
    );

    let shadow_alpha_base = f32::from(shadow.color.a) / 255.0;
    let sh_w = text_bitmap.width;
    let sh_h = text_bitmap.height;

    for gy in 0..sh_h {
        for gx in 0..sh_w {
            let sx = text_x as i64 + gx as i64 + shadow.offset_x as i64;
            let sy = text_y as i64 + gy as i64 + shadow.offset_y as i64;

            if sx < 0 || sy < 0 || sx >= frame_w as i64 || sy >= frame_h as i64 {
                continue;
            }

            let glyph_idx = (gy * sh_w + gx) as usize;
            let glyph_alpha = blurred.get(glyph_idx).copied().unwrap_or(0);
            if glyph_alpha == 0 {
                continue;
            }

            let alpha_f = glyph_alpha as f32 / 255.0 * shadow_alpha_base;
            let inv_alpha = 1.0 - alpha_f;
            let idx = (sy as u32 * frame_w + sx as u32) as usize * 4;
            if idx + 3 >= buffer.len() {
                continue;
            }
            buffer[idx] = (shadow.color.r as f32 * alpha_f + buffer[idx] as f32 * inv_alpha) as u8;
            buffer[idx + 1] =
                (shadow.color.g as f32 * alpha_f + buffer[idx + 1] as f32 * inv_alpha) as u8;
            buffer[idx + 2] =
                (shadow.color.b as f32 * alpha_f + buffer[idx + 2] as f32 * inv_alpha) as u8;
            buffer[idx + 3] =
                buffer[idx + 3].saturating_add(((255.0 - buffer[idx + 3] as f32) * alpha_f) as u8);
        }
    }

    // ----- 2. Render main text on top -----
    let text_alpha_base = f32::from(text_color.a) / 255.0;
    for gy in 0..text_bitmap.height {
        for gx in 0..text_bitmap.width {
            let px = text_x + gx;
            let py = text_y + gy;
            if px >= frame_w || py >= frame_h {
                continue;
            }

            let glyph_idx = (gy * text_bitmap.width + gx) as usize;
            let glyph_alpha = text_bitmap.bitmap.get(glyph_idx).copied().unwrap_or(0);
            if glyph_alpha == 0 {
                continue;
            }

            let alpha_f = glyph_alpha as f32 / 255.0 * text_alpha_base;
            let inv_alpha = 1.0 - alpha_f;
            let idx = (py * frame_w + px) as usize * 4;
            if idx + 3 >= buffer.len() {
                continue;
            }
            buffer[idx] = (text_color.r as f32 * alpha_f + buffer[idx] as f32 * inv_alpha) as u8;
            buffer[idx + 1] =
                (text_color.g as f32 * alpha_f + buffer[idx + 1] as f32 * inv_alpha) as u8;
            buffer[idx + 2] =
                (text_color.b as f32 * alpha_f + buffer[idx + 2] as f32 * inv_alpha) as u8;
            buffer[idx + 3] =
                buffer[idx + 3].saturating_add(((255.0 - buffer[idx + 3] as f32) * alpha_f) as u8);
        }
    }

    Ok(())
}

#[cfg(test)]
mod soft_shadow_tests {
    use super::*;

    #[test]
    fn test_gaussian_blur_preserves_size() {
        let bm = vec![255u8; 100]; // 10x10
        let blurred = gaussian_blur_alpha(&bm, 10, 10, 2.0);
        assert_eq!(blurred.len(), 100);
    }

    #[test]
    fn test_gaussian_blur_spreads_signal() {
        // A single bright pixel at the center, everything else dark.
        // After blur, neighbouring pixels should gain some value.
        let mut bm = vec![0u8; 25]; // 5x5
        bm[12] = 255; // center pixel
        let blurred = gaussian_blur_alpha(&bm, 5, 5, 1.5);
        // Corners should have gained signal from the blur
        assert!(blurred[0] > 0, "corner should gain signal from blur");
        // Center should still be the brightest
        assert!(
            blurred[12] > blurred[0],
            "center should be brighter than corner"
        );
    }

    #[test]
    fn test_gaussian_blur_zero_radius_is_near_identity() {
        let bm = vec![128u8, 64, 200, 10, 0];
        let blurred = gaussian_blur_alpha(&bm, 5, 1, 0.5);
        assert_eq!(blurred.len(), 5);
        // Values should be similar to original (slight blurring due to kernel)
        for (orig, blur) in bm.iter().zip(blurred.iter()) {
            let diff = (*orig as i32 - *blur as i32).unsigned_abs();
            assert!(diff < 80, "blur too aggressive: orig={orig} blur={blur}");
        }
    }

    #[test]
    fn test_gaussian_blur_empty_returns_empty() {
        let blurred = gaussian_blur_alpha(&[], 0, 0, 2.0);
        assert!(blurred.is_empty());
    }

    #[test]
    fn test_soft_shadow_config_defaults() {
        let cfg = SoftShadowConfig::default();
        assert_eq!(cfg.offset_x, 2);
        assert_eq!(cfg.offset_y, 2);
        assert!((cfg.blur_radius - 3.0).abs() < f32::EPSILON);
        assert_eq!(cfg.color.a, 180);
    }

    #[test]
    fn test_soft_shadow_config_broadcast() {
        let cfg = SoftShadowConfig::broadcast();
        assert_eq!(cfg.offset_x, 3);
        assert!((cfg.blur_radius - 5.0).abs() < f32::EPSILON);
        assert!(cfg.color.a > 180);
    }

    #[test]
    fn test_render_soft_shadow_basic() {
        let w = 64u32;
        let h = 32u32;
        let mut buf = vec![0u8; (w * h * 4) as usize];

        // Build a simple text bitmap: bright 8x8 block
        let bm = TextBitmap {
            bitmap: vec![200u8; 64],
            width: 8,
            height: 8,
        };

        let shadow = SoftShadowConfig::default();
        let result =
            render_soft_shadow_rgba(&mut buf, w, h, &bm, BurnInColor::white(), 10, 10, &shadow);
        assert!(result.is_ok());
        // Some pixels should have been written
        assert!(buf.iter().any(|&b| b > 0));
    }

    #[test]
    fn test_render_soft_shadow_buffer_too_small() {
        let mut buf = vec![0u8; 10];
        let bm = TextBitmap {
            bitmap: vec![255; 4],
            width: 2,
            height: 2,
        };
        let shadow = SoftShadowConfig::default();
        let result =
            render_soft_shadow_rgba(&mut buf, 100, 100, &bm, BurnInColor::white(), 0, 0, &shadow);
        assert!(result.is_err());
    }

    #[test]
    fn test_render_soft_shadow_shadow_precedes_text() {
        // Render shadow-only by using fully transparent text color,
        // then verify shadow pixels were painted.
        let w = 64u32;
        let h = 64u32;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let bm = TextBitmap {
            bitmap: vec![255u8; 100],
            width: 10,
            height: 10,
        };
        let shadow = SoftShadowConfig {
            offset_x: 5,
            offset_y: 5,
            blur_radius: 1.0,
            color: BurnInColor::new(0, 0, 0, 200),
        };
        render_soft_shadow_rgba(
            &mut buf,
            w,
            h,
            &bm,
            BurnInColor::new(255, 255, 255, 0), // transparent text
            0,
            0,
            &shadow,
        )
        .expect("render");
        // Shadow at offset (5,5) should have painted alpha
        assert!(buf.iter().any(|&b| b > 0), "shadow should paint pixels");
    }
}
