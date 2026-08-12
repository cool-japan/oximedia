//! VP8 motion compensation and sub-pixel interpolation.
//!
//! # Deprecated: defective early seed, not RFC 6386-accurate
//!
//! **Every public item in this module is `#[deprecated]`** (since 0.2.1,
//! scheduled for removal in 0.3.0). [`MotionVector`]'s unit handling and
//! this module's 6-tap filter tap/stride arithmetic do not match RFC 6386
//! SS18, and nothing in the decoder uses them. The real, bit-exact
//! sub-pixel motion compensation lives in `vp8::dec::mc` (used by
//! `Vp8Decoder`) and does not use these types. Kept only for API stability.
//!
//! This module implements motion compensation for VP8 inter prediction.
//! Motion vectors in VP8 have quarter-pixel precision and use 6-tap
//! Sinc-based interpolation filters.
//!
//! # Motion Vector Representation
//!
//! Motion vectors are stored in quarter-pixel units:
//! - Integer pixel: mv % 4 == 0
//! - Half pixel: mv % 4 == 2
//! - Quarter pixel: mv % 4 == 1 or 3

#![allow(dead_code)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::too_many_arguments)]

/// Motion vector with quarter-pixel precision.
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (motion-vector units and 6-tap filter tap/stride do not match RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotionVector {
    /// Horizontal component (quarter-pixel units).
    pub x: i16,
    /// Vertical component (quarter-pixel units).
    pub y: i16,
}

// Internal reference site: this impl block's own methods necessarily name
// `MotionVector`'s deprecated fields; the impl itself is not deprecated
// (see the module doc's policy on inherent methods), so it needs its own
// targeted allow.
#[allow(deprecated)]
impl MotionVector {
    /// Creates a new motion vector.
    #[must_use]
    pub const fn new(x: i16, y: i16) -> Self {
        Self { x, y }
    }

    /// Returns the integer pixel part of the horizontal component.
    #[must_use]
    pub const fn x_int(self) -> i32 {
        (self.x >> 2) as i32
    }

    /// Returns the integer pixel part of the vertical component.
    #[must_use]
    pub const fn y_int(self) -> i32 {
        (self.y >> 2) as i32
    }

    /// Returns the fractional part of the horizontal component (0-3).
    #[must_use]
    pub const fn x_frac(self) -> u8 {
        (self.x & 3) as u8
    }

    /// Returns the fractional part of the vertical component (0-3).
    #[must_use]
    pub const fn y_frac(self) -> u8 {
        (self.y & 3) as u8
    }

    /// Returns whether this is an integer-pixel motion vector.
    #[must_use]
    pub const fn is_integer(self) -> bool {
        self.x & 3 == 0 && self.y & 3 == 0
    }
}

/// VP8 6-tap interpolation filter coefficients.
///
/// VP8 uses different filter taps depending on the sub-pixel position.
/// Index: [fractional_position][tap]
const SUBPEL_FILTERS: [[i32; 6]; 8] = [
    [0, 0, 128, 0, 0, 0],     // Integer position (no filtering)
    [0, -6, 123, 12, -1, 0],  // 1/8 pixel
    [2, -11, 108, 36, -8, 1], // 1/4 pixel
    [0, -9, 93, 50, -6, 0],   // 3/8 pixel
    [3, -16, 77, 77, -16, 3], // 1/2 pixel (symmetric)
    [0, -6, 50, 93, -9, 0],   // 5/8 pixel
    [1, -8, 36, 108, -11, 2], // 3/4 pixel
    [0, -1, 12, 123, -6, 0],  // 7/8 pixel
];

/// Performs motion compensation for a block.
///
/// # Arguments
///
/// * `dst` - Destination buffer
/// * `dst_stride` - Stride of destination buffer
/// * `ref_frame` - Reference frame buffer
/// * `ref_stride` - Stride of reference frame
/// * `mv` - Motion vector
/// * `block_w` - Block width
/// * `block_h` - Block height
/// * `ref_x` - Reference block X position (integer pixels)
/// * `ref_y` - Reference block Y position (integer pixels)
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (motion-vector units and 6-tap filter tap/stride do not match RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// `#[deprecated]` on this function does not exempt its own signature/body
// from separately warning about the (also deprecated) `MotionVector`
// parameter and its methods; internal reference site.
#[allow(deprecated)]
#[allow(clippy::similar_names)]
pub fn motion_compensate(
    dst: &mut [u8],
    dst_stride: usize,
    ref_frame: &[u8],
    ref_stride: usize,
    mv: MotionVector,
    block_w: usize,
    block_h: usize,
    ref_x: usize,
    ref_y: usize,
) {
    let x_int = mv.x_int();
    let y_int = mv.y_int();
    let x_frac = mv.x_frac();
    let y_frac = mv.y_frac();

    // Calculate reference position
    let ref_x = (ref_x as i32 + x_int) as usize;
    let ref_y = (ref_y as i32 + y_int) as usize;

    if x_frac == 0 && y_frac == 0 {
        // Integer pixel - simple copy
        copy_block(
            dst, dst_stride, ref_frame, ref_stride, ref_x, ref_y, block_w, block_h,
        );
    } else if y_frac == 0 {
        // Horizontal filtering only
        filter_horizontal(
            dst, dst_stride, ref_frame, ref_stride, ref_x, ref_y, block_w, block_h, x_frac,
        );
    } else if x_frac == 0 {
        // Vertical filtering only
        filter_vertical(
            dst, dst_stride, ref_frame, ref_stride, ref_x, ref_y, block_w, block_h, y_frac,
        );
    } else {
        // Both horizontal and vertical filtering
        filter_2d(
            dst, dst_stride, ref_frame, ref_stride, ref_x, ref_y, block_w, block_h, x_frac, y_frac,
        );
    }
}

/// Copies a block without filtering (integer pixel).
fn copy_block(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
    width: usize,
    height: usize,
) {
    for row in 0..height {
        let dst_offset = row * dst_stride;
        let src_offset = (src_y + row) * src_stride + src_x;

        if src_offset + width <= src.len() && dst_offset + width <= dst.len() {
            dst[dst_offset..dst_offset + width]
                .copy_from_slice(&src[src_offset..src_offset + width]);
        }
    }
}

/// Applies horizontal 6-tap filter.
fn filter_horizontal(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
    width: usize,
    height: usize,
    frac: u8,
) {
    let filter = &SUBPEL_FILTERS[frac as usize];

    for row in 0..height {
        let dst_offset = row * dst_stride;
        let src_offset = (src_y + row) * src_stride + src_x;

        for col in 0..width {
            let mut sum = 0i32;

            // Apply 6-tap filter
            for (i, &tap) in filter.iter().enumerate() {
                let idx = src_offset + col + i;
                if idx < 2 || idx + 3 >= src.len() {
                    continue;
                }
                sum += tap * i32::from(src[idx - 2 + i]);
            }

            // Round and clamp
            let pixel = ((sum + 64) >> 7).clamp(0, 255) as u8;
            if dst_offset + col < dst.len() {
                dst[dst_offset + col] = pixel;
            }
        }
    }
}

/// Applies vertical 6-tap filter.
fn filter_vertical(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
    width: usize,
    height: usize,
    frac: u8,
) {
    let filter = &SUBPEL_FILTERS[frac as usize];

    for row in 0..height {
        let dst_offset = row * dst_stride;

        for col in 0..width {
            let mut sum = 0i32;

            // Apply 6-tap filter vertically
            for (i, &tap) in filter.iter().enumerate() {
                let src_row = src_y + row + i;
                if src_row < 2 || src_row + 3 >= src.len() / src_stride {
                    continue;
                }
                let idx = (src_row - 2 + i) * src_stride + src_x + col;
                if idx < src.len() {
                    sum += tap * i32::from(src[idx]);
                }
            }

            // Round and clamp
            let pixel = ((sum + 64) >> 7).clamp(0, 255) as u8;
            if dst_offset + col < dst.len() {
                dst[dst_offset + col] = pixel;
            }
        }
    }
}

/// Applies 2D filtering (separable horizontal then vertical).
#[allow(clippy::similar_names)]
fn filter_2d(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    src_x: usize,
    src_y: usize,
    width: usize,
    height: usize,
    x_frac: u8,
    y_frac: u8,
) {
    // Temporary buffer for horizontal filtering
    let mut temp = vec![0u8; (height + 5) * width];

    // First apply horizontal filter
    filter_horizontal(
        &mut temp,
        width,
        src,
        src_stride,
        src_x,
        src_y.saturating_sub(2),
        width,
        height + 5,
        x_frac,
    );

    // Then apply vertical filter on the temp buffer
    filter_vertical(dst, dst_stride, &temp, width, 0, 2, width, height, y_frac);
}

/// Clamps motion vector to valid range.
///
/// # Arguments
///
/// * `mv` - Motion vector to clamp
/// * `x` - Current block X position
/// * `y` - Current block Y position
/// * `width` - Block width
/// * `height` - Block height
/// * `frame_w` - Frame width
/// * `frame_h` - Frame height
#[deprecated(
    since = "0.2.1",
    note = "defective early seed (motion-vector units and 6-tap filter tap/stride do not match RFC 6386) superseded by the internal RFC 6386 pipeline behind Vp8Decoder; scheduled for removal in 0.3.0"
)]
// `#[deprecated]` on this function does not exempt its own signature/body
// from separately warning about the (also deprecated) `MotionVector`
// parameter, return type, and field accesses; internal reference site.
#[allow(deprecated)]
#[must_use]
pub fn clamp_mv(
    mv: MotionVector,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    frame_w: usize,
    frame_h: usize,
) -> MotionVector {
    let x = x as i32;
    let y = y as i32;
    let width = width as i32;
    let height = height as i32;
    let frame_w = frame_w as i32;
    let frame_h = frame_h as i32;

    // Calculate reference position
    let ref_x = x + mv.x_int();
    let ref_y = y + mv.y_int();

    // Clamp to frame boundaries
    let clamped_x = ref_x.clamp(0, frame_w - width);
    let clamped_y = ref_y.clamp(0, frame_h - height);

    // Adjust motion vector
    let new_x = ((clamped_x - x) << 2) as i16 + (mv.x & 3) as i16;
    let new_y = ((clamped_y - y) << 2) as i16 + (mv.y & 3) as i16;

    MotionVector::new(new_x, new_y)
}

