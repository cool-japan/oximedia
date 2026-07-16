# oximedia-storage

![Status: Stable](https://img.shields.io/badge/status-stable-green)

Cloud storage abstraction layer for OxiMedia providing unified access to S3, MinIO, Azure Blob Storage, Google Cloud Storage, and local filesystem.

Part of the [oximedia](https://github.com/cool-japan/oximedia) workspace — a comprehensive pure-Rust media processing framework.

Version: 0.2.0 — 2026-07-15 — extensively tested

## Features

- **Unified API** - Single `CloudStorage` trait interface across all cloud providers
- **Streaming** - Efficient streaming uploads/downloads without buffering entire files
- **Multipart Upload** - Automatic handling of large files with resumable checkpoint files
- **Progress Tracking** - Real-time progress callbacks
- **Retry Logic** - Exponential backoff with Weyl-sequence deterministic jitter
- **Parallel Transfers** - Concurrent chunk downloads for large files
- **Local Caching** - Optional LRU cache with write-through/write-back policies
- **Rate Limiting** - Token bucket bandwidth throttling
- **Async/Await** - Full async support with tokio
- **Access Logging** - Structured storage access log and audit trail
- **Deduplication** - Content-addressable deduplication storage
- **Integrity Checking** - Data integrity verification for stored objects
- **Lifecycle Policies** - Age-based transitions, cost tiers, expiration rules
- **Namespace Management** - Logical grouping of objects with hierarchical names
- **Quota Management** - Storage quota enforcement and reporting
- **Replication** - Multi-site replication with policy management
- **Retention Management** - Object retention holds and policies
- **Storage Events** - Publish/subscribe for object lifecycle events
- **Storage Metrics** - Operation counters, gauges, histograms, error rates
- **Tiering** - Automatic storage class tiering
- **Transfer Statistics** - Throughput metrics for uploads/downloads
- **Write-ahead Log** - Crash-safe storage mutation tracking and replay
- **Compression Store** - Transparent compression with ratio tracking (LZ4 and Zstd)
- **MinIO Backend** - S3-compatible self-hosted object storage
- **Batch Metadata Updates** - Chunked metadata update pipeline
- **Connection Pooling** - Idle connection pool with configurable lifetime

## Supported Providers

### Amazon S3
- Standard and multipart uploads
- Presigned URLs
- Object versioning
- Storage classes (Standard, IA, Glacier, etc.)
- Server-side encryption

**Note**: Requires Rust 1.91+ due to AWS SDK requirements. Enabled with the `s3` feature.

### MinIO (S3-compatible)
- S3-compatible self-hosted object storage
- Enabled with the `minio` feature (alias of `s3`)

### Azure Blob Storage
- Block blob operations
- Container management
- Access tiers (Hot/Cool/Archive)
- SAS token support
- Enabled with the `azure` feature

### Google Cloud Storage
- Standard uploads
- Bucket operations
- Object composition
- Signed URLs
- Enabled with the `gcs` feature

### Local Filesystem
- Always available (no feature flag required)
- Optional memory-mapped reads with the `mmap` feature

## Cargo features

| Feature | What it enables |
|---------|-----------------|
| `s3`    | Amazon S3 provider (Rust 1.91+ required) |
| `minio` | MinIO / S3-compatible backend (alias of `s3`) |
| `azure` | Azure Blob Storage provider |
| `gcs`   | Google Cloud Storage provider |
| `mmap`  | Memory-mapped local file reads (`MmapLocalReader`) |

Default: no features enabled.

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
oximedia-storage = { version = "0.2.0", features = ["azure", "gcs"] }

# Enable S3 / MinIO (requires Rust 1.91+)
# oximedia-storage = { version = "0.2.0", features = ["minio"] }
```

```rust
use oximedia_storage::{UnifiedConfig, CloudStorage, UploadOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = UnifiedConfig::azure("my-container", "myaccount")
        .with_credentials("myaccount", "account_key");

    let storage = oximedia_storage::azure::AzureStorage::new(config).await?;

    let options = UploadOptions::default();
    let etag = storage.upload_file(
        "media/video.mp4",
        std::path::Path::new("/local/path/video.mp4"),
        options
    ).await?;

    println!("Uploaded with ETag: {}", etag);
    Ok(())
}
```

## Connection options

### Per-client HTTP tuning (`ConnectionOptions`)

`connection_options::ConnectionOptions` configures HTTP transport for each
provider client.  All settings have production-ready defaults:

| Setting | Default | Description |
|---------|---------|-------------|
| `keep_alive` | `true` | TCP keep-alive probes |
| `keep_alive_interval_secs` | 30 | Probe interval |
| `http2_multiplexing` | `true` | HTTP/2 connection multiplexing |
| `max_concurrent_streams` | 100 | Max simultaneous HTTP/2 streams |
| `connect_timeout_secs` | 10 | TCP + TLS handshake timeout |
| `request_timeout_secs` | 30 | Full request timeout |
| `tcp_nodelay` | `true` | Disable Nagle's algorithm |
| `pool_idle_timeout_secs` | 60 | Idle connection PING interval |

Builder pattern example:

```rust
use oximedia_storage::connection_options::ConnectionOptions;

let opts = ConnectionOptions::default()
    .with_http2(true)
    .with_max_concurrent_streams(200);

// Reports estimated ×4 throughput for HTTP/2 with ≥100 streams
println!("{:.1}×", opts.estimated_throughput_multiplier());
```

### Idle connection pool (`ConnectionPoolConfig`)

`ConnectionPoolConfig` manages how connections are kept alive between requests:

| Setting | Default | Description |
|---------|---------|-------------|
| `max_idle_connections` | 10 | Maximum connections in idle pool |
| `idle_timeout_secs` | 60 | Evict idle connections after this duration |
| `max_lifetime_secs` | 300 | Evict all connections older than this |
| `acquire_timeout_secs` | 10 | Timeout waiting for a pool connection |

`ConnectionManager` maintains a `VecDeque` idle pool, evicts expired entries on
`acquire()`, and caps the pool at `max_idle_connections` on `release()`.

## Retry configuration

`RetryConfig` uses exponential back-off with Weyl-sequence deterministic jitter
to avoid thundering-herd retries:

| Parameter | Default |
|-----------|---------|
| `max_retries` | 3 |
| `backoff_multiplier` | 2× |
| `initial_delay_ms` | 500 ms |
| `max_delay_ms` | 30 000 ms |
| `jitter_factor` | 0.2 |

The following errors are **not retried**: `NotFound`, `PermissionDenied`,
`InvalidKey`, `QuotaExceeded`, `InvalidConfig`, `AuthenticationError`,
`UnsupportedOperation`.

## Transparent compression

The `compression_store` module wraps objects with a 4-byte magic header for
transparent compress/decompress on read:

| Algorithm | Active | Magic |
|-----------|--------|-------|
| LZ4 | Yes | `4C 5A 34 00` |
| Zstd level 3 | Yes | `28 B5 2F FD` |
| Gzip | No (passthrough) | — |
| Brotli | No (passthrough) | — |
| Snappy | No (passthrough) | — |

`CompressionPolicy::Auto` rule:
- Objects < 4 KiB → store uncompressed
- 4 KiB – 1 MiB → LZ4 (fast compression)
- > 1 MiB → Zstd level 3 (high ratio)

## Batch metadata updates

`BatchMetadataUpdater` validates and chunks metadata update requests:
- Key must be non-empty, ≤ 1 024 bytes, and contain no null bytes
- `chunk(batch_size)` splits the validated update list into slices
- Does **not** perform network calls; the caller is responsible for uploading each chunk

## API Overview

- `CloudStorage` — Main async trait: `upload_stream`, `upload_file`, `download_stream`, `download_file`, `list_objects`, `delete_object`, `copy_object`, `generate_presigned_url`
- `UnifiedConfig` — Provider configuration builder: `s3()`, `azure()`, `gcs()`, `with_credentials()`, `with_cache()`
- `StorageProvider` — S3, Azure, GCS
- `ObjectMetadata` — Key, size, content_type, etag, last_modified, custom metadata
- `UploadOptions` — Content type, metadata, storage class, encryption, ACL
- `DownloadOptions` — Byte range, conditional headers
- `ListOptions` / `ListResult` — Pagination and prefix filtering
- `StorageError` / `Result` — Comprehensive error handling
- `ProgressInfo` / `ProgressCallback` — Transfer progress reporting
- Modules: `access_log`, `bandwidth_throttle`, `batch_operations`, `cache`, `cache_layer`, `compression_store`, `connection_options`, `content_type`, `dedup_store`, `integrity_checker`, `inventory_report`, `lazy_metadata`, `lifecycle`, `local`, `migration_planner`, `minio`, `multipart_resumable`, `namespace`, `object_lock`, `object_store`, `object_versioning`, `path_resolver`, `predictive_prefetch`, `presigned_post`, `quota`, `replication`, `replication_policy`, `retention_manager`, `retry`, `server_side_copy`, `storage_events`, `storage_extras`, `storage_metrics`, `storage_migration`, `storage_policy`, `tiering`, `transfer`, `transfer_stats`, `versioning`, `write_ahead_log`

## License

Apache-2.0 — Copyright 2024-2026 COOLJAPAN OU (Team Kitasan)
