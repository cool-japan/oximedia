// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Cloudflare Images-compatible transformation options.
//!
//! This module defines the complete set of image transformation parameters
//! that mirror Cloudflare Images' URL-based API.  Every parameter supported by
//! Cloudflare's `/cdn-cgi/image/` endpoint is represented here, including
//! resize, crop, quality, format, effects (blur, sharpen, brightness, contrast,
//! gamma), rotation, trim, DPR, metadata, animation, background, border,
//! padding, compression strategy, and error handling.
//!
//! # [`TransformParams`] fields, defaults, and valid ranges
//!
//! Every field below defaults to the value shown when omitted from a URL or
//! query string (see [`TransformParams::default`] and the `parser` module's
//! `parse_param` for the corresponding URL key names / short aliases).
//!
//! | Field | Type | Default | Valid range / values |
//! |-------|------|---------|-----------------------|
//! | [`width`](TransformParams::width) | `Option<u32>` | `None` | `1..=`[`MAX_DIMENSION`] (12000) |
//! | [`height`](TransformParams::height) | `Option<u32>` | `None` | `1..=`[`MAX_DIMENSION`] (12000) |
//! | [`quality`](TransformParams::quality) | `u8` | [`DEFAULT_QUALITY`] (85) | `1..=100` |
//! | [`format`](TransformParams::format) | [`OutputFormat`] | [`OutputFormat::Auto`] | `auto`, `avif`, `webp`, `jpeg`, `png`, `gif`, `baseline`, `json` |
//! | [`fit`](TransformParams::fit) | [`FitMode`] | [`FitMode::ScaleDown`] | `scale-down`, `contain`, `cover`, `crop`, `pad`, `fill` |
//! | [`gravity`](TransformParams::gravity) | [`Gravity`] | [`Gravity::Center`] | `auto`, `center`, `top`, `bottom`, `left`, `right`, `face`, `"<x>x<y>"` focal point |
//! | [`sharpen`](TransformParams::sharpen) | `f64` | `0.0` (off) | `0.0..=`[`MAX_SHARPEN`] (10.0) |
//! | [`blur`](TransformParams::blur) | `f64` | `0.0` (off) | `0.0..=`[`MAX_BLUR_RADIUS`] (250.0) |
//! | [`brightness`](TransformParams::brightness) | `f64` | `0.0` (no change) | `-1.0..=1.0` |
//! | [`contrast`](TransformParams::contrast) | `f64` | `0.0` (no change) | `-1.0..=1.0` |
//! | [`gamma`](TransformParams::gamma) | `f64` | `1.0` (no change) | `0.0..=`[`MAX_GAMMA`] (10.0), exclusive of `0.0` |
//! | [`rotate`](TransformParams::rotate) | [`Rotation`] | [`Rotation::Deg0`] | `0`, `90`, `180`, `270`, `auto` (EXIF-driven) |
//! | [`trim`](TransformParams::trim) | `Option<Trim>` | `None` | 1 or 4 non-negative pixel counts (`top,right,bottom,left`) |
//! | [`dpr`](TransformParams::dpr) | `f64` | `1.0` ([`MIN_DPR`]) | [`MIN_DPR`]`..=`[`MAX_DPR`] (1.0..=4.0) |
//! | [`metadata`](TransformParams::metadata) | [`MetadataMode`] | [`MetadataMode::None`] | `keep`, `copyright`, `none` |
//! | [`anim`](TransformParams::anim) | `bool` | `true` (preserve frames) | `true`, `false` |
//! | [`background`](TransformParams::background) | [`Color`] | transparent | CSS hex (`#rgb`, `#rrggbb`, `#rrggbbaa`) or named colour |
//! | [`border`](TransformParams::border) | `Option<Border>` | `None` | `width:color` or `t,r,b,l:color` |
//! | [`pad`](TransformParams::pad) | `Option<Padding>` | `None` | 1, 2 (`tb,lr`), or 4 fractional values in `0.0..=1.0` |
//! | [`compression`](TransformParams::compression) | `Option<String>` | `None` | `fast`, `default`/`normal`, `best`/`slow` |
//! | [`onerror`](TransformParams::onerror) | `Option<String>` | `None` | e.g. `"redirect"` |
//! | [`output_options`](TransformParams::output_options) | `Option<`[`OutputOptions`]`>` | `None` | see [`OutputOptions`] (progressive JPEG, per-encode quality override) |
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::transform::{TransformParams, FitMode, OutputFormat, Rotation};
//!
//! let params = TransformParams {
//!     width: Some(800),
//!     height: Some(600),
//!     quality: 85,
//!     format: OutputFormat::Auto,
//!     fit: FitMode::Cover,
//!     rotate: Rotation::Auto,
//!     ..TransformParams::default()
//! };
//!
//! assert_eq!(params.effective_width(), Some(800));
//! assert!(params.validate().is_ok());
//! ```

use std::fmt;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum allowed dimension (width or height) in pixels.
pub const MAX_DIMENSION: u32 = 12000;

/// Default JPEG/WebP quality (1-100).
pub const DEFAULT_QUALITY: u8 = 85;

/// Maximum blur radius in pixels.
pub const MAX_BLUR_RADIUS: f64 = 250.0;

/// Maximum sharpen amount.
pub const MAX_SHARPEN: f64 = 10.0;

/// Maximum gamma value.
pub const MAX_GAMMA: f64 = 10.0;

/// Maximum device pixel ratio.
pub const MAX_DPR: f64 = 4.0;

/// Minimum device pixel ratio.
pub const MIN_DPR: f64 = 1.0;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur when parsing or validating image transformation parameters.
#[derive(Debug, Error)]
pub enum TransformParseError {
    /// A parameter has an invalid value.
    #[error("invalid parameter: {name}={value}")]
    InvalidParameter {
        /// Parameter name.
        name: String,
        /// Invalid value that was provided.
        value: String,
    },

