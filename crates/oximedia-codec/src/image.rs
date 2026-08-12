//! Image I/O for thumbnails and frame extraction.
//!
//! This module provides patent-free image encoding and decoding support:
//!
//! - **PNG** - Lossless compression, supports transparency
//! - **JPEG** - Lossy compression (decode only, patent concerns)
//! - **WebP** - Modern format with both lossy and lossless modes
//!
//! # Examples
//!
//! ## Decoding
//!
//! ```ignore
//! use oximedia_codec::image::{ImageDecoder, ImageFormat};
//!
//! let data = std::fs::read("frame.png")?;
//! let decoder = ImageDecoder::new(&data)?;
//! let frame = decoder.decode()?;
//! println!("Decoded {}x{} frame", frame.width, frame.height);
//! ```
//!
//! ## Encoding
//!
//! ```ignore
//! use oximedia_codec::image::{ImageEncoder, ImageFormat, EncoderConfig};
//!
//! let config = EncoderConfig::png();
//! let encoder = ImageEncoder::new(config);
//! let data = encoder.encode(&frame)?;
//! std::fs::write("output.png", &data)?;
//! ```

use crate::error::{CodecError, CodecResult};
use crate::frame::{Plane, VideoFrame};
use bytes::Bytes;
use oximedia_core::PixelFormat;
use std::io::Cursor;

/// Supported image formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// PNG - Portable Network Graphics (lossless).
    Png,
    /// JPEG - Joint Photographic Experts Group (lossy, decode only).
    Jpeg,
    /// WebP - Modern image format (lossy/lossless).
    WebP,
}

impl ImageFormat {
    /// Detect format from file signature.
    ///
    /// # Errors
    ///
    /// Returns error if format cannot be detected.
    pub fn from_bytes(data: &[u8]) -> CodecResult<Self> {
        if data.len() < 12 {
            return Err(CodecError::InvalidData("Data too short".into()));
        }

        // PNG signature: 89 50 4E 47 0D 0A 1A 0A
        if data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
            return Ok(Self::Png);
        }

        // JPEG signature: FF D8 FF
        if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
            return Ok(Self::Jpeg);
        }

        // WebP signature: RIFF....WEBP
        if data.starts_with(b"RIFF") && data.len() >= 12 && &data[8..12] == b"WEBP" {
            return Ok(Self::WebP);
        }

        Err(CodecError::UnsupportedFeature(
            "Unknown image format".into(),
        ))
    }

    /// Get file extension for this format.
    #[must_use]
    pub const fn extension(&self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::WebP => "webp",
        }
    }

    /// Check if format supports alpha channel.
    #[must_use]
    pub const fn supports_alpha(&self) -> bool {
        match self {
            Self::Png | Self::WebP => true,
            Self::Jpeg => false,
        }
    }
}

/// Image encoder configuration.
#[derive(Clone, Debug)]
pub struct EncoderConfig {
    /// Output format.
    pub format: ImageFormat,
    /// Quality setting (0-100, higher is better).
    /// Only used for lossy formats (WebP lossy mode).
    pub quality: u8,
    /// Use lossless compression (WebP only).
    pub lossless: bool,
}

impl EncoderConfig {
    /// Create PNG encoder config (lossless).
    #[must_use]
    pub const fn png() -> Self {
        Self {
            format: ImageFormat::Png,
            quality: 100,
            lossless: true,
        }
    }

    /// Create WebP encoder config with lossy compression.
    ///
    /// # Arguments
    ///
    /// * `quality` - Quality setting (0-100, higher is better)
    #[must_use]
    pub const fn webp_lossy(quality: u8) -> Self {
        Self {
            format: ImageFormat::WebP,
            quality,
            lossless: false,
        }
    }

    /// Create WebP encoder config with lossless compression.
    #[must_use]
    pub const fn webp_lossless() -> Self {
        Self {
            format: ImageFormat::WebP,
            quality: 100,
            lossless: true,
        }
    }
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self::png()
    }
}

/// Image decoder for converting image files to video frames.
pub struct ImageDecoder {
    format: ImageFormat,
    data: Bytes,
}

impl ImageDecoder {
    /// Create a new image decoder.
    ///
    /// # Errors
    ///
    /// Returns error if format cannot be detected.
    pub fn new(data: &[u8]) -> CodecResult<Self> {
        let format = ImageFormat::from_bytes(data)?;
        Ok(Self {
            format,
            data: Bytes::copy_from_slice(data),
        })
    }

    /// Get the detected format.
    #[must_use]
    pub const fn format(&self) -> ImageFormat {
        self.format
    }

    /// Decode the image to a video frame.
    ///
    /// # Errors
    ///
    /// Returns error if decoding fails.
    #[allow(clippy::too_many_lines)]
    pub fn decode(&self) -> CodecResult<VideoFrame> {
        match self.format {
            ImageFormat::Png => self.decode_png(),
            ImageFormat::Jpeg => self.decode_jpeg(),
            ImageFormat::WebP => self.decode_webp(),
        }
    }