#[cfg(test)]
// This module's entire purpose is exercising the deprecated types declared
// above; targeted (not blanket/crate-wide) allow for exactly that site.
#[allow(deprecated)]
mod tests {
    use super::*;

    #[test]
    fn test_motion_vector() {
        let mv = MotionVector::new(10, 6); // 2.5 pixels horizontal, 1.5 pixels vertical

        assert_eq!(mv.x_int(), 2);
        assert_eq!(mv.y_int(), 1);
        assert_eq!(mv.x_frac(), 2);
        assert_eq!(mv.y_frac(), 2);
        assert!(!mv.is_integer());

        let int_mv = MotionVector::new(8, 12); // 2 pixels horizontal, 3 pixels vertical
        assert_eq!(int_mv.x_int(), 2);
        assert_eq!(int_mv.y_int(), 3);
        assert!(int_mv.is_integer());
    }

    #[test]
    fn test_subpel_filters() {
        // Integer position filter should be [0, 0, 128, 0, 0, 0]
        assert_eq!(SUBPEL_FILTERS[0], [0, 0, 128, 0, 0, 0]);

        // Half-pixel filter should be symmetric
        assert_eq!(SUBPEL_FILTERS[4], [3, -16, 77, 77, -16, 3]);

        // All filters should sum to 128 (approximately, due to rounding)
        for filter in &SUBPEL_FILTERS {
            let sum: i32 = filter.iter().sum();
            assert!((sum - 128).abs() <= 2);
        }
    }

