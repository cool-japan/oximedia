// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Cloudflare Images-compatible URL image transformation.
//!
//! This crate provides a complete implementation of Cloudflare Images'
//! URL-based image transformation API, including:
//!
//! - **Parameter types** ([`transform`]) — strongly-typed structs and enums for
//!   every supported transformation option (resize, crop, quality, format,
//!   effects, rotation, etc.).
//! - **URL parsing** ([`parser`]) — parse `/cdn-cgi/image/` paths, query strings,
//!   and comma-separated transform strings into [`TransformParams`].
//! - **Content negotiation** ([`negotiation`]) — Accept-header-based format
//!   selection (AVIF > WebP > JPEG/PNG) and HTTP response header generation.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::parser::parse_cdn_url;
//! use oximedia_image_transform::transform::{OutputFormat, FitMode};
//!
//! let req = parse_cdn_url("/cdn-cgi/image/w=800,f=webp,fit=cover/photo.jpg").expect("parse cdn url");
//! assert_eq!(req.params.width, Some(800));
//! assert_eq!(req.params.format, OutputFormat::WebP);
//! assert_eq!(req.params.fit, FitMode::Cover);
//! assert_eq!(req.source_path, "photo.jpg");
//! ```

#![warn(missing_docs)]

pub mod animation;
pub mod batch_transform;
pub mod blur_region;
pub mod compose;
pub mod face_detect;
pub mod image_analysis;
pub mod inverse;
pub mod metrics;
pub mod negotiation;
pub mod origin_fetch;
pub mod parser;
pub mod processor;
pub mod quality;
pub mod response_cache;
pub mod responsive;
pub mod security;
pub mod transform;
pub mod transform_types;
pub mod watermark;

// Re-export key types at crate root for convenience.
pub use blur_region::{
    apply_blur_regions, BlurRegion, BlurRegionError, BlurRegionProcessor, BoxBlurRegion,
};
pub use face_detect::{DetectionConfig, DetectionResult};
pub use negotiation::{generate_cache_control, ClientHints};
pub use parser::{
    parse_cdn_url, parse_preset, parse_query_params, parse_transform_string, TransformPreset,
    TransformRequest,
};
pub use quality::ComplexityAnalysis;
pub use security::{SignedUrlConfig, SignedUrlError};
pub use transform::{
    Border, Color, Compression, FitMode, Gravity, MetadataMode, OutputFormat, OutputOptions,
    Padding, Rotation, TransformParams, TransformParseError, Trim,
};
pub use watermark::{WatermarkConfig, WatermarkPosition};
