//! CDN integration for uploading segments to cloud storage.
//!
//! Each backend has exactly one uploader type, delegating to the matching
//! `oximedia-storage` client under its Cargo feature (`cdn-aws`, `cdn-azure`,
//! `cdn-gcs`).  Without the feature every remote operation returns
//! [`CdnError::FeatureDisabled`] — there is no log-only uploader and no
//! synthesised URL for an object that was never uploaded.

mod azure;
mod gcs;
mod s3;
mod uploader;

pub use azure::{AzureCdnUploader, AZURE_FEATURE};
pub use gcs::{GcsCdnUploader, GCS_FEATURE};
pub use s3::{
    partition_into_parts, upload_to_s3, CdnError, MultipartConfig, S3CdnUploader, S3_FEATURE,
};
pub use uploader::{CdnBackend, CdnConfig, CdnUploader, UploadJob, UploadStatus};