    #[cfg(feature = "image-io")]
    fn decode_png(&self) -> CodecResult<VideoFrame> {
        let decoder = png::Decoder::new(Cursor::new(&self.data));
        let mut reader = decoder
            .read_info()
            .map_err(|e| CodecError::DecoderError(format!("PNG decode error: {e}")))?;

        let info = reader.info();
        let width = info.width;
        let height = info.height;
        let color_type = info.color_type;

        // Allocate buffer for decoded image
        let buffer_size = reader.output_buffer_size().ok_or_else(|| {
            CodecError::DecoderError("Cannot determine PNG output buffer size".into())
        })?;
        let mut buf = vec![0u8; buffer_size];
        let output_info = reader
            .next_frame(&mut buf)
            .map_err(|e| CodecError::DecoderError(format!("PNG decode error: {e}")))?;

        // Convert to appropriate pixel format
        let (format, data) = match color_type {
            png::ColorType::Rgb => {
                // RGB 8-bit
                (
                    PixelFormat::Rgb24,
                    buf[..output_info.buffer_size()].to_vec(),
                )
            }
            png::ColorType::Rgba => {
                // RGBA 8-bit
                (
                    PixelFormat::Rgba32,
                    buf[..output_info.buffer_size()].to_vec(),
                )
            }
            png::ColorType::Grayscale => {
                // Grayscale 8-bit
                (
                    PixelFormat::Gray8,
                    buf[..output_info.buffer_size()].to_vec(),
                )
            }
            png::ColorType::GrayscaleAlpha => {
                // Convert grayscale+alpha to RGBA
                let size = (width * height) as usize;
                let mut rgba = Vec::with_capacity(size * 4);
                for chunk in buf[..output_info.buffer_size()].chunks_exact(2) {
                    let gray = chunk[0];
                    let alpha = chunk[1];
                    rgba.extend_from_slice(&[gray, gray, gray, alpha]);
                }
                (PixelFormat::Rgba32, rgba)
            }
            png::ColorType::Indexed => {
                return Err(CodecError::UnsupportedFeature(
                    "Indexed PNG not supported".into(),
                ))
            }
        };

        // Create frame with single plane (packed format)
        let stride = data.len() / height as usize;
        let plane = Plane {
            data,
            stride,
            width,
            height,
        };

        let mut frame = VideoFrame::new(format, width, height);
        frame.planes = vec![plane];

        Ok(frame)
    }

    #[cfg(not(feature = "image-io"))]
    fn decode_png(&self) -> CodecResult<VideoFrame> {
        Err(CodecError::UnsupportedFeature(
            "PNG support not enabled".into(),
        ))
    }

    #[cfg(feature = "image-io")]
    fn decode_jpeg(&self) -> CodecResult<VideoFrame> {
        let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(&self.data));
        let pixels = decoder
            .decode()
            .map_err(|e| CodecError::DecoderError(format!("JPEG decode error: {e}")))?;

        let info = decoder
            .info()
            .ok_or_else(|| CodecError::DecoderError("No JPEG info available".into()))?;

        let width = u32::from(info.width);
        let height = u32::from(info.height);

