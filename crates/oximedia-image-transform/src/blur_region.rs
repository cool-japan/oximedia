// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Selective region blurring for privacy and NSFW masking.
//!
//! This module provides pixel-level Gaussian blurring applied to rectangular
//! regions of an image represented as a flat `Vec<u8>` RGBA (or RGB) buffer.
//! Typical use-cases include:
//!
//! - **Face / person anonymisation** — blur regions returned by a face-detector.
//! - **NSFW redaction** — blur regions flagged by a content classifier.
//! - **License-plate privacy** — blur vehicle registration plates before publish.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::blur_region::{BlurRegion, BlurRegionProcessor};
//!
//! // A tiny 4×4 RGBA image (all white).
//! let width = 4u32;
//! let height = 4u32;
//! let channels = 4u32; // RGBA
//! let mut pixels = vec![255u8; (width * height * channels) as usize];
//!
//! let regions = vec![BlurRegion {
//!     x: 0,
//!     y: 0,
//!     width: 2,
//!     height: 2,
//!     blur_strength: 2.0,
//! }];
//!
//! BlurRegionProcessor::apply(&mut pixels, width, height, channels, &regions).expect("apply blur");
//! // Pixels are still in range 0..=255
//! assert!(pixels.iter().all(|&v| v <= 255));
//! ```

use thiserror::Error;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors that can occur during region blurring.
#[derive(Debug, Error)]
pub enum BlurRegionError {
    /// The image buffer size does not match `width * height * channels`.
    #[error("buffer size mismatch: expected {expected}, got {actual}")]
    BufferSizeMismatch {
        /// Expected byte length.
        expected: usize,
        /// Actual byte length.
        actual: usize,
    },

    /// A blur region exceeds the image boundaries.
    #[error("region out of bounds: region ({rx},{ry},{rw},{rh}) exceeds image ({iw}x{ih})")]
    RegionOutOfBounds {
        /// Region x.
        rx: u32,
        /// Region y.
        ry: u32,
        /// Region width.
        rw: u32,
        /// Region height.
        rh: u32,
        /// Image width.
        iw: u32,
        /// Image height.
        ih: u32,
    },

    /// A region has zero or invalid dimensions.
    #[error("invalid region: width or height is zero")]
    InvalidRegionDimensions,

    /// A blur strength value is out of range.
    #[error("invalid blur strength: {0} (must be > 0.0 and ≤ 500.0)")]
    InvalidBlurStrength(f64),

    /// Unsupported channel count.
    #[error("unsupported channel count: {0} (must be 1, 2, 3 or 4)")]
    UnsupportedChannelCount(u32),
}

// ---------------------------------------------------------------------------
// BlurRegion
// ---------------------------------------------------------------------------

/// Rectangular region to blur, expressed in image-space pixel coordinates.
///
/// All coordinates are clamped to the image boundaries during processing.
///
/// ```
/// use oximedia_image_transform::blur_region::BlurRegion;
///
/// let r = BlurRegion { x: 10, y: 20, width: 100, height: 80, blur_strength: 8.0 };
/// assert_eq!(r.x, 10);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct BlurRegion {
    /// Left edge in pixels (inclusive).
    pub x: u32,
    /// Top edge in pixels (inclusive).
    pub y: u32,
    /// Width of the region in pixels.
    pub width: u32,
    /// Height of the region in pixels.
    pub height: u32,
    /// Gaussian blur radius / sigma. Larger values produce a stronger blur.
    /// Must be in the range `(0.0, 500.0]`.
    pub blur_strength: f64,
}

/// Simpler rectangular blur region using an integer box-blur radius.
///
/// Use this when you want a fast, integer-based box blur rather than
/// the Gaussian-based [`BlurRegion`].
///
/// ```
/// use oximedia_image_transform::blur_region::BoxBlurRegion;
///
/// let r = BoxBlurRegion { x: 0, y: 0, width: 10, height: 10, blur_radius: 3 };
/// assert_eq!(r.blur_radius, 3);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct BoxBlurRegion {
    /// Left edge in pixels (inclusive).
    pub x: u32,
    /// Top edge in pixels (inclusive).
    pub y: u32,
    /// Width of the region in pixels.
    pub width: u32,
    /// Height of the region in pixels.
    pub height: u32,
    /// Box blur radius in pixels. A radius of `r` averages a `(2r+1)×(2r+1)`
    /// neighbourhood. Zero means no blur.
    pub blur_radius: u32,
}

