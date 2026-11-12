// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Frame descriptors and buffer-length validation.
//!
//! All buffers handled by this crate are **tightly packed** (no row padding /
//! stride): a `width x height` RGBA8 frame is exactly `width * height * 4`
//! bytes and an f32 RGBA frame is exactly `width * height * 4` `f32` elements.
//!
//! Length computations use checked arithmetic because `usize` is 32-bit on the
//! `wasm32-unknown-unknown` target, so a large frame can overflow when
//! multiplied out.

use crate::error::CoreError;

/// Number of channels in a packed RGBA pixel.
pub const RGBA_CHANNELS: usize = 4;

/// Multiplies a list of factors, returning [`CoreError::DimensionOverflow`] on
/// overflow.
fn checked_product(factors: &[usize]) -> Result<usize, CoreError> {
    let mut acc: usize = 1;
    for &f in factors {
        acc = acc.checked_mul(f).ok_or(CoreError::DimensionOverflow)?;
    }
    Ok(acc)
}

/// Required length, in bytes, of a tightly packed RGBA8 buffer.
///
/// # Errors
///
/// Returns [`CoreError::DimensionOverflow`] if `width * height * 4` overflows
/// `usize`.
pub fn rgba8_len(width: usize, height: usize) -> Result<usize, CoreError> {
    checked_product(&[width, height, RGBA_CHANNELS])
}

/// Required length, in `f32` elements, of a tightly packed f32 RGBA buffer.
///
/// This equals [`rgba8_len`] (four elements per pixel) but is provided as a
/// distinct name so call sites document whether they mean bytes or floats.
///
/// # Errors
///
/// Returns [`CoreError::DimensionOverflow`] if `width * height * 4` overflows
/// `usize`.
pub fn rgba_f32_len(width: usize, height: usize) -> Result<usize, CoreError> {
    checked_product(&[width, height, RGBA_CHANNELS])
}

/// Validates that `data` is exactly the right length for a `width x height`
/// tightly packed RGBA8 frame.
///
/// # Errors
///
/// - [`CoreError::DimensionOverflow`] if the length computation overflows.
/// - [`CoreError::BufferLength`] if `data.len()` is not `width * height * 4`.
pub fn validate_rgba8(data: &[u8], width: usize, height: usize) -> Result<(), CoreError> {
    let expected = rgba8_len(width, height)?;
    if data.len() == expected {
        Ok(())
    } else {
        Err(CoreError::BufferLength {
            expected,
            actual: data.len(),
        })
    }
}

/// Validates that `data` is exactly the right length for a `width x height`
/// tightly packed f32 RGBA frame.
///
/// # Errors
///
/// - [`CoreError::DimensionOverflow`] if the length computation overflows.
/// - [`CoreError::BufferLength`] if `data.len()` is not `width * height * 4`.
pub fn validate_rgba_f32(data: &[f32], width: usize, height: usize) -> Result<(), CoreError> {
    let expected = rgba_f32_len(width, height)?;
    if data.len() == expected {
        Ok(())
    } else {
        Err(CoreError::BufferLength {
            expected,
            actual: data.len(),
        })
    }
}

/// Validated `width x height` frame geometry with helpers for the plane sizes
/// of the pixel layouts this crate converts between.
///
/// Chroma plane dimensions use **ceiling** division so that odd frame widths /
/// heights are handled correctly for 4:2:0 subsampling (the trailing lone
/// column / row shares the last chroma sample — "edge duplication").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameDims {
    /// Frame width in pixels.
    pub width: usize,
    /// Frame height in pixels.
    pub height: usize,
}

impl FrameDims {
    /// Creates a validated [`FrameDims`], rejecting a zero width or height.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::ZeroDimension`] if either dimension is zero.
    pub fn new(width: usize, height: usize) -> Result<Self, CoreError> {
        if width == 0 || height == 0 {
            return Err(CoreError::ZeroDimension);
        }
        Ok(Self { width, height })
    }

    /// Number of luma pixels (`width * height`).
    ///
    /// # Errors
    ///
    /// [`CoreError::DimensionOverflow`] on overflow.
    pub fn pixel_count(&self) -> Result<usize, CoreError> {
        checked_product(&[self.width, self.height])
    }

