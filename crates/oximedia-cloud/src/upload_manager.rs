#![allow(dead_code)]
//! Chunked upload management for cloud storage.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// State of an upload job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UploadState {
    /// Upload has not yet started.
    Pending,
    /// Upload is currently in progress.
    Uploading,
    /// Upload has been paused.
    Paused,
    /// Upload completed successfully.
    Completed,
    /// Upload failed with an error message.
    Failed(String),
}

impl UploadState {
    /// Returns true if the upload is actively in progress.
    pub fn is_active(&self) -> bool {
        matches!(self, UploadState::Uploading)
    }

    /// Returns true if the upload has finished (either completed or failed).
    pub fn is_terminal(&self) -> bool {
        matches!(self, UploadState::Completed | UploadState::Failed(_))
    }

    /// Returns true if the upload completed successfully.
    pub fn is_success(&self) -> bool {
        matches!(self, UploadState::Completed)
    }
}

/// A single chunk of a multipart upload.
#[derive(Debug, Clone)]
pub struct UploadChunk {
    /// Zero-based index of this chunk.
    pub index: usize,
    /// Byte offset within the full file.
    pub offset: u64,
    /// Size of this chunk in bytes.
    pub size: u64,
    /// Whether this chunk has been successfully uploaded.
    pub uploaded: bool,
    /// ETag or checksum returned by the cloud provider.
    pub etag: Option<String>,
}

impl UploadChunk {
    /// Creates a new chunk descriptor.
    pub fn new(index: usize, offset: u64, size: u64) -> Self {
        Self {
            index,
            offset,
            size,
            uploaded: false,
            etag: None,
        }
    }

    /// Marks the chunk as uploaded with the given ETag.
    pub fn mark_uploaded(&mut self, etag: impl Into<String>) {
        self.uploaded = true;
        self.etag = Some(etag.into());
    }

    /// Returns true if this chunk has been successfully uploaded.
    pub fn is_complete(&self) -> bool {
        self.uploaded
    }

    /// Returns the end byte offset (exclusive) of this chunk.
    pub fn end_offset(&self) -> u64 {
        self.offset + self.size
    }
}

/// An upload job tracking the full multipart upload of a single file.
#[derive(Debug)]
pub struct UploadJob {
    /// Unique job identifier.
    pub id: u64,
    /// Remote destination key or path.
    pub destination: String,
    /// Total size of the file in bytes.
    pub total_size: u64,
    /// Current state of the job.
    pub state: UploadState,
    /// Individual chunks.
    chunks: Vec<UploadChunk>,
}

impl UploadJob {
    /// Creates a new upload job and automatically splits the file into chunks.
    pub fn new(id: u64, destination: impl Into<String>, total_size: u64, chunk_size: u64) -> Self {
        let dest = destination.into();
        let chunk_size = chunk_size.max(1);
        let num_chunks = total_size.div_ceil(chunk_size) as usize;
        let chunks = (0..num_chunks)
            .map(|i| {
                let offset = i as u64 * chunk_size;
                let size = chunk_size.min(total_size - offset);
                UploadChunk::new(i, offset, size)
            })
            .collect();
        Self {
            id,
            destination: dest,
            total_size,
            state: UploadState::Pending,
            chunks,
        }
    }

    /// Returns the total number of chunks in this job.
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Returns the number of chunks that have been uploaded.
    pub fn uploaded_chunk_count(&self) -> usize {
        self.chunks.iter().filter(|c| c.is_complete()).count()
    }

    /// Returns the upload progress as a percentage in `[0.0, 100.0]`.
    #[allow(clippy::cast_precision_loss)]
    pub fn progress_pct(&self) -> f64 {
        if self.chunks.is_empty() {
            return 100.0;
        }
        let done = self.uploaded_chunk_count() as f64;
        let total = self.chunks.len() as f64;
        (done / total) * 100.0
    }

    /// Marks a chunk by index as uploaded.
    ///
    /// Returns `false` if the index is out of bounds.
    pub fn complete_chunk(&mut self, index: usize, etag: impl Into<String>) -> bool {
        if let Some(chunk) = self.chunks.get_mut(index) {
            chunk.mark_uploaded(etag);
            true
        } else {
            false
        }
    }

    /// Returns an iterator over all chunks.
    pub fn chunks(&self) -> impl Iterator<Item = &UploadChunk> {
        self.chunks.iter()
    }

