// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Batch image transform: process multiple output variants in a single request.
//!
//! Variant processing is parallelised with `rayon`: each [`TransformVariant`]
//! is validated and processed on a rayon thread-pool worker, so multiple
//! variants are handled concurrently.
//!
//! A [`BatchTransformRequest`] bundles a source image ID with a list of
//! [`TransformVariant`] descriptors (width, height, format, quality, suffix).
//! [`BatchTransformer::process_batch`] iterates over all variants, validates
//! each one, and returns a [`BatchTransformResult`] containing per-variant
//! [`TransformOutput`] entries.  Individual variant failures are captured in
//! the output rather than aborting the whole batch.
//!
//! # Example
//!
//! ```
//! use oximedia_image_transform::batch_transform::{
//!     BatchTransformer, BatchTransformRequest, TransformVariant,
//! };
//!
//! let request = BatchTransformRequest {
//!     source_id: "uploads/photo.jpg".to_string(),
//!     variants: vec![
//!         TransformVariant { width: 320, height: 240, format: "webp".to_string(), quality: 80, suffix: "thumb".to_string() },
//!         TransformVariant { width: 1280, height: 720, format: "jpeg".to_string(), quality: 85, suffix: "hd".to_string() },
//!     ],
//! };
//!
//! let transformer = BatchTransformer::new();
//! let result = transformer.process_batch(&request);
//!
//! assert_eq!(result.source_id, "uploads/photo.jpg");
//! assert_eq!(result.outputs.len(), 2);
//! assert!(result.outputs[0].success);
//! ```

use rayon::prelude::*;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// TransformVariant
// ---------------------------------------------------------------------------

/// A single output variant specification within a batch request.
///
/// Each variant describes the desired dimensions, format, quality, and a
/// unique suffix used when naming the output (e.g. `"thumb"` → `photo_thumb.webp`).
#[derive(Debug, Clone, PartialEq)]
pub struct TransformVariant {
    /// Target output width in pixels (must be 1-12 000).
    pub width: u32,
    /// Target output height in pixels (must be 1-12 000).
    pub height: u32,
    /// Output format string: `"jpeg"`, `"webp"`, `"avif"`, `"png"`, `"gif"`.
    pub format: String,
    /// Quality setting 1-100.
    pub quality: u8,
    /// Short suffix used to derive the output path/name for this variant.
    pub suffix: String,
}

impl TransformVariant {
    /// Validate the variant's field values.
    ///
    /// Returns `Err(String)` with a descriptive message on the first
    /// constraint violation.
    pub fn validate(&self) -> Result<(), String> {
        const MAX_DIM: u32 = 12_000;

        if self.width == 0 || self.width > MAX_DIM {
            return Err(format!(
                "variant '{}': width {} is out of range 1-{MAX_DIM}",
                self.suffix, self.width
            ));
        }

        if self.height == 0 || self.height > MAX_DIM {
            return Err(format!(
                "variant '{}': height {} is out of range 1-{MAX_DIM}",
                self.suffix, self.height
            ));
        }

        if self.quality == 0 || self.quality > 100 {
            return Err(format!(
                "variant '{}': quality {} is out of range 1-100",
                self.suffix, self.quality
            ));
        }

        if !is_known_format(&self.format) {
            return Err(format!(
                "variant '{}': unknown format '{}'",
                self.suffix, self.format
            ));
        }

        if self.suffix.is_empty() {
            return Err("variant suffix must not be empty".to_string());
        }

        Ok(())
    }

    /// Build the output filename/path for a given source base name.
    ///
    /// ```
    /// use oximedia_image_transform::batch_transform::TransformVariant;
    ///
    /// let v = TransformVariant {
    ///     width: 320, height: 240, format: "webp".to_string(),
    ///     quality: 80, suffix: "thumb".to_string(),
    /// };
    /// assert_eq!(v.output_path("photo"), "photo_thumb.webp");
    /// ```
    pub fn output_path(&self, base: &str) -> String {
        format!("{}_{}.{}", base, self.suffix, self.format)
    }
}

