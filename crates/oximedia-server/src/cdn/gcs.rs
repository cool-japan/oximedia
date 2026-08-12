//! Google Cloud Storage CDN upload endpoint.
//!
//! GCS uploads are proxied through the `oximedia-storage`
//! [`GcsStorage`](oximedia_storage::gcs::GcsStorage) backend, which implements
//! the full [`CloudStorage`](oximedia_storage::CloudStorage) trait.
//!
//! ## Feature gating — no silent no-ops
//!
//! The real network backend is enabled by the `cdn-gcs` Cargo feature, which
//! transitively turns on `oximedia-storage/gcs`.  **When the feature is
//! disabled every remote operation returns [`CdnError::FeatureDisabled`]**
//! naming the missing feature; nothing is uploaded, deleted or listed, and no
//! object URL is synthesised for an object that was never uploaded.
//!
//! [`GcsCdnUploader::object_url`] still computes the URL an object *would*
//! have, but it is an explicit, side-effect-free address calculation.

use crate::cdn::s3::CdnError;
use crate::cdn::CdnConfig;
use crate::error::ServerResult;
use std::path::Path;
use tracing::info;

#[cfg(feature = "cdn-gcs")]
use bytes::Bytes;
#[cfg(feature = "cdn-gcs")]
use oximedia_storage::{gcs::GcsStorage, CloudStorage, ListOptions, UnifiedConfig, UploadOptions};
#[cfg(feature = "cdn-gcs")]
use std::sync::Arc;

/// Name of the Cargo feature that enables the real GCS backend.
pub const GCS_FEATURE: &str = "cdn-gcs";

/// GCS CDN uploader.
///
/// Implements the same interface as [`super::s3::S3CdnUploader`] so that call
/// sites can be swapped without code changes.  Under the `cdn-gcs` feature
/// every operation performs real network I/O through `GcsStorage`
/// (`oximedia_storage::gcs::GcsStorage`, requires the `gcs` feature on
/// `oximedia-storage`); without it every operation fails with
/// [`CdnError::FeatureDisabled`].
pub struct GcsCdnUploader {
    /// GCS bucket name.
    bucket: String,

    /// Base path within the bucket.
    base_path: String,

    /// Public delivery domain, used in place of the raw bucket endpoint when
    /// [`CdnConfig::enable_cdn`] is set and a domain is configured.
    cdn_domain: Option<String>,

    /// Real GCS backend (present only when the `cdn-gcs` feature is enabled).
    #[cfg(feature = "cdn-gcs")]
    backend: Arc<GcsStorage>,
}

impl GcsCdnUploader {
    /// Creates a new `GcsCdnUploader` from CDN configuration.
    ///
    /// Under the `cdn-gcs` feature this constructs a real `GcsStorage` client.
    /// The GCS backend requires a project ID; it is taken from
    /// [`CdnConfig::project_id`](crate::cdn::CdnConfig::project_id) and falls
    /// back to an empty string when unset (sufficient for object-level
    /// operations, which do not need a project ID).
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying storage client cannot be created.
    pub async fn new(config: &CdnConfig) -> ServerResult<Self> {
        info!(
            bucket = %config.bucket,
            live = cfg!(feature = "cdn-gcs"),
            "GcsCdnUploader: initialising"
        );

        #[cfg(feature = "cdn-gcs")]
        let backend = {
            let project_id = config.project_id.clone().unwrap_or_default();
            let unified = UnifiedConfig::gcs(config.bucket.clone(), project_id);
            let storage = GcsStorage::new(unified)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            Arc::new(storage)
        };

        Ok(Self {
            bucket: config.bucket.clone(),
            base_path: config.base_path.clone(),
            cdn_domain: if config.enable_cdn {
                config.cdn_domain.clone().filter(|d| !d.is_empty())
            } else {
                None
            },
            #[cfg(feature = "cdn-gcs")]
            backend,
        })
    }

