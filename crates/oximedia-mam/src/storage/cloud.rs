//! Cloud storage backends: real delegation to `oximedia-storage`.
//!
//! `S3Storage` / `AzureStorage` / `GCSStorage` implement the legacy
//! [`StorageBackend`] trait (used by `StorageManager`) by delegating to the
//! real, feature-gated backends in `oximedia-storage` — the same backends
//! `MamStorage::from_uri` (in the parent module) wires up. When the matching
//! Cargo feature is disabled, every method returns an honest `Err`;
//! `exists()` never fabricates `true`.

use async_trait::async_trait;
use chrono::Utc;
use std::path::Path;
#[cfg(feature = "gcs")]
use std::path::PathBuf;
use tokio::sync::OnceCell;

use crate::{MamError, Result};

use super::{StorageBackend, StorageBackendType, StorageMetadata, StorageStatistics};

/// Convert a low-level `oximedia_storage` error into a `MamError`.
fn storage_err(e: oximedia_storage::StorageError) -> MamError {
    MamError::Internal(format!("cloud storage backend error: {e}"))
}

/// Page through `backend.list_objects` under `prefix` (whole bucket when
/// `None`), returning every object's real metadata. Shared by `list_via`
/// and `statistics_via` so both report enumerated data, not a placeholder.
async fn list_all_objects(
    backend: &dyn oximedia_storage::CloudStorage,
    prefix: Option<&str>,
) -> Result<Vec<oximedia_storage::ObjectMetadata>> {
    let mut objects = Vec::new();
    let mut continuation_token: Option<String> = None;
    loop {
        let options = oximedia_storage::ListOptions {
            prefix: prefix.map(String::from),
            delimiter: None,
            max_results: Some(1000),
            continuation_token: continuation_token.take(),
        };
        let result = backend.list_objects(options).await.map_err(storage_err)?;
        objects.extend(result.objects);
        if !result.has_more {
            break;
        }
        match result.next_token {
            Some(token) => continuation_token = Some(token),
            None => break,
        }
    }
    Ok(objects)
}

/// Upload `local_path`'s real bytes via `backend`, reporting the local
/// file's real size and the backend's real ETag (never a fabricated `0`).
async fn upload_via(
    backend: &dyn oximedia_storage::CloudStorage,
    local_path: &Path,
    remote_path: &str,
) -> Result<StorageMetadata> {
    let local_meta = tokio::fs::metadata(local_path).await?;
    let options = oximedia_storage::UploadOptions::default();
    let etag = backend
        .upload_file(remote_path, local_path, options)
        .await
        .map_err(storage_err)?;
    Ok(StorageMetadata {
        path: remote_path.to_string(),
        size: local_meta.len(),
        content_type: None,
        checksum: Some(etag).filter(|s| !s.is_empty()),
        created_at: Some(Utc::now()),
        updated_at: Some(Utc::now()),
    })
}

/// Download `remote_path`'s real bytes via `backend`. Creates the
/// destination's parent directory first: S3/GCS/Azure's `download_file`
/// call `File::create` directly and do not create it themselves.
async fn download_via(
    backend: &dyn oximedia_storage::CloudStorage,
    remote_path: &str,
    local_path: &Path,
) -> Result<()> {
    if let Some(parent) = local_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    backend
        .download_file(
            remote_path,
            local_path,
            oximedia_storage::DownloadOptions::default(),
        )
        .await
        .map_err(storage_err)
}

/// Delete `remote_path` via `backend`.
async fn delete_via(backend: &dyn oximedia_storage::CloudStorage, remote_path: &str) -> Result<()> {
    backend
        .delete_object(remote_path)
        .await
        .map_err(storage_err)
}

/// Check whether `remote_path` really exists in `backend` — a real
/// HEAD-style lookup, never an unconditional `true`.
async fn exists_via(
    backend: &dyn oximedia_storage::CloudStorage,
    remote_path: &str,
) -> Result<bool> {
    backend
        .object_exists(remote_path)
        .await
        .map_err(storage_err)
}

