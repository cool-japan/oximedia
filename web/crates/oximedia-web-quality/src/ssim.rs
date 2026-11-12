// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Real windowed SSIM (Structural Similarity Index) on normalized `f32`
//! luma planes.
//!
//! # Formula
//!
//! For each `11x11` Gaussian window position:
//!
//! ```text
//! SSIM(x,y) = [(2*mu_x*mu_y + C1) * (2*sigma_xy + C2)]
//!           / [(mu_x^2 + mu_y^2 + C1) * (sigma_x^2 + sigma_y^2 + C2)]
//! ```
//!
//! `mu`/`sigma` are the Gaussian-weighted window mean/variance/covariance;
//! `C1 = (K1*L)^2`, `C2 = (K2*L)^2` with `K1 = 0.01`, `K2 = 0.03`. This crate
//! normalizes luma to `[0.0, 1.0]` before running SSIM, so `L = 1.0`.
//!
//! Constants (window size `11`, `sigma = 1.5`, `K1`, `K2`) and the edge
//! handling (positions are only evaluated where the window fully fits —
//! `half_window..len-half_window`, i.e. edges are cropped, not padded or
//! reflected) are ported from `crates/oximedia-quality/src/ssim.rs`. The
//! **loop structure is not**: that file evaluates the `11x11` window density
//! directly at every pixel (`O(width*height*121)`); this module exploits the
//! Gaussian's separability (`exp(-(dx^2+dy^2)/2s^2) = exp(-dx^2/2s^2) *
//! exp(-dy^2/2s^2)`) to run a 1-D horizontal pass followed by a 1-D vertical
//! pass over each of the five window sums (`mu_x`, `mu_y`, `E[x^2]`,
//! `E[y^2]`, `E[xy]`), i.e. `O(width*height*window)`. A "uniform window"
//! (box-filter) fallback for tiny frames is intentionally not implemented —
//! [`SsimKernel::new`] rejects frames smaller than the window instead.
//!
//! VMAF is explicitly **not** implemented here: see the crate-level docs.

use crate::error::{QualityError, Result};

/// SSIM window edge length (`11x11`), matching the native port.
pub const WINDOW_SIZE: usize = 11;

/// `WINDOW_SIZE / 2`: how far the window extends from its center pixel.
const HALF_WINDOW: usize = WINDOW_SIZE / 2;

/// Gaussian standard deviation for the window weights.
const SIGMA: f64 = 1.5;

/// SSIM stabilization constant `C1 = (K1 * L)^2`, `K1 = 0.01`, `L = 1.0`.
const C1: f32 = 0.0001;

/// SSIM stabilization constant `C2 = (K2 * L)^2`, `K2 = 0.03`, `L = 1.0`.
const C2: f32 = 0.0009;

/// Number of channels written per pixel by [`SsimKernel::render_heatmap_rgba8`].
const HEATMAP_CHANNELS: usize = 4;

/// Builds the normalized 1-D Gaussian kernel (`WINDOW_SIZE` taps,
/// `sigma = 1.5`). The 2-D `11x11` window used by the native reference is
/// exactly the outer product of this kernel with itself, since a
/// zero-covariance 2-D Gaussian is separable and both the row and column
/// normalization factor out identically.
fn gaussian_kernel_1d() -> [f32; WINDOW_SIZE] {
    let center = (WINDOW_SIZE as f64 - 1.0) / 2.0;
    let mut raw = [0.0f64; WINDOW_SIZE];
    let mut sum = 0.0f64;
    for (i, slot) in raw.iter_mut().enumerate() {
        let d = i as f64 - center;
        let v = (-(d * d) / (2.0 * SIGMA * SIGMA)).exp();
        *slot = v;
        sum += v;
    }
    let mut kernel = [0.0f32; WINDOW_SIZE];
    for (k, v) in kernel.iter_mut().zip(raw.iter()) {
        *k = (*v / sum) as f32;
    }
    kernel
}

/// Squares every element of `src` into `dst` (`dst.len() == src.len()`).
#[inline]
fn square_into(src: &[f32], dst: &mut [f32]) {
    for (d, &s) in dst.iter_mut().zip(src.iter()) {
        *d = s * s;
    }
}

