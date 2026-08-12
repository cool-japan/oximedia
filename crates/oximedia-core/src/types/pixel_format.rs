//! Pixel format definitions for video frames.
//!
//! This module provides the [`PixelFormat`] enum representing the various
//! ways pixel data can be stored in video frames.

/// Pixel format for video frames.
///
/// Defines how pixel data is organized in memory, including color space,
/// bit depth, and plane layout.
///
/// # Examples
///
/// ```
/// use oximedia_core::types::PixelFormat;
///
/// let format = PixelFormat::Yuv420p;
/// assert!(format.is_planar());
/// assert_eq!(format.plane_count(), 3);
/// assert_eq!(format.bits_per_pixel(), 12);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
#[derive(Default)]
pub enum PixelFormat {
    /// YUV 4:2:0 planar, 8-bit per component.
    /// Most common format for video compression.
    #[default]
    Yuv420p,

    /// YUV 4:2:2 planar, 8-bit per component.
    /// Higher chroma resolution than 4:2:0.
    Yuv422p,

    /// YUV 4:4:4 planar, 8-bit per component.
    /// Full chroma resolution.
    Yuv444p,

    /// YUV 4:2:0 planar, 10-bit little-endian per component.
    /// Used for HDR content.
    Yuv420p10le,

    /// YUV 4:2:0 planar, 12-bit little-endian per component.
    /// Professional and cinema formats.
    Yuv420p12le,

    /// RGB 24-bit packed (8 bits per channel).
    /// Common for display and image files.
    Rgb24,

    /// RGBA 32-bit packed (8 bits per channel with alpha).
    /// RGB with transparency.
    Rgba32,

    /// Grayscale 8-bit.
    /// Single luminance channel.
    Gray8,

    /// Grayscale 16-bit.
    /// High bit-depth luminance.
    Gray16,

    /// NV12 semi-planar: Y plane + interleaved UV plane, 8-bit.
    /// Common hardware decoder output format.
    Nv12,

    /// NV21 semi-planar: Y plane + interleaved VU plane, 8-bit.
    /// Android camera native format (VU order, not UV).
    Nv21,

    /// P010 semi-planar: Y plane + interleaved UV plane, 10-bit little-endian.
    /// Used for 10-bit HDR hardware decode/encode (stored in 16-bit words).
    P010,

    /// P016 semi-planar: Y plane + interleaved UV plane, 16-bit little-endian.
    /// Full 16-bit precision semi-planar format for hardware interop.
    P016,

    /// YUV 4:2:2 planar, 10-bit little-endian per component.
    /// Native input format for ProRes 422 encoding. Each sample occupies a 16-bit
    /// word (little-endian), with the 10 significant bits in the low 10 bits.
    /// Plane layout: Y (w×h), Cb (w/2×h), Cr (w/2×h).
    Yuv422p10le,

    /// YUV 4:2:2 planar, 12-bit little-endian per component.
    /// Each sample occupies a 16-bit word (little-endian), with the 12 significant
    /// bits in the low 12 bits. Plane layout: Y (w×h), Cb (w/2×h), Cr (w/2×h).
    Yuv422p12le,

    /// YUV 4:4:4 planar, 10-bit little-endian per component.
    /// Full chroma resolution, each sample in a 16-bit word (10 significant bits).
    /// Plane layout: Y (w×h), Cb (w×h), Cr (w×h).
    Yuv444p10le,

    /// YUV 4:4:4 planar, 12-bit little-endian per component.
    /// Full chroma resolution, each sample in a 16-bit word (12 significant bits).
    /// Plane layout: Y (w×h), Cb (w×h), Cr (w×h).
    Yuv444p12le,

    /// YUV 4:2:0 planar, 16-bit little-endian per component.
    /// Each sample occupies a 16-bit word (little-endian), full 16-bit precision.
    /// Plane layout: Y (w×h), Cb (w/2×h/2), Cr (w/2×h/2).
    Yuv420p16le,

    /// YUV 4:2:2 planar, 16-bit little-endian per component.
    /// Each sample occupies a 16-bit word (little-endian), full 16-bit precision.
    /// Plane layout: Y (w×h), Cb (w/2×h), Cr (w/2×h).
    Yuv422p16le,

    /// YUV 4:4:4 planar, 16-bit little-endian per component.
    /// Full chroma resolution, each sample in a 16-bit word.
    /// Plane layout: Y (w×h), Cb (w×h), Cr (w×h).
    Yuv444p16le,

    /// YUYV 4:2:2 packed, 8-bit (FourCC YUYV/YUY2): Y0 U Y1 V per 2 pixels.
    /// Single plane; 2 bytes per pixel. The most common UVC webcam format.
    Yuyv422,

    /// UYVY 4:2:2 packed, 8-bit (FourCC UYVY/2vuy): U Y0 V Y1 per 2 pixels.
    /// Single plane; 2 bytes per pixel. AVFoundation's 2vuy, NDI's UYVY.
    Uyvy422,
}

