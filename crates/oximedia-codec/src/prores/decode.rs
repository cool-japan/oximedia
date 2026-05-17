//! Slice decode pipeline — entropy decode + dequantization + IDCT.
//!
//! **This module is the explicit extension point for the next phase of
//! ProRes work.** The header and quantization-matrix parsing landed in
//! sibling modules ([`super::frame`], [`super::picture`],
//! [`super::quant`]) is complete; what's stubbed here is the per-slice
//! decode pipeline that turns entropy-coded plane data into 10-bit
//! YUV422 sample arrays.
//!
//! ## What this revision provides
//!
//! - [`SliceData`] — the structured input that follows a successfully
//!   parsed [`super::SliceHeader`]: three (or four, with alpha) raw
//!   byte slices, one per plane.
//! - [`split_slice_planes`] — slices the post-header bytes of a slice
//!   into per-plane sub-slices using the sizes from the header. This is
//!   the last step before entropy decode and is fully implemented and
//!   tested.
//! - [`decode_slice_to_yuv422`] — the signature of the function the
//!   Phase 2 PR fills in. It currently returns
//!   [`DecodeError::NotImplemented`] so callers fail loudly rather than
//!   silently producing zeros. The signature, documentation, and
//!   structured output are stable.
//!
//! ## What the Phase 2 PR must add
//!
//! 1. **Entropy decode** (RDD 36 §6.5.5–6.5.6):
//!    - DC: differential coding across the slice's macroblocks, first
//!      block uses an absolute DC; deltas are decoded via exp-Golomb
//!      with adaptive Rice parameter.
//!    - AC: run/level pairs, also via adaptive exp-Golomb. End-of-block
//!      marker terminates each 8×8 block.
//! 2. **Dequantization** (RDD 36 §6.5.7):
//!    `coeff[i] = quantized[i] * matrix[i] * qscale` where matrix comes
//!    from the frame header and qscale from the slice header.
//! 3. **Inverse zigzag scan** (RDD 36 §6.5.7 Table 11).
//! 4. **8×8 integer IDCT** (RDD 36 §6.5.7).
//! 5. **Plane assembly**: 16-bit-internal samples → 10-bit clipped
//!    output, written into the caller-supplied destination planes at
//!    the right macroblock-row offset.

use thiserror::Error;

/// Errors emitted by the (Phase 2) slice decode pipeline.
#[derive(Debug, Error)]
pub enum DecodeError {
    /// One of the per-plane byte budgets overran the slice payload.
    #[error("slice plane sizes overrun: {0}")]
    PlaneOverrun(&'static str),

    /// Phase 2 placeholder. Will be removed when the decode pipeline
    /// is implemented end-to-end.
    #[error("ProRes slice decode is not yet implemented (Phase 2 work)")]
    NotImplemented,
}

/// Per-plane byte slices for one decoded slice header, ready for entropy decode.
#[derive(Debug, Clone, Copy)]
pub struct SliceData<'a> {
    /// Compressed luma data.
    pub luma: &'a [u8],
    /// Compressed Cb data.
    pub cb: &'a [u8],
    /// Compressed Cr data.
    pub cr: &'a [u8],
    /// Compressed alpha data (4444 + alpha streams only).
    pub alpha: Option<&'a [u8]>,
}

