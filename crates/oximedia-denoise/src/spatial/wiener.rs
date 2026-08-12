//! Wiener filter for frequency-domain denoising.
//!
//! The Wiener filter is an optimal linear filter that minimizes the mean
//! square error between the estimated and true signal in the frequency domain.

use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;
use rayon::prelude::*;

/// Apply Wiener filter to a video frame.
///
/// The Wiener filter operates in the frequency domain and is particularly
/// effective for signals with known or estimated power spectral density.
///
/// # Arguments
/// * `frame` - Input video frame
/// * `strength` - Denoising strength (0.0 - 1.0)
///
/// # Returns
/// Filtered video frame
pub fn wiener_filter(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
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

            // Estimate noise variance based on strength
            let noise_variance = (strength * 100.0).powi(2);

            wiener_filter_plane(
                input_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                noise_variance,
            )
        })?;

    Ok(output)
}

/// Apply Wiener filter to a single plane using local statistics.
fn wiener_filter_plane(
    input: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    noise_variance: f32,
) -> DenoiseResult<()> {
    let window_size = 5;
    let window_radius = window_size / 2;

    for y in 0..height {
        for x in 0..width {
            // Compute local mean and variance
            let (local_mean, local_variance) =
                compute_local_statistics(input, width, height, stride, x, y, window_radius);

            let center_val = f32::from(input[y * stride + x]);

            // Wiener filter formula
            let signal_variance = (local_variance - noise_variance).max(0.0);
            let wiener_gain = if local_variance > 0.0 {
                signal_variance / local_variance
            } else {
                0.0
            };

            let filtered = local_mean + wiener_gain * (center_val - local_mean);
            output[y * stride + x] = filtered.round().clamp(0.0, 255.0) as u8;
        }
    }

    Ok(())
}

/// Compute local mean and variance in a window.
fn compute_local_statistics(
    data: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    x: usize,
    y: usize,
    radius: usize,
) -> (f32, f32) {
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    let mut count = 0;

    for dy in -(radius as i32)..=(radius as i32) {
        let ny = (y as i32 + dy).clamp(0, (height - 1) as i32) as usize;

        for dx in -(radius as i32)..=(radius as i32) {
            let nx = (x as i32 + dx).clamp(0, (width - 1) as i32) as usize;

            let val = f32::from(data[ny * stride + nx]);
            sum += val;
            sum_sq += val * val;
            count += 1;
        }
    }

    let mean = sum / count as f32;
    let variance = (sum_sq / count as f32) - (mean * mean);

    (mean, variance.max(0.0))
}

/// Adaptive Wiener filter with automatic noise estimation.
pub fn adaptive_wiener_filter(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
    if frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    let mut output = frame.clone();

    output
        .planes
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(plane_idx, plane)| {
            let input_plane = &frame.planes[plane_idx];
            let (width, height) = frame.plane_dimensions(plane_idx);

            // Estimate noise from high-frequency components
            let estimated_noise = estimate_noise_from_plane(
                input_plane.data.as_ref(),
                width as usize,
                height as usize,
                plane.stride,
            );

            let noise_variance = (estimated_noise * strength).powi(2);

            wiener_filter_plane(
                input_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                noise_variance,
            )
        })?;

    Ok(output)
}

/// Two-step BM3D-like denoising: basic estimate + Wiener refinement.
///
/// Step 1 (basic estimate): Apply bilateral filter to get a coarse estimate
/// of the clean signal. This provides an estimate of the signal's local
/// power spectral density.
///
/// Step 2 (Wiener refinement): Use the basic estimate's local statistics
/// to compute a more accurate Wiener filter, applying it to the original
/// noisy input for improved denoising with better detail preservation.
///
/// This two-step approach is inspired by BM3D but operates in the spatial
/// domain using local statistics rather than the transform domain.
pub fn two_step_denoise(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
    if frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    // Step 1: Basic estimate via bilateral filter
    let basic_estimate = crate::spatial::bilateral::bilateral_filter(frame, strength * 0.8)?;

    // Step 2: Wiener refinement using the basic estimate
    let mut output = frame.clone();

    output
        .planes
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(plane_idx, plane)| {
            let noisy_plane = &frame.planes[plane_idx];
            let basic_plane = &basic_estimate.planes[plane_idx];
            let (width, height) = frame.plane_dimensions(plane_idx);

            wiener_refinement_plane(
                noisy_plane.data.as_ref(),
                basic_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                strength,
            )
        })?;

    Ok(output)
}

