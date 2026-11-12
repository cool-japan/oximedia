// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Geometry helpers: crop, border, padding, gravity calculations.

use crate::transform::{Border, Color, Gravity, Padding, Trim};

use super::{PixelBuffer, ProcessingError};

// ============================================================================
// Trim
// ============================================================================

/// Crop a fixed number of pixels from each edge.
pub(super) fn apply_trim(buffer: PixelBuffer, trim: &Trim) -> Result<PixelBuffer, ProcessingError> {
    if buffer.width == 0 || buffer.height == 0 {
        return Ok(buffer);
    }

    let left = trim.left.min(buffer.width);
    let right = trim.right.min(buffer.width.saturating_sub(left));
    let top = trim.top.min(buffer.height);
    let bottom = trim.bottom.min(buffer.height.saturating_sub(top));

    let new_w = buffer.width.saturating_sub(left + right);
    let new_h = buffer.height.saturating_sub(top + bottom);

    if new_w == 0 || new_h == 0 {
        return Ok(PixelBuffer::new(0, 0, buffer.channels));
    }

    crop_region(&buffer, left, top, new_w, new_h)
}

// ============================================================================
// Border
// ============================================================================

/// Add a coloured border around the image.
pub(super) fn apply_border(
    buffer: PixelBuffer,
    border: &Border,
) -> Result<PixelBuffer, ProcessingError> {
    let new_width = buffer.width.saturating_add(border.left + border.right);
    let new_height = buffer.height.saturating_add(border.top + border.bottom);
    if new_width == buffer.width && new_height == buffer.height {
        return Ok(buffer);
    }

    let ch = buffer.channels as usize;
    let border_pixel = color_to_pixel(&border.color, buffer.channels);
    let mut output = PixelBuffer::new(new_width, new_height, buffer.channels);

    // Fill with border colour
    for y in 0..new_height {
        for x in 0..new_width {
            output.set_pixel(x, y, &border_pixel[..ch]);
        }
    }

    // Copy original image into position
    for y in 0..buffer.height {
        let src_start = y as usize * buffer.stride();
        let row_bytes = buffer.stride();
        let dst_start = (y + border.top) as usize * output.stride() + border.left as usize * ch;
        if src_start + row_bytes <= buffer.data.len() && dst_start + row_bytes <= output.data.len()
        {
            output.data[dst_start..dst_start + row_bytes]
                .copy_from_slice(&buffer.data[src_start..src_start + row_bytes]);
        }
    }

    Ok(output)
}

// ============================================================================
// Padding
// ============================================================================

/// Add padding with a background colour.
///
/// Padding values are fractional (0.0..1.0) relative to the current buffer
/// dimensions.
pub(super) fn apply_padding(
    buffer: PixelBuffer,
    padding: &Padding,
    bg: Color,
) -> Result<PixelBuffer, ProcessingError> {
    let pad_top = (padding.top * buffer.height as f64).round() as u32;
    let pad_right = (padding.right * buffer.width as f64).round() as u32;
    let pad_bottom = (padding.bottom * buffer.height as f64).round() as u32;
    let pad_left = (padding.left * buffer.width as f64).round() as u32;

    let new_width = buffer.width.saturating_add(pad_left + pad_right);
    let new_height = buffer.height.saturating_add(pad_top + pad_bottom);
    let ch = buffer.channels as usize;
    let bg_pixel = color_to_pixel(&bg, buffer.channels);

    let mut output = PixelBuffer::new(new_width, new_height, buffer.channels);

    // Fill with background colour
    for y in 0..new_height {
        for x in 0..new_width {
            output.set_pixel(x, y, &bg_pixel[..ch]);
        }
    }

    // Copy original into padded position
    for y in 0..buffer.height {
        let src_start = y as usize * buffer.stride();
        let row_bytes = buffer.stride();
        let dst_start = (y + pad_top) as usize * output.stride() + pad_left as usize * ch;
        if src_start + row_bytes <= buffer.data.len() && dst_start + row_bytes <= output.data.len()
        {
            output.data[dst_start..dst_start + row_bytes]
                .copy_from_slice(&buffer.data[src_start..src_start + row_bytes]);
        }
    }

    Ok(output)
}

// ============================================================================
// Crop region helper
// ============================================================================

