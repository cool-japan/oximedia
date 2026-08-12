//! Temporal averaging denoising.
//!
//! Temporal averaging reduces noise by computing weighted averages of
//! corresponding pixels across multiple frames in a sequence.

use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;
use rayon::prelude::*;

/// Apply temporal averaging to a sequence of frames.
///
/// Computes a weighted average of corresponding pixels across frames,
/// with weights typically based on temporal distance and pixel similarity.
///
/// # Arguments
/// * `current_frame` - The current frame to denoise
/// * `frame_buffer` - Buffer of recent frames
/// * `strength` - Denoising strength (0.0 - 1.0)
///
/// # Returns
/// Denoised frame
pub fn temporal_average(
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

            temporal_average_plane(
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

/// Apply temporal averaging to a single plane.
fn temporal_average_plane(
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
            let center_val = f32::from(input_planes[center_idx][idx]);

            let mut sum = 0.0f32;
            let mut weight_sum = 0.0f32;

            // Compute weighted average across frames
            for (frame_idx, plane) in input_planes.iter().enumerate() {
                let val = f32::from(plane[idx]);

                // Temporal weight (exponential decay from center)
                let time_dist = (frame_idx as i32 - center_idx as i32).abs() as f32;
                let temporal_weight = (-time_dist / (strength * 2.0 + 0.1)).exp();

                // Pixel similarity weight
                let value_diff = (val - center_val).abs();
                let similarity_weight = (-(value_diff / (strength * 20.0 + 0.1)).powi(2)).exp();

                let weight = temporal_weight * similarity_weight;

                sum += val * weight;
                weight_sum += weight;
            }

            output[idx] = if weight_sum > 0.0 {
                (sum / weight_sum).round().clamp(0.0, 255.0) as u8
            } else {
                center_val as u8
            };
        }
    }

    Ok(())
}

/// Exponential temporal averaging (EMA-based).
pub fn exponential_temporal_average(
    current_frame: &VideoFrame,
    previous_average: Option<&VideoFrame>,
    alpha: f32,
) -> DenoiseResult<VideoFrame> {
    let Some(prev) = previous_average else {
        return Ok(current_frame.clone());
    };

    if current_frame.planes.is_empty() || prev.planes.is_empty() {
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
            let current_plane = &current_frame.planes[plane_idx];
            let prev_plane = &prev.planes[plane_idx];
            let (width, height) = current_frame.plane_dimensions(plane_idx);

            exponential_average_plane(
                current_plane.data.as_ref(),
                prev_plane.data.as_ref(),
                plane.data.as_mut(),
                width as usize,
                height as usize,
                plane.stride,
                alpha,
            )
        })?;

    Ok(output)
}

/// Apply exponential moving average to a single plane.
#[allow(clippy::too_many_arguments)]
fn exponential_average_plane(
    current: &[u8],
    previous: &[u8],
    output: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    alpha: f32,
) -> DenoiseResult<()> {
    for y in 0..height {
        for x in 0..width {
            let idx = y * stride + x;
            let curr_val = f32::from(current[idx]);
            let prev_val = f32::from(previous[idx]);

            // EMA: output = alpha * current + (1 - alpha) * previous
            let blended = alpha * curr_val + (1.0 - alpha) * prev_val;
            output[idx] = blended.round().clamp(0.0, 255.0) as u8;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_temporal_average() {
        let mut frames = Vec::new();
        for _ in 0..5 {
            let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
            frame.allocate();
            frames.push(frame);
        }

        let result = temporal_average(&frames[2], &frames, 0.5);
        assert!(result.is_ok());
    }

    #[test]
    fn test_temporal_average_empty_buffer() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = temporal_average(&frame, &[], 0.5);
        assert!(result.is_ok());
    }

    #[test]
    fn test_exponential_temporal_average() {
        let mut current = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        current.allocate();

        let mut previous = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        previous.allocate();

        let result = exponential_temporal_average(&current, Some(&previous), 0.3);
        assert!(result.is_ok());
    }

    #[test]
    fn test_exponential_temporal_average_no_previous() {
        let mut current = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        current.allocate();

        let result = exponential_temporal_average(&current, None, 0.3);
        assert!(result.is_ok());
    }
}
