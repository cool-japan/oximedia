// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Responsive image HTML generation.
//!
//! Generates `srcset` attribute values and full `<picture>` element HTML
//! covering multiple breakpoints and output formats (AVIF, WebP, JPEG, etc.).
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::responsive::{
//!     Breakpoint, ResponsiveImageSet, ResponsiveImageBuilder,
//! };
//! use oximedia_image_transform::transform::OutputFormat;
//!
//! let set = ResponsiveImageSet {
//!     base_url: "/cdn-cgi/image".to_string(),
//!     source_path: "photos/hero.jpg".to_string(),
//!     breakpoints: vec![
//!         Breakpoint { width: 320, media_query: None },
//!         Breakpoint { width: 768, media_query: None },
//!         Breakpoint { width: 1280, media_query: None },
//!     ],
//!     formats: vec![OutputFormat::Avif, OutputFormat::WebP, OutputFormat::Jpeg],
//!     quality: 85,
//!     sizes_hint: Some("(max-width: 768px) 100vw, 80vw".to_string()),
//!     lazy_load: true,
//!     alt_text: Some("A scenic hero image".to_string()),
//! };
//!
//! let html = set.render_picture_element();
//! assert!(html.contains("<picture>"));
//! assert!(html.contains("image/avif"));
//! assert!(html.contains("320w"));
//! ```

use crate::transform::OutputFormat;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors from responsive image generation.
#[derive(Debug, Error)]
pub enum ResponsiveError {
    /// No breakpoints were provided.
    #[error("responsive image set must have at least one breakpoint")]
    NoBreakpoints,

    /// No formats were provided.
    #[error("responsive image set must have at least one output format")]
    NoFormats,

    /// A breakpoint has a zero or invalid width.
    #[error("invalid breakpoint width: {0}")]
    InvalidBreakpointWidth(u32),
}

// ---------------------------------------------------------------------------
// Breakpoint
// ---------------------------------------------------------------------------

/// A single responsive breakpoint: a target display width and optional media query.
///
/// ```
/// use oximedia_image_transform::responsive::Breakpoint;
///
/// let bp = Breakpoint { width: 768, media_query: Some("(max-width: 768px)".to_string()) };
/// assert_eq!(bp.width, 768);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakpoint {
    /// Target image width in CSS pixels.
    pub width: u32,
    /// Optional CSS media query for `<source media="...">`.
    pub media_query: Option<String>,
}

impl Breakpoint {
    /// Create a simple breakpoint with no media query.
    pub fn simple(width: u32) -> Self {
        Self {
            width,
            media_query: None,
        }
    }

    /// Create a breakpoint with an explicit media query.
    pub fn with_media(width: u32, media_query: impl Into<String>) -> Self {
        Self {
            width,
            media_query: Some(media_query.into()),
        }
    }
}

// ---------------------------------------------------------------------------
// ResponsiveImageSet
// ---------------------------------------------------------------------------

/// A complete responsive image specification: breakpoints × formats.
///
/// Use [`render_srcset`](Self::render_srcset) to produce a plain `srcset` string,
/// or [`render_picture_element`](Self::render_picture_element) to produce a full
/// `<picture>` element with `<source>` elements per format.
#[derive(Debug, Clone)]
pub struct ResponsiveImageSet {
    /// CDN base URL prefix (e.g. `/cdn-cgi/image`).
    pub base_url: String,
    /// Source image path relative to the CDN origin.
    pub source_path: String,
    /// List of breakpoints to generate image URLs for.
    pub breakpoints: Vec<Breakpoint>,
    /// List of output formats to generate `<source>` elements for.
    /// Formats are used in order; the last one is the `<img>` fallback.
    pub formats: Vec<OutputFormat>,
    /// JPEG/WebP/AVIF quality (1–100).
    pub quality: u8,
    /// Value for the `sizes` attribute (e.g. `"(max-width: 768px) 100vw, 50vw"`).
    pub sizes_hint: Option<String>,
    /// Whether to add `loading="lazy"` to the generated `<img>`.
    pub lazy_load: bool,
    /// `alt` attribute text for the generated `<img>`.
    pub alt_text: Option<String>,
}

impl ResponsiveImageSet {
    /// Validate the set's configuration.
    pub fn validate(&self) -> Result<(), ResponsiveError> {
        if self.breakpoints.is_empty() {
            return Err(ResponsiveError::NoBreakpoints);
        }
        if self.formats.is_empty() {
            return Err(ResponsiveError::NoFormats);
        }
        for bp in &self.breakpoints {
            if bp.width == 0 {
                return Err(ResponsiveError::InvalidBreakpointWidth(bp.width));
            }
        }
        Ok(())
    }

