# oximedia-cloud

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Multi-cloud storage and media services integration for OxiMedia, supporting AWS, Azure, and Google Cloud Platform.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Multi-cloud Storage Abstraction** — Unified API for AWS S3, Azure Blob, and Google Cloud Storage
- **AWS Integration** — S3, MediaConvert, MediaLive, MediaPackage, CloudWatch, KMS (aws-sdk-*; opt-in `aws-sdk` feature, see below)
- **Azure Integration** — Azure Blob Storage and Azure Media Services (Pure-Rust REST client with Shared Key / SAS auth via reqwest + hmac)
- **GCP Integration** — Google Cloud Storage and GCP Media Services (REST API via reqwest)
- **Transfer Management** — Retry, resume, multipart upload, bandwidth throttling
- **Cost Optimization** — Storage tier management, cost estimation, egress policy
- **Security and Encryption** — KMS integration, server-side encryption, HMAC signing
- **CDN Integration** — CDN configuration and edge delivery
- **Multi-region** — Region selection and replication policies
- **Cloud Backup** — Incremental, differential, and versioned backups
- **Object Lifecycle** — Tier transitions, expiration, and archival rules
- **Cloud Transcoding** — Cloud-based transcoding pipeline integration
- **Event Bridge** — Event-driven automation

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-cloud = "0.2.0"
```

## Cargo features

The default build is 100% Pure Rust (no C/C++/assembly compiled in). TLS uses
`rustls` with the Pure-Rust `rustls-rustcrypto` provider, installed
automatically by every client constructor.

| Feature | Default | Description |
|---------|---------|-------------|
| `aws-sdk` | no | Official `aws-sdk-*` backend (`S3Storage`, `AwsMediaServices`). **Not Pure Rust**: the AWS smithy TLS stack only ships C-based crypto providers (ring / aws-lc / s2n), so enabling this compiles `ring` (C + assembly). Without it, use `CloudProvider::Generic` / `GenericStorage` for S3-compatible endpoints. |
| `oci` | no | OCI Object Storage conditional-compilation guards (deps are unconditional; Pure Rust). |

```rust
use oximedia_cloud::{CloudProvider, CloudStorage, create_storage};
use bytes::Bytes;

async fn example() -> Result<(), Box<dyn std::error::Error>> {
    let provider = CloudProvider::S3 {
        bucket: "my-bucket".to_string(),
        region: "us-east-1".to_string(),
    };

    let storage = create_storage(provider).await?;
    storage.upload("test.mp4", Bytes::from("data")).await?;
    Ok(())
}
```

## API Overview (46 source files, 701 public items)

**Core types:**
- `create_storage()` — Factory function for cloud storage backends
- `CloudProvider` — AWS S3, Azure Blob, GCS provider enum
- `CloudStorage` (trait) — Unified storage interface
- `ObjectInfo`, `ObjectMetadata` — Object information
- `StorageClass`, `TransferProgress`, `UploadOptions` — Transfer types
- `CostEstimator`, `StorageTier` — Cost management
- `Credentials`, `EncryptionConfig`, `KmsConfig` — Security types

**Backend modules:**
- `aws` — AWS S3, MediaConvert, MediaLive, MediaPackage, CloudWatch, KMS (requires the non-default `aws-sdk` feature)
- `azure` — Azure Blob Storage and Azure Media Services
- `gcp` — Google Cloud Storage and GCP Media Services
- `generic` — Generic storage provider abstraction

**Feature modules:**
- `transfer`, `upload_manager` — Transfer management and multipart uploads
- `cdn`, `cdn_config` — CDN configuration and edge delivery
- `cost`, `cost_model`, `cost_monitor` — Cost optimization and monitoring
- `security` — Encryption, credentials, and KMS integration
- `multicloud`, `multiregion` — Multi-cloud and multi-region strategies
- `cloud_backup` — Backup strategies (incremental, differential, versioned)
- `transcoding`, `transcoding_pipeline` — Cloud transcoding integration

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