/// Multiplies `a` and `b` element-wise into `dst` (all equal length).
#[inline]
fn multiply_into(a: &[f32], b: &[f32], dst: &mut [f32]) {
    for ((d, &av), &bv) in dst.iter_mut().zip(a.iter()).zip(b.iter()) {
        *d = av * bv;
    }
}

/// Horizontal (row-wise) 1-D Gaussian pass, "valid" mode: output column `x`
/// (`0..width - 2*HALF_WINDOW`) is the weighted sum of input columns
/// `x..x + WINDOW_SIZE`, centered at `x + HALF_WINDOW`.
///
/// `out` must be exactly `(width - 2*HALF_WINDOW) * height` long. Takes a
/// single already-materialized signal (`a`, `b`, `a*a`, `b*b`, or `a*b`) so
/// the convolution loop itself never branches on which signal it is —
/// [`SsimKernel::compute`] materializes the squared/product signals into
/// scratch once per call instead.
fn horizontal_blur(
    src: &[f32],
    width: usize,
    height: usize,
    kernel: &[f32; WINDOW_SIZE],
    out: &mut [f32],
) {
    let valid_width = width - 2 * HALF_WINDOW;
    for y in 0..height {
        let row = &src[y * width..(y + 1) * width];
        let out_row = &mut out[y * valid_width..(y + 1) * valid_width];
        for (x, slot) in out_row.iter_mut().enumerate() {
            let window = &row[x..x + WINDOW_SIZE];
            let mut acc = 0.0f32;
            for (&weight, &value) in kernel.iter().zip(window.iter()) {
                acc += weight * value;
            }
            *slot = acc;
        }
    }
}

/// Vertical (column-wise) 1-D Gaussian pass, "valid" mode, over the output
/// of [`horizontal_blur`]. `input` is `valid_width * height`; `out` is
/// `valid_width * (height - 2*HALF_WINDOW)`.
fn vertical_blur(
    input: &[f32],
    valid_width: usize,
    height: usize,
    kernel: &[f32; WINDOW_SIZE],
    out: &mut [f32],
) {
    let valid_height = height - 2 * HALF_WINDOW;
    for slot in out.iter_mut() {
        *slot = 0.0;
    }
    for (k, &weight) in kernel.iter().enumerate() {
        for y_out in 0..valid_height {
            let in_row = &input[(y_out + k) * valid_width..(y_out + k + 1) * valid_width];
            let out_row = &mut out[y_out * valid_width..(y_out + 1) * valid_width];
            for (o, &i) in out_row.iter_mut().zip(in_row.iter()) {
                *o += weight * i;
            }
        }
    }
}

/// Maps an SSIM value to a red (dissimilar) -> green (similar) heatmap
/// color. Inputs outside `[0.0, 1.0]` (e.g. slightly negative from
/// anti-correlated structure) are clamped by
/// [`oximedia_web_core::normalize::f32_to_u8`].
#[inline]
fn heatmap_color(ssim: f32) -> (u8, u8, u8) {
    let r = oximedia_web_core::normalize::f32_to_u8(1.0 - ssim);
    let g = oximedia_web_core::normalize::f32_to_u8(ssim);
    (r, g, 0)
}

/// Preallocated separable-Gaussian SSIM evaluator bound to a fixed frame
/// size. All working buffers are sized once in [`SsimKernel::new`]; calling
/// [`SsimKernel::compute`] repeatedly never allocates.
#[derive(Debug)]
pub struct SsimKernel {
    width: usize,
    height: usize,
    valid_width: usize,
    valid_height: usize,
    gaussian: [f32; WINDOW_SIZE],
    /// Full-resolution scratch (`width * height`) for the materialized
    /// `a*a` / `b*b` / `a*b` signals, reused for each in turn.
    raw_scratch: Vec<f32>,
    /// Horizontal-pass scratch (`valid_width * height`), reused for each of
    /// the five signals in turn.
    h_scratch: Vec<f32>,
    mu_a: Vec<f32>,
    mu_b: Vec<f32>,
    e_aa: Vec<f32>,
    e_bb: Vec<f32>,
    e_ab: Vec<f32>,
    /// Per-pixel SSIM over the valid region (`valid_width * valid_height`).
    map: Vec<f32>,
}

