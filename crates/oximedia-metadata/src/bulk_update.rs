//! Bulk metadata update: batch field updates, validation pipeline, and rollback support.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::{Metadata, MetadataFormat, MetadataValue};

/// A single field update operation.
#[derive(Debug, Clone)]
pub struct FieldUpdate {
    /// Field key to update.
    pub key: String,
    /// New value to set. `None` means the field should be removed.
    pub value: Option<String>,
}

impl FieldUpdate {
    /// Create an update that sets a field value.
    #[must_use]
    pub fn set(key: &str, value: &str) -> Self {
        Self {
            key: key.to_string(),
            value: Some(value.to_string()),
        }
    }

    /// Create an update that removes a field.
    #[must_use]
    pub fn remove(key: &str) -> Self {
        Self {
            key: key.to_string(),
            value: None,
        }
    }

    /// Return whether this update removes a field.
    #[must_use]
    pub fn is_removal(&self) -> bool {
        self.value.is_none()
    }
}

/// Validation error returned by the validation pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    /// Field that failed validation.
    pub field: String,
    /// Human-readable error message.
    pub message: String,
}

impl ValidationError {
    /// Create a new validation error.
    #[must_use]
    pub fn new(field: &str, message: &str) -> Self {
        Self {
            field: field.to_string(),
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "field '{}': {}", self.field, self.message)
    }
}

/// A validator function signature: takes field key + value, returns optional error message.
pub type ValidatorFn = Box<dyn Fn(&str, &str) -> Option<String> + Send + Sync>;

/// Validation pipeline that applies a series of validators to field updates.
pub struct ValidationPipeline {
    validators: Vec<ValidatorFn>,
}

impl ValidationPipeline {
    /// Create a new empty pipeline.
    #[must_use]
    pub fn new() -> Self {
        Self {
            validators: Vec::new(),
        }
    }

    /// Add a validator to the pipeline.
    pub fn add_validator<F>(&mut self, validator: F)
    where
        F: Fn(&str, &str) -> Option<String> + Send + Sync + 'static,
    {
        self.validators.push(Box::new(validator));
    }

    /// Run all validators against a field update.
    /// Returns all errors found (empty = valid).
    #[must_use]
    pub fn validate_update(&self, update: &FieldUpdate) -> Vec<ValidationError> {
        if let Some(ref value) = update.value {
            self.validators
                .iter()
                .filter_map(|v| v(&update.key, value))
                .map(|msg| ValidationError::new(&update.key, &msg))
                .collect()
        } else {
            // Removals are always valid in this pipeline
            Vec::new()
        }
    }

    /// Run validators against all updates in a batch.
    /// Returns a map from field key to its errors.
    #[must_use]
    pub fn validate_batch(&self, updates: &[FieldUpdate]) -> HashMap<String, Vec<ValidationError>> {
        let mut result: HashMap<String, Vec<ValidationError>> = HashMap::new();
        for update in updates {
            let errors = self.validate_update(update);
            if !errors.is_empty() {
                result.entry(update.key.clone()).or_default().extend(errors);
            }
        }
        result
    }

    /// Return the number of registered validators.
    #[must_use]
    pub fn validator_count(&self) -> usize {
        self.validators.len()
    }
}

impl Default for ValidationPipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of applying a bulk update.
#[derive(Debug, Clone)]
pub struct BulkUpdateResult {
    /// Number of fields successfully set.
    pub fields_set: usize,
    /// Number of fields removed.
    pub fields_removed: usize,
    /// Number of fields skipped due to validation errors.
    pub fields_skipped: usize,
    /// Collected validation errors (keyed by field name).
    pub errors: HashMap<String, Vec<String>>,
}

impl BulkUpdateResult {
    /// Return `true` if no validation errors occurred.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
    }

    /// Return the total number of updates applied (sets + removals).
    #[must_use]
    pub fn total_applied(&self) -> usize {
        self.fields_set + self.fields_removed
    }
}

/// Snapshot of metadata state used for rollback.
#[derive(Debug, Clone)]
pub struct MetadataSnapshot {
    data: HashMap<String, String>,
}

impl MetadataSnapshot {
    /// Create a snapshot from an existing data map.
    #[must_use]
    pub fn from_map(data: &HashMap<String, String>) -> Self {
        Self { data: data.clone() }
    }

    /// Restore the snapshot into a mutable data map.
    pub fn restore(&self, target: &mut HashMap<String, String>) {
        *target = self.data.clone();
    }