/// Fetch `remote_path`'s real metadata via `backend`.
async fn metadata_via(
    backend: &dyn oximedia_storage::CloudStorage,
    remote_path: &str,
) -> Result<StorageMetadata> {
    let m = backend
        .get_metadata(remote_path)
        .await
        .map_err(storage_err)?;
    Ok(StorageMetadata {
        path: m.key,
        size: m.size,
        content_type: m.content_type,
        checksum: m.etag,
        created_at: None,
        updated_at: Some(m.last_modified),
    })
}

/// List every real key under `prefix` via `backend`.
async fn list_via(
    backend: &dyn oximedia_storage::CloudStorage,
    prefix: &str,
) -> Result<Vec<String>> {
    let objects = list_all_objects(backend, Some(prefix)).await?;
    Ok(objects.into_iter().map(|o| o.key).collect())
}

/// Compute real bucket-wide statistics for `backend` by enumerating every
/// object. `available_space`/`used_space` are genuinely unknown for cloud
/// object storage via this API, so they are reported as `None`, not `0`.
async fn statistics_via(backend: &dyn oximedia_storage::CloudStorage) -> Result<StorageStatistics> {
    let objects = list_all_objects(backend, None).await?;
    Ok(StorageStatistics {
        total_files: objects.len() as u64,
        total_size: objects.iter().map(|o| o.size).sum(),
        available_space: None,
        used_space: None,
    })
}

/// Amazon S3 storage backend.
///
/// Delegates to the real `oximedia_storage::s3::S3Storage` client when the
/// `s3` feature is enabled; the client connects lazily on first use and is
/// cached for the lifetime of this value. Without the `s3` feature, every
/// method returns an honest [`MamError::Internal`] instead of a fabricated
/// success.
#[allow(dead_code)]
pub struct S3Storage {
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    backend_cell: OnceCell<Box<dyn oximedia_storage::CloudStorage>>,
}

impl S3Storage {
    /// Create a new S3 storage backend. The real client is not connected
    /// until the first operation is performed.
    #[must_use]
    pub fn new(bucket: String, region: String, access_key: String, secret_key: String) -> Self {
        Self {
            bucket,
            region,
            access_key,
            secret_key,
            backend_cell: OnceCell::new(),
        }
    }

    /// Test-only constructor that injects an already-built `CloudStorage`
    /// backend (e.g. `oximedia_storage::local::LocalStorage`), so the real
    /// `StorageBackend` delegation below can round-trip real bytes without
    /// network access or the `s3` feature.
    #[cfg(test)]
    fn with_backend_for_test(backend: Box<dyn oximedia_storage::CloudStorage>) -> Self {
        Self {
            bucket: "test-bucket".to_string(),
            region: "us-east-1".to_string(),
            access_key: String::new(),
            secret_key: String::new(),
            backend_cell: OnceCell::new_with(Some(backend)),
        }
    }

    /// Resolve the real S3 backend, connecting (and caching the connection)
    /// on first use.
    ///
    /// # Errors
    ///
    /// Returns an error if the `s3` feature is disabled, or if the
    /// underlying `oximedia-storage` S3 client fails to initialize.
    async fn backend(&self) -> Result<&dyn oximedia_storage::CloudStorage> {
        if let Some(backend) = self.backend_cell.get() {
            return Ok(backend.as_ref());
        }
        #[cfg(feature = "s3")]
        {
            let backend = self
                .backend_cell
                .get_or_try_init(|| async {
                    let config = oximedia_storage::UnifiedConfig::s3(
                        self.bucket.clone(),
                        self.region.clone(),
                    )
                    .with_credentials(self.access_key.clone(), self.secret_key.clone());
                    let storage = oximedia_storage::s3::S3Storage::new(config).await?;
                    let boxed: Box<dyn oximedia_storage::CloudStorage> = Box::new(storage);
                    Ok::<_, oximedia_storage::StorageError>(boxed)
                })
                .await
                .map_err(storage_err)?;
            Ok(backend.as_ref())
        }
        #[cfg(not(feature = "s3"))]
        {
            Err(MamError::Internal(format!(
                "S3 backend requires the \"s3\" feature (bucket: {})",
                self.bucket
            )))
        }
    }
}

#[async_trait]
impl StorageBackend for S3Storage {
    async fn upload(&self, local_path: &Path, remote_path: &str) -> Result<StorageMetadata> {
        upload_via(self.backend().await?, local_path, remote_path).await
    }

