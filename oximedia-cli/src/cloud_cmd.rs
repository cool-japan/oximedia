//! Cloud storage CLI commands.
//!
//! Provides commands for uploading, downloading, cloud transcoding,
//! job status, and cost estimation across S3, Azure, and GCS providers.
//!
//! Upload and download operations require provider-specific environment variables:
//!
//! - **S3 / AWS**: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`
//! - **Azure**: `AZURE_STORAGE_ACCOUNT`, `AZURE_STORAGE_KEY`
//! - **GCS**: `GOOGLE_APPLICATION_CREDENTIALS` (service account JSON path)

use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Command definitions
// ---------------------------------------------------------------------------

/// Cloud command subcommands.
#[derive(Subcommand, Debug)]
pub enum CloudCommand {
    /// Upload a file to cloud storage
    Upload {
        /// Local input file
        #[arg(short, long)]
        input: PathBuf,

        /// Cloud provider: s3, azure, gcs
        #[arg(long)]
        provider: String,

        /// Bucket or container name
        #[arg(long)]
        bucket: String,

        /// Remote object key (defaults to filename)
        #[arg(long)]
        key: Option<String>,

        /// Cloud region
        #[arg(long)]
        region: Option<String>,

        /// Force chunked multipart/resumable upload, even for files below
        /// the backend's automatic size threshold (all three backends
        /// already select this path automatically above their own
        /// threshold, so this only changes behaviour for smaller files)
        #[arg(long)]
        multipart: bool,

        /// Bandwidth limit in KB/s
        #[arg(long)]
        bandwidth_limit: Option<u32>,
    },

    /// Download a file from cloud storage
    Download {
        /// Cloud provider: s3, azure, gcs
        #[arg(long)]
        provider: String,

        /// Bucket or container name
        #[arg(long)]
        bucket: String,

        /// Remote object key
        #[arg(long)]
        key: String,

        /// Local output file
        #[arg(short, long)]
        output: PathBuf,

        /// Cloud region
        #[arg(long)]
        region: Option<String>,
    },

    /// Submit a cloud transcoding job
    Transcode {
        /// Cloud provider: s3, azure, gcs
        #[arg(long)]
        provider: String,

        /// Bucket or container name
        #[arg(long)]
        bucket: String,

        /// Input object key
        #[arg(long)]
        input_key: String,

        /// Output object key
        #[arg(long)]
        output_key: String,

        /// Transcoding preset (e.g., av1-1080p, vp9-4k)
        #[arg(long)]
        preset: Option<String>,

        /// Cloud region
        #[arg(long)]
        region: Option<String>,
    },

    /// Check a cloud job status
    Status {
        /// Cloud provider: s3, azure, gcs
        #[arg(long)]
        provider: String,

        /// Job identifier
        #[arg(long)]
        job_id: String,

        /// Cloud region
        #[arg(long)]
        region: Option<String>,
    },

