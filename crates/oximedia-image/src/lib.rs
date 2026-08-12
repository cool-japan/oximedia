//! Professional image sequence I/O for cinema and VFX workflows.
//!
//! `oximedia-image` provides high-performance reading and writing of professional
//! image formats commonly used in film, television, and visual effects:
//!
//! - **DPX** (Digital Picture Exchange) - SMPTE 268M-2003 v2.0
//! - **`OpenEXR`** - High dynamic range format with deep images
//! - **TIFF** - Tagged Image File Format including `BigTIFF`
//!
//! # Features
//!
//! - Full color depth support (8, 10, 12, 16-bit, float, half-float)
//! - Linear and logarithmic color spaces
//! - Metadata preservation (camera, display window, etc.)
//! - Sequence pattern matching (printf-style, hash notation)
//! - Parallel I/O with rayon on native targets (`wasm32`, which has no threads,
//!   transparently falls back to sequential iteration)
//! - Zero-copy operations where possible
//!
//! # Example
//!
//! ```no_run
//! use oximedia_image::{ImageSequence, SequencePattern};
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Load a DPX sequence
//! let pattern = SequencePattern::parse("render.%04d.dpx")?;
//! let sequence = ImageSequence::from_pattern(pattern, 1..=100)?;
//!
//! // Read frame 50
//! let frame = sequence.read_frame(50)?;
//! println!("Frame 50: {}x{}", frame.width, frame.height);
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]
#![allow(
    clippy::module_name_repetitions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    dead_code,
    clippy::pedantic
)]

pub mod adaptive_threshold;
pub mod advanced_morphology;
pub mod alpha_composite;
pub mod bilateral;
pub mod bilateral_filter;
pub mod blend_mode;
pub mod canny;
pub mod channel_ops;
pub mod color_adjust;
pub mod color_balance;
pub mod color_lut;
pub mod color_quantize;
pub mod color_science;
pub mod content_aware_resize;
pub mod convolution;
pub mod crop_region;
pub mod dct_block;
pub mod depth_map;
pub mod dither_engine;
pub mod dng;
pub mod dpx;
pub mod dpx_packed;
pub mod edge_detect;
pub mod exif_parser;
pub mod exr;
pub mod exr_multipart;
pub mod film_grain;
pub mod filter;
pub mod filters;
pub mod focus_stack;
pub mod format_detect;
pub mod frequency_domain;
pub mod gradient_magnitude;
pub mod guided_filter;
pub mod hdr_bracket;
pub mod hdr_merge;
pub mod heif;
pub mod histogram_eq;
pub mod histogram_ops;
pub mod icc_embed;
pub mod image_pyramid;
pub mod inpaint;
pub mod inpainting;
pub mod jpeg;
pub mod lens_correct;
/// Shared decode-safety limits (allocation / dimension ceilings) used to reject
/// decompression / allocation bombs built from a few malformed header bytes.
pub(crate) mod limits;
pub mod metadata_xmp;
pub mod mmap_reader;
pub mod morphology;
pub mod morphology_vhgw;
pub mod mosaic;
pub mod multi_layer_exr;
pub mod noise_estimation;
pub mod noise_gen;
pub mod perspective_warp;
pub mod pixel_pipeline;
pub mod png;
pub mod pyramid;
pub mod quantize;
pub mod raw;
pub mod raw_decode;
pub mod rotate_flip;
pub mod segmentation;
pub mod separable_conv;
pub mod sequence;
pub mod simd_convert;
pub mod stitch;
pub mod super_resolution;
pub mod texture_analysis;
pub mod texture_synthesis;
pub mod thumbnail_cache;
pub mod tiff;
pub mod tiled_convolution;
pub mod tone_curve;
pub mod transform;
pub mod webp;

mod error;
mod image;
mod parallel;
mod pattern;
#[cfg(test)]
mod wave13_tests;

pub use error::{ImageError, ImageResult};
pub use image::{ImageData, ImageFrame};
pub use mmap_reader::{BufferedImageReader, ImageSequenceFormat, MmapImageReader};
pub use pattern::SequencePattern;
pub use sequence::ImageSequence;