    /// Return the number of fields in the snapshot.
    #[must_use]
    pub fn field_count(&self) -> usize {
        self.data.len()
    }
}

/// Engine for applying bulk metadata updates with optional validation and rollback.
pub struct BulkUpdateEngine {
    /// Current metadata fields.
    data: HashMap<String, String>,
    /// Validation pipeline (optional).
    pipeline: Option<ValidationPipeline>,
    /// History of snapshots for rollback (newest last).
    history: Vec<MetadataSnapshot>,
    /// Maximum number of snapshots to retain.
    max_history: usize,
}

impl BulkUpdateEngine {
    /// Create a new engine with empty metadata.
    #[must_use]
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            pipeline: None,
            history: Vec::new(),
            max_history: 10,
        }
    }

    /// Create an engine pre-populated with existing data.
    #[must_use]
    pub fn with_data(data: HashMap<String, String>) -> Self {
        Self {
            data,
            pipeline: None,
            history: Vec::new(),
            max_history: 10,
        }
    }

    /// Attach a validation pipeline.
    pub fn set_pipeline(&mut self, pipeline: ValidationPipeline) {
        self.pipeline = Some(pipeline);
    }

    /// Set maximum number of undo snapshots to retain.
    pub fn set_max_history(&mut self, max: usize) {
        self.max_history = max;
    }

    /// Take a snapshot of current state.
    fn take_snapshot(&mut self) {
        let snapshot = MetadataSnapshot::from_map(&self.data);
        self.history.push(snapshot);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// Apply a batch of updates. Snapshots state first for rollback.
    /// Returns a `BulkUpdateResult` describing the outcome.
    #[must_use]
    pub fn apply(&mut self, updates: &[FieldUpdate]) -> BulkUpdateResult {
        self.take_snapshot();

        let mut fields_set = 0usize;
        let mut fields_removed = 0usize;
        let mut fields_skipped = 0usize;
        let mut errors: HashMap<String, Vec<String>> = HashMap::new();

        for update in updates {
            // Run validation if pipeline is present and it's a set operation
            if let Some(ref pipeline) = self.pipeline {
                let validation_errors = pipeline.validate_update(update);
                if !validation_errors.is_empty() {
                    let msgs: Vec<String> = validation_errors
                        .iter()
                        .map(|e| e.message.clone())
                        .collect();
                    errors.insert(update.key.clone(), msgs);
                    fields_skipped += 1;
                    continue;
                }
            }

            match &update.value {
                Some(v) => {
                    self.data.insert(update.key.clone(), v.clone());
                    fields_set += 1;
                }
                None => {
                    self.data.remove(&update.key);
                    fields_removed += 1;
                }
            }
        }

        BulkUpdateResult {
            fields_set,
            fields_removed,
            fields_skipped,
            errors,
        }
    }

    /// Roll back to the previous snapshot. Returns `true` on success.
    pub fn rollback(&mut self) -> bool {
        if let Some(snapshot) = self.history.pop() {
            snapshot.restore(&mut self.data);
            true
        } else {
            false
        }
    }

    /// Get the current value of a field.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(String::as_str)
    }

    /// Return a read-only reference to all current data.
    #[must_use]
    pub fn data(&self) -> &HashMap<String, String> {
        &self.data
    }

    /// Number of undo levels available.
    #[must_use]
    pub fn history_depth(&self) -> usize {
        self.history.len()
    }

    /// Persist the engine's current metadata to disk for all provided file paths.
    ///
    /// Returns one [`BulkWriteResult`] per path, even on individual failures.
    /// For [`BulkWriteMode::Sidecar`] a `.meta` file is written alongside each target.
    /// For [`BulkWriteMode::Embed`] the metadata is embedded into the file bytes in place
    /// using an atomic rename via a temp file; only formats detectable by magic bytes are
    /// supported (unsupported files receive `success: false`).
    pub fn write_batch(&self, paths: &[&Path], mode: BulkWriteMode) -> Vec<BulkWriteResult> {
        paths
            .iter()
            .map(|&p| match mode {
                BulkWriteMode::Sidecar => self.write_sidecar(p),
                BulkWriteMode::Embed => self.write_embed(p),
            })
            .collect()
    }

    /// Write a `.meta` sidecar file alongside `path`.
    fn write_sidecar(&self, path: &Path) -> BulkWriteResult {
        use crate::sidecar::{SidecarFile, SidecarFormat};

        // Build sidecar from current data map
        let sidecar_path = sidecar_path_for(path);
        let mut sidecar = SidecarFile::new(
            sidecar_path.to_string_lossy().as_ref(),
            SidecarFormat::JsonSidecar,
        );
        for (k, v) in &self.data {
            sidecar.set(k, v);
        }
        let content = sidecar.serialize();
        let bytes = content.len();
        match std::fs::write(&sidecar_path, &content) {
            Ok(()) => BulkWriteResult {
                path: sidecar_path,
                success: true,
                bytes_written: bytes,
                error: None,
            },
            Err(e) => BulkWriteResult {
                path: sidecar_path,
                success: false,
                bytes_written: 0,
                error: Some(e.to_string()),
            },
        }
    }

    /// Embed metadata into `path` using an atomic temp-file rename.
    fn write_embed(&self, path: &Path) -> BulkWriteResult {
        // Determine format from magic bytes — only attempt supported ones
        let file_data = match std::fs::read(path) {
            Ok(d) => d,
            Err(e) => {
                return BulkWriteResult {
                    path: path.to_path_buf(),
                    success: false,
                    bytes_written: 0,
                    error: Some(format!("read error: {e}")),
                };
            }
        };

        let format = match detect_embeddable_format(&file_data) {
            Some(f) => f,
            None => {
                return BulkWriteResult {
                    path: path.to_path_buf(),
                    success: false,
                    bytes_written: 0,
                    error: Some("unsupported format".to_string()),
                };
            }
        };

        let metadata = data_to_metadata(&self.data, format);
        let patched = match crate::embed::embed(&file_data, &metadata) {
            Ok(b) => b,
            Err(e) => {
                return BulkWriteResult {
                    path: path.to_path_buf(),
                    success: false,
                    bytes_written: 0,
                    error: Some(format!("embed error: {e}")),
                };
            }
        };

        // Write atomically via a sibling temp file
        let tmp_path = path.with_extension("oximedia_tmp");
        if let Err(e) = std::fs::write(&tmp_path, &patched) {
            return BulkWriteResult {
                path: path.to_path_buf(),
                success: false,
                bytes_written: 0,
                error: Some(format!("temp write error: {e}")),
            };
        }
        match std::fs::rename(&tmp_path, path) {
            Ok(()) => BulkWriteResult {
                path: path.to_path_buf(),
                success: true,
                bytes_written: patched.len(),
                error: None,
            },
            Err(e) => {
                // Best-effort cleanup of temp file on rename failure
                let _ = std::fs::remove_file(&tmp_path);
                BulkWriteResult {
                    path: path.to_path_buf(),
                    success: false,
                    bytes_written: 0,
                    error: Some(format!("rename error: {e}")),
                }
            }
        }
    }
}

