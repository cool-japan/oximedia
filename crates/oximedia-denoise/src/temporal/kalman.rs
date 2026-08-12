//! Kalman filter for temporal video denoising.
//!
//! The Kalman filter provides optimal recursive estimation for linear
//! systems, performing prediction and correction steps to track the
//! true signal while filtering out noise.

use crate::{DenoiseError, DenoiseResult};
use oximedia_codec::VideoFrame;

/// Kalman filter state for video denoising.
pub struct KalmanState {
    /// Estimated state (denoised frame).
    pub estimate: Vec<f32>,
    /// Estimation error covariance.
    pub error_covariance: Vec<f32>,
    /// Frame dimensions.
    pub width: usize,
    /// Frame height.
    pub height: usize,
}

impl KalmanState {
    /// Create a new Kalman filter state.
    pub fn new(width: usize, height: usize) -> Self {
        let size = width * height;
        Self {
            estimate: vec![0.0; size],
            error_covariance: vec![1.0; size],
            width,
            height,
        }
    }

    /// Initialize from a frame.
    pub fn initialize(&mut self, frame: &VideoFrame, plane_idx: usize) {
        let plane = &frame.planes[plane_idx];
        let (width, height) = frame.plane_dimensions(plane_idx);

        self.width = width as usize;
        self.height = height as usize;
        let size = self.width * self.height;

        self.estimate.clear();
        self.error_covariance.clear();
        self.estimate.resize(size, 0.0);
        self.error_covariance.resize(size, 1.0);

        // Initialize with frame data
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                self.estimate[idx] = f32::from(plane.data[y * plane.stride + x]);
            }
        }
    }
}

/// Apply Kalman filter for temporal denoising.
///
/// Uses a simple Kalman filter model where:
/// - State transition is identity (assumes static scene)
/// - Measurement is the current frame
///
/// # Arguments
/// * `frame` - Current frame to process
/// * `state` - Kalman filter state (will be updated)
/// * `process_noise` - Process noise variance (scene change sensitivity)
/// * `measurement_noise` - Measurement noise variance (noise level)
///
/// # Returns
/// Denoised frame
pub fn kalman_filter(
    frame: &VideoFrame,
    state: &mut KalmanState,
    process_noise: f32,
    measurement_noise: f32,
) -> DenoiseResult<VideoFrame> {
    if frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    let mut output = frame.clone();

    // Process each plane
    for (plane_idx, plane) in output.planes.iter_mut().enumerate() {
        let input_plane = &frame.planes[plane_idx];
        let (width, height) = frame.plane_dimensions(plane_idx);

        // Initialize state if needed
        if state.estimate.len() != width as usize * height as usize {
            state.initialize(frame, plane_idx);
        }

        kalman_filter_plane(
            input_plane.data.as_ref(),
            plane.data.as_mut(),
            state,
            width as usize,
            height as usize,
            plane.stride,
            process_noise,
            measurement_noise,
        )?;
    }

    Ok(output)
}

/// Apply Kalman filter to a single plane.
#[allow(clippy::too_many_arguments)]
fn kalman_filter_plane(
    input: &[u8],
    output: &mut [u8],
    state: &mut KalmanState,
    width: usize,
    height: usize,
    stride: usize,
    process_noise: f32,
    measurement_noise: f32,
) -> DenoiseResult<()> {
    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            let measurement = f32::from(input[y * stride + x]);

            // Prediction step
            let predicted_estimate = state.estimate[idx];
            let predicted_error = state.error_covariance[idx] + process_noise;

            // Update step
            let kalman_gain = predicted_error / (predicted_error + measurement_noise);
            let new_estimate =
                predicted_estimate + kalman_gain * (measurement - predicted_estimate);
            let new_error = (1.0 - kalman_gain) * predicted_error;

            // Update state
            state.estimate[idx] = new_estimate;
            state.error_covariance[idx] = new_error;

            // Output filtered value
            output[y * stride + x] = new_estimate.round().clamp(0.0, 255.0) as u8;
        }
    }

    Ok(())
}

/// Adaptive Kalman filter with automatic noise estimation.
pub fn adaptive_kalman_filter(
    frame: &VideoFrame,
    state: &mut KalmanState,
) -> DenoiseResult<VideoFrame> {
    // Estimate noise from frame variance
    let noise_estimate = estimate_noise_level(frame)?;

    let process_noise = (noise_estimate * 0.1).powi(2);
    let measurement_noise = (noise_estimate).powi(2);

    kalman_filter(frame, state, process_noise, measurement_noise)
}

/// Estimate noise level from a frame.
fn estimate_noise_level(frame: &VideoFrame) -> DenoiseResult<f32> {
    if frame.planes.is_empty() {
        return Err(DenoiseError::ProcessingError(
            "Frame has no planes".to_string(),
        ));
    }

    let plane = &frame.planes[0];
    let (width, height) = frame.plane_dimensions(0);

    let mut sum = 0.0f64;
    let mut count = 0;

    // Use Laplacian variance for noise estimation
    for y in 1..(height as usize - 1) {
        for x in 1..(width as usize - 1) {
            let idx = y * plane.stride + x;
            let center = f64::from(plane.data[idx]);

            let laplacian = 4.0 * center
                - f64::from(plane.data[idx - 1])
                - f64::from(plane.data[idx + 1])
                - f64::from(plane.data[idx - plane.stride])
                - f64::from(plane.data[idx + plane.stride]);

            sum += laplacian * laplacian;
            count += 1;
        }
    }

    let variance = if count > 0 {
        sum / f64::from(count)
    } else {
        0.0
    };
    let noise = (variance.sqrt() / 2.0) as f32;

    Ok(noise)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::PixelFormat;

    #[test]
    fn test_kalman_state_creation() {
        let state = KalmanState::new(64, 64);
        assert_eq!(state.estimate.len(), 64 * 64);
        assert_eq!(state.error_covariance.len(), 64 * 64);
    }

    #[test]
    fn test_kalman_filter() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let mut state = KalmanState::new(64, 64);

        let result = kalman_filter(&frame, &mut state, 1.0, 10.0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_adaptive_kalman_filter() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let mut state = KalmanState::new(64, 64);

        let result = adaptive_kalman_filter(&frame, &mut state);
        assert!(result.is_ok());
    }

    #[test]
    fn test_noise_estimation() {
        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 64, 64);
        frame.allocate();

        let result = estimate_noise_level(&frame);
        assert!(result.is_ok());
    }

    #[test]
    fn test_kalman_multiple_frames() {
        let mut state = KalmanState::new(32, 32);

        for _ in 0..10 {
            let mut frame = VideoFrame::new(PixelFormat::Yuv420p, 32, 32);
            frame.allocate();

            let result = kalman_filter(&frame, &mut state, 1.0, 10.0);
            assert!(result.is_ok());
        }
    }
}
