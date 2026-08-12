//! Storage management with multiple backends
//!
//! Provides comprehensive storage management for:
//! - Local filesystem storage
//! - Amazon S3 storage
//! - Azure Blob Storage
//! - Google Cloud Storage
//! - Network storage (NFS, SMB)
//! - Tiered storage (hot, warm, cold)
//! - Deduplication
//! - Storage analytics

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::database::Database;
use crate::{MamError, Result};

/// Storage manager handles all storage operations
pub struct StorageManager {
    db: Arc<Database>,
    backends: Arc<RwLock<HashMap<String, Arc<dyn StorageBackend>>>>,
    default_backend: String,
}

/// Storage backend trait
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Upload a file
    async fn upload(&self, local_path: &Path, remote_path: &str) -> Result<StorageMetadata>;

    /// Download a file
    async fn download(&self, remote_path: &str, local_path: &Path) -> Result<()>;

    /// Delete a file
    async fn delete(&self, remote_path: &str) -> Result<()>;

    /// Check if file exists
    async fn exists(&self, remote_path: &str) -> Result<bool>;

    /// Get file metadata
    async fn metadata(&self, remote_path: &str) -> Result<StorageMetadata>;

    /// List files in directory
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;

    /// Get storage statistics
    async fn statistics(&self) -> Result<StorageStatistics>;

    /// Get backend type
    fn backend_type(&self) -> StorageBackendType;
}

/// Storage backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageBackendType {
    /// Local filesystem
    Local,
    /// Amazon S3
    S3,
    /// Azure Blob Storage
    Azure,
    /// Google Cloud Storage
    GCS,
    /// Network File System
    NFS,
    /// SMB/CIFS
    SMB,
}

impl StorageBackendType {
    /// Convert to string
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::S3 => "s3",
            Self::Azure => "azure",
            Self::GCS => "gcs",
            Self::NFS => "nfs",
            Self::SMB => "smb",
        }
    }
}

/// Storage metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetadata {
    pub path: String,
    pub size: u64,
    pub content_type: Option<String>,
    pub checksum: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Storage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatistics {
    pub total_files: u64,
    pub total_size: u64,
    pub available_space: Option<u64>,
    pub used_space: Option<u64>,
}

/// Storage tier for tiered storage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageTier {
    /// Hot tier (fast access, high cost)
    Hot,
    /// Warm tier (medium access, medium cost)
    Warm,
    /// Cold tier (slow access, low cost)
    Cold,
    /// Archive tier (very slow access, very low cost)
    Archive,
}

impl StorageTier {
    /// Convert to string
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Archive => "archive",
        }
    }

    /// Get access time in seconds
    #[must_use]
    pub const fn access_time_seconds(&self) -> u32 {
        match self {
            Self::Hot => 1,
            Self::Warm => 60,
            Self::Cold => 300,
            Self::Archive => 3600,
        }
    }

    /// Get cost multiplier
    #[must_use]
    pub const fn cost_multiplier(&self) -> f32 {
        match self {
            Self::Hot => 1.0,
            Self::Warm => 0.5,
            Self::Cold => 0.1,
            Self::Archive => 0.01,
        }
    }
}

