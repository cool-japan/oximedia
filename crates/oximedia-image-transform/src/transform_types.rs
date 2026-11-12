// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Geometry, rotation, compression, and output-option types used by
//! [`TransformParams`](super::transform::TransformParams).

use std::fmt;

use super::transform::{Color, TransformParseError, DEFAULT_QUALITY};

// ---------------------------------------------------------------------------
// Border
// ---------------------------------------------------------------------------

/// Border specification added around the image.
///
/// ```
/// use oximedia_image_transform::transform::{Border, Color};
///
/// let border = Border {
///     color: Color::black(),
///     top: 5,
///     right: 5,
///     bottom: 5,
///     left: 5,
/// };
/// assert_eq!(border.top, 5);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Border {
    /// Border colour.
    pub color: Color,
    /// Top border width in pixels.
    pub top: u32,
    /// Right border width in pixels.
    pub right: u32,
    /// Bottom border width in pixels.
    pub bottom: u32,
    /// Left border width in pixels.
    pub left: u32,
}

impl Border {
    /// Create a uniform border on all sides.
    pub fn uniform(width: u32, color: Color) -> Self {
        Self {
            color,
            top: width,
            right: width,
            bottom: width,
            left: width,
        }
    }
}

// ---------------------------------------------------------------------------
// Padding
// ---------------------------------------------------------------------------

/// Padding specification (values are fractions of the output dimension, 0.0..1.0).
///
/// ```
/// use oximedia_image_transform::transform::Padding;
///
/// let p = Padding::uniform(0.05);
/// assert!((p.top - 0.05).abs() < 1e-9);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Padding {
    /// Top padding as fraction of output height.
    pub top: f64,
    /// Right padding as fraction of output width.
    pub right: f64,
    /// Bottom padding as fraction of output height.
    pub bottom: f64,
    /// Left padding as fraction of output width.
    pub left: f64,
}

impl Padding {
    /// Create uniform padding on all sides.
    pub fn uniform(value: f64) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

// ---------------------------------------------------------------------------
// Trim
// ---------------------------------------------------------------------------

/// Trim specification (auto-crop edges by the given number of pixels).
///
/// ```
/// use oximedia_image_transform::transform::Trim;
///
/// let t = Trim { top: 10, right: 0, bottom: 10, left: 0 };
/// assert_eq!(t.top, 10);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Trim {
    /// Pixels to trim from the top edge.
    pub top: u32,
    /// Pixels to trim from the right edge.
    pub right: u32,
    /// Pixels to trim from the bottom edge.
    pub bottom: u32,
    /// Pixels to trim from the left edge.
    pub left: u32,
}

impl Trim {
    /// Create uniform trim on all sides.
    pub fn uniform(pixels: u32) -> Self {
        Self {
            top: pixels,
            right: pixels,
            bottom: pixels,
            left: pixels,
        }
    }
}

// ---------------------------------------------------------------------------
// Rotation
// ---------------------------------------------------------------------------

/// Rotation angle.
///
/// ```
/// use oximedia_image_transform::transform::Rotation;
///
/// let r = Rotation::from_degrees(90).expect("valid rotation");
/// assert_eq!(r, Rotation::Deg90);
///
/// assert_eq!(Rotation::Auto.to_degrees(), None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rotation {
    /// No rotation.
    Deg0,
    /// 90 degrees clockwise.
    Deg90,
    /// 180 degrees.
    Deg180,
    /// 270 degrees clockwise (90 degrees counter-clockwise).
    Deg270,
    /// Automatic rotation based on EXIF orientation tag.
    Auto,
}

impl Default for Rotation {
    fn default() -> Self {
        Self::Deg0
    }
}

impl Rotation {
    /// Create a rotation from a degree value.
    pub fn from_degrees(deg: u16) -> Result<Self, TransformParseError> {
        match deg {
            0 => Ok(Self::Deg0),
            90 => Ok(Self::Deg90),
            180 => Ok(Self::Deg180),
            270 => Ok(Self::Deg270),
            _ => Err(TransformParseError::InvalidParameter {
                name: "rotate".to_string(),
                value: deg.to_string(),
            }),
        }
    }

    /// Parse a rotation string.  Accepts `"auto"`, `"0"`, `"90"`, `"180"`, `"270"`.
    pub fn from_str_loose(s: &str) -> Result<Self, TransformParseError> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "0" => Ok(Self::Deg0),
            "90" => Ok(Self::Deg90),
            "180" => Ok(Self::Deg180),
            "270" => Ok(Self::Deg270),
            _ => Err(TransformParseError::InvalidParameter {
                name: "rotate".to_string(),
                value: s.to_string(),
            }),
        }
    }

    /// Returns the rotation in degrees, or `None` for [`Auto`](Self::Auto).
    pub fn to_degrees(&self) -> Option<u16> {
        match self {
            Self::Deg0 => Some(0),
            Self::Deg90 => Some(90),
            Self::Deg180 => Some(180),
            Self::Deg270 => Some(270),
            Self::Auto => None,
        }
    }
}

impl fmt::Display for Rotation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => f.write_str("auto"),
            other => write!(f, "{}", other.to_degrees().unwrap_or(0)),
        }
    }
}

// ---------------------------------------------------------------------------
// Compression
// ---------------------------------------------------------------------------

/// Compression strategy hint.
///
/// ```
/// use oximedia_image_transform::transform::Compression;
/// assert_eq!(Compression::from_str_loose("fast").expect("valid compression"), Compression::Fast);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Compression {
    /// Fastest encoding, larger file size.
    Fast,
    /// Balanced speed and compression.
    Default,
    /// Best compression, slower encoding.
    Best,
}

impl Compression {
    /// Parse a compression string.
    pub fn from_str_loose(s: &str) -> Result<Self, TransformParseError> {
        match s.to_ascii_lowercase().as_str() {
            "fast" => Ok(Self::Fast),
            "default" | "normal" => Ok(Self::Default),
            "best" | "slow" => Ok(Self::Best),
            _ => Err(TransformParseError::InvalidParameter {
                name: "compression".to_string(),
                value: s.to_string(),
            }),
        }
    }

    /// Canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Default => "default",
            Self::Best => "best",
        }
    }
}

impl fmt::Display for Compression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// OutputOptions
// ---------------------------------------------------------------------------

/// Output encoding options that are not part of the core resize/crop pipeline.
///
/// These options control how the encoded output bytes are produced and can be
/// attached to `TransformParams::output_options` to influence the final
/// encoding step.
///
/// # Example
///
/// ```
/// use oximedia_image_transform::transform::{OutputOptions, TransformParams};
///
/// let mut params = TransformParams::default();
/// params.output_options = Some(OutputOptions { progressive_jpeg: true, quality: 80 });
/// assert!(params.is_progressive_output());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct OutputOptions {
    /// When `true`, the JPEG encoder should produce a progressive (multi-scan)
    /// JPEG stream. Progressive JPEGs can appear to load faster in browsers
    /// because they render a low-quality preview before the full image arrives.
    pub progressive_jpeg: bool,
    /// Per-encoding quality override (1-100). When present, this supersedes
    /// `TransformParams::quality` for the encoding step only.
    pub quality: u8,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            progressive_jpeg: false,
            quality: DEFAULT_QUALITY,
        }
    }
}
