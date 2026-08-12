//! Tests for the CDN uploaders — S3, GCS, Azure — and for the live-ingest
//! [`CdnUploader`] binding.
//!
//! The CDN uploaders route through the real `oximedia-storage` backends when
//! the matching `cdn-aws` / `cdn-gcs` / `cdn-azure` feature is enabled.  With
//! the feature off there is **no** fallback: every remote operation returns an
//! honest error naming the missing feature.  No test in this file fabricates
//! success, and none asserts that a URL is produced for an object that was
//! never uploaded.
//!
//! Test groups:
//!
//! * **Feature-off tests** — compiled per backend when its feature is absent.
//!   They run fully offline and assert honest errors plus pure URL arithmetic.
//! * **Live integration tests** — compiled only with the matching `cdn-*`
//!   feature.  Marked `#[ignore]` because they require real cloud credentials
//!   and network access; run them explicitly with
//!   `cargo test -p oximedia-server --features cdn-all -- --ignored`.

use oximedia_server::cdn::CdnConfig;

/// Build an S3/GCS-style CDN config (bucket + region + credentials).
fn default_config() -> CdnConfig {
    CdnConfig {
        backend: oximedia_server::cdn::CdnBackend::S3,
        bucket: "test-bucket".to_string(),
        region: "us-east-1".to_string(),
        access_key: "key".to_string(),
        secret_key: "secret".to_string(),
        base_path: "media".to_string(),
        public: true,
        enable_cdn: false,
        cdn_domain: None,
        project_id: Some("test-project".to_string()),
    }
}