        // JPEG decoder outputs RGB or grayscale
        let (format, data) = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => (PixelFormat::Rgb24, pixels),
            jpeg_decoder::PixelFormat::L8 => (PixelFormat::Gray8, pixels),
            jpeg_decoder::PixelFormat::CMYK32 => {
                // Convert CMYK to RGB
                let mut rgb = Vec::with_capacity((width * height * 3) as usize);
                for chunk in pixels.chunks_exact(4) {
                    let c = f32::from(chunk[0]) / 255.0;
                    let m = f32::from(chunk[1]) / 255.0;
                    let y = f32::from(chunk[2]) / 255.0;
                    let k = f32::from(chunk[3]) / 255.0;

                    let r = ((1.0 - c) * (1.0 - k) * 255.0) as u8;
                    let g = ((1.0 - m) * (1.0 - k) * 255.0) as u8;
                    let b = ((1.0 - y) * (1.0 - k) * 255.0) as u8;

                    rgb.extend_from_slice(&[r, g, b]);
                }
                (PixelFormat::Rgb24, rgb)
            }
            _ => {
                return Err(CodecError::UnsupportedFeature(format!(
                    "JPEG pixel format {:?} not supported",
                    info.pixel_format
                )))
            }
        };

        let stride = data.len() / height as usize;
        let plane = Plane {
            data,
            stride,
            width,
            height,
        };

        let mut frame = VideoFrame::new(format, width, height);
        frame.planes = vec![plane];

        Ok(frame)
    }

    #[cfg(not(feature = "image-io"))]
    fn decode_jpeg(&self) -> CodecResult<VideoFrame> {
        Err(CodecError::UnsupportedFeature(
            "JPEG support not enabled".into(),
        ))
    }

    #[cfg(feature = "image-io")]
    fn decode_webp(&self) -> CodecResult<VideoFrame> {
        use crate::webp::alpha::decode_alpha;
        use crate::webp::riff::{WebPContainer, WebPEncoding};
        use crate::webp::vp8l_decoder::Vp8lDecoder;

        let container = WebPContainer::parse(&self.data)?;
        let (width, height) = container.dimensions()?;

        match container.encoding {
            WebPEncoding::Lossless => {
                let chunk = container.bitstream_chunk().ok_or_else(|| {
                    CodecError::DecoderError("No VP8L bitstream chunk found".into())
                })?;
                let mut decoder = Vp8lDecoder::new();
                let decoded = decoder.decode(&chunk.data)?;
                Self::decoded_image_to_frame(&decoded)
            }
            WebPEncoding::Lossy => {
                let chunk = container.bitstream_chunk().ok_or_else(|| {
                    CodecError::DecoderError("No VP8 bitstream chunk found".into())
                })?;
                Self::decode_vp8_to_frame(&chunk.data, width, height)
            }
            WebPEncoding::Extended => {
                // Check for VP8L (lossless) bitstream first
                let vp8l_chunk = container
                    .chunks
                    .iter()
                    .find(|c| c.chunk_type == crate::webp::riff::ChunkType::Vp8L);
                let vp8_chunk = container
                    .chunks
                    .iter()
                    .find(|c| c.chunk_type == crate::webp::riff::ChunkType::Vp8);
                let alpha_chunk = container.alpha_chunk();

                if let Some(vp8l) = vp8l_chunk {
                    // Extended with VP8L (lossless with alpha)
                    let mut decoder = Vp8lDecoder::new();
                    let decoded = decoder.decode(&vp8l.data)?;
                    Self::decoded_image_to_frame(&decoded)
                } else if let Some(vp8) = vp8_chunk {
                    // Extended with VP8 (lossy, possibly with separate alpha)
                    let mut frame = Self::decode_vp8_to_frame(&vp8.data, width, height)?;

                    // Merge ALPH chunk alpha into the frame if present
                    if let Some(alph) = alpha_chunk {
                        let alpha_plane = decode_alpha(&alph.data, width, height)?;
                        // Convert RGB24 frame to RGBA32 with the decoded alpha
                        if frame.format == PixelFormat::Rgb24 && !frame.planes.is_empty() {
                            let rgb = &frame.planes[0].data;
                            let mut rgba =
                                Vec::with_capacity((width as usize) * (height as usize) * 4);
                            for (i, rgb_chunk) in rgb.chunks_exact(3).enumerate() {
                                rgba.extend_from_slice(rgb_chunk);
                                let a = alpha_plane.get(i).copied().unwrap_or(255);
                                rgba.push(a);
                            }
                            let stride = (width as usize) * 4;
                            frame = VideoFrame::new(PixelFormat::Rgba32, width, height);
                            frame.planes = vec![Plane {
                                data: rgba,
                                stride,
                                width,
                                height,
                            }];
                        }
                    }
                    Ok(frame)
                } else {
                    Err(CodecError::DecoderError(
                        "Extended WebP has no VP8 or VP8L bitstream".into(),
                    ))
                }
            }
        }
    }

    /// Convert a VP8L `DecodedImage` (ARGB u32 pixels) to a `VideoFrame`.
    fn decoded_image_to_frame(
        decoded: &crate::webp::vp8l_decoder::DecodedImage,
    ) -> CodecResult<VideoFrame> {
        let width = decoded.width;
        let height = decoded.height;
        let has_alpha = decoded.has_alpha;

        let mut rgba = Vec::with_capacity((width as usize) * (height as usize) * 4);
        for &pixel in &decoded.pixels {
            let a = ((pixel >> 24) & 0xFF) as u8;
            let r = ((pixel >> 16) & 0xFF) as u8;
            let g = ((pixel >> 8) & 0xFF) as u8;
            let b = (pixel & 0xFF) as u8;
            rgba.extend_from_slice(&[r, g, b, a]);
        }

        if !has_alpha {
            // Convert to RGB24
            let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
            for chunk in rgba.chunks_exact(4) {
                rgb.extend_from_slice(&chunk[..3]);
            }
            let stride = rgb.len() / height as usize;
            let plane = Plane {
                data: rgb,
                stride,
                width,
                height,
            };
            let mut frame = VideoFrame::new(PixelFormat::Rgb24, width, height);
            frame.planes = vec![plane];
            Ok(frame)
        } else {
            let stride = rgba.len() / height as usize;
            let plane = Plane {
                data: rgba,
                stride,
                width,
                height,
            };
            let mut frame = VideoFrame::new(PixelFormat::Rgba32, width, height);
            frame.planes = vec![plane];
            Ok(frame)
        }
    }

    /// Decode a VP8 lossy bitstream to an RGB24 `VideoFrame`.
    #[cfg(feature = "vp8")]
    fn decode_vp8_to_frame(data: &[u8], _width: u32, _height: u32) -> CodecResult<VideoFrame> {
        // Delegates to the dedicated `webp::vp8_decoder` module (bit-exact
        // vs libwebp; see `crates/oximedia-codec/tests/webp_vp8_lossy.rs`)
        // rather than driving a full `Vp8Decoder` `VideoDecoder` state
        // machine for what is always exactly one still-image key frame, so
        // there is a single lossy-WebP decode implementation, not two. Both
        // ultimately call the same `vp8::decode_keyframe` pipeline.
        let yuv_frame = crate::webp::vp8_decoder::decode_vp8_keyframe(data)?;
        convert_yuv420p_to_rgb(&yuv_frame)
    }

    /// Decode a VP8 lossy bitstream to an RGB24 `VideoFrame` (stub when vp8 feature disabled).
    #[cfg(not(feature = "vp8"))]
    fn decode_vp8_to_frame(_data: &[u8], _width: u32, _height: u32) -> CodecResult<VideoFrame> {
        Err(CodecError::UnsupportedFeature(
            "VP8 lossy decoding requires the 'vp8' feature".into(),
        ))
    }

    #[cfg(not(feature = "image-io"))]
    fn decode_webp(&self) -> CodecResult<VideoFrame> {
        Err(CodecError::UnsupportedFeature(
            "WebP support not enabled".into(),
        ))
    }
}