    /// Generate a `srcset` string for one format.
    ///
    /// ```
    /// use oximedia_image_transform::responsive::{Breakpoint, ResponsiveImageSet};
    /// use oximedia_image_transform::transform::OutputFormat;
    ///
    /// let set = ResponsiveImageSet {
    ///     base_url: "/img".to_string(),
    ///     source_path: "photo.jpg".to_string(),
    ///     breakpoints: vec![Breakpoint::simple(400), Breakpoint::simple(800)],
    ///     formats: vec![OutputFormat::WebP],
    ///     quality: 85,
    ///     sizes_hint: None,
    ///     lazy_load: false,
    ///     alt_text: None,
    /// };
    ///
    /// let srcset = set.render_srcset(OutputFormat::WebP);
    /// assert!(srcset.contains("400w"));
    /// assert!(srcset.contains("800w"));
    /// ```
    pub fn render_srcset(&self, format: OutputFormat) -> String {
        let mut entries: Vec<String> = self
            .breakpoints
            .iter()
            .map(|bp| {
                let url = self.build_url(bp.width, format);
                format!("{url} {w}w", w = bp.width)
            })
            .collect();
        entries.sort(); // stable ordering by string (width prefix)
        entries.join(", ")
    }

    /// Generate a full `<picture>` element HTML string.
    ///
    /// Structure:
    /// ```html
    /// <picture>
    ///   <source type="image/avif" srcset="..." sizes="...">
    ///   <source type="image/webp" srcset="..." sizes="...">
    ///   <img src="..." srcset="..." sizes="..." alt="..." loading="lazy">
    /// </picture>
    /// ```
    pub fn render_picture_element(&self) -> String {
        let mut html = String::from("<picture>\n");

        // <source> elements for all formats except the last (which is the <img> fallback)
        let source_formats = if self.formats.len() > 1 {
            &self.formats[..self.formats.len() - 1]
        } else {
            &self.formats[..0] // empty slice — single format uses <img> only
        };

        for &fmt in source_formats {
            html.push_str(&self.render_source_element(fmt));
        }

        // <img> fallback uses last format
        let fallback_fmt = *self.formats.last().unwrap_or(&OutputFormat::Jpeg);
        html.push_str(&self.render_img_element(fallback_fmt));

        html.push_str("</picture>");
        html
    }

    /// Render a single `<source>` element for the given format.
    fn render_source_element(&self, format: OutputFormat) -> String {
        let mime = format.mime_type();
        let srcset = self.render_srcset(format);
        let sizes_attr = self
            .sizes_hint
            .as_deref()
            .map(|s| format!(" sizes=\"{s}\""))
            .unwrap_or_default();

        // Add media attribute from breakpoints if all have the same media query
        // (simplified: use the first breakpoint's media query if present)
        let media_attr = self
            .breakpoints
            .first()
            .and_then(|bp| bp.media_query.as_deref())
            .map(|mq| format!(" media=\"{mq}\""))
            .unwrap_or_default();

        format!("  <source type=\"{mime}\"{media_attr} srcset=\"{srcset}\"{sizes_attr}>\n")
    }

    /// Render the `<img>` fallback element.
    fn render_img_element(&self, format: OutputFormat) -> String {
        // Default src uses the largest breakpoint
        let largest_bp = self
            .breakpoints
            .iter()
            .max_by_key(|bp| bp.width)
            .map(|bp| bp.width)
            .unwrap_or(800);

        let src = self.build_url(largest_bp, format);
        let srcset = self.render_srcset(format);

        let sizes_attr = self
            .sizes_hint
            .as_deref()
            .map(|s| format!(" sizes=\"{s}\""))
            .unwrap_or_default();

        let alt_attr = self
            .alt_text
            .as_deref()
            .map(|a| format!(" alt=\"{a}\""))
            .unwrap_or_else(|| " alt=\"\"".to_string());

        let loading_attr = if self.lazy_load {
            " loading=\"lazy\""
        } else {
            ""
        };

        format!("  <img src=\"{src}\" srcset=\"{srcset}\"{sizes_attr}{alt_attr}{loading_attr}>\n")
    }