    /// Returns the number of bytes uploaded so far.
    pub fn uploaded_bytes(&self) -> u64 {
        self.chunks
            .iter()
            .filter(|c| c.is_complete())
            .map(|c| c.size)
            .sum()
    }
}

/// Manager that tracks multiple concurrent upload jobs.
#[derive(Debug, Default)]
pub struct UploadManager {
    jobs: Vec<UploadJob>,
    next_id: u64,
    /// Default chunk size in bytes (5 MiB).
    chunk_size: u64,
}

impl UploadManager {
    /// Creates a new upload manager with the default 5 MiB chunk size.
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 0,
            chunk_size: 5 * 1024 * 1024,
        }
    }

    /// Creates a new upload manager with a custom chunk size.
    pub fn with_chunk_size(chunk_size: u64) -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 0,
            chunk_size,
        }
    }

    /// Creates and registers a new upload job.
    pub fn create_job(&mut self, destination: impl Into<String>, total_size: u64) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let job = UploadJob::new(id, destination, total_size, self.chunk_size);
        self.jobs.push(job);
        id
    }

    /// Returns a reference to a job by its ID.
    pub fn get_job(&self, id: u64) -> Option<&UploadJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Returns a mutable reference to a job by its ID.
    pub fn get_job_mut(&mut self, id: u64) -> Option<&mut UploadJob> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    /// Returns the number of jobs currently managed.
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Returns the number of active (uploading) jobs.
    pub fn active_job_count(&self) -> usize {
        self.jobs.iter().filter(|j| j.state.is_active()).count()
    }

    /// Returns the chunk count for a given job, or 0 if not found.
    pub fn chunk_count(&self, job_id: u64) -> usize {
        self.get_job(job_id).map_or(0, |j| j.chunk_count())
    }
}

// ── Resumable multipart upload ───────────────────────────────────────────────

/// A (part_number, etag) pair produced after a part upload completes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PartEtag {
    /// 1-based part number.
    pub part_number: u32,
    /// ETag returned by the cloud provider for this part.
    pub etag: String,
}

/// Raw bytes for one upload part together with its part number.
#[derive(Debug, Clone)]
pub struct PartData {
    /// 1-based part number.
    pub part_number: u32,
    /// The raw bytes to upload.
    pub data: Vec<u8>,
}

/// Persisted snapshot of upload progress used for resumption.
///
/// Serialised to JSON and stored alongside the uploaded file so that
/// interrupted uploads can be resumed without re-sending already-completed
/// parts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadCheckpoint {
    /// Remote destination key/path.
    pub destination: String,
    /// Total file size in bytes.
    pub total_size: u64,
    /// Provider-issued multipart upload ID (e.g. AWS upload ID).
    pub upload_id: String,
    /// Parts that have already been successfully uploaded.
    pub completed_parts: Vec<PartEtag>,
    /// Unix epoch timestamp (seconds) when the checkpoint was last updated.
    pub updated_at_epoch: u64,
}

impl UploadCheckpoint {
    /// Create a fresh checkpoint for a new upload.
    #[must_use]
    pub fn new(
        destination: impl Into<String>,
        total_size: u64,
        upload_id: impl Into<String>,
    ) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            destination: destination.into(),
            total_size,
            upload_id: upload_id.into(),
            completed_parts: Vec::new(),
            updated_at_epoch: now,
        }
    }

    /// Record a newly completed part and refresh the timestamp.
    pub fn add_part(&mut self, part: PartEtag) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.completed_parts.push(part);
        self.updated_at_epoch = now;
    }

    /// Returns `true` if the given part number has already been uploaded.
    #[must_use]
    pub fn is_part_done(&self, part_number: u32) -> bool {
        self.completed_parts
            .iter()
            .any(|p| p.part_number == part_number)
    }

    /// Serialise the checkpoint to a JSON string.
    ///
    /// # Errors
    ///
    /// Returns a `serde_json` error if serialisation fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserialise a checkpoint from a JSON string.
    ///
    /// # Errors
    ///
    /// Returns a `serde_json` error if the input is not valid JSON.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// A resumable multipart upload session that persists its progress to disk.
#[derive(Debug)]
pub struct ResumableUpload {
    /// Current checkpoint (in-memory, mirrored to disk on every part commit).
    pub checkpoint: UploadCheckpoint,
    /// Path to the JSON checkpoint file on disk.
    checkpoint_path: PathBuf,
}

