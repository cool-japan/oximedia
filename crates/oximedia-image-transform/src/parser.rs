// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! URL transformation parser for Cloudflare Images-compatible URLs.
//!
//! Supports three URL formats:
//! - **CDN path**: `/cdn-cgi/image/width=800,height=600/path/to/image.jpg`
//! - **Comma-separated**: `width=800,height=600,quality=85,format=auto`
//! - **Query-string**: `width=800&height=600&quality=85`
//!
//! Both long and short parameter names are supported:
//! - `w` / `width`, `h` / `height`, `q` / `quality`, `f` / `format`,
//!   `g` / `gravity`, `bg` / `background`
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::parser::{parse_cdn_url, parse_transform_string};
//! use oximedia_image_transform::transform::OutputFormat;
//!
//! let req = parse_cdn_url("/cdn-cgi/image/width=800,format=auto/images/photo.jpg").expect("parse url");
//! assert_eq!(req.params.width, Some(800));
//! assert_eq!(req.params.format, OutputFormat::Auto);
//! assert_eq!(req.source_path, "images/photo.jpg");
//! ```

use crate::transform::{
    Border, Color, Compression, FitMode, Gravity, MetadataMode, OutputFormat, Padding, Rotation,
    TransformParams, TransformParseError, Trim,
};

// ---------------------------------------------------------------------------
// TransformPreset
// ---------------------------------------------------------------------------

/// Named transform presets that map to commonly-used [`TransformParams`] configurations.
///
/// Presets provide a convenient shorthand for frequently-used dimension and
/// quality combinations. They can be referenced in CDN URLs as
/// `preset=thumbnail` (or the short aliases listed per variant).
///
/// ```
/// use oximedia_image_transform::parser::{parse_preset, TransformPreset};
///
/// let preset = parse_preset("thumbnail").expect("parse preset");
/// let params = preset.to_params();
/// assert_eq!(params.width, Some(100));
/// assert_eq!(params.height, Some(100));
/// assert_eq!(params.quality, 75);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformPreset {
    /// 100 × 100 pixels, quality 75 — suitable for grid thumbnails and list views.
    Thumbnail,
    /// 800 × 600 pixels, quality 85 — suitable for preview lightboxes.
    Preview,
    /// 1280 × 720 pixels, quality 90 — suitable for HD-ready display.
    HdReady,
}

impl TransformPreset {
    /// Convert this preset into a fully-populated [`TransformParams`].
    ///
    /// The returned params use [`FitMode::ScaleDown`] so the image is never
    /// upscaled beyond its native resolution.
    pub fn to_params(self) -> TransformParams {
        let mut params = TransformParams::default();
        match self {
            TransformPreset::Thumbnail => {
                params.width = Some(100);
                params.height = Some(100);
                params.quality = 75;
            }
            TransformPreset::Preview => {
                params.width = Some(800);
                params.height = Some(600);
                params.quality = 85;
            }
            TransformPreset::HdReady => {
                params.width = Some(1280);
                params.height = Some(720);
                params.quality = 90;
            }
        }
        params
    }

    /// Return the canonical name string for this preset (lower-snake-case).
    pub fn as_str(self) -> &'static str {
        match self {
            TransformPreset::Thumbnail => "thumbnail",
            TransformPreset::Preview => "preview",
            TransformPreset::HdReady => "hd_ready",
        }
    }
}