    /// A dimension exceeds the allowed maximum.
    #[error("invalid dimension: {0} exceeds maximum {1}")]
    DimensionTooLarge(u32, u32),

    /// Quality value is out of the 1-100 range.
    #[error("invalid quality: {0}, must be 1-100")]
    InvalidQuality(u8),

    /// A color string could not be parsed.
    #[error("invalid color: {0}")]
    InvalidColor(String),

    /// An unrecognised parameter name was encountered.
    #[error("unknown parameter: {0}")]
    UnknownParameter(String),

    /// An invalid output format string was provided.
    #[error("invalid format: {0}")]
    InvalidFormat(String),

    /// An invalid gravity/anchor string was provided.
    #[error("invalid gravity: {0}")]
    InvalidGravity(String),

    /// DPR value is outside the 1.0-4.0 range.
    #[error("invalid DPR: {0}, must be 1.0-4.0")]
    InvalidDpr(f64),

    /// The source image path is missing from the URL.
    #[error("missing required source path")]
    MissingSourcePath,

    /// Generic parse error with a descriptive message.
    #[error("parse error: {0}")]
    ParseError(String),

    /// A path-traversal or other security violation was detected.
    #[error("security violation: {0}")]
    SecurityViolation(String),
}

// ---------------------------------------------------------------------------
// FitMode
// ---------------------------------------------------------------------------

/// Fit mode for image resizing (Cloudflare Images compatible).
///
/// Controls how the source image is fitted into the requested width/height.
///
/// ```
/// use oximedia_image_transform::transform::FitMode;
///
/// let fit = FitMode::default();
/// assert_eq!(fit, FitMode::ScaleDown);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FitMode {
    /// Resize to fit within dimensions, preserving aspect ratio.
    /// Never enlarges — only shrinks. This is the Cloudflare default.
    ScaleDown,
    /// Resize to fit within dimensions, preserving aspect ratio.
    /// May letterbox if aspect ratios differ.
    Contain,
    /// Resize and crop to fill dimensions exactly, preserving aspect ratio.
    Cover,
    /// Same as [`Cover`](Self::Cover) but respects [`Gravity`] for crop position.
    Crop,
    /// Fit within dimensions and pad with [`background`](TransformParams::background) color.
    Pad,
    /// Stretch to exact dimensions, ignoring aspect ratio.
    Fill,
}

impl Default for FitMode {
    fn default() -> Self {
        Self::ScaleDown
    }
}

impl FitMode {
    /// Parse a fit mode string (case-insensitive).
    ///
    /// Accepts `"scale-down"`, `"scale_down"`, `"scaledown"`, `"contain"`,
    /// `"cover"`, `"crop"`, `"pad"`, and `"fill"`.
    ///
    /// ```
    /// use oximedia_image_transform::transform::FitMode;
    ///
    /// let fit = FitMode::from_str_loose("scale-down").expect("valid");
    /// assert_eq!(fit, FitMode::ScaleDown);
    ///
    /// let fit = FitMode::from_str_loose("Cover").expect("case insensitive");
    /// assert_eq!(fit, FitMode::Cover);
    /// ```
    pub fn from_str_loose(s: &str) -> Result<Self, TransformParseError> {
        match s.to_ascii_lowercase().as_str() {
            "scale-down" | "scale_down" | "scaledown" => Ok(Self::ScaleDown),
            "contain" => Ok(Self::Contain),
            "cover" => Ok(Self::Cover),
            "crop" => Ok(Self::Crop),
            "pad" => Ok(Self::Pad),
            "fill" => Ok(Self::Fill),
            _ => Err(TransformParseError::InvalidParameter {
                name: "fit".to_string(),
                value: s.to_string(),
            }),
        }
    }

    /// Canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ScaleDown => "scale-down",
            Self::Contain => "contain",
            Self::Cover => "cover",
            Self::Crop => "crop",
            Self::Pad => "pad",
            Self::Fill => "fill",
        }
    }
}

impl fmt::Display for FitMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Gravity
// ---------------------------------------------------------------------------

/// Gravity/anchor point for cropping.
///
/// When a [`FitMode::Cover`] or [`FitMode::Crop`] resize causes the image to be
/// cropped, the gravity determines which region of the source is preserved.
///
/// ```
/// use oximedia_image_transform::transform::Gravity;
///
/// let g = Gravity::from_str_loose("0.3x0.7").expect("focal point");
/// assert!(matches!(g, Gravity::FocalPoint(x, y) if (x - 0.3).abs() < 0.001));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Gravity {
    /// Content-aware automatic cropping (saliency-based).
    Auto,
    /// Center of the image (default).
    Center,
    /// Top edge center.
    Top,
    /// Bottom edge center.
    Bottom,
    /// Left edge center.
    Left,
    /// Right edge center.
    Right,
    /// Top-left corner.
    TopLeft,
    /// Top-right corner.
    TopRight,
    /// Bottom-left corner.
    BottomLeft,
    /// Bottom-right corner.
    BottomRight,
    /// Face-detection-based cropping.
    Face,
    /// Focal point as (x, y) normalised to 0.0..1.0.
    FocalPoint(f64, f64),
}

impl Default for Gravity {
    fn default() -> Self {
        Self::Center
    }
}

