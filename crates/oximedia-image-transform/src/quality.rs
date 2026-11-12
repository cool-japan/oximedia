// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! SSIM-guided quality auto-tuning for image transforms.
//!
//! Analyses image complexity (edge density, luminance variance, spatial
//! frequency) and recommends an optimal JPEG/WebP quality setting that
//! preserves perceptual quality while minimising file size.
//!
//! # Algorithm
//!
//! Three signals are combined into a single complexity score:
//! 1. **Edge density** — average gradient magnitude (Sobel-like).
//! 2. **Luminance variance** — overall contrast/texture measure.
//! 3. **Spatial frequency** — Laplacian-based high-frequency content.
//!
//! The complexity score maps to quality via a piecewise linear curve:
//! - Simple images (flat, smooth): quality 50..65
//! - Moderate detail: quality 65..80
//! - High detail: quality 80..90
//! - Very complex: quality 90..95

use crate::processor::PixelBuffer;
use crate::transform::TransformParams;

// ============================================================================
// ComplexityAnalysis result
// ============================================================================

/// Image complexity analysis result for SSIM-guided quality selection.
#[derive(Debug, Clone, PartialEq)]
pub struct ComplexityAnalysis {
    /// Average gradient magnitude (edge density).
    pub edge_density: f64,
    /// Luminance variance (contrast measure).
    pub luminance_variance: f64,
    /// Spatial frequency (high-frequency content measure).
    pub spatial_frequency: f64,
    /// Overall complexity score (0.0 = flat, 1.0 = highly complex).
    pub complexity: f64,
    /// Recommended quality (1-100).
    pub recommended_quality: u8,
}

// ============================================================================
// Public API
// ============================================================================

/// Analyse image complexity and recommend an optimal quality setting.
///
/// The algorithm combines three signals:
/// 1. **Edge density** — average gradient magnitude across the image.
/// 2. **Luminance variance** — measures overall contrast/texture.
/// 3. **Spatial frequency** — high-frequency content ratio.
///
/// Complex images need higher quality to avoid visible artefacts. Simple images
/// (flat gradients, solid colours) can use lower quality with imperceptible
/// quality loss.
pub fn analyse_complexity(buffer: &PixelBuffer) -> ComplexityAnalysis {
    if buffer.width < 2 || buffer.height < 2 {
        return ComplexityAnalysis {
            edge_density: 0.0,
            luminance_variance: 0.0,
            spatial_frequency: 0.0,
            complexity: 0.0,
            recommended_quality: 85,
        };
    }

    // Subsample for performance (max ~256x256 analysis grid)
    let step = ((buffer.width.max(buffer.height)) / 256).max(1);
    let sample_count = ((buffer.width / step) * (buffer.height / step)) as f64;

    if sample_count < 1.0 {
        return ComplexityAnalysis {
            edge_density: 0.0,
            luminance_variance: 0.0,
            spatial_frequency: 0.0,
            complexity: 0.0,
            recommended_quality: 85,
        };
    }

    // Pass 1: compute luminance statistics and gradients
    let mut lum_sum: f64 = 0.0;
    let mut lum_sq_sum: f64 = 0.0;
    let mut grad_sum: f64 = 0.0;
    let mut hf_sum: f64 = 0.0;
    let mut count: f64 = 0.0;

    for y in (0..buffer.height).step_by(step as usize) {
        for x in (0..buffer.width).step_by(step as usize) {
            let lum = pixel_luminance(buffer, x, y);
            lum_sum += lum;
            lum_sq_sum += lum * lum;

            // Gradient magnitude (Sobel-like central differences)
            let lum_right = if x + step < buffer.width {
                pixel_luminance(buffer, x + step, y)
            } else {
                lum
            };
            let lum_below = if y + step < buffer.height {
                pixel_luminance(buffer, x, y + step)
            } else {
                lum
            };
            let gx = lum_right - lum;
            let gy = lum_below - lum;
            grad_sum += (gx * gx + gy * gy).sqrt();

            // High-frequency: second derivative (Laplacian-like)
            let lum_left = if x >= step {
                pixel_luminance(buffer, x - step, y)
            } else {
                lum
            };
            let lum_above = if y >= step {
                pixel_luminance(buffer, x, y - step)
            } else {
                lum
            };
            let laplacian = (lum_right + lum_left + lum_below + lum_above - 4.0 * lum).abs();
            hf_sum += laplacian;

            count += 1.0;
        }
    }

    if count < 1.0 {
        return ComplexityAnalysis {
            edge_density: 0.0,
            luminance_variance: 0.0,
            spatial_frequency: 0.0,
            complexity: 0.0,
            recommended_quality: 85,
        };
    }

    let lum_mean = lum_sum / count;
    let lum_variance = (lum_sq_sum / count - lum_mean * lum_mean).max(0.0);
    let edge_density = grad_sum / count;
    let spatial_frequency = hf_sum / count;

    // Normalise signals to 0..1 range
    let edge_norm = (edge_density / 60.0).clamp(0.0, 1.0);
    let var_norm = (lum_variance / 3000.0).clamp(0.0, 1.0);
    let sf_norm = (spatial_frequency / 100.0).clamp(0.0, 1.0);

    // Weighted combination
    let complexity = (0.4 * edge_norm + 0.35 * var_norm + 0.25 * sf_norm).clamp(0.0, 1.0);

    let recommended_quality = complexity_to_quality(complexity);

    ComplexityAnalysis {
        edge_density,
        luminance_variance: lum_variance,
        spatial_frequency,
        complexity,
        recommended_quality,
    }
}

