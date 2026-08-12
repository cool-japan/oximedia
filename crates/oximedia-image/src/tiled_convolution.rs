//! Tiled parallel convolution for large images.
//!
//! Standard convolution processes the entire image sequentially, which is
//! inefficient for large images (4K+). This module splits the image into
//! rectangular tiles that can be processed independently and in parallel
//! using Rayon, while correctly handling tile-border overlap (halo) regions
//! so that the output is identical to a single-pass convolution.
//!
//! # Algorithm
//!
//! 1. Determine tile size and halo (half the kernel size).
//! 2. Partition the output image into non-overlapping tiles.
//! 3. For each tile, read the source region (tile + halo) from the input.
//! 4. Apply the kernel convolution to the source region.
//! 5. Copy the inner (non-halo) result back into the output buffer.
//!
//! Steps 3-5 run in parallel across tiles.

#![allow(dead_code)]

use crate::error::{ImageError, ImageResult};

// ── Kernel ──────────────────────────────────────────────────────────────────

/// A square convolution kernel.
#[derive(Debug, Clone)]
pub struct TileKernel {
    /// Kernel side length (must be odd).
    pub size: usize,
    /// Row-major weights, length = size * size.
    pub weights: Vec<f32>,
    /// Normalisation divisor.
    pub divisor: f32,
}

impl TileKernel {
    /// Create a new kernel, validating dimensions.
    pub fn new(size: usize, weights: Vec<f32>, divisor: f32) -> ImageResult<Self> {
        if size == 0 || size % 2 == 0 {
            return Err(ImageError::invalid_format(
                "Kernel size must be odd and positive",
            ));
        }
        if weights.len() != size * size {
            return Err(ImageError::invalid_format(format!(
                "Expected {} weights for {}x{} kernel, got {}",
                size * size,
                size,
                size,
                weights.len()
            )));
        }
        if divisor.abs() < f32::EPSILON {
            return Err(ImageError::invalid_format(
                "Kernel divisor must not be zero",
            ));
        }
        Ok(Self {
            size,
            weights,
            divisor,
        })
    }

    /// Half-width of the kernel (the "halo" needed around each tile).
    #[must_use]
    pub fn half(&self) -> usize {
        self.size / 2
    }

    /// Create a Gaussian blur kernel.
    pub fn gaussian(size: usize, sigma: f32) -> ImageResult<Self> {
        if size == 0 || size % 2 == 0 {
            return Err(ImageError::invalid_format("Size must be odd and positive"));
        }
        if sigma <= 0.0 {
            return Err(ImageError::invalid_format("Sigma must be positive"));
        }
        let half = size as i32 / 2;
        let mut weights = Vec::with_capacity(size * size);
        let mut sum = 0.0f32;
        let inv_2sigma2 = 1.0 / (2.0 * sigma * sigma);
        for y in -half..=half {
            for x in -half..=half {
                let w = (-(x * x + y * y) as f32 * inv_2sigma2).exp();
                weights.push(w);
                sum += w;
            }
        }
        Self::new(size, weights, sum)
    }

    /// Create a box blur kernel of the given size.
    pub fn box_blur(size: usize) -> ImageResult<Self> {
        if size == 0 || size % 2 == 0 {
            return Err(ImageError::invalid_format("Size must be odd and positive"));
        }
        let n = size * size;
        let weights = vec![1.0f32; n];
        Self::new(size, weights, n as f32)
    }

    /// Create a sharpen kernel.
    pub fn sharpen() -> ImageResult<Self> {
        #[rustfmt::skip]
        let weights = vec![
             0.0, -1.0,  0.0,
            -1.0,  5.0, -1.0,
             0.0, -1.0,  0.0,
        ];
        Self::new(3, weights, 1.0)
    }

    /// Create an edge-detect (Laplacian) kernel.
    pub fn laplacian() -> ImageResult<Self> {
        #[rustfmt::skip]
        let weights = vec![
            0.0,  1.0, 0.0,
            1.0, -4.0, 1.0,
            0.0,  1.0, 0.0,
        ];
        Self::new(3, weights, 1.0)
    }
}

// ── Tile config ─────────────────────────────────────────────────────────────

/// Configuration for tiled convolution.
#[derive(Debug, Clone)]
pub struct TileConfig {
    /// Width of each output tile (before halo expansion).
    pub tile_width: usize,
    /// Height of each output tile.
    pub tile_height: usize,
}

