//! Bilateral filter for edge-preserving spatial denoising.
//!
//! The bilateral filter is a non-linear, edge-preserving smoothing filter
//! that combines spatial and range (intensity) Gaussian kernels.

use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;
use rayon::prelude::*;

/// Apply bilateral filter to a video frame.
///
/// The bilateral filter preserves edges while smoothing uniform regions.
/// It uses both spatial proximity and intensity similarity to determine weights.
///
/// # Arguments
/// * `frame` - Input video frame
/// * `strength` - Denoising strength (0.0 - 1.0)
///
/// # Returns
/// Filtered video frame
pub fn bilateral_filter(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
    if frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    let mut output = frame.clone();

    // Process each plane in parallel
    output
        .planes
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(plane_idx, plane)| {
            let input_plane = &frame.planes[plane_idx];
            let (width, height) = frame.plane_dimensions(plane_idx);

            // Convert strength to filter parameters
            let sigma_space = 3.0 * strength;
            let sigma_range = 25.0 * strength;

            bilateral_filter_plane(
                input_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                sigma_space,
                sigma_range,
            )
        })?;

    Ok(output)
}

/// Apply bilateral filter to a single plane.
#[allow(clippy::too_many_arguments)]
fn bilateral_filter_plane(
    input: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    sigma_space: f32,
    sigma_range: f32,
) -> DenoiseResult<()> {
    let kernel_radius = (3.0 * sigma_space).ceil() as i32;
    let space_coeff = -0.5 / (sigma_space * sigma_space);
    let range_coeff = -0.5 / (sigma_range * sigma_range);

    // Precompute spatial weights
    let mut spatial_weights = vec![0.0f32; (2 * kernel_radius + 1) as usize];
    for i in 0..spatial_weights.len() {
        let d = (i as i32 - kernel_radius) as f32;
        spatial_weights[i] = (d * d * space_coeff).exp();
    }

    // Process each pixel
    for y in 0..height {
        for x in 0..width {
            let center_val = f32::from(input[y * stride + x]);
            let mut sum = 0.0f32;
            let mut weight_sum = 0.0f32;

            // Apply kernel
            for ky in -kernel_radius..=kernel_radius {
                let ny = (y as i32 + ky).clamp(0, (height - 1) as i32) as usize;

                for kx in -kernel_radius..=kernel_radius {
                    let nx = (x as i32 + kx).clamp(0, (width - 1) as i32) as usize;

                    let neighbor_val = f32::from(input[ny * stride + nx]);
                    let value_diff = neighbor_val - center_val;

                    // Combine spatial and range weights
                    let spatial_weight = spatial_weights[(ky + kernel_radius) as usize]
                        * spatial_weights[(kx + kernel_radius) as usize];
                    let range_weight = (value_diff * value_diff * range_coeff).exp();
                    let weight = spatial_weight * range_weight;

                    sum += neighbor_val * weight;
                    weight_sum += weight;
                }
            }

            output[y * stride + x] = if weight_sum > 0.0 {
                (sum / weight_sum).round().clamp(0.0, 255.0) as u8
            } else {
                input[y * stride + x]
            };
        }
    }

    Ok(())
}

/// Fast bilateral filter approximation using separable kernels.
pub fn fast_bilateral_filter(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
    if frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    let mut output = frame.clone();

    // Process each plane with fast approximation
    output
        .planes
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(plane_idx, plane)| {
            let input_plane = &frame.planes[plane_idx];
            let (width, height) = frame.plane_dimensions(plane_idx);

            let sigma_space = 2.0 * strength;
            let sigma_range = 20.0 * strength;

            fast_bilateral_filter_plane(
                input_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                sigma_space,
                sigma_range,
            )
        })?;

    Ok(output)
}