impl SsimKernel {
    /// Creates a kernel for `width x height` luma planes, preallocating
    /// every scratch buffer.
    ///
    /// # Errors
    ///
    /// Returns [`QualityError::WindowTooLarge`] if `width` or `height` is
    /// smaller than [`WINDOW_SIZE`] (`11`); there is no smaller-window
    /// fallback.
    pub fn new(width: usize, height: usize) -> Result<Self> {
        if width < WINDOW_SIZE || height < WINDOW_SIZE {
            return Err(QualityError::WindowTooLarge {
                width,
                height,
                window: WINDOW_SIZE,
            });
        }
        let valid_width = width - 2 * HALF_WINDOW;
        let valid_height = height - 2 * HALF_WINDOW;
        let valid_len = valid_width * valid_height;
        Ok(Self {
            width,
            height,
            valid_width,
            valid_height,
            gaussian: gaussian_kernel_1d(),
            raw_scratch: vec![0.0f32; width * height],
            h_scratch: vec![0.0f32; valid_width * height],
            mu_a: vec![0.0f32; valid_len],
            mu_b: vec![0.0f32; valid_len],
            e_aa: vec![0.0f32; valid_len],
            e_bb: vec![0.0f32; valid_len],
            e_ab: vec![0.0f32; valid_len],
            map: vec![0.0f32; valid_len],
        })
    }

    /// Frame width this kernel was constructed for.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// Frame height this kernel was constructed for.
    #[must_use]
    pub const fn height(&self) -> usize {
        self.height
    }

    /// Computes the per-pixel SSIM map for two `width * height` normalized
    /// (`[0.0, 1.0]`) luma planes.
    ///
    /// # Errors
    ///
    /// Returns [`QualityError::Core`] if either plane's length does not
    /// equal `width * height`.
    pub fn compute(&mut self, a: &[f32], b: &[f32]) -> Result<()> {
        let expected = self.width * self.height;
        if a.len() != expected {
            return Err(QualityError::Core(oximedia_web_core::CoreError::BufferLength {
                expected,
                actual: a.len(),
            }));
        }
        if b.len() != expected {
            return Err(QualityError::Core(oximedia_web_core::CoreError::BufferLength {
                expected,
                actual: b.len(),
            }));
        }

        let (width, height) = (self.width, self.height);

        horizontal_blur(a, width, height, &self.gaussian, &mut self.h_scratch);
        vertical_blur(&self.h_scratch, self.valid_width, height, &self.gaussian, &mut self.mu_a);

        horizontal_blur(b, width, height, &self.gaussian, &mut self.h_scratch);
        vertical_blur(&self.h_scratch, self.valid_width, height, &self.gaussian, &mut self.mu_b);

        square_into(a, &mut self.raw_scratch);
        horizontal_blur(&self.raw_scratch, width, height, &self.gaussian, &mut self.h_scratch);
        vertical_blur(&self.h_scratch, self.valid_width, height, &self.gaussian, &mut self.e_aa);

        square_into(b, &mut self.raw_scratch);
        horizontal_blur(&self.raw_scratch, width, height, &self.gaussian, &mut self.h_scratch);
        vertical_blur(&self.h_scratch, self.valid_width, height, &self.gaussian, &mut self.e_bb);

        multiply_into(a, b, &mut self.raw_scratch);
        horizontal_blur(&self.raw_scratch, width, height, &self.gaussian, &mut self.h_scratch);
        vertical_blur(&self.h_scratch, self.valid_width, height, &self.gaussian, &mut self.e_ab);

        for i in 0..self.map.len() {
            let mu_x = self.mu_a[i];
            let mu_y = self.mu_b[i];
            let sigma_x2 = self.e_aa[i] - mu_x * mu_x;
            let sigma_y2 = self.e_bb[i] - mu_y * mu_y;
            let sigma_xy = self.e_ab[i] - mu_x * mu_y;
            let numerator = (2.0 * mu_x * mu_y + C1) * (2.0 * sigma_xy + C2);
            let denominator = (mu_x * mu_x + mu_y * mu_y + C1) * (sigma_x2 + sigma_y2 + C2);
            self.map[i] = numerator / denominator;
        }
        Ok(())
    }