    /// Build a CDN URL for one breakpoint and format combination.
    fn build_url(&self, width: u32, format: OutputFormat) -> String {
        let fmt_str = format.as_str();
        let q = self.quality;
        format!(
            "{base}/w={width},q={q},f={fmt_str}/{path}",
            base = self.base_url.trim_end_matches('/'),
            path = self.source_path.trim_start_matches('/'),
        )
    }
}

// ---------------------------------------------------------------------------
// ResponsiveImageBuilder — fluent builder API
// ---------------------------------------------------------------------------

/// Fluent builder for [`ResponsiveImageSet`].
///
/// ```
/// use oximedia_image_transform::responsive::ResponsiveImageBuilder;
/// use oximedia_image_transform::transform::OutputFormat;
///
/// let set = ResponsiveImageBuilder::new("/cdn", "photo.jpg")
///     .breakpoints(vec![320, 768, 1280])
///     .formats(vec![OutputFormat::Avif, OutputFormat::Jpeg])
///     .quality(80)
///     .sizes("(max-width: 768px) 100vw, 50vw")
///     .lazy()
///     .alt("A photo")
///     .build();
///
/// let html = set.render_picture_element();
/// assert!(html.contains("image/avif"));
/// assert!(html.contains("loading=\"lazy\""));
/// ```
#[derive(Debug)]
pub struct ResponsiveImageBuilder {
    base_url: String,
    source_path: String,
    breakpoints: Vec<Breakpoint>,
    formats: Vec<OutputFormat>,
    quality: u8,
    sizes_hint: Option<String>,
    lazy_load: bool,
    alt_text: Option<String>,
}