impl PixelFormat {
    /// Returns the number of bits per pixel.
    ///
    /// For planar formats, this is the average bits per pixel
    /// considering all planes.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Yuv420p.bits_per_pixel(), 12);
    /// assert_eq!(PixelFormat::Rgb24.bits_per_pixel(), 24);
    /// assert_eq!(PixelFormat::Rgba32.bits_per_pixel(), 32);
    /// ```
    #[must_use]
    pub const fn bits_per_pixel(&self) -> u32 {
        match self {
            Self::Gray8 => 8,
            Self::Yuv420p | Self::Nv12 | Self::Nv21 => 12,
            Self::Yuv420p10le => 15,
            // Yuv422p10le: 10-bit samples in 16-bit words, 4:2:2 chroma
            // Average bpp = (w*h*2 + w/2*h*2 + w/2*h*2) / (w*h) * 8 / 2 = 20 bpp
            Self::Yuv422p | Self::Gray16 | Self::Yuyv422 | Self::Uyvy422 => 16,
            Self::Yuv422p10le => 20,
            Self::Yuv420p12le => 18,
            // Yuv422p12le: 12-bit samples in 16-bit words, 4:2:2 chroma
            // Average bpp = (w*h + w/2*h + w/2*h) * 2 bytes * 8 bits / (w*h) = 36 / ... = 24 avg but report 36 raw
            Self::Yuv422p12le => 36,
            // Yuv444p10le: 10-bit in 16-bit words, full 4:4:4 => 3 planes * 2 bytes * 8 / 2 = 30
            Self::Yuv444p10le => 30,
            // Yuv444p12le: 12-bit in 16-bit words, full 4:4:4 => 3 planes * 2 bytes = 36
            Self::Yuv444p12le => 36,
            Self::P010 => 24, // 10-bit in 16-bit words: Y(16) + UV(16) at 4:2:0 = 24
            Self::Yuv444p | Self::Rgb24 => 24,
            // 16-bit planar: (Y + Cb/4 + Cr/4) * 16 / pixel = 24 bpp for 4:2:0
            Self::Yuv420p16le => 24,
            // 16-bit planar: (Y + Cb/2 + Cr/2) * 16 / pixel = 32 bpp for 4:2:2
            Self::Yuv422p16le => 32,
            // 16-bit planar: Y + Cb + Cr, each 16-bit = 48 bpp for 4:4:4
            Self::Yuv444p16le => 48,
            Self::Rgba32 => 32,
            Self::P016 => 24, // 16-bit words: Y(16) + UV(16) at 4:2:0 = 24
        }
    }

    /// Returns the number of planes in this format.
    ///
    /// Planar formats have separate planes for each component,
    /// while packed formats store all components interleaved.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Yuv420p.plane_count(), 3);
    /// assert_eq!(PixelFormat::Rgb24.plane_count(), 1);
    /// assert_eq!(PixelFormat::Gray8.plane_count(), 1);
    /// ```
    #[must_use]
    pub const fn plane_count(&self) -> u32 {
        match self {
            Self::Yuv420p
            | Self::Yuv422p
            | Self::Yuv444p
            | Self::Yuv420p10le
            | Self::Yuv420p12le
            | Self::Yuv422p10le
            | Self::Yuv422p12le
            | Self::Yuv444p10le
            | Self::Yuv444p12le
            | Self::Yuv420p16le
            | Self::Yuv422p16le
            | Self::Yuv444p16le => 3,
            Self::Nv12 | Self::Nv21 | Self::P010 | Self::P016 => 2,
            Self::Rgb24
            | Self::Rgba32
            | Self::Gray8
            | Self::Gray16
            | Self::Yuyv422
            | Self::Uyvy422 => 1,
        }
    }

    /// Returns whether this format uses planar storage.
    ///
    /// Planar formats store each color component in a separate
    /// contiguous memory region.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert!(PixelFormat::Yuv420p.is_planar());
    /// assert!(!PixelFormat::Rgb24.is_planar());
    /// ```
    #[must_use]
    pub const fn is_planar(&self) -> bool {
        match self {
            Self::Yuv420p
            | Self::Yuv422p
            | Self::Yuv444p
            | Self::Yuv420p10le
            | Self::Yuv420p12le
            | Self::Yuv422p10le
            | Self::Yuv422p12le
            | Self::Yuv444p10le
            | Self::Yuv444p12le
            | Self::Yuv420p16le
            | Self::Yuv422p16le
            | Self::Yuv444p16le => true,
            Self::Rgb24 | Self::Rgba32 | Self::Gray8 | Self::Gray16 => false,
            // Semi-planar formats are considered non-planar (UV is interleaved)
            Self::Nv12 | Self::Nv21 | Self::P010 | Self::P016 => false,
            // Packed 4:2:2 formats interleave luma and chroma in one plane
            Self::Yuyv422 | Self::Uyvy422 => false,
        }
    }

