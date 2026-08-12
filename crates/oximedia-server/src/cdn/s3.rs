//! Amazon S3 CDN upload endpoint.
//!
//! Uploads media assets to an S3-compatible endpoint through the
//! `oximedia-storage` [`S3Storage`](oximedia_storage::s3::S3Storage) backend,
//! which implements the full
//! [`CloudStorage`](oximedia_storage::CloudStorage) trait (single-part and
//! multipart uploads, deletes, prefix listing, presigned URLs).
//!
//! ## Feature gating — no silent no-ops
//!
//! The real network backend is enabled by the `cdn-aws` Cargo feature, which
//! transitively turns on `oximedia-storage/s3`.  **When the feature is disabled
//! every remote operation returns [`CdnError::FeatureDisabled`]**, naming the
//! missing feature.  Nothing is uploaded, deleted or listed, and — critically —
//! no object URL is synthesised for an object that was never uploaded.  Earlier
//! revisions returned a valid-looking URL from a log-only fallback; that was a
//! fabrication and has been removed.
//!
//! [`S3CdnUploader::object_url`] still computes the URL an object *would* have,
//! but it is an explicit, side-effect-free address calculation — it never
//! implies the object exists.
//!
//! ## Multipart upload
//!
//! Uploads whose byte length is at or above
//! [`MultipartConfig::threshold_bytes`] (default: 8 MiB) are split into
//! fixed-size chunks of [`MultipartConfig::part_size`] bytes (default: 8 MiB)
//! by [`partition_into_parts`] and fed to
//! `S3Storage::upload_stream` as a chunked byte stream; the storage layer owns
//! the AWS multipart wire protocol (initiate / upload_part / complete / abort)
//! and the per-part parallelism.  Smaller payloads are sent as a single chunk.
//!
//! [`MultipartConfig::retry_attempts`] and
//! [`MultipartConfig::retry_backoff_ms`] apply to the whole upload call: a
//! failed `upload_stream` is retried with the configured back-off, and the
//! final failure is reported as [`CdnError::UploadFailed`].

use crate::cdn::CdnConfig;
use crate::error::ServerResult;
use std::path::Path;
use tracing::info;

#[cfg(feature = "cdn-aws")]
use bytes::Bytes;
#[cfg(feature = "cdn-aws")]
use oximedia_storage::{s3::S3Storage, CloudStorage, ListOptions, UnifiedConfig, UploadOptions};
#[cfg(feature = "cdn-aws")]
use std::sync::Arc;

/// Name of the Cargo feature that enables the real S3 backend.
pub const S3_FEATURE: &str = "cdn-aws";

// ── Error type ───────────────────────────────────────────────────────────────

/// Error type specific to CDN upload operations.
#[derive(Debug, thiserror::Error)]
pub enum CdnError {
    /// The underlying storage backend returned an error.
    #[error("Storage error: {0}")]
    Storage(String),
    /// An I/O error occurred while reading the local file.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The object key is invalid (empty, contains forbidden characters, etc.).
    #[error("Invalid key: {0}")]
    InvalidKey(String),
    /// The upload failed after exhausting every retry attempt.
    #[error("Upload of `{key}` failed after {attempts} attempt(s): {cause}")]
    UploadFailed {
        /// Object key that failed to upload.
        key: String,
        /// Number of attempts made.
        attempts: usize,
        /// Underlying error description.
        cause: String,
    },
    /// The Cargo feature carrying the real backend is disabled, so **no**
    /// network operation was performed.
    ///
    /// This is deliberately an error and not a silent success: returning `Ok`
    /// (or worse, a plausible object URL) for an object that was never uploaded
    /// would be a fabrication.
    #[error(
        "{operation} requires the `{feature}` Cargo feature of oximedia-server; \
         the feature is disabled, so nothing was uploaded, deleted or listed"
    )]
    FeatureDisabled {
        /// Human-readable operation name, e.g. `"S3 CDN upload"`.
        operation: &'static str,
        /// Cargo feature that must be enabled, e.g. `"cdn-aws"`.
        feature: &'static str,
    },
}

