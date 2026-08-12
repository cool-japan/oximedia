//! Temporal median filter for denoising.
//!
//! Temporal median filtering reduces noise by computing the median value
//! of corresponding pixels across multiple frames, effectively removing
//! impulse noise and temporal outliers.

use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;
use rayon::prelude::*;

/// Apply temporal median filter to a sequence of frames.
///
/// Computes the median of corresponding pixels across frames, which is
/// particularly effective against salt-and-pepper noise and temporal artifacts.
///
/// # Arguments
/// * `current_frame` - The current frame to denoise
/// * `frame_buffer` - Buffer of recent frames
///
/// # Returns
/// Denoised frame
pub fn temporal_median(
    current_frame: &VideoFrame,
    frame_buffer: &[VideoFrame],
) -> DenoiseResult<VideoFrame> {
    if frame_buffer.is_empty() {
        return Ok(current_frame.clone());
    }

    if current_frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    let mut output = current_frame.clone();

    // Process each plane in parallel
    output
        .planes
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(plane_idx, plane)| {
            let (width, height) = current_frame.plane_dimensions(plane_idx);

            // Collect corresponding planes from all frames
            let planes: Vec<&[u8]> = frame_buffer
                .iter()
                .map(|f| f.planes[plane_idx].data.as_ref())
                .collect();

            temporal_median_plane(
                &planes,
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
            )
        })?;

    Ok(output)
}

/// Apply temporal median to a single plane.
fn temporal_median_plane(
    input_planes: &[&[u8]],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
) -> DenoiseResult<()> {
    if input_planes.is_empty() {
        return Ok(());
    }

    let num_frames = input_planes.len();
    let mut pixel_values = vec![0u8; num_frames];

    for y in 0..height {
        for x in 0..width {
            let idx = y * stride + x;

            // Collect values from all frames
            for (i, plane) in input_planes.iter().enumerate() {
                pixel_values[i] = plane[idx];
            }

            // Compute median
            output[idx] = median(&mut pixel_values);
        }
    }

    Ok(())
}

/// Compute median of a slice (modifies the slice).
fn median(values: &mut [u8]) -> u8 {
    if values.is_empty() {
        return 0;
    }

    values.sort_unstable();
    let mid = values.len() / 2;

    if values.len() % 2 == 0 {
        // Average of two middle values
        ((u16::from(values[mid - 1]) + u16::from(values[mid])) / 2) as u8
    } else {
        values[mid]
    }
}

/// Weighted temporal median filter.
///
/// Similar to temporal median but applies weights to frames based on
/// their temporal distance from the current frame.
pub fn weighted_temporal_median(
    current_frame: &VideoFrame,
    frame_buffer: &[VideoFrame],
    strength: f32,
) -> DenoiseResult<VideoFrame> {
    if frame_buffer.is_empty() {
        return Ok(current_frame.clone());
    }

    if current_frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    let mut output = current_frame.clone();

    output
        .planes
        .par_iter_mut()
        .enumerate()
        .try_for_each(|(plane_idx, plane)| {
            let (width, height) = current_frame.plane_dimensions(plane_idx);

            let planes: Vec<&[u8]> = frame_buffer
                .iter()
                .map(|f| f.planes[plane_idx].data.as_ref())
                .collect();

            weighted_temporal_median_plane(
                &planes,
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                strength,
            )
        })?;

    Ok(output)
}

/// Apply weighted temporal median to a single plane.
fn weighted_temporal_median_plane(
    input_planes: &[&[u8]],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    strength: f32,
) -> DenoiseResult<()> {
    if input_planes.is_empty() {
        return Ok(());
    }

    let num_frames = input_planes.len();
    let center_idx = num_frames / 2;

    for y in 0..height {
        for x in 0..width {
            let idx = y * stride + x;

            // Collect weighted values
            let mut weighted_values = Vec::new();

            for (frame_idx, plane) in input_planes.iter().enumerate() {
                let val = plane[idx];

                // Compute weight based on temporal distance
                let time_dist = (frame_idx as i32 - center_idx as i32).abs() as f32;
                let weight = (-time_dist / (strength * 2.0 + 0.1)).exp();

                // Replicate value based on weight
                let replications = (weight * 10.0).round() as usize + 1;
                for _ in 0..replications {
                    weighted_values.push(val);
                }
            }

            // Compute median of weighted values
            output[idx] = median(&mut weighted_values);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_temporal_median() {
        let mut frames = Vec::new();
        for _ in 0..5 {
            let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
            frame.allocate();
            frames.push(frame);
        }

        let result = temporal_median(&frames[2], &frames);
        assert!(result.is_ok());
    }

    #[test]
    fn test_temporal_median_empty_buffer() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = temporal_median(&frame, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_median_function() {
        let mut values = vec![5, 1, 4, 2, 3];
        assert_eq!(median(&mut values), 3);

        let mut values_even = vec![1, 2, 3, 4];
        assert_eq!(median(&mut values_even), 2);

        let mut single = vec![42];
        assert_eq!(median(&mut single), 42);
    }

    #[test]
    fn test_weighted_temporal_median() {
        let mut frames = Vec::new();
        for _ in 0..5 {
            let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
            frame.allocate();
            frames.push(frame);
        }

        let result = weighted_temporal_median(&frames[2], &frames, 0.5);
        assert!(result.is_ok());
    }
}