impl TileConfig {
    /// Create a new tile config.
    #[must_use]
    pub fn new(tile_width: usize, tile_height: usize) -> Self {
        Self {
            tile_width,
            tile_height,
        }
    }

    /// Default config: 256x256 tiles.
    #[must_use]
    pub fn default_config() -> Self {
        Self::new(256, 256)
    }

    /// Number of tiles needed to cover the given dimensions.
    #[must_use]
    pub fn tile_count(&self, width: usize, height: usize) -> (usize, usize) {
        let cols = (width + self.tile_width - 1) / self.tile_width;
        let rows = (height + self.tile_height - 1) / self.tile_height;
        (cols, rows)
    }

    /// Total number of tiles.
    #[must_use]
    pub fn total_tiles(&self, width: usize, height: usize) -> usize {
        let (cols, rows) = self.tile_count(width, height);
        cols * rows
    }
}

impl Default for TileConfig {
    fn default() -> Self {
        Self::default_config()
    }
}

// ── Single-channel image buffer ─────────────────────────────────────────────

/// Row-major f32 grayscale image buffer.
#[derive(Debug, Clone)]
pub struct GrayImage {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Pixel data (row-major).
    pub data: Vec<f32>,
}

impl GrayImage {
    /// Create a new zero-filled image.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height],
        }
    }

    /// Create from existing data.
    pub fn from_data(width: usize, height: usize, data: Vec<f32>) -> ImageResult<Self> {
        if data.len() != width * height {
            return Err(ImageError::invalid_format(format!(
                "Expected {} elements, got {}",
                width * height,
                data.len()
            )));
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// Get pixel value with bounds checking, returning 0.0 for out-of-bounds.
    #[must_use]
    pub fn get_clamped(&self, x: i32, y: i32) -> f32 {
        let cx = x.clamp(0, self.width as i32 - 1) as usize;
        let cy = y.clamp(0, self.height as i32 - 1) as usize;
        self.data[cy * self.width + cx]
    }

    /// Set pixel value.
    pub fn set(&mut self, x: usize, y: usize, val: f32) {
        if x < self.width && y < self.height {
            self.data[y * self.width + x] = val;
        }
    }
}

// ── Tile descriptor ─────────────────────────────────────────────────────────

/// Describes one output tile's position and dimensions.
#[derive(Debug, Clone, Copy)]
struct TileDesc {
    /// Output tile X start.
    out_x: usize,
    /// Output tile Y start.
    out_y: usize,
    /// Output tile width (may be smaller at edges).
    out_w: usize,
    /// Output tile height.
    out_h: usize,
}

/// Generate all tile descriptors for an image.
fn generate_tiles(width: usize, height: usize, config: &TileConfig) -> Vec<TileDesc> {
    let mut tiles = Vec::new();
    let mut y = 0;
    while y < height {
        let out_h = (height - y).min(config.tile_height);
        let mut x = 0;
        while x < width {
            let out_w = (width - x).min(config.tile_width);
            tiles.push(TileDesc {
                out_x: x,
                out_y: y,
                out_w,
                out_h,
            });
            x += config.tile_width;
        }
        y += config.tile_height;
    }
    tiles
}

// ── Sequential convolution (reference / fallback) ───────────────────────────

/// Apply convolution to a single tile (processes the halo-expanded source region).
fn convolve_tile(src: &GrayImage, kernel: &TileKernel, tile: &TileDesc) -> Vec<f32> {
    let half = kernel.half() as i32;
    let mut output = Vec::with_capacity(tile.out_w * tile.out_h);

    for ty in 0..tile.out_h {
        let img_y = (tile.out_y + ty) as i32;
        for tx in 0..tile.out_w {
            let img_x = (tile.out_x + tx) as i32;
            let mut acc = 0.0f32;
            for ky in -half..=half {
                for kx in -half..=half {
                    let sy = img_y + ky;
                    let sx = img_x + kx;
                    let k_idx = ((ky + half) as usize) * kernel.size + (kx + half) as usize;
                    acc += src.get_clamped(sx, sy) * kernel.weights[k_idx];
                }
            }
            output.push(acc / kernel.divisor);
        }
    }
    output
}

/// Apply tiled convolution sequentially (for comparison / small images).
pub fn convolve_tiled_sequential(
    src: &GrayImage,
    kernel: &TileKernel,
    config: &TileConfig,
) -> ImageResult<GrayImage> {
    if src.width == 0 || src.height == 0 {
        return Err(ImageError::InvalidDimensions(
            src.width as u32,
            src.height as u32,
        ));
    }
    let tiles = generate_tiles(src.width, src.height, config);
    let mut output = GrayImage::new(src.width, src.height);

    for tile in &tiles {
        let tile_data = convolve_tile(src, kernel, tile);
        // Copy tile data into output
        for ty in 0..tile.out_h {
            for tx in 0..tile.out_w {
                let dst_x = tile.out_x + tx;
                let dst_y = tile.out_y + ty;
                output.data[dst_y * output.width + dst_x] = tile_data[ty * tile.out_w + tx];
            }
        }
    }
    Ok(output)
}

/// Apply tiled convolution in parallel using rayon (sequentially on `wasm32`).
pub fn convolve_tiled_parallel(
    src: &GrayImage,
    kernel: &TileKernel,
    config: &TileConfig,
) -> ImageResult<GrayImage> {
    use crate::parallel::map_slice;

    if src.width == 0 || src.height == 0 {
        return Err(ImageError::InvalidDimensions(
            src.width as u32,
            src.height as u32,
        ));
    }
    let tiles = generate_tiles(src.width, src.height, config);

    // Process tiles in parallel, each producing (TileDesc, Vec<f32>)
    let results: Vec<(TileDesc, Vec<f32>)> =
        map_slice(&tiles, |tile| (*tile, convolve_tile(src, kernel, tile)));

    let mut output = GrayImage::new(src.width, src.height);
    for (tile, tile_data) in &results {
        for ty in 0..tile.out_h {
            for tx in 0..tile.out_w {
                let dst_x = tile.out_x + tx;
                let dst_y = tile.out_y + ty;
                output.data[dst_y * output.width + dst_x] = tile_data[ty * tile.out_w + tx];
            }
        }
    }
    Ok(output)
}

/// Convenience: apply convolution using default tile config.
pub fn convolve_parallel(src: &GrayImage, kernel: &TileKernel) -> ImageResult<GrayImage> {
    convolve_tiled_parallel(src, kernel, &TileConfig::default())
}

/// Apply a full (non-tiled) convolution for reference/testing.
pub fn convolve_full(src: &GrayImage, kernel: &TileKernel) -> ImageResult<GrayImage> {
    if src.width == 0 || src.height == 0 {
        return Err(ImageError::InvalidDimensions(
            src.width as u32,
            src.height as u32,
        ));
    }
    let half = kernel.half() as i32;
    let mut output = GrayImage::new(src.width, src.height);

    for y in 0..src.height as i32 {
        for x in 0..src.width as i32 {
            let mut acc = 0.0f32;
            for ky in -half..=half {
                for kx in -half..=half {
                    let k_idx = ((ky + half) as usize) * kernel.size + (kx + half) as usize;
                    acc += src.get_clamped(x + kx, y + ky) * kernel.weights[k_idx];
                }
            }
            output.data[y as usize * output.width + x as usize] = acc / kernel.divisor;
        }
    }
    Ok(output)
}

// ── Multi-channel support ───────────────────────────────────────────────────

/// An RGB f32 image buffer.
#[derive(Debug, Clone)]
pub struct RgbImage {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Pixel data as [R, G, B] triplets, row-major.
    pub data: Vec<[f32; 3]>,
}

impl RgbImage {
    /// Create a new zero-filled RGB image.
    #[must_use]
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![[0.0; 3]; width * height],
        }
    }

    /// Split into three grayscale channels.
    #[must_use]
    pub fn split_channels(&self) -> [GrayImage; 3] {
        let n = self.width * self.height;
        let mut r = vec![0.0f32; n];
        let mut g = vec![0.0f32; n];
        let mut b = vec![0.0f32; n];
        for (i, px) in self.data.iter().enumerate() {
            r[i] = px[0];
            g[i] = px[1];
            b[i] = px[2];
        }
        [
            GrayImage {
                width: self.width,
                height: self.height,
                data: r,
            },
            GrayImage {
                width: self.width,
                height: self.height,
                data: g,
            },
            GrayImage {
                width: self.width,
                height: self.height,
                data: b,
            },
        ]
    }

    /// Merge three grayscale channels back into an RGB image.
    pub fn from_channels(channels: &[GrayImage; 3]) -> ImageResult<Self> {
        let w = channels[0].width;
        let h = channels[0].height;
        if channels[1].width != w
            || channels[1].height != h
            || channels[2].width != w
            || channels[2].height != h
        {
            return Err(ImageError::invalid_format("Channel dimensions mismatch"));
        }
        let n = w * h;
        let mut data = Vec::with_capacity(n);
        for i in 0..n {
            data.push([
                channels[0].data[i],
                channels[1].data[i],
                channels[2].data[i],
            ]);
        }
        Ok(Self {
            width: w,
            height: h,
            data,
        })
    }
}

