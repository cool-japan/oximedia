//! Bilateral filter — edge-preserving smoothing.
//!
//! The bilateral filter replaces each pixel with a weighted average of its
//! neighbours where weights consider both spatial proximity **and** intensity
//! similarity.  This preserves sharp edges while smoothing homogeneous regions.
//!
//! ## Algorithm
//! For each output pixel `p`:
//! ```text
//! dst[p] = sum_{q in N(p)} w(p,q) * I(q)  /  sum_{q in N(p)} w(p,q)
//! w(p,q) = exp(-||p-q||² / (2σs²))  *  exp(-(I(p)-I(q))² / (2σr²))
//! ```
//! where `N(p)` is the square neighbourhood of radius `r` around `p`.

#![allow(dead_code)]

use crate::parallel::map_range;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Bilateral filter configuration.
#[derive(Debug, Clone)]
pub struct BilateralConfig {
    /// Spatial Gaussian sigma (pixels).  Typical range: 2.0 – 10.0.
    pub sigma_spatial: f32,
    /// Range / intensity Gaussian sigma (normalised 0–1).  Typical: 0.1 – 0.3.
    pub sigma_range: f32,
    /// Neighbourhood radius (pixels).  Default = ⌈3 × σs⌉, minimum 1.
    pub radius: usize,
}

impl BilateralConfig {
    /// Create a config, computing `radius = max(1, ceil(3 * sigma_spatial))`.
    #[must_use]
    pub fn new(sigma_spatial: f32, sigma_range: f32) -> Self {
        let radius = ((3.0_f32 * sigma_spatial).ceil() as usize).max(1);
        Self {
            sigma_spatial,
            sigma_range,
            radius,
        }
    }

    /// Create a config with explicit radius (useful for fine-tuning).
    #[must_use]
    pub fn fine(sigma_spatial: f32, sigma_range: f32, radius: usize) -> Self {
        Self {
            sigma_spatial,
            sigma_range,
            radius: radius.max(1),
        }
    }
}

impl Default for BilateralConfig {
    /// Default: σs = 5.0, σr = 0.15.
    fn default() -> Self {
        Self::new(5.0, 0.15)
    }
}

// ---------------------------------------------------------------------------
// Grayscale bilateral filter
// ---------------------------------------------------------------------------