impl Gravity {
    /// Parse a gravity string.  Supports named values and `0.5x0.5` focal-point syntax.
    ///
    /// ```
    /// use oximedia_image_transform::transform::Gravity;
    ///
    /// assert_eq!(Gravity::from_str_loose("center").expect("valid"), Gravity::Center);
    /// assert_eq!(Gravity::from_str_loose("auto").expect("valid"), Gravity::Auto);
    /// assert_eq!(Gravity::from_str_loose("top-left").expect("valid"), Gravity::TopLeft);
    /// ```
    pub fn from_str_loose(s: &str) -> Result<Self, TransformParseError> {
        // Focal point syntax: "0.5x0.5"
        if let Some((lhs, rhs)) = s.split_once('x') {
            if let (Ok(x), Ok(y)) = (lhs.parse::<f64>(), rhs.parse::<f64>()) {
                if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) {
                    return Ok(Self::FocalPoint(x, y));
                }
                return Err(TransformParseError::InvalidGravity(s.to_string()));
            }
        }
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "center" | "centre" => Ok(Self::Center),
            "top" => Ok(Self::Top),
            "bottom" => Ok(Self::Bottom),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            "top-left" | "topleft" | "top_left" => Ok(Self::TopLeft),
            "top-right" | "topright" | "top_right" => Ok(Self::TopRight),
            "bottom-left" | "bottomleft" | "bottom_left" => Ok(Self::BottomLeft),
            "bottom-right" | "bottomright" | "bottom_right" => Ok(Self::BottomRight),
            "face" => Ok(Self::Face),
            _ => Err(TransformParseError::InvalidGravity(s.to_string())),
        }
    }

    /// Canonical string representation.
    pub fn as_str(&self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::Center => "center".to_string(),
            Self::Top => "top".to_string(),
            Self::Bottom => "bottom".to_string(),
            Self::Left => "left".to_string(),
            Self::Right => "right".to_string(),
            Self::TopLeft => "top-left".to_string(),
            Self::TopRight => "top-right".to_string(),
            Self::BottomLeft => "bottom-left".to_string(),
            Self::BottomRight => "bottom-right".to_string(),
            Self::Face => "face".to_string(),
            Self::FocalPoint(x, y) => format!("{x}x{y}"),
        }
    }
}

impl fmt::Display for Gravity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ---------------------------------------------------------------------------
// OutputFormat
// ---------------------------------------------------------------------------

/// Output format for the transformed image.
///
/// ```
/// use oximedia_image_transform::transform::OutputFormat;
///
/// let fmt = OutputFormat::from_str_loose("webp").expect("valid format");
/// assert_eq!(fmt.mime_type(), "image/webp");
/// assert_eq!(fmt.file_extension(), "webp");
/// assert!(fmt.supports_transparency());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputFormat {
    /// Automatic format negotiation based on Accept header.
    Auto,
    /// AV1 Image File Format — best compression, modern browsers.
    Avif,
    /// WebP — good compression, wide browser support.
    WebP,
    /// JPEG — universal support, lossy.
    Jpeg,
    /// PNG — lossless, supports transparency.
    Png,
    /// GIF — animation support, limited colours.
    Gif,
    /// Baseline JPEG (non-progressive).
    Baseline,
    /// JSON metadata output (image dimensions, format info only).
    Json,
}

impl Default for OutputFormat {
    fn default() -> Self {
        Self::Auto
    }
}

impl OutputFormat {
    /// Returns the MIME type for this format.
    ///
    /// ```
    /// use oximedia_image_transform::transform::OutputFormat;
    /// assert_eq!(OutputFormat::Avif.mime_type(), "image/avif");
    /// assert_eq!(OutputFormat::Json.mime_type(), "application/json");
    /// ```
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Auto => "image/jpeg", // fallback
            Self::Avif => "image/avif",
            Self::WebP => "image/webp",
            Self::Jpeg | Self::Baseline => "image/jpeg",
            Self::Png => "image/png",
            Self::Gif => "image/gif",
            Self::Json => "application/json",
        }
    }

    /// Returns the file extension for this format.
    pub fn file_extension(&self) -> &'static str {
        match self {
            Self::Auto => "jpg",
            Self::Avif => "avif",
            Self::WebP => "webp",
            Self::Jpeg | Self::Baseline => "jpg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Json => "json",
        }
    }

    /// Whether this format supports animation frames.
    pub fn supports_animation(&self) -> bool {
        matches!(self, Self::Gif | Self::WebP | Self::Avif)
    }

    /// Whether this format supports alpha transparency.
    pub fn supports_transparency(&self) -> bool {
        matches!(self, Self::Png | Self::WebP | Self::Avif | Self::Gif)
    }

    /// Parse a format string (case-insensitive).
    ///
    /// ```
    /// use oximedia_image_transform::transform::OutputFormat;
    ///
    /// assert_eq!(OutputFormat::from_str_loose("avif").expect("valid"), OutputFormat::Avif);
    /// assert_eq!(OutputFormat::from_str_loose("WEBP").expect("valid"), OutputFormat::WebP);
    /// assert_eq!(OutputFormat::from_str_loose("jpg").expect("valid"), OutputFormat::Jpeg);
    /// assert_eq!(OutputFormat::from_str_loose("json").expect("valid"), OutputFormat::Json);
    /// assert!(OutputFormat::from_str_loose("bmp").is_err());
    /// ```
    pub fn from_str_loose(s: &str) -> Result<Self, TransformParseError> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "avif" => Ok(Self::Avif),
            "webp" => Ok(Self::WebP),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "gif" => Ok(Self::Gif),
            "baseline" => Ok(Self::Baseline),
            "json" => Ok(Self::Json),
            _ => Err(TransformParseError::InvalidFormat(s.to_string())),
        }
    }

    /// Canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Avif => "avif",
            Self::WebP => "webp",
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Gif => "gif",
            Self::Baseline => "baseline",
            Self::Json => "json",
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// MetadataMode
// ---------------------------------------------------------------------------

/// Metadata preservation mode.
///
/// Controls which EXIF/XMP metadata is kept in the output image.
///
/// ```
/// use oximedia_image_transform::transform::MetadataMode;
/// let m = MetadataMode::default();
/// assert_eq!(m, MetadataMode::None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataMode {
    /// Keep all EXIF/XMP metadata.
    Keep,
    /// Strip all metadata (smallest output).
    None,
    /// Keep only copyright-related metadata.
    Copyright,
}