    /// Returns true if this format uses semi-planar storage.
    ///
    /// Semi-planar formats have a separate Y plane and a single interleaved
    /// chroma plane (UV or VU).
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert!(PixelFormat::Nv12.is_semi_planar());
    /// assert!(PixelFormat::P010.is_semi_planar());
    /// assert!(!PixelFormat::Yuv420p.is_semi_planar());
    /// ```
    #[must_use]
    pub const fn is_semi_planar(&self) -> bool {
        matches!(self, Self::Nv12 | Self::Nv21 | Self::P010 | Self::P016)
    }

    /// Returns the bits per component for this format.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Yuv420p.bits_per_component(), 8);
    /// assert_eq!(PixelFormat::Yuv420p10le.bits_per_component(), 10);
    /// assert_eq!(PixelFormat::Gray16.bits_per_component(), 16);
    /// ```
    #[must_use]
    pub const fn bits_per_component(&self) -> u32 {
        match self {
            Self::Yuv420p
            | Self::Yuv422p
            | Self::Yuv444p
            | Self::Rgb24
            | Self::Rgba32
            | Self::Gray8
            | Self::Nv12
            | Self::Nv21
            | Self::Yuyv422
            | Self::Uyvy422 => 8,
            Self::Yuv420p10le | Self::Yuv422p10le | Self::Yuv444p10le | Self::P010 => 10,
            Self::Yuv420p12le | Self::Yuv422p12le | Self::Yuv444p12le => 12,
            Self::Gray16
            | Self::P016
            | Self::Yuv420p16le
            | Self::Yuv422p16le
            | Self::Yuv444p16le => 16,
        }
    }

    /// Returns true if this is a YUV format.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert!(PixelFormat::Yuv420p.is_yuv());
    /// assert!(!PixelFormat::Rgb24.is_yuv());
    /// ```
    #[must_use]
    pub const fn is_yuv(&self) -> bool {
        matches!(
            self,
            Self::Yuv420p
                | Self::Yuv422p
                | Self::Yuv444p
                | Self::Yuv420p10le
                | Self::Yuv420p12le
                | Self::Yuv422p10le
                | Self::Yuv422p12le
                | Self::Yuv444p10le
                | Self::Yuv444p12le
                | Self::Yuv420p16le
                | Self::Yuv422p16le
                | Self::Yuv444p16le
                | Self::Nv12
                | Self::Nv21
                | Self::P010
                | Self::P016
                | Self::Yuyv422
                | Self::Uyvy422
        )
    }

    /// Returns true if this is an RGB format.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert!(PixelFormat::Rgb24.is_rgb());
    /// assert!(PixelFormat::Rgba32.is_rgb());
    /// assert!(!PixelFormat::Yuv420p.is_rgb());
    /// ```
    #[must_use]
    pub const fn is_rgb(&self) -> bool {
        matches!(self, Self::Rgb24 | Self::Rgba32)
    }

    /// Returns true if this format has an alpha channel.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert!(PixelFormat::Rgba32.has_alpha());
    /// assert!(!PixelFormat::Rgb24.has_alpha());
    /// ```
    #[must_use]
    pub const fn has_alpha(&self) -> bool {
        matches!(self, Self::Rgba32)
    }

    /// Returns the chroma subsampling ratio as (horizontal, vertical).
    ///
    /// For non-YUV formats, returns (1, 1) indicating no subsampling.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Yuv420p.chroma_subsampling(), (2, 2));
    /// assert_eq!(PixelFormat::Yuv422p.chroma_subsampling(), (2, 1));
    /// assert_eq!(PixelFormat::Yuv444p.chroma_subsampling(), (1, 1));
    /// ```
    #[must_use]
    pub const fn chroma_subsampling(&self) -> (u32, u32) {
        match self {
            Self::Yuv420p
            | Self::Yuv420p10le
            | Self::Yuv420p12le
            | Self::Yuv420p16le
            | Self::Nv12
            | Self::Nv21
            | Self::P010
            | Self::P016 => (2, 2),
            Self::Yuv422p
            | Self::Yuv422p10le
            | Self::Yuv422p12le
            | Self::Yuv422p16le
            | Self::Yuyv422
            | Self::Uyvy422 => (2, 1),
            Self::Yuv444p
            | Self::Yuv444p10le
            | Self::Yuv444p12le
            | Self::Yuv444p16le
            | Self::Rgb24
            | Self::Rgba32
            | Self::Gray8
            | Self::Gray16 => (1, 1),
        }
    }
}