    /// Whether this uploader is backed by a real GCS client.
    ///
    /// `false` on builds without the `cdn-gcs` feature; every remote operation
    /// then returns [`CdnError::FeatureDisabled`].
    #[must_use]
    pub const fn is_live(&self) -> bool {
        cfg!(feature = "cdn-gcs")
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
    /// Pure address calculation: performs no I/O and makes no claim that the
    /// object exists.
    #[must_use]
    pub fn object_url(&self, key: &str) -> String {
        let path = self.object_key(key);
        match &self.cdn_domain {
            Some(domain) => format!("https://{}/{}", domain.trim_end_matches('/'), path),
            None => format!("https://storage.googleapis.com/{}/{}", self.bucket, path),
        }
    }

    /// Upload a local file to GCS via resumable upload.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`] for an empty key, [`CdnError::Storage`]
    /// if the upload fails, or [`CdnError::FeatureDisabled`] on builds without
    /// the `cdn-gcs` feature.
    pub async fn upload(
        &self,
        local_path: &Path,
        key: &str,
    ) -> std::result::Result<String, CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-gcs")]
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
                "GcsCdnUploader: file upload complete"
            );
            Ok(self.object_url(key))
        }

        // No feature: fail before touching the file.
        #[cfg(not(feature = "cdn-gcs"))]
        {
            let _ = local_path;
            Err(CdnError::FeatureDisabled {
                operation: "GCS CDN file upload",
                feature: GCS_FEATURE,
            })
        }
    }

    /// Upload raw bytes to GCS.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`] for an empty key, [`CdnError::Storage`]
    /// if the upload fails, or [`CdnError::FeatureDisabled`] on builds without
    /// the `cdn-gcs` feature.
    pub async fn upload_bytes(
        &self,
        data: &[u8],
        key: &str,
    ) -> std::result::Result<String, CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-gcs")]
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
                bucket = %self.bucket,
                key = %object_key,
                bytes = data.len(),
                "GcsCdnUploader: byte upload complete"
            );
            Ok(self.object_url(key))
        }

        #[cfg(not(feature = "cdn-gcs"))]
        {
            let _ = data;
            Err(CdnError::FeatureDisabled {
                operation: "GCS CDN upload",
                feature: GCS_FEATURE,
            })
        }
    }

    /// Generates a signed URL for downloading an object.
    ///
    /// Delegates to the storage backend's real signing implementation.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`], [`CdnError::Storage`] if signing
    /// fails, or [`CdnError::FeatureDisabled`] on builds without the `cdn-gcs`
    /// feature (an unsigned URL would not be a signed URL).
    pub async fn signed_url(
        &self,
        key: &str,
        expires_in_secs: u64,
    ) -> std::result::Result<String, CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-gcs")]
        {
            let object_key = self.object_key(key);
            self.backend
                .generate_presigned_url(&object_key, expires_in_secs)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))
        }

        #[cfg(not(feature = "cdn-gcs"))]
        {
            let _ = expires_in_secs;
            Err(CdnError::FeatureDisabled {
                operation: "GCS signed URL generation",
                feature: GCS_FEATURE,
            })
        }
    }

    /// Deletes an object from GCS.
    ///
    /// # Errors
    ///
    /// Returns [`CdnError::InvalidKey`] for an empty key, [`CdnError::Storage`]
    /// if the delete request fails, or [`CdnError::FeatureDisabled`] on builds
    /// without the `cdn-gcs` feature.
    pub async fn delete(&self, key: &str) -> std::result::Result<(), CdnError> {
        if key.is_empty() {
            return Err(CdnError::InvalidKey("Key must not be empty".to_string()));
        }

        #[cfg(feature = "cdn-gcs")]
        {
            let object_key = self.object_key(key);
            self.backend
                .delete_object(&object_key)
                .await
                .map_err(|e| CdnError::Storage(e.to_string()))?;
            info!(bucket = %self.bucket, key = %object_key, "GcsCdnUploader: delete complete");
            Ok(())
        }

        #[cfg(not(feature = "cdn-gcs"))]
        {
            Err(CdnError::FeatureDisabled {
                operation: "GCS CDN delete",
                feature: GCS_FEATURE,
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
    /// [`CdnError::FeatureDisabled`] on builds without the `cdn-gcs` feature
    /// (an empty list would falsely claim the bucket holds no such objects).
    pub async fn list(&self, prefix: &str) -> std::result::Result<Vec<String>, CdnError> {
        #[cfg(feature = "cdn-gcs")]
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
                "GcsCdnUploader: list complete"
            );
            Ok(keys)
        }

        #[cfg(not(feature = "cdn-gcs"))]
        {
            let _ = prefix;
            Err(CdnError::FeatureDisabled {
                operation: "GCS CDN listing",
                feature: GCS_FEATURE,
            })
        }
    }
}

#[cfg(all(test, not(feature = "cdn-gcs")))]
mod tests {
    use super::*;
    use crate::cdn::CdnBackend;

    fn test_config() -> CdnConfig {
        CdnConfig {
            backend: CdnBackend::Gcs,
            bucket: "test-bucket".to_string(),
            region: "us-east-1".to_string(),
            access_key: String::new(),
            secret_key: String::new(),
            base_path: "media".to_string(),
            public: true,
            enable_cdn: false,
            cdn_domain: None,
            project_id: Some("test-project".to_string()),
        }
    }

    #[cfg(not(feature = "cdn-gcs"))]
    #[tokio::test]
    async fn object_url_uses_gcs_endpoint() {
        let uploader = GcsCdnUploader::new(&test_config()).await.expect("init");
        assert_eq!(
            uploader.object_url("clips/a.mp4"),
            "https://storage.googleapis.com/test-bucket/media/clips/a.mp4"
        );
        assert!(!uploader.is_live());
    }

    #[cfg(not(feature = "cdn-gcs"))]
    #[tokio::test]
    async fn upload_bytes_is_honest_error_without_feature() {
        let uploader = GcsCdnUploader::new(&test_config()).await.expect("init");
        let err = uploader
            .upload_bytes(b"media", "footage/raw.mp4")
            .await
            .expect_err("must not claim success without cdn-gcs");
        let msg = err.to_string();
        assert!(
            msg.contains("cdn-gcs"),
            "error must name the feature: {msg}"
        );
        assert!(
            !msg.contains("storage.googleapis.com"),
            "error must not hand back an object URL: {msg}"
        );
    }

    #[cfg(not(feature = "cdn-gcs"))]
    #[tokio::test]
    async fn empty_key_rejected_before_feature_check() {
        let uploader = GcsCdnUploader::new(&test_config()).await.expect("init");
        let err = uploader
            .upload_bytes(b"x", "")
            .await
            .expect_err("empty key");
        assert!(matches!(err, CdnError::InvalidKey(_)));
    }
}