impl ResponsiveImageBuilder {
    /// Create a new builder with default settings.
    pub fn new(base_url: impl Into<String>, source_path: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            source_path: source_path.into(),
            breakpoints: Vec::new(),
            formats: vec![OutputFormat::Jpeg],
            quality: 85,
            sizes_hint: None,
            lazy_load: false,
            alt_text: None,
        }
    }

    /// Set breakpoint widths (plain pixel values, no media queries).
    pub fn breakpoints(mut self, widths: Vec<u32>) -> Self {
        self.breakpoints = widths.into_iter().map(Breakpoint::simple).collect();
        self
    }

    /// Set breakpoints with explicit [`Breakpoint`] structs.
    pub fn with_breakpoints(mut self, breakpoints: Vec<Breakpoint>) -> Self {
        self.breakpoints = breakpoints;
        self
    }

    /// Set the output formats (order matters: first is most preferred).
    pub fn formats(mut self, formats: Vec<OutputFormat>) -> Self {
        self.formats = formats;
        self
    }

    /// Set the quality (1-100).
    pub fn quality(mut self, q: u8) -> Self {
        self.quality = q;
        self
    }

    /// Set the `sizes` attribute hint.
    pub fn sizes(mut self, sizes: impl Into<String>) -> Self {
        self.sizes_hint = Some(sizes.into());
        self
    }

    /// Enable `loading="lazy"` on the `<img>` element.
    pub fn lazy(mut self) -> Self {
        self.lazy_load = true;
        self
    }

    /// Set the `alt` text.
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.alt_text = Some(alt.into());
        self
    }

    /// Build the [`ResponsiveImageSet`].
    pub fn build(self) -> ResponsiveImageSet {
        ResponsiveImageSet {
            base_url: self.base_url,
            source_path: self.source_path,
            breakpoints: self.breakpoints,
            formats: self.formats,
            quality: self.quality,
            sizes_hint: self.sizes_hint,
            lazy_load: self.lazy_load,
            alt_text: self.alt_text,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_set() -> ResponsiveImageSet {
        ResponsiveImageSet {
            base_url: "/img".to_string(),
            source_path: "photo.jpg".to_string(),
            breakpoints: vec![
                Breakpoint::simple(320),
                Breakpoint::simple(768),
                Breakpoint::simple(1280),
            ],
            formats: vec![OutputFormat::Avif, OutputFormat::WebP, OutputFormat::Jpeg],
            quality: 85,
            sizes_hint: Some("(max-width: 768px) 100vw, 80vw".to_string()),
            lazy_load: true,
            alt_text: Some("A test image".to_string()),
        }
    }

    #[test]
    fn test_srcset_contains_all_widths() {
        let set = make_set();
        let srcset = set.render_srcset(OutputFormat::WebP);
        assert!(srcset.contains("320w"));
        assert!(srcset.contains("768w"));
        assert!(srcset.contains("1280w"));
    }

    #[test]
    fn test_srcset_contains_format() {
        let set = make_set();
        let srcset = set.render_srcset(OutputFormat::Avif);
        assert!(srcset.contains("f=avif"));
    }

    #[test]
    fn test_picture_element_structure() {
        let set = make_set();
        let html = set.render_picture_element();
        assert!(html.starts_with("<picture>"));
        assert!(html.ends_with("</picture>"));
        assert!(html.contains("<source"));
        assert!(html.contains("<img"));
    }

    #[test]
    fn test_picture_element_has_avif_source() {
        let set = make_set();
        let html = set.render_picture_element();
        assert!(html.contains("image/avif"));
    }

    #[test]
    fn test_picture_element_has_webp_source() {
        let set = make_set();
        let html = set.render_picture_element();
        assert!(html.contains("image/webp"));
    }

    #[test]
    fn test_picture_element_lazy_loading() {
        let set = make_set();
        let html = set.render_picture_element();
        assert!(html.contains("loading=\"lazy\""));
    }

    #[test]
    fn test_picture_element_alt_text() {
        let set = make_set();
        let html = set.render_picture_element();
        assert!(html.contains("alt=\"A test image\""));
    }

    #[test]
    fn test_picture_element_sizes_attr() {
        let set = make_set();
        let html = set.render_picture_element();
        assert!(html.contains("sizes="));
        assert!(html.contains("100vw"));
    }

    #[test]
    fn test_builder_fluent_api() {
        let set = ResponsiveImageBuilder::new("/cdn-cgi/image", "banner.png")
            .breakpoints(vec![480, 960, 1440])
            .formats(vec![OutputFormat::Avif, OutputFormat::Jpeg])
            .quality(80)
            .sizes("100vw")
            .lazy()
            .alt("Banner")
            .build();

        assert_eq!(set.breakpoints.len(), 3);
        assert_eq!(set.quality, 80);
        assert!(set.lazy_load);
        assert_eq!(set.alt_text.as_deref(), Some("Banner"));

        let html = set.render_picture_element();
        assert!(html.contains("480w"));
        assert!(html.contains("960w"));
        assert!(html.contains("1440w"));
        assert!(html.contains("image/avif"));
    }

    #[test]
    fn test_validate_empty_breakpoints() {
        let mut set = make_set();
        set.breakpoints.clear();
        assert!(matches!(
            set.validate(),
            Err(ResponsiveError::NoBreakpoints)
        ));
    }

    #[test]
    fn test_validate_empty_formats() {
        let mut set = make_set();
        set.formats.clear();
        assert!(matches!(set.validate(), Err(ResponsiveError::NoFormats)));
    }

    #[test]
    fn test_validate_zero_width_breakpoint() {
        let mut set = make_set();
        set.breakpoints.push(Breakpoint::simple(0));
        assert!(matches!(
            set.validate(),
            Err(ResponsiveError::InvalidBreakpointWidth(0))
        ));
    }

    #[test]
    fn test_validate_ok() {
        let set = make_set();
        assert!(set.validate().is_ok());
    }

    #[test]
    fn test_single_format_no_source_elements() {
        let set = ResponsiveImageSet {
            base_url: "/img".to_string(),
            source_path: "photo.jpg".to_string(),
            breakpoints: vec![Breakpoint::simple(800)],
            formats: vec![OutputFormat::Jpeg],
            quality: 85,
            sizes_hint: None,
            lazy_load: false,
            alt_text: None,
        };
        let html = set.render_picture_element();
        // With single format, <source> elements are not emitted
        assert!(!html.contains("<source"));
        assert!(html.contains("<img"));
    }

    #[test]
    fn test_breakpoint_with_media_query() {
        let bp = Breakpoint::with_media(768, "(max-width: 768px)");
        assert_eq!(bp.media_query.as_deref(), Some("(max-width: 768px)"));
    }

    #[test]
    fn test_url_format() {
        let set = ResponsiveImageSet {
            base_url: "/cdn-cgi/image".to_string(),
            source_path: "test.jpg".to_string(),
            breakpoints: vec![Breakpoint::simple(800)],
            formats: vec![OutputFormat::WebP],
            quality: 90,
            sizes_hint: None,
            lazy_load: false,
            alt_text: None,
        };
        let srcset = set.render_srcset(OutputFormat::WebP);
        assert!(srcset.contains("/cdn-cgi/image/w=800,q=90,f=webp/test.jpg"));
    }
}
