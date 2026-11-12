// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Image analysis utilities: dominant colour extraction and BlurHash encoding.
//!
//! # Dominant colour — median-cut algorithm
//!
//! [`DominantColorExtractor`] implements the **median-cut** quantisation
//! algorithm to find the `n` most representative colours in an RGBA pixel
//! buffer.  The algorithm:
//!
//! 1. Collects all non-transparent pixels into a bucket.
//! 2. Repeatedly splits the bucket along the channel with the widest range.
//! 3. Returns the mean colour of each final bucket.
//!
//! # BlurHash
//!
//! [`BlurHashEncoder`] produces a compact, pure-Rust
//! [BlurHash](https://blurha.sh/) string from an RGBA pixel buffer.  The
//! hash can be decoded client-side to display a smooth placeholder while the
//! full image loads.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::image_analysis::{DominantColorExtractor, BlurHashEncoder};
//!
//! // A 2×2 RGBA image: red, green, blue, white pixels
//! let pixels = vec![
//!     255u8, 0, 0, 255,  // red
//!     0, 255, 0, 255,    // green
//!     0, 0, 255, 255,    // blue
//!     255, 255, 255, 255, // white
//! ];
//!
//! let colors = DominantColorExtractor::extract(&pixels, 2, 2, 4, 3).expect("extract colors");
//! assert_eq!(colors.len(), 3);
//!
//! let hash = BlurHashEncoder::encode(&pixels, 2, 2, 4, 4, 3).expect("encode blurhash");
//! assert!(!hash.is_empty());
//! ```

use thiserror::Error;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from image analysis operations.
#[derive(Debug, Error)]
pub enum ImageAnalysisError {
    /// Buffer size does not match `width * height * channels`.
    #[error("buffer size mismatch: expected {expected}, got {actual}")]
    BufferSizeMismatch {
        /// Expected bytes.
        expected: usize,
        /// Actual bytes.
        actual: usize,
    },

    /// Unsupported channel count (must be 3 or 4).
    #[error("unsupported channel count: {0} (must be 3 or 4)")]
    UnsupportedChannelCount(u32),

    /// Requested number of colours is out of range.
    #[error("invalid color count: {0} (must be 1–256)")]
    InvalidColorCount(usize),

    /// BlurHash component dimensions are out of range.
    #[error("invalid blurhash components: x={x_comp} y={y_comp} (must be 1–9)")]
    InvalidBlurHashComponents {
        /// X components.
        x_comp: u32,
        /// Y components.
        y_comp: u32,
    },

    /// Image is too small to analyse.
    #[error("image too small: {w}×{h} (minimum 1×1)")]
    ImageTooSmall {
        /// Width.
        w: u32,
        /// Height.
        h: u32,
    },
}

// ---------------------------------------------------------------------------
// RGB helper
// ---------------------------------------------------------------------------

/// A simple 8-bit RGB colour used in analysis results.
///
/// ```
/// use oximedia_image_transform::image_analysis::RgbColor;
///
/// let c = RgbColor { r: 255, g: 128, b: 0 };
/// assert_eq!(c.r, 255);
/// assert_eq!(c.to_hex(), "#ff8000");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbColor {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