    async fn download(&self, remote_path: &str, local_path: &Path) -> Result<()> {
        download_via(self.backend().await?, remote_path, local_path).await
    }

    async fn delete(&self, remote_path: &str) -> Result<()> {
        delete_via(self.backend().await?, remote_path).await
    }

    async fn exists(&self, remote_path: &str) -> Result<bool> {
        exists_via(self.backend().await?, remote_path).await
    }

    async fn metadata(&self, remote_path: &str) -> Result<StorageMetadata> {
        metadata_via(self.backend().await?, remote_path).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        list_via(self.backend().await?, prefix).await
    }

    async fn statistics(&self) -> Result<StorageStatistics> {
        statistics_via(self.backend().await?).await
    }

    fn backend_type(&self) -> StorageBackendType {
        StorageBackendType::S3
    }
}

/// Azure Blob Storage backend.
///
/// Delegates to the real `oximedia_storage::azure::AzureStorage` client
/// when the `azure` feature is enabled. Without the feature, every method
/// returns an honest [`MamError::Internal`] instead of a fabricated success.
#[allow(dead_code)]
pub struct AzureStorage {
    account: String,
    container: String,
    access_key: String,
    backend_cell: OnceCell<Box<dyn oximedia_storage::CloudStorage>>,
}

impl AzureStorage {
    /// Create a new Azure storage backend. `access_key` is the base64
    /// storage account access key (as shown in the Azure Portal). The real
    /// client is not connected until the first operation is performed.
    #[must_use]
    pub fn new(account: String, container: String, access_key: String) -> Self {
        Self {
            account,
            container,
            access_key,
            backend_cell: OnceCell::new(),
        }
    }

    /// Test-only constructor; see [`S3Storage::with_backend_for_test`].
    #[cfg(test)]
    fn with_backend_for_test(backend: Box<dyn oximedia_storage::CloudStorage>) -> Self {
        Self {
            account: "test-account".to_string(),
            container: "test-container".to_string(),
            access_key: String::new(),
            backend_cell: OnceCell::new_with(Some(backend)),
        }
    }

    /// Resolve the real Azure backend, connecting (and caching the
    /// connection) on first use.
    ///
    /// # Errors
    ///
    /// Returns an error if the `azure` feature is disabled, or if the
    /// underlying `oximedia-storage` Azure client fails to initialize.
    async fn backend(&self) -> Result<&dyn oximedia_storage::CloudStorage> {
        if let Some(backend) = self.backend_cell.get() {
            return Ok(backend.as_ref());
        }
        #[cfg(feature = "azure")]
        {
            let backend = self
                .backend_cell
                .get_or_try_init(|| async {
                    let config = oximedia_storage::UnifiedConfig::azure(
                        self.container.clone(),
                        self.account.clone(),
                    )
                    .with_credentials(self.account.clone(), self.access_key.clone());
                    let storage = oximedia_storage::azure::AzureStorage::new(config).await?;
                    let boxed: Box<dyn oximedia_storage::CloudStorage> = Box::new(storage);
                    Ok::<_, oximedia_storage::StorageError>(boxed)
                })
                .await
                .map_err(storage_err)?;
            Ok(backend.as_ref())
        }
        #[cfg(not(feature = "azure"))]
        {
            Err(MamError::Internal(format!(
                "Azure backend requires the \"azure\" feature (container: {})",
                self.container
            )))
        }
    }
}

#[async_trait]
impl StorageBackend for AzureStorage {
    async fn upload(&self, local_path: &Path, remote_path: &str) -> Result<StorageMetadata> {
        upload_via(self.backend().await?, local_path, remote_path).await
    }

    async fn download(&self, remote_path: &str, local_path: &Path) -> Result<()> {
        download_via(self.backend().await?, remote_path, local_path).await
    }

    async fn delete(&self, remote_path: &str) -> Result<()> {
        delete_via(self.backend().await?, remote_path).await
    }

    async fn exists(&self, remote_path: &str) -> Result<bool> {
        exists_via(self.backend().await?, remote_path).await
    }

    async fn metadata(&self, remote_path: &str) -> Result<StorageMetadata> {
        metadata_via(self.backend().await?, remote_path).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        list_via(self.backend().await?, prefix).await
    }

    async fn statistics(&self) -> Result<StorageStatistics> {
        statistics_via(self.backend().await?).await
    }

