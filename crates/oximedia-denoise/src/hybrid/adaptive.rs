//! Adaptive denoising based on local content analysis.
//!
//! Adaptive filtering adjusts denoising strength based on local image
//! characteristics such as variance, edges, and texture.

use crate::spatial::{bilateral, nlmeans};
use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;
use rayon::prelude::*;

/// Apply content-adaptive denoising.
///
/// Analyzes local image content and applies stronger denoising in smooth
/// regions while preserving detail in textured and edge regions.
///
/// # Arguments
/// * `frame` - Input video frame
/// * `strength` - Base denoising strength (0.0 - 1.0)
///
/// # Returns
/// Adaptively denoised frame
pub fn adaptive_denoise(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
    if frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    // Compute activity map (variance-based)
    let activity_map = compute_activity_map(frame);

    // Apply adaptive filtering
    let mut output = frame.clone();

    output
        .planes
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(plane_idx, plane)| {
            let input_plane = &frame.planes[plane_idx];
            let (width, height) = frame.plane_dimensions(plane_idx);

            adaptive_filter_plane(
                input_plane.data.as_ref(),
                plane.data.as_mut(),
                &activity_map,
                width as usize,
                height as usize,
                plane.stride,
                strength,
            )
        })?;

    Ok(output)
}

/// Compute activity map showing local variance.
fn compute_activity_map(frame: &VideoFrame) -> Vec<f32> {
    let plane = &frame.planes[0]; // Use luma plane
    let (width, height) = frame.plane_dimensions(0);
    let window_size = 7;
    let radius = window_size / 2;

    let mut activity_map = vec![0.0f32; width as usize * height as usize];

    for y in 0..(height as usize) {
        for x in 0..(width as usize) {
            let variance = compute_local_variance(
                plane.data.as_ref(),
                x,
                y,
                width as usize,
                height as usize,
                plane.stride,
                radius,
            );
            activity_map[y * width as usize + x] = variance.sqrt();
        }
    }

    activity_map
}

/// Compute local variance in a window.
#[allow(clippy::too_many_arguments)]
fn compute_local_variance(
    data: &[u8],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    stride: usize,
    radius: usize,
) -> f32 {
    let mut sum = 0.0f64;
    let mut sum_sq = 0.0f64;
    let mut count = 0;

    for dy in -(radius as i32)..=(radius as i32) {
        let ny = (y as i32 + dy).clamp(0, (height - 1) as i32) as usize;

        for dx in -(radius as i32)..=(radius as i32) {
            let nx = (x as i32 + dx).clamp(0, (width - 1) as i32) as usize;

            let val = f64::from(data[ny * stride + nx]);
            sum += val;
            sum_sq += val * val;
            count += 1;
        }
    }

    let mean = sum / f64::from(count);
    let variance = (sum_sq / f64::from(count)) - (mean * mean);

    variance.max(0.0) as f32
}

/// Apply adaptive filtering to a single plane.
#[allow(clippy::too_many_arguments, clippy::unnecessary_wraps)]
fn adaptive_filter_plane(
    input: &[u8],
    output: &mut [u8],
    activity_map: &[f32],
    width: usize,
    height: usize,
    stride: usize,
    base_strength: f32,
) -> DenoiseResult<()> {
    let window_size = 5;
    let radius = window_size / 2;

    for y in 0..height {
        for x in 0..width {
            let activity = activity_map[y * width + x];

            // Lower activity (smooth regions) -> higher denoising
            // Higher activity (edges/texture) -> lower denoising
            let normalized_activity = (activity / 30.0).clamp(0.0, 1.0);
            let local_strength = base_strength * (1.0 - normalized_activity);

            // Apply local filtering
            let filtered =
                apply_local_filter(input, x, y, width, height, stride, radius, local_strength);

            output[y * stride + x] = filtered;
        }
    }

    Ok(())
}

/// Apply local bilateral-style filter.
#[allow(clippy::too_many_arguments)]
fn apply_local_filter(
    data: &[u8],
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    stride: usize,
    radius: usize,
    strength: f32,
) -> u8 {
    let center_val = f32::from(data[y * stride + x]);
    let sigma_range = 20.0 * strength;
    let range_coeff = -0.5 / (sigma_range * sigma_range);

    let mut sum = 0.0f32;
    let mut weight_sum = 0.0f32;

    for dy in -(radius as i32)..=(radius as i32) {
        let ny = (y as i32 + dy).clamp(0, (height - 1) as i32) as usize;

        for dx in -(radius as i32)..=(radius as i32) {
            let nx = (x as i32 + dx).clamp(0, (width - 1) as i32) as usize;

            let neighbor_val = f32::from(data[ny * stride + nx]);
            let value_diff = neighbor_val - center_val;
            let weight = (value_diff * value_diff * range_coeff).exp();

            sum += neighbor_val * weight;
            weight_sum += weight;
        }
    }

    if weight_sum > 0.0 {
        (sum / weight_sum).round().clamp(0.0, 255.0) as u8
    } else {
        data[y * stride + x]
    }
}

/// Adaptive denoising with automatic algorithm selection.
pub fn auto_adaptive_denoise(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
    // Analyze frame characteristics
    let avg_activity = analyze_frame_activity(frame)?;

    // Choose algorithm based on content
    if avg_activity < 10.0 {
        // Low detail - use stronger denoising
        nlmeans::fast_nlmeans_filter(frame, strength)
    } else if avg_activity < 30.0 {
        // Medium detail - use bilateral
        bilateral::bilateral_filter(frame, strength)
    } else {
        // High detail - use adaptive
        adaptive_denoise(frame, strength * 0.7)
    }
}

/// Analyze average activity level of a frame.
fn analyze_frame_activity(frame: &VideoFrame) -> DenoiseResult<f32> {
    let activity_map = compute_activity_map(frame);

    let sum: f32 = activity_map.iter().sum();
    let avg = sum / activity_map.len() as f32;

    Ok(avg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_adaptive_denoise() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = adaptive_denoise(&frame, 0.5);
        assert!(result.is_ok());

        let filtered = result.expect("filtered should be valid");
        assert_eq!(filtered.width, 64);
        assert_eq!(filtered.height, 64);
    }

    #[test]
    fn test_activity_map() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let activity = compute_activity_map(&frame);
        assert_eq!(activity.len(), 64 * 64);
    }

    #[test]
    fn test_auto_adaptive_denoise() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = auto_adaptive_denoise(&frame, 0.5);
        assert!(result.is_ok());
    }

    #[test]
    fn test_frame_activity_analysis() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = analyze_frame_activity(&frame);
        assert!(result.is_ok());

        let activity = result.expect("activity should be valid");
        assert!(activity >= 0.0);
    }

    #[test]
    fn test_local_variance() {
        let data = vec![128u8; 64 * 64];
        let variance = compute_local_variance(&data, 32, 32, 64, 64, 64, 3);
        assert!(variance < f32::EPSILON);
    }
}
