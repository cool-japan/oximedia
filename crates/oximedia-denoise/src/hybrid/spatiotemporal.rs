//! Spatio-temporal denoising filter.
//!
//! Combines spatial and temporal denoising to leverage both within-frame
//! and across-frame redundancy for superior noise reduction.

use crate::spatial::bilateral;
use crate::temporal::average;
use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;
use rayon::prelude::*;

/// Apply combined spatio-temporal denoising.
///
/// First applies temporal averaging across frames, then spatial denoising
/// to the result for comprehensive noise reduction.
///
/// # Arguments
/// * `current_frame` - The current frame to denoise
/// * `frame_buffer` - Buffer of recent frames
/// * `strength` - Denoising strength (0.0 - 1.0)
/// * `preserve_edges` - Whether to preserve edges
///
/// # Returns
/// Denoised frame
pub fn spatio_temporal_denoise(
    current_frame: &VideoFrame,
    frame_buffer: &[VideoFrame],
    strength: f32,
    preserve_edges: bool,
) -> DenoiseResult<VideoFrame> {
    // Apply temporal denoising first
    let temporal_result = if frame_buffer.len() >= 3 {
        average::temporal_average(current_frame, frame_buffer, strength * 0.6)?
    } else {
        current_frame.clone()
    };

    // Apply spatial denoising
    if preserve_edges {
        bilateral::bilateral_filter(&temporal_result, strength * 0.7)
    } else {
        spatial_box_filter(&temporal_result, strength)
    }
}

/// Simple box filter for spatial denoising.
fn spatial_box_filter(frame: &VideoFrame, strength: f32) -> DenoiseResult<VideoFrame> {
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

            let kernel_size = (strength * 5.0).round() as usize + 1;
            let kernel_size = if kernel_size % 2 == 0 {
                kernel_size + 1
            } else {
                kernel_size
            };

            box_filter_plane(
                input_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                kernel_size,
            )
        })?;

    Ok(output)
}

/// Apply box filter to a single plane.
fn box_filter_plane(
    input: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    kernel_size: usize,
) -> DenoiseResult<()> {
    let radius = kernel_size / 2;

    for y in 0..height {
        for x in 0..width {
            let mut sum = 0u32;
            let mut count = 0u32;

            for ky in -(radius as i32)..=(radius as i32) {
                let ny = (y as i32 + ky).clamp(0, (height - 1) as i32) as usize;

                for kx in -(radius as i32)..=(radius as i32) {
                    let nx = (x as i32 + kx).clamp(0, (width - 1) as i32) as usize;

                    sum += u32::from(input[ny * stride + nx]);
                    count += 1;
                }
            }

            output[y * stride + x] = (sum / count) as u8;
        }
    }

    Ok(())
}

/// Adaptive spatio-temporal denoising with motion detection.
pub fn adaptive_spatio_temporal(
    current_frame: &VideoFrame,
    frame_buffer: &[VideoFrame],
    strength: f32,
) -> DenoiseResult<VideoFrame> {
    if frame_buffer.len() < 2 {
        return bilateral::bilateral_filter(current_frame, strength);
    }

    // Detect motion/scene change
    let motion_amount = detect_motion(current_frame, &frame_buffer[frame_buffer.len() - 2]);

    // Adjust temporal/spatial balance based on motion
    let temporal_strength = strength * (1.0 - motion_amount);
    let spatial_strength = strength * (0.5 + 0.5 * motion_amount);

    // Apply temporal filtering
    let temporal_result = if motion_amount < 0.8 {
        average::temporal_average(current_frame, frame_buffer, temporal_strength)?
    } else {
        current_frame.clone()
    };

    // Apply spatial filtering
    bilateral::bilateral_filter(&temporal_result, spatial_strength)
}

/// Detect motion between two frames (returns 0.0 for no motion, 1.0 for high motion).
fn detect_motion(current: &VideoFrame, previous: &VideoFrame) -> f32 {
    if current.planes.is_empty() || previous.planes.is_empty() {
        return 0.0;
    }

    let current_plane = &current.planes[0];
    let previous_plane = &previous.planes[0];
    let (width, height) = current.plane_dimensions(0);

    let mut diff_sum = 0u64;
    let mut count = 0u64;

    for y in 0..(height as usize) {
        for x in 0..(width as usize) {
            let idx = y * current_plane.stride + x;
            let diff =
                (i32::from(current_plane.data[idx]) - i32::from(previous_plane.data[idx])).abs();
            diff_sum += diff as u64;
            count += 1;
        }
    }

    let avg_diff = if count > 0 {
        diff_sum as f32 / count as f32
    } else {
        0.0
    };

    // Normalize to 0-1 range (assuming max meaningful difference is 50)
    (avg_diff / 50.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_spatio_temporal_denoise() {
        let mut frames = Vec::new();
        for _ in 0..5 {
            let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
            frame.allocate();
            frames.push(frame);
        }

        let result = spatio_temporal_denoise(&frames[2], &frames, 0.5, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_spatio_temporal_few_frames() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let frames = vec![frame.clone()];

        let result = spatio_temporal_denoise(&frame, &frames, 0.5, true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_adaptive_spatio_temporal() {
        let mut frames = Vec::new();
        for _ in 0..3 {
            let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
            frame.allocate();
            frames.push(frame);
        }

        let result = adaptive_spatio_temporal(&frames[2], &frames, 0.5);
        assert!(result.is_ok());
    }

    #[test]
    fn test_motion_detection() {
        let mut current = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        current.allocate();

        let mut previous = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        previous.allocate();

        let motion = detect_motion(&current, &previous);
        assert!((0.0..=1.0).contains(&motion));
    }

    #[test]
    fn test_box_filter() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
        frame.allocate();

        let result = spatial_box_filter(&frame, 0.5);
        assert!(result.is_ok());
    }
}