    fn backend_type(&self) -> StorageBackendType {
        StorageBackendType::Azure
    }
}

/// Google Cloud Storage backend.
///
/// Delegates to the real `oximedia_storage::gcs::GcsStorage` client when
/// the `gcs` feature is enabled. Without the feature, every method returns
/// an honest [`MamError::Internal`] instead of a fabricated success.
#[allow(dead_code)]
pub struct GCSStorage {
    bucket: String,
    project_id: String,
    credentials_path: String,
    backend_cell: OnceCell<Box<dyn oximedia_storage::CloudStorage>>,
}

impl GCSStorage {
    /// Create a new GCS storage backend. `credentials_path`, when
    /// non-empty, is passed through as `UnifiedConfig::credentials_file`.
    /// The real client is not connected until the first operation is
    /// performed.
    #[must_use]
    pub fn new(bucket: String, project_id: String, credentials_path: String) -> Self {
        Self {
            bucket,
            project_id,
            credentials_path,
            backend_cell: OnceCell::new(),
        }
    }

    /// Test-only constructor; see [`S3Storage::with_backend_for_test`].
    #[cfg(test)]
    fn with_backend_for_test(backend: Box<dyn oximedia_storage::CloudStorage>) -> Self {
        Self {
            bucket: "test-bucket".to_string(),
            project_id: "test-project".to_string(),
            credentials_path: String::new(),
            backend_cell: OnceCell::new_with(Some(backend)),
        }
    }

    /// Resolve the real GCS backend, connecting (and caching the
    /// connection) on first use.
    ///
    /// # Errors
    ///
    /// Returns an error if the `gcs` feature is disabled, or if the
    /// underlying `oximedia-storage` GCS client fails to initialize.
    async fn backend(&self) -> Result<&dyn oximedia_storage::CloudStorage> {
        if let Some(backend) = self.backend_cell.get() {
            return Ok(backend.as_ref());
        }
        #[cfg(feature = "gcs")]
        {
            let backend = self
                .backend_cell
                .get_or_try_init(|| async {
                    let mut config = oximedia_storage::UnifiedConfig::gcs(
                        self.bucket.clone(),
                        self.project_id.clone(),
                    );
                    if !self.credentials_path.is_empty() {
                        config.credentials_file = Some(PathBuf::from(&self.credentials_path));
                    }
                    let storage = oximedia_storage::gcs::GcsStorage::new(config).await?;
                    let boxed: Box<dyn oximedia_storage::CloudStorage> = Box::new(storage);
                    Ok::<_, oximedia_storage::StorageError>(boxed)
                })
                .await
                .map_err(storage_err)?;
            Ok(backend.as_ref())
        }
        #[cfg(not(feature = "gcs"))]
        {
            Err(MamError::Internal(format!(
                "GCS backend requires the \"gcs\" feature (bucket: {})",
                self.bucket
            )))
        }
    }
}

#[async_trait]
impl StorageBackend for GCSStorage {
    async fn upload(&self, local_path: &Path, remote_path: &str) -> Result<StorageMetadata> {
        upload_via(self.backend().await?, local_path, remote_path).await
    }

    async fn download(&self, remote_path: &str, local_path: &Path) -> Result<()> {
        download_via(self.backend().await?, remote_path, local_path).await
    }

    async fn delete(&self, remote_path: &str) -> Result<()> {
        delete_via(self.backend().await?, remote_path).await
    }

    async fn exists(&self, remote_path: &str) -> Result<bool> {
        exists_via(self.backend().await?, remote_path).await
    }

    async fn metadata(&self, remote_path: &str) -> Result<StorageMetadata> {
        metadata_via(self.backend().await?, remote_path).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        list_via(self.backend().await?, prefix).await
    }

    async fn statistics(&self) -> Result<StorageStatistics> {
        statistics_via(self.backend().await?).await
    }