    /// Length of a tightly packed RGBA8 buffer for this frame.
    ///
    /// # Errors
    ///
    /// [`CoreError::DimensionOverflow`] on overflow.
    pub fn rgba8_len(&self) -> Result<usize, CoreError> {
        rgba8_len(self.width, self.height)
    }

    /// Length of a luma (single-channel 8-bit) plane for this frame.
    ///
    /// # Errors
    ///
    /// [`CoreError::DimensionOverflow`] on overflow.
    pub fn luma_len(&self) -> Result<usize, CoreError> {
        self.pixel_count()
    }

    /// Chroma-plane width for 4:2:0 subsampling: `ceil(width / 2)`.
    #[must_use]
    pub const fn chroma_width(&self) -> usize {
        self.width / 2 + self.width % 2
    }

    /// Chroma-plane height for 4:2:0 subsampling: `ceil(height / 2)`.
    #[must_use]
    pub const fn chroma_height(&self) -> usize {
        self.height / 2 + self.height % 2
    }

    /// Length of a single I420 chroma plane (`chroma_width * chroma_height`).
    ///
    /// # Errors
    ///
    /// [`CoreError::DimensionOverflow`] on overflow.
    pub fn chroma_len(&self) -> Result<usize, CoreError> {
        checked_product(&[self.chroma_width(), self.chroma_height()])
    }

    /// Length of an NV12 interleaved UV plane (`chroma_len * 2`).
    ///
    /// # Errors
    ///
    /// [`CoreError::DimensionOverflow`] on overflow.
    pub fn nv12_uv_len(&self) -> Result<usize, CoreError> {
        checked_product(&[self.chroma_width(), self.chroma_height(), 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba8_len_basic() {
        assert_eq!(rgba8_len(2, 3).unwrap(), 24);
        assert_eq!(rgba_f32_len(4, 4).unwrap(), 64);
    }

    #[test]
    fn overflow_is_reported_not_panicked() {
        let big = usize::MAX;
        assert_eq!(rgba8_len(big, big), Err(CoreError::DimensionOverflow));
    }

    #[test]
    fn validate_rgba8_accepts_exact_and_rejects_off_by_one() {
        let ok = vec![0u8; 4 * 4 * 4];
        assert_eq!(validate_rgba8(&ok, 4, 4), Ok(()));
        let bad = vec![0u8; 4 * 4 * 4 - 1];
        assert_eq!(
            validate_rgba8(&bad, 4, 4),
            Err(CoreError::BufferLength {
                expected: 64,
                actual: 63
            })
        );
    }

    #[test]
    fn validate_rgba_f32_length() {
        let ok = vec![0.0f32; 2 * 2 * 4];
        assert_eq!(validate_rgba_f32(&ok, 2, 2), Ok(()));
        let bad = vec![0.0f32; 2 * 2 * 4 + 5];
        assert!(matches!(
            validate_rgba_f32(&bad, 2, 2),
            Err(CoreError::BufferLength { .. })
        ));
    }

    #[test]
    fn frame_dims_rejects_zero() {
        assert_eq!(FrameDims::new(0, 4), Err(CoreError::ZeroDimension));
        assert_eq!(FrameDims::new(4, 0), Err(CoreError::ZeroDimension));
    }

    #[test]
    fn chroma_dims_use_ceiling_for_odd() {
        let d = FrameDims::new(5, 3).unwrap();
        assert_eq!(d.chroma_width(), 3); // ceil(5/2)
        assert_eq!(d.chroma_height(), 2); // ceil(3/2)
        assert_eq!(d.chroma_len().unwrap(), 6);
        assert_eq!(d.nv12_uv_len().unwrap(), 12);
    }

    #[test]
    fn even_dims_plane_sizes() {
        let d = FrameDims::new(8, 8).unwrap();
        assert_eq!(d.pixel_count().unwrap(), 64);
        assert_eq!(d.rgba8_len().unwrap(), 256);
        assert_eq!(d.luma_len().unwrap(), 64);
        assert_eq!(d.chroma_len().unwrap(), 16);
        assert_eq!(d.nv12_uv_len().unwrap(), 32);
    }
}