impl From<CdnError> for crate::error::ServerError {
    fn from(e: CdnError) -> Self {
        Self::Internal(e.to_string())
    }
}

// ── MultipartConfig ──────────────────────────────────────────────────────────

/// Configuration for S3 multipart uploads.
///
/// Payloads at or above [`threshold_bytes`](Self::threshold_bytes) (default:
/// 8 MiB) are chunked into [`part_size`](Self::part_size) pieces and streamed
/// to the storage layer, which performs the S3 multipart protocol.  Smaller
/// payloads are streamed as a single chunk.
///
/// The 10 000-part S3 limit means the maximum object size at the default
/// `part_size` of 8 MiB is 80 GiB; larger objects need a larger `part_size`.
#[derive(Debug, Clone)]
pub struct MultipartConfig {
    /// Minimum payload size (inclusive) that triggers chunked/multipart upload.
    ///
    /// Default: 8 MiB.
    pub threshold_bytes: usize,
    /// Target size for each streamed chunk.
    ///
    /// The last chunk may be smaller.  Must be greater than zero.
    /// Default: 8 MiB.
    pub part_size: usize,
    /// Advisory upper bound on concurrent part uploads (currently inert).
    ///
    /// The AWS multipart protocol is executed by `oximedia-storage`, which owns
    /// the actual concurrency; this value is carried for callers and future
    /// backends that expose a tunable.  Default: 4.
    pub max_parallel_parts: usize,
    /// Maximum number of attempts for the whole upload (first attempt + retries).
    ///
    /// Default: 3.
    pub retry_attempts: usize,
    /// Sleep duration in milliseconds before each retry attempt.
    ///
    /// `retry_backoff_ms[i]` is the delay before the `(i+1)`-th retry.  If the
    /// slice is shorter than `retry_attempts - 1` the last entry is reused.
    /// Default: `[500, 1000, 2000]`.
    pub retry_backoff_ms: Vec<u64>,
}

impl Default for MultipartConfig {
    fn default() -> Self {
        Self {
            threshold_bytes: 8 * 1024 * 1024,
            part_size: 8 * 1024 * 1024,
            max_parallel_parts: 4,
            retry_attempts: 3,
            retry_backoff_ms: vec![500, 1000, 2000],
        }
    }
}

// ── Pure helper: partition ───────────────────────────────────────────────────

/// Split `data` into contiguous slices of at most `part_size` bytes.
///
/// The final slice may be smaller.  Returns an empty `Vec` only if `data` is
/// empty.
///
/// # Panics
///
/// Panics if `part_size` is zero.  Callers that take `part_size` from
/// configuration must validate it first; [`S3CdnUploader::upload_bytes_with_config`]
/// rejects a zero `part_size` with [`CdnError::Storage`] before reaching here.
#[must_use]
pub fn partition_into_parts(data: &[u8], part_size: usize) -> Vec<&[u8]> {
    assert!(part_size > 0, "part_size must be > 0");
    data.chunks(part_size).collect()
}

// ── S3CdnUploader ────────────────────────────────────────────────────────────

/// S3 CDN uploader.
///
/// Wraps the `oximedia-storage` `S3Storage`
/// (`oximedia_storage::s3::S3Storage`, requires the `s3` feature on
/// `oximedia-storage`) backend to upload media assets to an S3-compatible
/// endpoint.
///
/// Under the `cdn-aws` feature every operation performs real network I/O.
/// Without it, every operation fails with [`CdnError::FeatureDisabled`]; see
/// [`is_live`](Self::is_live).
pub struct S3CdnUploader {
    /// Bucket name.
    bucket: String,

    /// Region identifier.
    region: String,

    /// Base path prefix within the bucket.
    base_path: String,

    /// Public delivery domain, used in place of the raw bucket endpoint when
    /// [`CdnConfig::enable_cdn`] is set and a domain is configured.
    cdn_domain: Option<String>,

    /// Real S3 backend (present only when the `cdn-aws` feature is enabled).
    #[cfg(feature = "cdn-aws")]
    backend: Arc<S3Storage>,
}