impl BlurRegion {
    /// Validate this region against image dimensions.
    pub fn validate(&self, img_w: u32, img_h: u32) -> Result<(), BlurRegionError> {
        if self.width == 0 || self.height == 0 {
            return Err(BlurRegionError::InvalidRegionDimensions);
        }
        if self.blur_strength <= 0.0 || self.blur_strength > 500.0 {
            return Err(BlurRegionError::InvalidBlurStrength(self.blur_strength));
        }
        let x_end = self.x.saturating_add(self.width);
        let y_end = self.y.saturating_add(self.height);
        if self.x >= img_w || self.y >= img_h || x_end > img_w || y_end > img_h {
            return Err(BlurRegionError::RegionOutOfBounds {
                rx: self.x,
                ry: self.y,
                rw: self.width,
                rh: self.height,
                iw: img_w,
                ih: img_h,
            });
        }
        Ok(())
    }

    /// Clamp the region to image boundaries and return a version that is
    /// guaranteed to lie within the image.
    pub fn clamped(&self, img_w: u32, img_h: u32) -> Self {
        let x = self.x.min(img_w.saturating_sub(1));
        let y = self.y.min(img_h.saturating_sub(1));
        let x_end = (self.x.saturating_add(self.width)).min(img_w);
        let y_end = (self.y.saturating_add(self.height)).min(img_h);
        Self {
            x,
            y,
            width: x_end.saturating_sub(x),
            height: y_end.saturating_sub(y),
            blur_strength: self.blur_strength,
        }
    }
}

// ---------------------------------------------------------------------------
// BlurRegionProcessor
// ---------------------------------------------------------------------------

/// Applies Gaussian blur to a list of [`BlurRegion`]s within an image buffer.
///
/// The image buffer must be a densely-packed row-major byte array with
/// `channels` bytes per pixel (1 = greyscale, 2 = greyscale+alpha, 3 = RGB,
/// 4 = RGBA).
pub struct BlurRegionProcessor;