/// Auto-tune quality based on image content.
///
/// If the requested quality is the default (85), replaces it with a
/// content-aware value. If the user explicitly set a quality, it is
/// respected unchanged.
pub fn auto_tune_quality(buffer: &PixelBuffer, params: &mut TransformParams) {
    if params.quality != crate::transform::DEFAULT_QUALITY {
        return;
    }
    let analysis = analyse_complexity(buffer);
    params.quality = analysis.recommended_quality;
}

/// Simplified SSIM estimate between two pixel buffers.
///
/// Returns a value in 0.0..1.0 where 1.0 means identical. This is a
/// single-scale mean SSIM computed over the luminance channel with
/// an 8x8 sliding window.
pub fn estimate_ssim(a: &PixelBuffer, b: &PixelBuffer) -> f64 {
    if a.width != b.width || a.height != b.height || a.width < 8 || a.height < 8 {
        return 0.0;
    }

    let window = 8u32;
    let c1: f64 = (0.01 * 255.0) * (0.01 * 255.0);
    let c2: f64 = (0.03 * 255.0) * (0.03 * 255.0);

    let mut ssim_sum: f64 = 0.0;
    let mut window_count: f64 = 0.0;

    let step = ((a.width.max(a.height)) / 64).max(1);

    let mut wy = 0;
    while wy + window <= a.height {
        let mut wx = 0;
        while wx + window <= a.width {
            let mut mean_a: f64 = 0.0;
            let mut mean_b: f64 = 0.0;
            let n = (window * window) as f64;

            for dy in 0..window {
                for dx in 0..window {
                    mean_a += pixel_luminance(a, wx + dx, wy + dy);
                    mean_b += pixel_luminance(b, wx + dx, wy + dy);
                }
            }
            mean_a /= n;
            mean_b /= n;

            let mut var_a: f64 = 0.0;
            let mut var_b: f64 = 0.0;
            let mut cov: f64 = 0.0;

            for dy in 0..window {
                for dx in 0..window {
                    let la = pixel_luminance(a, wx + dx, wy + dy) - mean_a;
                    let lb = pixel_luminance(b, wx + dx, wy + dy) - mean_b;
                    var_a += la * la;
                    var_b += lb * lb;
                    cov += la * lb;
                }
            }
            var_a /= n - 1.0;
            var_b /= n - 1.0;
            cov /= n - 1.0;

            let numerator = (2.0 * mean_a * mean_b + c1) * (2.0 * cov + c2);
            let denominator = (mean_a * mean_a + mean_b * mean_b + c1) * (var_a + var_b + c2);

            if denominator.abs() > f64::EPSILON {
                ssim_sum += numerator / denominator;
            }
            window_count += 1.0;

            wx += step;
        }
        wy += step;
    }

    if window_count > 0.0 {
        (ssim_sum / window_count).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Map a complexity score to a quality value.
fn complexity_to_quality(complexity: f64) -> u8 {
    let q = if complexity < 0.2 {
        50.0 + complexity * 75.0
    } else if complexity < 0.5 {
        65.0 + (complexity - 0.2) * 50.0
    } else if complexity < 0.8 {
        80.0 + (complexity - 0.5) * 33.3
    } else {
        90.0 + (complexity - 0.8) * 25.0
    };
    (q.round() as u8).clamp(1, 100)
}

/// Get luminance from a pixel.
fn pixel_luminance(buffer: &PixelBuffer, x: u32, y: u32) -> f64 {
    match buffer.get_pixel(x, y) {
        Some(p) if buffer.channels >= 3 => {
            0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
        }
        Some(p) => p[0] as f64,
        None => 0.0,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_solid_buffer(width: u32, height: u32, color: [u8; 4]) -> PixelBuffer {
        let mut buf = PixelBuffer::new(width, height, 4);
        for y in 0..height {
            for x in 0..width {
                buf.set_pixel(x, y, &color);
            }
        }
        buf
    }

    fn make_test_buffer(width: u32, height: u32) -> PixelBuffer {
        let mut buf = PixelBuffer::new(width, height, 4);
        for y in 0..height {
            for x in 0..width {
                let r = ((x * 255) / width.max(1)) as u8;
                let g = ((y * 255) / height.max(1)) as u8;
                buf.set_pixel(x, y, &[r, g, 128, 255]);
            }
        }
        buf
    }

    // ── Complexity analysis ──

    #[test]
    fn test_analyse_complexity_solid() {
        let buf = make_solid_buffer(100, 100, [128, 128, 128, 255]);
        let analysis = analyse_complexity(&buf);
        assert!(analysis.complexity < 0.2);
        assert!(analysis.recommended_quality < 70);
        assert!(analysis.edge_density < 1.0);
    }

    #[test]
    fn test_analyse_complexity_gradient() {
        let buf = make_test_buffer(100, 100);
        let analysis = analyse_complexity(&buf);
        assert!(analysis.complexity > 0.0);
        assert!(analysis.recommended_quality >= 50);
        assert!(analysis.recommended_quality <= 100);
    }

    #[test]
    fn test_analyse_complexity_checkerboard() {
        let mut buf = PixelBuffer::new(100, 100, 4);
        for y in 0..100 {
            for x in 0..100 {
                let v = if (x + y) % 2 == 0 { 0u8 } else { 255u8 };
                buf.set_pixel(x, y, &[v, v, v, 255]);
            }
        }
        let analysis = analyse_complexity(&buf);
        assert!(analysis.complexity > 0.3);
        assert!(analysis.recommended_quality > 70);
    }

    #[test]
    fn test_analyse_complexity_tiny_buffer() {
        let buf = PixelBuffer::new(1, 1, 4);
        let analysis = analyse_complexity(&buf);
        assert!((analysis.complexity - 0.0).abs() < f64::EPSILON);
        assert_eq!(analysis.recommended_quality, 85);
    }

    #[test]
    fn test_analyse_complexity_empty_buffer() {
        let buf = PixelBuffer::new(0, 0, 4);
        let analysis = analyse_complexity(&buf);
        assert_eq!(analysis.recommended_quality, 85);
    }

    // ── Auto-tune ──

    #[test]
    fn test_auto_tune_quality_modifies_default() {
        let buf = make_solid_buffer(100, 100, [128, 128, 128, 255]);
        let mut params = TransformParams::default();
        assert_eq!(params.quality, 85);
        auto_tune_quality(&buf, &mut params);
        assert!(params.quality < 85);
    }

    #[test]
    fn test_auto_tune_quality_preserves_explicit() {
        let buf = make_solid_buffer(100, 100, [128, 128, 128, 255]);
        let mut params = TransformParams::default();
        params.quality = 95;
        auto_tune_quality(&buf, &mut params);
        assert_eq!(params.quality, 95);
    }

    // ── Quality mapping ──

    #[test]
    fn test_complexity_to_quality_range() {
        assert_eq!(complexity_to_quality(0.0), 50);
        assert_eq!(complexity_to_quality(1.0), 95);
        assert_eq!(complexity_to_quality(0.5), 80);

        for i in 0..=100 {
            let c = i as f64 / 100.0;
            let q = complexity_to_quality(c);
            assert!(q >= 1 && q <= 100);
        }
    }

    // ── SSIM ──

    #[test]
    fn test_ssim_identical() {
        let buf = make_test_buffer(32, 32);
        let ssim = estimate_ssim(&buf, &buf);
        assert!((ssim - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_ssim_different() {
        let a = make_solid_buffer(32, 32, [0, 0, 0, 255]);
        let b = make_solid_buffer(32, 32, [255, 255, 255, 255]);
        let ssim = estimate_ssim(&a, &b);
        assert!(ssim < 0.1);
    }

    #[test]
    fn test_ssim_similar() {
        let a = make_solid_buffer(32, 32, [128, 128, 128, 255]);
        let b = make_solid_buffer(32, 32, [130, 130, 130, 255]);
        let ssim = estimate_ssim(&a, &b);
        assert!(ssim > 0.9);
    }

    #[test]
    fn test_ssim_different_sizes() {
        let a = make_test_buffer(32, 32);
        let b = make_test_buffer(64, 64);
        let ssim = estimate_ssim(&a, &b);
        assert!((ssim - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_ssim_tiny_image() {
        let a = make_test_buffer(4, 4);
        let b = make_test_buffer(4, 4);
        let ssim = estimate_ssim(&a, &b);
        assert!((ssim - 0.0).abs() < f64::EPSILON);
    }

    // ── Pixel luminance ──

    #[test]
    fn test_pixel_luminance_white() {
        let buf = make_solid_buffer(4, 4, [255, 255, 255, 255]);
        let l = pixel_luminance(&buf, 0, 0);
        assert!((l - 255.0).abs() < 1.0);
    }

    #[test]
    fn test_pixel_luminance_black() {
        let buf = make_solid_buffer(4, 4, [0, 0, 0, 255]);
        let l = pixel_luminance(&buf, 0, 0);
        assert!(l.abs() < f64::EPSILON);
    }

    #[test]
    fn test_pixel_luminance_out_of_bounds() {
        let buf = make_solid_buffer(4, 4, [128, 128, 128, 255]);
        let l = pixel_luminance(&buf, 10, 10);
        assert!(l.abs() < f64::EPSILON);
    }
}