impl S3CdnUploader {
    /// Creates a new `S3CdnUploader` from CDN configuration.
    ///
    /// Under the `cdn-aws` feature this constructs a real `S3Storage` client
    /// from the supplied credentials and region.  Without the feature the
    /// uploader is inert: it can compute object URLs but every remote operation
    /// returns [`CdnError::FeatureDisabled`].
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage client cannot be created.
    pub async fn new(config: &CdnConfig) -> ServerResult<Self> {
        info!(
            bucket = %config.bucket,
            region = %config.region,
            live = cfg!(feature = "cdn-aws"),
            "S3CdnUploader: initialising"
        );

        #[cfg(feature = "cdn-aws")]
        let backend = {
            let mut unified = UnifiedConfig::s3(config.bucket.clone(), config.region.clone());
            if !config.access_key.is_empty() || !config.secret_key.is_empty() {
                unified =
                    unified.with_credentials(config.access_key.clone(), config.secret_key.clone());
            }
            let storage = S3Storage::new(unified)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            Arc::new(storage)
        };

        Ok(Self {
            bucket: config.bucket.clone(),
            region: config.region.clone(),
            base_path: config.base_path.clone(),
            cdn_domain: if config.enable_cdn {
                config.cdn_domain.clone().filter(|d| !d.is_empty())
            } else {
                None
            },
            #[cfg(feature = "cdn-aws")]
            backend,
        })
    }

    /// Whether this uploader is backed by a real S3 client.
    ///
    /// `false` on builds without the `cdn-aws` feature; every remote operation
    /// then returns [`CdnError::FeatureDisabled`].
    #[must_use]
    pub const fn is_live(&self) -> bool {
        cfg!(feature = "cdn-aws")
    }

    /// Prefix `key` with the configured base path to form the full object key.
    fn object_key(&self, key: &str) -> String {
        if self.base_path.is_empty() {
            key.to_string()
        } else {
            format!("{}/{}", self.base_path.trim_end_matches('/'), key)
        }
    }

    /// Computes the public URL an object with `key` would have.
    ///
    /// This is a pure address calculation: it performs no I/O and makes **no**
    /// claim that the object exists or was uploaded.  When
    /// [`CdnConfig::enable_cdn`] is set with a [`CdnConfig::cdn_domain`], the
    /// CDN domain is used in place of the raw bucket endpoint.
    #[must_use]
    pub fn object_url(&self, key: &str) -> String {
        let path = self.object_key(key);
        match &self.cdn_domain {
            Some(domain) => format!("https://{}/{}", domain.trim_end_matches('/'), path),
            None => format!(
                "https://{}.s3.{}.amazonaws.com/{}",
                self.bucket, self.region, path
            ),
        }
    }