/// Image encoder for converting video frames to image files.
pub struct ImageEncoder {
    config: EncoderConfig,
}

impl ImageEncoder {
    /// Create a new image encoder.
    #[must_use]
    pub const fn new(config: EncoderConfig) -> Self {
        Self { config }
    }

    /// Encode a video frame to image data.
    ///
    /// # Errors
    ///
    /// Returns error if encoding fails or frame format is unsupported.
    pub fn encode(&self, frame: &VideoFrame) -> CodecResult<Vec<u8>> {
        match self.config.format {
            ImageFormat::Png => self.encode_png(frame),
            ImageFormat::Jpeg => Err(CodecError::UnsupportedFeature(
                "JPEG encoding not supported (patent concerns)".into(),
            )),
            ImageFormat::WebP => self.encode_webp(frame),
        }
    }

    #[cfg(feature = "image-io")]
    #[allow(clippy::too_many_lines)]
    fn encode_png(&self, frame: &VideoFrame) -> CodecResult<Vec<u8>> {
        let mut output = Vec::new();
        let mut encoder = png::Encoder::new(Cursor::new(&mut output), frame.width, frame.height);

        // Set color type based on pixel format
        let (color_type, bit_depth) = match frame.format {
            PixelFormat::Rgb24 => (png::ColorType::Rgb, png::BitDepth::Eight),
            PixelFormat::Rgba32 => (png::ColorType::Rgba, png::BitDepth::Eight),
            PixelFormat::Gray8 => (png::ColorType::Grayscale, png::BitDepth::Eight),
            PixelFormat::Gray16 => (png::ColorType::Grayscale, png::BitDepth::Sixteen),
            _ => {
                return Err(CodecError::UnsupportedFeature(format!(
                    "Pixel format {} not supported for PNG encoding",
                    frame.format
                )))
            }
        };

        encoder.set_color(color_type);
        encoder.set_depth(bit_depth);
        encoder.set_compression(png::Compression::default());

        let mut writer = encoder
            .write_header()
            .map_err(|e| CodecError::Internal(format!("PNG encode error: {e}")))?;

        // Get pixel data from frame
        if frame.planes.is_empty() {
            return Err(CodecError::InvalidData("Frame has no planes".into()));
        }

        writer
            .write_image_data(&frame.planes[0].data)
            .map_err(|e| CodecError::Internal(format!("PNG encode error: {e}")))?;

        writer
            .finish()
            .map_err(|e| CodecError::Internal(format!("PNG encode error: {e}")))?;

        Ok(output)
    }

    #[cfg(not(feature = "image-io"))]
    fn encode_png(&self, _frame: &VideoFrame) -> CodecResult<Vec<u8>> {
        Err(CodecError::UnsupportedFeature(
            "PNG support not enabled".into(),
        ))
    }

    #[cfg(feature = "image-io")]
    fn encode_webp(&self, frame: &VideoFrame) -> CodecResult<Vec<u8>> {
        // Get RGB/RGBA data from frame
        let (width, height, data) = match frame.format {
            PixelFormat::Rgb24 | PixelFormat::Rgba32 => {
                if frame.planes.is_empty() {
                    return Err(CodecError::InvalidData("Frame has no planes".into()));
                }
                (frame.width, frame.height, &frame.planes[0].data)
            }
            PixelFormat::Gray8 => {
                // Convert grayscale to RGB
                if frame.planes.is_empty() {
                    return Err(CodecError::InvalidData("Frame has no planes".into()));
                }
                let gray_data = &frame.planes[0].data;
                let mut rgb = Vec::with_capacity(gray_data.len() * 3);
                for &gray in gray_data.iter() {
                    rgb.extend_from_slice(&[gray, gray, gray]);
                }
                return self.encode_webp_rgb(frame.width, frame.height, &rgb, false);
            }
            _ => {
                return Err(CodecError::UnsupportedFeature(format!(
                    "Pixel format {} not supported for WebP encoding",
                    frame.format
                )))
            }
        };

        let has_alpha = frame.format == PixelFormat::Rgba32;
        self.encode_webp_rgb(width, height, data, has_alpha)
    }