fn fast_bilateral_filter_plane(
    input: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    sigma_space: f32,
    sigma_range: f32,
) -> DenoiseResult<()> {
    let kernel_radius = (2.0 * sigma_space).ceil() as i32;
    let range_coeff = -0.5 / (sigma_range * sigma_range);

    // Simplified box filter approximation
    for y in 0..height {
        for x in 0..width {
            let center_val = f32::from(input[y * stride + x]);
            let mut sum = 0.0f32;
            let mut weight_sum = 0.0f32;

            for ky in -kernel_radius..=kernel_radius {
                let ny = (y as i32 + ky).clamp(0, (height - 1) as i32) as usize;

                for kx in -kernel_radius..=kernel_radius {
                    let nx = (x as i32 + kx).clamp(0, (width - 1) as i32) as usize;

                    let neighbor_val = f32::from(input[ny * stride + nx]);
                    let value_diff = neighbor_val - center_val;
                    let weight = (value_diff * value_diff * range_coeff).exp();

                    sum += neighbor_val * weight;
                    weight_sum += weight;
                }
            }

            output[y * stride + x] = if weight_sum > 0.0 {
                (sum / weight_sum).round().clamp(0.0, 255.0) as u8
            } else {
                input[y * stride + x]
            };
        }
    }

    Ok(())
}

/// Apply bilateral filter using SIMD acceleration where available.
///
/// On x86_64 hosts, this function checks for SSE4.1 at runtime and uses
/// SIMD float arithmetic in the inner accumulation loop for improved
/// throughput. A scalar fallback is used on other architectures or when
/// SSE4.1 is not detected at runtime.
///
/// # Arguments
/// * `src`         - Input grayscale pixel data (row-major, length = `width * height`)
/// * `width`       - Image width in pixels
/// * `height`      - Image height in pixels
/// * `sigma_space` - Spatial Gaussian sigma (controls neighbourhood radius)
/// * `sigma_color` - Range Gaussian sigma (controls intensity tolerance)
///
/// # Returns
/// Denoised pixel data with the same dimensions as the input.
pub fn apply_simd(
    src: &[u8],
    width: u32,
    height: u32,
    sigma_space: f32,
    sigma_color: f32,
) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let stride = w;

    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("sse4.1") {
            return apply_simd_sse41(src, w, h, stride, sigma_space, sigma_color);
        }
    }

    apply_simd_scalar(src, w, h, stride, sigma_space, sigma_color)
}

/// Scalar bilateral filter kernel used as the portable fallback.
///
/// Uses LUT-based range weight precomputation for consistent performance
/// with the SIMD path.
fn apply_simd_scalar(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    sigma_space: f32,
    sigma_color: f32,
) -> Vec<u8> {
    // Guard against degenerate sigma values
    let sigma_space = sigma_space.max(1e-6);
    let sigma_color = sigma_color.max(1e-6);

    let kernel_radius = (3.0 * sigma_space).ceil() as i32;
    let space_coeff = -0.5 / (sigma_space * sigma_space);
    let range_coeff = -0.5 / (sigma_color * sigma_color);

    // Precompute 1-D spatial kernel
    let klen = (2 * kernel_radius + 1) as usize;
    let spatial_kernel: Vec<f32> = (0..klen)
        .map(|i| {
            let d = (i as i32 - kernel_radius) as f32;
            (d * d * space_coeff).exp()
        })
        .collect();

    // Precompute range LUT for d in [0, 255]
    let range_lut: Vec<f32> = (0usize..256)
        .map(|d| {
            let df = d as f32;
            (df * df * range_coeff).exp()
        })
        .collect();

    let mut output = vec![0u8; width * height];

    for y in 0..height {
        for x in 0..width {
            let center = src[y * stride + x];
            let mut sum = 0.0f32;
            let mut weight_sum = 0.0f32;

            for ky in -kernel_radius..=kernel_radius {
                let ny = (y as i32 + ky).clamp(0, (height - 1) as i32) as usize;
                let ky_idx = (ky + kernel_radius) as usize;
                let spatial_y = spatial_kernel[ky_idx];

                for kx in -kernel_radius..=kernel_radius {
                    let nx = (x as i32 + kx).clamp(0, (width - 1) as i32) as usize;
                    let kx_idx = (kx + kernel_radius) as usize;

                    let neighbor = src[ny * stride + nx];
                    let neighbor_f = f32::from(neighbor);

                    let d = if neighbor > center {
                        (neighbor - center) as usize
                    } else {
                        (center - neighbor) as usize
                    };
                    let range_w = range_lut[d.min(255)];
                    let weight = spatial_y * spatial_kernel[kx_idx] * range_w;

                    sum += neighbor_f * weight;
                    weight_sum += weight;
                }
            }

            output[y * stride + x] = if weight_sum > 0.0 {
                (sum / weight_sum).round().clamp(0.0, 255.0) as u8
            } else {
                center
            };
        }
    }

    output
}