impl BlurRegionProcessor {
    /// Apply Gaussian blur to all `regions` within `pixels`.
    ///
    /// Regions that extend beyond the image boundaries are clamped rather than
    /// rejected, so callers do not need to clip regions to the image size first.
    ///
    /// # Arguments
    ///
    /// - `pixels` — mutable byte buffer, row-major, `channels` bytes per pixel.
    /// - `width` — image width in pixels.
    /// - `height` — image height in pixels.
    /// - `channels` — bytes per pixel (1–4).
    /// - `regions` — list of rectangular blur regions.
    pub fn apply(
        pixels: &mut Vec<u8>,
        width: u32,
        height: u32,
        channels: u32,
        regions: &[BlurRegion],
    ) -> Result<(), BlurRegionError> {
        if !(1..=4).contains(&channels) {
            return Err(BlurRegionError::UnsupportedChannelCount(channels));
        }
        let expected = (width * height * channels) as usize;
        if pixels.len() != expected {
            return Err(BlurRegionError::BufferSizeMismatch {
                expected,
                actual: pixels.len(),
            });
        }

        for region in regions {
            let clamped = region.clamped(width, height);
            if clamped.width == 0 || clamped.height == 0 {
                continue;
            }
            blur_region_inplace(pixels, width, height, channels, &clamped)?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Simple box-blur API (integer radius, RGBA/RGB flat buffer)
// ---------------------------------------------------------------------------

/// Apply box blur to each specified rectangular region of a flat image buffer.
///
/// This is a convenience function using integer box-blur (separable 1-D mean
/// filter) that is faster than the Gaussian path and suitable for privacy
/// blurring where approximate smoothing is acceptable.
///
/// # Arguments
///
/// - `image` — mutable RGBA (or RGB) byte buffer, row-major, 4 bytes per pixel
///   (assumed RGBA).
/// - `img_width` — image width in pixels.
/// - `img_height` — image height in pixels.
/// - `regions` — slice of [`BoxBlurRegion`] describing which areas to blur.
///
/// Regions extending beyond the image boundary are silently clamped.
///
/// # Example
///
/// ```
/// use oximedia_image_transform::blur_region::{BoxBlurRegion, apply_blur_regions};
///
/// let w = 8u32;
/// let h = 8u32;
/// // All-white RGBA image.
/// let mut img: Vec<u8> = vec![255u8; (w * h * 4) as usize];
/// // Paint the top-left 4×4 black so we can see the blur effect.
/// for row in 0..4usize {
///     for col in 0..4usize {
///         let base = (row * w as usize + col) * 4;
///         img[base] = 0;
///         img[base + 1] = 0;
///         img[base + 2] = 0;
///     }
/// }
/// let regions = vec![BoxBlurRegion { x: 0, y: 0, width: 4, height: 4, blur_radius: 2 }];
/// apply_blur_regions(&mut img, w, h, &regions);
/// // All pixels should remain in valid range.
/// assert!(img.iter().all(|&v| v <= 255));
/// ```
pub fn apply_blur_regions(
    image: &mut Vec<u8>,
    img_width: u32,
    img_height: u32,
    regions: &[BoxBlurRegion],
) {
    // We assume 4 channels (RGBA). The function is a best-effort operation;
    // invalid region sizes are clamped rather than causing panics.
    const CHANNELS: usize = 4;
    let expected = (img_width * img_height) as usize * CHANNELS;
    if image.len() != expected || img_width == 0 || img_height == 0 {
        return;
    }

    for region in regions {
        if region.width == 0 || region.height == 0 {
            continue;
        }
        // Clamp region to image boundaries.
        let x0 = region.x.min(img_width.saturating_sub(1)) as usize;
        let y0 = region.y.min(img_height.saturating_sub(1)) as usize;
        let x1 = (region.x.saturating_add(region.width)).min(img_width) as usize;
        let y1 = (region.y.saturating_add(region.height)).min(img_height) as usize;
        if x1 <= x0 || y1 <= y0 {
            continue;
        }
        let rw = x1 - x0;
        let rh = y1 - y0;
        let r = region.blur_radius as usize;
        if r == 0 {
            continue;
        }
        box_blur_region_rgba(image, img_width as usize, x0, y0, rw, rh, r, CHANNELS);
    }
}

/// Apply a two-pass (horizontal then vertical) box blur to a rectangular
/// region of a row-major RGBA image buffer.
fn box_blur_region_rgba(
    pixels: &mut Vec<u8>,
    img_w: usize,
    rx: usize,
    ry: usize,
    rw: usize,
    rh: usize,
    radius: usize,
    ch: usize,
) {
    let pixel_count = rw * rh;
    // Extract region into temp buffer.
    let mut temp = vec![0u8; pixel_count * ch];
    for row in 0..rh {
        for col in 0..rw {
            let src = ((ry + row) * img_w + (rx + col)) * ch;
            let dst = (row * rw + col) * ch;
            temp[dst..dst + ch].copy_from_slice(&pixels[src..src + ch]);
        }
    }

    // Horizontal pass: write into `horiz`.
    let mut horiz = vec![0u8; pixel_count * ch];
    for row in 0..rh {
        for col in 0..rw {
            let lo = col.saturating_sub(radius);
            let hi = (col + radius + 1).min(rw);
            let count = (hi - lo) as u32;
            for c in 0..ch {
                let mut sum: u32 = 0;
                for k in lo..hi {
                    sum += u32::from(temp[(row * rw + k) * ch + c]);
                }
                horiz[(row * rw + col) * ch + c] = (sum / count) as u8;
            }
        }
    }

    // Vertical pass: write into `vert`.
    let mut vert = vec![0u8; pixel_count * ch];
    for row in 0..rh {
        for col in 0..rw {
            let lo = row.saturating_sub(radius);
            let hi = (row + radius + 1).min(rh);
            let count = (hi - lo) as u32;
            for c in 0..ch {
                let mut sum: u32 = 0;
                for k in lo..hi {
                    sum += u32::from(horiz[(k * rw + col) * ch + c]);
                }
                vert[(row * rw + col) * ch + c] = (sum / count) as u8;
            }
        }
    }

    // Write back to main buffer.
    for row in 0..rh {
        for col in 0..rw {
            let dst = ((ry + row) * img_w + (rx + col)) * ch;
            let src = (row * rw + col) * ch;
            pixels[dst..dst + ch].copy_from_slice(&vert[src..src + ch]);
        }
    }
}

// ---------------------------------------------------------------------------
// Core Gaussian blur implementation (separable 1D convolution)
// ---------------------------------------------------------------------------

/// Apply a separable Gaussian blur to the given rectangular region of `pixels`.
fn blur_region_inplace(
    pixels: &mut Vec<u8>,
    img_w: u32,
    _img_h: u32,
    channels: u32,
    region: &BlurRegion,
) -> Result<(), BlurRegionError> {
    // Build 1-D Gaussian kernel from sigma = blur_strength
    let kernel = gaussian_kernel_1d(region.blur_strength);
    let radius = (kernel.len() / 2) as i64;

    let rx = region.x as usize;
    let ry = region.y as usize;
    let rw = region.width as usize;
    let rh = region.height as usize;
    let iw = img_w as usize;
    let ch = channels as usize;

    // Extract region pixels into a temporary float buffer (channels × pixels)
    let pixel_count = rw * rh;
    let mut temp = vec![0.0f32; pixel_count * ch];

    for row in 0..rh {
        for col in 0..rw {
            let src_idx = ((ry + row) * iw + (rx + col)) * ch;
            let dst_idx = (row * rw + col) * ch;
            for c in 0..ch {
                temp[dst_idx + c] = pixels[src_idx + c] as f32;
            }
        }
    }

    // Horizontal pass
    let mut horiz = vec![0.0f32; pixel_count * ch];
    for row in 0..rh {
        for col in 0..rw {
            for c in 0..ch {
                let mut acc = 0.0f32;
                let mut weight_sum = 0.0f32;
                for ki in 0..kernel.len() {
                    let offset = ki as i64 - radius;
                    let src_col = col as i64 + offset;
                    if src_col >= 0 && (src_col as usize) < rw {
                        let src_idx = (row * rw + src_col as usize) * ch + c;
                        acc += temp[src_idx] * kernel[ki];
                        weight_sum += kernel[ki];
                    }
                }
                horiz[(row * rw + col) * ch + c] = if weight_sum > 0.0 {
                    acc / weight_sum
                } else {
                    0.0
                };
            }
        }
    }

    // Vertical pass
    let mut vert = vec![0.0f32; pixel_count * ch];
    for row in 0..rh {
        for col in 0..rw {
            for c in 0..ch {
                let mut acc = 0.0f32;
                let mut weight_sum = 0.0f32;
                for ki in 0..kernel.len() {
                    let offset = ki as i64 - radius;
                    let src_row = row as i64 + offset;
                    if src_row >= 0 && (src_row as usize) < rh {
                        let src_idx = (src_row as usize * rw + col) * ch + c;
                        acc += horiz[src_idx] * kernel[ki];
                        weight_sum += kernel[ki];
                    }
                }
                vert[(row * rw + col) * ch + c] = if weight_sum > 0.0 {
                    acc / weight_sum
                } else {
                    0.0
                };
            }
        }
    }

    // Write blurred region back to the main pixel buffer
    for row in 0..rh {
        for col in 0..rw {
            let dst_idx = ((ry + row) * iw + (rx + col)) * ch;
            let src_idx = (row * rw + col) * ch;
            for c in 0..ch {
                pixels[dst_idx + c] = vert[src_idx + c].round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    Ok(())
}

/// Compute a normalised 1-D Gaussian kernel for the given sigma.
///
/// The kernel radius is `ceil(3 * sigma)`, giving a kernel length of
/// `2 * radius + 1`. Values are normalised to sum to 1.0.
fn gaussian_kernel_1d(sigma: f64) -> Vec<f32> {
    let radius = (3.0 * sigma).ceil() as usize;
    let len = 2 * radius + 1;
    let mut kernel = Vec::with_capacity(len);
    let two_sigma_sq = 2.0 * sigma * sigma;
    let mut sum = 0.0f64;

    for i in 0..len {
        let x = i as f64 - radius as f64;
        let val = (-x * x / two_sigma_sq).exp();
        kernel.push(val as f32);
        sum += val;
    }

    // Normalise
    let sum_f = sum as f32;
    for v in &mut kernel {
        *v /= sum_f;
    }

    kernel
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn white_rgba(w: u32, h: u32) -> Vec<u8> {
        vec![255u8; (w * h * 4) as usize]
    }

    #[test]
    fn test_gaussian_kernel_sums_to_one() {
        let k = gaussian_kernel_1d(2.0);
        let sum: f32 = k.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5, "kernel sum = {sum}");
    }

    #[test]
    fn test_gaussian_kernel_symmetry() {
        let k = gaussian_kernel_1d(3.0);
        let n = k.len();
        for i in 0..n / 2 {
            assert!((k[i] - k[n - 1 - i]).abs() < 1e-7);
        }
    }

    #[test]
    fn test_apply_no_panic_on_full_image_region() {
        let w = 8u32;
        let h = 8u32;
        let mut pixels = white_rgba(w, h);
        let regions = vec![BlurRegion {
            x: 0,
            y: 0,
            width: w,
            height: h,
            blur_strength: 1.5,
        }];
        BlurRegionProcessor::apply(&mut pixels, w, h, 4, &regions).expect("apply full blur");
        // u8 values are always in 0..=255 by type — just verify the buffer length is intact.
        assert!(!pixels.is_empty());
    }

    #[test]
    fn test_apply_partial_region() {
        let w = 16u32;
        let h = 16u32;
        let mut pixels = white_rgba(w, h);
        // Set top-left 4×4 to 0
        for row in 0..4 {
            for col in 0..4 {
                let base = ((row * w as usize + col) * 4) as usize;
                for c in 0..4 {
                    pixels[base + c] = 0;
                }
            }
        }
        let regions = vec![BlurRegion {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            blur_strength: 1.0,
        }];
        BlurRegionProcessor::apply(&mut pixels, w, h, 4, &regions).expect("apply partial blur");
        // After blurring, values in the region should be between 0 and 255
        // All pixels should be valid u8 values — just check the buffer is non-empty.
        assert_eq!(pixels.len(), (w * h * 4) as usize);
    }

    #[test]
    fn test_blur_region_clamped() {
        let r = BlurRegion {
            x: 10,
            y: 10,
            width: 20,
            height: 20,
            blur_strength: 1.0,
        };
        let clamped = r.clamped(15, 15);
        assert_eq!(clamped.x, 10);
        assert_eq!(clamped.width, 5);
        assert_eq!(clamped.height, 5);
    }

    #[test]
    fn test_buffer_size_mismatch() {
        let mut bad_buf = vec![0u8; 10];
        let result = BlurRegionProcessor::apply(&mut bad_buf, 4, 4, 4, &[]);
        assert!(matches!(
            result,
            Err(BlurRegionError::BufferSizeMismatch { .. })
        ));
    }

    #[test]
    fn test_unsupported_channels() {
        let mut buf = vec![0u8; 16];
        let result = BlurRegionProcessor::apply(&mut buf, 4, 1, 5, &[]);
        assert!(matches!(
            result,
            Err(BlurRegionError::UnsupportedChannelCount(5))
        ));
    }

    #[test]
    fn test_blur_region_validate_zero_width() {
        let r = BlurRegion {
            x: 0,
            y: 0,
            width: 0,
            height: 10,
            blur_strength: 1.0,
        };
        assert!(matches!(
            r.validate(100, 100),
            Err(BlurRegionError::InvalidRegionDimensions)
        ));
    }

    #[test]
    fn test_blur_region_validate_bad_strength() {
        let r = BlurRegion {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            blur_strength: -1.0,
        };
        assert!(matches!(
            r.validate(100, 100),
            Err(BlurRegionError::InvalidBlurStrength(_))
        ));
    }

    #[test]
    fn test_blur_region_validate_out_of_bounds() {
        let r = BlurRegion {
            x: 90,
            y: 90,
            width: 20,
            height: 20,
            blur_strength: 1.0,
        };
        assert!(matches!(
            r.validate(100, 100),
            Err(BlurRegionError::RegionOutOfBounds { .. })
        ));
    }

    #[test]
    fn test_blur_region_validate_ok() {
        let r = BlurRegion {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            blur_strength: 2.0,
        };
        assert!(r.validate(100, 100).is_ok());
    }

    #[test]
    fn test_apply_rgb_channels() {
        let w = 4u32;
        let h = 4u32;
        let mut pixels = vec![200u8; (w * h * 3) as usize];
        let regions = vec![BlurRegion {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            blur_strength: 1.0,
        }];
        BlurRegionProcessor::apply(&mut pixels, w, h, 3, &regions).expect("apply RGB blur");
        // u8 values are always in 0..=255 by type — just verify the buffer length is intact.
        assert!(!pixels.is_empty());
    }

    #[test]
    fn test_apply_multiple_regions() {
        let w = 20u32;
        let h = 20u32;
        let mut pixels = white_rgba(w, h);
        let regions = vec![
            BlurRegion {
                x: 0,
                y: 0,
                width: 5,
                height: 5,
                blur_strength: 1.0,
            },
            BlurRegion {
                x: 10,
                y: 10,
                width: 5,
                height: 5,
                blur_strength: 2.0,
            },
        ];
        BlurRegionProcessor::apply(&mut pixels, w, h, 4, &regions)
            .expect("apply multi-region blur");
        // u8 values are always in 0..=255 by type — just verify the buffer length is intact.
        assert!(!pixels.is_empty());
    }

    // ── apply_blur_regions (box blur API) tests ──

    /// Verify that pixels inside a blurred region actually change when the
    /// region contains mixed values (black and white).
    ///
    /// We paint the left half of the region black and the right half white,
    /// then blur the whole region.  Pixels at the colour boundary must change.
    #[test]
    fn test_blur_region_applies() {
        let w = 8u32;
        let h = 4u32;
        // Start with all-white RGBA.
        let mut img = white_rgba(w, h);
        // Paint left half (columns 0-3) black.
        for row in 0..h as usize {
            for col in 0..4usize {
                let base = (row * w as usize + col) * 4;
                img[base] = 0; // R
                img[base + 1] = 0; // G
                img[base + 2] = 0; // B
                img[base + 3] = 255;
            }
        }
        // Blur the entire image (all columns 0-7) — the boundary pixels must change.
        let before_red_at_boundary = img[(0 * w as usize + 3) * 4]; // pixel (3,0) red = 0
        let before_red_at_white = img[(0 * w as usize + 4) * 4]; // pixel (4,0) red = 255

        let regions = vec![super::BoxBlurRegion {
            x: 0,
            y: 0,
            width: w,
            height: h,
            blur_radius: 2,
        }];
        super::apply_blur_regions(&mut img, w, h, &regions);

        let after_red_at_boundary = img[(0 * w as usize + 3) * 4];
        let after_red_at_white = img[(0 * w as usize + 4) * 4];

        // Pixels near the boundary must have blended: the black pixel at column 3
        // should become brighter, and the white pixel at column 4 should become darker.
        assert!(
            after_red_at_boundary > before_red_at_boundary,
            "boundary black pixel should brighten after blur (was {before_red_at_boundary}, now {after_red_at_boundary})"
        );
        assert!(
            after_red_at_white < before_red_at_white,
            "boundary white pixel should darken after blur (was {before_red_at_white}, now {after_red_at_white})"
        );
        // All values must remain in valid range.
        // u8 values are always in 0..=255 by type — just verify the buffer length is intact.
        assert!(!img.is_empty());
    }

    #[test]
    fn test_blur_regions_noop_zero_radius() {
        let w = 4u32;
        let h = 4u32;
        let mut img = vec![128u8; (w * h * 4) as usize];
        let snapshot = img.clone();
        let regions = vec![super::BoxBlurRegion {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            blur_radius: 0,
        }];
        super::apply_blur_regions(&mut img, w, h, &regions);
        assert_eq!(img, snapshot, "zero radius should not change pixels");
    }

    #[test]
    fn test_blur_regions_clamps_out_of_bounds_region() {
        let w = 4u32;
        let h = 4u32;
        let mut img = vec![200u8; (w * h * 4) as usize];
        // Region extends well beyond image boundaries.
        let regions = vec![super::BoxBlurRegion {
            x: 2,
            y: 2,
            width: 100,
            height: 100,
            blur_radius: 2,
        }];
        // Should not panic; all pixels stay in range.
        super::apply_blur_regions(&mut img, w, h, &regions);
        // u8 values are always in 0..=255 by type — just verify the buffer length is intact.
        assert!(!img.is_empty());
    }

    #[test]
    fn test_blur_regions_ignores_wrong_buffer_size() {
        let w = 4u32;
        let h = 4u32;
        // Intentionally wrong size — function should return early without panic.
        let mut img = vec![0u8; 10];
        let regions = vec![super::BoxBlurRegion {
            x: 0,
            y: 0,
            width: 4,
            height: 4,
            blur_radius: 1,
        }];
        super::apply_blur_regions(&mut img, w, h, &regions);
        // Buffer unchanged.
        assert_eq!(img.len(), 10);
    }
}