    fn encode_webp_rgb(
        &self,
        width: u32,
        height: u32,
        data: &[u8],
        has_alpha: bool,
    ) -> CodecResult<Vec<u8>> {
        use crate::webp::alpha::encode_alpha;
        use crate::webp::encoder::WebPLossyEncoder;
        use crate::webp::riff::WebPWriter;
        use crate::webp::vp8l_encoder::Vp8lEncoder;

        if self.config.lossless {
            // Convert RGB/RGBA bytes to ARGB u32 pixels
            let pixel_count = (width as usize) * (height as usize);
            let mut pixels = Vec::with_capacity(pixel_count);

            if has_alpha {
                for chunk in data.chunks_exact(4) {
                    let r = u32::from(chunk[0]);
                    let g = u32::from(chunk[1]);
                    let b = u32::from(chunk[2]);
                    let a = u32::from(chunk[3]);
                    pixels.push((a << 24) | (r << 16) | (g << 8) | b);
                }
            } else {
                for chunk in data.chunks_exact(3) {
                    let r = u32::from(chunk[0]);
                    let g = u32::from(chunk[1]);
                    let b = u32::from(chunk[2]);
                    pixels.push((0xFF << 24) | (r << 16) | (g << 8) | b);
                }
            }

            let encoder = Vp8lEncoder::new(100);
            let vp8l_data = encoder.encode(&pixels, width, height, has_alpha)?;
            Ok(WebPWriter::write_lossless(&vp8l_data))
        } else {
            let quality = self.config.quality.clamp(0, 100);
            let lossy_encoder = WebPLossyEncoder::new(quality);

            if has_alpha {
                let (vp8_data, alpha_data) = lossy_encoder.encode_rgba(data, width, height)?;
                let alpha_chunk = encode_alpha(&alpha_data, width, height)?;
                Ok(WebPWriter::write_extended(
                    &vp8_data,
                    Some(&alpha_chunk),
                    width,
                    height,
                ))
            } else {
                let vp8_data = lossy_encoder.encode_rgb(data, width, height)?;
                Ok(WebPWriter::write_lossy(&vp8_data))
            }
        }
    }

    #[cfg(not(feature = "image-io"))]
    fn encode_webp(&self, _frame: &VideoFrame) -> CodecResult<Vec<u8>> {
        Err(CodecError::UnsupportedFeature(
            "WebP support not enabled".into(),
        ))
    }
}

/// Convert RGB to YUV color space.
///
/// Uses BT.709 coefficients for HD content.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
pub fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r = f32::from(r);
    let g = f32::from(g);
    let b = f32::from(b);

    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let u = (b - y) / 1.8556 + 128.0;
    let v = (r - y) / 1.5748 + 128.0;

    (
        y.clamp(0.0, 255.0) as u8,
        u.clamp(0.0, 255.0) as u8,
        v.clamp(0.0, 255.0) as u8,
    )
}

/// Convert YUV to RGB color space.
///
/// Uses BT.709 coefficients for HD content.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)]
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let y = f32::from(y);
    let u = f32::from(u) - 128.0;
    let v = f32::from(v) - 128.0;

    let r = y + 1.5748 * v;
    let g = y - 0.1873 * u - 0.4681 * v;
    let b = y + 1.8556 * u;

    (
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    )
}