impl ResumableUpload {
    /// Attempt to resume an existing upload or start a fresh one.
    ///
    /// If a checkpoint file exists at `checkpoint_path`, it is loaded and the
    /// upload is resumed from where it left off.  Otherwise, `new_checkpoint`
    /// is used to initialise a fresh upload.
    ///
    /// # Errors
    ///
    /// Returns an `std::io::Error` if the checkpoint file cannot be read, or a
    /// `serde_json::Error` if the checkpoint JSON is malformed.
    pub fn resume_or_start(
        checkpoint_path: &Path,
        new_checkpoint: UploadCheckpoint,
    ) -> Result<Self, ResumableUploadError> {
        let checkpoint = if checkpoint_path.exists() {
            let json =
                std::fs::read_to_string(checkpoint_path).map_err(ResumableUploadError::Io)?;
            UploadCheckpoint::from_json(&json).map_err(ResumableUploadError::Json)?
        } else {
            new_checkpoint
        };

        Ok(Self {
            checkpoint,
            checkpoint_path: checkpoint_path.to_path_buf(),
        })
    }

    /// Commit a newly completed part: update the in-memory checkpoint and
    /// persist it to the checkpoint file.
    ///
    /// # Errors
    ///
    /// Returns an error if writing the checkpoint file fails.
    pub fn commit_part(&mut self, part: PartEtag) -> Result<(), ResumableUploadError> {
        self.checkpoint.add_part(part);
        self.persist()
    }

    /// Write the checkpoint to disk.
    ///
    /// # Errors
    ///
    /// Returns an error if serialisation or the file write fails.
    pub fn persist(&self) -> Result<(), ResumableUploadError> {
        let json = self
            .checkpoint
            .to_json()
            .map_err(ResumableUploadError::Json)?;
        std::fs::write(&self.checkpoint_path, json).map_err(ResumableUploadError::Io)
    }

    /// Remove the checkpoint file once the upload has been finalised.
    ///
    /// # Errors
    ///
    /// Returns an `std::io::Error` if the file cannot be removed.
    pub fn finalize(self) -> Result<(), ResumableUploadError> {
        if self.checkpoint_path.exists() {
            std::fs::remove_file(&self.checkpoint_path).map_err(ResumableUploadError::Io)?;
        }
        Ok(())
    }

    /// Returns the list of parts already uploaded.
    #[must_use]
    pub fn completed_parts(&self) -> &[PartEtag] {
        &self.checkpoint.completed_parts
    }
}

/// Errors that can occur during a resumable upload operation.
#[derive(Debug)]
pub enum ResumableUploadError {
    /// An I/O error when reading or writing the checkpoint file.
    Io(std::io::Error),
    /// A JSON (de)serialisation error.
    Json(serde_json::Error),
}

impl std::fmt::Display for ResumableUploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResumableUploadError::Io(e) => write!(f, "I/O error: {e}"),
            ResumableUploadError::Json(e) => write!(f, "JSON error: {e}"),
        }
    }
}

impl std::error::Error for ResumableUploadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ResumableUploadError::Io(e) => Some(e),
            ResumableUploadError::Json(e) => Some(e),
        }
    }
}

// ── Parallel multipart upload ────────────────────────────────────────────────