    /// Validate that `key` is acceptable for S3.
    fn validate_key(key: &str) -> std::result::Result<(), CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }
        if key.contains("..") {
            return Err(CdnError::InvalidKey(format!(
                "Key contains path traversal: {key}"
            )));
        }
        Ok(())
    }

    /// Upload a local file to S3, selecting single-chunk or multipart upload
    /// automatically based on the supplied [`MultipartConfig`] threshold.
    ///
    /// Returns the public URL of the uploaded object.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`] for malformed keys, [`CdnError::Io`] if
    /// the file cannot be read, [`CdnError::Storage`] / [`CdnError::UploadFailed`]
    /// if the upload fails, or [`CdnError::FeatureDisabled`] on builds without
    /// the `cdn-aws` feature.
    pub async fn upload(
        &self,
        local_path: &Path,
        key: &str,
    ) -> std::result::Result<String, CdnError> {
        self.upload_with_config(local_path, key, &MultipartConfig::default())
            .await
    }

    /// Like [`upload`](Self::upload) but with an explicit [`MultipartConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::Io`], [`CdnError::Storage`],
    /// [`CdnError::UploadFailed`], or [`CdnError::FeatureDisabled`].
    pub async fn upload_with_config(
        &self,
        local_path: &Path,
        key: &str,
        config: &MultipartConfig,
    ) -> std::result::Result<String, CdnError> {
        Self::validate_key(key)?;
        let _ = config;

        #[cfg(feature = "cdn-aws")]
        {
            let object_key = self.object_key(key);
            self.backend
                .upload_file(&object_key, local_path, UploadOptions::default())
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            info!(
                bucket = %self.bucket,
                key = %object_key,
                path = %local_path.display(),
                "S3CdnUploader: file upload complete"
            );
            Ok(self.object_url(key))
        }

        // No feature: fail before touching the file — reading bytes we can
        // never upload would only mislead.
        #[cfg(not(feature = "cdn-aws"))]
        {
            let _ = local_path;
            Err(CdnError::FeatureDisabled {
                operation: "S3 CDN file upload",
                feature: S3_FEATURE,
            })
        }
    }

    /// Upload raw bytes to S3 using the default [`MultipartConfig`].
    ///
    /// Returns the public URL of the uploaded object.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::UploadFailed`], or
    /// [`CdnError::FeatureDisabled`].
    pub async fn upload_bytes(
        &self,
        data: &[u8],
        key: &str,
    ) -> std::result::Result<String, CdnError> {
        self.upload_bytes_with_config(data, key, &MultipartConfig::default())
            .await
    }

    /// Like [`upload_bytes`](Self::upload_bytes) but with an explicit
    /// [`MultipartConfig`].
    ///
    /// Payloads below [`MultipartConfig::threshold_bytes`] are streamed as a
    /// single chunk; larger payloads are chunked at
    /// [`MultipartConfig::part_size`] and streamed to the storage layer, which
    /// runs the S3 multipart protocol.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::Storage`] (zero
    /// `part_size`), [`CdnError::UploadFailed`], or [`CdnError::FeatureDisabled`].
    pub async fn upload_bytes_with_config(
        &self,
        data: &[u8],
        key: &str,
        config: &MultipartConfig,
    ) -> std::result::Result<String, CdnError> {
        Self::validate_key(key)?;
        if config.part_size == 0 {
            return Err(CdnError::Storage(
                "MultipartConfig::part_size must be greater than zero".to_string(),
            ));
        }

        #[cfg(feature = "cdn-aws")]
        {
            let chunks: Vec<Bytes> = if data.len() >= config.threshold_bytes {
                partition_into_parts(data, config.part_size)
                    .into_iter()
                    .map(Bytes::copy_from_slice)
                    .collect()
            } else {
                vec![Bytes::copy_from_slice(data)]
            };
            info!(
                bucket = %self.bucket,
                key = %key,
                bytes = data.len(),
                chunks = chunks.len(),
                "S3CdnUploader: beginning upload"
            );
            self.put_chunks_with_retry(key, &chunks, data.len() as u64, config)
                .await?;
            Ok(self.object_url(key))
        }

        #[cfg(not(feature = "cdn-aws"))]
        {
            let _ = data;
            Err(CdnError::FeatureDisabled {
                operation: "S3 CDN upload",
                feature: S3_FEATURE,
            })
        }
    }

    /// Stream `chunks` to the storage layer, retrying the whole upload on
    /// failure according to `config`.
    #[cfg(feature = "cdn-aws")]
    async fn put_chunks_with_retry(
        &self,
        key: &str,
        chunks: &[Bytes],
        total_size: u64,
        config: &MultipartConfig,
    ) -> std::result::Result<(), CdnError> {
        let object_key = self.object_key(key);
        let attempts = config.retry_attempts.max(1);
        let mut last_err = String::new();

        for attempt in 0..attempts {
            if attempt > 0 {
                let delay_idx = (attempt - 1).min(config.retry_backoff_ms.len().saturating_sub(1));
                let delay_ms = config
                    .retry_backoff_ms
                    .get(delay_idx)
                    .copied()
                    .unwrap_or(2000);
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                info!(
                    key = %object_key,
                    attempt = attempt + 1,
                    "S3CdnUploader: retrying upload"
                );
            }

            // `ByteStream` is `'static`, so the stream must own its chunks.
            // Cloning a `Bytes` is a refcount bump, not a data copy.
            let owned: Vec<Bytes> = chunks.to_vec();
            let stream = futures::stream::iter(
                owned
                    .into_iter()
                    .map(Ok::<Bytes, oximedia_storage::StorageError>),
            );
            match self
                .backend
                .upload_stream(
                    &object_key,
                    Box::pin(stream),
                    Some(total_size),
                    UploadOptions::default(),
                )
                .await
            {
                Ok(_) => {
                    info!(
                        bucket = %self.bucket,
                        key = %object_key,
                        chunks = chunks.len(),
                        bytes = total_size,
                        "S3CdnUploader: upload complete"
                    );
                    return Ok(());
                }
                Err(e) => last_err = e.to_string(),
            }
        }

        Err(CdnError::UploadFailed {
            key: object_key,
            attempts,
            cause: last_err,
        })
    }

    /// Generates a presigned URL for downloading an object.
    ///
    /// Delegates to the storage backend's real signing implementation; the
    /// returned URL carries a valid signature.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::Storage`] if signing
    /// fails, or [`CdnError::FeatureDisabled`] on builds without the `cdn-aws`
    /// feature (an unsigned URL would not be a presigned URL).
    pub async fn presigned_url(
        &self,
        key: &str,
        expires_in_secs: u64,
    ) -> std::result::Result<String, CdnError> {
        Self::validate_key(key)?;

        #[cfg(feature = "cdn-aws")]
        {
            let object_key = self.object_key(key);
            self.backend
                .generate_presigned_url(&object_key, expires_in_secs)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))
        }

        #[cfg(not(feature = "cdn-aws"))]
        {
            let _ = expires_in_secs;
            Err(CdnError::FeatureDisabled {
                operation: "S3 presigned URL generation",
                feature: S3_FEATURE,
            })
        }
    }

    /// Deletes an object from S3.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`] for a malformed key,
    /// [`CdnError::Storage`] if the delete request fails, or
    /// [`CdnError::FeatureDisabled`] on builds without the `cdn-aws` feature.
    pub async fn delete(&self, key: &str) -> std::result::Result<(), CdnError> {
        Self::validate_key(key)?;

        #[cfg(feature = "cdn-aws")]
        {
            let object_key = self.object_key(key);
            self.backend
                .delete_object(&object_key)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            info!(bucket = %self.bucket, key = %object_key, "S3CdnUploader: delete complete");
            Ok(())
        }

        #[cfg(not(feature = "cdn-aws"))]
        {
            Err(CdnError::FeatureDisabled {
                operation: "S3 CDN delete",
                feature: S3_FEATURE,
            })
        }
    }

    /// Lists objects with a given prefix.
    ///
    /// The supplied `prefix` is resolved relative to the configured base path.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::Storage`] if the listing request fails, or
    /// [`CdnError::FeatureDisabled`] on builds without the `cdn-aws` feature
    /// (an empty list would falsely claim the bucket holds no such objects).
    pub async fn list(&self, prefix: &str) -> std::result::Result<Vec<String>, CdnError> {
        #[cfg(feature = "cdn-aws")]
        {
            let full_prefix = self.object_key(prefix);
            let result = self
                .backend
                .list_objects(ListOptions {
                    prefix: Some(full_prefix),
                    ..ListOptions::default()
                })
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            let keys: Vec<String> = result.objects.into_iter().map(|o| o.key).collect();
            info!(
                bucket = %self.bucket,
                prefix = %prefix,
                count = keys.len(),
                "S3CdnUploader: list complete"
            );
            Ok(keys)
        }

        #[cfg(not(feature = "cdn-aws"))]
        {
            let _ = prefix;
            Err(CdnError::FeatureDisabled {
                operation: "S3 CDN listing",
                feature: S3_FEATURE,
            })
        }
    }
}