/// SSE4.1-accelerated bilateral filter inner loop.
///
/// On non-x86_64 builds this function is not compiled; the call-site
/// is guarded by `#[cfg(target_arch = "x86_64")]` and a runtime
/// `is_x86_feature_detected!("sse4.1")` check.
#[cfg(target_arch = "x86_64")]
#[allow(unsafe_code)]
fn apply_simd_sse41(
    src: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    sigma_space: f32,
    sigma_color: f32,
) -> Vec<u8> {
    use std::arch::x86_64::{__m128, _mm_loadu_ps, _mm_mul_ps, _mm_set1_ps, _mm_storeu_ps};

    let sigma_space = sigma_space.max(1e-6);
    let sigma_color = sigma_color.max(1e-6);

    let kernel_radius = (3.0 * sigma_space).ceil() as i32;
    let space_coeff = -0.5 / (sigma_space * sigma_space);
    let range_coeff = -0.5 / (sigma_color * sigma_color);

    let klen = (2 * kernel_radius + 1) as usize;
    let spatial_kernel: Vec<f32> = (0..klen)
        .map(|i| {
            let d = (i as i32 - kernel_radius) as f32;
            (d * d * space_coeff).exp()
        })
        .collect();

    let range_lut: Vec<f32> = (0usize..256)
        .map(|d| {
            let df = d as f32;
            (df * df * range_coeff).exp()
        })
        .collect();

    let mut output = vec![0u8; width * height];

    for y in 0..height {
        for x in 0..width {
            let center = src[y * stride + x];
            let mut sum = 0.0f32;
            let mut weight_sum = 0.0f32;

            for ky in -kernel_radius..=kernel_radius {
                let ny = (y as i32 + ky).clamp(0, (height - 1) as i32) as usize;
                let ky_idx = (ky + kernel_radius) as usize;
                let spatial_y = spatial_kernel[ky_idx];

                // Process kx positions in groups of 4 via SSE4.1 vectorisation
                let mut kx_idx = 0usize;
                while kx_idx + 4 <= klen {
                    let mut neighbors = [0.0f32; 4];
                    let mut spatial_xs = [0.0f32; 4];
                    let mut range_ws = [0.0f32; 4];

                    for lane in 0..4 {
                        let kx = (kx_idx + lane) as i32 - kernel_radius;
                        let nx = (x as i32 + kx).clamp(0, (width - 1) as i32) as usize;
                        let neighbor = src[ny * stride + nx];
                        neighbors[lane] = f32::from(neighbor);
                        spatial_xs[lane] = spatial_kernel[kx_idx + lane];
                        let d = if neighbor > center {
                            (neighbor - center) as usize
                        } else {
                            (center - neighbor) as usize
                        };
                        range_ws[lane] = range_lut[d.min(255)];
                    }

                    // SAFETY: arrays are stack-allocated, correctly aligned f32 data.
                    unsafe {
                        let n_v: __m128 = _mm_loadu_ps(neighbors.as_ptr());
                        let sw_v: __m128 = _mm_loadu_ps(spatial_xs.as_ptr());
                        let rw_v: __m128 = _mm_loadu_ps(range_ws.as_ptr());
                        let sy_v: __m128 = _mm_set1_ps(spatial_y);

                        // weight = spatial_y * spatial_x * range_w
                        let w_v = _mm_mul_ps(_mm_mul_ps(sy_v, sw_v), rw_v);
                        let wn_v = _mm_mul_ps(w_v, n_v);

                        let mut wn_arr = [0.0f32; 4];
                        let mut w_arr = [0.0f32; 4];
                        _mm_storeu_ps(wn_arr.as_mut_ptr(), wn_v);
                        _mm_storeu_ps(w_arr.as_mut_ptr(), w_v);

                        for lane in 0..4 {
                            sum += wn_arr[lane];
                            weight_sum += w_arr[lane];
                        }
                    }

                    kx_idx += 4;
                }

                // Scalar tail for remaining kernel columns
                while kx_idx < klen {
                    let kx = kx_idx as i32 - kernel_radius;
                    let nx = (x as i32 + kx).clamp(0, (width - 1) as i32) as usize;
                    let neighbor = src[ny * stride + nx];
                    let neighbor_f = f32::from(neighbor);
                    let d = if neighbor > center {
                        (neighbor - center) as usize
                    } else {
                        (center - neighbor) as usize
                    };
                    let range_w = range_lut[d.min(255)];
                    let weight = spatial_y * spatial_kernel[kx_idx] * range_w;
                    sum += neighbor_f * weight;
                    weight_sum += weight;
                    kx_idx += 1;
                }
            }

            output[y * stride + x] = if weight_sum > 0.0 {
                (sum / weight_sum).round().clamp(0.0, 255.0) as u8
            } else {
                center
            };
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_bilateral_filter() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = bilateral_filter(&frame, 0.5);
        assert!(result.is_ok());

        let filtered = result.expect("filtered should be valid");
        assert_eq!(filtered.width, 64);
        assert_eq!(filtered.height, 64);
    }

    #[test]
    fn test_fast_bilateral_filter() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = fast_bilateral_filter(&frame, 0.5);
        assert!(result.is_ok());
    }

    #[test]
    fn test_bilateral_zero_strength() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        frame.allocate();

        let result = bilateral_filter(&frame, 0.0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_bilateral_max_strength() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        frame.allocate();

        let result = bilateral_filter(&frame, 1.0);
        assert!(result.is_ok());
    }

    // ----- Regression tests for the clone-discard identity bug -----
    //
    // `bilateral_filter`/`fast_bilateral_filter` used to call their `*_plane`
    // worker with `&mut plane.data.clone()` — a temporary that was mutated
    // and then immediately discarded, so `plane.data` (the buffer actually
    // returned to the caller) never changed. These tests exercise the
    // public `VideoFrame`-level API (not the private `*_plane` helpers,
    // which were always correct) so they fail if that call-site regresses.

    /// Build a Yuv420p frame whose luma plane is a flat base value with
    /// deterministic pseudo-noise (no RNG dependency), i.e. exactly the
    /// "flat region with noise" scenario bilateral filtering should smooth.
    fn build_noisy_luma_frame(w: u32, h: u32) -> VideoFrame {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, w, h);
        frame.allocate();
        let stride = frame.planes[0].stride;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let idx = y * w as usize + x;
                // Deterministic noise: base 120, +20 on every third pixel.
                let val: u8 = if idx % 3 == 0 { 140 } else { 120 };
                frame.planes[0].data[y * stride + x] = val;
            }
        }
        frame
    }

    /// Variance of the top-left `w`×`h` region of the luma plane (stride-aware).
    fn luma_variance(frame: &VideoFrame, w: usize, h: usize) -> f64 {
        let stride = frame.planes[0].stride;
        let mut vals = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                vals.push(f64::from(frame.planes[0].data[y * stride + x]));
            }
        }
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        vals.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64
    }

    #[test]
    fn test_bilateral_filter_output_differs_from_input() {
        let frame = build_noisy_luma_frame(48, 48);
        let filtered = bilateral_filter(&frame, 0.6).expect("bilateral_filter should succeed");

        let in_stride = frame.planes[0].stride;
        let out_stride = filtered.planes[0].stride;
        let mut differs = false;
        for y in 0..48usize {
            for x in 0..48usize {
                if filtered.planes[0].data[y * out_stride + x]
                    != frame.planes[0].data[y * in_stride + x]
                {
                    differs = true;
                }
            }
        }
        assert!(
            differs,
            "bilateral_filter output must differ from noisy input; identical output means \
             the filtered buffer was written to a temporary and discarded"
        );
    }

    #[test]
    fn test_bilateral_filter_reduces_local_variance() {
        let frame = build_noisy_luma_frame(48, 48);
        let filtered = bilateral_filter(&frame, 0.6).expect("bilateral_filter should succeed");

        let input_var = luma_variance(&frame, 48, 48);
        let output_var = luma_variance(&filtered, 48, 48);
        assert!(
            output_var < input_var,
            "bilateral_filter should reduce local variance: input={input_var:.4} output={output_var:.4}"
        );
    }

    #[test]
    fn test_bilateral_filter_deterministic_across_runs() {
        let frame = build_noisy_luma_frame(40, 40);
        let first = bilateral_filter(&frame, 0.6).expect("first run should succeed");
        let second = bilateral_filter(&frame, 0.6).expect("second run should succeed");

        for plane_idx in 0..first.planes.len() {
            assert_eq!(
                first.planes[plane_idx].data, second.planes[plane_idx].data,
                "bilateral_filter must be deterministic across runs on identical input \
                 (plane {plane_idx})"
            );
        }
    }

    #[test]
    fn test_bilateral_filter_flat_plane_stays_flat() {
        // A perfectly uniform plane has zero local variance everywhere, so
        // bilateral filtering must not introduce ringing/drift.
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        frame.allocate();
        let stride = frame.planes[0].stride;
        for y in 0..32usize {
            for x in 0..32usize {
                frame.planes[0].data[y * stride + x] = 96;
            }
        }

        let filtered = bilateral_filter(&frame, 0.8).expect("bilateral_filter should succeed");
        let out_stride = filtered.planes[0].stride;
        for y in 0..32usize {
            for x in 0..32usize {
                let v = filtered.planes[0].data[y * out_stride + x];
                assert!(
                    (i32::from(v) - 96).abs() <= 1,
                    "flat plane must stay flat (±1) at ({x},{y}), got {v}"
                );
            }
        }
    }

    #[test]
    fn test_fast_bilateral_filter_output_differs_from_input() {
        // fast_bilateral_filter had the identical clone-discard bug; cover it too.
        let frame = build_noisy_luma_frame(40, 40);
        let filtered =
            fast_bilateral_filter(&frame, 0.6).expect("fast_bilateral_filter should succeed");

        let in_stride = frame.planes[0].stride;
        let out_stride = filtered.planes[0].stride;
        let mut differs = false;
        for y in 0..40usize {
            for x in 0..40usize {
                if filtered.planes[0].data[y * out_stride + x]
                    != frame.planes[0].data[y * in_stride + x]
                {
                    differs = true;
                }
            }
        }
        assert!(
            differs,
            "fast_bilateral_filter output must differ from noisy input"
        );
    }

    // ----- apply_simd standalone function tests -----

    #[test]
    fn test_apply_simd_output_length() {
        let w = 16u32;
        let h = 16u32;
        let src = vec![128u8; (w * h) as usize];
        let out = apply_simd(&src, w, h, 2.0, 30.0);
        assert_eq!(out.len(), (w * h) as usize);
    }

    #[test]
    fn test_apply_simd_uniform_input_unchanged() {
        // A uniform image should remain uniform after bilateral filtering.
        let w = 16u32;
        let h = 16u32;
        let src = vec![100u8; (w * h) as usize];
        let out = apply_simd(&src, w, h, 2.0, 30.0);
        for &px in &out {
            assert_eq!(px, 100, "uniform input should stay constant");
        }
    }

    #[test]
    fn test_apply_simd_scalar_consistency() {
        // SIMD and scalar paths must agree within ±1 intensity unit.
        let w = 16u32;
        let h = 16u32;
        // Build a ramp: use modular arithmetic to avoid u8 overflow at 256 pixels.
        let src: Vec<u8> = (0..w * h).map(|i| (i % 256) as u8).collect();

        let scalar = apply_simd_scalar(&src, w as usize, h as usize, w as usize, 2.0, 30.0);
        let simd = apply_simd(&src, w, h, 2.0, 30.0);

        for (i, (&s, &d)) in scalar.iter().zip(simd.iter()).enumerate() {
            let diff = (s as i32 - d as i32).unsigned_abs();
            assert!(diff <= 1, "pixel {i}: scalar={s} vs simd={d}, diff={diff}");
        }
    }

    #[test]
    fn test_apply_simd_small_sigma() {
        // Very small sigma_space => tight filter; output should not panic.
        let w = 8u32;
        let h = 8u32;
        let src: Vec<u8> = (0u8..64).collect();
        let out = apply_simd(&src, w, h, 0.5, 10.0);
        assert_eq!(out.len(), 64);
    }

    // ----- Edge-preservation metric tests -----

    /// Compute variance of a subslice of a pixel buffer.
    fn compute_variance(buf: &[u8], start: usize, len: usize) -> f64 {
        let slice = &buf[start..start + len];
        let mean = slice.iter().map(|&v| v as f64).sum::<f64>() / len as f64;
        slice
            .iter()
            .map(|&v| (v as f64 - mean).powi(2))
            .sum::<f64>()
            / len as f64
    }

    /// Build a 64×64 grayscale buffer: left half = 50, right half = 200,
    /// with deterministic noise applied using a simple index-based pattern.
    fn build_edge_noisy_buffer() -> Vec<u8> {
        let w = 64usize;
        let h = 64usize;
        let mut buf = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let idx = y * w + x;
                let base: u8 = if x < 32 { 50 } else { 200 };
                // Deterministic noise: add 10 when (idx % 7 < 3), else 0
                buf[idx] = if idx % 7 < 3 {
                    base.saturating_add(10)
                } else {
                    base
                };
            }
        }
        buf
    }

    #[test]
    fn test_bilateral_preserves_hard_edge() {
        let w = 64u32;
        let h = 64u32;
        let noisy = build_edge_noisy_buffer();
        let output = apply_simd(&noisy, w, h, 3.0, 30.0);

        // Check that the edge at x=31→32 retains significant contrast for at least 50% of rows.
        let total_rows = h as usize;
        let mut preserved_count = 0usize;
        for y in 0..total_rows {
            let left_px = output[y * 64 + 31] as i32;
            let right_px = output[y * 64 + 32] as i32;
            let contrast = (right_px - left_px).unsigned_abs();
            if contrast > 100 {
                preserved_count += 1;
            }
        }
        assert!(
            preserved_count * 2 >= total_rows,
            "edge contrast > 100 for only {preserved_count}/{total_rows} rows; bilateral should preserve the hard edge for ≥50% of rows"
        );
    }

    #[test]
    fn test_bilateral_reduces_flat_region_variance() {
        let w = 64u32;
        let h = 64u32;
        let noisy = build_edge_noisy_buffer();
        let output = apply_simd(&noisy, w, h, 3.0, 30.0);

        // The left patch (first 20 rows, left half) is a flat region (base = 50) with noise.
        // Bilateral should reduce variance there.
        let noisy_var = compute_variance(&noisy, 0, 20 * 64);
        let output_var = compute_variance(&output, 0, 20 * 64);
        assert!(
            output_var < noisy_var,
            "bilateral should reduce variance in flat region: noisy={noisy_var:.4} output={output_var:.4}"
        );
    }
}
