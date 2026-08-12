// Copyright 2024 OxiMedia Project
// Licensed under the Apache License, Version 2.0

//! Shared image decode + pixel-format normalization for rendered frames.
//!
//! Both [`crate::output_assembly`] and [`crate::quality_metrics`] need to
//! turn a rendered frame file's bytes into real pixel data:
//!
//! - Assembly needs it to learn a JPEG frame's real width/height (for the
//!   MJPEG-in-AVI route), or to get real YUV420p planes to write into a Y4M
//!   stream (for any other decodable format).
//! - Quality metrics need real YUV420p/grayscale planes to feed into
//!   `oximedia_quality`'s per-plane algorithms (PSNR/SSIM/blockiness/blur),
//!   all of which treat `planes[0]` as literal per-pixel luma/intensity
//!   data.
//!
//! Centralizing decode+normalize here matters for correctness, not just
//! deduplication: `oximedia_codec::image::ImageDecoder` returns packed
//! RGB24/RGBA32 for JPEG/PNG/WebP sources (one plane, 3-4 bytes per pixel),
//! and feeding that directly to a plane-indexed algorithm that assumes
//! `planes[0].len() == width * height` would silently read the wrong bytes
//! at the wrong offsets -- a real-looking but semantically meaningless
//! score, not an honest failure. [`decode_normalized`] always converts
//! RGB/RGBA to real YUV420p planes first (via
//! `oximedia_codec::image::convert_rgb_to_yuv420p`) so callers only ever see
//! a frame whose planes are genuinely addressable by `y * width + x`.

use crate::error::{Error, Result};
use oximedia_codec::frame::VideoFrame;
use oximedia_codec::image::{convert_rgb_to_yuv420p, ImageDecoder};
use oximedia_core::PixelFormat;

/// Decodes an image file's bytes and returns its real width/height, without
/// normalizing pixel data. Used by the MJPEG-in-AVI assembly route, which
/// muxes the original JPEG bytes unchanged and only needs the dimensions.
///
/// # Errors
///
/// Returns `Err` if the bytes cannot be decoded by any format
/// `oximedia_codec::image::ImageDecoder` supports.
pub(crate) fn decode_dimensions(data: &[u8]) -> Result<(u32, u32)> {
    let frame = ImageDecoder::new(data)
        .and_then(|d| d.decode())
        .map_err(|e| Error::Other(format!("frame decode failed: {e}")))?;
    Ok((frame.width, frame.height))
}

/// Decodes an image file's bytes and normalizes the result so `planes[0]`
/// is real per-pixel luma (or grayscale) intensity data: already-planar
/// YUV420p/grayscale frames pass through unchanged, RGB24/RGBA32 frames are
/// converted to YUV420p.
///
/// # Errors
///
/// Returns `Err` if the bytes cannot be decoded, or decode to a pixel
/// format this function does not know how to normalize (e.g. any of the
/// packed HDR/semi-planar formats `oximedia_codec::image` does not
/// currently produce from still images).
pub(crate) fn decode_normalized(data: &[u8]) -> Result<VideoFrame> {
    let frame = ImageDecoder::new(data)
        .and_then(|d| d.decode())
        .map_err(|e| Error::Other(format!("frame decode failed: {e}")))?;

    match frame.format {
        PixelFormat::Rgb24 | PixelFormat::Rgba32 => convert_rgb_to_yuv420p(&frame)
            .map_err(|e| Error::Other(format!("RGB->YUV420p normalization failed: {e}"))),
        PixelFormat::Yuv420p | PixelFormat::Gray8 => Ok(frame),
        other => Err(Error::Other(format!(
            "frame decoded to a pixel format not supported for assembly/quality processing: \
             {other:?}"
        ))),
    }
}

/// Converts a normalized [`VideoFrame`] (see [`decode_normalized`]) into an
/// [`oximedia_quality::Frame`] for use with PSNR/SSIM/blockiness/blur.
pub(crate) fn to_quality_frame(frame: &VideoFrame) -> oximedia_quality::Frame {
    oximedia_quality::Frame {
        width: frame.width as usize,
        height: frame.height as usize,
        format: frame.format,
        planes: frame.planes.iter().map(|p| p.data.clone()).collect(),
        strides: frame.planes.iter().map(|p| p.stride).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_dimensions_rejects_garbage() {
        let result = decode_dimensions(b"not an image");
        assert!(result.is_err());
    }

    #[test]
    fn decode_normalized_rejects_garbage() {
        let result = decode_normalized(b"not an image");
        assert!(result.is_err());
    }

    #[test]
    fn decode_normalized_png_roundtrip_yields_plausible_yuv() {
        // Build a tiny real PNG via oximedia_codec's own encoder, then
        // confirm decode_normalized produces properly-addressable YUV420p
        // planes (not packed RGB reinterpreted as luma).
        use oximedia_codec::frame::{Plane, VideoFrame as CodecVideoFrame};
        use oximedia_codec::image::{EncoderConfig, ImageEncoder};

        let width = 16u32;
        let height = 16u32;
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.push((x * 16) as u8);
                rgb.push((y * 16) as u8);
                rgb.push(128);
            }
        }
        let mut frame = CodecVideoFrame::new(PixelFormat::Rgb24, width, height);
        frame.planes = vec![Plane::with_dimensions(
            rgb,
            (width * 3) as usize,
            width,
            height,
        )];

        let encoder = ImageEncoder::new(EncoderConfig::png());
        let png_bytes = encoder
            .encode(&frame)
            .expect("PNG encode should succeed in test");

        let normalized = decode_normalized(&png_bytes).expect("should decode normalized PNG");
        assert_eq!(normalized.format, PixelFormat::Yuv420p);
        assert_eq!(normalized.planes.len(), 3);
        assert_eq!(normalized.planes[0].data.len(), (width * height) as usize);
    }
}