    fn backend_type(&self) -> StorageBackendType {
        StorageBackendType::GCS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Cloud backend adapters: round-trip real bytes via a local backend ────
    //
    // `with_backend_for_test` injects `oximedia_storage::local::LocalStorage`
    // in place of the real S3/Azure/GCS client, so these tests exercise the
    // exact same `upload_via`/`download_via`/... delegation code the real
    // cloud features use, without network access and regardless of which
    // Cargo features are enabled.

    #[tokio::test]
    async fn test_s3_storage_roundtrip_via_local_backend() {
        let root = std::env::temp_dir().join(format!("mam-s3-adapter-{}", std::process::id()));
        let local = oximedia_storage::local::LocalStorage::new(&root)
            .await
            .expect("local backend");
        let storage = S3Storage::with_backend_for_test(Box::new(local));

        let src_dir = std::env::temp_dir().join(format!("mam-s3-src-{}", std::process::id()));
        tokio::fs::create_dir_all(&src_dir).await.expect("src dir");
        let src_file = src_dir.join("clip.bin");
        tokio::fs::write(&src_file, b"s3-adapter-bytes")
            .await
            .expect("write src");

        let meta = storage
            .upload(&src_file, "clips/clip.bin")
            .await
            .expect("upload succeeds");
        assert_eq!(meta.size, 16);

        assert!(storage
            .exists("clips/clip.bin")
            .await
            .expect("exists check"));

        let fetched = storage
            .metadata("clips/clip.bin")
            .await
            .expect("metadata succeeds");
        assert_eq!(fetched.size, 16);

        let listed = storage.list("clips/").await.expect("list succeeds");
        assert!(listed.iter().any(|k| k.ends_with("clip.bin")));

        let stats = storage.statistics().await.expect("statistics succeeds");
        assert_eq!(stats.total_files, 1);
        assert_eq!(stats.total_size, 16);

        let dest_file = src_dir.join("nested").join("downloaded.bin");
        storage
            .download("clips/clip.bin", &dest_file)
            .await
            .expect("download succeeds");
        let downloaded = tokio::fs::read(&dest_file).await.expect("read downloaded");
        assert_eq!(downloaded, b"s3-adapter-bytes");

        storage
            .delete("clips/clip.bin")
            .await
            .expect("delete succeeds");
        assert!(!storage
            .exists("clips/clip.bin")
            .await
            .expect("exists after delete"));

        let _ = tokio::fs::remove_dir_all(&root).await;
        let _ = tokio::fs::remove_dir_all(&src_dir).await;
    }

    #[tokio::test]
    async fn test_azure_storage_roundtrip_via_local_backend() {
        let root = std::env::temp_dir().join(format!("mam-azure-adapter-{}", std::process::id()));
        let local = oximedia_storage::local::LocalStorage::new(&root)
            .await
            .expect("local backend");
        let storage = AzureStorage::with_backend_for_test(Box::new(local));

        let src_dir = std::env::temp_dir().join(format!("mam-azure-src-{}", std::process::id()));
        tokio::fs::create_dir_all(&src_dir).await.expect("src dir");
        let src_file = src_dir.join("blob.bin");
        tokio::fs::write(&src_file, b"azure-adapter-bytes")
            .await
            .expect("write src");

        let meta = storage
            .upload(&src_file, "media/blob.bin")
            .await
            .expect("upload succeeds");
        assert_eq!(meta.size, 19);
        assert!(storage
            .exists("media/blob.bin")
            .await
            .expect("exists check"));

        let dest_file = src_dir.join("downloaded-blob.bin");
        storage
            .download("media/blob.bin", &dest_file)
            .await
            .expect("download succeeds");
        let downloaded = tokio::fs::read(&dest_file).await.expect("read downloaded");
        assert_eq!(downloaded, b"azure-adapter-bytes");

        storage
            .delete("media/blob.bin")
            .await
            .expect("delete succeeds");
        assert!(!storage
            .exists("media/blob.bin")
            .await
            .expect("exists after delete"));

        let _ = tokio::fs::remove_dir_all(&root).await;
        let _ = tokio::fs::remove_dir_all(&src_dir).await;
    }

    #[tokio::test]
    async fn test_gcs_storage_roundtrip_via_local_backend() {
        let root = std::env::temp_dir().join(format!("mam-gcs-adapter-{}", std::process::id()));
        let local = oximedia_storage::local::LocalStorage::new(&root)
            .await
            .expect("local backend");
        let storage = GCSStorage::with_backend_for_test(Box::new(local));

        let src_dir = std::env::temp_dir().join(format!("mam-gcs-src-{}", std::process::id()));
        tokio::fs::create_dir_all(&src_dir).await.expect("src dir");
        let src_file = src_dir.join("object.bin");
        tokio::fs::write(&src_file, b"gcs-adapter-bytes")
            .await
            .expect("write src");

        let meta = storage
            .upload(&src_file, "data/object.bin")
            .await
            .expect("upload succeeds");
        assert_eq!(meta.size, 17);
        assert!(storage
            .exists("data/object.bin")
            .await
            .expect("exists check"));

        let dest_file = src_dir.join("downloaded-object.bin");
        storage
            .download("data/object.bin", &dest_file)
            .await
            .expect("download succeeds");
        let downloaded = tokio::fs::read(&dest_file).await.expect("read downloaded");
        assert_eq!(downloaded, b"gcs-adapter-bytes");

        storage
            .delete("data/object.bin")
            .await
            .expect("delete succeeds");
        assert!(!storage
            .exists("data/object.bin")
            .await
            .expect("exists after delete"));

        let _ = tokio::fs::remove_dir_all(&root).await;
        let _ = tokio::fs::remove_dir_all(&src_dir).await;
    }

    // ── Cloud backends: honest Err when the feature is disabled ─────────────
    //
    // Constructed via the real `new()` (not `with_backend_for_test`), so
    // `backend_cell` starts empty and every method must reach the
    // `#[cfg(not(feature = "..."))]` arm of `backend()`.

    #[cfg(not(feature = "s3"))]
    #[tokio::test]
    async fn test_s3_storage_feature_off_is_honest_err() {
        let storage = S3Storage::new(
            "bucket".to_string(),
            "us-east-1".to_string(),
            "ak".to_string(),
            "sk".to_string(),
        );
        let tmp = std::env::temp_dir().join(format!("mam-s3-off-{}", std::process::id()));
        tokio::fs::write(&tmp, b"x").await.expect("write tmp");

        assert!(storage.upload(&tmp, "k").await.is_err());
        assert!(storage.download("k", &tmp).await.is_err());
        assert!(storage.delete("k").await.is_err());
        assert!(
            storage.exists("k").await.is_err(),
            "exists() must never fabricate `true` when the s3 feature is off"
        );
        assert!(storage.metadata("k").await.is_err());
        assert!(storage.list("prefix/").await.is_err());
        assert!(storage.statistics().await.is_err());

        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[cfg(not(feature = "azure"))]
    #[tokio::test]
    async fn test_azure_storage_feature_off_is_honest_err() {
        let storage = AzureStorage::new(
            "account".to_string(),
            "container".to_string(),
            "key".to_string(),
        );
        let tmp = std::env::temp_dir().join(format!("mam-azure-off-{}", std::process::id()));
        tokio::fs::write(&tmp, b"x").await.expect("write tmp");

        assert!(storage.upload(&tmp, "k").await.is_err());
        assert!(storage.download("k", &tmp).await.is_err());
        assert!(storage.delete("k").await.is_err());
        assert!(
            storage.exists("k").await.is_err(),
            "exists() must never fabricate `true` when the azure feature is off"
        );
        assert!(storage.metadata("k").await.is_err());
        assert!(storage.list("prefix/").await.is_err());
        assert!(storage.statistics().await.is_err());

        let _ = tokio::fs::remove_file(&tmp).await;
    }

    #[cfg(not(feature = "gcs"))]
    #[tokio::test]
    async fn test_gcs_storage_feature_off_is_honest_err() {
        let storage = GCSStorage::new("bucket".to_string(), "project".to_string(), String::new());
        let tmp = std::env::temp_dir().join(format!("mam-gcs-off-{}", std::process::id()));
        tokio::fs::write(&tmp, b"x").await.expect("write tmp");

        assert!(storage.upload(&tmp, "k").await.is_err());
        assert!(storage.download("k", &tmp).await.is_err());
        assert!(storage.delete("k").await.is_err());
        assert!(
            storage.exists("k").await.is_err(),
            "exists() must never fabricate `true` when the gcs feature is off"
        );
        assert!(storage.metadata("k").await.is_err());
        assert!(storage.list("prefix/").await.is_err());
        assert!(storage.statistics().await.is_err());

        let _ = tokio::fs::remove_file(&tmp).await;
    }
}
