// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Saliency-based face detection for smart cropping.
//!
//! Implements a skin-tone detection and saliency analysis pipeline that
//! identifies face-like regions in an image. When used with
//! [`Gravity::Auto`](crate::transform::Gravity::Auto) or
//! [`Gravity::Face`](crate::transform::Gravity::Face), the detected region
//! centre is used as the crop gravity anchor.
//!
//! # Algorithm
//!
//! 1. **Skin-tone detection** — pixels are classified using a YCbCr colour
//!    model: `80 <= Cb <= 120`, `133 <= Cr <= 173` (empirical ranges that
//!    generalize well across skin tones).
//! 2. **Saliency map** — a gradient magnitude map (Sobel-like) is computed,
//!    which highlights edges and high-contrast regions.
//! 3. **Combined score** — skin-tone probability and saliency are blended
//!    into a single weight map.
//! 4. **Weighted centroid** — the (x, y) centre of mass of the weight map
//!    is returned as the focal gravity point.
//!
//! All algorithms are pure Rust with no external dependencies.

use crate::processor::PixelBuffer;

// ============================================================================
// Face detection result
// ============================================================================

/// Result of a face/saliency detection pass.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectionResult {
    /// Normalised x coordinate (0.0..1.0) of the gravity centre.
    pub gravity_x: f64,
    /// Normalised y coordinate (0.0..1.0) of the gravity centre.
    pub gravity_y: f64,
    /// Number of skin-tone pixels detected.
    pub skin_pixel_count: usize,
    /// Total saliency score (sum of gradient magnitudes).
    pub saliency_score: f64,
    /// Confidence (0.0..1.0). Higher values indicate stronger face evidence.
    pub confidence: f64,
}

impl Default for DetectionResult {
    fn default() -> Self {
        Self {
            gravity_x: 0.5,
            gravity_y: 0.5,
            skin_pixel_count: 0,
            saliency_score: 0.0,
            confidence: 0.0,
        }
    }
}

// ============================================================================
// Detection configuration
// ============================================================================

/// Configuration for the face/saliency detector.
#[derive(Debug, Clone)]
pub struct DetectionConfig {
    /// Weight for skin-tone signal in the combined score (0.0..1.0).
    pub skin_weight: f64,
    /// Weight for saliency signal in the combined score (0.0..1.0).
    pub saliency_weight: f64,
    /// Subsample stride for analysis (higher = faster, less precise).
    pub subsample: u32,
    /// Minimum skin pixel ratio to consider detection confident.
    pub min_skin_ratio: f64,
}

