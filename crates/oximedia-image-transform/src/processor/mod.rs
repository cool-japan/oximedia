// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Image transformation processing pipeline.
//!
//! Implements the actual pixel-level image transformations using pure Rust algorithms
//! (no C/FFI dependencies). The pipeline processes operations in the correct order:
//!
//! **Decode -> Trim -> Resize/Crop -> Rotate -> Color adjustments -> Sharpen/Blur -> Border/Padding -> Encode**
//!
//! # Architecture
//!
//! The processor works with [`PixelBuffer`] -- a simple container for raw pixel data
//! in either RGB or RGBA layout. A [`TransformParams`] specification is converted
//! into an ordered `Vec<PipelineStep>` via [`build_pipeline`], then each step is
//! applied in sequence by [`apply_transforms`].
//!
//! All image processing algorithms (bilinear/Lanczos resize, separable Gaussian blur,
//! unsharp mask, rotation, gamma LUT, etc.) are implemented from scratch in pure Rust.

pub(crate) mod color;
pub(crate) mod filters;
pub(crate) mod geometry;
pub(crate) mod resize;
pub mod streaming;

#[cfg(test)]
mod tests;

use crate::transform::{
    Border, Color, FitMode, Gravity, OutputFormat, Padding, Rotation, TransformParams, Trim,
};

// Re-export public algorithm functions so crate users can access them directly
// via `oximedia_image_transform::processor::*`.
pub use filters::{gaussian_blur, unsharp_mask};
pub use geometry::calculate_crop_rect;
pub use resize::{bilinear_resize, lanczos_resize};

// ============================================================================
// PixelBuffer
// ============================================================================

/// A simple pixel buffer for processing.
///
/// Stores raw pixel data in row-major order with either RGB (3 channels)
/// or RGBA (4 channels) layout.
#[derive(Debug, Clone)]
pub struct PixelBuffer {
    /// Raw pixel data in row-major order (RGB or RGBA).
    pub data: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Number of channels per pixel: 3 (RGB) or 4 (RGBA).
    pub channels: u8,
}

impl PixelBuffer {
    /// Create a new pixel buffer filled with zeros.
    pub fn new(width: u32, height: u32, channels: u8) -> Self {
        let len = width as usize * height as usize * channels as usize;
        Self {
            data: vec![0u8; len],
            width,
            height,
            channels,
        }
    }

    /// Create a pixel buffer from existing RGBA data.
    ///
    /// Returns an error if the data length does not match `width * height * 4`.
    pub fn from_rgba(data: Vec<u8>, width: u32, height: u32) -> Result<Self, ProcessingError> {
        let expected = width as usize * height as usize * 4;
        if data.len() != expected {
            return Err(ProcessingError::InvalidDimensions {
                width,
                height,
                data_len: data.len(),
            });
        }
        Ok(Self {
            data,
            width,
            height,
            channels: 4,
        })
    }

    /// Create a pixel buffer from existing RGB data.
    ///
    /// Returns an error if the data length does not match `width * height * 3`.
    pub fn from_rgb(data: Vec<u8>, width: u32, height: u32) -> Result<Self, ProcessingError> {
        let expected = width as usize * height as usize * 3;
        if data.len() != expected {
            return Err(ProcessingError::InvalidDimensions {
                width,
                height,
                data_len: data.len(),
            });
        }
        Ok(Self {
            data,
            width,
            height,
            channels: 3,
        })
    }