    /// Mean SSIM over the valid (window-covered) region, accumulated in
    /// `f64` for a stable scalar result.
    #[must_use]
    pub fn mean(&self) -> f64 {
        if self.map.is_empty() {
            return 1.0;
        }
        let sum: f64 = self.map.iter().map(|&v| f64::from(v)).sum();
        sum / self.map.len() as f64
    }

    /// Renders the last [`compute`](Self::compute)d SSIM map as an RGBA8
    /// heatmap (red = dissimilar, green = similar) into `out`, which must be
    /// exactly `width * height * 4` bytes. Pixels outside the valid window
    /// region (the `HALF_WINDOW`-pixel border) repeat the nearest valid
    /// value.
    pub fn render_heatmap_rgba8(&self, out: &mut [u8]) {
        for y in 0..self.height {
            let vy = clamp_to_valid(y, HALF_WINDOW, self.valid_height);
            let out_row = &mut out[y * self.width * HEATMAP_CHANNELS..][..self.width * HEATMAP_CHANNELS];
            for (x, px) in out_row.chunks_exact_mut(HEATMAP_CHANNELS).enumerate() {
                let vx = clamp_to_valid(x, HALF_WINDOW, self.valid_width);
                let (r, g, b) = heatmap_color(self.map[vy * self.valid_width + vx]);
                px[0] = r;
                px[1] = g;
                px[2] = b;
                px[3] = 255;
            }
        }
    }
}