impl Default for MetadataMode {
    fn default() -> Self {
        Self::None
    }
}

impl MetadataMode {
    /// Parse a metadata mode string.
    ///
    /// ```
    /// use oximedia_image_transform::transform::MetadataMode;
    /// assert_eq!(MetadataMode::from_str_loose("keep").expect("valid"), MetadataMode::Keep);
    /// assert_eq!(MetadataMode::from_str_loose("strip").expect("valid"), MetadataMode::None);
    /// ```
    pub fn from_str_loose(s: &str) -> Result<Self, TransformParseError> {
        match s.to_ascii_lowercase().as_str() {
            "keep" | "all" => Ok(Self::Keep),
            "copyright" => Ok(Self::Copyright),
            "none" | "strip" => Ok(Self::None),
            _ => Err(TransformParseError::InvalidParameter {
                name: "metadata".to_string(),
                value: s.to_string(),
            }),
        }
    }

    /// Canonical string representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Copyright => "copyright",
            Self::None => "none",
        }
    }
}

impl fmt::Display for MetadataMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Color (RGBA)
// ---------------------------------------------------------------------------

/// RGBA colour for background/border/padding fill.
///
/// ```
/// use oximedia_image_transform::transform::Color;
///
/// let c = Color::from_css("#ff8800").expect("valid color");
/// assert_eq!(c.r, 255);
/// assert_eq!(c.g, 136);
/// assert_eq!(c.b, 0);
/// assert_eq!(c.a, 255);
///
/// let c = Color::from_css("#ff880080").expect("valid color with alpha");
/// assert_eq!(c.a, 128);
///
/// let t = Color::transparent();
/// assert_eq!(t.a, 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Color {
    /// Red channel (0-255).
    pub r: u8,
    /// Green channel (0-255).
    pub g: u8,
    /// Blue channel (0-255).
    pub b: u8,
    /// Alpha channel (0-255, 0 = fully transparent, 255 = fully opaque).
    pub a: u8,
}

impl Color {
    /// Create a new RGBA colour.
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Fully transparent black.
    pub fn transparent() -> Self {
        Self {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        }
    }

    /// Opaque white.
    pub fn white() -> Self {
        Self {
            r: 255,
            g: 255,
            b: 255,
            a: 255,
        }
    }

    /// Opaque black.
    pub fn black() -> Self {
        Self {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }
    }

    /// Parse a CSS-like colour string.
    ///
    /// Supported formats:
    /// - `#RRGGBB` / `RRGGBB` (alpha defaults to 255)
    /// - `#RRGGBBAA` / `RRGGBBAA`
    /// - `rgb(R,G,B)` where R, G, B are 0-255
    /// - `rgba(R,G,B,A)` where R, G, B are 0-255 and A is 0.0-1.0
    ///
    /// ```
    /// use oximedia_image_transform::transform::Color;
    ///
    /// let c = Color::from_css("#ff0000").expect("parse red");
    /// assert_eq!(c, Color::new(255, 0, 0, 255));
    ///
    /// let c = Color::from_css("00ff00").expect("parse green");
    /// assert_eq!(c, Color::new(0, 255, 0, 255));
    ///
    /// let c = Color::from_css("ff000080").expect("parse red alpha");
    /// assert_eq!(c, Color::new(255, 0, 0, 128));
    ///
    /// let c = Color::from_css("rgb(128,64,32)").expect("parse rgb");
    /// assert_eq!(c, Color::new(128, 64, 32, 255));
    ///
    /// let c = Color::from_css("rgba(128,64,32,0.5)").expect("parse rgba");
    /// assert_eq!(c.a, 128);
    /// ```
    pub fn from_css(s: &str) -> Result<Self, TransformParseError> {
        let s = s.trim();

        // Try rgb(...) / rgba(...)
        if let Some(inner) = s
            .strip_prefix("rgba(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return Self::parse_rgba_func(inner);
        }
        if let Some(inner) = s
            .strip_prefix("rgb(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return Self::parse_rgb_func(inner);
        }

        // Hex formats
        Self::from_hex(s)
    }