// ---------------------------------------------------------------------------
// BatchTransformRequest
// ---------------------------------------------------------------------------

/// A batch request: one source image → multiple output variants.
///
/// ```
/// use oximedia_image_transform::batch_transform::{BatchTransformRequest, TransformVariant};
///
/// let req = BatchTransformRequest {
///     source_id: "images/hero.jpg".to_string(),
///     variants: vec![
///         TransformVariant { width: 800, height: 600, format: "jpeg".to_string(), quality: 85, suffix: "large".to_string() },
///     ],
/// };
/// assert_eq!(req.variants.len(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct BatchTransformRequest {
    /// Identifier for the source image (path, UUID, or URL).
    pub source_id: String,
    /// List of output variants to produce from the source.
    pub variants: Vec<TransformVariant>,
}

// ---------------------------------------------------------------------------
// TransformOutput
// ---------------------------------------------------------------------------

/// The result of processing one [`TransformVariant`] from a batch.
#[derive(Debug, Clone)]
pub struct TransformOutput {
    /// The variant that was processed.
    pub variant: TransformVariant,
    /// Whether this variant was processed successfully.
    pub success: bool,
    /// Error message if `success` is `false`.
    pub error: Option<String>,
    /// Output path derived from the source base name and variant suffix.
    pub output_path: String,
}

// ---------------------------------------------------------------------------
// BatchTransformResult
// ---------------------------------------------------------------------------

/// The result of a complete batch transform.
#[derive(Debug, Clone)]
pub struct BatchTransformResult {
    /// Source image identifier (echoed from the request).
    pub source_id: String,
    /// Per-variant outputs, in the same order as the request's `variants`.
    pub outputs: Vec<TransformOutput>,
}

impl BatchTransformResult {
    /// Count of successfully processed variants.
    pub fn success_count(&self) -> usize {
        self.outputs.iter().filter(|o| o.success).count()
    }

    /// Count of failed variants.
    pub fn failure_count(&self) -> usize {
        self.outputs.iter().filter(|o| !o.success).count()
    }

    /// Returns `true` if all variants succeeded.
    pub fn all_succeeded(&self) -> bool {
        self.outputs.iter().all(|o| o.success)
    }

    /// Returns `true` if at least one variant failed.
    pub fn has_failures(&self) -> bool {
        self.outputs.iter().any(|o| !o.success)
    }

    /// Collect all successful output paths.
    pub fn successful_paths(&self) -> Vec<&str> {
        self.outputs
            .iter()
            .filter(|o| o.success)
            .map(|o| o.output_path.as_str())
            .collect()
    }