impl Default for BulkUpdateEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── Public types for batched I/O ────────────────────────────────────────────

/// How to persist the bulk update to disk.
#[derive(Debug, Clone, PartialEq)]
pub enum BulkWriteMode {
    /// Write a sidecar file (`.meta` / JSON) alongside each target file.
    Sidecar,
    /// Embed metadata directly into the file bytes (for supported formats).
    Embed,
}

/// Result of writing one file in a batch.
#[derive(Debug)]
pub struct BulkWriteResult {
    /// Destination path that was written (sidecar path for Sidecar mode).
    pub path: PathBuf,
    /// `true` when the write succeeded without error.
    pub success: bool,
    /// Number of bytes written to disk; `0` on failure.
    pub bytes_written: usize,
    /// Error message if `success` is `false`.
    pub error: Option<String>,
}

// ── Private helpers ──────────────────────────────────────────────────────────

/// Return the conventional `.meta` sidecar path alongside `media_path`.
fn sidecar_path_for(media_path: &Path) -> PathBuf {
    let mut p = media_path.to_path_buf();
    let ext = p
        .extension()
        .map(|e| format!("{}.meta", e.to_string_lossy()))
        .unwrap_or_else(|| "meta".to_string());
    p.set_extension(&ext);
    p
}

/// Detect which [`MetadataFormat`] can be embedded from magic bytes.
/// Returns `None` for formats that are not supported by the embed path.
fn detect_embeddable_format(data: &[u8]) -> Option<MetadataFormat> {
    if data.len() >= 3 && &data[0..3] == b"ID3" {
        return Some(MetadataFormat::Id3v2);
    }
    if data.len() >= 8 && &data[0..8] == b"APETAGEX" {
        return Some(MetadataFormat::Apev2);
    }
    // Container formats whose tag vocabulary is free-form name/value pairs, so
    // the engine's arbitrary keys map onto them directly. `crate::embed` splices
    // all three container-aware (FLAC metadata blocks, Ogg comment-packet
    // re-lacing, EBML `Tags`).
    if data.len() >= 4 && &data[0..4] == b"fLaC" {
        return Some(MetadataFormat::VorbisComments);
    }
    if data.len() >= 4 && &data[0..4] == b"OggS" {
        return Some(MetadataFormat::VorbisComments);
    }
    if data.len() >= 4 && data[0..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(MetadataFormat::Matroska);
    }
    // JPEG is deliberately absent: its bytes alone do not say whether the caller
    // means Exif, XMP or IPTC, and guessing one would write the tags where the
    // caller cannot find them. MP4 is absent because iTunes atom names are
    // four-byte identifiers that these free-form keys do not map onto.
    None
}

