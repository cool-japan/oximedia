//! Real execution back-ends for the media-oriented [`TaskType`] variants.
//!
//! [`crate::executor::DefaultTaskExecutor`] used to *log* the intent of the
//! `HttpRequest`, `Transcode`, `QualityControl` and `Analysis` task types and
//! return `Ok(())`, so a workflow DAG advanced exactly as if the media work
//! had happened. This module contains the implementations that actually
//! perform that work:
//!
//! | Module | Task type | Backed by |
//! |---|---|---|
//! | [`http`] | [`TaskType::HttpRequest`] | `reqwest` (Pure-Rust rustls) |
//! | [`transcode`] | [`TaskType::Transcode`] | `oximedia-transcode` `TranscodePipeline` |
//! | [`qc`] | [`TaskType::QualityControl`] | `oximedia-qc` `QualityControl` |
//! | [`analysis`] | [`TaskType::Analysis`] | `oximedia-analysis` / `oximedia-audio-analysis` / `oximedia-quality` |
//! | [`media`] | (shared) | `oximedia-container` demuxers + `oximedia-codec` PCM decode |
//!
//! # Honesty contract
//!
//! Every entry point in this module either performs the requested work or
//! returns an [`Err`] naming the precise limitation. Nothing here reports
//! success for work it did not do:
//!
//! - unknown codec / preset / QC profile / analysis input format → `Err`
//!   listing what *is* accepted,
//! - a transcode whose output file is missing or empty → `Err`,
//! - a QC report with failing checks → `Err` quoting the failures,
//! - an analysis whose input cannot be decoded → `Err`.
//!
//! [`TaskType`]: crate::task::TaskType
//! [`TaskType::HttpRequest`]: crate::task::TaskType::HttpRequest
//! [`TaskType::Transcode`]: crate::task::TaskType::Transcode
//! [`TaskType::QualityControl`]: crate::task::TaskType::QualityControl
//! [`TaskType::Analysis`]: crate::task::TaskType::Analysis

pub mod analysis;
pub mod http;
pub mod media;
pub mod qc;
pub mod transcode;

use std::path::PathBuf;

/// Evidence produced by a successfully executed task.
///
/// The executor copies these fields into [`crate::task::TaskResult`] so a
/// completed task carries a machine-readable record of what it did (HTTP
/// status, transcode output size, QC verdict, analysis measurements) instead
/// of an empty `data: None` / `outputs: []`.
#[derive(Debug, Clone, Default)]
pub struct TaskOutcome {
    /// Structured result payload (stored in `TaskResult::data`).
    pub data: Option<serde_json::Value>,
    /// Files the task produced (stored in `TaskResult::outputs`).
    pub outputs: Vec<PathBuf>,
}

impl TaskOutcome {
    /// An outcome with no payload and no produced files.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            data: None,
            outputs: Vec::new(),
        }
    }

    /// An outcome carrying a structured payload.
    #[must_use]
    pub const fn with_data(data: serde_json::Value) -> Self {
        Self {
            data: Some(data),
            outputs: Vec::new(),
        }
    }

    /// Adds a produced output file to this outcome.
    #[must_use]
    pub fn and_output(mut self, path: impl Into<PathBuf>) -> Self {
        self.outputs.push(path.into());
        self
    }
}

/// Verifies that `path` exists and is a non-empty regular file.
///
/// Used after every operation that claims to have produced a file, so a
/// zero-byte or missing artefact is reported as a failure instead of being
/// silently accepted.
///
/// # Errors
///
/// Returns [`crate::error::WorkflowError`] when the file is missing, is not a
/// regular file, or has a length of zero bytes.
pub(crate) fn verify_non_empty_file(
    path: &std::path::Path,
    what: &str,
) -> crate::error::Result<u64> {
    use crate::error::WorkflowError;

    let metadata = std::fs::metadata(path).map_err(|e| {
        WorkflowError::generic(format!("{what} did not produce {}: {e}", path.display()))
    })?;
    if !metadata.is_file() {
        return Err(WorkflowError::generic(format!(
            "{what} output {} is not a regular file",
            path.display()
        )));
    }
    if metadata.len() == 0 {
        return Err(WorkflowError::generic(format!(
            "{what} produced an empty file: {}",
            path.display()
        )));
    }
    Ok(metadata.len())
}