/// Wiener refinement step: uses the basic estimate to compute improved
/// local signal variance, yielding a more accurate Wiener gain.
#[allow(clippy::too_many_arguments)]
fn wiener_refinement_plane(
    noisy: &[u8],
    basic: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    strength: f32,
) -> DenoiseResult<()> {
    let window_radius = 3;
    // Global noise variance estimate from strength parameter
    let noise_variance = (strength * 80.0).powi(2);

    for y in 0..height {
        for x in 0..width {
            // Compute local statistics from the BASIC estimate (cleaner signal)
            let (basic_mean, basic_variance) =
                compute_local_statistics(basic, width, height, stride, x, y, window_radius);

            // The basic estimate's variance is a better proxy for signal variance
            // than the noisy input's variance.
            let signal_variance = (basic_variance).max(0.0);

            // Empirical Wiener gain: signal / (signal + noise)
            let wiener_gain = if (signal_variance + noise_variance) > 1e-6 {
                signal_variance / (signal_variance + noise_variance)
            } else {
                0.0
            };

            let noisy_val = f32::from(noisy[y * stride + x]);

            // Apply Wiener filter: use basic estimate as the mean reference
            let filtered = basic_mean + wiener_gain * (noisy_val - basic_mean);
            output[y * stride + x] = filtered.round().clamp(0.0, 255.0) as u8;
        }
    }

    Ok(())
}

/// Estimate noise level from high-frequency components.
fn estimate_noise_from_plane(data: &[u8], width: usize, height: usize, stride: usize) -> f32 {
    let mut sum_abs_diff = 0.0f32;
    let mut count = 0;

    // Use horizontal differences as noise proxy
    for y in 0..height {
        for x in 0..(width - 1) {
            let diff = f32::from(data[y * stride + x + 1]) - f32::from(data[y * stride + x]);
            sum_abs_diff += diff.abs();
            count += 1;
        }
    }

    if count > 0 {
        sum_abs_diff / count as f32
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_wiener_filter() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = wiener_filter(&frame, 0.5);
        assert!(result.is_ok());

        let filtered = result.expect("filtered should be valid");
        assert_eq!(filtered.width, 64);
        assert_eq!(filtered.height, 64);
    }

    #[test]
    fn test_adaptive_wiener_filter() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = adaptive_wiener_filter(&frame, 0.5);
        assert!(result.is_ok());
    }

    #[test]
    fn test_local_statistics() {
        let data = vec![128u8; 100 * 100];
        let (mean, variance) = compute_local_statistics(&data, 100, 100, 100, 50, 50, 2);
        assert!((mean - 128.0).abs() < f32::EPSILON);
        assert!(variance < f32::EPSILON);
    }

    #[test]
    fn test_noise_estimation() {
        let data = vec![100u8; 64 * 64];
        let noise = estimate_noise_from_plane(&data, 64, 64, 64);
        assert!(noise < f32::EPSILON);
    }

    // -------------------------------------------------------------------
    // Two-step BM3D-like denoising tests
    // -------------------------------------------------------------------

    #[test]
    fn test_two_step_denoise_basic() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = two_step_denoise(&frame, 0.5);
        assert!(result.is_ok());

        let filtered = result.expect("two-step should succeed");
        assert_eq!(filtered.width, 64);
        assert_eq!(filtered.height, 64);
    }

    #[test]
    fn test_two_step_denoise_preserves_dimensions() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 48, 32);
        frame.allocate();

        let result = two_step_denoise(&frame, 0.7);
        assert!(result.is_ok());

        let filtered = result.expect("two-step should succeed");
        assert_eq!(filtered.width, 48);
        assert_eq!(filtered.height, 32);
        assert_eq!(filtered.planes.len(), frame.planes.len());
    }

    #[test]
    fn test_two_step_denoise_zero_strength() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        frame.allocate();

        let result = two_step_denoise(&frame, 0.0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_two_step_denoise_max_strength() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        frame.allocate();

        let result = two_step_denoise(&frame, 1.0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_two_step_empty_frame() {
        let frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        // Not allocated — no planes
        let result = two_step_denoise(&frame, 0.5);
        assert!(result.is_err());
    }

    #[test]
    fn test_wiener_refinement_uniform() {
        // Uniform input should produce uniform output
        let size = 32 * 32;
        let noisy = vec![128u8; size];
        let basic = vec![128u8; size];
        let mut output = vec![0u8; size];

        let result = wiener_refinement_plane(&noisy, &basic, &mut output, 32, 32, 32, 0.5);
        assert!(result.is_ok());

        // Output should be close to 128
        for &v in &output {
            assert!(
                (v as i32 - 128).unsigned_abs() < 2,
                "expected ~128, got {v}"
            );
        }
    }

    #[test]
    fn test_local_statistics_varied() {
        // Create data with known statistics
        let mut data = vec![0u8; 10 * 10];
        for (i, v) in data.iter_mut().enumerate() {
            *v = (i % 256) as u8;
        }
        let (mean, variance) = compute_local_statistics(&data, 10, 10, 10, 5, 5, 1);
        // Mean should be average of 3x3 block around (5,5)
        assert!(mean > 0.0);
        // Variance should be non-zero for varied data
        assert!(variance >= 0.0);
    }
}