    /// Estimate cloud costs
    Cost {
        /// Cloud provider: s3, azure, gcs
        #[arg(long)]
        provider: String,

        /// Storage amount in GB
        #[arg(long)]
        storage_gb: f64,

        /// Egress (data transfer out) in GB
        #[arg(long)]
        egress_gb: Option<f64>,

        /// Transcoding minutes
        #[arg(long)]
        transcode_minutes: Option<f64>,

        /// Cloud region
        #[arg(long)]
        region: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Cost data
// ---------------------------------------------------------------------------

/// Per-provider pricing constants (approximate USD/month, standard tier).
struct PricingTier {
    storage_per_gb: f64,
    egress_per_gb: f64,
    transcode_per_min: f64,
    name: &'static str,
}

fn pricing_for(provider: &str, region: &str) -> Result<PricingTier> {
    // Simplified pricing model; real implementation would query provider APIs
    let _region = region; // region could vary pricing; here we use defaults
    match provider.to_lowercase().as_str() {
        "s3" | "aws" => Ok(PricingTier {
            storage_per_gb: 0.023,
            egress_per_gb: 0.09,
            transcode_per_min: 0.024,
            name: "AWS S3",
        }),
        "azure" => Ok(PricingTier {
            storage_per_gb: 0.018,
            egress_per_gb: 0.087,
            transcode_per_min: 0.022,
            name: "Azure Blob",
        }),
        "gcs" | "google" => Ok(PricingTier {
            storage_per_gb: 0.020,
            egress_per_gb: 0.12,
            transcode_per_min: 0.025,
            name: "Google Cloud Storage",
        }),
        other => Err(anyhow::anyhow!(
            "Unknown cloud provider '{}'. Supported: s3, azure, gcs",
            other
        )),
    }
}

fn validate_provider(provider: &str) -> Result<()> {
    match provider.to_lowercase().as_str() {
        "s3" | "aws" | "azure" | "gcs" | "google" => Ok(()),
        other => Err(anyhow::anyhow!(
            "Unknown cloud provider '{}'. Supported: s3, azure, gcs",
            other
        )),
    }
}

fn format_provider(provider: &str) -> &str {
    match provider.to_lowercase().as_str() {
        "s3" | "aws" => "AWS S3",
        "azure" => "Azure Blob Storage",
        "gcs" | "google" => "Google Cloud Storage",
        _ => provider,
    }
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

/// Handle cloud command dispatch.
pub async fn handle_cloud_command(command: CloudCommand, json_output: bool) -> Result<()> {
    match command {
        CloudCommand::Upload {
            input,
            provider,
            bucket,
            key,
            region,
            multipart,
            bandwidth_limit,
        } => {
            run_upload(
                &input,
                &provider,
                &bucket,
                &key,
                &region,
                multipart,
                bandwidth_limit,
                json_output,
            )
            .await
        }
        CloudCommand::Download {
            provider,
            bucket,
            key,
            output,
            region,
        } => run_download(&provider, &bucket, &key, &output, &region, json_output).await,
        CloudCommand::Transcode {
            provider,
            bucket,
            input_key,
            output_key,
            preset,
            region,
        } => {
            run_transcode(
                &provider,
                &bucket,
                &input_key,
                &output_key,
                &preset,
                &region,
                json_output,
            )
            .await
        }
        CloudCommand::Status {
            provider,
            job_id,
            region,
        } => run_status(&provider, &job_id, &region, json_output).await,
        CloudCommand::Cost {
            provider,
            storage_gb,
            egress_gb,
            transcode_minutes,
            region,
        } => {
            run_cost(
                &provider,
                storage_gb,
                egress_gb,
                transcode_minutes,
                &region,
                json_output,
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// Provider credential resolution
// ---------------------------------------------------------------------------

/// Cloud provider kinds, normalised from CLI argument strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderKind {
    S3,
    Azure,
    Gcs,
}

impl ProviderKind {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "s3" | "aws" => Some(Self::S3),
            "azure" => Some(Self::Azure),
            "gcs" | "google" => Some(Self::Gcs),
            _ => None,
        }
    }
}

/// Resolved credentials for a provider.
#[derive(Debug)]
struct ProviderCredentials {
    kind: ProviderKind,
    region: String,
    /// S3/Azure access key or storage-account name.
    access_key: Option<String>,
    /// S3/Azure secret key.
    secret_key: Option<String>,
}

/// Read and validate provider credentials from environment variables.
///
/// Returns `Err` with a descriptive message (naming the missing variable) if
/// any required credential is absent.  Never panics.
fn resolve_credentials(provider: &str, region: Option<&str>) -> Result<ProviderCredentials> {
    let kind = ProviderKind::from_str(provider).ok_or_else(|| {
        anyhow::anyhow!("unknown provider: {provider}; supported: s3, azure, gcs")
    })?;

    match kind {
        ProviderKind::S3 => {
            let access_key = std::env::var("AWS_ACCESS_KEY_ID").map_err(|_| {
                anyhow::anyhow!(
                    "AWS_ACCESS_KEY_ID environment variable not set; \
                     configure credentials before uploading to S3"
                )
            })?;
            let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").map_err(|_| {
                anyhow::anyhow!(
                    "AWS_SECRET_ACCESS_KEY environment variable not set; \
                     configure credentials before uploading to S3"
                )
            })?;
            let resolved_region = region
                .map(String::from)
                .or_else(|| std::env::var("AWS_REGION").ok())
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| "us-east-1".to_string());
            Ok(ProviderCredentials {
                kind,
                region: resolved_region,
                access_key: Some(access_key),
                secret_key: Some(secret_key),
            })
        }
        ProviderKind::Azure => {
            let account = std::env::var("AZURE_STORAGE_ACCOUNT").map_err(|_| {
                anyhow::anyhow!(
                    "AZURE_STORAGE_ACCOUNT environment variable not set; \
                     configure credentials before uploading to Azure"
                )
            })?;
            let key = std::env::var("AZURE_STORAGE_KEY").map_err(|_| {
                anyhow::anyhow!(
                    "AZURE_STORAGE_KEY environment variable not set; \
                     configure credentials before uploading to Azure"
                )
            })?;
            Ok(ProviderCredentials {
                kind,
                region: region.unwrap_or("global").to_string(),
                access_key: Some(account),
                secret_key: Some(key),
            })
        }
        ProviderKind::Gcs => {
            // GCS relies on Application Default Credentials; check the env var
            // that most GCS SDK implementations respect.
            let _creds = std::env::var("GOOGLE_APPLICATION_CREDENTIALS").map_err(|_| {
                anyhow::anyhow!(
                    "GOOGLE_APPLICATION_CREDENTIALS environment variable not set; \
                     set it to the path of a service account JSON file before uploading to GCS"
                )
            })?;
            Ok(ProviderCredentials {
                kind,
                region: region.unwrap_or("us-central1").to_string(),
                access_key: None,
                secret_key: None,
            })
        }
    }
}

/// Build a `UnifiedConfig` from resolved credentials.
fn build_unified_config(
    creds: &ProviderCredentials,
    bucket: &str,
) -> oximedia_storage::UnifiedConfig {
    use oximedia_storage::{StorageProvider, UnifiedConfig};

    match creds.kind {
        ProviderKind::S3 => {
            let mut cfg = UnifiedConfig {
                provider: StorageProvider::S3,
                bucket: bucket.to_string(),
                region: Some(creds.region.clone()),
                endpoint: None,
                access_key: creds.access_key.clone(),
                secret_key: creds.secret_key.clone(),
                project_id: None,
                credentials_file: None,
                transfer_acceleration: false,
                path_style: false,
                max_connections: 10,
                timeout_seconds: 300,
                enable_cache: false,
                cache_dir: None,
                max_cache_size: 10 * 1024 * 1024 * 1024,
                retry: oximedia_storage::RetryConfig::default(),
                pool_config: oximedia_storage::ConnectionPoolConfig::default(),
            };
            // Ensure access_key / secret_key are set for explicit credentials
            if let (Some(ak), Some(sk)) = (&creds.access_key, &creds.secret_key) {
                cfg = cfg.with_credentials(ak.clone(), sk.clone());
            }
            cfg
        }
        ProviderKind::Azure => UnifiedConfig {
            provider: StorageProvider::Azure,
            bucket: bucket.to_string(),
            region: None,
            endpoint: None,
            access_key: creds.access_key.clone(),
            secret_key: creds.secret_key.clone(),
            project_id: None,
            credentials_file: None,
            transfer_acceleration: false,
            path_style: false,
            max_connections: 10,
            timeout_seconds: 300,
            enable_cache: false,
            cache_dir: None,
            max_cache_size: 10 * 1024 * 1024 * 1024,
            retry: oximedia_storage::RetryConfig::default(),
            pool_config: oximedia_storage::ConnectionPoolConfig::default(),
        },
        ProviderKind::Gcs => UnifiedConfig {
            provider: StorageProvider::GCS,
            bucket: bucket.to_string(),
            region: Some(creds.region.clone()),
            endpoint: None,
            access_key: None,
            secret_key: None,
            project_id: None,
            credentials_file: None,
            transfer_acceleration: false,
            path_style: false,
            max_connections: 10,
            timeout_seconds: 300,
            enable_cache: false,
            cache_dir: None,
            max_cache_size: 10 * 1024 * 1024 * 1024,
            retry: oximedia_storage::RetryConfig::default(),
            pool_config: oximedia_storage::ConnectionPoolConfig::default(),
        },
    }
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_upload(
    input: &PathBuf,
    provider: &str,
    bucket: &str,
    key: &Option<String>,
    region: &Option<String>,
    multipart: bool,
    bandwidth_limit: Option<u32>,
    json_output: bool,
) -> Result<()> {
    validate_provider(provider)?;

    if !input.exists() {
        return Err(anyhow::anyhow!("Input file not found: {}", input.display()));
    }

    let meta = std::fs::metadata(input).context("Failed to read file metadata")?;
    let remote_key = key.clone().unwrap_or_else(|| {
        input
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });

    // Validate credentials up-front; this returns a clear error message if any
    // required environment variable is missing — never returns a simulation string.
    let creds = resolve_credentials(provider, region.as_deref())?;
    let region_str = creds.region.clone();

    let config = build_unified_config(&creds, bucket);

    // Perform the upload using the real oximedia-storage backend.
    let etag = upload_file_via_storage(
        config,
        creds.kind,
        input,
        &remote_key,
        meta.len(),
        multipart,
    )
    .await
    .with_context(|| format!("Upload to {}/{} failed", bucket, remote_key))?;

    if let Some(limit) = bandwidth_limit {
        // Bandwidth limiting is handled at the OS / network layer; log intent.
        if !json_output {
            eprintln!(
                "Note: requested bandwidth limit {} KB/s (advisory only)",
                limit
            );
        }
    }

    if json_output {
        let result = serde_json::json!({
            "command": "upload",
            "provider": format_provider(provider),
            "bucket": bucket,
            "key": remote_key,
            "region": region_str,
            "size_bytes": meta.len(),
            "etag": etag,
            "multipart_forced": multipart,
            "status": "uploaded",
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Cloud Upload".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:22} {}", "Provider:", format_provider(provider));
        println!("{:22} {}", "Bucket:", bucket);
        println!("{:22} {}", "Remote key:", remote_key);
        println!("{:22} {}", "Region:", region_str);
        println!("{:22} {}", "Local file:", input.display());
        println!(
            "{:22} {:.2} MB",
            "File size:",
            meta.len() as f64 / (1024.0 * 1024.0)
        );
        println!("{:22} {}", "ETag:", etag);
        if multipart {
            println!("{:22} {}", "Multipart:", "forced".cyan());
        }
        println!("{:22} {}", "Status:", "uploaded".green().bold());
    }

    Ok(())
}

/// Decide the size hint passed to [`oximedia_storage::CloudStorage::upload_stream`]
/// for a given upload, honoring `--multipart`.
///
/// All three backends (S3 `multipart_upload`, Azure `upload_blocks`, GCS
/// `resumable_upload`) already select their chunked/resumable upload path
/// whenever the size hint is `None` — see each backend's `upload_stream`:
/// `size.map_or(true, |s| s > THRESHOLD)` (S3, Azure) / `size.is_none_or(...)`
/// (GCS) — and each already selects that same chunked path automatically
/// once the real size crosses its own threshold (10 MB for S3, comparable
/// values for Azure/GCS). So passing `None` genuinely forces the chunked
/// path via the crate's real "unknown size" branch; it only changes
/// observable behaviour for files *below* every backend's auto-multipart
/// threshold.
///
/// For files at or above `FORCE_MULTIPART_SAFE_MAX` we pass the real size
/// instead of `None` even when `--multipart` was requested: multipart
/// already triggers naturally there (so honoring the flag's intent is a
/// truthful no-op), and passing `None` would make S3's chunk-size
/// calculator size chunks from an unknown 0-byte total (a fixed 5 MB
/// chunk), which for very large files can exceed the 10 000-part ceiling
/// and fail an upload that unforced multipart would have completed.
fn upload_size_hint(force_multipart: bool, size: u64) -> Option<u64> {
    /// Conservative safe ceiling for forcing multipart via an unknown-size
    /// hint: comfortably below the point where any backend's fixed minimum
    /// chunk size, applied to the real `size`, would need more than its
    /// maximum part count.
    const FORCE_MULTIPART_SAFE_MAX: u64 = 1024 * 1024 * 1024; // 1 GiB

    if force_multipart && size <= FORCE_MULTIPART_SAFE_MAX {
        None
    } else {
        Some(size)
    }
}

/// Wrap a local file as an [`oximedia_storage::ByteStream`], read in
/// bounded chunks so large files are not buffered into memory at once.
///
/// Only called from the `#[cfg(feature = "s3"/"azure"/"gcs")]` arms of
/// [`upload_file_via_storage`]; with none of those compiled in there is no
/// caller, so this is feature-gated identically rather than left dead.
#[cfg(any(feature = "s3", feature = "azure", feature = "gcs"))]
async fn file_byte_stream(path: &std::path::Path) -> Result<oximedia_storage::ByteStream> {
    let file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Failed to open file for streaming: {}", path.display()))?;

    let stream = futures::stream::try_unfold(file, |mut file| async move {
        use tokio::io::AsyncReadExt;
        const CHUNK: usize = 256 * 1024;
        let mut buffer = vec![0u8; CHUNK];
        let n = file
            .read(&mut buffer)
            .await
            .map_err(oximedia_storage::StorageError::from)?;
        if n == 0 {
            Ok(None)
        } else {
            buffer.truncate(n);
            Ok(Some((bytes::Bytes::from(buffer), file)))
        }
    });

    Ok(Box::pin(stream))
}

/// Dispatch file upload to the appropriate storage backend.
///
/// Uses compile-time feature gates to ensure only compiled backends are called.
/// Returns the ETag string on success.
///
/// Always goes through `CloudStorage::upload_stream` (rather than
/// `upload_file`) so `force_multipart` can genuinely change behaviour via
/// [`upload_size_hint`]: passing an unknown size makes every backend select
/// its real chunked/resumable upload path instead of a single PUT, even for
/// small files.
async fn upload_file_via_storage(
    config: oximedia_storage::UnifiedConfig,
    kind: ProviderKind,
    file_path: &std::path::Path,
    key: &str,
    size: u64,
    force_multipart: bool,
) -> Result<String> {
    use oximedia_storage::UploadOptions;

    let opts = UploadOptions::default();
    let size_hint = upload_size_hint(force_multipart, size);

    match kind {
        ProviderKind::S3 => {
            #[cfg(feature = "s3")]
            {
                use oximedia_storage::CloudStorage as _;
                let storage = oximedia_storage::s3::S3Storage::new(config)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create S3 client: {}", e))?;
                let stream = file_byte_stream(file_path).await?;
                storage
                    .upload_stream(key, stream, size_hint, opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("S3 upload failed: {}", e))
            }
            #[cfg(not(feature = "s3"))]
            {
                let _ = (config, file_path, key, opts, size_hint);
                Err(anyhow::anyhow!(
                    "S3 backend is not compiled in; rebuild oximedia-cli with --features s3"
                ))
            }
        }
        ProviderKind::Azure => {
            #[cfg(feature = "azure")]
            {
                use oximedia_storage::CloudStorage as _;
                let storage = oximedia_storage::azure::AzureStorage::new(config)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create Azure client: {}", e))?;
                let stream = file_byte_stream(file_path).await?;
                storage
                    .upload_stream(key, stream, size_hint, opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("Azure upload failed: {}", e))
            }
            #[cfg(not(feature = "azure"))]
            {
                let _ = (config, file_path, key, opts, size_hint);
                Err(anyhow::anyhow!(
                    "Azure backend is not compiled in; rebuild oximedia-cli with --features azure"
                ))
            }
        }
        ProviderKind::Gcs => {
            #[cfg(feature = "gcs")]
            {
                use oximedia_storage::CloudStorage as _;
                let storage = oximedia_storage::gcs::GcsStorage::new(config)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create GCS client: {}", e))?;
                let stream = file_byte_stream(file_path).await?;
                storage
                    .upload_stream(key, stream, size_hint, opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("GCS upload failed: {}", e))
            }
            #[cfg(not(feature = "gcs"))]
            {
                let _ = (config, file_path, key, opts, size_hint);
                Err(anyhow::anyhow!(
                    "GCS backend is not compiled in; rebuild oximedia-cli with --features gcs"
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

async fn run_download(
    provider: &str,
    bucket: &str,
    key: &str,
    output: &PathBuf,
    region: &Option<String>,
    json_output: bool,
) -> Result<()> {
    validate_provider(provider)?;

    // Validate credentials up-front.
    let creds = resolve_credentials(provider, region.as_deref())?;
    let region_str = creds.region.clone();
    let config = build_unified_config(&creds, bucket);

    // Perform the download using the real oximedia-storage backend.
    download_file_via_storage(config, creds.kind, key, output)
        .await
        .with_context(|| format!("Download of {}/{} failed", bucket, key))?;

    let file_size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);

    if json_output {
        let result = serde_json::json!({
            "command": "download",
            "provider": format_provider(provider),
            "bucket": bucket,
            "key": key,
            "region": region_str,
            "output": output.display().to_string(),
            "size_bytes": file_size,
            "status": "downloaded",
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else if !crate::progress::is_quiet() {
        println!("{}", "Cloud Download".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:22} {}", "Provider:", format_provider(provider));
        println!("{:22} {}", "Bucket:", bucket);
        println!("{:22} {}", "Remote key:", key);
        println!("{:22} {}", "Region:", region_str);
        println!("{:22} {}", "Output:", output.display());
        println!(
            "{:22} {:.2} MB",
            "File size:",
            file_size as f64 / (1024.0 * 1024.0)
        );
        println!("{:22} {}", "Status:", "downloaded".green().bold());
    }

    Ok(())
}

/// Dispatch file download to the appropriate storage backend.
async fn download_file_via_storage(
    config: oximedia_storage::UnifiedConfig,
    kind: ProviderKind,
    key: &str,
    output: &std::path::Path,
) -> Result<()> {
    use oximedia_storage::DownloadOptions;

    let opts = DownloadOptions::default();

    match kind {
        ProviderKind::S3 => {
            #[cfg(feature = "s3")]
            {
                use oximedia_storage::CloudStorage as _;
                let storage = oximedia_storage::s3::S3Storage::new(config)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create S3 client: {}", e))?;
                storage
                    .download_file(key, output, opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("S3 download failed: {}", e))
            }
            #[cfg(not(feature = "s3"))]
            {
                let _ = (config, key, output, opts);
                Err(anyhow::anyhow!(
                    "S3 backend is not compiled in; rebuild oximedia-cli with --features s3"
                ))
            }
        }
        ProviderKind::Azure => {
            #[cfg(feature = "azure")]
            {
                use oximedia_storage::CloudStorage as _;
                let storage = oximedia_storage::azure::AzureStorage::new(config)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create Azure client: {}", e))?;
                storage
                    .download_file(key, output, opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("Azure download failed: {}", e))
            }
            #[cfg(not(feature = "azure"))]
            {
                let _ = (config, key, output, opts);
                Err(anyhow::anyhow!(
                    "Azure backend is not compiled in; rebuild oximedia-cli with --features azure"
                ))
            }
        }
        ProviderKind::Gcs => {
            #[cfg(feature = "gcs")]
            {
                use oximedia_storage::CloudStorage as _;
                let storage = oximedia_storage::gcs::GcsStorage::new(config)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create GCS client: {}", e))?;
                storage
                    .download_file(key, output, opts)
                    .await
                    .map_err(|e| anyhow::anyhow!("GCS download failed: {}", e))
            }
            #[cfg(not(feature = "gcs"))]
            {
                let _ = (config, key, output, opts);
                Err(anyhow::anyhow!(
                    "GCS backend is not compiled in; rebuild oximedia-cli with --features gcs"
                ))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Transcode
// ---------------------------------------------------------------------------

async fn run_transcode(
    _provider: &str,
    _bucket: &str,
    _input_key: &str,
    _output_key: &str,
    _preset: &Option<String>,
    _region: &Option<String>,
    _json_output: bool,
) -> Result<()> {
    // Managed cloud transcoding (AWS MediaConvert, Azure Media Services, etc.) is not
    // implemented in OxiMedia.  Use `oximedia transcode` for local transcoding.
    Err(anyhow::anyhow!(
        "managed cloud transcoding is not available; \
         use 'oximedia transcode' for local transcoding instead"
    ))
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

async fn run_status(
    _provider: &str,
    _job_id: &str,
    _region: &Option<String>,
    _json_output: bool,
) -> Result<()> {
    // Job status tracking requires a managed cloud transcoding service.
    // OxiMedia does not integrate with AWS MediaConvert, Azure Media Services, etc.
    Err(anyhow::anyhow!(
        "cloud job status requires a managed cloud transcoding service; \
         use 'oximedia transcode' for local jobs"
    ))
}

// ---------------------------------------------------------------------------
// Cost estimation
// ---------------------------------------------------------------------------

async fn run_cost(
    provider: &str,
    storage_gb: f64,
    egress_gb: Option<f64>,
    transcode_minutes: Option<f64>,
    region: &Option<String>,
    json_output: bool,
) -> Result<()> {
    let region_str = region.as_deref().unwrap_or("us-east-1");
    let pricing = pricing_for(provider, region_str)?;

    let egress = egress_gb.unwrap_or(0.0);
    let transcode = transcode_minutes.unwrap_or(0.0);

    let storage_cost = storage_gb * pricing.storage_per_gb;
    let egress_cost = egress * pricing.egress_per_gb;
    let transcode_cost = transcode * pricing.transcode_per_min;
    let total_cost = storage_cost + egress_cost + transcode_cost;

    if json_output {
        let result = serde_json::json!({
            "command": "cost",
            "provider": pricing.name,
            "region": region_str,
            "storage_gb": storage_gb,
            "egress_gb": egress,
            "transcode_minutes": transcode,
            "storage_cost_usd": format!("{:.4}", storage_cost),
            "egress_cost_usd": format!("{:.4}", egress_cost),
            "transcode_cost_usd": format!("{:.4}", transcode_cost),
            "total_cost_usd": format!("{:.4}", total_cost),
            "currency": "USD",
            "note": "Estimates based on standard tier pricing",
        });
        let s = serde_json::to_string_pretty(&result).context("Failed to serialize")?;
        println!("{s}");
    } else {
        println!("{}", "Cloud Cost Estimate".green().bold());
        println!("{}", "=".repeat(60));
        println!("{:22} {}", "Provider:", pricing.name);
        println!("{:22} {}", "Region:", region_str);
        println!();
        println!("{}", "Usage".cyan().bold());
        println!("{}", "-".repeat(60));
        println!("{:22} {:.2} GB", "Storage:", storage_gb);
        println!("{:22} {:.2} GB", "Egress:", egress);
        println!("{:22} {:.1} min", "Transcode:", transcode);
        println!();
        println!("{}", "Cost Breakdown (USD/month)".cyan().bold());
        println!("{}", "-".repeat(60));
        println!(
            "{:22} ${:.4}  (${:.4}/GB)",
            "Storage:", storage_cost, pricing.storage_per_gb
        );
        println!(
            "{:22} ${:.4}  (${:.4}/GB)",
            "Egress:", egress_cost, pricing.egress_per_gb
        );
        println!(
            "{:22} ${:.4}  (${:.4}/min)",
            "Transcode:", transcode_cost, pricing.transcode_per_min
        );
        println!("{}", "-".repeat(60));
        println!(
            "{:22} {}",
            "TOTAL:",
            format!("${:.4}", total_cost).green().bold()
        );
        println!();
        println!(
            "{}",
            "Note: Estimates based on standard tier pricing. Actual costs may vary.".dimmed()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_provider_known() {
        assert!(validate_provider("s3").is_ok());
        assert!(validate_provider("azure").is_ok());
        assert!(validate_provider("gcs").is_ok());
        assert!(validate_provider("aws").is_ok());
        assert!(validate_provider("google").is_ok());
    }

    #[test]
    fn test_validate_provider_unknown() {
        assert!(validate_provider("dropbox").is_err());
        assert!(validate_provider("").is_err());
    }

    #[test]
    fn test_pricing_s3() {
        let p = pricing_for("s3", "us-east-1");
        assert!(p.is_ok());
        let p = p.expect("should succeed");
        assert!(p.storage_per_gb > 0.0);
        assert!(p.egress_per_gb > 0.0);
        assert!(p.transcode_per_min > 0.0);
    }

    #[test]
    fn test_pricing_unknown() {
        let p = pricing_for("dropbox", "us-east-1");
        assert!(p.is_err());
    }

    #[test]
    fn test_cost_calculation() {
        let p = pricing_for("s3", "us-east-1").expect("should succeed");
        let storage_cost = 100.0 * p.storage_per_gb;
        let egress_cost = 50.0 * p.egress_per_gb;
        let transcode_cost = 120.0 * p.transcode_per_min;
        let total = storage_cost + egress_cost + transcode_cost;
        assert!(total > 0.0);
        // Sanity: 100GB S3 + 50GB egress + 120min transcode should be > $5
        assert!(total > 5.0);
    }

    #[test]
    fn unknown_provider_returns_err() {
        match resolve_credentials("unknown-provider-xyz", None) {
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("unknown") || msg.contains("provider"),
                    "got: {msg}"
                );
            }
            Ok(_) => panic!("expected error for unknown provider"),
        }
    }

    #[test]
    fn s3_missing_key_id_returns_err() {
        // This test is only reliable when AWS credentials are absent in the environment.
        // We check the error message shape when the var happens to be missing.
        let result = resolve_credentials("s3", None);
        if let Err(e) = result {
            let msg = e.to_string();
            // Must mention AWS_ACCESS_KEY_ID or credentials — never "simulation"
            assert!(
                msg.contains("AWS_ACCESS_KEY_ID") || msg.contains("credentials"),
                "expected credential mention, got: {msg}"
            );
            assert!(
                !msg.to_lowercase().contains("simulation"),
                "must not say simulation: {msg}"
            );
        }
        // If result is Ok (creds are set in the environment), that's also fine.
    }

    #[test]
    fn provider_kind_from_str_roundtrip() {
        assert_eq!(ProviderKind::from_str("s3"), Some(ProviderKind::S3));
        assert_eq!(ProviderKind::from_str("aws"), Some(ProviderKind::S3));
        assert_eq!(ProviderKind::from_str("azure"), Some(ProviderKind::Azure));
        assert_eq!(ProviderKind::from_str("gcs"), Some(ProviderKind::Gcs));
        assert_eq!(ProviderKind::from_str("google"), Some(ProviderKind::Gcs));
        assert_eq!(ProviderKind::from_str("dropbox"), None);
    }

    // ── --multipart real-behaviour tests ─────────────────────────────────
    //
    // `--multipart` previously had no effect at all (a warning said so).
    // These test the pure decision function that now genuinely changes
    // which upload path every backend takes; there is no live-cloud test
    // here on purpose (no fabricated network calls in a unit test).

    #[test]
    fn upload_size_hint_unforced_always_passes_real_size() {
        assert_eq!(upload_size_hint(false, 0), Some(0));
        assert_eq!(upload_size_hint(false, 1_000), Some(1_000));
        assert_eq!(
            upload_size_hint(false, 100 * 1024 * 1024 * 1024),
            Some(100 * 1024 * 1024 * 1024)
        );
    }

    #[test]
    fn upload_size_hint_forced_small_file_forces_unknown_size() {
        // Below the safe ceiling: forcing must genuinely change the hint to
        // `None`, which is what drives every backend's chunked path.
        assert_eq!(upload_size_hint(true, 0), None);
        assert_eq!(upload_size_hint(true, 1_000), None);
        assert_eq!(upload_size_hint(true, 1024 * 1024 * 1024), None);
    }

    #[test]
    fn upload_size_hint_forced_huge_file_keeps_real_size() {
        // Above the safe ceiling: multipart already triggers automatically
        // from the real size, so honoring `--multipart` must not risk the
        // 10 000-part ceiling by reporting an unknown (0-byte) total.
        let huge = 100 * 1024 * 1024 * 1024; // 100 GiB
        assert_eq!(upload_size_hint(true, huge), Some(huge));
    }

    #[cfg(any(feature = "s3", feature = "azure", feature = "gcs"))]
    #[tokio::test]
    async fn file_byte_stream_round_trips_real_bytes() {
        use futures::StreamExt;

        let path = std::env::temp_dir().join(format!(
            "oximedia_cloud_cmd_test_stream_{}.bin",
            std::process::id()
        ));
        let payload = vec![7u8; 600 * 1024]; // > one 256 KiB chunk
        std::fs::write(&path, &payload).expect("write fixture");

        let mut stream = file_byte_stream(&path).await.expect("open stream");
        let mut collected = Vec::new();
        while let Some(chunk) = stream.next().await {
            collected.extend_from_slice(&chunk.expect("chunk read"));
        }

        assert_eq!(
            collected, payload,
            "stream must reproduce the file bytes exactly"
        );
        std::fs::remove_file(&path).ok();
    }
}