/// Parse a preset name string into a [`TransformPreset`], or return `None` if
/// the name is not recognised.
///
/// Matching is case-insensitive and accepts a selection of common aliases:
///
/// | Preset        | Accepted names                              |
/// |---------------|---------------------------------------------|
/// | `Thumbnail`   | `thumbnail`, `thumb`, `tn`                  |
/// | `Preview`     | `preview`, `prev`                           |
/// | `HdReady`     | `hd_ready`, `hd-ready`, `hdready`, `hd720`  |
///
/// # Example
///
/// ```
/// use oximedia_image_transform::parser::{parse_preset, TransformPreset};
///
/// assert_eq!(parse_preset("thumb"), Some(TransformPreset::Thumbnail));
/// assert_eq!(parse_preset("PREVIEW"), Some(TransformPreset::Preview));
/// assert_eq!(parse_preset("hd720"), Some(TransformPreset::HdReady));
/// assert!(parse_preset("unknown").is_none());
/// ```
pub fn parse_preset(name: &str) -> Option<TransformPreset> {
    match name.trim().to_ascii_lowercase().as_str() {
        "thumbnail" | "thumb" | "tn" => Some(TransformPreset::Thumbnail),
        "preview" | "prev" => Some(TransformPreset::Preview),
        "hd_ready" | "hd-ready" | "hdready" | "hd720" => Some(TransformPreset::HdReady),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// TransformRequest
// ---------------------------------------------------------------------------

/// A parsed transform request comprising the transformation parameters and the
/// source image path.
///
/// ```
/// use oximedia_image_transform::parser::{parse_cdn_url, TransformRequest};
///
/// let req = parse_cdn_url("/cdn-cgi/image/width=400/photo.jpg").expect("parse url");
/// assert_eq!(req.source_path, "photo.jpg");
/// assert_eq!(req.params.width, Some(400));
/// ```
#[derive(Debug, Clone)]
pub struct TransformRequest {
    /// Parsed transformation parameters.
    pub params: TransformParams,
    /// Source image path (sanitised, no leading slash).
    pub source_path: String,
}

// ---------------------------------------------------------------------------
// Public parsing functions
// ---------------------------------------------------------------------------

/// Parse `/cdn-cgi/image/<transforms>/<path>` URL format.
///
/// Strips the `/cdn-cgi/image/` prefix, splits transform parameters from the
/// source image path, and returns a [`TransformRequest`].
///
/// ```
/// use oximedia_image_transform::parser::parse_cdn_url;
/// use oximedia_image_transform::transform::{FitMode, OutputFormat};
///
/// let req = parse_cdn_url("/cdn-cgi/image/w=400,q=85,f=webp,fit=cover/uploads/2024/banner.png")
///     .expect("parse url");
/// assert_eq!(req.params.width, Some(400));
/// assert_eq!(req.params.quality, 85);
/// assert_eq!(req.params.format, OutputFormat::WebP);
/// assert_eq!(req.params.fit, FitMode::Cover);
/// assert_eq!(req.source_path, "uploads/2024/banner.png");
/// ```
pub fn parse_cdn_url(path: &str) -> Result<TransformRequest, TransformParseError> {
    let path = path.trim();
    let path = path.strip_prefix('/').unwrap_or(path);

    let rest = path.strip_prefix("cdn-cgi/image/").ok_or_else(|| {
        TransformParseError::ParseError("URL must start with /cdn-cgi/image/".to_string())
    })?;

    if rest.is_empty() {
        return Err(TransformParseError::ParseError(
            "Missing transform parameters and image path".to_string(),
        ));
    }

    let (transform_str, image_path) = split_transform_and_path(rest)?;
    let params = parse_transform_string(transform_str)?;
    let clean_path = sanitize_image_path(&image_path)?;

    Ok(TransformRequest {
        params,
        source_path: clean_path,
    })
}

/// Parse query parameter format: `width=800&height=600&format=auto`.
///
/// Unknown keys are silently ignored for forward compatibility.
///
/// ```
/// use oximedia_image_transform::parser::parse_query_params;
/// use oximedia_image_transform::transform::OutputFormat;
///
/// let params = parse_query_params("w=200&h=100&q=75&f=webp").expect("parse query");
/// assert_eq!(params.width, Some(200));
/// assert_eq!(params.height, Some(100));
/// assert_eq!(params.quality, 75);
/// assert_eq!(params.format, OutputFormat::WebP);
/// ```
pub fn parse_query_params(query: &str) -> Result<TransformParams, TransformParseError> {
    let mut params = TransformParams::default();

    if query.is_empty() {
        return Ok(params);
    }

    for pair in query.split('&') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => continue,
        };
        parse_param(&mut params, key, value)?;
    }

    Ok(params)
}

/// Parse a comma-separated transform string: `"width=800,height=600,quality=85"`.
///
/// Unknown keys are silently ignored for forward compatibility.
/// Bare keys without a value are also ignored.
///
/// ```
/// use oximedia_image_transform::parser::parse_transform_string;
///
/// let params = parse_transform_string("width=800,height=600,quality=85").expect("parse transform");
/// assert_eq!(params.width, Some(800));
/// assert_eq!(params.height, Some(600));
/// assert_eq!(params.quality, 85);
/// ```
pub fn parse_transform_string(s: &str) -> Result<TransformParams, TransformParseError> {
    let mut params = TransformParams::default();

    if s.is_empty() {
        return Ok(params);
    }

    for pair in s.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => {
                // Bare key with no value — ignore for forward compat
                continue;
            }
        };
        parse_param(&mut params, key, value)?;
    }

    Ok(params)
}

/// Generate a deterministic cache key from a [`TransformRequest`].
///
/// The key combines the source path with an FNV-1a hash of the normalised
/// transform parameters plus the source path.
///
/// ```
/// use oximedia_image_transform::parser::{parse_cdn_url, generate_cache_key};
///
/// let req = parse_cdn_url("/cdn-cgi/image/width=800/photo.jpg").expect("parse url");
/// let key1 = generate_cache_key(&req);
/// let key2 = generate_cache_key(&req);
/// assert_eq!(key1, key2);
/// ```
pub fn generate_cache_key(request: &TransformRequest) -> String {
    let transform_part = request.params.cache_key();
    let combined = format!("{}:{}", request.source_path, transform_part);
    let hash = fnv1a_hash(combined.as_bytes());
    format!("{}_{:016x}", request.source_path, hash)
}

// ---------------------------------------------------------------------------
// Legacy API (kept for backwards compatibility with negotiation module)
// ---------------------------------------------------------------------------

/// Parse a `/cdn-cgi/image/<transform_str>/<original_path>` URL.
///
/// Returns the parsed transform and the original image path as a tuple.
/// Prefer [`parse_cdn_url`] which returns a [`TransformRequest`].
pub fn parse_cdn_cgi_path(path: &str) -> Result<(TransformParams, String), TransformParseError> {
    let req = parse_cdn_url(path)?;
    Ok((req.params, req.source_path))
}