/// Upload multiple parts in parallel using a Rayon thread pool.
///
/// `parts` is a list of `PartData` values to be passed to `upload_fn` in
/// parallel.  The parallelism is bounded to `concurrency` threads via a
/// dedicated `rayon::ThreadPool`.
///
/// Returns one `Result<PartEtag, E>` per input part **in the original order**.
///
/// # Errors
///
/// Each element of the returned `Vec` may be an `Err` if the upload for that
/// part failed; the caller is responsible for inspecting every result.
pub fn parallel_upload_parts<F, E>(
    parts: Vec<PartData>,
    concurrency: usize,
    upload_fn: F,
) -> Vec<Result<PartEtag, E>>
where
    F: Fn(&PartData) -> Result<PartEtag, E> + Send + Sync,
    E: Send,
{
    let concurrency = concurrency.max(1);
    match rayon::ThreadPoolBuilder::new()
        .num_threads(concurrency)
        .build()
    {
        Ok(pool) => pool.install(|| {
            parts
                .par_chunks(1)
                .map(|chunk| upload_fn(&chunk[0]))
                .collect()
        }),
        Err(_) => {
            // Thread pool creation failed (e.g. OS resource exhaustion) —
            // fall back to sequential execution instead of requiring a
            // second pool-creation attempt to succeed.
            parts.iter().map(|p| upload_fn(p)).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_state_is_active() {
        assert!(UploadState::Uploading.is_active());
        assert!(!UploadState::Pending.is_active());
        assert!(!UploadState::Completed.is_active());
    }

    #[test]
    fn test_upload_state_is_terminal() {
        assert!(UploadState::Completed.is_terminal());
        assert!(UploadState::Failed("err".into()).is_terminal());
        assert!(!UploadState::Uploading.is_terminal());
    }

    #[test]
    fn test_upload_state_is_success() {
        assert!(UploadState::Completed.is_success());
        assert!(!UploadState::Failed("x".into()).is_success());
    }

    #[test]
    fn test_chunk_is_complete_initially_false() {
        let c = UploadChunk::new(0, 0, 1024);
        assert!(!c.is_complete());
    }

    #[test]
    fn test_chunk_mark_uploaded() {
        let mut c = UploadChunk::new(0, 0, 1024);
        c.mark_uploaded("etag-abc");
        assert!(c.is_complete());
        assert_eq!(c.etag.as_deref(), Some("etag-abc"));
    }

    #[test]
    fn test_chunk_end_offset() {
        let c = UploadChunk::new(0, 1000, 500);
        assert_eq!(c.end_offset(), 1500);
    }

    #[test]
    fn test_upload_job_chunk_count() {
        // 10 MiB file, 5 MiB chunks → 2 chunks
        let job = UploadJob::new(0, "dest/file.mp4", 10 * 1024 * 1024, 5 * 1024 * 1024);
        assert_eq!(job.chunk_count(), 2);
    }

    #[test]
    fn test_upload_job_chunk_count_remainder() {
        // 11 MiB file, 5 MiB chunks → 3 chunks
        let job = UploadJob::new(0, "dest/file.mp4", 11 * 1024 * 1024, 5 * 1024 * 1024);
        assert_eq!(job.chunk_count(), 3);
    }

    #[test]
    fn test_progress_pct_initial() {
        let job = UploadJob::new(0, "dest", 1000, 100);
        assert!((job.progress_pct() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_progress_pct_after_completion() {
        let mut job = UploadJob::new(0, "dest", 1000, 100);
        for i in 0..10 {
            job.complete_chunk(i, format!("etag-{}", i));
        }
        assert!((job.progress_pct() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn test_progress_pct_partial() {
        let mut job = UploadJob::new(0, "dest", 1000, 100);
        job.complete_chunk(0, "e0");
        job.complete_chunk(1, "e1");
        job.complete_chunk(2, "e2");
        job.complete_chunk(3, "e3");
        job.complete_chunk(4, "e4");
        // 5 of 10 done
        assert!((job.progress_pct() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn test_complete_chunk_out_of_bounds() {
        let mut job = UploadJob::new(0, "dest", 100, 100);
        assert!(!job.complete_chunk(999, "etag"));
    }

    #[test]
    fn test_uploaded_bytes() {
        let mut job = UploadJob::new(0, "dest", 300, 100);
        job.complete_chunk(0, "e0");
        assert_eq!(job.uploaded_bytes(), 100);
    }

    #[test]
    fn test_manager_create_job() {
        let mut mgr = UploadManager::with_chunk_size(100);
        let id = mgr.create_job("remote/file.mp4", 300);
        assert_eq!(mgr.job_count(), 1);
        assert_eq!(mgr.chunk_count(id), 3);
    }

    #[test]
    fn test_manager_get_job() {
        let mut mgr = UploadManager::with_chunk_size(100);
        let id = mgr.create_job("file.mp4", 200);
        let job = mgr.get_job(id);
        assert!(job.is_some());
    }

    #[test]
    fn test_manager_active_job_count() {
        let mut mgr = UploadManager::with_chunk_size(100);
        let id = mgr.create_job("file.mp4", 100);
        mgr.get_job_mut(id)
            .expect("get_job_mut should succeed")
            .state = UploadState::Uploading;
        assert_eq!(mgr.active_job_count(), 1);
    }

    // ── UploadCheckpoint tests ────────────────────────────────────────────────

    #[test]
    fn test_checkpoint_new_starts_empty() {
        let cp = UploadCheckpoint::new("s3://bucket/key.mp4", 1_000_000, "upload-id-abc");
        assert_eq!(cp.destination, "s3://bucket/key.mp4");
        assert_eq!(cp.upload_id, "upload-id-abc");
        assert!(cp.completed_parts.is_empty());
    }

    #[test]
    fn test_checkpoint_add_part() {
        let mut cp = UploadCheckpoint::new("dest", 1000, "uid");
        cp.add_part(PartEtag {
            part_number: 1,
            etag: "etag1".into(),
        });
        assert_eq!(cp.completed_parts.len(), 1);
        assert!(cp.is_part_done(1));
        assert!(!cp.is_part_done(2));
    }

    #[test]
    fn test_checkpoint_json_roundtrip() {
        let mut cp = UploadCheckpoint::new("bucket/key", 500, "id-xyz");
        cp.add_part(PartEtag {
            part_number: 1,
            etag: "etag-abc".into(),
        });
        let json = cp.to_json().expect("serialisation must succeed");
        let restored = UploadCheckpoint::from_json(&json).expect("deserialisation must succeed");
        assert_eq!(restored.upload_id, "id-xyz");
        assert_eq!(restored.completed_parts.len(), 1);
        assert_eq!(restored.completed_parts[0].etag, "etag-abc");
    }

    // ── ResumableUpload tests ─────────────────────────────────────────────────

    #[test]
    fn test_resumable_upload_fresh_start() {
        let dir = std::env::temp_dir();
        let cp_path = dir.join("oximedia_test_fresh_checkpoint.json");
        // Ensure no stale file from a previous run
        let _ = std::fs::remove_file(&cp_path);

        let checkpoint = UploadCheckpoint::new("dest/file.mp4", 2048, "fresh-id");
        let upload = ResumableUpload::resume_or_start(&cp_path, checkpoint)
            .expect("fresh start must succeed");
        assert_eq!(upload.checkpoint.upload_id, "fresh-id");
        assert!(upload.completed_parts().is_empty());
    }

    #[test]
    fn test_resumable_upload_commit_and_reload() {
        let dir = std::env::temp_dir();
        let cp_path = dir.join("oximedia_test_resumable_checkpoint.json");
        let _ = std::fs::remove_file(&cp_path);

        let checkpoint = UploadCheckpoint::new("dest/video.mp4", 4096, "resume-id");
        let mut upload = ResumableUpload::resume_or_start(&cp_path, checkpoint)
            .expect("fresh start must succeed");

        upload
            .commit_part(PartEtag {
                part_number: 1,
                etag: "e1".into(),
            })
            .expect("commit_part must succeed");
        upload
            .commit_part(PartEtag {
                part_number: 2,
                etag: "e2".into(),
            })
            .expect("commit_part must succeed");
        assert_eq!(upload.completed_parts().len(), 2);

        // Re-load from the persisted checkpoint file
        let dummy_new = UploadCheckpoint::new("dest/video.mp4", 4096, "resume-id");
        let reloaded =
            ResumableUpload::resume_or_start(&cp_path, dummy_new).expect("resume must succeed");
        assert_eq!(reloaded.completed_parts().len(), 2);
        assert!(reloaded.checkpoint.is_part_done(1));
        assert!(reloaded.checkpoint.is_part_done(2));

        // Clean up
        reloaded.finalize().expect("finalize must succeed");
        assert!(!cp_path.exists());
    }

    // ── parallel_upload_parts tests ───────────────────────────────────────────

    #[test]
    fn test_parallel_upload_parts_all_succeed() {
        let parts: Vec<PartData> = (1u32..=4)
            .map(|n| PartData {
                part_number: n,
                data: vec![n as u8; 16],
            })
            .collect();

        let results: Vec<Result<PartEtag, String>> = parallel_upload_parts(parts, 2, |p| {
            Ok(PartEtag {
                part_number: p.part_number,
                etag: format!("etag-{}", p.part_number),
            })
        });

        assert_eq!(results.len(), 4);
        for (i, r) in results.iter().enumerate() {
            let etag = r.as_ref().expect("upload must succeed");
            assert_eq!(etag.part_number, (i + 1) as u32);
        }
    }

    #[test]
    fn test_parallel_upload_parts_some_fail() {
        let parts: Vec<PartData> = (1u32..=3)
            .map(|n| PartData {
                part_number: n,
                data: vec![0u8; 8],
            })
            .collect();

        let results: Vec<Result<PartEtag, String>> = parallel_upload_parts(parts, 3, |p| {
            if p.part_number == 2 {
                Err(format!("part {} failed", p.part_number))
            } else {
                Ok(PartEtag {
                    part_number: p.part_number,
                    etag: "ok".into(),
                })
            }
        });

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_err());
        assert!(results[2].is_ok());
    }
}