impl PixelFormat {
    /// Computes the minimum buffer size in bytes for a frame of the given dimensions.
    ///
    /// Accounts for chroma subsampling and bit depth. Assumes no padding between rows.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// // 1920x1080 YUV420p: Y=1920*1080 + U=960*540 + V=960*540 = 3110400
    /// assert_eq!(PixelFormat::Yuv420p.frame_buffer_size(1920, 1080), 3_110_400);
    /// ```
    #[must_use]
    pub const fn frame_buffer_size(&self, width: u32, height: u32) -> usize {
        let w = width as usize;
        let h = height as usize;
        match self {
            Self::Gray8 => w * h,
            Self::Gray16 => w * h * 2,
            Self::Rgb24 => w * h * 3,
            Self::Rgba32 => w * h * 4,
            // Packed YUV 4:2:2 (single interleaved plane): 2 bytes per pixel
            Self::Yuyv422 | Self::Uyvy422 => w * h * 2,
            // Planar YUV 8-bit
            Self::Yuv420p => w * h + 2 * (w / 2) * (h / 2), // Y + U + V
            Self::Yuv422p => w * h + 2 * (w / 2) * h,       // Y + U + V
            Self::Yuv444p => w * h * 3,                     // Y + U + V
            // Planar YUV high bit-depth (stored in 16-bit words)
            Self::Yuv420p10le | Self::Yuv420p12le => (w * h + 2 * (w / 2) * (h / 2)) * 2,
            // Planar YUV 4:2:2 10/12-bit (16-bit words): Y(w*h) + Cb(w/2*h) + Cr(w/2*h), each 2 bytes
            Self::Yuv422p10le | Self::Yuv422p12le => (w * h + 2 * (w / 2) * h) * 2,
            // Planar YUV 4:4:4 10/12-bit (16-bit words): Y(w*h) + Cb(w*h) + Cr(w*h), each 2 bytes
            Self::Yuv444p10le | Self::Yuv444p12le => w * h * 3 * 2,
            // Planar YUV 16-bit: same layout as 10/12-bit (2 bytes per sample)
            Self::Yuv420p16le => (w * h + 2 * (w / 2) * (h / 2)) * 2,
            Self::Yuv422p16le => (w * h + 2 * (w / 2) * h) * 2,
            Self::Yuv444p16le => w * h * 3 * 2,
            // Semi-planar 8-bit: Y plane + interleaved UV plane
            Self::Nv12 | Self::Nv21 => w * h + (w / 2) * 2 * (h / 2),
            // Semi-planar high bit-depth (16-bit words)
            Self::P010 | Self::P016 => (w * h + (w / 2) * 2 * (h / 2)) * 2,
        }
    }

    /// Returns the row stride in bytes for a given plane at the given width.
    ///
    /// # Arguments
    ///
    /// * `width` - The width of the frame in pixels
    /// * `plane` - The plane index (0 = Y/RGB, 1 = U/UV, 2 = V)
    ///
    /// Returns `None` if the plane index is out of range.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// assert_eq!(PixelFormat::Yuv420p.stride_for_width(1920, 0), Some(1920));
    /// assert_eq!(PixelFormat::Yuv420p.stride_for_width(1920, 1), Some(960));
    /// assert_eq!(PixelFormat::Yuv420p.stride_for_width(1920, 3), None);
    /// ```
    #[must_use]
    pub const fn stride_for_width(&self, width: u32, plane: u32) -> Option<usize> {
        let w = width as usize;
        match self {
            Self::Gray8 => {
                if plane == 0 {
                    Some(w)
                } else {
                    None
                }
            }
            Self::Gray16 => {
                if plane == 0 {
                    Some(w * 2)
                } else {
                    None
                }
            }
            Self::Rgb24 => {
                if plane == 0 {
                    Some(w * 3)
                } else {
                    None
                }
            }
            Self::Rgba32 => {
                if plane == 0 {
                    Some(w * 4)
                } else {
                    None
                }
            }
            // Packed 4:2:2: single interleaved plane, 2 bytes per pixel
            Self::Yuyv422 | Self::Uyvy422 => {
                if plane == 0 {
                    Some(w * 2)
                } else {
                    None
                }
            }
            Self::Yuv420p => match plane {
                0 => Some(w),
                1 | 2 => Some(w / 2),
                _ => None,
            },
            Self::Yuv422p => match plane {
                0 => Some(w),
                1 | 2 => Some(w / 2),
                _ => None,
            },
            Self::Yuv444p => match plane {
                0..=2 => Some(w),
                _ => None,
            },
            Self::Yuv420p10le | Self::Yuv420p12le => match plane {
                0 => Some(w * 2),
                1 | 2 => Some((w / 2) * 2),
                _ => None,
            },
            Self::Yuv422p10le | Self::Yuv422p12le => match plane {
                0 => Some(w * 2),
                1 | 2 => Some((w / 2) * 2),
                _ => None,
            },
            Self::Yuv444p10le | Self::Yuv444p12le => match plane {
                0..=2 => Some(w * 2),
                _ => None,
            },
            Self::Yuv420p16le => match plane {
                0 => Some(w * 2),
                1 | 2 => Some((w / 2) * 2),
                _ => None,
            },
            Self::Yuv422p16le => match plane {
                0 => Some(w * 2),
                1 | 2 => Some((w / 2) * 2),
                _ => None,
            },
            Self::Yuv444p16le => match plane {
                0..=2 => Some(w * 2),
                _ => None,
            },
            Self::Nv12 | Self::Nv21 => match plane {
                0 => Some(w),
                1 => Some(w), // interleaved UV: 2 components * (w/2) = w bytes
                _ => None,
            },
            Self::P010 | Self::P016 => match plane {
                0 => Some(w * 2),
                1 => Some(w * 2), // interleaved UV in 16-bit words: 2 * (w/2) * 2 = 2*w
                _ => None,
            },
        }
    }
}