/// Apply tiled parallel convolution to an RGB image (processes each channel independently).
pub fn convolve_rgb_parallel(
    src: &RgbImage,
    kernel: &TileKernel,
    config: &TileConfig,
) -> ImageResult<RgbImage> {
    let channels = src.split_channels();
    let r_out = convolve_tiled_parallel(&channels[0], kernel, config)?;
    let g_out = convolve_tiled_parallel(&channels[1], kernel, config)?;
    let b_out = convolve_tiled_parallel(&channels[2], kernel, config)?;
    RgbImage::from_channels(&[r_out, g_out, b_out])
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_gradient(w: usize, h: usize) -> GrayImage {
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.data[y * w + x] = (x + y) as f32 / (w + h) as f32;
            }
        }
        img
    }

    fn make_constant(w: usize, h: usize, val: f32) -> GrayImage {
        GrayImage {
            width: w,
            height: h,
            data: vec![val; w * h],
        }
    }

    #[test]
    fn test_kernel_creation_valid() {
        let k = TileKernel::new(3, vec![1.0; 9], 9.0);
        assert!(k.is_ok());
        let k = k.expect("valid");
        assert_eq!(k.half(), 1);
    }

    #[test]
    fn test_kernel_creation_invalid_even_size() {
        assert!(TileKernel::new(4, vec![1.0; 16], 1.0).is_err());
    }

    #[test]
    fn test_kernel_creation_wrong_weight_count() {
        assert!(TileKernel::new(3, vec![1.0; 8], 1.0).is_err());
    }

    #[test]
    fn test_kernel_zero_divisor() {
        assert!(TileKernel::new(3, vec![1.0; 9], 0.0).is_err());
    }

    #[test]
    fn test_gaussian_kernel() {
        let k = TileKernel::gaussian(5, 1.0).expect("gauss");
        assert_eq!(k.size, 5);
        assert_eq!(k.weights.len(), 25);
        // Gaussian weights should be symmetric
        assert!((k.weights[0] - k.weights[4]).abs() < 1e-10);
        assert!((k.weights[0] - k.weights[20]).abs() < 1e-10);
    }

    #[test]
    fn test_box_blur_identity_on_constant() {
        let img = make_constant(16, 16, 0.5);
        let kernel = TileKernel::box_blur(3).expect("box");
        let out = convolve_full(&img, &kernel).expect("conv");
        for &v in &out.data {
            assert!((v - 0.5).abs() < 1e-5, "expected 0.5, got {v}");
        }
    }

    #[test]
    fn test_tile_config_counts() {
        let config = TileConfig::new(10, 10);
        assert_eq!(config.tile_count(25, 25), (3, 3));
        assert_eq!(config.total_tiles(25, 25), 9);
        assert_eq!(config.tile_count(10, 10), (1, 1));
        assert_eq!(config.tile_count(11, 11), (2, 2));
    }

    #[test]
    fn test_tiled_matches_full_convolution() {
        let img = make_gradient(32, 32);
        let kernel = TileKernel::gaussian(3, 1.0).expect("gauss");

        let full = convolve_full(&img, &kernel).expect("full");
        let tiled =
            convolve_tiled_sequential(&img, &kernel, &TileConfig::new(8, 8)).expect("tiled");

        assert_eq!(full.data.len(), tiled.data.len());
        for (i, (&f, &t)) in full.data.iter().zip(tiled.data.iter()).enumerate() {
            assert!(
                (f - t).abs() < 1e-5,
                "Mismatch at pixel {i}: full={f}, tiled={t}"
            );
        }
    }

    #[test]
    fn test_parallel_matches_sequential() {
        let img = make_gradient(50, 50);
        let kernel = TileKernel::box_blur(5).expect("box");
        let config = TileConfig::new(16, 16);

        let seq = convolve_tiled_sequential(&img, &kernel, &config).expect("seq");
        let par = convolve_tiled_parallel(&img, &kernel, &config).expect("par");

        for (i, (&s, &p)) in seq.data.iter().zip(par.data.iter()).enumerate() {
            assert!(
                (s - p).abs() < 1e-5,
                "Mismatch at pixel {i}: seq={s}, par={p}"
            );
        }
    }

    #[test]
    fn test_sharpen_kernel() {
        let k = TileKernel::sharpen().expect("sharpen");
        assert_eq!(k.size, 3);
        assert_eq!(k.weights.len(), 9);
        // Center weight should be positive (5)
        assert!(k.weights[4] > 0.0);
    }

    #[test]
    fn test_laplacian_kernel() {
        let k = TileKernel::laplacian().expect("lap");
        // Sum of Laplacian should be 0
        let sum: f32 = k.weights.iter().sum();
        assert!(sum.abs() < 1e-6);
    }

    #[test]
    fn test_gray_image_clamped_access() {
        let img = make_gradient(4, 4);
        // Out-of-bounds should clamp
        let v1 = img.get_clamped(-1, -1);
        let v2 = img.get_clamped(0, 0);
        assert!((v1 - v2).abs() < 1e-10); // both clamp to (0,0)

        let v3 = img.get_clamped(100, 100);
        let v4 = img.get_clamped(3, 3);
        assert!((v3 - v4).abs() < 1e-10); // both clamp to (3,3)
    }

    #[test]
    fn test_rgb_split_merge_round_trip() {
        let mut rgb = RgbImage::new(4, 4);
        for (i, px) in rgb.data.iter_mut().enumerate() {
            *px = [i as f32, (i as f32) * 2.0, (i as f32) * 3.0];
        }
        let channels = rgb.split_channels();
        let merged = RgbImage::from_channels(&channels).expect("merge");
        for (a, b) in rgb.data.iter().zip(merged.data.iter()) {
            assert!((a[0] - b[0]).abs() < 1e-10);
            assert!((a[1] - b[1]).abs() < 1e-10);
            assert!((a[2] - b[2]).abs() < 1e-10);
        }
    }

    #[test]
    fn test_rgb_parallel_convolution() {
        let mut rgb = RgbImage::new(16, 16);
        for px in rgb.data.iter_mut() {
            *px = [0.5, 0.5, 0.5];
        }
        let kernel = TileKernel::box_blur(3).expect("box");
        let config = TileConfig::new(8, 8);
        let out = convolve_rgb_parallel(&rgb, &kernel, &config).expect("conv");
        for px in &out.data {
            for c in 0..3 {
                assert!((px[c] - 0.5).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn test_empty_image_error() {
        let img = GrayImage::new(0, 0);
        let kernel = TileKernel::box_blur(3).expect("box");
        assert!(convolve_full(&img, &kernel).is_err());
        assert!(convolve_tiled_parallel(&img, &kernel, &TileConfig::default()).is_err());
    }

    #[test]
    fn test_single_pixel_image() {
        let img = GrayImage::from_data(1, 1, vec![42.0]).expect("img");
        let kernel = TileKernel::box_blur(3).expect("box");
        let out = convolve_full(&img, &kernel).expect("conv");
        // Single pixel with box blur and clamped borders should return same value
        assert!((out.data[0] - 42.0).abs() < 1e-5);
    }

    #[test]
    fn test_large_kernel_on_small_image() {
        let img = make_constant(4, 4, 1.0);
        let kernel = TileKernel::gaussian(7, 2.0).expect("gauss");
        let config = TileConfig::new(2, 2);
        let out = convolve_tiled_parallel(&img, &kernel, &config).expect("conv");
        // Constant image should remain constant after Gaussian blur
        for &v in &out.data {
            assert!((v - 1.0).abs() < 1e-4, "expected ~1.0, got {v}");
        }
    }

    #[test]
    fn test_tile_generation() {
        let tiles = generate_tiles(25, 25, &TileConfig::new(10, 10));
        assert_eq!(tiles.len(), 9); // 3x3 tiles
                                    // Last column tiles should be narrower
        let last_col = tiles.iter().filter(|t| t.out_x == 20).collect::<Vec<_>>();
        assert!(!last_col.is_empty());
        for t in &last_col {
            assert_eq!(t.out_w, 5);
        }
    }
}