/// Storage location record
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct StorageLocation {
    pub id: Uuid,
    pub asset_id: Uuid,
    pub backend: String,
    pub path: String,
    pub tier: String,
    pub size: Option<i64>,
    pub checksum: Option<String>,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Local filesystem storage backend
pub struct LocalStorage {
    root_path: PathBuf,
}

impl LocalStorage {
    /// Create a new local storage backend
    #[must_use]
    pub fn new(root_path: String) -> Self {
        Self {
            root_path: PathBuf::from(root_path),
        }
    }

    fn resolve_path(&self, remote_path: &str) -> PathBuf {
        self.root_path.join(remote_path.trim_start_matches('/'))
    }
}

#[async_trait]
impl StorageBackend for LocalStorage {
    async fn upload(&self, local_path: &Path, remote_path: &str) -> Result<StorageMetadata> {
        let dest_path = self.resolve_path(remote_path);

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Copy file
        tokio::fs::copy(local_path, &dest_path).await?;

        // Get metadata
        let metadata = tokio::fs::metadata(&dest_path).await?;

        Ok(StorageMetadata {
            path: remote_path.to_string(),
            size: metadata.len(),
            content_type: None,
            checksum: None,
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
        })
    }

    async fn download(&self, remote_path: &str, local_path: &Path) -> Result<()> {
        let src_path = self.resolve_path(remote_path);

        // Create parent directories
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Copy file
        tokio::fs::copy(src_path, local_path).await?;

        Ok(())
    }

    async fn delete(&self, remote_path: &str) -> Result<()> {
        let path = self.resolve_path(remote_path);
        tokio::fs::remove_file(path).await?;
        Ok(())
    }

    async fn exists(&self, remote_path: &str) -> Result<bool> {
        let path = self.resolve_path(remote_path);
        Ok(path.exists())
    }

    async fn metadata(&self, remote_path: &str) -> Result<StorageMetadata> {
        let path = self.resolve_path(remote_path);
        let metadata = tokio::fs::metadata(&path).await?;

        Ok(StorageMetadata {
            path: remote_path.to_string(),
            size: metadata.len(),
            content_type: None,
            checksum: None,
            created_at: metadata.created().ok().map(|t| t.into()),
            updated_at: metadata.modified().ok().map(|t| t.into()),
        })
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let dir_path = self.resolve_path(prefix);
        let mut entries = Vec::new();

        let mut dir = tokio::fs::read_dir(dir_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            if let Ok(file_name) = entry.file_name().into_string() {
                entries.push(format!("{}/{}", prefix.trim_end_matches('/'), file_name));
            }
        }

        Ok(entries)
    }

    async fn statistics(&self) -> Result<StorageStatistics> {
        // Get filesystem statistics (simplified)
        Ok(StorageStatistics {
            total_files: 0,
            total_size: 0,
            available_space: None,
            used_space: None,
        })
    }

    fn backend_type(&self) -> StorageBackendType {
        StorageBackendType::Local
    }
}

// ── Cloud storage backends: real delegation to `oximedia-storage` ──────────
//
// S3Storage / AzureStorage / GCSStorage (defined in the `cloud` submodule)
// implement the legacy `StorageBackend` trait by delegating to the real,
// feature-gated backends in `oximedia-storage` — the same backends
// `MamStorage::from_uri` wires up further below. When the matching Cargo
// feature is disabled, every method returns an honest `Err`; `exists()`
// never fabricates `true`. Re-exported here so the public path
// (`oximedia_mam::storage::S3Storage` etc.) is unchanged.
mod cloud;
pub use cloud::{AzureStorage, GCSStorage, S3Storage};

impl StorageManager {
    /// Create a new storage manager
    #[must_use]
    pub fn new(db: Arc<Database>, default_backend: String) -> Self {
        Self {
            db,
            backends: Arc::new(RwLock::new(HashMap::new())),
            default_backend,
        }
    }

    /// Register a storage backend
    pub async fn register_backend(
        &self,
        name: String,
        backend: Arc<dyn StorageBackend>,
    ) -> Result<()> {
        self.backends.write().await.insert(name, backend);
        Ok(())
    }

    /// Upload file to storage
    ///
    /// # Errors
    ///
    /// Returns an error if upload fails
    pub async fn upload_file(
        &self,
        asset_id: Uuid,
        local_path: &Path,
        remote_path: &str,
        backend_name: Option<String>,
        tier: StorageTier,
    ) -> Result<StorageLocation> {
        let backend_name = backend_name.unwrap_or_else(|| self.default_backend.clone());

        let backends = self.backends.read().await;
        let backend = backends
            .get(&backend_name)
            .ok_or_else(|| MamError::Internal(format!("Backend not found: {backend_name}")))?;

        // Upload file
        let metadata = backend.upload(local_path, remote_path).await?;

        // Create storage location record
        let location = sqlx::query_as::<_, StorageLocation>(
            "INSERT INTO storage_locations
             (id, asset_id, backend, path, tier, size, checksum, is_primary, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, true, NOW(), NOW())
             RETURNING *",
        )
        .bind(Uuid::new_v4())
        .bind(asset_id)
        .bind(&backend_name)
        .bind(remote_path)
        .bind(tier.as_str())
        .bind(metadata.size as i64)
        .bind(metadata.checksum)
        .fetch_one(self.db.pool())
        .await?;

        Ok(location)
    }

    /// Download file from storage
    ///
    /// # Errors
    ///
    /// Returns an error if download fails
    pub async fn download_file(
        &self,
        asset_id: Uuid,
        local_path: &Path,
        backend_name: Option<String>,
    ) -> Result<()> {
        // Get primary storage location
        let location = sqlx::query_as::<_, StorageLocation>(
            "SELECT * FROM storage_locations
             WHERE asset_id = $1 AND is_primary = true
             LIMIT 1",
        )
        .bind(asset_id)
        .fetch_one(self.db.pool())
        .await?;

        let backend_name = backend_name.unwrap_or(location.backend);

        let backends = self.backends.read().await;
        let backend = backends
            .get(&backend_name)
            .ok_or_else(|| MamError::Internal(format!("Backend not found: {backend_name}")))?;

        // Download file
        backend.download(&location.path, local_path).await?;

        Ok(())
    }

    /// Delete file from storage
    ///
    /// # Errors
    ///
    /// Returns an error if deletion fails
    pub async fn delete_file(&self, asset_id: Uuid) -> Result<()> {
        // Get all storage locations
        let locations = sqlx::query_as::<_, StorageLocation>(
            "SELECT * FROM storage_locations WHERE asset_id = $1",
        )
        .bind(asset_id)
        .fetch_all(self.db.pool())
        .await?;

        let backends = self.backends.read().await;

        // Delete from all locations
        for location in &locations {
            if let Some(backend) = backends.get(&location.backend) {
                if let Err(e) = backend.delete(&location.path).await {
                    tracing::error!("Failed to delete file from {}: {}", location.backend, e);
                }
            }
        }

        // Delete storage location records
        sqlx::query("DELETE FROM storage_locations WHERE asset_id = $1")
            .bind(asset_id)
            .execute(self.db.pool())
            .await?;

        Ok(())
    }

    /// Get storage locations for asset
    ///
    /// # Errors
    ///
    /// Returns an error if query fails
    pub async fn get_storage_locations(&self, asset_id: Uuid) -> Result<Vec<StorageLocation>> {
        let locations = sqlx::query_as::<_, StorageLocation>(
            "SELECT * FROM storage_locations WHERE asset_id = $1 ORDER BY is_primary DESC, created_at DESC",
        )
        .bind(asset_id)
        .fetch_all(self.db.pool())
        .await?;

        Ok(locations)
    }

    /// Move asset to different storage tier
    ///
    /// # Errors
    ///
    /// Returns an error if tier change fails
    pub async fn change_tier(&self, asset_id: Uuid, new_tier: StorageTier) -> Result<()> {
        sqlx::query(
            "UPDATE storage_locations
             SET tier = $2, updated_at = NOW()
             WHERE asset_id = $1",
        )
        .bind(asset_id)
        .bind(new_tier.as_str())
        .execute(self.db.pool())
        .await?;

        Ok(())
    }

    /// Get storage statistics across all backends
    ///
    /// # Errors
    ///
    /// Returns an error if statistics gathering fails
    pub async fn get_statistics(&self) -> Result<HashMap<String, StorageStatistics>> {
        let mut stats = HashMap::new();

        let backends = self.backends.read().await;
        for (name, backend) in backends.iter() {
            if let Ok(backend_stats) = backend.statistics().await {
                stats.insert(name.clone(), backend_stats);
            }
        }

        Ok(stats)
    }

    /// Check storage health
    ///
    /// # Errors
    ///
    /// Returns an error if health check fails
    pub async fn check_health(&self) -> Result<bool> {
        let backends = self.backends.read().await;

        for (name, backend) in backends.iter() {
            // Try to get statistics as health check
            if backend.statistics().await.is_err() {
                tracing::warn!("Storage backend {} is unhealthy", name);
                return Ok(false);
            }
        }

        Ok(true)
    }
}

// ── MamStorage: thin delegation to oximedia-storage ─────────────────────────

/// Error type for `MamStorage` operations.
#[derive(Debug, thiserror::Error)]
pub enum MamStorageError {
    /// The URI could not be parsed.
    #[error("Invalid URI: {0}")]
    InvalidUri(String),
    /// The URI scheme is not supported by any available backend.
    #[error("Unsupported URI scheme: {0}")]
    UnsupportedScheme(String),
    /// The underlying storage backend returned an error.
    #[error("Backend error: {0}")]
    Backend(String),
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<oximedia_storage::StorageError> for MamStorageError {
    fn from(e: oximedia_storage::StorageError) -> Self {
        Self::Backend(e.to_string())
    }
}

/// URI scheme understood by `MamStorage::from_uri`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MamStorageScheme {
    /// Local filesystem (`file://` or a bare relative/absolute path).
    Local(PathBuf),
    /// Amazon S3 (`s3://bucket/prefix`).
    S3 { bucket: String, prefix: String },
    /// Google Cloud Storage (`gs://bucket/prefix`).
    Gcs { bucket: String, prefix: String },
    /// Azure Blob Storage (`https://<account>.blob.core.windows.net/<container>`).
    Azure {
        account: String,
        container: String,
        prefix: String,
    },
}

/// Inner storage implementation.
///
/// `Local` always uses `oximedia-storage::local::LocalStorage`.
///
/// `Remote` routes I/O through a real cloud backend (`S3Storage`, `GcsStorage`,
/// or `AzureStorage` from `oximedia-storage`) when the matching `s3`/`gcs`/
/// `azure` Cargo feature is enabled.  When the feature is **off** the `backend`
/// field is `None` and every operation falls back to the `local_shadow`
/// `LocalStorage` rooted in a temp directory — a deliberate, pure-Rust
/// "local-shadow" degraded mode that keeps the crate dependency-free by
/// default.
enum MamStorageInner {
    Local(oximedia_storage::local::LocalStorage),
    /// Remote backend — uses `backend` when present, else the local shadow.
    Remote {
        /// Real cloud backend; `None` when the matching feature is disabled.
        backend: Option<Box<dyn oximedia_storage::CloudStorage>>,
        /// Pure-Rust fallback used when `backend` is `None`.
        local_shadow: oximedia_storage::local::LocalStorage,
    },
}

/// Thin async storage delegation layer for the MAM subsystem.
///
/// Create with [`MamStorage::from_uri`] or [`MamStorage::local`].
pub struct MamStorage {
    inner: MamStorageInner,
}

impl MamStorage {
    /// Create a `MamStorage` backed by a local root directory.
    ///
    /// # Errors
    ///
    /// Returns an error if the root directory cannot be created.
    pub async fn local(root: impl Into<PathBuf>) -> std::result::Result<Self, MamStorageError> {
        let ls = oximedia_storage::local::LocalStorage::new(root).await?;
        Ok(Self {
            inner: MamStorageInner::Local(ls),
        })
    }

    /// Parse a URI and create the appropriate storage backend.
    ///
    /// Supported schemes:
    /// * `file:///path` or bare `/path` or `relative/path` → local filesystem
    /// * `s3://bucket/prefix` → Amazon S3
    /// * `gs://bucket/prefix` → Google Cloud Storage
    /// * `https://<account>.blob.core.windows.net/<container>[/prefix]` → Azure Blob Storage
    ///
    /// # Cloud backends and feature flags
    ///
    /// The `s3`, `gcs`, and `azure` Cargo features each enable a real network
    /// backend from `oximedia-storage`.  When the matching feature is **not**
    /// enabled the URI is still accepted but every operation is routed to a
    /// pure-Rust "local-shadow" `LocalStorage` rooted in a temp directory; no
    /// cloud SDK is compiled in.  This keeps the default build dependency-free.
    ///
    /// # Credential and configuration sourcing
    ///
    /// Cloud credentials are **not** embedded in the path component of the URI.
    /// They are read from URI query parameters or environment variables:
    ///
    /// * **S3** (`s3://bucket/prefix?...`): `access_key`, `secret_key`,
    ///   `region`, and `endpoint` query parameters; otherwise the standard AWS
    ///   credential chain (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
    ///   `AWS_REGION`/`AWS_DEFAULT_REGION`) is used.  `region` defaults to
    ///   `us-east-1` when neither a query parameter nor an env var is present.
    /// * **GCS** (`gs://bucket/prefix?project=...`): the `gs://` URI grammar
    ///   has no project field, so the GCS `project_id` is taken from the
    ///   `project` query parameter or, failing that, the `GOOGLE_CLOUD_PROJECT`
    ///   environment variable.  It may be empty for object-level operations.
    ///   Authentication uses Application Default Credentials.
    /// * **Azure**: the storage account is parsed from the host; the account
    ///   key is read from the `account_key` query parameter or the
    ///   `AZURE_STORAGE_KEY` environment variable (required by the backend).
    ///
    /// # Errors
    ///
    /// Returns [`MamStorageError::InvalidUri`] if the URI cannot be parsed,
    /// [`MamStorageError::UnsupportedScheme`] for unknown schemes, or
    /// [`MamStorageError::Backend`] if an enabled cloud backend fails to
    /// initialise.
    pub async fn from_uri(uri: &str) -> std::result::Result<Self, MamStorageError> {
        let scheme = Self::parse_scheme(uri)?;
        match &scheme {
            MamStorageScheme::Local(path) => {
                let ls = oximedia_storage::local::LocalStorage::new(path).await?;
                Ok(Self {
                    inner: MamStorageInner::Local(ls),
                })
            }
            MamStorageScheme::S3 { bucket, prefix } => {
                let target = format!("s3://{bucket}/{prefix}");
                let backend = Self::build_s3_backend(uri, bucket).await?;
                Self::log_remote_mode("s3", &target, backend.is_some());
                let shadow_root = std::env::temp_dir().join(format!("mam-s3-{bucket}"));
                let ls = oximedia_storage::local::LocalStorage::new(&shadow_root).await?;
                Ok(Self {
                    inner: MamStorageInner::Remote {
                        backend,
                        local_shadow: ls,
                    },
                })
            }
            MamStorageScheme::Gcs { bucket, prefix } => {
                let target = format!("gs://{bucket}/{prefix}");
                let backend = Self::build_gcs_backend(uri, bucket).await?;
                Self::log_remote_mode("gs", &target, backend.is_some());
                let shadow_root = std::env::temp_dir().join(format!("mam-gs-{bucket}"));
                let ls = oximedia_storage::local::LocalStorage::new(&shadow_root).await?;
                Ok(Self {
                    inner: MamStorageInner::Remote {
                        backend,
                        local_shadow: ls,
                    },
                })
            }
            MamStorageScheme::Azure {
                account,
                container,
                prefix,
            } => {
                let target =
                    format!("https://{account}.blob.core.windows.net/{container}/{prefix}");
                let backend = Self::build_azure_backend(uri, account, container).await?;
                Self::log_remote_mode("azure", &target, backend.is_some());
                let shadow_root =
                    std::env::temp_dir().join(format!("mam-az-{account}-{container}"));
                let ls = oximedia_storage::local::LocalStorage::new(&shadow_root).await?;
                Ok(Self {
                    inner: MamStorageInner::Remote {
                        backend,
                        local_shadow: ls,
                    },
                })
            }
        }
    }

    /// Emit a consistent log line describing the chosen remote mode.
    fn log_remote_mode(scheme: &str, target: &str, real_backend: bool) {
        if real_backend {
            tracing::info!(
                scheme = %scheme,
                target = %target,
                "MamStorage: cloud backend active"
            );
        } else {
            tracing::info!(
                scheme = %scheme,
                target = %target,
                "MamStorage: local-shadow mode (cloud feature disabled)"
            );
        }
    }

    /// Build the S3 backend when the `s3` feature is enabled.
    ///
    /// Returns `Ok(None)` when the feature is disabled.
    async fn build_s3_backend(
        uri: &str,
        bucket: &str,
    ) -> std::result::Result<Option<Box<dyn oximedia_storage::CloudStorage>>, MamStorageError> {
        #[cfg(feature = "s3")]
        {
            let params = parse_query_params(uri);
            let region = params
                .get("region")
                .cloned()
                .or_else(|| std::env::var("AWS_REGION").ok())
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| "us-east-1".to_string());
            let mut config = oximedia_storage::UnifiedConfig::s3(bucket.to_string(), region);
            let access_key = params
                .get("access_key")
                .cloned()
                .or_else(|| std::env::var("AWS_ACCESS_KEY_ID").ok());
            let secret_key = params
                .get("secret_key")
                .cloned()
                .or_else(|| std::env::var("AWS_SECRET_ACCESS_KEY").ok());
            if let (Some(ak), Some(sk)) = (access_key, secret_key) {
                config = config.with_credentials(ak, sk);
            }
            if let Some(endpoint) = params.get("endpoint") {
                config = config.with_endpoint(endpoint.clone());
            }
            let storage = oximedia_storage::s3::S3Storage::new(config).await?;
            Ok(Some(Box::new(storage)))
        }
        #[cfg(not(feature = "s3"))]
        {
            // `s3` feature disabled — fall back to the local shadow.
            let _ = (uri, bucket);
            Ok(None)
        }
    }

    /// Build the GCS backend when the `gcs` feature is enabled.
    ///
    /// The GCS `project_id` is sourced from the `?project=` query parameter or
    /// the `GOOGLE_CLOUD_PROJECT` environment variable.  Returns `Ok(None)`
    /// when the feature is disabled.
    async fn build_gcs_backend(
        uri: &str,
        bucket: &str,
    ) -> std::result::Result<Option<Box<dyn oximedia_storage::CloudStorage>>, MamStorageError> {
        #[cfg(feature = "gcs")]
        {
            let params = parse_query_params(uri);
            let project_id = params
                .get("project")
                .cloned()
                .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok())
                .unwrap_or_default();
            let config = oximedia_storage::UnifiedConfig::gcs(bucket.to_string(), project_id);
            let storage = oximedia_storage::gcs::GcsStorage::new(config).await?;
            Ok(Some(Box::new(storage)))
        }
        #[cfg(not(feature = "gcs"))]
        {
            // `gcs` feature disabled — fall back to the local shadow.
            let _ = (uri, bucket);
            Ok(None)
        }
    }

    /// Build the Azure backend when the `azure` feature is enabled.
    ///
    /// The account key is sourced from the `?account_key=` query parameter or
    /// the `AZURE_STORAGE_KEY` environment variable.  Returns `Ok(None)` when
    /// the feature is disabled.
    async fn build_azure_backend(
        uri: &str,
        account: &str,
        container: &str,
    ) -> std::result::Result<Option<Box<dyn oximedia_storage::CloudStorage>>, MamStorageError> {
        #[cfg(feature = "azure")]
        {
            let params = parse_query_params(uri);
            let account_key = params
                .get("account_key")
                .cloned()
                .or_else(|| std::env::var("AZURE_STORAGE_KEY").ok())
                .ok_or_else(|| {
                    MamStorageError::InvalidUri(
                        "Azure account key required: pass ?account_key=... or set \
                         AZURE_STORAGE_KEY"
                            .to_string(),
                    )
                })?;
            let config =
                oximedia_storage::UnifiedConfig::azure(container.to_string(), account.to_string())
                    .with_credentials(account.to_string(), account_key);
            let storage = oximedia_storage::azure::AzureStorage::new(config).await?;
            Ok(Some(Box::new(storage)))
        }
        #[cfg(not(feature = "azure"))]
        {
            // `azure` feature disabled — fall back to the local shadow.
            let _ = (uri, account, container);
            Ok(None)
        }
    }

    /// Parse the URI scheme without creating any I/O resources.
    ///
    /// # Errors
    ///
    /// Returns an error if the URI cannot be parsed.
    pub fn parse_scheme(uri: &str) -> std::result::Result<MamStorageScheme, MamStorageError> {
        // Bare path (relative or absolute) → Local
        if !uri.contains("://") {
            return Ok(MamStorageScheme::Local(PathBuf::from(uri)));
        }

        let colon_slash = uri
            .find("://")
            .ok_or_else(|| MamStorageError::InvalidUri(format!("No scheme separator in: {uri}")))?;
        let scheme = &uri[..colon_slash];
        // Strip the query (`?...`) and fragment (`#...`) components: credentials
        // and the GCS `project` are carried there and must not pollute the
        // bucket / container / prefix path values.
        let rest = &uri[colon_slash + 3..];
        let rest = rest.split(['?', '#']).next().unwrap_or(rest);

        match scheme.to_ascii_lowercase().as_str() {
            "file" => {
                // file:///absolute/path or file://relative
                let path = if rest.starts_with('/') {
                    rest.to_string()
                } else {
                    format!("/{rest}")
                };
                Ok(MamStorageScheme::Local(PathBuf::from(path)))
            }
            "s3" => {
                let (bucket, prefix) = Self::split_bucket_prefix(rest);
                Ok(MamStorageScheme::S3 { bucket, prefix })
            }
            "gs" => {
                let (bucket, prefix) = Self::split_bucket_prefix(rest);
                Ok(MamStorageScheme::Gcs { bucket, prefix })
            }
            "https" => {
                // Azure: https://<account>.blob.core.windows.net/<container>[/<prefix>]
                if rest.contains(".blob.core.windows.net") {
                    let (host, path_part) = rest.split_once('/').ok_or_else(|| {
                        MamStorageError::InvalidUri(format!("Azure URI missing container: {uri}"))
                    })?;
                    let account = host
                        .split('.')
                        .next()
                        .ok_or_else(|| {
                            MamStorageError::InvalidUri(format!(
                                "Cannot extract Azure account from: {host}"
                            ))
                        })?
                        .to_string();
                    let (container, prefix) = path_part
                        .split_once('/')
                        .map(|(c, p)| (c.to_string(), p.to_string()))
                        .unwrap_or_else(|| (path_part.to_string(), String::new()));
                    Ok(MamStorageScheme::Azure {
                        account,
                        container,
                        prefix,
                    })
                } else {
                    Err(MamStorageError::UnsupportedScheme(format!(
                        "https:// is only supported for Azure Blob Storage URLs: {uri}"
                    )))
                }
            }
            other => Err(MamStorageError::UnsupportedScheme(other.to_string())),
        }
    }

    fn split_bucket_prefix(rest: &str) -> (String, String) {
        match rest.split_once('/') {
            Some((bucket, prefix)) => (bucket.to_string(), prefix.to_string()),
            None => (rest.to_string(), String::new()),
        }
    }

    /// Return the `CloudStorage` implementation that handles I/O for `key`.
    ///
    /// This is the real cloud backend when an `s3`/`gcs`/`azure` feature is
    /// enabled and the URI selected a remote scheme; otherwise it is the local
    /// filesystem store (a direct `LocalStorage`, or the temp-dir
    /// "local-shadow" `LocalStorage` of a remote target in degraded mode).
    fn active_backend(&self) -> &dyn oximedia_storage::CloudStorage {
        match &self.inner {
            MamStorageInner::Local(ls) => ls,
            MamStorageInner::Remote {
                backend,
                local_shadow,
                ..
            } => match backend {
                Some(b) => b.as_ref(),
                None => local_shadow,
            },
        }
    }

    /// Store `data` at `key`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage backend fails.
    pub async fn put(&self, key: &str, data: &[u8]) -> std::result::Result<(), MamStorageError> {
        let options = oximedia_storage::UploadOptions::default();
        // Use upload_stream to avoid a tempfile round-trip
        let bytes = bytes::Bytes::copy_from_slice(data);
        let stream: oximedia_storage::ByteStream =
            Box::pin(futures::stream::once(async move { Ok(bytes) }));
        self.active_backend()
            .upload_stream(key, stream, Some(data.len() as u64), options)
            .await
            .map_err(MamStorageError::from)?;
        Ok(())
    }

    /// Retrieve data stored at `key`.
    ///
    /// # Errors
    ///
    /// Returns an error if the key does not exist or the backend fails.
    pub async fn get(&self, key: &str) -> std::result::Result<Vec<u8>, MamStorageError> {
        use futures::StreamExt as _;
        let mut stream = self
            .active_backend()
            .download_stream(key, oximedia_storage::DownloadOptions::default())
            .await
            .map_err(MamStorageError::from)?;
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let b = chunk.map_err(MamStorageError::from)?;
            buf.extend_from_slice(&b);
        }
        Ok(buf)
    }

    /// List keys under `prefix`.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend fails.
    pub async fn list(&self, prefix: &str) -> std::result::Result<Vec<String>, MamStorageError> {
        let options = oximedia_storage::ListOptions {
            prefix: Some(prefix.to_string()),
            delimiter: None,
            max_results: Some(10_000),
            continuation_token: None,
        };
        let result = self
            .active_backend()
            .list_objects(options)
            .await
            .map_err(MamStorageError::from)?;
        Ok(result.objects.into_iter().map(|m| m.key).collect())
    }

    /// Delete the object stored at `key`.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend fails.
    pub async fn delete(&self, key: &str) -> std::result::Result<(), MamStorageError> {
        self.active_backend()
            .delete_object(key)
            .await
            .map_err(MamStorageError::from)
    }

    /// Check whether `key` exists.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend fails.
    pub async fn exists(&self, key: &str) -> std::result::Result<bool, MamStorageError> {
        self.active_backend()
            .object_exists(key)
            .await
            .map_err(MamStorageError::from)
    }

    /// Returns `true` if this storage routes I/O through a real cloud backend.
    ///
    /// Returns `false` for a local-filesystem store and for a remote target
    /// running in pure-Rust local-shadow degraded mode (cloud feature off).
    #[must_use]
    pub fn has_cloud_backend(&self) -> bool {
        matches!(
            &self.inner,
            MamStorageInner::Remote {
                backend: Some(_),
                ..
            }
        )
    }
}