    /// Collect all error messages from failed variants.
    pub fn errors(&self) -> Vec<(&TransformVariant, &str)> {
        self.outputs
            .iter()
            .filter(|o| !o.success)
            .filter_map(|o| o.error.as_deref().map(|e| (&o.variant, e)))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// BatchTransformer
// ---------------------------------------------------------------------------

/// Executes batch transform requests.
///
/// `BatchTransformer` is intentionally stateless in its current implementation:
/// it validates each variant and simulates successful transform processing
/// (no actual pixel I/O is performed here — that is the responsibility of
/// the caller or a higher-level pipeline stage).  Validation errors are
/// captured per-variant so that a single bad variant does not abort the batch.
///
/// # Example
///
/// ```
/// use oximedia_image_transform::batch_transform::{BatchTransformer, BatchTransformRequest, TransformVariant};
///
/// let transformer = BatchTransformer::new();
/// let request = BatchTransformRequest {
///     source_id: "test/image.png".to_string(),
///     variants: vec![
///         TransformVariant { width: 100, height: 100, format: "webp".to_string(), quality: 75, suffix: "sm".to_string() },
///         TransformVariant { width: 0, height: 100, format: "webp".to_string(), quality: 75, suffix: "bad".to_string() },
///     ],
/// };
///
/// let result = transformer.process_batch(&request);
/// assert_eq!(result.success_count(), 1);
/// assert_eq!(result.failure_count(), 1);
/// ```
#[derive(Debug, Default)]
pub struct BatchTransformer {
    /// Optional per-format quality overrides applied during variant processing.
    quality_overrides: HashMap<String, u8>,
}

impl BatchTransformer {
    /// Create a new `BatchTransformer` with no overrides.
    pub fn new() -> Self {
        Self {
            quality_overrides: HashMap::new(),
        }
    }

    /// Register a quality override for a specific format.
    ///
    /// When processing a variant whose format matches `format`, the variant's
    /// quality is replaced with `quality`.
    pub fn with_quality_override(mut self, format: impl Into<String>, quality: u8) -> Self {
        self.quality_overrides.insert(format.into(), quality);
        self
    }

    /// Process all variants in the request, collecting per-variant results.
    ///
    /// Validation failures are recorded in `TransformOutput::error` with
    /// `success = false`; they do not abort processing of remaining variants.
    pub fn process_batch(&self, request: &BatchTransformRequest) -> BatchTransformResult {
        let source_base = derive_base_name(&request.source_id);

        let outputs: Vec<TransformOutput> = request
            .variants
            .par_iter()
            .map(|variant| self.process_variant(variant, &source_base))
            .collect();

        BatchTransformResult {
            source_id: request.source_id.clone(),
            outputs,
        }
    }

    /// Process a single variant.
    fn process_variant(&self, variant: &TransformVariant, source_base: &str) -> TransformOutput {
        // Apply quality override if configured.
        let effective_variant =
            if let Some(&override_q) = self.quality_overrides.get(&variant.format) {
                let mut v = variant.clone();
                v.quality = override_q;
                v
            } else {
                variant.clone()
            };

        // Validate before processing.
        if let Err(msg) = effective_variant.validate() {
            return TransformOutput {
                variant: variant.clone(),
                success: false,
                error: Some(msg),
                output_path: String::new(),
            };
        }

        let output_path = effective_variant.output_path(source_base);

        // Simulate a successful transform (actual pixel processing is external).
        TransformOutput {
            variant: effective_variant,
            success: true,
            error: None,
            output_path,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Derive a base name from a source identifier.
///
/// Strips directory prefixes and file extension.
/// `"images/hero.jpg"` → `"hero"`.
fn derive_base_name(source_id: &str) -> String {
    let without_dir = source_id.rsplit('/').next().unwrap_or(source_id);
    if let Some(pos) = without_dir.rfind('.') {
        without_dir[..pos].to_string()
    } else {
        without_dir.to_string()
    }
}

/// Returns `true` if `format` is one of the known output format strings.
fn is_known_format(format: &str) -> bool {
    matches!(
        format.to_ascii_lowercase().as_str(),
        "jpeg" | "jpg" | "webp" | "avif" | "png" | "gif" | "baseline" | "json"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn good_variant(suffix: &str) -> TransformVariant {
        TransformVariant {
            width: 800,
            height: 600,
            format: "webp".to_string(),
            quality: 85,
            suffix: suffix.to_string(),
        }
    }

    fn bad_variant_zero_width(suffix: &str) -> TransformVariant {
        TransformVariant {
            width: 0,
            height: 600,
            format: "webp".to_string(),
            quality: 85,
            suffix: suffix.to_string(),
        }
    }

    // ── TransformVariant ──

    #[test]
    fn test_variant_validate_ok() {
        assert!(good_variant("thumb").validate().is_ok());
    }

    #[test]
    fn test_variant_validate_zero_width() {
        let v = bad_variant_zero_width("bad");
        assert!(v.validate().is_err());
    }

    #[test]
    fn test_variant_validate_quality_out_of_range() {
        let mut v = good_variant("q");
        v.quality = 0;
        assert!(v.validate().is_err());
        v.quality = 101;
        assert!(v.validate().is_err());
    }

    #[test]
    fn test_variant_validate_unknown_format() {
        let mut v = good_variant("fmt");
        v.format = "bmp".to_string();
        assert!(v.validate().is_err());
    }

    #[test]
    fn test_variant_validate_empty_suffix() {
        let mut v = good_variant("ok");
        v.suffix = String::new();
        assert!(v.validate().is_err());
    }

    #[test]
    fn test_variant_output_path() {
        let v = good_variant("hd");
        assert_eq!(v.output_path("photo"), "photo_hd.webp");
    }

    #[test]
    fn test_variant_all_known_formats() {
        for fmt in &[
            "jpeg", "jpg", "webp", "avif", "png", "gif", "baseline", "json",
        ] {
            let mut v = good_variant("test");
            v.format = fmt.to_string();
            assert!(v.validate().is_ok(), "format {fmt} should be valid");
        }
    }

    // ── BatchTransformer ──

    #[test]
    fn test_process_batch_all_success() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "images/hero.jpg".to_string(),
            variants: vec![good_variant("sm"), good_variant("lg")],
        };
        let result = transformer.process_batch(&request);
        assert_eq!(result.success_count(), 2);
        assert_eq!(result.failure_count(), 0);
        assert!(result.all_succeeded());
    }

    #[test]
    fn test_process_batch_partial_failure() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "photo.png".to_string(),
            variants: vec![good_variant("ok"), bad_variant_zero_width("fail")],
        };
        let result = transformer.process_batch(&request);
        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 1);
        assert!(result.has_failures());
    }

    #[test]
    fn test_process_batch_source_id_echoed() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "my/unique/image.avif".to_string(),
            variants: vec![],
        };
        let result = transformer.process_batch(&request);
        assert_eq!(result.source_id, "my/unique/image.avif");
    }

    #[test]
    fn test_process_batch_output_path_derived() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "uploads/banner.jpg".to_string(),
            variants: vec![good_variant("thumb")],
        };
        let result = transformer.process_batch(&request);
        assert_eq!(result.outputs[0].output_path, "banner_thumb.webp");
    }

    #[test]
    fn test_process_batch_error_message_on_failure() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "img.jpg".to_string(),
            variants: vec![bad_variant_zero_width("fail")],
        };
        let result = transformer.process_batch(&request);
        assert!(!result.outputs[0].success);
        assert!(result.outputs[0].error.is_some());
    }

    #[test]
    fn test_process_batch_does_not_abort_on_failure() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "img.jpg".to_string(),
            variants: vec![
                bad_variant_zero_width("bad1"),
                good_variant("good"),
                bad_variant_zero_width("bad2"),
            ],
        };
        let result = transformer.process_batch(&request);
        assert_eq!(result.outputs.len(), 3);
        assert_eq!(result.success_count(), 1);
        assert_eq!(result.failure_count(), 2);
    }

    #[test]
    fn test_quality_override() {
        let transformer = BatchTransformer::new().with_quality_override("webp", 60);
        let request = BatchTransformRequest {
            source_id: "img.jpg".to_string(),
            variants: vec![good_variant("sm")],
        };
        let result = transformer.process_batch(&request);
        assert!(result.outputs[0].success);
        assert_eq!(result.outputs[0].variant.quality, 60);
    }

    #[test]
    fn test_successful_paths() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "photo.jpg".to_string(),
            variants: vec![
                good_variant("a"),
                bad_variant_zero_width("b"),
                good_variant("c"),
            ],
        };
        let result = transformer.process_batch(&request);
        let paths = result.successful_paths();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_derive_base_name() {
        assert_eq!(derive_base_name("images/hero.jpg"), "hero");
        assert_eq!(derive_base_name("photo.png"), "photo");
        assert_eq!(derive_base_name("no_extension"), "no_extension");
        assert_eq!(derive_base_name("a/b/c/d.avif"), "d");
    }

    // ── BatchTransformResult ──

    #[test]
    fn test_result_errors_helper() {
        let transformer = BatchTransformer::new();
        let request = BatchTransformRequest {
            source_id: "x.jpg".to_string(),
            variants: vec![bad_variant_zero_width("fail")],
        };
        let result = transformer.process_batch(&request);
        let errors = result.errors();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].1.contains("width"));
    }
}