/// Apply bilateral filter to a grayscale image (f32 pixels in 0.0 … 1.0).
///
/// # Parameters
/// - `src`    – input pixel buffer, length `width * height`.
/// - `dst`    – output pixel buffer, length `width * height`.
/// - `width`  – image width in pixels.
/// - `height` – image height in pixels.
/// - `config` – bilateral filter parameters.
///
/// # Errors
/// Returns `Err` if buffer lengths do not match `width * height`.
#[allow(clippy::cast_precision_loss)]
pub fn bilateral_filter_gray(
    src: &[f32],
    dst: &mut [f32],
    width: usize,
    height: usize,
    config: &BilateralConfig,
) -> Result<(), String> {
    let n = width * height;
    if src.len() != n {
        return Err(format!(
            "bilateral_filter_gray: src length {} != width*height {}",
            src.len(),
            n
        ));
    }
    if dst.len() != n {
        return Err(format!(
            "bilateral_filter_gray: dst length {} != width*height {}",
            dst.len(),
            n
        ));
    }
    if width == 0 || height == 0 {
        return Ok(());
    }

    validate_config(config)?;

    let two_ss2 = 2.0 * config.sigma_spatial * config.sigma_spatial;
    let two_sr2 = 2.0 * config.sigma_range * config.sigma_range;
    let r = config.radius as isize;

    for cy in 0..height {
        for cx in 0..width {
            let center_val = src[cy * width + cx];
            let mut weight_sum = 0.0_f32;
            let mut value_sum = 0.0_f32;

            let y_lo = (cy as isize - r).max(0) as usize;
            let y_hi = (cy as isize + r).min(height as isize - 1) as usize;
            let x_lo = (cx as isize - r).max(0) as usize;
            let x_hi = (cx as isize + r).min(width as isize - 1) as usize;

            for ny in y_lo..=y_hi {
                let dy = (ny as isize - cy as isize) as f32;
                for nx in x_lo..=x_hi {
                    let dx = (nx as isize - cx as isize) as f32;
                    let neighbor_val = src[ny * width + nx];

                    let spatial_w = (-(dx * dx + dy * dy) / two_ss2).exp();
                    let range_diff = center_val - neighbor_val;
                    let range_w = (-(range_diff * range_diff) / two_sr2).exp();
                    let w = spatial_w * range_w;

                    weight_sum += w;
                    value_sum += w * neighbor_val;
                }
            }

            dst[cy * width + cx] = if weight_sum > f32::EPSILON {
                value_sum / weight_sum
            } else {
                center_val
            };
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// RGBA bilateral filter
// ---------------------------------------------------------------------------

/// Apply bilateral filter to an RGBA u8 image.
///
/// Each RGB channel is filtered independently.  The alpha channel is copied
/// unchanged.  Pixel layout is `[R, G, B, A, R, G, B, A, …]`.
///
/// # Parameters
/// - `src`    – input pixel buffer, length `4 * width * height`.
/// - `dst`    – output pixel buffer, length `4 * width * height`.
///
/// # Errors
/// Returns `Err` if buffer lengths do not match `4 * width * height`.
#[allow(clippy::cast_precision_loss)]
pub fn bilateral_filter_rgba(
    src: &[u8],
    dst: &mut [u8],
    width: usize,
    height: usize,
    config: &BilateralConfig,
) -> Result<(), String> {
    let n = width * height;
    let n4 = 4 * n;
    if src.len() != n4 {
        return Err(format!(
            "bilateral_filter_rgba: src length {} != 4*width*height {}",
            src.len(),
            n4
        ));
    }
    if dst.len() != n4 {
        return Err(format!(
            "bilateral_filter_rgba: dst length {} != 4*width*height {}",
            dst.len(),
            n4
        ));
    }
    if width == 0 || height == 0 {
        return Ok(());
    }

    validate_config(config)?;

    // Process each RGB channel independently
    for channel in 0..3usize {
        // Extract channel to f32 [0,1]
        let mut chan_src = vec![0.0_f32; n];
        for i in 0..n {
            chan_src[i] = src[i * 4 + channel] as f32 / 255.0;
        }

        let mut chan_dst = vec![0.0_f32; n];
        bilateral_filter_gray(&chan_src, &mut chan_dst, width, height, config)?;

        // Write back, clamping to [0,255]
        for i in 0..n {
            let v = (chan_dst[i] * 255.0).round().clamp(0.0, 255.0) as u8;
            dst[i * 4 + channel] = v;
        }
    }

    // Copy alpha channel unchanged
    for i in 0..n {
        dst[i * 4 + 3] = src[i * 4 + 3];
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn validate_config(config: &BilateralConfig) -> Result<(), String> {
    if config.sigma_spatial <= 0.0 {
        return Err(format!(
            "bilateral: sigma_spatial must be > 0, got {}",
            config.sigma_spatial
        ));
    }
    if config.sigma_range <= 0.0 {
        return Err(format!(
            "bilateral: sigma_range must be > 0, got {}",
            config.sigma_range
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Convenience free-function API (RGB u8)
// ---------------------------------------------------------------------------

/// Apply bilateral filter to an RGB u8 image.
///
/// Each of the three colour channels is filtered independently.  The result is
/// returned as a new `Vec<u8>` with the same length as `img` (i.e.
/// `3 * w * h`).  Pixel layout is interleaved RGB: `[R0,G0,B0, R1,G1,B1, …]`.
///
/// # Parameters
/// - `img`     – input RGB pixel buffer, length `3 * w * h`.
/// - `w`       – image width in pixels.
/// - `h`       – image height in pixels.
/// - `sigma_s` – spatial Gaussian sigma (pixels).
/// - `sigma_r` – range Gaussian sigma (normalised 0–1).
///
/// # Panics
/// Panics if `img.len() != 3 * w * h` or if either sigma is non-positive.
#[allow(clippy::cast_precision_loss)]
#[must_use]
pub fn bilateral_filter(img: &[u8], w: u32, h: u32, sigma_s: f32, sigma_r: f32) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    let n = w * h;
    assert_eq!(
        img.len(),
        3 * n,
        "bilateral_filter: img length must equal 3 * w * h"
    );

    let cfg = BilateralConfig::new(sigma_s, sigma_r);
    let two_ss2 = 2.0 * cfg.sigma_spatial * cfg.sigma_spatial;
    let two_sr2 = 2.0 * cfg.sigma_range * cfg.sigma_range;
    let r = cfg.radius as isize;

    let mut out = vec![0u8; 3 * n];

    for channel in 0..3usize {
        // Extract channel to f32 normalised [0, 1]
        let chan_src: Vec<f32> = (0..n)
            .map(|i| img[i * 3 + channel] as f32 / 255.0)
            .collect();

        let mut chan_dst = vec![0.0_f32; n];
        for cy in 0..h {
            for cx in 0..w {
                let center_val = chan_src[cy * w + cx];
                let mut weight_sum = 0.0_f32;
                let mut value_sum = 0.0_f32;

                let y_lo = (cy as isize - r).max(0) as usize;
                let y_hi = (cy as isize + r).min(h as isize - 1) as usize;
                let x_lo = (cx as isize - r).max(0) as usize;
                let x_hi = (cx as isize + r).min(w as isize - 1) as usize;

                for ny in y_lo..=y_hi {
                    let dy = (ny as isize - cy as isize) as f32;
                    for nx in x_lo..=x_hi {
                        let dx = (nx as isize - cx as isize) as f32;
                        let neighbor_val = chan_src[ny * w + nx];

                        let spatial_w = (-(dx * dx + dy * dy) / two_ss2).exp();
                        let range_diff = center_val - neighbor_val;
                        let range_w = (-(range_diff * range_diff) / two_sr2).exp();
                        let wt = spatial_w * range_w;

                        weight_sum += wt;
                        value_sum += wt * neighbor_val;
                    }
                }

                chan_dst[cy * w + cx] = if weight_sum > f32::EPSILON {
                    value_sum / weight_sum
                } else {
                    center_val
                };
            }
        }

        // Write back
        for i in 0..n {
            out[i * 3 + channel] = (chan_dst[i] * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    out
}

// ---------------------------------------------------------------------------
// RGBA8 in-place bilateral filter
// ---------------------------------------------------------------------------

/// Apply bilateral filter to an RGBA8 image buffer (in-place).
///
/// - `sigma_spatial`: spatial Gaussian sigma (controls blur radius, typical: 3–10)
/// - `sigma_range`: range Gaussian sigma (controls edge sensitivity, typical: 25–75 for u8)
/// - The spatial kernel half-size = ceil(2 * sigma_spatial).
///
/// Alpha channel is left unchanged. RGB channels are filtered independently.
///
/// This is a convenience wrapper around the config-based API that uses u8 pixel
/// values directly (sigma_range in 0–255 range rather than 0–1).
pub fn bilateral_filter_rgba8_inplace(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    sigma_spatial: f32,
    sigma_range: f32,
) {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h * 4 {
        return;
    }
    if sigma_spatial <= 0.0 {
        return;
    }

    let kernel_radius = (2.0 * sigma_spatial).ceil() as isize;
    let spatial_denom = 2.0_f64 * (sigma_spatial as f64) * (sigma_spatial as f64);
    let range_denom = 2.0_f64 * (sigma_range as f64) * (sigma_range as f64);

    let src = pixels.to_vec();

    // Process rows in parallel
    let row_results: Vec<Vec<u8>> = map_range(h, |y| {
        let mut row_buf = vec![0u8; w * 4];
        for x in 0..w {
            let base = (y * w + x) * 4;
            // Copy alpha unchanged
            row_buf[x * 4 + 3] = src[base + 3];

            // Process R, G, B channels
            for ch in 0..3 {
                let center_val = src[base + ch] as f64;
                let mut weighted_sum = 0.0_f64;
                let mut total_weight = 0.0_f64;

                let y_lo = (y as isize - kernel_radius).max(0) as usize;
                let y_hi = (y as isize + kernel_radius).min(h as isize - 1) as usize;
                let x_lo = (x as isize - kernel_radius).max(0) as usize;
                let x_hi = (x as isize + kernel_radius).min(w as isize - 1) as usize;

                for ny in y_lo..=y_hi {
                    let dy = ny as f64 - y as f64;
                    for nx in x_lo..=x_hi {
                        let dx = nx as f64 - x as f64;
                        let neighbor_idx = (ny * w + nx) * 4 + ch;
                        let neighbor_val = src[neighbor_idx] as f64;

                        let spatial_w = (-(dx * dx + dy * dy) / spatial_denom).exp();

                        let diff = center_val - neighbor_val;
                        let range_w = if range_denom > 0.0 {
                            (-(diff * diff) / range_denom).exp()
                        } else if diff.abs() < 0.5 {
                            1.0
                        } else {
                            0.0
                        };

                        let w_combined = spatial_w * range_w;
                        weighted_sum += w_combined * neighbor_val;
                        total_weight += w_combined;
                    }
                }

                let result = if total_weight > 0.0 {
                    (weighted_sum / total_weight).clamp(0.0, 255.0).round() as u8
                } else {
                    src[base + ch]
                };
                row_buf[x * 4 + ch] = result;
            }
        }
        row_buf
    });

    // Write back results
    for (y, row) in row_results.into_iter().enumerate() {
        let start = y * w * 4;
        pixels[start..start + w * 4].copy_from_slice(&row);
    }
}

/// Apply bilateral filter to a grayscale (single-channel) u8 buffer (in-place).
///
/// - `sigma_spatial`: spatial Gaussian sigma (controls blur radius)
/// - `sigma_range`: range Gaussian sigma (controls edge sensitivity, in u8 range 0–255)
pub fn bilateral_filter_gray_u8_inplace(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    sigma_spatial: f32,
    sigma_range: f32,
) {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 || pixels.len() < w * h {
        return;
    }
    if sigma_spatial <= 0.0 {
        return;
    }

    let kernel_radius = (2.0 * sigma_spatial).ceil() as isize;
    let spatial_denom = 2.0_f64 * (sigma_spatial as f64) * (sigma_spatial as f64);
    let range_denom = 2.0_f64 * (sigma_range as f64) * (sigma_range as f64);

    let src = pixels.to_vec();

    let row_results: Vec<Vec<u8>> = map_range(h, |y| {
        let mut row_buf = vec![0u8; w];
        for x in 0..w {
            let center_val = src[y * w + x] as f64;
            let mut weighted_sum = 0.0_f64;
            let mut total_weight = 0.0_f64;

            let y_lo = (y as isize - kernel_radius).max(0) as usize;
            let y_hi = (y as isize + kernel_radius).min(h as isize - 1) as usize;
            let x_lo = (x as isize - kernel_radius).max(0) as usize;
            let x_hi = (x as isize + kernel_radius).min(w as isize - 1) as usize;

            for ny in y_lo..=y_hi {
                let dy = ny as f64 - y as f64;
                for nx in x_lo..=x_hi {
                    let dx = nx as f64 - x as f64;
                    let neighbor_val = src[ny * w + nx] as f64;

                    let spatial_w = (-(dx * dx + dy * dy) / spatial_denom).exp();

                    let diff = center_val - neighbor_val;
                    let range_w = if range_denom > 0.0 {
                        (-(diff * diff) / range_denom).exp()
                    } else if diff.abs() < 0.5 {
                        1.0
                    } else {
                        0.0
                    };

                    let w_combined = spatial_w * range_w;
                    weighted_sum += w_combined * neighbor_val;
                    total_weight += w_combined;
                }
            }

            row_buf[x] = if total_weight > 0.0 {
                (weighted_sum / total_weight).clamp(0.0, 255.0).round() as u8
            } else {
                src[y * w + x]
            };
        }
        row_buf
    });

    for (y, row) in row_results.into_iter().enumerate() {
        let start = y * w;
        pixels[start..start + w].copy_from_slice(&row);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: create a uniform grayscale buffer.
    fn uniform_gray(size: usize, value: f32) -> Vec<f32> {
        vec![value; size]
    }

    #[test]
    fn default_config_fields() {
        let cfg = BilateralConfig::default();
        assert!((cfg.sigma_spatial - 5.0).abs() < 1e-6);
        assert!((cfg.sigma_range - 0.15).abs() < 1e-6);
        assert!(cfg.radius >= 1);
    }

    #[test]
    fn new_config_radius_formula() {
        // radius = ceil(3 * sigma_spatial) clamped to >= 1
        let cfg = BilateralConfig::new(2.0, 0.1);
        assert_eq!(cfg.radius, 6); // ceil(6.0) = 6
        let cfg2 = BilateralConfig::new(0.1, 0.1);
        assert_eq!(cfg2.radius, 1); // ceil(0.3)=1
    }

    #[test]
    fn fine_config_sets_explicit_radius() {
        let cfg = BilateralConfig::fine(5.0, 0.15, 3);
        assert_eq!(cfg.radius, 3);
    }

    #[test]
    fn uniform_image_unchanged_with_tight_range() {
        // A perfectly uniform image should not change regardless of parameters
        let w = 8;
        let h = 8;
        let src = uniform_gray(w * h, 0.5);
        let mut dst = vec![0.0_f32; w * h];
        let cfg = BilateralConfig::new(3.0, 0.1);
        bilateral_filter_gray(&src, &mut dst, w, h, &cfg).expect("bilateral should succeed");
        for (i, &v) in dst.iter().enumerate() {
            assert!((v - 0.5).abs() < 1e-4, "pixel {i}: expected 0.5 got {v}");
        }
    }

    #[test]
    fn large_sigma_range_behaves_like_gaussian_blur() {
        // With huge sigma_range every pixel has nearly equal range weight
        // → should still produce a finite result and output same size
        let w = 5;
        let h = 5;
        let src: Vec<f32> = (0..w * h)
            .map(|i| (i as f32) / (w * h - 1) as f32)
            .collect();
        let mut dst = vec![0.0_f32; w * h];
        let cfg = BilateralConfig::new(2.0, 1000.0);
        bilateral_filter_gray(&src, &mut dst, w, h, &cfg).expect("bilateral should succeed");
        assert_eq!(dst.len(), w * h);
        assert!(dst.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn edge_preservation_high_contrast() {
        // Two half-planes: left = 0.0, right = 1.0
        // With tight sigma_range, the edge should be preserved (no significant blur)
        let w = 10;
        let h = 5;
        let mut src = vec![0.0_f32; w * h];
        for y in 0..h {
            for x in 5..w {
                src[y * w + x] = 1.0;
            }
        }
        let mut dst = vec![0.0_f32; w * h];
        let cfg = BilateralConfig::fine(3.0, 0.05, 3);
        bilateral_filter_gray(&src, &mut dst, w, h, &cfg).expect("bilateral should succeed");

        // Left-of-centre pixel should stay dark
        let left_val = dst[2 * w + 2];
        assert!(
            left_val < 0.15,
            "left pixel expected < 0.15, got {left_val}"
        );
        // Right-of-centre pixel should stay bright
        let right_val = dst[2 * w + 7];
        assert!(
            right_val > 0.85,
            "right pixel expected > 0.85, got {right_val}"
        );
    }

    #[test]
    fn wrong_src_size_returns_err() {
        let mut dst = vec![0.0_f32; 25];
        let cfg = BilateralConfig::default();
        let result = bilateral_filter_gray(&[0.0; 10], &mut dst, 5, 5, &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn wrong_dst_size_returns_err() {
        let src = vec![0.0_f32; 25];
        let mut dst = vec![0.0_f32; 10];
        let cfg = BilateralConfig::default();
        let result = bilateral_filter_gray(&src, &mut dst, 5, 5, &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn rgba_alpha_channel_unchanged() {
        let w = 4;
        let h = 4;
        let n = w * h * 4;
        // src: all channels 128, alpha alternates 0/255
        let mut src = vec![128u8; n];
        for i in 0..w * h {
            src[i * 4 + 3] = if i % 2 == 0 { 0 } else { 255 };
        }
        let mut dst = vec![0u8; n];
        let cfg = BilateralConfig::new(2.0, 0.1);
        bilateral_filter_rgba(&src, &mut dst, w, h, &cfg).expect("rgba bilateral should succeed");

        for i in 0..w * h {
            let expected_alpha = if i % 2 == 0 { 0 } else { 255 };
            assert_eq!(
                dst[i * 4 + 3],
                expected_alpha,
                "alpha mismatch at pixel {i}"
            );
        }
    }

    #[test]
    fn rgba_wrong_buffer_size_returns_err() {
        let src = vec![0u8; 10];
        let mut dst = vec![0u8; 64];
        let cfg = BilateralConfig::default();
        let result = bilateral_filter_rgba(&src, &mut dst, 4, 4, &cfg);
        assert!(result.is_err());
    }

    #[test]
    fn bilateral_filter_returns_correct_length() {
        let w = 8u32;
        let h = 6u32;
        let img = vec![128u8; (3 * w * h) as usize];
        let out = bilateral_filter(&img, w, h, 2.0, 0.15);
        assert_eq!(out.len(), img.len());
    }

    #[test]
    fn bilateral_filter_uniform_image_unchanged() {
        let w = 6u32;
        let h = 6u32;
        let img = vec![100u8; (3 * w * h) as usize];
        let out = bilateral_filter(&img, w, h, 3.0, 0.1);
        // Uniform input should stay uniform
        for &v in &out {
            assert!((v as i32 - 100).abs() <= 1, "expected ~100 but got {v}");
        }
    }

    #[test]
    fn bilateral_filter_all_values_in_range() {
        let w = 10u32;
        let h = 10u32;
        let img: Vec<u8> = (0..(3 * w * h) as usize).map(|i| (i % 256) as u8).collect();
        let out = bilateral_filter(&img, w, h, 2.0, 0.2);
        // Verify output has the same length as input
        assert_eq!(out.len(), img.len());
    }

    // ---- RGBA8 in-place / u8 gray in-place tests ----

    #[test]
    fn test_bilateral_uniform_unchanged() {
        let mut pixels = vec![128u8; 16 * 16 * 4];
        for i in (3..pixels.len()).step_by(4) {
            pixels[i] = 255;
        }
        let original = pixels.clone();
        bilateral_filter_rgba8_inplace(&mut pixels, 16, 16, 5.0, 50.0);
        assert_eq!(pixels, original);
    }

    #[test]
    fn test_bilateral_preserves_sharp_edge() {
        // 20×20 image: left half black, right half white
        let w = 20u32;
        let h = 20u32;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let idx = ((y * w + x) * 4) as usize;
                let val = if x < w / 2 { 0u8 } else { 255u8 };
                pixels[idx] = val;
                pixels[idx + 1] = val;
                pixels[idx + 2] = val;
                pixels[idx + 3] = 255;
            }
        }
        // Small sigma_range: only very similar colours get blended, so edge is preserved
        bilateral_filter_rgba8_inplace(&mut pixels, w, h, 3.0, 10.0);

        let left_idx = (10 * w + 9) as usize * 4;
        let right_idx = (10 * w + 10) as usize * 4;
        let left_val = pixels[left_idx] as i32;
        let right_val = pixels[right_idx] as i32;
        let diff = (right_val - left_val).abs();
        assert!(
            diff > 200,
            "Edge not preserved: left={left_val}, right={right_val}"
        );
    }

    #[test]
    fn test_bilateral_smooths_noise() {
        let w = 10u32;
        let h = 10u32;
        let mut pixels = vec![128u8; (w * h * 4) as usize];
        for i in (3..pixels.len()).step_by(4) {
            pixels[i] = 255;
        }
        // Add noise to centre pixel
        let center = ((5 * w + 5) * 4) as usize;
        pixels[center] = 200;
        pixels[center + 1] = 200;
        pixels[center + 2] = 200;

        bilateral_filter_rgba8_inplace(&mut pixels, w, h, 3.0, 75.0);

        let smoothed_r = pixels[center];
        assert!(
            (smoothed_r as i32 - 128).abs() < (200i32 - 128).abs(),
            "Noise not smoothed: got {smoothed_r}"
        );
    }

    #[test]
    fn test_bilateral_alpha_preserved() {
        let w = 8u32;
        let h = 8u32;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        for i in 0..(w * h) as usize {
            pixels[i * 4 + 3] = (i % 256) as u8;
        }
        let alphas: Vec<u8> = (0..(w * h) as usize).map(|i| pixels[i * 4 + 3]).collect();

        bilateral_filter_rgba8_inplace(&mut pixels, w, h, 3.0, 50.0);

        for i in 0..(w * h) as usize {
            assert_eq!(pixels[i * 4 + 3], alphas[i], "Alpha changed at pixel {i}");
        }
    }

    #[test]
    fn test_bilateral_zero_sigma_unchanged() {
        let mut pixels = vec![100u8; 4 * 4 * 4];
        for i in (3..pixels.len()).step_by(4) {
            pixels[i] = 255;
        }
        pixels[0] = 50;
        pixels[4] = 200;
        let original = pixels.clone();
        bilateral_filter_rgba8_inplace(&mut pixels, 4, 4, 0.0, 50.0);
        assert_eq!(pixels, original);
    }

    #[test]
    fn test_bilateral_no_panic_small() {
        let mut pixels = vec![128u8, 64, 32, 255];
        bilateral_filter_rgba8_inplace(&mut pixels, 1, 1, 5.0, 50.0);
        assert_eq!(pixels[3], 255);
    }

    #[test]
    fn test_bilateral_gray() {
        let w = 10u32;
        let h = 10u32;
        let mut pixels = vec![128u8; (w * h) as usize];
        pixels[55] = 200;

        bilateral_filter_gray_u8_inplace(&mut pixels, w, h, 3.0, 75.0);

        assert!(
            (pixels[55] as i32 - 128).abs() < (200i32 - 128).abs(),
            "Gray noise not smoothed: got {}",
            pixels[55]
        );
    }
}