    /// Parse a hex colour string: `#RRGGBB`, `RRGGBB`, `#RRGGBBAA`, `RRGGBBAA`.
    pub fn from_hex(s: &str) -> Result<Self, TransformParseError> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        match hex.len() {
            6 => {
                let r = parse_hex_pair(&hex[0..2], s)?;
                let g = parse_hex_pair(&hex[2..4], s)?;
                let b = parse_hex_pair(&hex[4..6], s)?;
                Ok(Self { r, g, b, a: 255 })
            }
            8 => {
                let r = parse_hex_pair(&hex[0..2], s)?;
                let g = parse_hex_pair(&hex[2..4], s)?;
                let b = parse_hex_pair(&hex[4..6], s)?;
                let a = parse_hex_pair(&hex[6..8], s)?;
                Ok(Self { r, g, b, a })
            }
            _ => Err(TransformParseError::InvalidColor(s.to_string())),
        }
    }

    /// Convert to hex string without `#` prefix (6 hex digits if fully opaque, 8 otherwise).
    pub fn to_hex(&self) -> String {
        if self.a == 255 {
            format!("{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }

    // -- private helpers --

    fn parse_rgb_func(inner: &str) -> Result<Self, TransformParseError> {
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() != 3 {
            return Err(TransformParseError::InvalidColor(format!("rgb({inner})")));
        }
        let r = parts[0]
            .parse::<u8>()
            .map_err(|_| TransformParseError::InvalidColor(format!("rgb({inner})")))?;
        let g = parts[1]
            .parse::<u8>()
            .map_err(|_| TransformParseError::InvalidColor(format!("rgb({inner})")))?;
        let b = parts[2]
            .parse::<u8>()
            .map_err(|_| TransformParseError::InvalidColor(format!("rgb({inner})")))?;
        Ok(Self { r, g, b, a: 255 })
    }

    fn parse_rgba_func(inner: &str) -> Result<Self, TransformParseError> {
        let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
        if parts.len() != 4 {
            return Err(TransformParseError::InvalidColor(format!("rgba({inner})")));
        }
        let r = parts[0]
            .parse::<u8>()
            .map_err(|_| TransformParseError::InvalidColor(format!("rgba({inner})")))?;
        let g = parts[1]
            .parse::<u8>()
            .map_err(|_| TransformParseError::InvalidColor(format!("rgba({inner})")))?;
        let b = parts[2]
            .parse::<u8>()
            .map_err(|_| TransformParseError::InvalidColor(format!("rgba({inner})")))?;
        let alpha_f = parts[3]
            .parse::<f64>()
            .map_err(|_| TransformParseError::InvalidColor(format!("rgba({inner})")))?;
        let a = (alpha_f.clamp(0.0, 1.0) * 255.0).round() as u8;
        Ok(Self { r, g, b, a })
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.to_hex())
    }
}

/// Parse a two-character hex pair into a `u8`.
fn parse_hex_pair(pair: &str, original: &str) -> Result<u8, TransformParseError> {
    u8::from_str_radix(pair, 16)
        .map_err(|_| TransformParseError::InvalidColor(original.to_string()))
}

// Re-exported from `transform_types` module.
pub use crate::transform_types::{Border, Compression, OutputOptions, Padding, Rotation, Trim};

// ---------------------------------------------------------------------------
// Aspect ratio utility
// ---------------------------------------------------------------------------

/// Compute target `(width, height)` that preserves the source aspect ratio
/// given a requested `(req_w, req_h)` and a [`FitMode`].
///
/// | Mode                                    | Behaviour                                          |
/// |-----------------------------------------|----------------------------------------------------|
/// | [`FitMode::Contain`] / [`FitMode::ScaleDown`] | Fit *inside* the box — returned dims ≤ `req`. |
/// | [`FitMode::Cover`] / [`FitMode::Crop`]  | Fill the box — returned dims ≥ `req`.              |
/// | All other modes                         | Return `(req_w, req_h)` unchanged.                 |
///
/// If `src_w` or `src_h` is zero the function returns `(req_w, req_h)` unchanged.
///
/// # Example
///
/// ```
/// use oximedia_image_transform::transform::{enforce_aspect_ratio, FitMode};
///
/// // 1600×900 source → fit inside 800×600 box
/// let (w, h) = enforce_aspect_ratio(1600, 900, 800, 600, FitMode::Contain);
/// assert_eq!(w, 800);
/// assert_eq!(h, 450);
///
/// // 1600×900 source → cover 400×400 box
/// let (w, h) = enforce_aspect_ratio(1600, 900, 400, 400, FitMode::Cover);
/// assert_eq!(w, 711); // ≥ 400 on both axes
/// assert_eq!(h, 400);
/// ```
pub fn enforce_aspect_ratio(
    src_w: u32,
    src_h: u32,
    req_w: u32,
    req_h: u32,
    fit_mode: FitMode,
) -> (u32, u32) {
    if src_w == 0 || src_h == 0 {
        return (req_w, req_h);
    }
    match fit_mode {
        FitMode::Contain | FitMode::ScaleDown => {
            // Shrink so that *both* dimensions fit inside the box.
            let scale = f64::min(req_w as f64 / src_w as f64, req_h as f64 / src_h as f64);
            let out_w = (src_w as f64 * scale).round() as u32;
            let out_h = (src_h as f64 * scale).round() as u32;
            (out_w.max(1), out_h.max(1))
        }
        FitMode::Cover | FitMode::Crop => {
            // Scale so that *both* dimensions fill or exceed the box.
            let scale = f64::max(req_w as f64 / src_w as f64, req_h as f64 / src_h as f64);
            let out_w = (src_w as f64 * scale).round() as u32;
            let out_h = (src_h as f64 * scale).round() as u32;
            (out_w.max(1), out_h.max(1))
        }
        // Pad and Fill return the requested box dimensions unchanged.
        FitMode::Pad | FitMode::Fill => (req_w, req_h),
    }
}

// ---------------------------------------------------------------------------
// TransformParams
// ---------------------------------------------------------------------------

/// Complete image transformation parameters (Cloudflare Images-compatible).
///
/// This struct carries every option that the Cloudflare Images URL API supports.
/// Sensible defaults are provided via [`Default`]; individual fields can be
/// overridden as needed.
///
/// # Example
///
/// ```
/// use oximedia_image_transform::transform::TransformParams;
///
/// let p = TransformParams::default();
/// assert_eq!(p.quality, 85);
/// assert!(p.effective_width().is_none()); // no width set
///
/// let mut p = TransformParams::default();
/// p.width = Some(800);
/// p.dpr = 2.0;
/// assert_eq!(p.effective_width(), Some(1600));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct TransformParams {
    /// Desired output width in pixels (before DPR multiplication).
    pub width: Option<u32>,
    /// Desired output height in pixels (before DPR multiplication).
    pub height: Option<u32>,
    /// Output quality, 1-100 (default 85).
    pub quality: u8,
    /// Output image format.
    pub format: OutputFormat,
    /// How the image should fit into the target dimensions.
    pub fit: FitMode,
    /// Crop anchor point.
    pub gravity: Gravity,
    /// Sharpen amount (0.0-10.0, 0 = no sharpening).
    pub sharpen: f64,
    /// Gaussian blur radius in pixels (0.0-250.0, 0 = no blur).
    pub blur: f64,
    /// Brightness adjustment (-1.0 to 1.0, 0 = no change).
    pub brightness: f64,
    /// Contrast adjustment (-1.0 to 1.0, 0 = no change).
    pub contrast: f64,
    /// Gamma correction (>0.0, default 1.0 = no change).
    pub gamma: f64,
    /// Rotation.
    pub rotate: Rotation,
    /// Auto-trim edges specification.
    pub trim: Option<Trim>,
    /// Device pixel ratio multiplier (default 1.0, max 4.0).
    pub dpr: f64,
    /// Metadata preservation mode.
    pub metadata: MetadataMode,
    /// Whether to preserve animation frames (default `true`).
    pub anim: bool,
    /// Background colour for transparent images or padding.
    pub background: Color,
    /// Optional border around the image.
    pub border: Option<Border>,
    /// Optional padding (fractional).
    pub pad: Option<Padding>,
    /// Compression strategy hint.
    pub compression: Option<String>,
    /// Error-handling fallback behaviour (e.g. `"redirect"`).
    pub onerror: Option<String>,
    /// Optional per-request output encoding options (progressive JPEG, quality
    /// override, etc.). When absent, sensible defaults are used.
    pub output_options: Option<OutputOptions>,
}

