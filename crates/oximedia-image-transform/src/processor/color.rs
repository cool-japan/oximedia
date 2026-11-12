// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Color adjustments: rotation, brightness, contrast, gamma.

use crate::transform::Rotation;

use super::{PixelBuffer, ProcessingError};

// ============================================================================
// Rotation
// ============================================================================

/// Rotate by 90/180/270 degrees clockwise.
pub(super) fn apply_rotation(
    buffer: PixelBuffer,
    rotation: Rotation,
) -> Result<PixelBuffer, ProcessingError> {
    let ch = buffer.channels as usize;

    match rotation {
        Rotation::Deg0 => Ok(buffer),
        Rotation::Deg90 => {
            let mut output = PixelBuffer::new(buffer.height, buffer.width, buffer.channels);
            for y in 0..buffer.height {
                for x in 0..buffer.width {
                    let new_x = buffer.height - 1 - y;
                    let new_y = x;
                    if let Some(p) = buffer.get_pixel(x, y) {
                        output.set_pixel(new_x, new_y, &p[..ch]);
                    }
                }
            }
            Ok(output)
        }
        Rotation::Deg180 => {
            let mut output = PixelBuffer::new(buffer.width, buffer.height, buffer.channels);
            for y in 0..buffer.height {
                for x in 0..buffer.width {
                    if let Some(p) = buffer.get_pixel(x, y) {
                        output.set_pixel(buffer.width - 1 - x, buffer.height - 1 - y, &p[..ch]);
                    }
                }
            }
            Ok(output)
        }
        Rotation::Deg270 => {
            let mut output = PixelBuffer::new(buffer.height, buffer.width, buffer.channels);
            for y in 0..buffer.height {
                for x in 0..buffer.width {
                    let new_x = y;
                    let new_y = buffer.width - 1 - x;
                    if let Some(p) = buffer.get_pixel(x, y) {
                        output.set_pixel(new_x, new_y, &p[..ch]);
                    }
                }
            }
            Ok(output)
        }
        Rotation::Auto => {
            // Auto rotation based on EXIF -- treated as no-op at pixel level
            // (EXIF handling is external)
            Ok(buffer)
        }
    }
}

// ============================================================================
// Brightness
// ============================================================================

/// Adjust brightness. Value range: -1.0 (black) to 1.0 (white).
///
/// Per-channel formula: `new = clamp(old + value * 255, 0, 255)`.
/// Alpha channel is preserved unchanged.
pub(super) fn apply_brightness(
    mut buffer: PixelBuffer,
    value: f64,
) -> Result<PixelBuffer, ProcessingError> {
    let offset = (value * 255.0).round() as i16;
    let ch = buffer.channels as usize;
    let color_ch = if ch >= 4 { 3 } else { ch };

    for pixel in buffer.data.chunks_exact_mut(ch) {
        for c in 0..color_ch {
            let v = pixel[c] as i16 + offset;
            pixel[c] = v.clamp(0, 255) as u8;
        }
    }
    Ok(buffer)
}

// ============================================================================
// Contrast
// ============================================================================

/// Adjust contrast. Value range: -1.0 (flat gray) to 1.0 (maximum contrast).
///
/// Per-channel formula: `new = clamp((old - 128) * (1 + value) + 128, 0, 255)`.
/// Alpha channel is preserved unchanged.
pub(super) fn apply_contrast(
    mut buffer: PixelBuffer,
    value: f64,
) -> Result<PixelBuffer, ProcessingError> {
    let factor = 1.0 + value;
    let ch = buffer.channels as usize;
    let color_ch = if ch >= 4 { 3 } else { ch };

    for pixel in buffer.data.chunks_exact_mut(ch) {
        for c in 0..color_ch {
            let v = ((pixel[c] as f64 - 128.0) * factor + 128.0).round();
            pixel[c] = v.clamp(0.0, 255.0) as u8;
        }
    }
    Ok(buffer)
}

// ============================================================================
// Gamma
// ============================================================================

/// Apply gamma correction using a 256-entry lookup table.
///
/// Per-channel formula: `new = 255 * (old / 255) ^ (1 / gamma)`.
/// Alpha channel is preserved unchanged.
pub(super) fn apply_gamma(
    mut buffer: PixelBuffer,
    gamma: f64,
) -> Result<PixelBuffer, ProcessingError> {
    if gamma <= 0.0 {
        return Err(ProcessingError::ProcessingFailed(
            "gamma must be positive".to_string(),
        ));
    }

    let inv_gamma = 1.0 / gamma;
    let mut lut = [0u8; 256];
    for (i, entry) in lut.iter_mut().enumerate() {
        let normalized = i as f64 / 255.0;
        *entry = (255.0 * normalized.powf(inv_gamma))
            .round()
            .clamp(0.0, 255.0) as u8;
    }

    let ch = buffer.channels as usize;
    let color_ch = if ch >= 4 { 3 } else { ch };

    for pixel in buffer.data.chunks_exact_mut(ch) {
        for c in 0..color_ch {
            pixel[c] = lut[pixel[c] as usize];
        }
    }
    Ok(buffer)
}