/// Convert a video frame from RGB to YUV420p format.
///
/// Chroma planes are sized by ceiling division (`div_ceil(2)`) of width and
/// height, matching the VP8/WebP odd-dimension convention (see
/// `webp::vp8_decoder::yuv420p_to_rgb24`): an odd width or height still
/// produces one extra chroma column/row rather than being truncated. For
/// even `width`/`height` this is identical to floor division.
///
/// # Errors
///
/// - Returns [`CodecError::InvalidParameter`] if frame is not in RGB24 or
///   Rgba32 format.
/// - Returns [`CodecError::InvalidData`] if the frame has no planes, or if
///   the RGB/RGBA plane is shorter than `width * height * bytes_per_pixel`.
pub fn convert_rgb_to_yuv420p(frame: &VideoFrame) -> CodecResult<VideoFrame> {
    if !matches!(frame.format, PixelFormat::Rgb24 | PixelFormat::Rgba32) {
        return Err(CodecError::InvalidParameter(
            "Frame must be RGB24 or Rgba32".into(),
        ));
    }

    if frame.planes.is_empty() {
        return Err(CodecError::InvalidData("Frame has no planes".into()));
    }

    let width = frame.width as usize;
    let height = frame.height as usize;
    let rgb_data = &frame.planes[0].data;
    let bytes_per_pixel = if frame.format == PixelFormat::Rgb24 {
        3
    } else {
        4
    };

    let expected_rgb_len = width * height * bytes_per_pixel;
    if rgb_data.len() < expected_rgb_len {
        return Err(CodecError::InvalidData(format!(
            "RGB plane too short: expected at least {expected_rgb_len}, got {}",
            rgb_data.len()
        )));
    }

    // Allocate YUV planes. 4:2:0 chroma planes are ceiling-divided
    // (VP8/WebP odd-dimension convention), not floor-divided -- an odd
    // width/height still gets one extra chroma column/row.
    let y_size = width * height;
    let uv_width = width.div_ceil(2);
    let uv_height = height.div_ceil(2);
    let uv_size = uv_width * uv_height;

    let mut y_plane = vec![0u8; y_size];
    let mut u_plane = vec![0u8; uv_size];
    let mut v_plane = vec![0u8; uv_size];

    // Convert RGB to YUV420p
    for y in 0..height {
        for x in 0..width {
            let rgb_idx = (y * width + x) * bytes_per_pixel;
            let r = rgb_data[rgb_idx];
            let g = rgb_data[rgb_idx + 1];
            let b = rgb_data[rgb_idx + 2];

            let (y_val, u_val, v_val) = rgb_to_yuv(r, g, b);
            y_plane[y * width + x] = y_val;

            // Subsample U and V (4:2:0): sample the top-left pixel of each
            // 2x2 block. `uv_width` is ceil-divided, so the last (partial)
            // column/row of an odd-sized frame still lands on a valid
            // chroma index -- before this fix, `uv_width` was floor-divided
            // here while `x`/`y` still walked up to the odd `width`/
            // `height`, writing `u_plane[(y / 2) * uv_width + x / 2]` out of
            // bounds (e.g. 3x3: index 1 into a 1-byte plane).
            if x % 2 == 0 && y % 2 == 0 {
                let uv_idx = (y / 2) * uv_width + (x / 2);
                u_plane[uv_idx] = u_val;
                v_plane[uv_idx] = v_val;
            }
        }
    }

    let mut yuv_frame = VideoFrame::new(PixelFormat::Yuv420p, frame.width, frame.height);
    yuv_frame.planes = vec![
        Plane {
            data: y_plane,
            stride: width,
            width: frame.width,
            height: frame.height,
        },
        Plane {
            data: u_plane,
            stride: uv_width,
            width: frame.width.div_ceil(2),
            height: frame.height.div_ceil(2),
        },
        Plane {
            data: v_plane,
            stride: uv_width,
            width: frame.width.div_ceil(2),
            height: frame.height.div_ceil(2),
        },
    ];
    yuv_frame.timestamp = frame.timestamp;
    yuv_frame.frame_type = frame.frame_type;
    yuv_frame.color_info = frame.color_info;

    Ok(yuv_frame)
}