impl Default for TransformParams {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            quality: DEFAULT_QUALITY,
            format: OutputFormat::Auto,
            fit: FitMode::ScaleDown,
            gravity: Gravity::Center,
            sharpen: 0.0,
            blur: 0.0,
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.0,
            rotate: Rotation::Deg0,
            trim: None,
            dpr: 1.0,
            metadata: MetadataMode::None,
            anim: true,
            background: Color::transparent(),
            border: None,
            pad: None,
            compression: None,
            onerror: None,
            output_options: None,
        }
    }
}

impl TransformParams {
    /// Get effective width after DPR multiplication.
    ///
    /// ```
    /// use oximedia_image_transform::transform::TransformParams;
    ///
    /// let mut p = TransformParams::default();
    /// p.width = Some(400);
    /// p.dpr = 2.0;
    /// assert_eq!(p.effective_width(), Some(800));
    /// ```
    pub fn effective_width(&self) -> Option<u32> {
        self.width.map(|w| {
            let scaled = (f64::from(w) * self.dpr).round() as u32;
            scaled.min(MAX_DIMENSION).max(1)
        })
    }

    /// Get effective height after DPR multiplication.
    ///
    /// ```
    /// use oximedia_image_transform::transform::TransformParams;
    ///
    /// let mut p = TransformParams::default();
    /// p.height = Some(300);
    /// p.dpr = 3.0;
    /// assert_eq!(p.effective_height(), Some(900));
    /// ```
    pub fn effective_height(&self) -> Option<u32> {
        self.height.map(|h| {
            let scaled = (f64::from(h) * self.dpr).round() as u32;
            scaled.min(MAX_DIMENSION).max(1)
        })
    }

    /// Returns `true` if the output should be encoded as a progressive JPEG.
    ///
    /// Progressive JPEG mode is enabled when:
    /// - `output_options` is set **and**
    /// - `output_options.progressive_jpeg` is `true` **and**
    /// - the output format is JPEG-compatible (`Jpeg`, `Baseline`, or `Auto`).
    ///
    /// For non-JPEG formats (WebP, AVIF, PNG, …) this always returns `false`
    /// because progressive encoding is JPEG-specific.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_image_transform::transform::{OutputFormat, OutputOptions, TransformParams};
    ///
    /// let mut p = TransformParams::default();
    /// assert!(!p.is_progressive_output());
    ///
    /// p.output_options = Some(OutputOptions { progressive_jpeg: true, quality: 85 });
    /// // Default format is Auto, which is JPEG-compatible.
    /// assert!(p.is_progressive_output());
    ///
    /// p.format = OutputFormat::WebP;
    /// assert!(!p.is_progressive_output(), "progressive does not apply to WebP");
    /// ```
    pub fn is_progressive_output(&self) -> bool {
        let opts = match &self.output_options {
            Some(o) => o,
            None => return false,
        };
        if !opts.progressive_jpeg {
            return false;
        }
        // Progressive encoding is only meaningful for JPEG streams.
        matches!(
            self.format,
            OutputFormat::Jpeg | OutputFormat::Baseline | OutputFormat::Auto
        )
    }