impl std::str::FromStr for PixelFormat {
    type Err = crate::OxiError;

    /// Parses a pixel format name string.
    ///
    /// # Examples
    ///
    /// ```
    /// use oximedia_core::types::PixelFormat;
    ///
    /// let fmt: PixelFormat = "yuv420p".parse().expect("should parse");
    /// assert_eq!(fmt, PixelFormat::Yuv420p);
    /// ```
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "yuv420p" => Ok(Self::Yuv420p),
            "yuv422p" => Ok(Self::Yuv422p),
            "yuv444p" => Ok(Self::Yuv444p),
            "yuv420p10le" | "yuv420p10" => Ok(Self::Yuv420p10le),
            "yuv420p12le" | "yuv420p12" => Ok(Self::Yuv420p12le),
            "yuv422p10le" | "yuv422p10" => Ok(Self::Yuv422p10le),
            "yuv422p12le" | "yuv422p12" => Ok(Self::Yuv422p12le),
            "yuv444p10le" | "yuv444p10" => Ok(Self::Yuv444p10le),
            "yuv444p12le" | "yuv444p12" => Ok(Self::Yuv444p12le),
            "yuv420p16le" | "yuv420p16" => Ok(Self::Yuv420p16le),
            "yuv422p16le" | "yuv422p16" => Ok(Self::Yuv422p16le),
            "yuv444p16le" | "yuv444p16" => Ok(Self::Yuv444p16le),
            "yuyv422" | "yuyv" | "yuy2" => Ok(Self::Yuyv422),
            "uyvy422" | "uyvy" | "2vuy" => Ok(Self::Uyvy422),
            "rgb24" | "rgb" => Ok(Self::Rgb24),
            "rgba32" | "rgba" => Ok(Self::Rgba32),
            "gray8" | "gray" | "grey8" | "grey" => Ok(Self::Gray8),
            "gray16" | "grey16" => Ok(Self::Gray16),
            "nv12" => Ok(Self::Nv12),
            "nv21" => Ok(Self::Nv21),
            "p010" => Ok(Self::P010),
            "p016" => Ok(Self::P016),
            _ => Err(crate::OxiError::Unsupported(format!(
                "Unknown pixel format: {s}"
            ))),
        }
    }
}

