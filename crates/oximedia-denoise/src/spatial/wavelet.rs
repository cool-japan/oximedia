//! Wavelet-based denoising.
//!
//! Wavelet denoising uses multi-resolution analysis to separate noise from
//! signal in the wavelet domain, applying thresholding to wavelet coefficients.

use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;
use rayon::prelude::*;

/// Thresholding method for wavelet coefficients.
#[derive(Clone, Copy, Debug)]
pub enum ThresholdMethod {
    /// Hard thresholding - zeros coefficients below threshold.
    Hard,
    /// Soft thresholding - shrinks coefficients towards zero.
    Soft,
    /// Garrote thresholding - non-negative variant.
    Garrote,
}

/// Apply wavelet denoising to a video frame.
///
/// Decomposes the frame into wavelet coefficients, applies thresholding
/// to remove noise, and reconstructs the denoised image.
///
/// # Arguments
/// * `frame` - Input video frame
/// * `strength` - Denoising strength (0.0 - 1.0)
/// * `method` - Thresholding method
///
/// # Returns
/// Filtered video frame
pub fn wavelet_denoise(
    frame: &VideoFrame,
    strength: f32,
    method: ThresholdMethod,
) -> DenoiseResult<VideoFrame> {
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

            wavelet_denoise_plane(
                input_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                strength,
                method,
            )
        })?;

    Ok(output)
}

/// Apply wavelet denoising to a single plane.
#[allow(clippy::too_many_arguments)]
fn wavelet_denoise_plane(
    input: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    strength: f32,
    method: ThresholdMethod,
) -> DenoiseResult<()> {
    // Convert to f32 for processing
    let mut coeffs: Vec<f32> = input.iter().map(|&x| f32::from(x)).collect();

    // Perform Haar wavelet transform (simplest wavelet)
    let levels = 2; // Number of decomposition levels
    for _level in 0..levels {
        haar_transform_2d(&mut coeffs, width, height, stride);
    }

    // Estimate threshold based on noise level
    let threshold = estimate_wavelet_threshold(&coeffs, strength);

    // Apply thresholding
    apply_threshold(&mut coeffs, threshold, method);

    // Inverse wavelet transform
    for _level in 0..levels {
        inverse_haar_transform_2d(&mut coeffs, width, height, stride);
    }

    // Convert back to u8
    for y in 0..height {
        for x in 0..width {
            output[y * stride + x] = coeffs[y * stride + x].round().clamp(0.0, 255.0) as u8;
        }
    }

    Ok(())
}

/// Perform 2D Haar wavelet transform.
fn haar_transform_2d(data: &mut [f32], width: usize, height: usize, stride: usize) {
    // Row-wise transform
    for y in 0..height {
        haar_transform_1d(&mut data[y * stride..y * stride + width]);
    }

    // Column-wise transform
    let mut col_buffer = vec![0.0f32; height];
    for x in 0..width {
        for y in 0..height {
            col_buffer[y] = data[y * stride + x];
        }
        haar_transform_1d(&mut col_buffer);
        for y in 0..height {
            data[y * stride + x] = col_buffer[y];
        }
    }
}

/// Perform 1D Haar wavelet transform.
fn haar_transform_1d(data: &mut [f32]) {
    let n = data.len();
    if n < 2 {
        return;
    }

    let mut temp = vec![0.0f32; n];
    let half = n / 2;

    // Compute averages and differences
    for i in 0..half {
        temp[i] = (data[2 * i] + data[2 * i + 1]) / std::f32::consts::SQRT_2;
        temp[half + i] = (data[2 * i] - data[2 * i + 1]) / std::f32::consts::SQRT_2;
    }

    data.copy_from_slice(&temp);
}