    /// Validate all parameter ranges.
    ///
    /// Checks:
    /// - width/height in 1..=12000
    /// - quality in 1..=100
    /// - dpr in 1.0..=4.0
    /// - sharpen in 0.0..=10.0
    /// - blur in 0.0..=250.0
    /// - brightness in -1.0..=1.0
    /// - contrast in -1.0..=1.0
    /// - gamma in 0.0..=10.0
    /// - focal point coordinates in 0.0..=1.0
    ///
    /// ```
    /// use oximedia_image_transform::transform::TransformParams;
    ///
    /// let p = TransformParams::default();
    /// assert!(p.validate().is_ok());
    ///
    /// let mut p = TransformParams::default();
    /// p.width = Some(20000);
    /// assert!(p.validate().is_err());
    /// ```
    pub fn validate(&self) -> Result<(), TransformParseError> {
        // Dimensions
        if let Some(w) = self.width {
            if w == 0 || w > MAX_DIMENSION {
                return Err(TransformParseError::DimensionTooLarge(w, MAX_DIMENSION));
            }
        }
        if let Some(h) = self.height {
            if h == 0 || h > MAX_DIMENSION {
                return Err(TransformParseError::DimensionTooLarge(h, MAX_DIMENSION));
            }
        }

        // Quality
        if self.quality == 0 || self.quality > 100 {
            return Err(TransformParseError::InvalidQuality(self.quality));
        }

        // DPR
        if self.dpr < MIN_DPR || self.dpr > MAX_DPR {
            return Err(TransformParseError::InvalidDpr(self.dpr));
        }

        // Sharpen
        if self.sharpen < 0.0 || self.sharpen > MAX_SHARPEN {
            return Err(TransformParseError::InvalidParameter {
                name: "sharpen".to_string(),
                value: self.sharpen.to_string(),
            });
        }

        // Blur
        if self.blur < 0.0 || self.blur > MAX_BLUR_RADIUS {
            return Err(TransformParseError::InvalidParameter {
                name: "blur".to_string(),
                value: self.blur.to_string(),
            });
        }

        // Brightness
        if self.brightness < -1.0 || self.brightness > 1.0 {
            return Err(TransformParseError::InvalidParameter {
                name: "brightness".to_string(),
                value: self.brightness.to_string(),
            });
        }

        // Contrast
        if self.contrast < -1.0 || self.contrast > 1.0 {
            return Err(TransformParseError::InvalidParameter {
                name: "contrast".to_string(),
                value: self.contrast.to_string(),
            });
        }

        // Gamma
        if self.gamma < 0.0 || self.gamma > MAX_GAMMA {
            return Err(TransformParseError::InvalidParameter {
                name: "gamma".to_string(),
                value: self.gamma.to_string(),
            });
        }

        // Focal point
        if let Gravity::FocalPoint(x, y) = &self.gravity {
            if *x < 0.0 || *x > 1.0 || *y < 0.0 || *y > 1.0 {
                return Err(TransformParseError::InvalidGravity(format!("{x}x{y}")));
            }
        }

        Ok(())
    }

    /// Generate a deterministic cache key string from these transform params.
    ///
    /// Only non-default values are included. Keys are sorted alphabetically
    /// for determinism.
    ///
    /// ```
    /// use oximedia_image_transform::transform::TransformParams;
    ///
    /// let mut p = TransformParams::default();
    /// p.width = Some(800);
    /// let key = p.cache_key();
    /// assert!(key.contains("width=800"));
    /// ```
    pub fn cache_key(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let defaults = TransformParams::default();

        if self.anim != defaults.anim {
            parts.push(format!("anim={}", self.anim));
        }
        if self.background != defaults.background {
            parts.push(format!("background={}", self.background.to_hex()));
        }
        if (self.blur - defaults.blur).abs() > f64::EPSILON {
            parts.push(format!("blur={}", self.blur));
        }
        if let Some(ref border) = self.border {
            parts.push(format!(
                "border={},{},{},{}:{}",
                border.top,
                border.right,
                border.bottom,
                border.left,
                border.color.to_hex()
            ));
        }
        if (self.brightness - defaults.brightness).abs() > f64::EPSILON {
            parts.push(format!("brightness={}", self.brightness));
        }
        if let Some(ref comp) = self.compression {
            parts.push(format!("compression={comp}"));
        }
        if (self.contrast - defaults.contrast).abs() > f64::EPSILON {
            parts.push(format!("contrast={}", self.contrast));
        }
        if (self.dpr - defaults.dpr).abs() > f64::EPSILON {
            parts.push(format!("dpr={}", self.dpr));
        }
        if self.fit != defaults.fit {
            parts.push(format!("fit={}", self.fit));
        }
        if self.format != defaults.format {
            parts.push(format!("format={}", self.format));
        }
        if (self.gamma - defaults.gamma).abs() > f64::EPSILON {
            parts.push(format!("gamma={}", self.gamma));
        }
        if self.gravity != defaults.gravity {
            parts.push(format!("gravity={}", self.gravity));
        }
        if let Some(h) = self.height {
            parts.push(format!("height={h}"));
        }
        if self.metadata != defaults.metadata {
            parts.push(format!("metadata={}", self.metadata));
        }
        if let Some(ref pad) = self.pad {
            parts.push(format!(
                "pad={},{},{},{}",
                pad.top, pad.right, pad.bottom, pad.left
            ));
        }
        if self.quality != defaults.quality {
            parts.push(format!("quality={}", self.quality));
        }
        if self.rotate != defaults.rotate {
            parts.push(format!("rotate={}", self.rotate));
        }
        if (self.sharpen - defaults.sharpen).abs() > f64::EPSILON {
            parts.push(format!("sharpen={}", self.sharpen));
        }
        if let Some(ref trim) = self.trim {
            parts.push(format!(
                "trim={},{},{},{}",
                trim.top, trim.right, trim.bottom, trim.left
            ));
        }
        if let Some(w) = self.width {
            parts.push(format!("width={w}"));
        }
        // onerror is intentionally excluded (affects error handling, not output)
        if let Some(ref opts) = self.output_options {
            if opts.progressive_jpeg {
                parts.push("progressive=true".to_string());
            }
            if opts.quality != DEFAULT_QUALITY {
                parts.push(format!("output_quality={}", opts.quality));
            }
        }

        parts.join(",")
    }