/// Parse `key=value` pairs from the query component of a URI.
///
/// Returns an empty map when the URI has no `?` query part.  Values are
/// percent-decoded for the small set of escapes that occur in credentials and
/// region identifiers (`%20`, `%2F`, `%3A`, `%2B`, `%3D`).
///
/// Only compiled when a cloud feature (or the test harness) needs it.
#[cfg(any(feature = "s3", feature = "gcs", feature = "azure", test))]
fn parse_query_params(uri: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    let Some((_, query)) = uri.split_once('?') else {
        return params;
    };
    // A fragment, if present, terminates the query.
    let query = query.split('#').next().unwrap_or(query);
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        if let Some((k, v)) = pair.split_once('=') {
            params.insert(percent_decode(k), percent_decode(v));
        } else {
            params.insert(percent_decode(pair), String::new());
        }
    }
    params
}

/// Minimal percent-decoder for URI query values.
///
/// Only compiled when a cloud feature (or the test harness) needs it.
#[cfg(any(feature = "s3", feature = "gcs", feature = "azure", test))]
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_backend_type_as_str() {
        assert_eq!(StorageBackendType::Local.as_str(), "local");
        assert_eq!(StorageBackendType::S3.as_str(), "s3");
        assert_eq!(StorageBackendType::Azure.as_str(), "azure");
        assert_eq!(StorageBackendType::GCS.as_str(), "gcs");
    }

    #[test]
    fn test_storage_tier_as_str() {
        assert_eq!(StorageTier::Hot.as_str(), "hot");
        assert_eq!(StorageTier::Warm.as_str(), "warm");
        assert_eq!(StorageTier::Cold.as_str(), "cold");
        assert_eq!(StorageTier::Archive.as_str(), "archive");
    }

    #[test]
    fn test_storage_tier_access_time() {
        assert_eq!(StorageTier::Hot.access_time_seconds(), 1);
        assert_eq!(StorageTier::Warm.access_time_seconds(), 60);
        assert_eq!(StorageTier::Cold.access_time_seconds(), 300);
        assert_eq!(StorageTier::Archive.access_time_seconds(), 3600);
    }

    #[test]
    fn test_storage_tier_cost_multiplier() {
        assert_eq!(StorageTier::Hot.cost_multiplier(), 1.0);
        assert_eq!(StorageTier::Warm.cost_multiplier(), 0.5);
        assert_eq!(StorageTier::Cold.cost_multiplier(), 0.1);
        assert_eq!(StorageTier::Archive.cost_multiplier(), 0.01);
    }

    #[test]
    fn test_storage_metadata_serialization() {
        let metadata = StorageMetadata {
            path: "/test/file.mp4".to_string(),
            size: 1024,
            content_type: Some("video/mp4".to_string()),
            checksum: Some("abc123".to_string()),
            created_at: Some(Utc::now()),
            updated_at: Some(Utc::now()),
        };

        let json = serde_json::to_string(&metadata).expect("should succeed in test");
        let deserialized: StorageMetadata =
            serde_json::from_str(&json).expect("should succeed in test");

        assert_eq!(deserialized.path, "/test/file.mp4");
        assert_eq!(deserialized.size, 1024);
    }

    #[tokio::test]
    async fn test_local_storage_resolve_path() {
        let storage = LocalStorage::new("/var/storage".to_string());
        let resolved = storage.resolve_path("/assets/test.mp4");
        assert_eq!(resolved, PathBuf::from("/var/storage/assets/test.mp4"));
    }

    // ── MamStorage URI parsing ───────────────────────────────────────────────

    #[test]
    fn test_parse_scheme_local_bare_path() {
        let scheme = MamStorage::parse_scheme("/tmp/assets").expect("bare path parses");
        assert_eq!(
            scheme,
            MamStorageScheme::Local(PathBuf::from("/tmp/assets"))
        );
    }

    #[test]
    fn test_parse_scheme_file_uri() {
        let scheme = MamStorage::parse_scheme("file:///var/data").expect("file uri parses");
        assert_eq!(scheme, MamStorageScheme::Local(PathBuf::from("/var/data")));
    }

    #[test]
    fn test_parse_scheme_s3() {
        let scheme = MamStorage::parse_scheme("s3://my-bucket/media/clips").expect("s3 parses");
        assert_eq!(
            scheme,
            MamStorageScheme::S3 {
                bucket: "my-bucket".to_string(),
                prefix: "media/clips".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_scheme_s3_strips_query() {
        // Credentials in the query must not leak into the prefix.
        let scheme = MamStorage::parse_scheme(
            "s3://my-bucket/media?access_key=AK&secret_key=SK&region=eu-west-1",
        )
        .expect("s3 with query parses");
        assert_eq!(
            scheme,
            MamStorageScheme::S3 {
                bucket: "my-bucket".to_string(),
                prefix: "media".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_scheme_gcs_strips_project_query() {
        let scheme = MamStorage::parse_scheme("gs://bucket-x/prefix-y?project=proj-123")
            .expect("gcs with project query parses");
        assert_eq!(
            scheme,
            MamStorageScheme::Gcs {
                bucket: "bucket-x".to_string(),
                prefix: "prefix-y".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_scheme_azure_strips_query() {
        let scheme = MamStorage::parse_scheme(
            "https://acct.blob.core.windows.net/container/prefix?account_key=KEY",
        )
        .expect("azure with query parses");
        assert_eq!(
            scheme,
            MamStorageScheme::Azure {
                account: "acct".to_string(),
                container: "container".to_string(),
                prefix: "prefix".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_scheme_unsupported() {
        let err = MamStorage::parse_scheme("ftp://host/path").expect_err("ftp is unsupported");
        assert!(matches!(err, MamStorageError::UnsupportedScheme(_)));
    }

    #[test]
    fn test_parse_query_params_basic() {
        let params =
            parse_query_params("s3://bucket/prefix?access_key=AK&secret_key=SK&region=us-east-2");
        assert_eq!(params.get("access_key").map(String::as_str), Some("AK"));
        assert_eq!(params.get("secret_key").map(String::as_str), Some("SK"));
        assert_eq!(params.get("region").map(String::as_str), Some("us-east-2"));
    }

    #[test]
    fn test_parse_query_params_none() {
        let params = parse_query_params("s3://bucket/prefix");
        assert!(params.is_empty());
    }

    #[test]
    fn test_parse_query_params_percent_decode() {
        // A secret key containing reserved characters, percent-encoded.
        let params = parse_query_params("s3://b/p?secret_key=a%2Fb%2Bc%3Dd");
        assert_eq!(
            params.get("secret_key").map(String::as_str),
            Some("a/b+c=d")
        );
    }

    #[test]
    fn test_parse_query_params_plus_is_space() {
        let params = parse_query_params("s3://b/p?label=hot+tier");
        assert_eq!(params.get("label").map(String::as_str), Some("hot tier"));
    }

    #[test]
    fn test_parse_query_params_flag_without_value() {
        let params = parse_query_params("s3://b/p?path_style&region=us");
        assert_eq!(params.get("path_style").map(String::as_str), Some(""));
        assert_eq!(params.get("region").map(String::as_str), Some("us"));
    }

    #[test]
    fn test_percent_decode_passthrough() {
        assert_eq!(percent_decode("plain-value"), "plain-value");
        // An invalid escape is left untouched.
        assert_eq!(percent_decode("a%zz"), "a%zz");
    }

    // ── MamStorage local-shadow round-trips (offline; default features) ───────

    #[tokio::test]
    async fn test_mam_storage_local_roundtrip() {
        let root = std::env::temp_dir().join(format!("mam-s9-local-{}", std::process::id()));
        let storage = MamStorage::local(&root).await.expect("local storage");
        assert!(!storage.has_cloud_backend());

        storage
            .put("clip/a.bin", b"hello-mam")
            .await
            .expect("put succeeds");
        assert!(storage.exists("clip/a.bin").await.expect("exists check"));
        let data = storage.get("clip/a.bin").await.expect("get succeeds");
        assert_eq!(data, b"hello-mam");

        let listed = storage.list("clip/").await.expect("list succeeds");
        assert!(listed.iter().any(|k| k.ends_with("a.bin")));

        storage.delete("clip/a.bin").await.expect("delete succeeds");
        assert!(!storage
            .exists("clip/a.bin")
            .await
            .expect("exists after delete"));

        let _ = tokio::fs::remove_dir_all(&root).await;
    }

    // When the `s3` feature is enabled the AWS SDK is initialised during
    // `from_uri`; on macOS the TLS certificate loader panics ("could not load
    // platform certs") when the keychain is unavailable (CI, --all-features
    // runs, sandboxed environments).  The real-credential path is already
    // covered by the `#[ignore]` test below, so we only run this test when the
    // `s3` feature is off and the call exercises the pure-Rust local-shadow.
    #[cfg(not(feature = "s3"))]
    #[tokio::test]
    async fn test_mam_storage_s3_uri_local_shadow_roundtrip() {
        // Without the `s3` feature this routes to the temp-dir local shadow.
        let storage = MamStorage::from_uri("s3://oximedia-s9-test/assets")
            .await
            .expect("s3 uri yields a storage handle");
        assert!(!storage.has_cloud_backend(), "expected local-shadow mode");
        storage
            .put("doc/x.txt", b"shadow-bytes")
            .await
            .expect("shadow put");
        let got = storage.get("doc/x.txt").await.expect("shadow get");
        assert_eq!(got, b"shadow-bytes");
        storage.delete("doc/x.txt").await.expect("shadow delete");
    }

    #[cfg(not(feature = "gcs"))]
    #[tokio::test]
    async fn test_mam_storage_gcs_uri_local_shadow() {
        let storage = MamStorage::from_uri("gs://oximedia-s9-test/media?project=demo")
            .await
            .expect("gs uri yields a storage handle");
        assert!(!storage.has_cloud_backend());
        storage
            .put("g/y.txt", b"gcs-shadow")
            .await
            .expect("shadow put");
        let got = storage.get("g/y.txt").await.expect("shadow get");
        assert_eq!(got, b"gcs-shadow");
        storage.delete("g/y.txt").await.expect("shadow delete");
    }

    #[cfg(not(feature = "azure"))]
    #[tokio::test]
    async fn test_mam_storage_azure_uri_local_shadow() {
        // With the `azure` feature off, no account key is needed; the URI is
        // accepted and routed to the local shadow.
        let storage = MamStorage::from_uri("https://acct.blob.core.windows.net/container/media")
            .await
            .expect("azure uri yields a storage handle");
        assert!(!storage.has_cloud_backend());
        storage
            .put("a/z.txt", b"azure-shadow")
            .await
            .expect("shadow put");
        let got = storage.get("a/z.txt").await.expect("shadow get");
        assert_eq!(got, b"azure-shadow");
        storage.delete("a/z.txt").await.expect("shadow delete");
    }

    // ── Feature-gated real-backend tests (network + credentials; ignored) ────

    #[cfg(feature = "s3")]
    #[tokio::test]
    #[ignore = "requires live S3 credentials and network access"]
    async fn test_mam_storage_s3_real_backend_roundtrip() {
        // Expects AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_REGION and a
        // real bucket named via the OXIMEDIA_S3_TEST_BUCKET env var.
        let bucket = std::env::var("OXIMEDIA_S3_TEST_BUCKET")
            .expect("set OXIMEDIA_S3_TEST_BUCKET for this ignored test");
        let uri = format!("s3://{bucket}/oximedia-mam-s9");
        let storage = MamStorage::from_uri(&uri).await.expect("s3 backend");
        assert!(storage.has_cloud_backend());
        storage
            .put("roundtrip.bin", b"s3-real")
            .await
            .expect("real put");
        let got = storage.get("roundtrip.bin").await.expect("real get");
        assert_eq!(got, b"s3-real");
        storage.delete("roundtrip.bin").await.expect("real delete");
    }

    #[cfg(feature = "gcs")]
    #[tokio::test]
    #[ignore = "requires live GCS credentials (ADC) and network access"]
    async fn test_mam_storage_gcs_real_backend_roundtrip() {
        // Expects Application Default Credentials, GOOGLE_CLOUD_PROJECT, and a
        // real bucket named via the OXIMEDIA_GCS_TEST_BUCKET env var.
        let bucket = std::env::var("OXIMEDIA_GCS_TEST_BUCKET")
            .expect("set OXIMEDIA_GCS_TEST_BUCKET for this ignored test");
        let uri = format!("gs://{bucket}/oximedia-mam-s9");
        let storage = MamStorage::from_uri(&uri).await.expect("gcs backend");
        assert!(storage.has_cloud_backend());
        storage
            .put("roundtrip.bin", b"gcs-real")
            .await
            .expect("real put");
        let got = storage.get("roundtrip.bin").await.expect("real get");
        assert_eq!(got, b"gcs-real");
        storage.delete("roundtrip.bin").await.expect("real delete");
    }

    #[cfg(feature = "azure")]
    #[tokio::test]
    #[ignore = "requires live Azure credentials and network access"]
    async fn test_mam_storage_azure_real_backend_roundtrip() {
        // Expects AZURE_STORAGE_KEY plus a real account/container named via the
        // OXIMEDIA_AZURE_TEST_ACCOUNT / OXIMEDIA_AZURE_TEST_CONTAINER env vars.
        let account = std::env::var("OXIMEDIA_AZURE_TEST_ACCOUNT")
            .expect("set OXIMEDIA_AZURE_TEST_ACCOUNT for this ignored test");
        let container = std::env::var("OXIMEDIA_AZURE_TEST_CONTAINER")
            .expect("set OXIMEDIA_AZURE_TEST_CONTAINER for this ignored test");
        let uri = format!("https://{account}.blob.core.windows.net/{container}/oximedia-mam-s9");
        let storage = MamStorage::from_uri(&uri).await.expect("azure backend");
        assert!(storage.has_cloud_backend());
        storage
            .put("roundtrip.bin", b"azure-real")
            .await
            .expect("real put");
        let got = storage.get("roundtrip.bin").await.expect("real get");
        assert_eq!(got, b"azure-real");
        storage.delete("roundtrip.bin").await.expect("real delete");
    }
}