impl Default for DetectionConfig {
    fn default() -> Self {
        Self {
            skin_weight: 0.7,
            saliency_weight: 0.3,
            subsample: 2,
            min_skin_ratio: 0.005,
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Detect faces / salient regions and return a [`DetectionResult`].
///
/// The returned gravity coordinates can be used directly with
/// [`Gravity::FocalPoint`](crate::transform::Gravity::FocalPoint).
pub fn detect_faces(buffer: &PixelBuffer, config: &DetectionConfig) -> DetectionResult {
    if buffer.width < 2 || buffer.height < 2 {
        return DetectionResult::default();
    }

    let step = config.subsample.max(1);
    let sample_w = (buffer.width + step - 1) / step;
    let sample_h = (buffer.height + step - 1) / step;
    let total_samples = sample_w as usize * sample_h as usize;

    if total_samples == 0 {
        return DetectionResult::default();
    }

    // Build combined weight map
    let mut skin_count: usize = 0;
    let mut total_saliency: f64 = 0.0;
    let mut weight_sum: f64 = 0.0;
    let mut wx_sum: f64 = 0.0;
    let mut wy_sum: f64 = 0.0;

    for sy in (0..buffer.height).step_by(step as usize) {
        for sx in (0..buffer.width).step_by(step as usize) {
            let rgba = get_rgba(buffer, sx, sy);
            let (r, g, b) = (rgba[0], rgba[1], rgba[2]);

            // 1. Skin-tone score
            let skin = skin_tone_score(r, g, b);
            if skin > 0.5 {
                skin_count += 1;
            }

            // 2. Saliency (gradient magnitude via differences with neighbours)
            let saliency = gradient_magnitude(buffer, sx, sy, step);
            total_saliency += saliency;

            // 3. Combined weight
            let w = skin * config.skin_weight + saliency * config.saliency_weight;
            weight_sum += w;
            wx_sum += w * sx as f64;
            wy_sum += w * sy as f64;
        }
    }

    let skin_ratio = skin_count as f64 / total_samples as f64;

    if weight_sum < f64::EPSILON {
        return DetectionResult {
            gravity_x: 0.5,
            gravity_y: 0.5,
            skin_pixel_count: skin_count,
            saliency_score: total_saliency,
            confidence: 0.0,
        };
    }

    let cx = wx_sum / weight_sum;
    let cy = wy_sum / weight_sum;

    // Normalise to 0..1
    let gx = (cx / buffer.width.saturating_sub(1).max(1) as f64).clamp(0.0, 1.0);
    let gy = (cy / buffer.height.saturating_sub(1).max(1) as f64).clamp(0.0, 1.0);

    // Confidence based on skin ratio and saliency
    let confidence = if skin_ratio >= config.min_skin_ratio {
        (skin_ratio * 10.0).clamp(0.0, 1.0)
    } else {
        let sal_norm = total_saliency / total_samples as f64;
        (sal_norm / 128.0).clamp(0.0, 0.5)
    };

    DetectionResult {
        gravity_x: gx,
        gravity_y: gy,
        skin_pixel_count: skin_count,
        saliency_score: total_saliency,
        confidence,
    }
}

/// Detect the gravity centre and return it as `(x, y)` normalised coordinates.
///
/// This is a convenience wrapper around [`detect_faces`].
pub fn smart_gravity(buffer: &PixelBuffer) -> (f64, f64) {
    let result = detect_faces(buffer, &DetectionConfig::default());
    (result.gravity_x, result.gravity_y)
}

// ============================================================================
// Skin-tone detection (YCbCr model)
// ============================================================================

/// Compute a skin-tone probability score for an (R, G, B) pixel.
///
/// Converts to YCbCr and checks empirical skin-tone ranges:
/// - Cb: 80..=120
/// - Cr: 133..=173
/// - Y: >= 50 (not too dark)
///
/// Returns 0.0 (not skin) or 1.0 (skin).
fn skin_tone_score(r: u8, g: u8, b: u8) -> f64 {
    let rf = r as f64;
    let gf = g as f64;
    let bf = b as f64;

    // ITU-R BT.601 YCbCr conversion
    let y = 0.299 * rf + 0.587 * gf + 0.114 * bf;
    let cb = 128.0 - 0.168736 * rf - 0.331264 * gf + 0.5 * bf;
    let cr = 128.0 + 0.5 * rf - 0.418688 * gf - 0.081312 * bf;

    // Empirical skin-tone bounds
    if y >= 50.0 && (80.0..=120.0).contains(&cb) && (133.0..=173.0).contains(&cr) {
        1.0
    } else {
        0.0
    }
}

// ============================================================================
// Saliency (gradient magnitude)
// ============================================================================

/// Compute gradient magnitude at (x, y) using central differences.
///
/// Uses luminance of neighbouring pixels. Returns a value in 0..~360
/// (sqrt of summed squared gradients in x and y).
fn gradient_magnitude(buffer: &PixelBuffer, x: u32, y: u32, step: u32) -> f64 {
    let lum_c = luminance(get_rgba(buffer, x, y));

    // Horizontal gradient
    let lum_l = if x >= step {
        luminance(get_rgba(buffer, x - step, y))
    } else {
        lum_c
    };
    let lum_r = if x + step < buffer.width {
        luminance(get_rgba(buffer, x + step, y))
    } else {
        lum_c
    };
    let gx = lum_r - lum_l;

    // Vertical gradient
    let lum_t = if y >= step {
        luminance(get_rgba(buffer, x, y - step))
    } else {
        lum_c
    };
    let lum_b = if y + step < buffer.height {
        luminance(get_rgba(buffer, x, y + step))
    } else {
        lum_c
    };
    let gy = lum_b - lum_t;

    (gx * gx + gy * gy).sqrt()
}

/// Compute perceptual luminance from RGBA.
fn luminance(rgba: [u8; 4]) -> f64 {
    0.299 * rgba[0] as f64 + 0.587 * rgba[1] as f64 + 0.114 * rgba[2] as f64
}

/// Get pixel as [R, G, B, A] with bounds clamping.
fn get_rgba(buffer: &PixelBuffer, x: u32, y: u32) -> [u8; 4] {
    let cx = x.min(buffer.width.saturating_sub(1));
    let cy = y.min(buffer.height.saturating_sub(1));
    match buffer.get_pixel(cx, cy) {
        Some(p) if buffer.channels >= 4 => [p[0], p[1], p[2], p[3]],
        Some(p) if buffer.channels >= 3 => [p[0], p[1], p[2], 255],
        Some(p) => [p[0], p[0], p[0], 255],
        None => [0, 0, 0, 255],
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

    // ── Default result ──

    #[test]
    fn test_default_result() {
        let r = DetectionResult::default();
        assert!((r.gravity_x - 0.5).abs() < f64::EPSILON);
        assert!((r.gravity_y - 0.5).abs() < f64::EPSILON);
        assert_eq!(r.skin_pixel_count, 0);
        assert_eq!(r.confidence, 0.0);
    }

    // ── Tiny buffer ──

    #[test]
    fn test_tiny_buffer_returns_center() {
        let buf = PixelBuffer::new(1, 1, 4);
        let result = detect_faces(&buf, &DetectionConfig::default());
        assert!((result.gravity_x - 0.5).abs() < f64::EPSILON);
    }

    // ── Uniform buffer ──

    #[test]
    fn test_uniform_gray_low_saliency() {
        let buf = make_solid_buffer(100, 100, [128, 128, 128, 255]);
        let result = detect_faces(&buf, &DetectionConfig::default());
        // Uniform gray: no gradients, no skin
        assert_eq!(result.skin_pixel_count, 0);
        // Saliency should be zero (no gradients)
        assert!(result.saliency_score.abs() < 1.0);
    }

    // ── Skin-tone detection ──

    #[test]
    fn test_skin_tone_warm_skin() {
        // Typical warm skin tone in RGB
        let score = skin_tone_score(200, 150, 120);
        assert!(score > 0.5);
    }

    #[test]
    fn test_skin_tone_blue_not_skin() {
        let score = skin_tone_score(0, 0, 255);
        assert!(score < 0.5);
    }

    #[test]
    fn test_skin_tone_green_not_skin() {
        let score = skin_tone_score(0, 255, 0);
        assert!(score < 0.5);
    }

    #[test]
    fn test_skin_tone_dark_not_skin() {
        // Very dark pixels should be rejected (Y < 50)
        let score = skin_tone_score(10, 10, 10);
        assert!(score < 0.5);
    }

    #[test]
    fn test_skin_tone_white_not_skin() {
        let score = skin_tone_score(255, 255, 255);
        assert!(score < 0.5);
    }

    #[test]
    fn test_skin_tone_various_tones() {
        // Light skin
        assert!(skin_tone_score(230, 180, 150) > 0.5);
        // Medium skin
        assert!(skin_tone_score(190, 140, 110) > 0.5);
        // Various non-skin colors
        assert!(skin_tone_score(255, 0, 0) < 0.5); // pure red
        assert!(skin_tone_score(128, 128, 128) < 0.5); // gray
    }

    // ── Gradient magnitude ──

    #[test]
    fn test_gradient_uniform_is_zero() {
        let buf = make_solid_buffer(50, 50, [128, 128, 128, 255]);
        let g = gradient_magnitude(&buf, 25, 25, 1);
        assert!(g.abs() < f64::EPSILON);
    }

    #[test]
    fn test_gradient_sharp_edge() {
        let mut buf = PixelBuffer::new(20, 20, 4);
        // Left half black, right half white
        for y in 0..20 {
            for x in 0..10 {
                buf.set_pixel(x, y, &[0, 0, 0, 255]);
            }
            for x in 10..20 {
                buf.set_pixel(x, y, &[255, 255, 255, 255]);
            }
        }
        // Gradient at the edge should be high
        let g = gradient_magnitude(&buf, 10, 10, 1);
        assert!(g > 100.0);
    }

    #[test]
    fn test_gradient_at_border() {
        let buf = make_solid_buffer(10, 10, [128, 128, 128, 255]);
        // Should not panic at borders
        let g = gradient_magnitude(&buf, 0, 0, 1);
        assert!(g.abs() < f64::EPSILON);
    }

    // ── Smart gravity ──

    #[test]
    fn test_smart_gravity_uniform() {
        let buf = make_solid_buffer(100, 100, [128, 128, 128, 255]);
        let (gx, gy) = smart_gravity(&buf);
        // Uniform buffer: gravity should be near center
        assert!((gx - 0.5).abs() < 0.2);
        assert!((gy - 0.5).abs() < 0.2);
    }

    #[test]
    fn test_smart_gravity_bright_corner() {
        let mut buf = make_solid_buffer(100, 100, [0, 0, 0, 255]);
        // Put a bright region in top-left corner
        for y in 0..30 {
            for x in 0..30 {
                buf.set_pixel(x, y, &[255, 255, 255, 255]);
            }
        }
        let (gx, gy) = smart_gravity(&buf);
        // Gravity should be pulled toward the bright corner (edges have gradient)
        assert!(gx < 0.5);
        assert!(gy < 0.5);
    }

    #[test]
    fn test_smart_gravity_skin_region() {
        let mut buf = make_solid_buffer(200, 200, [50, 50, 50, 255]);
        // Paint a skin-toned region in the bottom-right
        for y in 150..200 {
            for x in 150..200 {
                buf.set_pixel(x, y, &[200, 150, 120, 255]);
            }
        }
        let config = DetectionConfig::default();
        let result = detect_faces(&buf, &config);
        // Should detect skin pixels
        assert!(result.skin_pixel_count > 0);
        // Gravity should be pulled toward the skin region
        assert!(result.gravity_x > 0.5);
        assert!(result.gravity_y > 0.5);
    }

    // ── Luminance ──

    #[test]
    fn test_luminance_black() {
        assert!(luminance([0, 0, 0, 255]).abs() < f64::EPSILON);
    }

    #[test]
    fn test_luminance_white() {
        let l = luminance([255, 255, 255, 255]);
        assert!((l - 255.0).abs() < 1.0);
    }

    #[test]
    fn test_luminance_green_brightest() {
        // Green contributes most to luminance (0.587)
        let lg = luminance([0, 255, 0, 255]);
        let lr = luminance([255, 0, 0, 255]);
        let lb = luminance([0, 0, 255, 255]);
        assert!(lg > lr);
        assert!(lg > lb);
    }

    // ── DetectionConfig ──

    #[test]
    fn test_detection_config_defaults() {
        let config = DetectionConfig::default();
        assert!((config.skin_weight - 0.7).abs() < f64::EPSILON);
        assert!((config.saliency_weight - 0.3).abs() < f64::EPSILON);
        assert_eq!(config.subsample, 2);
        assert!((config.min_skin_ratio - 0.005).abs() < f64::EPSILON);
    }

    // ── Edge cases ──

    #[test]
    fn test_detect_faces_zero_size() {
        let buf = PixelBuffer::new(0, 0, 4);
        let result = detect_faces(&buf, &DetectionConfig::default());
        assert!((result.gravity_x - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_detect_faces_subsample_large() {
        let buf = make_solid_buffer(100, 100, [200, 150, 120, 255]);
        let config = DetectionConfig {
            subsample: 50,
            ..DetectionConfig::default()
        };
        let result = detect_faces(&buf, &config);
        assert!(result.skin_pixel_count > 0);
    }

    #[test]
    fn test_detect_rgb_buffer() {
        let data = vec![128u8; 50 * 50 * 3];
        let buf = PixelBuffer::from_rgb(data, 50, 50).expect("valid");
        let (gx, gy) = smart_gravity(&buf);
        assert!(gx >= 0.0 && gx <= 1.0);
        assert!(gy >= 0.0 && gy <= 1.0);
    }

    #[test]
    fn test_detect_confidence_with_skin() {
        let mut buf = make_solid_buffer(100, 100, [50, 50, 50, 255]);
        // Fill 50% with skin tone
        for y in 0..100 {
            for x in 50..100 {
                buf.set_pixel(x, y, &[200, 150, 120, 255]);
            }
        }
        let result = detect_faces(&buf, &DetectionConfig::default());
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn test_detect_confidence_no_skin() {
        let buf = make_solid_buffer(100, 100, [128, 128, 128, 255]);
        let result = detect_faces(&buf, &DetectionConfig::default());
        assert!(result.confidence < 0.5);
    }
}