/// Pixel data type for image frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelType {
    /// Unsigned 8-bit integer per component.
    U8,
    /// Unsigned 10-bit integer per component (stored in u16).
    U10,
    /// Unsigned 12-bit integer per component (stored in u16).
    U12,
    /// Unsigned 16-bit integer per component.
    U16,
    /// Unsigned 32-bit integer per component.
    U32,
    /// 16-bit floating point (half precision).
    F16,
    /// 32-bit floating point (single precision).
    F32,
}

impl PixelType {
    /// Returns the number of bytes needed to store one pixel component.
    #[must_use]
    pub const fn bytes_per_component(&self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U10 | Self::U12 | Self::U16 | Self::F16 => 2,
            Self::U32 | Self::F32 => 4,
        }
    }

    /// Returns true if this is a floating-point type.
    #[must_use]
    pub const fn is_float(&self) -> bool {
        matches!(self, Self::F16 | Self::F32)
    }

    /// Returns the bit depth of this pixel type.
    #[must_use]
    pub const fn bit_depth(&self) -> u8 {
        match self {
            Self::U8 => 8,
            Self::U10 => 10,
            Self::U12 => 12,
            Self::U16 | Self::F16 => 16,
            Self::U32 | Self::F32 => 32,
        }
    }
}

/// Color space for image data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    /// Linear RGB (no gamma correction).
    LinearRgb,
    /// sRGB (standard RGB with gamma 2.2).
    Srgb,
    /// Rec. 709 (HDTV standard).
    Rec709,
    /// Rec. 2020 (UHDTV standard).
    Rec2020,
    /// DCI-P3 (digital cinema).
    DciP3,
    /// Logarithmic (Cineon/DPX).
    Log,
    /// Grayscale/luminance only.
    Luma,
    /// YCbCr (component video).
    YCbCr,
    /// CMYK (print).
    Cmyk,
}

/// Compression method for image data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Compression {
    /// No compression.
    None,
    /// Run-length encoding.
    Rle,
    /// ZIP compression (deflate).
    Zip,
    /// ZIP compression per scanline.
    ZipScanline,
    /// LZW compression.
    Lzw,
    /// `PackBits` compression.
    PackBits,
    /// PIZ wavelet compression (EXR).
    Piz,
    /// PXR24 compression (EXR).
    Pxr24,
    /// B44 compression (EXR).
    B44,
    /// B44A compression (EXR).
    B44a,
    /// DWAA compression (EXR).
    Dwaa,
    /// DWAB compression (EXR).
    Dwab,
}

/// Endianness for multi-byte values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Endian {
    /// Big-endian (most significant byte first).
    Big,
    /// Little-endian (least significant byte first).
    Little,
}

impl Endian {
    /// Returns the native endianness of the current platform.
    #[must_use]
    pub const fn native() -> Self {
        #[cfg(target_endian = "big")]
        return Self::Big;
        #[cfg(target_endian = "little")]
        return Self::Little;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pixel_type_bytes() {
        assert_eq!(PixelType::U8.bytes_per_component(), 1);
        assert_eq!(PixelType::U10.bytes_per_component(), 2);
        assert_eq!(PixelType::U16.bytes_per_component(), 2);
        assert_eq!(PixelType::F32.bytes_per_component(), 4);
    }

    #[test]
    fn test_pixel_type_bit_depth() {
        assert_eq!(PixelType::U8.bit_depth(), 8);
        assert_eq!(PixelType::U10.bit_depth(), 10);
        assert_eq!(PixelType::U12.bit_depth(), 12);
        assert_eq!(PixelType::F16.bit_depth(), 16);
    }

    #[test]
    fn test_pixel_type_is_float() {
        assert!(!PixelType::U8.is_float());
        assert!(!PixelType::U16.is_float());
        assert!(PixelType::F16.is_float());
        assert!(PixelType::F32.is_float());
    }

    #[test]
    fn test_endian_native() {
        let native = Endian::native();
        #[cfg(target_endian = "little")]
        assert_eq!(native, Endian::Little);
        #[cfg(target_endian = "big")]
        assert_eq!(native, Endian::Big);
    }
}