    #[test]
    fn test_copy_block() {
        let src = vec![100u8; 64]; // 8x8 block of 100s
        let mut dst = vec![0u8; 64];

        copy_block(&mut dst, 8, &src, 8, 0, 0, 4, 4);

        // First 4x4 should be copied
        for row in 0..4 {
            for col in 0..4 {
                assert_eq!(dst[row * 8 + col], 100);
            }
        }
    }

    #[test]
    fn test_motion_compensate_integer() {
        let ref_frame = vec![50u8; 256]; // 16x16 reference
        let mut dst = vec![0u8; 64]; // 8x8 destination

        let mv = MotionVector::new(0, 0); // Integer pixel

        motion_compensate(&mut dst, 8, &ref_frame, 16, mv, 8, 8, 0, 0);

        // Should be simple copy
        assert!(dst.iter().all(|&p| p == 50));
    }

    #[test]
    fn test_clamp_mv() {
        let mv = MotionVector::new(100, 100); // Large motion vector

        let clamped = clamp_mv(mv, 10, 10, 4, 4, 32, 32);

        // Should be clamped to frame boundaries
        let ref_x = 10 + clamped.x_int();
        let ref_y = 10 + clamped.y_int();

        assert!(ref_x >= 0);
        assert!(ref_y >= 0);
        assert!(ref_x + 4 <= 32);
        assert!(ref_y + 4 <= 32);
    }

    #[test]
    fn test_clamp_mv_negative() {
        let mv = MotionVector::new(-100, -100); // Large negative MV

        let clamped = clamp_mv(mv, 10, 10, 4, 4, 32, 32);

        let ref_x = 10 + clamped.x_int();
        let ref_y = 10 + clamped.y_int();

        assert!(ref_x >= 0);
        assert!(ref_y >= 0);
    }

    #[test]
    fn test_filter_horizontal() {
        let src = vec![100u8; 256];
        let mut dst = vec![0u8; 64];

        // Apply horizontal filter with quarter-pixel precision
        filter_horizontal(&mut dst, 8, &src, 16, 0, 0, 8, 8, 2);

        // Output should be non-zero (filtered)
        assert!(dst.iter().any(|&p| p > 0));
    }

    #[test]
    fn test_filter_vertical() {
        let src = vec![100u8; 256];
        let mut dst = vec![0u8; 64];

        filter_vertical(&mut dst, 8, &src, 16, 0, 0, 8, 8, 2);

        // Output should be non-zero (filtered)
        assert!(dst.iter().any(|&p| p > 0));
    }

    #[test]
    fn test_filter_2d() {
        let src = vec![100u8; 256];
        let mut dst = vec![0u8; 64];

        filter_2d(&mut dst, 8, &src, 16, 0, 0, 8, 8, 2, 2);

        // Output should be non-zero (filtered)
        assert!(dst.iter().any(|&p| p > 0));
    }

    #[test]
    fn test_motion_vector_zero() {
        let mv = MotionVector::new(0, 0);
        assert_eq!(mv.x_int(), 0);
        assert_eq!(mv.y_int(), 0);
        assert_eq!(mv.x_frac(), 0);
        assert_eq!(mv.y_frac(), 0);
        assert!(mv.is_integer());
    }
}
