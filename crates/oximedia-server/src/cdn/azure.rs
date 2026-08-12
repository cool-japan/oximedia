//! Azure Blob Storage CDN upload endpoint.
//!
//! Uploads are proxied through the `oximedia-storage`
//! [`AzureStorage`](oximedia_storage::azure::AzureStorage) backend, which
//! implements the full [`CloudStorage`](oximedia_storage::CloudStorage) trait
//! and owns the block-blob staging/commit wire protocol (Put Block / Put Block
//! List) as well as Shared Key authentication.
//!
//! ## Feature gating — no silent no-ops
//!
//! The real network backend is enabled by the `cdn-azure` Cargo feature, which
//! transitively turns on `oximedia-storage/azure`.  **When the feature is
//! disabled every remote operation returns [`CdnError::FeatureDisabled`]**
//! naming the missing feature; nothing is uploaded, deleted or listed, and no
//! blob URL is synthesised for a blob that was never uploaded.
//!
//! [`AzureCdnUploader::object_url`] still computes the URL a blob *would* have,
//! but it is an explicit, side-effect-free address calculation.

use crate::cdn::s3::CdnError;
use crate::cdn::CdnConfig;
use crate::error::ServerResult;
use std::path::Path;
use tracing::info;

#[cfg(feature = "cdn-azure")]
use bytes::Bytes;
#[cfg(feature = "cdn-azure")]
use oximedia_storage::{
    azure::AzureStorage, CloudStorage, ListOptions, UnifiedConfig, UploadOptions,
};
#[cfg(feature = "cdn-azure")]
use std::sync::Arc;

/// Name of the Cargo feature that enables the real Azure backend.
pub const AZURE_FEATURE: &str = "cdn-azure";

/// Azure Blob Storage CDN uploader.
///
/// Implements the same interface as [`super::s3::S3CdnUploader`].  Under the
/// `cdn-azure` feature every operation performs real network I/O through
/// `AzureStorage` (`oximedia_storage::azure::AzureStorage`, requires the
/// `azure` feature on `oximedia-storage`); without it every operation fails
/// with [`CdnError::FeatureDisabled`].
pub struct AzureCdnUploader {
    /// Azure storage account name.
    account: String,

    /// Container name.
    container: String,

    /// Base path within the container.
    base_path: String,

    /// Public delivery domain, used in place of the raw blob endpoint when
    /// [`CdnConfig::enable_cdn`] is set and a domain is configured.
    cdn_domain: Option<String>,

    /// Real Azure backend (present only when the `cdn-azure` feature is enabled).
    #[cfg(feature = "cdn-azure")]
    backend: Arc<AzureStorage>,
}

impl AzureCdnUploader {
    /// Creates a new `AzureCdnUploader` from CDN configuration.
    ///
    /// For Azure, `config.bucket` is the container name, `config.region` is the
    /// storage account name, and `config.secret_key` is the account access key.
    ///
    /// Under the `cdn-azure` feature this constructs a real `AzureStorage`
    /// client; the account key is required for that path.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage client cannot be created.
    pub async fn new(config: &CdnConfig) -> ServerResult<Self> {
        // Treat `region` as account name for Azure; fall back to generic "account".
        let account = if config.region.is_empty() {
            "account".to_string()
        } else {
            config.region.clone()
        };
        info!(
            account = %account,
            container = %config.bucket,
            live = cfg!(feature = "cdn-azure"),
            "AzureCdnUploader: initialising"
        );

        #[cfg(feature = "cdn-azure")]
        let backend = {
            let unified = UnifiedConfig::azure(config.bucket.clone(), account.clone())
                .with_credentials(account.clone(), config.secret_key.clone());
            let storage = AzureStorage::new(unified)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            Arc::new(storage)
        };

        Ok(Self {
            account,
            container: config.bucket.clone(),
            base_path: config.base_path.clone(),
            cdn_domain: if config.enable_cdn {
                config.cdn_domain.clone().filter(|d| !d.is_empty())
            } else {
                None
            },
            #[cfg(feature = "cdn-azure")]
            backend,
        })
    }