/// Build an Azure-style CDN config: `bucket` is the container, `region` is the
/// storage account name, `secret_key` is the account access key.
fn azure_config() -> CdnConfig {
    CdnConfig {
        backend: oximedia_server::cdn::CdnBackend::Azure,
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

// ════════════════════════════════════════════════════════════════════════════
// CdnConfig / CdnBackend — always compiled (no feature dependency)
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn test_cdn_config_default_has_no_project_id() {
    let cfg = CdnConfig::default();
    assert!(
        cfg.project_id.is_none(),
        "project_id should default to None"
    );
}

#[test]
fn test_cdn_config_project_id_is_settable() {
    let mut cfg = CdnConfig::default();
    cfg.project_id = Some("my-gcp-project".to_string());
    assert_eq!(cfg.project_id.as_deref(), Some("my-gcp-project"));
}

#[test]
fn test_cdn_config_clone_preserves_project_id() {
    let cfg = default_config();
    let cloned = cfg.clone();
    assert_eq!(cloned.project_id, cfg.project_id);
}

#[test]
fn test_cdn_config_default_is_not_addressable() {
    // The default config has no bucket, so it cannot address any object and
    // must not be accepted by an uploader.
    assert!(CdnConfig::default().validate().is_err());
}

#[test]
fn test_backend_feature_names() {
    use oximedia_server::cdn::CdnBackend;
    assert_eq!(CdnBackend::S3.required_feature(), "cdn-aws");
    assert_eq!(CdnBackend::Gcs.required_feature(), "cdn-gcs");
    assert_eq!(CdnBackend::Azure.required_feature(), "cdn-azure");
    assert_eq!(CdnBackend::S3.is_enabled(), cfg!(feature = "cdn-aws"));
    assert_eq!(CdnBackend::Gcs.is_enabled(), cfg!(feature = "cdn-gcs"));
    assert_eq!(CdnBackend::Azure.is_enabled(), cfg!(feature = "cdn-azure"));
}

// ════════════════════════════════════════════════════════════════════════════
// S3 — feature off
// ════════════════════════════════════════════════════════════════════════════

#[cfg(not(feature = "cdn-aws"))]
mod s3_feature_off {
    use super::default_config;
    use oximedia_server::cdn::S3CdnUploader;

    /// Every S3 error must name the feature and must never leak an object URL.
    fn assert_honest(err: &oximedia_server::cdn::CdnError) {
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

    #[tokio::test]
    async fn uploader_reports_it_is_not_live() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        assert!(!uploader.is_live());
    }

    #[tokio::test]
    async fn upload_bytes_single_part_is_honest_error() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        let data = vec![0u8; 1024];
        let err = uploader
            .upload_bytes(&data, "clips/sample.mp4")
            .await
            .expect_err("no upload can happen without cdn-aws");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn upload_bytes_multipart_is_honest_error() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        let data = vec![1u8; 9 * 1024 * 1024]; // above the 8 MiB threshold
        let err = uploader
            .upload_bytes(&data, "large/file.mp4")
            .await
            .expect_err("no upload can happen without cdn-aws");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn upload_file_is_honest_error() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        let dir = std::env::temp_dir().join(format!("oximedia_s3_off_{}", super::nanos()));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let path = dir.join("test.mp4");
        tokio::fs::write(&path, b"fake media").await.expect("write");

        let err = uploader
            .upload(&path, "uploads/test.mp4")
            .await
            .expect_err("no upload can happen without cdn-aws");
        assert_honest(&err);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn presigned_url_is_honest_error() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        let err = uploader
            .presigned_url("asset.mp4", 3600)
            .await
            .expect_err("an unsigned URL is not a presigned URL");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn delete_is_honest_error() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        let err = uploader
            .delete("asset.mp4")
            .await
            .expect_err("nothing was deleted");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn list_is_honest_error_not_empty_vec() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        let err = uploader
            .list("clips/")
            .await
            .expect_err("an empty listing would falsely claim the bucket is empty");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn invalid_keys_are_still_rejected() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        assert!(uploader.upload_bytes(b"data", "").await.is_err());
        assert!(uploader
            .upload_bytes(b"data", "../escape.mp4")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn object_url_is_pure_address_arithmetic() {
        let uploader = S3CdnUploader::new(&default_config())
            .await
            .expect("s3 init");
        assert_eq!(
            uploader.object_url("clips/sample.mp4"),
            "https://test-bucket.s3.us-east-1.amazonaws.com/media/clips/sample.mp4"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// GCS — feature off
// ════════════════════════════════════════════════════════════════════════════

#[cfg(not(feature = "cdn-gcs"))]
mod gcs_feature_off {
    use super::default_config;
    use oximedia_server::cdn::GcsCdnUploader;

    fn assert_honest(err: &oximedia_server::cdn::CdnError) {
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

    #[tokio::test]
    async fn uploader_reports_it_is_not_live() {
        let uploader = GcsCdnUploader::new(&default_config())
            .await
            .expect("gcs init");
        assert!(!uploader.is_live());
    }

    #[tokio::test]
    async fn upload_bytes_is_honest_error() {
        let uploader = GcsCdnUploader::new(&default_config())
            .await
            .expect("gcs init");
        let err = uploader
            .upload_bytes(b"media data", "footage/raw.mp4")
            .await
            .expect_err("no upload can happen without cdn-gcs");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn upload_file_is_honest_error() {
        let uploader = GcsCdnUploader::new(&default_config())
            .await
            .expect("gcs init");
        let dir = std::env::temp_dir().join(format!("oximedia_gcs_off_{}", super::nanos()));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let path = dir.join("video.mp4");
        tokio::fs::write(&path, b"gcs video").await.expect("write");

        let err = uploader
            .upload(&path, "gcs/video.mp4")
            .await
            .expect_err("no upload can happen without cdn-gcs");
        assert_honest(&err);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn signed_url_is_honest_error() {
        let uploader = GcsCdnUploader::new(&default_config())
            .await
            .expect("gcs init");
        let err = uploader
            .signed_url("asset.mp4", 600)
            .await
            .expect_err("an unsigned URL is not a signed URL");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn delete_and_list_are_honest_errors() {
        let uploader = GcsCdnUploader::new(&default_config())
            .await
            .expect("gcs init");
        assert_honest(&uploader.delete("asset.mp4").await.expect_err("delete"));
        assert_honest(&uploader.list("footage/").await.expect_err("list"));
    }

    #[tokio::test]
    async fn object_url_is_pure_address_arithmetic() {
        let uploader = GcsCdnUploader::new(&default_config())
            .await
            .expect("gcs init");
        assert_eq!(
            uploader.object_url("footage/raw.mp4"),
            "https://storage.googleapis.com/test-bucket/media/footage/raw.mp4"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Azure — feature off
// ════════════════════════════════════════════════════════════════════════════

#[cfg(not(feature = "cdn-azure"))]
mod azure_feature_off {
    use super::azure_config;
    use oximedia_server::cdn::AzureCdnUploader;

    fn assert_honest(err: &oximedia_server::cdn::CdnError) {
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

    #[tokio::test]
    async fn uploader_reports_it_is_not_live() {
        let uploader = AzureCdnUploader::new(&azure_config())
            .await
            .expect("azure init");
        assert!(!uploader.is_live());
    }

    #[tokio::test]
    async fn upload_bytes_single_block_is_honest_error() {
        let uploader = AzureCdnUploader::new(&azure_config())
            .await
            .expect("azure init");
        let err = uploader
            .upload_bytes(b"small blob", "blobs/small.mp4")
            .await
            .expect_err("no upload can happen without cdn-azure");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn upload_bytes_multi_block_is_honest_error() {
        let uploader = AzureCdnUploader::new(&azure_config())
            .await
            .expect("azure init");
        let data = vec![0u8; 12 * 1024 * 1024];
        let err = uploader
            .upload_bytes(&data, "blobs/large.mp4")
            .await
            .expect_err("no upload can happen without cdn-azure");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn upload_file_is_honest_error() {
        let uploader = AzureCdnUploader::new(&azure_config())
            .await
            .expect("azure init");
        let dir = std::env::temp_dir().join(format!("oximedia_azure_off_{}", super::nanos()));
        tokio::fs::create_dir_all(&dir).await.expect("mkdir");
        let path = dir.join("content.mp4");
        tokio::fs::write(&path, b"azure blob content")
            .await
            .expect("write");

        let err = uploader
            .upload(&path, "azure/content.mp4")
            .await
            .expect_err("no upload can happen without cdn-azure");
        assert_honest(&err);
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn sas_url_is_honest_error() {
        let uploader = AzureCdnUploader::new(&azure_config())
            .await
            .expect("azure init");
        let err = uploader
            .sas_url("asset.mp4", 3600)
            .await
            .expect_err("an unsigned URL is not a SAS URL");
        assert_honest(&err);
    }

    #[tokio::test]
    async fn delete_and_list_are_honest_errors() {
        let uploader = AzureCdnUploader::new(&azure_config())
            .await
            .expect("azure init");
        assert_honest(&uploader.delete("blob.mp4").await.expect_err("delete"));
        assert_honest(&uploader.list("blobs/").await.expect_err("list"));
    }

    #[tokio::test]
    async fn object_url_is_pure_address_arithmetic() {
        let uploader = AzureCdnUploader::new(&azure_config())
            .await
            .expect("azure init");
        assert_eq!(
            uploader.object_url("blobs/small.mp4"),
            "https://myaccount.blob.core.windows.net/mycontainer/assets/blobs/small.mp4"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Live-ingest binding (CdnUploader) — the RTMP -> CDN production path
// ════════════════════════════════════════════════════════════════════════════

/// The live path must refuse to build an uploader whose backend feature is
/// absent, instead of accepting packets it can never deliver.
#[cfg(not(feature = "cdn-aws"))]
#[tokio::test]
async fn test_live_cdn_uploader_refuses_s3_without_feature() {
    use oximedia_server::cdn::CdnUploader;
    let err = CdnUploader::with_config(default_config())
        .await
        .expect_err("live CDN path must not build without cdn-aws");
    assert!(err.to_string().contains("cdn-aws"), "{err}");
}

#[cfg(not(feature = "cdn-azure"))]
#[tokio::test]
async fn test_live_cdn_uploader_refuses_azure_without_feature() {
    use oximedia_server::cdn::CdnUploader;
    let err = CdnUploader::with_config(azure_config())
        .await
        .expect_err("live CDN path must not build without cdn-azure");
    assert!(err.to_string().contains("cdn-azure"), "{err}");
}

#[cfg(not(feature = "cdn-gcs"))]
#[tokio::test]
async fn test_live_cdn_uploader_refuses_gcs_without_feature() {
    use oximedia_server::cdn::{CdnBackend, CdnUploader};
    let mut config = default_config();
    config.backend = CdnBackend::Gcs;
    let err = CdnUploader::with_config(config)
        .await
        .expect_err("live CDN path must not build without cdn-gcs");
    assert!(err.to_string().contains("cdn-gcs"), "{err}");
}

/// Enabling CDN upload without a destination must fail at construction rather
/// than silently discarding every packet of the stream.
#[tokio::test]
async fn test_rtmp_ingest_refuses_cdn_upload_without_destination() {
    use oximedia_server::metrics::MetricsCollector;
    use oximedia_server::rtmp::{RtmpIngestConfig, RtmpIngestServer};
    use std::net::SocketAddr;
    use std::sync::Arc;

    let config = RtmpIngestConfig {
        bind_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
        enable_transcoding: false,
        enable_recording: false,
        enable_cdn_upload: true,
        cdn: None,
        ..Default::default()
    };
    let Err(err) = RtmpIngestServer::new(config, Arc::new(MetricsCollector::new())).await else {
        panic!("ingest server must refuse CDN upload without a destination");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("cdn"),
        "error should mention CDN config: {msg}"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// Live integration tests — compiled only with the matching cdn-* feature.
// Marked #[ignore]: they need real cloud credentials + network access.
// Run with e.g. `cargo test -p oximedia-server --features cdn-all -- --ignored`.
// ════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "cdn-aws")]
#[tokio::test]
#[ignore = "requires real AWS S3 credentials and network access"]
async fn test_s3_live_upload_delete_roundtrip() {
    use oximedia_server::cdn::S3CdnUploader;
    let uploader = S3CdnUploader::new(&default_config())
        .await
        .expect("s3 init");
    assert!(uploader.is_live());
    let key = "ci-integration/roundtrip.bin";
    let payload = b"oximedia s3 integration payload".to_vec();
    let url = uploader
        .upload_bytes(&payload, key)
        .await
        .expect("s3 live upload");
    assert!(url.contains("test-bucket"));

    let listed = uploader
        .list("ci-integration/")
        .await
        .expect("s3 live list");
    assert!(
        listed.iter().any(|k| k.ends_with("roundtrip.bin")),
        "uploaded object should appear in the listing"
    );

    uploader.delete(key).await.expect("s3 live delete");
}

#[cfg(feature = "cdn-aws")]
#[tokio::test]
#[ignore = "requires real AWS S3 credentials and network access"]
async fn test_s3_live_upload_file() {
    use oximedia_server::cdn::S3CdnUploader;
    let uploader = S3CdnUploader::new(&default_config())
        .await
        .expect("s3 init");
    let dir = std::env::temp_dir().join("oximedia_s3_live");
    tokio::fs::create_dir_all(&dir).await.expect("mkdir");
    let path = dir.join("upload.bin");
    tokio::fs::write(&path, b"file payload")
        .await
        .expect("write");

    let url = uploader
        .upload(&path, "ci-integration/file-upload.bin")
        .await
        .expect("s3 live file upload");
    assert!(!url.is_empty());

    uploader
        .delete("ci-integration/file-upload.bin")
        .await
        .expect("s3 live delete");
    let _ = tokio::fs::remove_dir_all(&dir).await;
}

#[cfg(feature = "cdn-aws")]
#[tokio::test]
#[ignore = "requires real AWS S3 credentials and network access"]
async fn test_live_cdn_uploader_uploads_a_segment() {
    use oximedia_server::cdn::CdnUploader;
    let uploader = CdnUploader::with_config(default_config())
        .await
        .expect("live CDN uploader");
    let url = uploader
        .upload_bytes_now("ci-integration/segment.ts", b"segment bytes")
        .await
        .expect("live segment upload");
    assert!(url.contains("test-bucket"));
}

#[cfg(feature = "cdn-gcs")]
#[tokio::test]
#[ignore = "requires real GCS credentials and network access"]
async fn test_gcs_live_upload_delete_roundtrip() {
    use oximedia_server::cdn::GcsCdnUploader;
    let uploader = GcsCdnUploader::new(&default_config())
        .await
        .expect("gcs init");
    assert!(uploader.is_live());
    let key = "ci-integration/roundtrip.bin";
    let payload = b"oximedia gcs integration payload".to_vec();
    let url = uploader
        .upload_bytes(&payload, key)
        .await
        .expect("gcs live upload");
    assert!(url.contains("storage.googleapis.com"));

    let listed = uploader
        .list("ci-integration/")
        .await
        .expect("gcs live list");
    assert!(
        listed.iter().any(|k| k.ends_with("roundtrip.bin")),
        "uploaded object should appear in the listing"
    );

    uploader.delete(key).await.expect("gcs live delete");
}

#[cfg(feature = "cdn-azure")]
#[tokio::test]
#[ignore = "requires real Azure Blob Storage credentials and network access"]
async fn test_azure_live_upload_delete_roundtrip() {
    use oximedia_server::cdn::AzureCdnUploader;
    let uploader = AzureCdnUploader::new(&azure_config())
        .await
        .expect("azure init");
    assert!(uploader.is_live());
    let key = "ci-integration/roundtrip.bin";
    let payload = b"oximedia azure integration payload".to_vec();
    let url = uploader
        .upload_bytes(&payload, key)
        .await
        .expect("azure live upload");
    assert!(url.contains("blob.core.windows.net"));

    let listed = uploader
        .list("ci-integration/")
        .await
        .expect("azure live list");
    assert!(
        listed.iter().any(|k| k.ends_with("roundtrip.bin")),
        "uploaded object should appear in the listing"
    );

    uploader.delete(key).await.expect("azure live delete");
}

/// Nanosecond suffix for unique temp directories.
#[cfg(not(all(feature = "cdn-aws", feature = "cdn-gcs", feature = "cdn-azure")))]
fn nanos() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}