/// Build a [`Metadata`] container from the engine's `HashMap<String, String>`.
///
/// Every value is stored as [`MetadataValue::Text`].  For [`MetadataFormat::Id3v2`]
/// common human-readable field names (e.g. `"title"`) are mapped to their canonical
/// four-character ID3v2 frame IDs (e.g. `"TIT2"`) so that the ID3v2 writer — which
/// requires exactly 4-char frame IDs — does not reject them.  Keys that already look
/// like 4-char uppercase frame IDs are passed through unchanged.  Unknown keys are
/// skipped for the ID3v2 format to avoid hard-to-diagnose write errors.
fn data_to_metadata(data: &HashMap<String, String>, format: MetadataFormat) -> Metadata {
    let mut metadata = Metadata::new(format);
    for (k, v) in data {
        let key = if format == MetadataFormat::Id3v2 {
            match k.to_lowercase().as_str() {
                "title" => "TIT2".to_string(),
                "artist" => "TPE1".to_string(),
                "album" => "TALB".to_string(),
                "year" | "date" => "TDRC".to_string(),
                "genre" => "TCON".to_string(),
                "comment" => "COMM".to_string(),
                "tracknumber" | "track" => "TRCK".to_string(),
                "albumartist" => "TPE2".to_string(),
                "composer" => "TCOM".to_string(),
                "discnumber" | "disc" => "TPOS".to_string(),
                "lyrics" => "USLT".to_string(),
                "copyright" => "TCOP".to_string(),
                "encoder" => "TSSE".to_string(),
                "language" => "TLAN".to_string(),
                "bpm" => "TBPM".to_string(),
                // A key that is already exactly 4 uppercase ASCII chars is
                // assumed to be a native frame ID and passed through directly.
                _ if k.len() == 4
                    && k.chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
                {
                    k.clone()
                }
                // All other arbitrary keys are skipped for ID3v2 because the
                // writer enforces the 4-char frame ID constraint.
                _ => continue,
            }
        } else if matches!(
            format,
            MetadataFormat::VorbisComments | MetadataFormat::Matroska
        ) {
            // Vorbis comments and Matroska `SimpleTag` names are conventionally
            // uppercase (TITLE, ARTIST, …), and the Vorbis reader upper-cases
            // field names on parse, so writing them uppercase keeps a write →
            // read round-trip key-stable.
            k.to_uppercase()
        } else {
            k.clone()
        };
        metadata.insert(key, MetadataValue::Text(v.clone()));
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_field_update_set() {
        let u = FieldUpdate::set("title", "Hello");
        assert_eq!(u.key, "title");
        assert_eq!(u.value.as_deref(), Some("Hello"));
        assert!(!u.is_removal());
    }

    #[test]
    fn test_field_update_remove() {
        let u = FieldUpdate::remove("artist");
        assert!(u.is_removal());
    }

    #[test]
    fn test_validation_error_display() {
        let e = ValidationError::new("genre", "must not be empty");
        let s = e.to_string();
        assert!(s.contains("genre"));
        assert!(s.contains("must not be empty"));
    }

    #[test]
    fn test_pipeline_no_validators_always_valid() {
        let pipeline = ValidationPipeline::new();
        let update = FieldUpdate::set("title", "something");
        let errors = pipeline.validate_update(&update);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_pipeline_validator_catches_empty() {
        let mut pipeline = ValidationPipeline::new();
        pipeline.add_validator(|_key, value| {
            if value.is_empty() {
                Some("must not be empty".to_string())
            } else {
                None
            }
        });
        let update = FieldUpdate::set("title", "");
        let errors = pipeline.validate_update(&update);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "title");
    }

    #[test]
    fn test_pipeline_removal_always_valid() {
        let mut pipeline = ValidationPipeline::new();
        pipeline.add_validator(|_, _| Some("always error".to_string()));
        let update = FieldUpdate::remove("field");
        let errors = pipeline.validate_update(&update);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_pipeline_validate_batch() {
        let mut pipeline = ValidationPipeline::new();
        pipeline.add_validator(|_, v| {
            if v.len() > 3 {
                Some("too long".to_string())
            } else {
                None
            }
        });
        let updates = vec![
            FieldUpdate::set("a", "ok"),
            FieldUpdate::set("b", "toolong"),
        ];
        let result = pipeline.validate_batch(&updates);
        assert!(!result.contains_key("a"));
        assert!(result.contains_key("b"));
    }

    #[test]
    fn test_engine_apply_sets_fields() {
        let mut engine = BulkUpdateEngine::new();
        let updates = vec![
            FieldUpdate::set("title", "My Song"),
            FieldUpdate::set("artist", "Artist"),
        ];
        let result = engine.apply(&updates);
        assert_eq!(result.fields_set, 2);
        assert_eq!(result.fields_removed, 0);
        assert_eq!(engine.get("title"), Some("My Song"));
    }

    #[test]
    fn test_engine_apply_removes_field() {
        let mut data = HashMap::new();
        data.insert("title".to_string(), "Old Title".to_string());
        let mut engine = BulkUpdateEngine::with_data(data);
        let updates = vec![FieldUpdate::remove("title")];
        let result = engine.apply(&updates);
        assert_eq!(result.fields_removed, 1);
        assert!(engine.get("title").is_none());
    }

    #[test]
    fn test_engine_rollback() {
        let mut engine = BulkUpdateEngine::new();
        let _ = engine.apply(&[FieldUpdate::set("title", "v1")]);
        let _ = engine.apply(&[FieldUpdate::set("title", "v2")]);
        assert_eq!(engine.get("title"), Some("v2"));
        let ok = engine.rollback();
        assert!(ok);
        assert_eq!(engine.get("title"), Some("v1"));
    }

    #[test]
    fn test_engine_rollback_empty_history() {
        let mut engine = BulkUpdateEngine::new();
        assert!(!engine.rollback());
    }

    #[test]
    fn test_engine_validation_skips_invalid() {
        let mut pipeline = ValidationPipeline::new();
        pipeline.add_validator(|_, v| {
            if v.is_empty() {
                Some("empty not allowed".to_string())
            } else {
                None
            }
        });
        let mut engine = BulkUpdateEngine::new();
        engine.set_pipeline(pipeline);
        let updates = vec![
            FieldUpdate::set("title", "Good"),
            FieldUpdate::set("bad_field", ""),
        ];
        let result = engine.apply(&updates);
        assert_eq!(result.fields_set, 1);
        assert_eq!(result.fields_skipped, 1);
        assert!(result.errors.contains_key("bad_field"));
    }

    #[test]
    fn test_engine_history_depth() {
        let mut engine = BulkUpdateEngine::new();
        let _ = engine.apply(&[FieldUpdate::set("a", "1")]);
        let _ = engine.apply(&[FieldUpdate::set("a", "2")]);
        assert_eq!(engine.history_depth(), 2);
    }

    #[test]
    fn test_snapshot_restore() {
        let mut data = HashMap::new();
        data.insert("x".to_string(), "original".to_string());
        let snapshot = MetadataSnapshot::from_map(&data);
        data.insert("x".to_string(), "modified".to_string());
        snapshot.restore(&mut data);
        assert_eq!(data.get("x").map(String::as_str), Some("original"));
    }

    #[test]
    fn test_result_total_applied() {
        let result = BulkUpdateResult {
            fields_set: 3,
            fields_removed: 2,
            fields_skipped: 1,
            errors: HashMap::new(),
        };
        assert_eq!(result.total_applied(), 5);
        assert!(result.is_clean());
    }

    /// Verify that `write_batch` in Sidecar mode creates `.meta` sidecar files
    /// and that the serialised content contains the expected key-value pairs.
    #[test]
    fn test_bulk_write_sidecar_roundtrip() {
        use std::fs;

        let tmp = std::env::temp_dir().join("oximedia_bulk_write_sidecar_roundtrip");
        fs::create_dir_all(&tmp).expect("create temp dir");

        // Create 3 dummy media files so the parent path exists
        let names = ["clip_a.mp4", "clip_b.mkv", "clip_c.mov"];
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        for name in &names {
            let p = tmp.join(name);
            fs::write(&p, b"dummy media content").expect("write dummy file");
            paths.push(p);
        }

        // Build engine with known fields
        let mut data = HashMap::new();
        data.insert("title".to_string(), "Test Title".to_string());
        data.insert("artist".to_string(), "Test Artist".to_string());
        let engine = BulkUpdateEngine::with_data(data);

        let path_refs: Vec<&std::path::Path> = paths.iter().map(|p| p.as_path()).collect();
        let results = engine.write_batch(&path_refs, BulkWriteMode::Sidecar);

        assert_eq!(results.len(), 3, "should have one result per path");

        for result in &results {
            assert!(
                result.success,
                "sidecar write should succeed: {:?}",
                result.error
            );
            assert!(result.bytes_written > 0, "should have written bytes");
            // Sidecar file must exist and contain expected content
            let content =
                fs::read_to_string(&result.path).expect("sidecar file should be readable");
            assert!(
                content.contains("title") || content.contains("artist"),
                "sidecar content should contain field names, got: {content}"
            );
        }

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Verify that `write_batch` in Embed mode returns `success: false` with a
    /// non-empty error string for files whose format is not embeddable (e.g. `.txt`).
    #[test]
    fn test_bulk_write_embed_unsupported() {
        use std::fs;

        let tmp = std::env::temp_dir().join("oximedia_bulk_write_embed_unsupported");
        fs::create_dir_all(&tmp).expect("create temp dir");

        // A plain-text file has no embeddable magic bytes
        let txt_path = tmp.join("readme.txt");
        fs::write(&txt_path, b"hello world").expect("write txt file");

        let mut data = HashMap::new();
        data.insert("title".to_string(), "Should Fail".to_string());
        let engine = BulkUpdateEngine::with_data(data);

        let results = engine.write_batch(&[txt_path.as_path()], BulkWriteMode::Embed);

        assert_eq!(results.len(), 1);
        let r = &results[0];
        assert!(!r.success, "embed of .txt should fail");
        assert_eq!(r.bytes_written, 0);
        assert!(r.error.is_some(), "error field should be populated");

        // Cleanup
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_bulk_write_embed_flac_writes_a_real_comment_block() {
        use std::fs;

        // A minimal but well-formed FLAC stream: "fLaC" + a last STREAMINFO
        // block + opaque audio frames.
        let mut flac = b"fLaC".to_vec();
        flac.extend_from_slice(&[0x80, 0x00, 0x00, 0x22]); // last block, STREAMINFO, 34 bytes
        flac.extend_from_slice(&[0u8; 34]);
        flac.extend_from_slice(b"AUDIOFRAMES");

        let tmp = std::env::temp_dir().join("oximedia_bulk_write_embed_flac");
        fs::create_dir_all(&tmp).expect("create temp dir");
        let path = tmp.join("track.flac");
        fs::write(&path, &flac).expect("write flac fixture");

        let mut data = HashMap::new();
        data.insert("title".to_string(), "Bulk Embedded".to_string());
        let engine = BulkUpdateEngine::with_data(data);

        let results = engine.write_batch(&[path.as_path()], BulkWriteMode::Embed);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].success,
            "FLAC embed should succeed: {:?}",
            results[0].error
        );

        // The file on disk must now carry a readable VORBIS_COMMENT block, and
        // its audio frames must be untouched.
        let written = fs::read(&path).expect("read patched flac");
        assert!(written.ends_with(b"AUDIOFRAMES"));
        let mut pos = 4usize;
        let mut comment_block = None;
        loop {
            let is_last = written[pos] & 0x80 != 0;
            let block_type = written[pos] & 0x7F;
            let len = (usize::from(written[pos + 1]) << 16)
                | (usize::from(written[pos + 2]) << 8)
                | usize::from(written[pos + 3]);
            if block_type == 4 {
                comment_block = Some(written[pos + 4..pos + 4 + len].to_vec());
            }
            pos += 4 + len;
            if is_last {
                break;
            }
        }
        let comment_block = comment_block.expect("VORBIS_COMMENT block present");
        let parsed = crate::vorbis::parse(&comment_block).expect("parse comment block");
        assert_eq!(
            parsed.get("TITLE").and_then(MetadataValue::as_text),
            Some("Bulk Embedded"),
            "free-form keys must be written uppercase so the reader finds them"
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