    /// Get pixel at (x, y) as a slice of channel values.
    ///
    /// Returns `None` if the coordinates are out of bounds.
    pub fn get_pixel(&self, x: u32, y: u32) -> Option<&[u8]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let idx = (y as usize * self.width as usize + x as usize) * self.channels as usize;
        self.data.get(idx..idx + self.channels as usize)
    }

    /// Set pixel at (x, y) from a slice of channel values.
    ///
    /// Does nothing if the coordinates are out of bounds.
    pub fn set_pixel(&mut self, x: u32, y: u32, pixel: &[u8]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let ch = self.channels as usize;
        let idx = (y as usize * self.width as usize + x as usize) * ch;
        if idx + ch <= self.data.len() && pixel.len() >= ch {
            self.data[idx..idx + ch].copy_from_slice(&pixel[..ch]);
        }
    }

    /// Get pixel with bilinear interpolation at fractional coordinates.
    ///
    /// Returns RGBA values. For RGB buffers, alpha is set to 255.
    /// Out-of-bounds coordinates are clamped to the nearest edge pixel.
    pub fn sample_bilinear(&self, x: f64, y: f64) -> [u8; 4] {
        if self.width == 0 || self.height == 0 {
            return [0, 0, 0, 255];
        }

        let max_x = (self.width as f64) - 1.0;
        let max_y = (self.height as f64) - 1.0;
        let x = x.clamp(0.0, max_x);
        let y = y.clamp(0.0, max_y);

        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);

        let fx = x - x.floor();
        let fy = y - y.floor();

        let p00 = self.get_pixel_rgba(x0, y0);
        let p10 = self.get_pixel_rgba(x1, y0);
        let p01 = self.get_pixel_rgba(x0, y1);
        let p11 = self.get_pixel_rgba(x1, y1);

        let mut result = [0u8; 4];
        for i in 0..4 {
            let top = p00[i] as f64 * (1.0 - fx) + p10[i] as f64 * fx;
            let bottom = p01[i] as f64 * (1.0 - fx) + p11[i] as f64 * fx;
            let value = top * (1.0 - fy) + bottom * fy;
            result[i] = value.round().clamp(0.0, 255.0) as u8;
        }
        result
    }

    /// Get pixel as RGBA (pads RGB with alpha=255).
    pub(crate) fn get_pixel_rgba(&self, x: u32, y: u32) -> [u8; 4] {
        match self.get_pixel(x, y) {
            Some(p) if self.channels == 4 => [p[0], p[1], p[2], p[3]],
            Some(p) if self.channels >= 3 => [p[0], p[1], p[2], 255],
            Some(p) if self.channels == 1 => [p[0], p[0], p[0], 255],
            _ => [0, 0, 0, 255],
        }
    }

    /// Row stride in bytes.
    pub(crate) fn stride(&self) -> usize {
        self.width as usize * self.channels as usize
    }
}

// ============================================================================
// ProcessingError
// ============================================================================

/// Errors that can occur during pixel processing.
#[derive(Debug, thiserror::Error)]
pub enum ProcessingError {
    /// Buffer dimensions do not match the data length.
    #[error("invalid buffer dimensions: {width}x{height} with {data_len} bytes")]
    InvalidDimensions {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// Actual data length in bytes.
        data_len: usize,
    },

    /// A processing operation failed.
    #[error("processing failed: {0}")]
    ProcessingFailed(String),

    /// The requested operation is not supported.
    #[error("unsupported operation: {0}")]
    UnsupportedOperation(String),
}

// ============================================================================
// PipelineStep
// ============================================================================

/// A single step in the processing pipeline.
#[derive(Debug, Clone)]
pub enum PipelineStep {
    /// Trim a fixed number of pixels from each edge.
    Trim(Trim),
    /// Resize the image to target dimensions with a fit mode and gravity.
    Resize {
        /// Target width (0 = derive from aspect ratio).
        width: u32,
        /// Target height (0 = derive from aspect ratio).
        height: u32,
        /// How to fit the image into the target dimensions.
        fit: FitMode,
        /// Anchor point for cropping.
        gravity: Gravity,
    },
    /// Rotate by 90-degree increments.
    Rotate(Rotation),
    /// Adjust brightness (-1.0 to 1.0).
    Brightness(f64),
    /// Adjust contrast (-1.0 to 1.0).
    Contrast(f64),
    /// Apply gamma correction (> 0.0).
    Gamma(f64),
    /// Unsharp-mask sharpening amount.
    Sharpen(f64),
    /// Gaussian blur radius (sigma).
    Blur(f64),
    /// Add a coloured border around the image.
    AddBorder(Border),
    /// Add padding with a background colour.
    AddPadding(Padding, Color),
}

// ============================================================================
// Pipeline building
// ============================================================================