/// Maps a full-resolution coordinate to the nearest valid-region index,
/// clamping into the border rather than indexing out of bounds.
#[inline]
fn clamp_to_valid(coord: usize, half: usize, valid_len: usize) -> usize {
    if coord < half {
        0
    } else {
        let shifted = coord - half;
        if shifted >= valid_len {
            valid_len - 1
        } else {
            shifted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic pseudo-random `[0.0, 1.0]` image generator (LCG, no
    /// `rand` crate — matches the pattern used across `oximedia-web-core`'s
    /// own tests).
    struct Lcg(u64);
    impl Lcg {
        fn new(seed: u64) -> Self {
            Self(seed)
        }
        fn next_u8(&mut self) -> u8 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 56) as u8
        }
    }

    fn lcg_image(width: usize, height: usize, seed: u64) -> Vec<f32> {
        let mut lcg = Lcg::new(seed);
        (0..width * height)
            .map(|_| f32::from(lcg.next_u8()) / 255.0)
            .collect()
    }

    /// Naive `O(width * height * WINDOW_SIZE^2)` reference: evaluates the
    /// full `11x11` window density directly at every valid pixel, exactly
    /// like `crates/oximedia-quality/src/ssim.rs::calculate_ssim_at_position`
    /// (ported edge handling: only positions where the window fully fits
    /// are evaluated).
    fn naive_ssim_mean(a: &[f32], b: &[f32], width: usize, height: usize) -> f64 {
        let k = gaussian_kernel_1d();
        let mut window = [[0.0f32; WINDOW_SIZE]; WINDOW_SIZE];
        for (dy, row) in window.iter_mut().enumerate() {
            for (dx, cell) in row.iter_mut().enumerate() {
                *cell = k[dy] * k[dx];
            }
        }

        let mut sum = 0.0f64;
        let mut count = 0u64;
        for cy in HALF_WINDOW..height - HALF_WINDOW {
            for cx in HALF_WINDOW..width - HALF_WINDOW {
                let mut mu_a = 0.0f32;
                let mut mu_b = 0.0f32;
                let mut e_aa = 0.0f32;
                let mut e_bb = 0.0f32;
                let mut e_ab = 0.0f32;
                for (dy, row) in window.iter().enumerate() {
                    let y = cy - HALF_WINDOW + dy;
                    for (dx, &w) in row.iter().enumerate() {
                        let x = cx - HALF_WINDOW + dx;
                        let av = a[y * width + x];
                        let bv = b[y * width + x];
                        mu_a += w * av;
                        mu_b += w * bv;
                        e_aa += w * av * av;
                        e_bb += w * bv * bv;
                        e_ab += w * av * bv;
                    }
                }
                let sigma_a2 = e_aa - mu_a * mu_a;
                let sigma_b2 = e_bb - mu_b * mu_b;
                let sigma_ab = e_ab - mu_a * mu_b;
                let numerator = (2.0 * mu_a * mu_b + C1) * (2.0 * sigma_ab + C2);
                let denominator = (mu_a * mu_a + mu_b * mu_b + C1) * (sigma_a2 + sigma_b2 + C2);
                sum += f64::from(numerator / denominator);
                count += 1;
            }
        }
        sum / count as f64
    }

    fn cross_validate(width: usize, height: usize, seed_a: u64, seed_b: u64) {
        let a = lcg_image(width, height, seed_a);
        let b = lcg_image(width, height, seed_b);
        let mut kernel = SsimKernel::new(width, height).unwrap();
        kernel.compute(&a, &b).unwrap();
        let separable = kernel.mean();
        let naive = naive_ssim_mean(&a, &b, width, height);
        assert!(
            (separable - naive).abs() <= 1e-5,
            "{width}x{height}: separable={separable} naive={naive} diff={}",
            (separable - naive).abs()
        );
    }

    #[test]
    fn separable_matches_naive_64x48() {
        cross_validate(64, 48, 0x1234_5678, 0x9abc_def0);
    }

    #[test]
    fn separable_matches_naive_33x17() {
        cross_validate(33, 17, 0xdead_beef, 0xc0ffee00);
    }

    #[test]
    fn separable_matches_naive_same_image() {
        // a == b via distinct seeds that happen to be identical: run with
        // literally the same buffer instead so the naive path also sees
        // bit-identical input.
        let a = lcg_image(48, 36, 0x42);
        let mut kernel = SsimKernel::new(48, 36).unwrap();
        kernel.compute(&a, &a).unwrap();
        let separable = kernel.mean();
        let naive = naive_ssim_mean(&a, &a, 48, 36);
        assert!((separable - naive).abs() <= 1e-5);
    }

    #[test]
    fn identical_images_ssim_is_exactly_one() {
        let a = lcg_image(32, 32, 0x1357_9bdf);
        let mut kernel = SsimKernel::new(32, 32).unwrap();
        kernel.compute(&a, &a).unwrap();
        assert_eq!(kernel.mean(), 1.0);
        assert!(kernel.map.iter().all(|&v| v == 1.0));
    }

    #[test]
    fn very_different_images_have_low_ssim() {
        let a = vec![0.0f32; 32 * 32];
        let b = vec![1.0f32; 32 * 32];
        let mut kernel = SsimKernel::new(32, 32).unwrap();
        kernel.compute(&a, &b).unwrap();
        assert!(kernel.mean() < 0.1, "mean={}", kernel.mean());
    }

    #[test]
    fn window_too_large_errors() {
        assert!(matches!(
            SsimKernel::new(10, 20),
            Err(QualityError::WindowTooLarge { .. })
        ));
        assert!(matches!(
            SsimKernel::new(20, 5),
            Err(QualityError::WindowTooLarge { .. })
        ));
    }

    #[test]
    fn mismatched_plane_lengths_error() {
        let mut kernel = SsimKernel::new(16, 16).unwrap();
        let a = vec![0.0f32; 16 * 16];
        let b = vec![0.0f32; 16 * 16 - 1];
        assert!(matches!(
            kernel.compute(&a, &b),
            Err(QualityError::Core(oximedia_web_core::CoreError::BufferLength { .. }))
        ));
    }

    #[test]
    fn gaussian_kernel_sums_to_one() {
        let k = gaussian_kernel_1d();
        let sum: f32 = k.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        // Symmetric and centered: the middle tap is the largest.
        let center = k[HALF_WINDOW];
        assert!(center > k[0]);
        assert!((k[0] - k[WINDOW_SIZE - 1]).abs() < 1e-7);
    }

    #[test]
    fn heatmap_covers_full_resolution_and_borders_are_defined() {
        let mut kernel = SsimKernel::new(16, 16).unwrap();
        let a = lcg_image(16, 16, 1);
        let b = lcg_image(16, 16, 2);
        kernel.compute(&a, &b).unwrap();
        let mut out = vec![0u8; 16 * 16 * HEATMAP_CHANNELS];
        kernel.render_heatmap_rgba8(&mut out);
        for px in out.chunks_exact(HEATMAP_CHANNELS) {
            assert_eq!(px[3], 255);
        }
    }
}