impl RgbColor {
    /// Encode as a CSS hex string (`#rrggbb`).
    pub fn to_hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

// ---------------------------------------------------------------------------
// DominantColorExtractor — median-cut algorithm
// ---------------------------------------------------------------------------

/// Extracts dominant colours from a pixel buffer using the median-cut algorithm.
pub struct DominantColorExtractor;

impl DominantColorExtractor {
    /// Extract up to `num_colors` dominant colours from an RGBA or RGB pixel buffer.
    ///
    /// # Arguments
    ///
    /// - `pixels` — flat byte buffer, row-major, `channels` bytes per pixel.
    /// - `width` — image width.
    /// - `height` — image height.
    /// - `channels` — 3 (RGB) or 4 (RGBA).
    /// - `num_colors` — number of dominant colours to return (1–256).
    ///
    /// # Returns
    ///
    /// A `Vec<RgbColor>` of length `num_colors`, sorted by frequency (most
    /// dominant first). Transparent pixels (alpha < 128) are excluded from
    /// the analysis when `channels == 4`.
    pub fn extract(
        pixels: &[u8],
        width: u32,
        height: u32,
        channels: u32,
        num_colors: usize,
    ) -> Result<Vec<RgbColor>, ImageAnalysisError> {
        validate_inputs(pixels, width, height, channels)?;
        if num_colors == 0 || num_colors > 256 {
            return Err(ImageAnalysisError::InvalidColorCount(num_colors));
        }

        // Collect opaque pixels
        let raw_pixels = collect_opaque_pixels(pixels, channels);
        if raw_pixels.is_empty() {
            // All transparent — return black
            return Ok(vec![RgbColor { r: 0, g: 0, b: 0 }; 1]);
        }

        // Median-cut quantisation
        let buckets = median_cut(raw_pixels, num_colors);

        // Compute mean colour per bucket
        let mut colors: Vec<RgbColor> = buckets.iter().map(|bucket| mean_color(bucket)).collect();

        // Sort by bucket size descending (most dominant first)
        let mut bucket_sizes: Vec<(RgbColor, usize)> = buckets
            .iter()
            .zip(colors.iter())
            .map(|(b, &c)| (c, b.len()))
            .collect();
        bucket_sizes.sort_by(|a, b| b.1.cmp(&a.1));
        colors = bucket_sizes.into_iter().map(|(c, _)| c).collect();

        Ok(colors)
    }
}

/// Collect non-transparent pixels as `[r, g, b]` triplets.
fn collect_opaque_pixels(pixels: &[u8], channels: u32) -> Vec<[u8; 3]> {
    let ch = channels as usize;
    let mut result = Vec::with_capacity(pixels.len() / ch);
    let mut i = 0;
    while i + ch <= pixels.len() {
        let alpha = if ch == 4 { pixels[i + 3] } else { 255 };
        if alpha >= 128 {
            result.push([pixels[i], pixels[i + 1], pixels[i + 2]]);
        }
        i += ch;
    }
    result
}

/// Compute the mean colour of a bucket.
fn mean_color(bucket: &[[u8; 3]]) -> RgbColor {
    if bucket.is_empty() {
        return RgbColor { r: 0, g: 0, b: 0 };
    }
    let sum_r: u64 = bucket.iter().map(|p| p[0] as u64).sum();
    let sum_g: u64 = bucket.iter().map(|p| p[1] as u64).sum();
    let sum_b: u64 = bucket.iter().map(|p| p[2] as u64).sum();
    let n = bucket.len() as u64;
    RgbColor {
        r: (sum_r / n) as u8,
        g: (sum_g / n) as u8,
        b: (sum_b / n) as u8,
    }
}

/// Core median-cut algorithm.
///
/// Recursively splits the largest bucket along the widest colour channel until
/// `target_count` buckets are reached.
fn median_cut(mut pixels: Vec<[u8; 3]>, target_count: usize) -> Vec<Vec<[u8; 3]>> {
    let mut buckets: Vec<Vec<[u8; 3]>> = vec![std::mem::take(&mut pixels)];

    while buckets.len() < target_count {
        // Find the bucket with the greatest colour range
        let split_idx = buckets
            .iter()
            .enumerate()
            .max_by_key(|(_, b)| color_range(b))
            .map(|(i, _)| i);

        let Some(idx) = split_idx else { break };

        let bucket = buckets.remove(idx);
        if bucket.is_empty() {
            break;
        }

        // Find the channel with the widest range within this bucket
        let channel = widest_channel(&bucket);

        // Sort bucket by that channel and split at median
        let mut sorted = bucket;
        sorted.sort_unstable_by_key(|p| p[channel]);
        let mid = sorted.len() / 2;
        let right = sorted.split_off(mid);
        buckets.push(sorted);
        buckets.push(right);
    }

    buckets
}

/// Compute the total colour range of a bucket (max - min across all channels).
fn color_range(bucket: &[[u8; 3]]) -> u32 {
    if bucket.is_empty() {
        return 0;
    }
    let mut range = 0u32;
    for ch in 0..3 {
        let min = bucket.iter().map(|p| p[ch]).min().unwrap_or(0);
        let max = bucket.iter().map(|p| p[ch]).max().unwrap_or(0);
        range += (max - min) as u32;
    }
    range
}

/// Find the channel (0=R, 1=G, 2=B) with the widest value range.
fn widest_channel(bucket: &[[u8; 3]]) -> usize {
    let mut best_ch = 0;
    let mut best_range = 0u32;
    for ch in 0..3 {
        let min = bucket.iter().map(|p| p[ch]).min().unwrap_or(0);
        let max = bucket.iter().map(|p| p[ch]).max().unwrap_or(0);
        let range = (max - min) as u32;
        if range > best_range {
            best_range = range;
            best_ch = ch;
        }
    }
    best_ch
}

// ---------------------------------------------------------------------------
// BlurHashEncoder — pure-Rust BlurHash v1 encoding
// ---------------------------------------------------------------------------

/// Encodes a pixel buffer as a BlurHash string.
///
/// BlurHash is a compact (~20–30 character) representation of an image's
/// colour structure, suitable for use as a low-quality placeholder.
///
/// This implementation follows the [BlurHash specification v1](https://github.com/woltapp/blurhash/blob/master/Algorithm.md).
pub struct BlurHashEncoder;

impl BlurHashEncoder {
    /// Encode an RGBA or RGB image as a BlurHash string.
    ///
    /// # Arguments
    ///
    /// - `pixels` — flat byte buffer, row-major, `channels` bytes per pixel.
    /// - `width` — image width.
    /// - `height` — image height.
    /// - `channels` — 3 (RGB) or 4 (RGBA).
    /// - `x_components` — number of DCT components in X direction (1–9).
    /// - `y_components` — number of DCT components in Y direction (1–9).
    ///
    /// # Returns
    ///
    /// A BlurHash string that can be decoded by any compliant BlurHash library.
    pub fn encode(
        pixels: &[u8],
        width: u32,
        height: u32,
        channels: u32,
        x_components: u32,
        y_components: u32,
    ) -> Result<String, ImageAnalysisError> {
        validate_inputs(pixels, width, height, channels)?;
        if x_components < 1 || x_components > 9 || y_components < 1 || y_components > 9 {
            return Err(ImageAnalysisError::InvalidBlurHashComponents {
                x_comp: x_components,
                y_comp: y_components,
            });
        }

        let w = width as usize;
        let h = height as usize;
        let xc = x_components as usize;
        let yc = y_components as usize;
        let ch = channels as usize;

        // Compute DCT components
        let mut components: Vec<[f64; 3]> = Vec::with_capacity(xc * yc);
        for j in 0..yc {
            for i in 0..xc {
                let norm = if i == 0 && j == 0 { 1.0 } else { 2.0 };
                let mut r = 0.0f64;
                let mut g = 0.0f64;
                let mut b = 0.0f64;
                for y in 0..h {
                    for x in 0..w {
                        let basis = norm
                            * (std::f64::consts::PI * i as f64 * x as f64 / w as f64).cos()
                            * (std::f64::consts::PI * j as f64 * y as f64 / h as f64).cos();
                        let pix = &pixels[(y * w + x) * ch..];
                        r += basis * srgb_to_linear(pix[0]);
                        g += basis * srgb_to_linear(pix[1]);
                        b += basis * srgb_to_linear(pix[2]);
                    }
                }
                let scale = 1.0 / (w * h) as f64;
                components.push([r * scale, g * scale, b * scale]);
            }
        }

        // Encode using BlurHash base83 encoding
        let mut hash = String::new();

        // Size flag: (x_components - 1) + (y_components - 1) * 9
        let size_flag = (xc - 1) + (yc - 1) * 9;
        encode_base83(size_flag as u64, 1, &mut hash);

        // Quantised maximum AC component value
        let max_ac_value: f64;
        if xc * yc > 1 {
            let actual_max = components[1..]
                .iter()
                .flat_map(|c| c.iter())
                .map(|&v| v.abs())
                .fold(0.0f64, f64::max);
            let quantised_max = (actual_max * 166.0 - 0.5).floor().clamp(0.0, 82.0) as u64;
            max_ac_value = (quantised_max + 1) as f64 / 166.0;
            encode_base83(quantised_max, 1, &mut hash);
        } else {
            max_ac_value = 1.0;
            encode_base83(0, 1, &mut hash);
        }

        // DC component
        encode_base83(encode_dc(&components[0]), 4, &mut hash);

        // AC components
        for component in components.iter().skip(1) {
            encode_base83(encode_ac(component, max_ac_value), 2, &mut hash);
        }

        Ok(hash)
    }
}

// ---------------------------------------------------------------------------
// BlurHash internal helpers
// ---------------------------------------------------------------------------

/// Convert sRGB byte to linear light value.
fn srgb_to_linear(value: u8) -> f64 {
    let v = value as f64 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert linear light value back to sRGB byte.
fn linear_to_srgb(value: f64) -> u64 {
    let v = value.clamp(0.0, 1.0);
    let out = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (out * 255.0 + 0.5).floor() as u64
}

/// Encode a DC component as a packed RGB integer.
fn encode_dc(component: &[f64; 3]) -> u64 {
    let r = linear_to_srgb(component[0]);
    let g = linear_to_srgb(component[1]);
    let b = linear_to_srgb(component[2]);
    (r << 16) | (g << 8) | b
}

/// Encode an AC component into a quantised integer.
fn encode_ac(component: &[f64; 3], max_ac: f64) -> u64 {
    let quant_r = sign_pow(component[0] / max_ac, 0.5);
    let quant_g = sign_pow(component[1] / max_ac, 0.5);
    let quant_b = sign_pow(component[2] / max_ac, 0.5);
    let qr = ((quant_r * 9.0 + 9.5).floor() as u64).min(18);
    let qg = ((quant_g * 9.0 + 9.5).floor() as u64).min(18);
    let qb = ((quant_b * 9.0 + 9.5).floor() as u64).min(18);
    qr * 19 * 19 + qg * 19 + qb
}

/// Sign-preserving power function.
fn sign_pow(value: f64, exp: f64) -> f64 {
    value.abs().powf(exp).copysign(value)
}

/// Base83 character table (same as original BlurHash spec).
const BASE83_CHARS: &[u8] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~";

/// Encode `value` in base83, writing exactly `length` characters.
fn encode_base83(value: u64, length: usize, out: &mut String) {
    for i in (0..length).rev() {
        let digit = (value / 83u64.pow(i as u32)) % 83;
        out.push(BASE83_CHARS[digit as usize] as char);
    }
}

// ---------------------------------------------------------------------------
// Shared validation
// ---------------------------------------------------------------------------

fn validate_inputs(
    pixels: &[u8],
    width: u32,
    height: u32,
    channels: u32,
) -> Result<(), ImageAnalysisError> {
    if width == 0 || height == 0 {
        return Err(ImageAnalysisError::ImageTooSmall {
            w: width,
            h: height,
        });
    }
    if channels != 3 && channels != 4 {
        return Err(ImageAnalysisError::UnsupportedChannelCount(channels));
    }
    let expected = (width * height * channels) as usize;
    if pixels.len() != expected {
        return Err(ImageAnalysisError::BufferSizeMismatch {
            expected,
            actual: pixels.len(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rgba(pixels: &[(u8, u8, u8, u8)]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(pixels.len() * 4);
        for &(r, g, b, a) in pixels {
            buf.extend_from_slice(&[r, g, b, a]);
        }
        buf
    }

    fn solid_rgba(r: u8, g: u8, b: u8, w: u32, h: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..(w * h) {
            buf.extend_from_slice(&[r, g, b, 255]);
        }
        buf
    }

    // ── DominantColorExtractor ──

    #[test]
    fn test_extract_single_color() {
        let pixels = solid_rgba(200, 100, 50, 4, 4);
        let colors =
            DominantColorExtractor::extract(&pixels, 4, 4, 4, 1).expect("extract single color");
        assert_eq!(colors.len(), 1);
        let c = colors[0];
        assert_eq!(c.r, 200);
        assert_eq!(c.g, 100);
        assert_eq!(c.b, 50);
    }

    #[test]
    fn test_extract_multiple_colors() {
        let pixels = make_rgba(&[
            (255, 0, 0, 255),
            (0, 255, 0, 255),
            (0, 0, 255, 255),
            (255, 255, 0, 255),
        ]);
        let colors =
            DominantColorExtractor::extract(&pixels, 4, 1, 4, 3).expect("extract multiple colors");
        assert_eq!(colors.len(), 3);
    }

    #[test]
    fn test_extract_ignores_transparent_pixels() {
        let pixels = make_rgba(&[
            (255, 0, 0, 255),
            (0, 255, 0, 0), // fully transparent — should be ignored
            (0, 0, 255, 255),
            (255, 255, 0, 255),
        ]);
        let colors = DominantColorExtractor::extract(&pixels, 4, 1, 4, 2)
            .expect("extract ignoring transparent");
        assert!(!colors.is_empty());
    }

    #[test]
    fn test_extract_invalid_color_count() {
        let pixels = solid_rgba(0, 0, 0, 2, 2);
        assert!(matches!(
            DominantColorExtractor::extract(&pixels, 2, 2, 4, 0),
            Err(ImageAnalysisError::InvalidColorCount(0))
        ));
        assert!(matches!(
            DominantColorExtractor::extract(&pixels, 2, 2, 4, 257),
            Err(ImageAnalysisError::InvalidColorCount(257))
        ));
    }

    #[test]
    fn test_extract_unsupported_channels() {
        // Channel count 5 is unsupported — must return UnsupportedChannelCount.
        let pixels = vec![0u8; 20]; // 2×2×5
        assert!(matches!(
            DominantColorExtractor::extract(&pixels, 2, 2, 5, 1),
            Err(ImageAnalysisError::UnsupportedChannelCount(5))
        ));
    }

    #[test]
    fn test_rgb_color_to_hex() {
        let c = RgbColor {
            r: 255,
            g: 128,
            b: 0,
        };
        assert_eq!(c.to_hex(), "#ff8000");
    }

    #[test]
    fn test_extract_rgb_channels() {
        // 3-channel RGB
        let pixels: Vec<u8> = vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 128, 128];
        let colors =
            DominantColorExtractor::extract(&pixels, 4, 1, 3, 2).expect("extract RGB colors");
        assert!(!colors.is_empty());
    }

    // ── BlurHashEncoder ──

    #[test]
    fn test_blurhash_non_empty() {
        let pixels = solid_rgba(100, 150, 200, 8, 8);
        let hash = BlurHashEncoder::encode(&pixels, 8, 8, 4, 4, 3).expect("encode blurhash");
        assert!(!hash.is_empty());
    }

    #[test]
    fn test_blurhash_deterministic() {
        let pixels = solid_rgba(80, 120, 160, 6, 6);
        let h1 = BlurHashEncoder::encode(&pixels, 6, 6, 4, 4, 3).expect("encode h1");
        let h2 = BlurHashEncoder::encode(&pixels, 6, 6, 4, 3, 4).expect("encode h2");
        // Different components → different hashes
        assert_ne!(h1, h2);
        // Same inputs → same hash
        let h3 = BlurHashEncoder::encode(&pixels, 6, 6, 4, 4, 3).expect("encode h3");
        assert_eq!(h1, h3);
    }

    #[test]
    fn test_blurhash_length_varies_with_components() {
        let pixels = solid_rgba(100, 100, 100, 8, 8);
        let h1 = BlurHashEncoder::encode(&pixels, 8, 8, 4, 1, 1).expect("encode 1x1 components");
        let h4 = BlurHashEncoder::encode(&pixels, 8, 8, 4, 4, 4).expect("encode 4x4 components");
        // More components → longer hash
        assert!(h4.len() > h1.len(), "h4={} h1={}", h4.len(), h1.len());
    }

    #[test]
    fn test_blurhash_invalid_components() {
        let pixels = solid_rgba(100, 100, 100, 4, 4);
        assert!(matches!(
            BlurHashEncoder::encode(&pixels, 4, 4, 4, 0, 3),
            Err(ImageAnalysisError::InvalidBlurHashComponents { .. })
        ));
        assert!(matches!(
            BlurHashEncoder::encode(&pixels, 4, 4, 4, 4, 10),
            Err(ImageAnalysisError::InvalidBlurHashComponents { .. })
        ));
    }

    #[test]
    fn test_blurhash_buffer_size_mismatch() {
        let pixels = vec![0u8; 10]; // wrong size
        assert!(matches!(
            BlurHashEncoder::encode(&pixels, 4, 4, 4, 4, 3),
            Err(ImageAnalysisError::BufferSizeMismatch { .. })
        ));
    }

    #[test]
    fn test_blurhash_rgb_channels() {
        let pixels: Vec<u8> = (0..27).collect(); // 3×3×3
        let hash = BlurHashEncoder::encode(&pixels, 3, 3, 3, 2, 2).expect("encode RGB blurhash");
        assert!(!hash.is_empty());
    }

    #[test]
    fn test_srgb_linear_roundtrip_white() {
        let v = srgb_to_linear(255);
        // Linear white ≈ 1.0
        assert!((v - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_srgb_linear_roundtrip_black() {
        let v = srgb_to_linear(0);
        assert!(v.abs() < 1e-9);
    }

    #[test]
    fn test_median_cut_reduces_to_n_buckets() {
        let pixels: Vec<[u8; 3]> = (0u8..=255).map(|v| [v, 255 - v, 128]).collect();
        let buckets = median_cut(pixels, 8);
        assert_eq!(buckets.len(), 8);
    }

    #[test]
    fn test_image_too_small() {
        let pixels: Vec<u8> = vec![];
        assert!(matches!(
            DominantColorExtractor::extract(&pixels, 0, 0, 4, 1),
            Err(ImageAnalysisError::ImageTooSmall { .. })
        ));
    }
}