    /// Serialize these parameters back into a canonical comma-separated
    /// transform string that [`parse_transform_string`] re-parses into an
    /// equal `TransformParams`.
    ///
    /// [`parse_transform_string`]: crate::parser::parse_transform_string
    ///
    /// Unlike [`cache_key`](Self::cache_key) — which sorts keys alphabetically
    /// and deliberately omits round-trip-irrelevant fields such as `onerror`
    /// (so that distinct error-handling behaviour shares one cache entry) — this
    /// method is a *true inverse* of the parser:
    ///
    /// - Fields are emitted in a fixed, human-readable **canonical order**
    ///   (dimensions → quality → format → fit → gravity → effects → geometry →
    ///   delivery options), independent of how the input string was ordered.
    /// - Fields at their [`Default`] value are **omitted**, keeping the output
    ///   compact and making `parse → serialize → parse` stable.
    /// - Every emitted key is one the parser can read back, so the result is
    ///   guaranteed to re-parse into a value equal to `self`.
    ///
    /// As a consequence the function is **idempotent** after the first round:
    /// `serialize(parse(serialize(p))) == serialize(p)`.
    ///
    /// # Example
    ///
    /// ```
    /// use oximedia_image_transform::parser::parse_transform_string;
    /// use oximedia_image_transform::transform::TransformParams;
    ///
    /// let original = parse_transform_string("h=600,w=800,q=85").expect("parse");
    /// let canonical = original.serialize();
    /// // Canonical order places width before height; default quality (85) is dropped.
    /// assert_eq!(canonical, "width=800,height=600");
    ///
    /// let reparsed = parse_transform_string(&canonical).expect("reparse");
    /// assert_eq!(original, reparsed);
    /// ```
    pub fn serialize(&self) -> String {
        let defaults = TransformParams::default();
        let opt_defaults = OutputOptions::default();
        let mut parts: Vec<String> = Vec::new();

        // -- Dimensions --
        if let Some(w) = self.width {
            parts.push(format!("width={w}"));
        }
        if let Some(h) = self.height {
            parts.push(format!("height={h}"));
        }

        // -- Quality / format --
        if self.quality != defaults.quality {
            parts.push(format!("quality={}", self.quality));
        }
        if self.format != defaults.format {
            parts.push(format!("format={}", self.format));
        }

        // -- Fit / gravity --
        if self.fit != defaults.fit {
            parts.push(format!("fit={}", self.fit));
        }
        if self.gravity != defaults.gravity {
            parts.push(format!("gravity={}", self.gravity));
        }

        // -- Pixel effects --
        if (self.sharpen - defaults.sharpen).abs() > f64::EPSILON {
            parts.push(format!("sharpen={}", self.sharpen));
        }
        if (self.blur - defaults.blur).abs() > f64::EPSILON {
            parts.push(format!("blur={}", self.blur));
        }
        if (self.brightness - defaults.brightness).abs() > f64::EPSILON {
            parts.push(format!("brightness={}", self.brightness));
        }
        if (self.contrast - defaults.contrast).abs() > f64::EPSILON {
            parts.push(format!("contrast={}", self.contrast));
        }
        if (self.gamma - defaults.gamma).abs() > f64::EPSILON {
            parts.push(format!("gamma={}", self.gamma));
        }

        // -- Geometry --
        if self.rotate != defaults.rotate {
            parts.push(format!("rotate={}", self.rotate));
        }
        if let Some(ref trim) = self.trim {
            // Comma-free encoding: the comma is the top-level field delimiter,
            // so four-sided values use `x` (e.g. `trim=10x5x10x5`). A uniform
            // trim collapses to the single-value short form.
            if trim.top == trim.right && trim.right == trim.bottom && trim.bottom == trim.left {
                parts.push(format!("trim={}", trim.top));
            } else {
                parts.push(format!(
                    "trim={}x{}x{}x{}",
                    trim.top, trim.right, trim.bottom, trim.left
                ));
            }
        }
        if let Some(ref border) = self.border {
            let dims = if border.top == border.right
                && border.right == border.bottom
                && border.bottom == border.left
            {
                format!("{}", border.top)
            } else {
                format!(
                    "{}x{}x{}x{}",
                    border.top, border.right, border.bottom, border.left
                )
            };
            parts.push(format!("border={}:{}", dims, border.color.to_hex()));
        }
        if let Some(ref pad) = self.pad {
            if pad.top == pad.right && pad.right == pad.bottom && pad.bottom == pad.left {
                parts.push(format!("pad={}", pad.top));
            } else {
                parts.push(format!(
                    "pad={}x{}x{}x{}",
                    pad.top, pad.right, pad.bottom, pad.left
                ));
            }
        }

        // -- Delivery options --
        if (self.dpr - defaults.dpr).abs() > f64::EPSILON {
            parts.push(format!("dpr={}", self.dpr));
        }
        if self.metadata != defaults.metadata {
            parts.push(format!("metadata={}", self.metadata));
        }
        if self.anim != defaults.anim {
            parts.push(format!("anim={}", self.anim));
        }
        if self.background != defaults.background {
            parts.push(format!("background={}", self.background.to_hex()));
        }
        if let Some(ref comp) = self.compression {
            parts.push(format!("compression={comp}"));
        }
        if let Some(ref onerror) = self.onerror {
            parts.push(format!("onerror={onerror}"));
        }
        if let Some(ref opts) = self.output_options {
            if opts.progressive_jpeg != opt_defaults.progressive_jpeg {
                parts.push(format!("progressive={}", opts.progressive_jpeg));
            }
            if opts.quality != opt_defaults.quality {
                parts.push(format!("output_quality={}", opts.quality));
            }
        }

        parts.join(",")
    }

    /// Returns `true` if this transform is effectively a no-op.
    pub fn is_identity(&self) -> bool {
        let defaults = TransformParams::default();
        self.width.is_none()
            && self.height.is_none()
            && self.format == defaults.format
            && (self.sharpen - defaults.sharpen).abs() < f64::EPSILON
            && (self.blur - defaults.blur).abs() < f64::EPSILON
            && (self.brightness - defaults.brightness).abs() < f64::EPSILON
            && (self.contrast - defaults.contrast).abs() < f64::EPSILON
            && (self.gamma - defaults.gamma).abs() < f64::EPSILON
            && self.rotate == defaults.rotate
            && self.trim.is_none()
            && self.border.is_none()
            && self.pad.is_none()
    }
}

impl fmt::Display for TransformParams {
    /// Format back to Cloudflare-style comma-separated string:
    /// `width=800,height=600,quality=85,format=auto`.
    ///
    /// Only includes fields that differ from the defaults.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.cache_key())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "transform_tests.rs"]
mod tests;