/// Build the processing pipeline from transform parameters.
///
/// Steps are ordered:
/// **Trim -> Resize -> Rotate -> Color adjustments -> Sharpen/Blur -> Border -> Padding**
pub fn build_pipeline(params: &TransformParams, _output_format: OutputFormat) -> Vec<PipelineStep> {
    let mut steps = Vec::new();

    // 1. Trim
    if let Some(trim) = params.trim {
        if trim.top > 0 || trim.right > 0 || trim.bottom > 0 || trim.left > 0 {
            steps.push(PipelineStep::Trim(trim));
        }
    }

    // 2. Resize (requires at least width or height)
    let eff_w = params.effective_width();
    let eff_h = params.effective_height();
    if eff_w.is_some() || eff_h.is_some() {
        steps.push(PipelineStep::Resize {
            width: eff_w.unwrap_or(0),
            height: eff_h.unwrap_or(0),
            fit: params.fit,
            gravity: params.gravity.clone(),
        });
    }

    // 3. Rotate
    match params.rotate {
        Rotation::Deg0 => {}
        other => steps.push(PipelineStep::Rotate(other)),
    }

    // 4. Color adjustments
    if params.brightness.abs() > f64::EPSILON {
        steps.push(PipelineStep::Brightness(params.brightness));
    }
    if params.contrast.abs() > f64::EPSILON {
        steps.push(PipelineStep::Contrast(params.contrast));
    }
    if (params.gamma - 1.0).abs() > f64::EPSILON {
        steps.push(PipelineStep::Gamma(params.gamma));
    }

    // 5. Sharpen / Blur
    if params.sharpen > f64::EPSILON {
        steps.push(PipelineStep::Sharpen(params.sharpen));
    }
    if params.blur > f64::EPSILON {
        steps.push(PipelineStep::Blur(params.blur));
    }

    // 6. Border
    if let Some(border) = params.border {
        if border.top > 0 || border.right > 0 || border.bottom > 0 || border.left > 0 {
            steps.push(PipelineStep::AddBorder(border));
        }
    }

    // 7. Padding
    if let Some(pad) = params.pad {
        if pad.top > f64::EPSILON
            || pad.right > f64::EPSILON
            || pad.bottom > f64::EPSILON
            || pad.left > f64::EPSILON
        {
            steps.push(PipelineStep::AddPadding(pad, params.background));
        }
    }

    steps
}

// ============================================================================
// Pipeline execution
// ============================================================================

/// Apply the full transformation pipeline to a pixel buffer.
///
/// Builds the pipeline from the given parameters and executes each step in order.
/// Returns a new buffer with all transformations applied.
pub fn apply_transforms(
    buffer: &mut PixelBuffer,
    params: &TransformParams,
) -> Result<PixelBuffer, ProcessingError> {
    let pipeline = build_pipeline(params, params.format);
    let mut result = buffer.clone();
    for step in &pipeline {
        result = apply_step(result, step)?;
    }
    Ok(result)
}

/// Apply a single pipeline step to a pixel buffer.
fn apply_step(buffer: PixelBuffer, step: &PipelineStep) -> Result<PixelBuffer, ProcessingError> {
    match step {
        PipelineStep::Trim(trim) => geometry::apply_trim(buffer, trim),
        PipelineStep::Resize {
            width,
            height,
            fit,
            gravity,
        } => resize::apply_resize(buffer, *width, *height, *fit, gravity),
        PipelineStep::Rotate(rotation) => color::apply_rotation(buffer, *rotation),
        PipelineStep::Brightness(v) => color::apply_brightness(buffer, *v),
        PipelineStep::Contrast(v) => color::apply_contrast(buffer, *v),
        PipelineStep::Gamma(v) => color::apply_gamma(buffer, *v),
        PipelineStep::Sharpen(v) => filters::apply_sharpen(buffer, *v),
        PipelineStep::Blur(v) => filters::apply_blur(buffer, *v),
        PipelineStep::AddBorder(border) => geometry::apply_border(buffer, border),
        PipelineStep::AddPadding(padding, bg) => geometry::apply_padding(buffer, padding, *bg),
    }
}