    /// Whether this uploader is backed by a real Azure client.
    ///
    /// `false` on builds without the `cdn-azure` feature; every remote
    /// operation then returns [`CdnError::FeatureDisabled`].
    #[must_use]
    pub const fn is_live(&self) -> bool {
        cfg!(feature = "cdn-azure")
    }

    /// Prefix `key` with the configured base path to form the full blob name.
    fn object_key(&self, key: &str) -> String {
        if self.base_path.is_empty() {
            key.to_string()
        } else {
            format!("{}/{}", self.base_path.trim_end_matches('/'), key)
        }
    }

    /// Computes the public URL a blob with `key` would have.
    ///
    /// Pure address calculation: performs no I/O and makes no claim that the
    /// blob exists.
    #[must_use]
    pub fn object_url(&self, key: &str) -> String {
        let path = self.object_key(key);
        match &self.cdn_domain {
            Some(domain) => format!("https://{}/{}", domain.trim_end_matches('/'), path),
            None => format!(
                "https://{}.blob.core.windows.net/{}/{}",
                self.account, self.container, path
            ),
        }
    }

    /// Upload a local file to Azure Blob Storage.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`] for an empty key, [`CdnError::Storage`]
    /// if the upload fails, or [`CdnError::FeatureDisabled`] on builds without
    /// the `cdn-azure` feature.
    pub async fn upload(
        &self,
        local_path: &Path,
        key: &str,
    ) -> std::result::Result<String, CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-azure")]
        {
            let object_key = self.object_key(key);
            self.backend
                .upload_file(&object_key, local_path, UploadOptions::default())
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            info!(
                account = %self.account,
                container = %self.container,
                key = %object_key,
                path = %local_path.display(),
                "AzureCdnUploader: file upload complete"
            );
            Ok(self.object_url(key))
        }

        // No feature: fail before touching the file.
        #[cfg(not(feature = "cdn-azure"))]
        {
            let _ = local_path;
            Err(CdnError::FeatureDisabled {
                operation: "Azure CDN file upload",
                feature: AZURE_FEATURE,
            })
        }
    }

    /// Upload raw bytes to Azure Blob Storage.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::Storage`], or
    /// [`CdnError::FeatureDisabled`].
    pub async fn upload_bytes(
        &self,
        data: &[u8],
        key: &str,
    ) -> std::result::Result<String, CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-azure")]
        {
            let object_key = self.object_key(key);
            let size = data.len() as u64;
            let bytes = Bytes::copy_from_slice(data);
            let stream =
                futures::stream::once(
                    async move { Ok::<Bytes, oximedia_storage::StorageError>(bytes) },
                );
            self.backend
                .upload_stream(
                    &object_key,
                    Box::pin(stream),
                    Some(size),
                    UploadOptions::default(),
                )
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            info!(
                account = %self.account,
                container = %self.container,
                key = %object_key,
                bytes = data.len(),
                "AzureCdnUploader: byte upload complete"
            );
            Ok(self.object_url(key))
        }

        #[cfg(not(feature = "cdn-azure"))]
        {
            let _ = data;
            Err(CdnError::FeatureDisabled {
                operation: "Azure CDN upload",
                feature: AZURE_FEATURE,
            })
        }
    }

    /// Generates a SAS URL for a blob.
    ///
    /// Delegates to the storage backend's real Shared Access Signature
    /// implementation.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::Storage`] if signing
    /// fails, or [`CdnError::FeatureDisabled`] on builds without the
    /// `cdn-azure` feature (an unsigned URL would not be a SAS URL).
    pub async fn sas_url(
        &self,
        key: &str,
        expires_in_secs: u64,
    ) -> std::result::Result<String, CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-azure")]
        {
            let object_key = self.object_key(key);
            self.backend
                .generate_presigned_url(&object_key, expires_in_secs)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))
        }

        #[cfg(not(feature = "cdn-azure"))]
        {
            let _ = expires_in_secs;
            Err(CdnError::FeatureDisabled {
                operation: "Azure SAS URL generation",
                feature: AZURE_FEATURE,
            })
        }
    }

    /// Deletes a blob.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::Storage`], or
    /// [`CdnError::FeatureDisabled`].
    pub async fn delete(&self, key: &str) -> std::result::Result<(), CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-azure")]
        {
            let object_key = self.object_key(key);
            self.backend
                .delete_object(&object_key)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            info!(
                account = %self.account,
                container = %self.container,
                key = %object_key,
                "AzureCdnUploader: delete complete"
            );
            Ok(())
        }

        #[cfg(not(feature = "cdn-azure"))]
        {
            Err(CdnError::FeatureDisabled {
                operation: "Azure CDN delete",
                feature: AZURE_FEATURE,
            })
        }
    }

    /// Lists blobs with a prefix.
    ///
    /// The supplied `prefix` is resolved relative to the configured base path.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::Storage`] if the listing request fails, or
    /// [`CdnError::FeatureDisabled`] on builds without the `cdn-azure` feature
    /// (an empty list would falsely claim the container holds no such blobs).
    pub async fn list(&self, prefix: &str) -> std::result::Result<Vec<String>, CdnError> {
        #[cfg(feature = "cdn-azure")]
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
                account = %self.account,
                container = %self.container,
                prefix = %prefix,
                count = keys.len(),
                "AzureCdnUploader: list complete"
            );
            Ok(keys)
        }

        #[cfg(not(feature = "cdn-azure"))]
        {
            let _ = prefix;
            Err(CdnError::FeatureDisabled {
                operation: "Azure CDN listing",
                feature: AZURE_FEATURE,
            })
        }
    }
}