/// Perform 2D inverse Haar wavelet transform.
fn inverse_haar_transform_2d(data: &mut [f32], width: usize, height: usize, stride: usize) {
    // Column-wise inverse transform
    let mut col_buffer = vec![0.0f32; height];
    for x in 0..width {
        for y in 0..height {
            col_buffer[y] = data[y * stride + x];
        }
        inverse_haar_transform_1d(&mut col_buffer);
        for y in 0..height {
            data[y * stride + x] = col_buffer[y];
        }
    }

    // Row-wise inverse transform
    for y in 0..height {
        inverse_haar_transform_1d(&mut data[y * stride..y * stride + width]);
    }
}

/// Perform 1D inverse Haar wavelet transform.
fn inverse_haar_transform_1d(data: &mut [f32]) {
    let n = data.len();
    if n < 2 {
        return;
    }

    let mut temp = vec![0.0f32; n];
    let half = n / 2;

    // Reconstruct from averages and differences
    for i in 0..half {
        temp[2 * i] = (data[i] + data[half + i]) / std::f32::consts::SQRT_2;
        temp[2 * i + 1] = (data[i] - data[half + i]) / std::f32::consts::SQRT_2;
    }

    data.copy_from_slice(&temp);
}

/// Estimate threshold using universal threshold.
fn estimate_wavelet_threshold(coeffs: &[f32], strength: f32) -> f32 {
    // Estimate noise standard deviation using median absolute deviation
    let mut abs_coeffs: Vec<f32> = coeffs.iter().map(|&x| x.abs()).collect();
    abs_coeffs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let median = if abs_coeffs.is_empty() {
        0.0
    } else {
        abs_coeffs[abs_coeffs.len() / 2]
    };

    let sigma = median / 0.6745; // MAD estimator

    // Universal threshold
    let n = coeffs.len() as f32;
    sigma * (2.0 * n.ln()).sqrt() * strength
}

/// Apply thresholding to wavelet coefficients.
fn apply_threshold(coeffs: &mut [f32], threshold: f32, method: ThresholdMethod) {
    for coeff in coeffs.iter_mut() {
        *coeff = match method {
            ThresholdMethod::Hard => hard_threshold(*coeff, threshold),
            ThresholdMethod::Soft => soft_threshold(*coeff, threshold),
            ThresholdMethod::Garrote => garrote_threshold(*coeff, threshold),
        };
    }
}

/// Hard thresholding function.
fn hard_threshold(x: f32, threshold: f32) -> f32 {
    if x.abs() > threshold {
        x
    } else {
        0.0
    }
}

/// Soft thresholding function.
fn soft_threshold(x: f32, threshold: f32) -> f32 {
    if x.abs() > threshold {
        x.signum() * (x.abs() - threshold)
    } else {
        0.0
    }
}

/// Garrote thresholding function.
fn garrote_threshold(x: f32, threshold: f32) -> f32 {
    if x.abs() > threshold {
        x - (threshold * threshold / x)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_wavelet_denoise() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = wavelet_denoise(&frame, 0.5, ThresholdMethod::Soft);
        assert!(result.is_ok());

        let filtered = result.expect("filtered should be valid");
        assert_eq!(filtered.width, 64);
        assert_eq!(filtered.height, 64);
    }

    #[test]
    fn test_haar_transform() {
        let mut data = vec![1.0, 2.0, 3.0, 4.0];
        haar_transform_1d(&mut data);
        inverse_haar_transform_1d(&mut data);

        // Should reconstruct original (with some floating point error)
        assert!((data[0] - 1.0).abs() < 0.01);
        assert!((data[1] - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_threshold_methods() {
        assert_eq!(hard_threshold(5.0, 3.0), 5.0);
        assert_eq!(hard_threshold(2.0, 3.0), 0.0);

        assert_eq!(soft_threshold(5.0, 3.0), 2.0);
        assert_eq!(soft_threshold(2.0, 3.0), 0.0);

        assert!(garrote_threshold(5.0, 3.0) > 0.0);
    }

    #[test]
    fn test_wavelet_all_methods() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        frame.allocate();

        for method in [
            ThresholdMethod::Hard,
            ThresholdMethod::Soft,
            ThresholdMethod::Garrote,
        ] {
            let result = wavelet_denoise(&frame, 0.5, method);
            assert!(result.is_ok());
        }
    }
}