/// Split the bytes immediately following a slice header into per-plane
/// sub-slices using the sizes declared in the header.
///
/// `luma_size + cb_size + cr_size + alpha_size` must equal `data.len()`;
/// the function returns [`DecodeError::PlaneOverrun`] otherwise. This
/// catches truncated streams and bit-flip corruption at slice boundaries.
pub fn split_slice_planes<'a>(
    data: &'a [u8],
    luma_size: u16,
    cb_size: u16,
    cr_size: u16,
    alpha_size: Option<u16>,
) -> Result<SliceData<'a>, DecodeError> {
    let total = usize::from(luma_size)
        + usize::from(cb_size)
        + usize::from(cr_size)
        + alpha_size.map_or(0, usize::from);
    if total != data.len() {
        return Err(DecodeError::PlaneOverrun(
            "sum of plane sizes != slice data length",
        ));
    }
    let mut cursor = 0usize;
    let luma = &data[cursor..cursor + usize::from(luma_size)];
    cursor += usize::from(luma_size);
    let cb = &data[cursor..cursor + usize::from(cb_size)];
    cursor += usize::from(cb_size);
    let cr = &data[cursor..cursor + usize::from(cr_size)];
    cursor += usize::from(cr_size);
    let alpha = alpha_size.map(|s| &data[cursor..cursor + usize::from(s)]);
    Ok(SliceData {
        luma,
        cb,
        cr,
        alpha,
    })
}

/// Decode one slice's worth of entropy-coded plane data into 10-bit
/// luma + 10-bit chroma sample arrays.
///
/// **This function is the Phase 2 entry point.** Currently returns
/// [`DecodeError::NotImplemented`]. The signature, output shape, and
/// docstring are stable — the implementation goes here.
///
/// `quant_matrix_luma` and `quant_matrix_chroma` come from the frame
/// header. `qscale` comes from the slice header. `slice_mb_width` is
/// the macroblock width of this slice (typically 8).
///
/// On success the destination plane buffers must be filled with
/// reconstructed 10-bit samples (0..=1023), row-major. Luma is
/// `slice_mb_width * 16 × 16` samples; chroma is `slice_mb_width * 8 × 16`
/// for 4:2:2.
pub fn decode_slice_to_yuv422(
    _slice: SliceData<'_>,
    _quant_matrix_luma: &[u8; 64],
    _quant_matrix_chroma: &[u8; 64],
    _qscale: u8,
    _slice_mb_width: usize,
    _dst_luma: &mut [u16],
    _dst_cb: &mut [u16],
    _dst_cr: &mut [u16],
) -> Result<(), DecodeError> {
    Err(DecodeError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_planes_no_alpha() {
        let data: Vec<u8> = (0u8..30).collect();
        let s = split_slice_planes(&data, 10, 8, 12, None).unwrap();
        assert_eq!(s.luma.len(), 10);
        assert_eq!(s.cb.len(), 8);
        assert_eq!(s.cr.len(), 12);
        assert!(s.alpha.is_none());
        // Verify contiguous layout: each plane starts where previous ended.
        assert_eq!(s.luma[0], 0);
        assert_eq!(s.cb[0], 10);
        assert_eq!(s.cr[0], 18);
    }

    #[test]
    fn split_planes_with_alpha() {
        let data: Vec<u8> = (0u8..40).collect();
        let s = split_slice_planes(&data, 10, 8, 12, Some(10)).unwrap();
        assert_eq!(s.alpha.unwrap().len(), 10);
        assert_eq!(s.alpha.unwrap()[0], 30);
    }

    #[test]
    fn split_planes_size_mismatch_errors() {
        let data = [0u8; 30];
        assert!(matches!(
            split_slice_planes(&data, 10, 8, 13, None).unwrap_err(),
            DecodeError::PlaneOverrun(_)
        ));
    }

    #[test]
    fn decode_slice_phase2_stub_fails_loudly() {
        // The Phase-2 entry point must NOT silently produce zeros; it
        // returns NotImplemented until the real pipeline lands. This
        // pins the public surface so callers don't accidentally rely
        // on a no-op decoder.
        let data = [0u8; 0];
        let slice = SliceData {
            luma: &data,
            cb: &data,
            cr: &data,
            alpha: None,
        };
        let mut y = vec![0u16; 256];
        let mut cb = vec![0u16; 128];
        let mut cr = vec![0u16; 128];
        let zero_mat = [4u8; 64];
        let err = decode_slice_to_yuv422(slice, &zero_mat, &zero_mat, 4, 1, &mut y, &mut cb, &mut cr)
            .unwrap_err();
        assert!(matches!(err, DecodeError::NotImplemented));
    }
}