/// Crop a rectangular region from the buffer.
pub(super) fn crop_region(
    buffer: &PixelBuffer,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<PixelBuffer, ProcessingError> {
    if width == 0 || height == 0 {
        return Ok(PixelBuffer::new(0, 0, buffer.channels));
    }
    if x + width > buffer.width || y + height > buffer.height {
        return Err(ProcessingError::ProcessingFailed(format!(
            "crop region ({x},{y},{width},{height}) exceeds buffer ({}x{})",
            buffer.width, buffer.height
        )));
    }

    let ch = buffer.channels as usize;
    let mut output = PixelBuffer::new(width, height, buffer.channels);
    let src_stride = buffer.stride();
    let dst_stride = output.stride();

    for row in 0..height {
        let src_start = (y + row) as usize * src_stride + x as usize * ch;
        let dst_start = row as usize * dst_stride;
        let row_bytes = width as usize * ch;
        output.data[dst_start..dst_start + row_bytes]
            .copy_from_slice(&buffer.data[src_start..src_start + row_bytes]);
    }

    Ok(output)
}

// ============================================================================
// Gravity and crop rect helpers
// ============================================================================

/// Calculate crop rectangle for a given gravity.
///
/// Returns `(x, y, crop_width, crop_height)`.
pub fn calculate_crop_rect(
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
    gravity: &Gravity,
) -> (u32, u32, u32, u32) {
    let cw = dst_width.min(src_width);
    let ch = dst_height.min(src_height);
    let excess_x = src_width.saturating_sub(cw);
    let excess_y = src_height.saturating_sub(ch);

    let (gx, gy) = gravity_to_fractions(gravity);
    let x = (excess_x as f64 * gx).round() as u32;
    let y = (excess_y as f64 * gy).round() as u32;

    (x, y, cw, ch)
}

/// Convert gravity to fractional offsets (0.0 - 1.0) for x and y.
pub(super) fn gravity_to_fractions(gravity: &Gravity) -> (f64, f64) {
    match gravity {
        Gravity::Auto | Gravity::Center | Gravity::Face => (0.5, 0.5),
        Gravity::Top => (0.5, 0.0),
        Gravity::Bottom => (0.5, 1.0),
        Gravity::Left => (0.0, 0.5),
        Gravity::Right => (1.0, 0.5),
        Gravity::TopLeft => (0.0, 0.0),
        Gravity::TopRight => (1.0, 0.0),
        Gravity::BottomLeft => (0.0, 1.0),
        Gravity::BottomRight => (1.0, 1.0),
        Gravity::FocalPoint(x, y) => (*x, *y),
    }
}

/// Pad an image to exact dimensions, centering it on a solid background.
pub(super) fn pad_to_exact_size(
    buffer: PixelBuffer,
    target_width: u32,
    target_height: u32,
    bg: Color,
) -> Result<PixelBuffer, ProcessingError> {
    let pad_x = target_width.saturating_sub(buffer.width);
    let pad_y = target_height.saturating_sub(buffer.height);
    let left = pad_x / 2;
    let top = pad_y / 2;

    let new_width = buffer.width.saturating_add(pad_x);
    let new_height = buffer.height.saturating_add(pad_y);
    let ch = buffer.channels as usize;
    let bg_pixel = color_to_pixel(&bg, buffer.channels);

    let mut output = PixelBuffer::new(new_width, new_height, buffer.channels);

    for y in 0..new_height {
        for x in 0..new_width {
            output.set_pixel(x, y, &bg_pixel[..ch]);
        }
    }

    for y in 0..buffer.height {
        let src_start = y as usize * buffer.stride();
        let row_bytes = buffer.stride();
        let dst_start = (y + top) as usize * output.stride() + left as usize * ch;
        if src_start + row_bytes <= buffer.data.len() && dst_start + row_bytes <= output.data.len()
        {
            output.data[dst_start..dst_start + row_bytes]
                .copy_from_slice(&buffer.data[src_start..src_start + row_bytes]);
        }
    }

    Ok(output)
}

// ============================================================================
// Color pixel helper
// ============================================================================

/// Convert a [`Color`] to a pixel array suitable for the given channel count.
pub(super) fn color_to_pixel(color: &Color, channels: u8) -> [u8; 4] {
    match channels {
        4 => [color.r, color.g, color.b, color.a],
        _ => [color.r, color.g, color.b, 255],
    }
}