/// Cache key for transformed images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    /// FNV-1a hash of normalised transform parameters + source path.
    pub transform_hash: u64,
    /// Source image path.
    pub source_path: String,
}

impl CacheKey {
    /// Produce a combined string key suitable for file paths or cache lookups.
    pub fn as_string(&self) -> String {
        format!("{}_{:016x}", self.source_path, self.transform_hash)
    }
}

/// Compute a cache key from image path and transform params.
///
/// Uses FNV-1a hashing of the normalised (sorted, canonical) transform string.
pub fn compute_cache_key(path: &str, params: &TransformParams) -> CacheKey {
    let normalized = params.cache_key();
    let hash = fnv1a_hash(format!("{path}:{normalized}").as_bytes());
    CacheKey {
        transform_hash: hash,
        source_path: path.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Internal: parse a single parameter
// ---------------------------------------------------------------------------

/// Parse a single transform parameter key=value pair.
///
/// Handles all Cloudflare Images parameters including short aliases:
/// `w`, `h`, `q`, `f`, `g`, `bg`.
fn parse_param(
    params: &mut TransformParams,
    key: &str,
    value: &str,
) -> Result<(), TransformParseError> {
    match key.to_ascii_lowercase().as_str() {
        // Dimensions
        "width" | "w" => {
            params.width = Some(parse_u32(key, value)?);
        }
        "height" | "h" => {
            params.height = Some(parse_u32(key, value)?);
        }

        // Quality
        "quality" | "q" => {
            params.quality = parse_u8(key, value)?;
        }

        // Format
        "format" | "f" => {
            params.format = OutputFormat::from_str_loose(value)?;
        }

        // Fit
        "fit" => {
            params.fit = FitMode::from_str_loose(value)?;
        }

        // Gravity
        "gravity" | "g" => {
            params.gravity = Gravity::from_str_loose(value)?;
        }

        // Effects
        "sharpen" => {
            params.sharpen = parse_f64(key, value)?;
        }
        "blur" => {
            params.blur = parse_f64(key, value)?;
        }
        "brightness" => {
            params.brightness = parse_f64(key, value)?;
        }
        "contrast" => {
            params.contrast = parse_f64(key, value)?;
        }
        "gamma" => {
            params.gamma = parse_f64(key, value)?;
        }

        // Rotation
        "rotate" => {
            params.rotate = Rotation::from_str_loose(value)?;
        }

        // Trim
        "trim" => {
            params.trim = Some(parse_trim(value)?);
        }

        // DPR
        "dpr" => {
            params.dpr = parse_f64(key, value)?;
        }

        // Metadata
        "metadata" => {
            params.metadata = MetadataMode::from_str_loose(value)?;
        }

        // Animation
        "anim" => {
            params.anim = parse_bool(value);
        }

        // Background
        "background" | "bg" => {
            params.background = Color::from_css(value)?;
        }

        // Border
        "border" => {
            params.border = Some(parse_border(value)?);
        }

        // Padding
        "pad" | "padding" => {
            params.pad = Some(parse_padding(value)?);
        }

        // Compression
        "compression" => {
            // Validate via Compression enum, then store as string
            let c = Compression::from_str_loose(value)?;
            params.compression = Some(c.as_str().to_string());
        }

        // On-error
        "onerror" => {
            params.onerror = Some(value.to_string());
        }

        // Progressive JPEG output flag (inverse of the `progressive=` token
        // emitted by `TransformParams::serialize`).
        "progressive" => {
            let opts = params.output_options.get_or_insert_with(Default::default);
            opts.progressive_jpeg = parse_bool(value);
        }

        // Per-encoding output quality override (inverse of the `output_quality=`
        // token emitted by `TransformParams::serialize`).
        "output_quality" => {
            let q = parse_u8(key, value)?;
            let opts = params.output_options.get_or_insert_with(Default::default);
            opts.quality = q;
        }

        // Preset shorthand: applies width/height/quality from the named preset.
        "preset" => {
            if let Some(preset) = parse_preset(value) {
                let preset_params = preset.to_params();
                // Only apply preset values that have not already been set by
                // explicit parameters earlier in the parse stream.
                if params.width.is_none() {
                    params.width = preset_params.width;
                }
                if params.height.is_none() {
                    params.height = preset_params.height;
                }
                if params.quality == crate::transform::DEFAULT_QUALITY {
                    params.quality = preset_params.quality;
                }
            }
            // Unknown preset names are silently ignored.
        }

        _ => {
            // Unknown keys silently ignored for forward compatibility
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Split the rest of the path (after `cdn-cgi/image/`) into transform string
/// and image path.
fn split_transform_and_path(rest: &str) -> Result<(&str, String), TransformParseError> {
    if let Some(idx) = rest.find('/') {
        let transform_part = &rest[..idx];
        let path_part = &rest[idx + 1..];
        if path_part.is_empty() {
            return Err(TransformParseError::MissingSourcePath);
        }
        Ok((transform_part, path_part.to_string()))
    } else {
        Err(TransformParseError::MissingSourcePath)
    }
}

/// Sanitize an image path: decode percent-encoding, prevent directory traversal.
fn sanitize_image_path(path: &str) -> Result<String, TransformParseError> {
    let decoded = percent_decode(path);

    // Strip path traversal sequences
    let clean = decoded
        .replace("../", "")
        .replace("..\\", "")
        .replace('\0', "");

    // Reject paths that still contain suspicious patterns
    if clean.contains("..") {
        return Err(TransformParseError::SecurityViolation(
            "Path traversal detected".to_string(),
        ));
    }

    // Normalise leading slashes
    let clean = clean.trim_start_matches('/');
    if clean.is_empty() {
        return Err(TransformParseError::MissingSourcePath);
    }

    Ok(clean.to_string())
}

/// Simple percent-decoding (handles %XX sequences).
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_digit(bytes[i + 1]);
            let lo = hex_digit(bytes[i + 2]);
            if let (Some(h), Some(l)) = (hi, lo) {
                result.push(char::from(h << 4 | l));
                i += 3;
                continue;
            }
        }
        result.push(char::from(bytes[i]));
        i += 1;
    }
    result
}

/// Convert a hex ASCII byte to its numeric value.
fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// FNV-1a hash (64-bit).
fn fnv1a_hash(data: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// -- Numeric parsers --

fn parse_u32(key: &str, value: &str) -> Result<u32, TransformParseError> {
    value
        .parse::<u32>()
        .map_err(|_| TransformParseError::InvalidParameter {
            name: key.to_string(),
            value: value.to_string(),
        })
}

fn parse_u8(key: &str, value: &str) -> Result<u8, TransformParseError> {
    value
        .parse::<u8>()
        .map_err(|_| TransformParseError::InvalidParameter {
            name: key.to_string(),
            value: value.to_string(),
        })
}

fn parse_f64(key: &str, value: &str) -> Result<f64, TransformParseError> {
    value
        .parse::<f64>()
        .map_err(|_| TransformParseError::InvalidParameter {
            name: key.to_string(),
            value: value.to_string(),
        })
}

fn parse_bool(value: &str) -> bool {
    matches!(value.to_ascii_lowercase().as_str(), "true" | "1" | "yes")
}

/// Split a multi-side geometry value into its components.
///
/// Accepts both the comma form (`"10,5,10,5"`, used by query-string parsing
/// where the comma is not a top-level delimiter) and the `x` form
/// (`"10x5x10x5"`, emitted by [`TransformParams::serialize`] so the value
/// survives the comma-separated top-level split). Empty segments are dropped.
///
/// [`TransformParams::serialize`]: crate::transform::TransformParams::serialize
fn split_sides(value: &str) -> Vec<&str> {
    value
        .split(['x', ','])
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Parse a trim specification.
///
/// Supports:
/// - Single value: `"10"` -> uniform trim of 10px on all sides
/// - Four values: `"10,5,10,5"` or `"10x5x10x5"` -> top, right, bottom, left
fn parse_trim(value: &str) -> Result<Trim, TransformParseError> {
    if value.is_empty() {
        return Ok(Trim::uniform(10)); // default threshold
    }
    let parts = split_sides(value);
    match parts.len() {
        1 => {
            let v = parse_u32("trim", parts[0])?;
            Ok(Trim::uniform(v))
        }
        4 => {
            let top = parse_u32("trim", parts[0])?;
            let right = parse_u32("trim", parts[1])?;
            let bottom = parse_u32("trim", parts[2])?;
            let left = parse_u32("trim", parts[3])?;
            Ok(Trim {
                top,
                right,
                bottom,
                left,
            })
        }
        _ => Err(TransformParseError::InvalidParameter {
            name: "trim".to_string(),
            value: value.to_string(),
        }),
    }
}

/// Parse border specification.
///
/// Format: `width:color` e.g. `"5:ff0000"`, or `"top,right,bottom,left:color"`
/// / `"topxrightxbottomxleft:color"` for per-side widths.
fn parse_border(value: &str) -> Result<Border, TransformParseError> {
    let (dims_str, color_str) =
        value
            .split_once(':')
            .ok_or_else(|| TransformParseError::InvalidParameter {
                name: "border".to_string(),
                value: value.to_string(),
            })?;

    let color = Color::from_css(color_str)?;
    let dim_parts = split_sides(dims_str);

    match dim_parts.len() {
        1 => {
            let w = parse_u32("border", dim_parts[0])?;
            Ok(Border::uniform(w, color))
        }
        4 => {
            let top = parse_u32("border", dim_parts[0])?;
            let right = parse_u32("border", dim_parts[1])?;
            let bottom = parse_u32("border", dim_parts[2])?;
            let left = parse_u32("border", dim_parts[3])?;
            Ok(Border {
                color,
                top,
                right,
                bottom,
                left,
            })
        }
        _ => Err(TransformParseError::InvalidParameter {
            name: "border".to_string(),
            value: value.to_string(),
        }),
    }
}

/// Parse padding specification.
///
/// Format: `value` (uniform), `top,right,bottom,left`, or `topxrightxbottomxleft`.
/// A two-value `vertical,horizontal` shorthand is also accepted.
/// Values are fractional (0.0-1.0).
/// An optional `:color` suffix is accepted and ignored (background is handled separately).
fn parse_padding(value: &str) -> Result<Padding, TransformParseError> {
    // Strip optional color suffix
    let dims_str = if let Some((dims, _color)) = value.split_once(':') {
        dims
    } else {
        value
    };

    let parts = split_sides(dims_str);

    match parts.len() {
        1 => {
            let v = parse_f64("pad", parts[0])?;
            Ok(Padding::uniform(v))
        }
        2 => {
            let tb = parse_f64("pad", parts[0])?;
            let lr = parse_f64("pad", parts[1])?;
            Ok(Padding {
                top: tb,
                right: lr,
                bottom: tb,
                left: lr,
            })
        }
        4 => {
            let top = parse_f64("pad", parts[0])?;
            let right = parse_f64("pad", parts[1])?;
            let bottom = parse_f64("pad", parts[2])?;
            let left = parse_f64("pad", parts[3])?;
            Ok(Padding {
                top,
                right,
                bottom,
                left,
            })
        }
        _ => Err(TransformParseError::InvalidParameter {
            name: "pad".to_string(),
            value: value.to_string(),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_transform_string tests ──

    #[test]
    fn test_parse_basic() {
        let p = parse_transform_string("width=800,height=600,quality=85")
            .expect("parse basic transform");
        assert_eq!(p.width, Some(800));
        assert_eq!(p.height, Some(600));
        assert_eq!(p.quality, 85);
    }

    #[test]
    fn test_parse_format_auto() {
        let p = parse_transform_string("format=auto").expect("parse format auto");
        assert_eq!(p.format, OutputFormat::Auto);
    }

    #[test]
    fn test_parse_all_cloudflare_params() {
        let s = "width=800,height=600,quality=85,format=webp,fit=cover,gravity=center,\
                 sharpen=2.0,blur=5.0,brightness=0.1,contrast=-0.2,gamma=2.2,rotate=90,\
                 dpr=2.0,metadata=none,anim=true,compression=fast";
        let p = parse_transform_string(s).expect("parse all cloudflare params");
        assert_eq!(p.width, Some(800));
        assert_eq!(p.height, Some(600));
        assert_eq!(p.quality, 85);
        assert_eq!(p.format, OutputFormat::WebP);
        assert_eq!(p.fit, FitMode::Cover);
        assert_eq!(p.gravity, Gravity::Center);
        assert!((p.sharpen - 2.0).abs() < 0.001);
        assert!((p.blur - 5.0).abs() < 0.001);
        assert!((p.brightness - 0.1).abs() < 0.001);
        assert!((p.contrast - (-0.2)).abs() < 0.001);
        assert!((p.gamma - 2.2).abs() < 0.001);
        assert_eq!(p.rotate, Rotation::Deg90);
        assert!((p.dpr - 2.0).abs() < 0.001);
        assert_eq!(p.metadata, MetadataMode::None);
        assert!(p.anim);
        assert!(p.compression.is_some());
    }

    #[test]
    fn test_parse_short_aliases() {
        let p = parse_transform_string("w=400,h=300,q=90,f=avif").expect("parse short aliases");
        assert_eq!(p.width, Some(400));
        assert_eq!(p.height, Some(300));
        assert_eq!(p.quality, 90);
        assert_eq!(p.format, OutputFormat::Avif);
    }

    #[test]
    fn test_parse_gravity_short_alias() {
        let p = parse_transform_string("g=top-left").expect("parse gravity alias");
        assert_eq!(p.gravity, Gravity::TopLeft);
    }

    #[test]
    fn test_parse_background_short_alias() {
        let p = parse_transform_string("bg=#00ff00").expect("parse background alias");
        assert_eq!(p.background.g, 255);
    }

    #[test]
    fn test_parse_empty_string() {
        let p = parse_transform_string("").expect("parse empty string");
        assert!(p.is_identity());
    }

    #[test]
    fn test_parse_unknown_keys_ignored() {
        let p = parse_transform_string("width=100,future_param=42,height=50")
            .expect("parse with unknown keys");
        assert_eq!(p.width, Some(100));
        assert_eq!(p.height, Some(50));
    }

    #[test]
    fn test_parse_bare_key_ignored() {
        let p =
            parse_transform_string("width=100,bare_key,height=50").expect("parse with bare key");
        assert_eq!(p.width, Some(100));
        assert_eq!(p.height, Some(50));
    }

    #[test]
    fn test_parse_invalid_number() {
        let result = parse_transform_string("width=abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_format() {
        let result = parse_transform_string("format=bmp");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_border_uniform() {
        let p = parse_transform_string("border=5:ff0000").expect("parse border uniform");
        let border = p.border.expect("border should be set");
        assert_eq!(border.top, 5);
        assert_eq!(border.right, 5);
        assert_eq!(border.color.r, 255);
        assert_eq!(border.color.g, 0);
        assert_eq!(border.color.b, 0);
    }

    #[test]
    fn test_parse_border_four_sides() {
        let p = parse_query_params("border=1,2,3,4:00ff00").expect("parse four-side border");
        let border = p.border.expect("border should be set");
        assert_eq!(border.top, 1);
        assert_eq!(border.right, 2);
        assert_eq!(border.bottom, 3);
        assert_eq!(border.left, 4);
        assert_eq!(border.color.g, 255);
    }

    #[test]
    fn test_parse_padding_uniform() {
        let p = parse_query_params("pad=0.05").expect("parse uniform padding");
        let pad = p.pad.expect("pad should be set");
        assert!((pad.top - 0.05).abs() < 0.001);
        assert!((pad.left - 0.05).abs() < 0.001);
    }

    #[test]
    fn test_parse_padding_two_values() {
        let p = parse_query_params("pad=0.05,0.1").expect("parse two-value padding");
        let pad = p.pad.expect("pad should be set");
        assert!((pad.top - 0.05).abs() < 1e-9);
        assert!((pad.right - 0.1).abs() < 1e-9);
        assert!((pad.bottom - 0.05).abs() < 1e-9);
        assert!((pad.left - 0.1).abs() < 1e-9);
    }

    #[test]
    fn test_parse_padding_four_values() {
        let p = parse_query_params("pad=0.01,0.02,0.03,0.04").expect("parse four-value padding");
        let pad = p.pad.expect("pad should be set");
        assert!((pad.top - 0.01).abs() < 0.001);
        assert!((pad.right - 0.02).abs() < 0.001);
        assert!((pad.bottom - 0.03).abs() < 0.001);
        assert!((pad.left - 0.04).abs() < 0.001);
    }

    #[test]
    fn test_parse_background() {
        let p = parse_transform_string("background=#00ff00").expect("parse background color");
        assert_eq!(p.background.g, 255);
    }

    #[test]
    fn test_parse_trim_uniform() {
        let p = parse_transform_string("trim=20").expect("parse trim");
        let trim = p.trim.expect("trim should be set");
        assert_eq!(trim.top, 20);
        assert_eq!(trim.right, 20);
    }

    #[test]
    fn test_parse_onerror() {
        let p = parse_transform_string("onerror=https://example.com/fallback.jpg")
            .expect("parse onerror");
        assert_eq!(
            p.onerror,
            Some("https://example.com/fallback.jpg".to_string())
        );
    }

    #[test]
    fn test_parse_gravity_focal_point() {
        let p = parse_transform_string("gravity=0.3x0.7").expect("parse focal point gravity");
        assert!(matches!(p.gravity, Gravity::FocalPoint(_, _)));
    }

    #[test]
    fn test_parse_anim_false() {
        let p = parse_transform_string("anim=false").expect("parse anim false");
        assert!(!p.anim);
    }

    #[test]
    fn test_parse_anim_true() {
        let p = parse_transform_string("anim=true").expect("parse anim true");
        assert!(p.anim);
    }

    #[test]
    fn test_parse_rotate_auto() {
        let p = parse_transform_string("rotate=auto").expect("parse rotate auto");
        assert_eq!(p.rotate, Rotation::Auto);
    }

    #[test]
    fn test_parse_rotate_degrees() {
        let p = parse_transform_string("rotate=270").expect("parse rotate 270");
        assert_eq!(p.rotate, Rotation::Deg270);
    }

    // ── parse_cdn_url tests ──

    #[test]
    fn test_cdn_url_basic() {
        let req = parse_cdn_url("/cdn-cgi/image/width=800,height=600/images/photo.jpg")
            .expect("parse cdn url");
        assert_eq!(req.params.width, Some(800));
        assert_eq!(req.params.height, Some(600));
        assert_eq!(req.source_path, "images/photo.jpg");
    }

    #[test]
    fn test_cdn_url_complex() {
        let req = parse_cdn_url(
            "/cdn-cgi/image/width=400,quality=85,format=auto,fit=cover/uploads/2024/banner.png",
        )
        .expect("parse complex cdn url");
        assert_eq!(req.params.width, Some(400));
        assert_eq!(req.params.quality, 85);
        assert_eq!(req.params.format, OutputFormat::Auto);
        assert_eq!(req.params.fit, FitMode::Cover);
        assert_eq!(req.source_path, "uploads/2024/banner.png");
    }

    #[test]
    fn test_cdn_url_no_leading_slash() {
        let req = parse_cdn_url("cdn-cgi/image/width=100/photo.jpg")
            .expect("parse cdn url no leading slash");
        assert_eq!(req.source_path, "photo.jpg");
    }

    #[test]
    fn test_cdn_url_invalid_prefix() {
        let result = parse_cdn_url("/images/photo.jpg");
        assert!(result.is_err());
    }

    #[test]
    fn test_cdn_url_missing_image_path() {
        let result = parse_cdn_url("/cdn-cgi/image/width=800");
        assert!(result.is_err());
    }

    #[test]
    fn test_cdn_url_traversal_prevention() {
        let req = parse_cdn_url("/cdn-cgi/image/width=100/../../etc/passwd")
            .expect("parse traversal url");
        assert!(!req.source_path.contains(".."));
    }

    #[test]
    fn test_cdn_url_url_encoded() {
        let req = parse_cdn_url("/cdn-cgi/image/width=100/path%20with%20spaces/photo.jpg")
            .expect("parse url-encoded cdn url");
        assert_eq!(req.source_path, "path with spaces/photo.jpg");
    }

    #[test]
    fn test_cdn_url_no_transforms() {
        // Empty transforms but valid path
        let req = parse_cdn_url("/cdn-cgi/image//photo.jpg").expect("parse cdn url no transforms");
        assert_eq!(req.source_path, "photo.jpg");
    }

    // ── parse_cdn_cgi_path (legacy API) tests ──

    #[test]
    fn test_cdn_cgi_path_basic() {
        let (p, path) = parse_cdn_cgi_path("/cdn-cgi/image/width=800,height=600/images/photo.jpg")
            .expect("parse cdn cgi path");
        assert_eq!(p.width, Some(800));
        assert_eq!(p.height, Some(600));
        assert_eq!(path, "images/photo.jpg");
    }

    // ── parse_query_params tests ──

    #[test]
    fn test_query_basic() {
        let p = parse_query_params("width=800&height=600&quality=85").expect("parse query basic");
        assert_eq!(p.width, Some(800));
        assert_eq!(p.height, Some(600));
        assert_eq!(p.quality, 85);
    }

    #[test]
    fn test_query_empty() {
        let p = parse_query_params("").expect("parse empty query");
        assert!(p.is_identity());
    }

    #[test]
    fn test_query_aliases() {
        let p = parse_query_params("w=200&h=100&q=75&f=webp").expect("parse query aliases");
        assert_eq!(p.width, Some(200));
        assert_eq!(p.height, Some(100));
        assert_eq!(p.quality, 75);
        assert_eq!(p.format, OutputFormat::WebP);
    }

    #[test]
    fn test_query_unknown_params_ignored() {
        let p =
            parse_query_params("width=100&page=1&sort=date").expect("parse query unknown params");
        assert_eq!(p.width, Some(100));
    }

    // ── Cache key tests ──

    #[test]
    fn test_cache_key_deterministic() {
        let req = TransformRequest {
            params: {
                let mut p = TransformParams::default();
                p.width = Some(800);
                p.quality = 90;
                p
            },
            source_path: "images/test.jpg".to_string(),
        };
        let k1 = generate_cache_key(&req);
        let k2 = generate_cache_key(&req);
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_cache_key_different_params() {
        let req1 = TransformRequest {
            params: {
                let mut p = TransformParams::default();
                p.width = Some(800);
                p
            },
            source_path: "test.jpg".to_string(),
        };
        let req2 = TransformRequest {
            params: {
                let mut p = TransformParams::default();
                p.width = Some(400);
                p
            },
            source_path: "test.jpg".to_string(),
        };
        assert_ne!(generate_cache_key(&req1), generate_cache_key(&req2));
    }

    #[test]
    fn test_cache_key_different_paths() {
        let req1 = TransformRequest {
            params: {
                let mut p = TransformParams::default();
                p.width = Some(800);
                p
            },
            source_path: "a.jpg".to_string(),
        };
        let req2 = TransformRequest {
            params: {
                let mut p = TransformParams::default();
                p.width = Some(800);
                p
            },
            source_path: "b.jpg".to_string(),
        };
        assert_ne!(generate_cache_key(&req1), generate_cache_key(&req2));
    }

    #[test]
    fn test_compute_cache_key_deterministic() {
        let mut p = TransformParams::default();
        p.width = Some(800);
        p.quality = 90;
        let k1 = compute_cache_key("images/test.jpg", &p);
        let k2 = compute_cache_key("images/test.jpg", &p);
        assert_eq!(k1.transform_hash, k2.transform_hash);
    }

    #[test]
    fn test_compute_cache_key_different_params() {
        let mut p1 = TransformParams::default();
        p1.width = Some(800);
        let mut p2 = TransformParams::default();
        p2.width = Some(400);
        let k1 = compute_cache_key("test.jpg", &p1);
        let k2 = compute_cache_key("test.jpg", &p2);
        assert_ne!(k1.transform_hash, k2.transform_hash);
    }

    #[test]
    fn test_compute_cache_key_different_paths() {
        let mut p = TransformParams::default();
        p.width = Some(800);
        let k1 = compute_cache_key("a.jpg", &p);
        let k2 = compute_cache_key("b.jpg", &p);
        assert_ne!(k1.transform_hash, k2.transform_hash);
    }

    // ── Round-trip tests ──

    #[test]
    fn test_round_trip_parse_display_parse() {
        let original = "width=800,height=600,quality=90,format=webp,fit=cover";
        let params = parse_transform_string(original).expect("parse original");
        let displayed = format!("{params}");
        let reparsed = parse_transform_string(&displayed).expect("reparse displayed");
        assert_eq!(params.width, reparsed.width);
        assert_eq!(params.height, reparsed.height);
        assert_eq!(params.quality, reparsed.quality);
        assert_eq!(params.format, reparsed.format);
        assert_eq!(params.fit, reparsed.fit);
    }

    #[test]
    fn test_round_trip_with_effects() {
        let original = "blur=5,sharpen=2,brightness=0.1,contrast=-0.2,gamma=2.2";
        let params = parse_transform_string(original).expect("parse effects");
        let displayed = format!("{params}");
        let reparsed = parse_transform_string(&displayed).expect("reparse effects");
        assert!((params.blur - reparsed.blur).abs() < 0.001);
        assert!((params.sharpen - reparsed.sharpen).abs() < 0.001);
        assert!((params.brightness - reparsed.brightness).abs() < 0.001);
        assert!((params.contrast - reparsed.contrast).abs() < 0.001);
        assert!((params.gamma - reparsed.gamma).abs() < 0.001);
    }

    // ── Percent decode tests ──

    #[test]
    fn test_percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("a%2Fb"), "a/b");
        assert_eq!(percent_decode("no%encoding"), "no%encoding"); // invalid sequence preserved
    }

    #[test]
    fn test_percent_decode_empty() {
        assert_eq!(percent_decode(""), "");
    }

    // ── FNV hash tests ──

    #[test]
    fn test_fnv1a_empty() {
        let h = fnv1a_hash(b"");
        assert_eq!(h, 0xcbf2_9ce4_8422_2325);
    }

    #[test]
    fn test_fnv1a_deterministic() {
        let h1 = fnv1a_hash(b"test");
        let h2 = fnv1a_hash(b"test");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_fnv1a_different_inputs() {
        let h1 = fnv1a_hash(b"hello");
        let h2 = fnv1a_hash(b"world");
        assert_ne!(h1, h2);
    }

    // ── TransformPreset tests ──

    /// Thumbnail preset must return exactly 100×100 at quality 75.
    #[test]
    fn test_preset_thumbnail() {
        let preset = parse_preset("thumbnail").expect("thumbnail should parse");
        assert_eq!(preset, super::TransformPreset::Thumbnail);
        let params = preset.to_params();
        assert_eq!(params.width, Some(100), "thumbnail width must be 100");
        assert_eq!(params.height, Some(100), "thumbnail height must be 100");
        assert_eq!(params.quality, 75, "thumbnail quality must be 75");
    }

    #[test]
    fn test_preset_thumbnail_alias_thumb() {
        assert_eq!(
            parse_preset("thumb"),
            Some(super::TransformPreset::Thumbnail)
        );
    }

    #[test]
    fn test_preset_thumbnail_alias_tn() {
        assert_eq!(parse_preset("tn"), Some(super::TransformPreset::Thumbnail));
    }

    #[test]
    fn test_preset_preview() {
        let preset = parse_preset("preview").expect("preview should parse");
        let params = preset.to_params();
        assert_eq!(params.width, Some(800));
        assert_eq!(params.height, Some(600));
        assert_eq!(params.quality, 85);
    }

    #[test]
    fn test_preset_hd_ready() {
        let preset = parse_preset("hd_ready").expect("hd_ready should parse");
        let params = preset.to_params();
        assert_eq!(params.width, Some(1280));
        assert_eq!(params.height, Some(720));
        assert_eq!(params.quality, 90);
    }

    #[test]
    fn test_preset_hd_ready_aliases() {
        assert_eq!(
            parse_preset("hd-ready"),
            Some(super::TransformPreset::HdReady)
        );
        assert_eq!(
            parse_preset("hdready"),
            Some(super::TransformPreset::HdReady)
        );
        assert_eq!(parse_preset("hd720"), Some(super::TransformPreset::HdReady));
    }

    #[test]
    fn test_preset_case_insensitive() {
        assert_eq!(
            parse_preset("THUMBNAIL"),
            Some(super::TransformPreset::Thumbnail)
        );
        assert_eq!(
            parse_preset("PREVIEW"),
            Some(super::TransformPreset::Preview)
        );
        assert_eq!(
            parse_preset("HD_READY"),
            Some(super::TransformPreset::HdReady)
        );
    }

    #[test]
    fn test_preset_unknown_returns_none() {
        assert!(parse_preset("unknown").is_none());
        assert!(parse_preset("").is_none());
        assert!(parse_preset("4k").is_none());
    }

    #[test]
    fn test_preset_in_cdn_url() {
        let req = parse_cdn_url("/cdn-cgi/image/preset=thumbnail/photo.jpg")
            .expect("parse preset in cdn url");
        assert_eq!(req.params.width, Some(100));
        assert_eq!(req.params.height, Some(100));
        assert_eq!(req.params.quality, 75);
    }

    #[test]
    fn test_preset_overridden_by_explicit_params() {
        // Explicit width before preset — preset should NOT override it.
        let req = parse_cdn_url("/cdn-cgi/image/width=200,preset=thumbnail/photo.jpg")
            .expect("parse preset override in cdn url");
        assert_eq!(
            req.params.width,
            Some(200),
            "explicit width should win over preset"
        );
        // Height from preset is still applied because none was set.
        assert_eq!(req.params.height, Some(100));
    }

    #[test]
    fn test_preset_as_str() {
        assert_eq!(super::TransformPreset::Thumbnail.as_str(), "thumbnail");
        assert_eq!(super::TransformPreset::Preview.as_str(), "preview");
        assert_eq!(super::TransformPreset::HdReady.as_str(), "hd_ready");
    }
}
