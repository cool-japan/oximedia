//! Transfer management with chunked uploads, retry, and progress tracking

use bytes::{Bytes, BytesMut};
use futures::stream::{self, StreamExt};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::error::Result;
use crate::types::{CloudStorage, TransferProgress};

/// Transfer manager for handling chunked uploads and downloads
pub struct TransferManager {
    /// Storage backend
    storage: Arc<dyn CloudStorage>,
    /// Configuration
    config: TransferConfig,
    /// Active transfers
    transfers: Arc<RwLock<HashMap<String, TransferState>>>,
}

impl TransferManager {
    /// Create a new transfer manager
    #[must_use]
    pub fn new(storage: Arc<dyn CloudStorage>, config: TransferConfig) -> Self {
        Self {
            storage,
            config,
            transfers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Upload data with chunking and retry
    ///
    /// # Errors
    ///
    /// Returns an error if the upload fails after all retries
    pub async fn upload(
        &self,
        key: &str,
        data: Bytes,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<()> {
        let transfer_id = format!("upload-{key}");
        self.init_transfer(&transfer_id, data.len() as u64);

        if data.len() <= self.config.chunk_size {
            // Single-part upload
            self.upload_single_part(key, data, &transfer_id, progress_tx)
                .await
        } else {
            // Multi-part upload
            self.upload_multipart(key, data, &transfer_id, progress_tx)
                .await
        }
    }

    /// Download data with chunking and retry
    ///
    /// # Errors
    ///
    /// Returns an error if the download fails after all retries
    pub async fn download(
        &self,
        key: &str,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<Bytes> {
        let transfer_id = format!("download-{key}");

        // Get object metadata to determine size
        let metadata = self.storage.get_metadata(key).await?;
        let total_size = metadata.info.size;

        self.init_transfer(&transfer_id, total_size);

        if total_size <= self.config.chunk_size as u64 {
            // Single-part download
            self.download_single_part(key, &transfer_id, progress_tx)
                .await
        } else {
            // Multi-part download
            self.download_multipart(key, total_size, &transfer_id, progress_tx)
                .await
        }
    }

    /// Upload single part with retry
    async fn upload_single_part(
        &self,
        key: &str,
        data: Bytes,
        transfer_id: &str,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<()> {
        let mut attempts = 0;
        let total_size = data.len() as u64;

        loop {
            match self.storage.upload(key, data.clone()).await {
                Ok(()) => {
                    self.update_progress(transfer_id, total_size, total_size, &progress_tx)
                        .await;
                    self.complete_transfer(transfer_id);
                    return Ok(());
                }
                Err(e) if e.is_retryable() && attempts < self.config.max_retries => {
                    attempts += 1;
                    tracing::warn!("Upload attempt {} failed: {}", attempts, e);
                    sleep(self.retry_delay(attempts)).await;
                }
                Err(e) => {
                    self.fail_transfer(transfer_id);
                    return Err(e);
                }
            }
        }
    }

    /// Upload with multipart
    async fn upload_multipart(
        &self,
        key: &str,
        data: Bytes,
        transfer_id: &str,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<()> {
        let chunk_size = self.config.chunk_size;
        let total_size = data.len() as u64;
        let num_chunks = data.len().div_ceil(chunk_size);

        // Split data into chunks
        let chunks: Vec<Bytes> = (0..num_chunks)
            .map(|i| {
                let start = i * chunk_size;
                let end = std::cmp::min(start + chunk_size, data.len());
                data.slice(start..end)
            })
            .collect();

        // Upload chunks in parallel
        let max_concurrent = self.config.max_concurrent_transfers;
        let mut bytes_transferred = 0u64;

        let chunk_futures: Vec<_> = chunks
            .into_iter()
            .enumerate()
            .map(|(i, chunk)| {
                let chunk_key = format!("{key}.part{i}");
                let storage = self.storage.clone();
                let chunk_len = chunk.len() as u64;

                async move {
                    let mut attempts = 0;
                    loop {
                        match storage.upload(&chunk_key, chunk.clone()).await {
                            Ok(()) => return Ok(chunk_len),
                            Err(e) if e.is_retryable() && attempts < self.config.max_retries => {
                                attempts += 1;
                                sleep(Duration::from_secs(2u64.pow(attempts))).await;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
            })
            .collect();

        let mut stream = stream::iter(chunk_futures).buffer_unordered(max_concurrent);

        while let Some(result) = stream.next().await {
            let chunk_len = result?;
            bytes_transferred += chunk_len;
            self.update_progress(transfer_id, bytes_transferred, total_size, &progress_tx)
                .await;
        }

        // Combine chunks (implementation-specific)
        // For now, we mark as complete
        self.complete_transfer(transfer_id);
        Ok(())
    }

    /// Download single part with retry
    async fn download_single_part(
        &self,
        key: &str,
        transfer_id: &str,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<Bytes> {
        let mut attempts = 0;

        loop {
            match self.storage.download(key).await {
                Ok(data) => {
                    let total_size = data.len() as u64;
                    self.update_progress(transfer_id, total_size, total_size, &progress_tx)
                        .await;
                    self.complete_transfer(transfer_id);
                    return Ok(data);
                }
                Err(e) if e.is_retryable() && attempts < self.config.max_retries => {
                    attempts += 1;
                    tracing::warn!("Download attempt {} failed: {}", attempts, e);
                    sleep(self.retry_delay(attempts)).await;
                }
                Err(e) => {
                    self.fail_transfer(transfer_id);
                    return Err(e);
                }
            }
        }
    }

    /// Download with multipart using byte ranges
    async fn download_multipart(
        &self,
        key: &str,
        total_size: u64,
        transfer_id: &str,
        progress_tx: Option<mpsc::Sender<TransferProgress>>,
    ) -> Result<Bytes> {
        let chunk_size = self.config.chunk_size as u64;
        let num_chunks = total_size.div_ceil(chunk_size);

        // Create range requests
        let ranges: Vec<(u64, u64)> = (0..num_chunks)
            .map(|i| {
                let start = i * chunk_size;
                let end = std::cmp::min(start + chunk_size - 1, total_size - 1);
                (start, end)
            })
            .collect();

        let max_concurrent = self.config.max_concurrent_transfers;
        let mut bytes_transferred = 0u64;

        // Download chunks in parallel
        let chunk_futures: Vec<_> = ranges
            .into_iter()
            .map(|(start, end)| {
                let storage = self.storage.clone();
                let key = key.to_string();

                async move {
                    let mut attempts = 0;
                    loop {
                        match storage.download_range(&key, start, end).await {
                            Ok(data) => return Ok((start, data)),
                            Err(e) if e.is_retryable() && attempts < self.config.max_retries => {
                                attempts += 1;
                                sleep(Duration::from_secs(2u64.pow(attempts))).await;
                            }
                            Err(e) => return Err(e),
                        }
                    }
                }
            })
            .collect();

        let mut stream = stream::iter(chunk_futures).buffer_unordered(max_concurrent);
        let mut chunks: Vec<(u64, Bytes)> = Vec::new();

        while let Some(result) = stream.next().await {
            let (offset, chunk) = result?;
            bytes_transferred += chunk.len() as u64;
            chunks.push((offset, chunk));
            self.update_progress(transfer_id, bytes_transferred, total_size, &progress_tx)
                .await;
        }

        // Sort chunks by offset and combine
        chunks.sort_by_key(|(offset, _)| *offset);
        let mut combined = BytesMut::with_capacity(total_size as usize);
        for (_, chunk) in chunks {
            combined.extend_from_slice(&chunk);
        }

        self.complete_transfer(transfer_id);
        Ok(combined.freeze())
    }

    /// Initialize a transfer
    fn init_transfer(&self, transfer_id: &str, total_size: u64) {
        let state = TransferState {
            total_size,
            bytes_transferred: 0,
            start_time: Instant::now(),
            status: TransferStatus::InProgress,
        };
        self.transfers
            .write()
            .insert(transfer_id.to_string(), state);
    }

    /// Update transfer progress
    async fn update_progress(
        &self,
        transfer_id: &str,
        bytes_transferred: u64,
        total_size: u64,
        progress_tx: &Option<mpsc::Sender<TransferProgress>>,
    ) {
        let (_elapsed, rate_bps, eta_secs) = {
            if let Some(state) = self.transfers.write().get_mut(transfer_id) {
                state.bytes_transferred = bytes_transferred;

                let elapsed = state.start_time.elapsed().as_secs_f64();
                let rate_bps = if elapsed > 0.0 {
                    bytes_transferred as f64 / elapsed
                } else {
                    0.0
                };

                let remaining_bytes = total_size.saturating_sub(bytes_transferred);
                let eta_secs = if rate_bps > 0.0 {
                    Some(remaining_bytes as f64 / rate_bps)
                } else {
                    None
                };
                (elapsed, rate_bps, eta_secs)
            } else {
                (0.0, 0.0, None)
            }
        };

        if let Some(tx) = progress_tx {
            let progress = TransferProgress {
                bytes_transferred,
                total_bytes: total_size,
                rate_bps,
                eta_secs,
            };
            let _ = tx.send(progress).await;
        }
    }

    /// Mark transfer as complete
    fn complete_transfer(&self, transfer_id: &str) {
        if let Some(state) = self.transfers.write().get_mut(transfer_id) {
            state.status = TransferStatus::Completed;
        }
    }

    /// Mark transfer as failed
    fn fail_transfer(&self, transfer_id: &str) {
        if let Some(state) = self.transfers.write().get_mut(transfer_id) {
            state.status = TransferStatus::Failed;
        }
    }

    /// Calculate retry delay with exponential backoff
    fn retry_delay(&self, attempt: u32) -> Duration {
        let base_delay = Duration::from_secs(1);
        let max_delay = Duration::from_secs(60);
        let delay = base_delay * 2u32.pow(attempt);
        std::cmp::min(delay, max_delay)
    }

    /// Get transfer status
    #[must_use]
    pub fn get_status(&self, transfer_id: &str) -> Option<TransferState> {
        self.transfers.read().get(transfer_id).cloned()
    }
}

/// Transfer configuration
#[derive(Debug, Clone)]
pub struct TransferConfig {
    /// Chunk size for multipart transfers
    pub chunk_size: usize,
    /// Maximum concurrent transfers
    pub max_concurrent_transfers: usize,
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Enable checksum verification
    pub verify_checksum: bool,
    /// Bandwidth limit in bytes per second (None = unlimited)
    pub bandwidth_limit_bps: Option<u64>,
}

impl Default for TransferConfig {
    fn default() -> Self {
        Self {
            chunk_size: 5 * 1024 * 1024, // 5 MB
            max_concurrent_transfers: 4,
            max_retries: 3,
            verify_checksum: true,
            bandwidth_limit_bps: None,
        }
    }
}

impl TransferConfig {
    /// Create configuration optimized for small files
    #[must_use]
    pub fn small_files() -> Self {
        Self {
            chunk_size: 1024 * 1024, // 1 MB
            max_concurrent_transfers: 8,
            max_retries: 3,
            verify_checksum: true,
            bandwidth_limit_bps: None,
        }
    }

    /// Create configuration optimized for large files
    #[must_use]
    pub fn large_files() -> Self {
        Self {
            chunk_size: 20 * 1024 * 1024, // 20 MB
            max_concurrent_transfers: 8,
            max_retries: 5,
            verify_checksum: true,
            bandwidth_limit_bps: None,
        }
    }
}

/// Transfer state
#[derive(Debug, Clone)]
pub struct TransferState {
    /// Total size in bytes
    pub total_size: u64,
    /// Bytes transferred
    pub bytes_transferred: u64,
    /// Start time
    pub start_time: Instant,
    /// Status
    pub status: TransferStatus,
}

/// Transfer status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferStatus {
    /// Transfer in progress
    InProgress,
    /// Transfer completed
    Completed,
    /// Transfer failed
    Failed,
    /// Transfer paused
    Paused,
}

/// Checksum calculator
pub struct ChecksumCalculator {
    /// MD5 hasher
    md5: md5::Md5,
    /// SHA256 hasher
    sha256: sha2::Sha256,
}

impl ChecksumCalculator {
    /// Create a new checksum calculator
    #[must_use]
    pub fn new() -> Self {
        use sha2::Digest;
        Self {
            md5: md5::Md5::new(),
            sha256: sha2::Sha256::new(),
        }
    }

    /// Update with data
    pub fn update(&mut self, data: &[u8]) {
        use sha2::Digest;
        self.md5.update(data);
        self.sha256.update(data);
    }

    /// Finalize and get checksums
    #[must_use]
    pub fn finalize(self) -> Checksums {
        use sha2::Digest;
        let md5_digest = self.md5.finalize();
        let sha256_digest = self.sha256.finalize();

        Checksums {
            md5: hex::encode(&md5_digest[..]),
            sha256: hex::encode(&sha256_digest[..]),
        }
    }
}

impl Default for ChecksumCalculator {
    fn default() -> Self {
        Self::new()
    }
}

/// Checksums for data verification
#[derive(Debug, Clone)]
pub struct Checksums {
    /// MD5 checksum
    pub md5: String,
    /// SHA256 checksum
    pub sha256: String,
}

impl Checksums {
    /// Verify MD5 checksum
    #[must_use]
    pub fn verify_md5(&self, expected: &str) -> bool {
        self.md5.eq_ignore_ascii_case(expected)
    }

    /// Verify SHA256 checksum
    #[must_use]
    pub fn verify_sha256(&self, expected: &str) -> bool {
        self.sha256.eq_ignore_ascii_case(expected)
    }
}

/// Retry policy for transient failure handling with exponential backoff.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of attempts (first attempt + retries).
    pub max_attempts: u32,
    /// Initial delay in milliseconds before the first retry.
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds (caps the exponential growth).
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 200,
            max_delay_ms: 30_000,
        }
    }
}

impl RetryPolicy {
    /// Compute the backoff delay for the given attempt index (0-based).
    ///
    /// `delay = min(base_delay_ms * 2^attempt, max_delay_ms)`
    #[must_use]
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let shift = attempt.min(62); // guard against overflow
        let multiplier = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
        let ms = self
            .base_delay_ms
            .saturating_mul(multiplier)
            .min(self.max_delay_ms);
        Duration::from_millis(ms)
    }
}

/// Execute a synchronous closure with exponential-backoff retries.
///
/// `op` is called up to `policy.max_attempts` times.  Between attempts the
/// calling thread sleeps for the computed backoff duration.  The closure
/// receives the current 0-based attempt index.
///
/// # Errors
///
/// Returns the last error produced by `op` if all attempts are exhausted.
pub fn execute_with_retry<F, T, E>(policy: &RetryPolicy, mut op: F) -> std::result::Result<T, E>
where
    F: FnMut(u32) -> std::result::Result<T, E>,
    E: std::fmt::Debug,
{
    // `RetryPolicy::max_attempts` is a plain `pub` field with no lower-bound
    // enforcement, so a caller-constructed policy could set it to 0. Clamp
    // to at least one attempt so the loop below is provably non-empty
    // instead of relying on an unenforced caller invariant.
    let max_attempts = policy.max_attempts.max(1);
    let mut last_err: Option<E> = None;
    for attempt in 0..max_attempts {
        match op(attempt) {
            Ok(value) => return Ok(value),
            Err(e) => {
                tracing::warn!("Attempt {} failed: {:?}", attempt + 1, e);
                last_err = Some(e);
                if attempt + 1 < max_attempts {
                    std::thread::sleep(policy.delay_for_attempt(attempt));
                }
            }
        }
    }
    // The loop above always runs at least once now (`max_attempts >= 1`),
    // so `last_err` is guaranteed `Some` here — the `Ok` arm above is the
    // only other way out of the function.
    Err(last_err.expect("max_attempts clamped to >= 1 above, so the loop ran at least once"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transfer_config_defaults() {
        let config = TransferConfig::default();
        assert_eq!(config.chunk_size, 5 * 1024 * 1024);
        assert_eq!(config.max_concurrent_transfers, 4);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_transfer_config_presets() {
        let small = TransferConfig::small_files();
        assert_eq!(small.chunk_size, 1024 * 1024);

        let large = TransferConfig::large_files();
        assert_eq!(large.chunk_size, 20 * 1024 * 1024);
    }

    #[test]
    fn test_checksum_calculator() {
        let mut calc = ChecksumCalculator::new();
        calc.update(b"test data");
        let checksums = calc.finalize();

        assert!(!checksums.md5.is_empty());
        assert!(!checksums.sha256.is_empty());
    }

    #[test]
    fn test_checksum_verification() {
        let mut calc = ChecksumCalculator::new();
        calc.update(b"test");
        let checksums = calc.finalize();

        // Known MD5 of "test"
        assert!(checksums.verify_md5("098f6bcd4621d373cade4e832627b4f6"));
    }

    #[test]
    fn test_transfer_status() {
        assert_eq!(TransferStatus::InProgress, TransferStatus::InProgress);
        assert_ne!(TransferStatus::InProgress, TransferStatus::Completed);
    }

    // ── RetryPolicy tests ────────────────────────────────────────────────────

    #[test]
    fn test_retry_policy_default() {
        let p = RetryPolicy::default();
        assert_eq!(p.max_attempts, 3);
        assert_eq!(p.base_delay_ms, 200);
        assert_eq!(p.max_delay_ms, 30_000);
    }

    #[test]
    fn test_retry_policy_delay_doubles() {
        let p = RetryPolicy {
            max_attempts: 5,
            base_delay_ms: 100,
            max_delay_ms: 10_000,
        };
        assert_eq!(p.delay_for_attempt(0), Duration::from_millis(100));
        assert_eq!(p.delay_for_attempt(1), Duration::from_millis(200));
        assert_eq!(p.delay_for_attempt(2), Duration::from_millis(400));
        assert_eq!(p.delay_for_attempt(3), Duration::from_millis(800));
    }

    #[test]
    fn test_retry_policy_delay_capped_at_max() {
        let p = RetryPolicy {
            max_attempts: 10,
            base_delay_ms: 1_000,
            max_delay_ms: 3_000,
        };
        // 1000 * 2^10 = 1_024_000 >> 3000, should be capped
        assert_eq!(p.delay_for_attempt(10), Duration::from_millis(3_000));
    }

    #[test]
    fn test_execute_with_retry_success_first_try() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let mut call_count = 0u32;
        let result: std::result::Result<i32, &str> = execute_with_retry(&policy, |_attempt| {
            call_count += 1;
            Ok(42)
        });
        assert_eq!(result, Ok(42));
        assert_eq!(call_count, 1);
    }

    #[test]
    fn test_execute_with_retry_succeeds_on_second_attempt() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let mut call_count = 0u32;
        let result: std::result::Result<i32, &str> = execute_with_retry(&policy, |_attempt| {
            call_count += 1;
            if call_count < 2 {
                Err("transient")
            } else {
                Ok(99)
            }
        });
        assert_eq!(result, Ok(99));
        assert_eq!(call_count, 2);
    }

    #[test]
    fn test_execute_with_retry_exhausts_attempts() {
        let policy = RetryPolicy {
            max_attempts: 3,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let mut call_count = 0u32;
        let result: std::result::Result<i32, &str> = execute_with_retry(&policy, |_attempt| {
            call_count += 1;
            Err("always fails")
        });
        assert!(result.is_err());
        assert_eq!(call_count, 3);
    }

    #[test]
    fn test_execute_with_retry_zero_max_attempts_errs_instead_of_panicking() {
        // `max_attempts` is a public field with no lower-bound enforcement;
        // a misconfigured policy of 0 must return an `Err`, not panic.
        let policy = RetryPolicy {
            max_attempts: 0,
            base_delay_ms: 0,
            max_delay_ms: 0,
        };
        let mut call_count = 0u32;
        let result: std::result::Result<i32, &str> = execute_with_retry(&policy, |_attempt| {
            call_count += 1;
            Err("always fails")
        });
        assert!(result.is_err());
        // Clamped to at least one attempt rather than silently doing nothing.
        assert_eq!(call_count, 1);
    }
}