impl std::fmt::Display for PixelFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Yuv420p => "yuv420p",
            Self::Yuv422p => "yuv422p",
            Self::Yuv444p => "yuv444p",
            Self::Yuv420p10le => "yuv420p10le",
            Self::Yuv420p12le => "yuv420p12le",
            Self::Yuv422p10le => "yuv422p10le",
            Self::Yuv422p12le => "yuv422p12le",
            Self::Yuv444p10le => "yuv444p10le",
            Self::Yuv444p12le => "yuv444p12le",
            Self::Yuv420p16le => "yuv420p16le",
            Self::Yuv422p16le => "yuv422p16le",
            Self::Yuv444p16le => "yuv444p16le",
            Self::Yuyv422 => "yuyv422",
            Self::Uyvy422 => "uyvy422",
            Self::Rgb24 => "rgb24",
            Self::Rgba32 => "rgba32",
            Self::Gray8 => "gray8",
            Self::Gray16 => "gray16",
            Self::Nv12 => "nv12",
            Self::Nv21 => "nv21",
            Self::P010 => "p010",
            Self::P016 => "p016",
        };
        write!(f, "{name}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bits_per_pixel() {
        assert_eq!(PixelFormat::Yuv420p.bits_per_pixel(), 12);
        assert_eq!(PixelFormat::Yuv422p.bits_per_pixel(), 16);
        assert_eq!(PixelFormat::Yuv444p.bits_per_pixel(), 24);
        assert_eq!(PixelFormat::Rgb24.bits_per_pixel(), 24);
        assert_eq!(PixelFormat::Rgba32.bits_per_pixel(), 32);
        assert_eq!(PixelFormat::Gray8.bits_per_pixel(), 8);
        assert_eq!(PixelFormat::Gray16.bits_per_pixel(), 16);
    }

    #[test]
    fn test_plane_count() {
        assert_eq!(PixelFormat::Yuv420p.plane_count(), 3);
        assert_eq!(PixelFormat::Yuv422p.plane_count(), 3);
        assert_eq!(PixelFormat::Rgb24.plane_count(), 1);
        assert_eq!(PixelFormat::Gray8.plane_count(), 1);
    }

    #[test]
    fn test_is_planar() {
        assert!(PixelFormat::Yuv420p.is_planar());
        assert!(PixelFormat::Yuv422p.is_planar());
        assert!(!PixelFormat::Rgb24.is_planar());
        assert!(!PixelFormat::Rgba32.is_planar());
    }

    #[test]
    fn test_is_yuv() {
        assert!(PixelFormat::Yuv420p.is_yuv());
        assert!(PixelFormat::Yuv420p10le.is_yuv());
        assert!(!PixelFormat::Rgb24.is_yuv());
    }

    #[test]
    fn test_chroma_subsampling() {
        assert_eq!(PixelFormat::Yuv420p.chroma_subsampling(), (2, 2));
        assert_eq!(PixelFormat::Yuv422p.chroma_subsampling(), (2, 1));
        assert_eq!(PixelFormat::Yuv444p.chroma_subsampling(), (1, 1));
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", PixelFormat::Yuv420p), "yuv420p");
        assert_eq!(format!("{}", PixelFormat::Rgb24), "rgb24");
    }

    // ── NV12/NV21/P010/P016 tests ────────────────────────────────────

    #[test]
    fn test_nv12_properties() {
        let fmt = PixelFormat::Nv12;
        assert_eq!(fmt.bits_per_pixel(), 12);
        assert_eq!(fmt.plane_count(), 2);
        assert!(!fmt.is_planar());
        assert!(fmt.is_semi_planar());
        assert!(fmt.is_yuv());
        assert!(!fmt.is_rgb());
        assert!(!fmt.has_alpha());
        assert_eq!(fmt.bits_per_component(), 8);
        assert_eq!(fmt.chroma_subsampling(), (2, 2));
        assert_eq!(format!("{fmt}"), "nv12");
    }

    #[test]
    fn test_nv21_properties() {
        let fmt = PixelFormat::Nv21;
        assert_eq!(fmt.bits_per_pixel(), 12);
        assert_eq!(fmt.plane_count(), 2);
        assert!(!fmt.is_planar());
        assert!(fmt.is_semi_planar());
        assert!(fmt.is_yuv());
        assert_eq!(fmt.bits_per_component(), 8);
        assert_eq!(fmt.chroma_subsampling(), (2, 2));
        assert_eq!(format!("{fmt}"), "nv21");
    }

    #[test]
    fn test_p010_properties() {
        let fmt = PixelFormat::P010;
        assert_eq!(fmt.bits_per_pixel(), 24);
        assert_eq!(fmt.plane_count(), 2);
        assert!(!fmt.is_planar());
        assert!(fmt.is_semi_planar());
        assert!(fmt.is_yuv());
        assert_eq!(fmt.bits_per_component(), 10);
        assert_eq!(fmt.chroma_subsampling(), (2, 2));
        assert_eq!(format!("{fmt}"), "p010");
    }

    #[test]
    fn test_p016_properties() {
        let fmt = PixelFormat::P016;
        assert_eq!(fmt.bits_per_pixel(), 24);
        assert_eq!(fmt.plane_count(), 2);
        assert!(!fmt.is_planar());
        assert!(fmt.is_semi_planar());
        assert!(fmt.is_yuv());
        assert_eq!(fmt.bits_per_component(), 16);
        assert_eq!(fmt.chroma_subsampling(), (2, 2));
        assert_eq!(format!("{fmt}"), "p016");
    }

    #[test]
    fn test_semi_planar_false_for_planar() {
        assert!(!PixelFormat::Yuv420p.is_semi_planar());
        assert!(!PixelFormat::Yuv422p.is_semi_planar());
        assert!(!PixelFormat::Rgb24.is_semi_planar());
        assert!(!PixelFormat::Gray8.is_semi_planar());
    }

    // ── frame_buffer_size tests ─────────────────────────────────────

    #[test]
    fn test_frame_buffer_size_yuv420p_1080p() {
        // Y=1920*1080 + U=960*540 + V=960*540 = 2073600 + 518400 + 518400 = 3110400
        assert_eq!(
            PixelFormat::Yuv420p.frame_buffer_size(1920, 1080),
            3_110_400
        );
    }

    #[test]
    fn test_frame_buffer_size_yuv422p() {
        // Y=1920*1080 + U=960*1080 + V=960*1080 = 2073600 + 1036800 + 1036800
        assert_eq!(
            PixelFormat::Yuv422p.frame_buffer_size(1920, 1080),
            4_147_200
        );
    }

    #[test]
    fn test_frame_buffer_size_yuv444p() {
        assert_eq!(
            PixelFormat::Yuv444p.frame_buffer_size(1920, 1080),
            1920 * 1080 * 3
        );
    }

    #[test]
    fn test_frame_buffer_size_rgb24() {
        assert_eq!(
            PixelFormat::Rgb24.frame_buffer_size(1920, 1080),
            1920 * 1080 * 3
        );
    }

    #[test]
    fn test_frame_buffer_size_rgba32() {
        assert_eq!(
            PixelFormat::Rgba32.frame_buffer_size(1920, 1080),
            1920 * 1080 * 4
        );
    }

    #[test]
    fn test_frame_buffer_size_gray8() {
        assert_eq!(PixelFormat::Gray8.frame_buffer_size(640, 480), 640 * 480);
    }

    #[test]
    fn test_frame_buffer_size_gray16() {
        assert_eq!(
            PixelFormat::Gray16.frame_buffer_size(640, 480),
            640 * 480 * 2
        );
    }

    #[test]
    fn test_frame_buffer_size_nv12() {
        // Y=1920*1080 + UV=1920*540 = 2073600 + 1036800 = 3110400
        let size = PixelFormat::Nv12.frame_buffer_size(1920, 1080);
        assert_eq!(size, 3_110_400);
    }

    #[test]
    fn test_frame_buffer_size_nv21() {
        assert_eq!(
            PixelFormat::Nv21.frame_buffer_size(1920, 1080),
            PixelFormat::Nv12.frame_buffer_size(1920, 1080),
        );
    }

    #[test]
    fn test_frame_buffer_size_p010() {
        // Same as NV12 but 2 bytes per sample
        let nv12 = PixelFormat::Nv12.frame_buffer_size(1920, 1080);
        assert_eq!(PixelFormat::P010.frame_buffer_size(1920, 1080), nv12 * 2);
    }

    #[test]
    fn test_frame_buffer_size_p016() {
        let nv12 = PixelFormat::Nv12.frame_buffer_size(1920, 1080);
        assert_eq!(PixelFormat::P016.frame_buffer_size(1920, 1080), nv12 * 2);
    }

    #[test]
    fn test_frame_buffer_size_yuv420p10le() {
        let yuv420 = PixelFormat::Yuv420p.frame_buffer_size(1920, 1080);
        assert_eq!(
            PixelFormat::Yuv420p10le.frame_buffer_size(1920, 1080),
            yuv420 * 2
        );
    }

    // ── stride_for_width tests ──────────────────────────────────────

    #[test]
    fn test_stride_yuv420p() {
        assert_eq!(PixelFormat::Yuv420p.stride_for_width(1920, 0), Some(1920));
        assert_eq!(PixelFormat::Yuv420p.stride_for_width(1920, 1), Some(960));
        assert_eq!(PixelFormat::Yuv420p.stride_for_width(1920, 2), Some(960));
        assert_eq!(PixelFormat::Yuv420p.stride_for_width(1920, 3), None);
    }

    #[test]
    fn test_stride_nv12() {
        assert_eq!(PixelFormat::Nv12.stride_for_width(1920, 0), Some(1920));
        assert_eq!(PixelFormat::Nv12.stride_for_width(1920, 1), Some(1920));
        assert_eq!(PixelFormat::Nv12.stride_for_width(1920, 2), None);
    }

    #[test]
    fn test_stride_p010() {
        assert_eq!(PixelFormat::P010.stride_for_width(1920, 0), Some(3840));
        assert_eq!(PixelFormat::P010.stride_for_width(1920, 1), Some(3840));
        assert_eq!(PixelFormat::P010.stride_for_width(1920, 2), None);
    }

    #[test]
    fn test_stride_rgb24() {
        assert_eq!(PixelFormat::Rgb24.stride_for_width(1920, 0), Some(5760));
        assert_eq!(PixelFormat::Rgb24.stride_for_width(1920, 1), None);
    }

    #[test]
    fn test_stride_rgba32() {
        assert_eq!(PixelFormat::Rgba32.stride_for_width(1920, 0), Some(7680));
        assert_eq!(PixelFormat::Rgba32.stride_for_width(1920, 1), None);
    }

    #[test]
    fn test_stride_yuv444p() {
        assert_eq!(PixelFormat::Yuv444p.stride_for_width(1920, 0), Some(1920));
        assert_eq!(PixelFormat::Yuv444p.stride_for_width(1920, 1), Some(1920));
        assert_eq!(PixelFormat::Yuv444p.stride_for_width(1920, 2), Some(1920));
        assert_eq!(PixelFormat::Yuv444p.stride_for_width(1920, 3), None);
    }

    // ── FromStr tests ───────────────────────────────────────────────

    #[test]
    fn test_from_str_all_formats() {
        assert_eq!(
            "yuv420p".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Yuv420p
        );
        assert_eq!(
            "yuv422p".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Yuv422p
        );
        assert_eq!(
            "yuv444p".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Yuv444p
        );
        assert_eq!(
            "yuv420p10le".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Yuv420p10le
        );
        assert_eq!(
            "yuv420p12le".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Yuv420p12le
        );
        assert_eq!(
            "rgb24".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Rgb24
        );
        assert_eq!(
            "rgba32".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Rgba32
        );
        assert_eq!(
            "gray8".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Gray8
        );
        assert_eq!(
            "gray16".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Gray16
        );
        assert_eq!(
            "nv12".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Nv12
        );
        assert_eq!(
            "nv21".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Nv21
        );
        assert_eq!(
            "p010".parse::<PixelFormat>().expect("parse"),
            PixelFormat::P010
        );
        assert_eq!(
            "p016".parse::<PixelFormat>().expect("parse"),
            PixelFormat::P016
        );
    }

    #[test]
    fn test_from_str_case_insensitive() {
        assert_eq!(
            "YUV420P".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Yuv420p
        );
        assert_eq!(
            "NV12".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Nv12
        );
        assert_eq!(
            "P010".parse::<PixelFormat>().expect("parse"),
            PixelFormat::P010
        );
    }

    #[test]
    fn test_from_str_aliases() {
        assert_eq!(
            "rgb".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Rgb24
        );
        assert_eq!(
            "rgba".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Rgba32
        );
        assert_eq!(
            "gray".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Gray8
        );
        assert_eq!(
            "grey".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Gray8
        );
        assert_eq!(
            "grey8".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Gray8
        );
        assert_eq!(
            "grey16".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Gray16
        );
        assert_eq!(
            "yuv420p10".parse::<PixelFormat>().expect("parse"),
            PixelFormat::Yuv420p10le
        );
    }

    #[test]
    fn test_from_str_unknown() {
        assert!("h264".parse::<PixelFormat>().is_err());
        assert!("unknown".parse::<PixelFormat>().is_err());
    }

    #[test]
    fn test_from_str_roundtrip() {
        let formats = [
            PixelFormat::Yuv420p,
            PixelFormat::Nv12,
            PixelFormat::P010,
            PixelFormat::Rgb24,
            PixelFormat::Gray8,
        ];
        for fmt in &formats {
            let s = format!("{fmt}");
            let parsed: PixelFormat = s.parse().expect("roundtrip should work");
            assert_eq!(*fmt, parsed);
        }
    }

    // ── Yuyv422 / Uyvy422 packed 4:2:2 tests ─────────────────────────

    #[test]
    fn test_yuyv422_properties() {
        let fmt = PixelFormat::Yuyv422;
        assert_eq!(fmt.bits_per_pixel(), 16);
        assert_eq!(fmt.plane_count(), 1);
        assert_eq!(fmt.frame_buffer_size(64, 48), 6144);
        assert_eq!(fmt.stride_for_width(64, 0), Some(128));
        assert_eq!(fmt.chroma_subsampling(), (2, 1));
        assert!(fmt.is_yuv());
        assert!(!fmt.is_rgb());
        assert!(!fmt.has_alpha());
        assert!(!fmt.is_planar());
        assert!(!fmt.is_semi_planar());
        assert_eq!(fmt.bits_per_component(), 8);
        assert_eq!(format!("{fmt}"), "yuyv422");
    }

    #[test]
    fn test_uyvy422_properties() {
        let fmt = PixelFormat::Uyvy422;
        assert_eq!(fmt.bits_per_pixel(), 16);
        assert_eq!(fmt.plane_count(), 1);
        assert_eq!(fmt.frame_buffer_size(64, 48), 6144);
        assert_eq!(fmt.stride_for_width(64, 0), Some(128));
        assert_eq!(fmt.chroma_subsampling(), (2, 1));
        assert!(fmt.is_yuv());
        assert!(!fmt.is_rgb());
        assert!(!fmt.has_alpha());
        assert!(!fmt.is_planar());
        assert!(!fmt.is_semi_planar());
        assert_eq!(fmt.bits_per_component(), 8);
        assert_eq!(format!("{fmt}"), "uyvy422");
    }

    #[test]
    fn test_yuyv422_uyvy422_stride_out_of_range() {
        assert_eq!(PixelFormat::Yuyv422.stride_for_width(64, 1), None);
        assert_eq!(PixelFormat::Uyvy422.stride_for_width(64, 1), None);
    }

    #[test]
    fn test_yuyv422_uyvy422_from_str_roundtrip() {
        let cases = [
            ("yuyv422", PixelFormat::Yuyv422),
            ("yuyv", PixelFormat::Yuyv422),
            ("yuy2", PixelFormat::Yuyv422),
            ("uyvy422", PixelFormat::Uyvy422),
            ("uyvy", PixelFormat::Uyvy422),
            ("2vuy", PixelFormat::Uyvy422),
        ];
        for (s, fmt) in cases {
            assert_eq!(s.parse::<PixelFormat>().expect("parse"), fmt);
        }
        assert_eq!(format!("{}", PixelFormat::Yuyv422), "yuyv422");
        assert_eq!(format!("{}", PixelFormat::Uyvy422), "uyvy422");
    }
}