/// Convert a video frame from YUV420p to RGB24 format.
///
/// Chroma planes are addressed using ceiling division (`div_ceil(2)`) for
/// both width and height, matching the VP8/WebP odd-dimension convention
/// (see `webp::vp8_decoder::yuv420p_to_rgb24`): for an odd `width` or
/// `height`, a 4:2:0 chroma plane has one extra row/column beyond floor
/// division. For even `width`/`height` this is identical to floor division.
///
/// # Errors
///
/// - Returns [`CodecError::InvalidParameter`] if frame is not in YUV420p
///   format.
/// - Returns [`CodecError::InvalidData`] if the frame does not have exactly
///   3 planes, if the Y plane is shorter than `width * height`, or if the U
///   or V plane is shorter than the ceil-divided 4:2:0 chroma size
///   (`div_ceil(width, 2) * div_ceil(height, 2)`).
pub fn convert_yuv420p_to_rgb(frame: &VideoFrame) -> CodecResult<VideoFrame> {
    if frame.format != PixelFormat::Yuv420p {
        return Err(CodecError::InvalidParameter("Frame must be YUV420p".into()));
    }

    if frame.planes.len() != 3 {
        return Err(CodecError::InvalidData("YUV420p requires 3 planes".into()));
    }

    let width = frame.width as usize;
    let height = frame.height as usize;
    let y_data = &frame.planes[0].data;
    let u_data = &frame.planes[1].data;
    let v_data = &frame.planes[2].data;

    // 4:2:0 chroma planes are ceiling-divided (VP8/WebP odd-dimension
    // convention), not floor-divided: an odd width/height leaves one extra
    // chroma column/row. Before this fix, a floor-divided `uv_width` here
    // silently sheared chroma for an odd-width WebP decoded through
    // `ImageDecoder` (`decode_vp8_to_frame` emits correctly ceil-sized
    // planes; this converter then read them with the wrong stride).
    let uv_width = width.div_ceil(2);
    let uv_height = height.div_ceil(2);

    if y_data.len() < width * height {
        return Err(CodecError::InvalidData(format!(
            "Y plane too short: expected at least {}, got {}",
            width * height,
            y_data.len()
        )));
    }
    if u_data.len() < uv_width * uv_height {
        return Err(CodecError::InvalidData(format!(
            "U plane too short for ceil-divided 4:2:0 chroma: expected at least {}, got {}",
            uv_width * uv_height,
            u_data.len()
        )));
    }
    if v_data.len() < uv_width * uv_height {
        return Err(CodecError::InvalidData(format!(
            "V plane too short for ceil-divided 4:2:0 chroma: expected at least {}, got {}",
            uv_width * uv_height,
            v_data.len()
        )));
    }

    let rgb_size = width * height * 3;
    let mut rgb_data = vec![0u8; rgb_size];

    // Convert YUV420p to RGB
    for y in 0..height {
        for x in 0..width {
            let y_val = y_data[y * width + x];
            let uv_idx = (y / 2) * uv_width + (x / 2);
            let u_val = u_data[uv_idx];
            let v_val = v_data[uv_idx];

            let (r, g, b) = yuv_to_rgb(y_val, u_val, v_val);

            let rgb_idx = (y * width + x) * 3;
            rgb_data[rgb_idx] = r;
            rgb_data[rgb_idx + 1] = g;
            rgb_data[rgb_idx + 2] = b;
        }
    }

    let mut rgb_frame = VideoFrame::new(PixelFormat::Rgb24, frame.width, frame.height);
    rgb_frame.planes = vec![Plane {
        data: rgb_data,
        stride: width * 3,
        width: frame.width,
        height: frame.height,
    }];
    rgb_frame.timestamp = frame.timestamp;
    rgb_frame.frame_type = frame.frame_type;
    rgb_frame.color_info = frame.color_info;

    Ok(rgb_frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a 3x3 (odd-dimension) YUV420p `VideoFrame` by hand with
    /// correctly ceil-sized (2x2) chroma planes -- exactly what
    /// `webp::vp8_decoder::decode_vp8_keyframe` actually emits for an
    /// odd-width/height frame. Every chroma sample is distinct so a wrong
    /// index reads a visibly wrong value instead of coincidentally matching.
    fn make_odd_yuv420p_frame() -> VideoFrame {
        let width = 3u32;
        let height = 3u32;
        let y_plane: Vec<u8> = (0..9u32).map(|i| (20 + i * 20) as u8).collect();
        let u_plane = vec![10u8, 60, 110, 160];
        let v_plane = vec![20u8, 70, 120, 170];

        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
        frame.planes = vec![
            Plane {
                data: y_plane,
                stride: width as usize,
                width,
                height,
            },
            Plane {
                data: u_plane,
                stride: 2,
                width: 2,
                height: 2,
            },
            Plane {
                data: v_plane,
                stride: 2,
                width: 2,
                height: 2,
            },
        ];
        frame
    }

    #[test]
    fn convert_yuv420p_to_rgb_odd_width_reads_ceil_sized_chroma_correctly() {
        let frame = make_odd_yuv420p_frame();
        let y_data = frame.planes[0].data.clone();
        let u_data = frame.planes[1].data.clone();
        let v_data = frame.planes[2].data.clone();

        let rgb_frame = convert_yuv420p_to_rgb(&frame)
            .expect("odd-dimension 4:2:0 frame with correctly ceil-sized chroma must convert");
        assert_eq!(rgb_frame.width, 3);
        assert_eq!(rgb_frame.height, 3);
        let rgb = &rgb_frame.planes[0].data;
        let width = 3usize;

        let pixel = |x: usize, y: usize| -> (u8, u8, u8) {
            let idx = (y * width + x) * 3;
            (rgb[idx], rgb[idx + 1], rgb[idx + 2])
        };

        // (0,0): top-left, chroma index 0 -- baseline sanity check.
        assert_eq!(pixel(0, 0), yuv_to_rgb(y_data[0], u_data[0], v_data[0]));

        // (2,0): rightmost column of the top row. Chroma column = x/2 = 1,
        // which exists only because `uv_width` is ceil-divided to 2 (a
        // genuinely floor-sized plane would be 1 column wide and not have
        // this column at all). Row 0 makes the row-stride term zero, so
        // this pixel alone does not distinguish a floor-*stride* bug from
        // the fix -- see (0,2)/(2,2) below for that -- but it does pin the
        // column-index arithmetic.
        assert_eq!(pixel(2, 0), yuv_to_rgb(y_data[2], u_data[1], v_data[1]));

        // (0,2): bottom-left. Chroma row = y/2 = 1, column = x/2 = 0, so the
        // chroma index is `1 * uv_width + 0`. This is the silent-shear
        // pixel: a floor-divided `uv_width` (1, matching the pre-fix
        // `width / 2`) would read index `1 * 1 + 0 = 1` (`u_data[1]`,
        // in-bounds but WRONG) instead of the correct
        // `1 * 2 + 0 = 2` (`u_data[2]`) -- exactly the reported bug (wrong
        // pixel value, no panic), not an out-of-bounds crash.
        assert_eq!(pixel(0, 2), yuv_to_rgb(y_data[6], u_data[2], v_data[2]));

        // (2,2): bottom-right, chroma (row 1, col 1) of the 2-wide plane =
        // index 3. A floor-divided `uv_width` would read index
        // `1 * 1 + 1 = 2` (`u_data[2]`, also wrong) instead of the correct
        // `1 * 2 + 1 = 3` (`u_data[3]`).
        assert_eq!(pixel(2, 2), yuv_to_rgb(y_data[8], u_data[3], v_data[3]));
    }

    #[test]
    fn convert_yuv420p_to_rgb_rejects_short_floor_sized_chroma() {
        let width = 3u32;
        let height = 3u32;
        let y_plane = vec![128u8; 9];
        // Old floor-sized (1x1) chroma plane -- exactly what the pre-fix
        // `width / 2` computation would have sized it to.
        let u_plane = vec![128u8; 1];
        let v_plane = vec![128u8; 1];

        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
        frame.planes = vec![
            Plane {
                data: y_plane,
                stride: 3,
                width,
                height,
            },
            Plane {
                data: u_plane,
                stride: 1,
                width: 1,
                height: 1,
            },
            Plane {
                data: v_plane,
                stride: 1,
                width: 1,
                height: 1,
            },
        ];

        let result = convert_yuv420p_to_rgb(&frame);
        assert!(
            matches!(result, Err(CodecError::InvalidData(_))),
            "short (floor-sized) chroma planes must be a typed error, not a panic or Ok: {result:?}"
        );
    }

    #[test]
    fn convert_yuv420p_to_rgb_even_dims_4x2_unchanged() {
        // Even dimensions: div_ceil(2) == floor division, so this pins the
        // fix as a provable no-op for even dims (uv 2x1, as before).
        let width = 4u32;
        let height = 2u32;
        let y_plane = vec![128u8; 8];
        let u_plane = vec![64u8; 2];
        let v_plane = vec![192u8; 2];

        let mut frame = VideoFrame::new(PixelFormat::Yuv420p, width, height);
        frame.planes = vec![
            Plane {
                data: y_plane,
                stride: 4,
                width,
                height,
            },
            Plane {
                data: u_plane,
                stride: 2,
                width: 2,
                height: 1,
            },
            Plane {
                data: v_plane,
                stride: 2,
                width: 2,
                height: 1,
            },
        ];

        let rgb_frame = convert_yuv420p_to_rgb(&frame).expect("even-dimension frame must convert");
        assert_eq!(rgb_frame.planes[0].data.len(), 8 * 3);
        let expected = yuv_to_rgb(128, 64, 192);
        for px in rgb_frame.planes[0].data.chunks_exact(3) {
            assert_eq!((px[0], px[1], px[2]), expected);
        }
    }

    #[test]
    fn convert_rgb_to_yuv420p_odd_dims_no_longer_panics() {
        // 3x3 RGB24: before the fix, the chroma subsample loop wrote
        // u_plane[(y/2)*uv_width + x/2] with a floor-sized (1-byte) plane,
        // panicking out of bounds at x=2 or y=2.
        let width = 3u32;
        let height = 3u32;
        let rgb_data: Vec<u8> = (0..(width * height * 3))
            .map(|i| (i * 7 % 256) as u8)
            .collect();

        let mut frame = VideoFrame::new(PixelFormat::Rgb24, width, height);
        frame.planes = vec![Plane {
            data: rgb_data,
            stride: (width as usize) * 3,
            width,
            height,
        }];

        let yuv_frame = convert_rgb_to_yuv420p(&frame)
            .expect("odd-dimension RGB24 frame must convert without panicking");
        assert_eq!(yuv_frame.planes.len(), 3);
        assert_eq!(yuv_frame.planes[1].data.len(), 4); // ceil(3/2) * ceil(3/2) = 2*2
        assert_eq!(yuv_frame.planes[2].data.len(), 4);
        assert_eq!(yuv_frame.planes[1].width, 2);
        assert_eq!(yuv_frame.planes[1].height, 2);
        assert_eq!(yuv_frame.planes[2].width, 2);
        assert_eq!(yuv_frame.planes[2].height, 2);
    }

    #[test]
    fn convert_rgb_to_yuv420p_even_dims_4x2_unchanged() {
        // 4x2: even in both dimensions, so ceil == floor division and this
        // pins the pre-fix plane sizes/strides as a regression guard.
        let width = 4u32;
        let height = 2u32;
        let rgb_data = vec![128u8; (width * height * 3) as usize];

        let mut frame = VideoFrame::new(PixelFormat::Rgb24, width, height);
        frame.planes = vec![Plane {
            data: rgb_data,
            stride: (width as usize) * 3,
            width,
            height,
        }];

        let yuv_frame = convert_rgb_to_yuv420p(&frame).expect("even-dimension frame must convert");
        assert_eq!(yuv_frame.planes[0].data.len(), 8); // 4*2
        assert_eq!(yuv_frame.planes[1].data.len(), 2); // uv 2x1
        assert_eq!(yuv_frame.planes[2].data.len(), 2);
        assert_eq!(yuv_frame.planes[1].width, 2);
        assert_eq!(yuv_frame.planes[1].height, 1);
        assert_eq!(yuv_frame.planes[1].stride, 2);
        assert_eq!(yuv_frame.planes[2].width, 2);
        assert_eq!(yuv_frame.planes[2].height, 1);
    }
}