// ── Top-level convenience function ───────────────────────────────────────────

/// Upload data to S3, automatically chunking large payloads for multipart.
///
/// Payloads whose size is at or above `config.threshold_bytes` are chunked at
/// `config.part_size` and streamed to the S3 multipart API; smaller payloads
/// are sent as a single chunk.
///
/// # Errors
///
/// Propagates [`CdnError`] from the underlying uploader, including
/// [`CdnError::FeatureDisabled`] when `cdn-aws` is off.
pub async fn upload_to_s3(
    uploader: &S3CdnUploader,
    key: &str,
    data: &[u8],
    config: &MultipartConfig,
) -> std::result::Result<String, CdnError> {
    uploader.upload_bytes_with_config(data, key, config).await
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── partition_into_parts ──────────────────────────────────────────────────

    #[test]
    fn partition_exact_multiple() {
        let data = vec![0u8; 32 * 1024 * 1024]; // 32 MiB
        let parts = partition_into_parts(&data, 8 * 1024 * 1024);
        assert_eq!(parts.len(), 4, "32 MiB / 8 MiB = 4 parts");
        for (i, p) in parts.iter().enumerate() {
            assert_eq!(p.len(), 8 * 1024 * 1024, "part {i} must be exactly 8 MiB");
        }
    }

    #[test]
    fn partition_with_remainder() {
        // 25 MiB: 3 × 8 MiB + 1 MiB remainder
        let data = vec![1u8; 25 * 1024 * 1024];
        let parts = partition_into_parts(&data, 8 * 1024 * 1024);
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].len(), 8 * 1024 * 1024);
        assert_eq!(parts[1].len(), 8 * 1024 * 1024);
        assert_eq!(parts[2].len(), 8 * 1024 * 1024);
        assert_eq!(parts[3].len(), 1024 * 1024);
    }

    #[test]
    fn partition_smaller_than_part_size() {
        let data = vec![2u8; 1024];
        let parts = partition_into_parts(&data, 8 * 1024 * 1024);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].len(), 1024);
    }

    #[test]
    fn partition_empty() {
        let data: Vec<u8> = Vec::new();
        let parts = partition_into_parts(&data, 8 * 1024 * 1024);
        assert!(parts.is_empty());
    }

    #[test]
    fn partition_content_preserved() {
        let data: Vec<u8> = (0u8..=255).cycle().take(20 * 1024 * 1024).collect();
        let parts = partition_into_parts(&data, 8 * 1024 * 1024);
        let reassembled: Vec<u8> = parts.iter().flat_map(|p| p.iter().copied()).collect();
        assert_eq!(reassembled, data, "reassembled data must be byte-identical");
    }

    // ── MultipartConfig defaults ──────────────────────────────────────────────

    #[test]
    fn multipart_config_defaults() {
        let cfg = MultipartConfig::default();
        assert_eq!(cfg.threshold_bytes, 8 * 1024 * 1024);
        assert_eq!(cfg.part_size, 8 * 1024 * 1024);
        assert_eq!(cfg.max_parallel_parts, 4);
        assert_eq!(cfg.retry_attempts, 3);
        assert_eq!(cfg.retry_backoff_ms, vec![500, 1000, 2000]);
    }

    // ── URL calculation (feature-independent, pure) ───────────────────────────

    /// Only the feature-off tests build an uploader without credentials; with
    /// `cdn-aws` on, construction would need a real AWS client.
    #[cfg(not(feature = "cdn-aws"))]
    fn test_config() -> CdnConfig {
        use crate::cdn::CdnBackend;
        CdnConfig {
            backend: CdnBackend::S3,
            bucket: "test-bucket".to_string(),
            region: "us-east-1".to_string(),
            access_key: String::new(),
            secret_key: String::new(),
            base_path: "media".to_string(),
            public: true,
            enable_cdn: false,
            cdn_domain: None,
            project_id: None,
        }
    }

    #[tokio::test]
    #[cfg(not(feature = "cdn-aws"))]
    async fn object_url_uses_bucket_endpoint_without_cdn_domain() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        assert_eq!(
            uploader.object_url("clips/a.mp4"),
            "https://test-bucket.s3.us-east-1.amazonaws.com/media/clips/a.mp4"
        );
    }

    #[tokio::test]
    #[cfg(not(feature = "cdn-aws"))]
    async fn object_url_uses_cdn_domain_when_enabled() {
        let mut config = test_config();
        config.enable_cdn = true;
        config.cdn_domain = Some("cdn.example.com".to_string());
        let uploader = S3CdnUploader::new(&config).await.expect("init");
        assert_eq!(
            uploader.object_url("clips/a.mp4"),
            "https://cdn.example.com/media/clips/a.mp4"
        );
    }

    // ── Feature-off honesty: never Ok(url) for a non-uploaded object ──────────

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn uploader_is_not_live_without_feature() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        assert!(!uploader.is_live());
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn small_upload_is_honest_error_without_feature() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        let data = vec![0u8; 1024 * 1024]; // 1 MiB — below the 8 MiB threshold
        let err = uploader
            .upload_bytes_with_config(&data, "clips/small.mp4", &MultipartConfig::default())
            .await
            .expect_err("must not claim success without cdn-aws");
        let msg = err.to_string();
        assert!(
            msg.contains("cdn-aws"),
            "error must name the feature: {msg}"
        );
        assert!(
            !msg.contains("amazonaws.com"),
            "error must not hand back an object URL: {msg}"
        );
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn multipart_upload_is_honest_error_without_feature() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        let data = vec![0xABu8; 32 * 1024 * 1024]; // 32 MiB → would be 4 parts
        let cfg = MultipartConfig {
            retry_backoff_ms: vec![0, 0, 0],
            ..MultipartConfig::default()
        };
        let err = uploader
            .upload_bytes_with_config(&data, "large/video.mp4", &cfg)
            .await
            .expect_err("must not claim success without cdn-aws");
        assert!(err.to_string().contains("cdn-aws"));

        // The pure partition helper still agrees with the documented chunking.
        let parts = partition_into_parts(&data, cfg.part_size);
        assert_eq!(parts.len(), 4, "32 MiB / 8 MiB = 4 parts");
        let reassembled: Vec<u8> = parts.iter().flat_map(|p| p.iter().copied()).collect();
        assert_eq!(reassembled, data, "assembled data byte-identical to source");
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn upload_to_s3_helper_propagates_feature_error() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        let data = vec![0u8; 512];
        let err = upload_to_s3(&uploader, "tiny.bin", &data, &MultipartConfig::default())
            .await
            .expect_err("helper must propagate the honest error");
        assert!(err.to_string().contains("cdn-aws"));
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn file_upload_does_not_read_file_without_feature() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        // Path deliberately does not exist: the feature check must fire first,
        // so we get FeatureDisabled and not an I/O error.
        let missing = std::env::temp_dir().join("oximedia-cdn-does-not-exist.bin");
        let err = uploader
            .upload(&missing, "clips/x.mp4")
            .await
            .expect_err("must not claim success without cdn-aws");
        assert!(err.to_string().contains("cdn-aws"), "{err}");
    }

    // ── Key validation still applies before the feature check ────────────────

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn empty_key_rejected() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        let err = uploader
            .upload_bytes(b"data", "")
            .await
            .expect_err("empty key");
        assert!(matches!(err, CdnError::InvalidKey(_)));
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn path_traversal_key_rejected() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        let err = uploader
            .upload_bytes(b"data", "../escape.mp4")
            .await
            .expect_err("traversal key");
        assert!(matches!(err, CdnError::InvalidKey(_)));
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn zero_part_size_rejected_without_panicking() {
        let uploader = S3CdnUploader::new(&test_config()).await.expect("init");
        let cfg = MultipartConfig {
            part_size: 0,
            ..MultipartConfig::default()
        };
        let err = uploader
            .upload_bytes_with_config(b"data", "k.bin", &cfg)
            .await
            .expect_err("zero part_size");
        assert!(matches!(err, CdnError::Storage(_)), "{err}");
    }
}