#[cfg(all(test, not(feature = "cdn-azure")))]
mod tests {
    use super::*;
    use crate::cdn::CdnBackend;

    fn azure_config() -> CdnConfig {
        CdnConfig {
            backend: CdnBackend::Azure,
            bucket: "mycontainer".to_string(),
            region: "myaccount".to_string(),
            access_key: String::new(),
            secret_key: "YWNjb3VudGtleQ==".to_string(),
            base_path: "assets".to_string(),
            public: true,
            enable_cdn: false,
            cdn_domain: None,
            project_id: None,
        }
    }

    #[cfg(not(feature = "cdn-azure"))]
    #[tokio::test]
    async fn object_url_uses_blob_endpoint() {
        let uploader = AzureCdnUploader::new(&azure_config()).await.expect("init");
        assert_eq!(
            uploader.object_url("blobs/a.mp4"),
            "https://myaccount.blob.core.windows.net/mycontainer/assets/blobs/a.mp4"
        );
        assert!(!uploader.is_live());
    }

    #[cfg(not(feature = "cdn-azure"))]
    #[tokio::test]
    async fn upload_bytes_is_honest_error_without_feature() {
        let uploader = AzureCdnUploader::new(&azure_config()).await.expect("init");
        let err = uploader
            .upload_bytes(b"blob", "blobs/small.mp4")
            .await
            .expect_err("must not claim success without cdn-azure");
        let msg = err.to_string();
        assert!(
            msg.contains("cdn-azure"),
            "error must name the feature: {msg}"
        );
        assert!(
            !msg.contains("blob.core.windows.net"),
            "error must not hand back a blob URL: {msg}"
        );
    }

    #[cfg(not(feature = "cdn-azure"))]
    #[tokio::test]
    async fn sas_url_is_honest_error_without_feature() {
        let uploader = AzureCdnUploader::new(&azure_config()).await.expect("init");
        let err = uploader
            .sas_url("blobs/a.mp4", 3600)
            .await
            .expect_err("unsigned URL is not a SAS URL");
        assert!(err.to_string().contains("cdn-azure"));
    }
}
