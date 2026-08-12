//! CDN uploader for distributing live content to cloud storage.
//!
//! This is the binding used by the live RTMP ingest path
//! ([`crate::rtmp::RtmpIngestServer`]).  It dispatches to the typed uploaders
//! ([`S3CdnUploader`], [`AzureCdnUploader`], [`GcsCdnUploader`]), each of which
//! delegates to a real `oximedia-storage` backend under the matching Cargo
//! feature (`cdn-aws` / `cdn-azure` / `cdn-gcs`).
//!
//! ## Honesty contract
//!
//! * There is **no** log-only uploader.  Earlier revisions bound this type to
//!   `S3Uploader` / `GcsUploader` / `AzureUploader` wrappers whose `upload()`
//!   logged a line and returned `Ok(())` with no feature gate at all, so the
//!   production path uploaded nothing even when `cdn-aws` was enabled.  Those
//!   types have been deleted.
//! * [`CdnUploader::with_config`] fails fast when the Cargo feature carrying
//!   the selected backend is disabled, so a server configured for CDN upload
//!   refuses to start rather than silently discarding every packet (and rather
//!   than emitting one error per packet for the lifetime of the stream).
//! * Queueing methods ([`CdnUploader::upload_packet`],
//!   [`CdnUploader::upload_segment`]) return `Ok` only for *queueing*, which is
//!   what they do; the upload outcome is observable through
//!   [`CdnUploader::get_job`] / [`CdnUploader::list_jobs`] and the
//!   `cdn_upload_*` metrics.  [`CdnUploader::upload_bytes_now`] awaits the real
//!   upload and returns the real object URL or a real error.

use crate::cdn::{AzureCdnUploader, CdnError, GcsCdnUploader, S3CdnUploader};
use crate::error::{ServerError, ServerResult};
use crate::metrics::MetricsCollector;
use oximedia_net::rtmp::MediaPacket;
use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};
use uuid::Uuid;

/// Maximum number of upload jobs retained for inspection.
///
/// The live path enqueues one job per packet, so the ledger is bounded: the
/// oldest entries are evicted once this many jobs are tracked.
const MAX_TRACKED_JOBS: usize = 512;

/// Metric: uploads accepted into the queue.
const METRIC_QUEUED: &str = "cdn_upload_queued_total";
/// Metric: uploads that completed against the real backend.
const METRIC_COMPLETED: &str = "cdn_upload_completed_total";
/// Metric: uploads that failed (queue rejection or backend error).
const METRIC_FAILED: &str = "cdn_upload_failed_total";
/// Metric: bytes successfully uploaded.
const METRIC_BYTES: &str = "cdn_upload_bytes_total";

/// CDN backend type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdnBackend {
    /// Amazon S3.
    S3,
    /// Azure Blob Storage.
    Azure,
    /// Google Cloud Storage.
    Gcs,
}

impl CdnBackend {
    /// Name of the Cargo feature that must be enabled for this backend to
    /// perform real network I/O.
    #[must_use]
    pub const fn required_feature(self) -> &'static str {
        match self {
            Self::S3 => crate::cdn::S3_FEATURE,
            Self::Azure => crate::cdn::AZURE_FEATURE,
            Self::Gcs => crate::cdn::GCS_FEATURE,
        }
    }

    /// Whether this build can actually talk to the backend.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        match self {
            Self::S3 => cfg!(feature = "cdn-aws"),
            Self::Azure => cfg!(feature = "cdn-azure"),
            Self::Gcs => cfg!(feature = "cdn-gcs"),
        }
    }
}

/// CDN configuration.
#[derive(Debug, Clone)]
pub struct CdnConfig {
    /// CDN backend.
    pub backend: CdnBackend,

    /// Bucket/container name.
    pub bucket: String,

    /// Region (for Azure: the storage account name).
    pub region: String,

    /// Access key ID.
    pub access_key: String,

    /// Secret access key (for Azure: the account access key).
    pub secret_key: String,

    /// Base path in bucket.
    pub base_path: String,

    /// Enable public access.
    pub public: bool,

    /// Enable CDN distribution (use [`Self::cdn_domain`] for object URLs).
    pub enable_cdn: bool,

    /// CDN domain.
    pub cdn_domain: Option<String>,

    /// Google Cloud project ID (required by the GCS backend; ignored by S3/Azure).
    pub project_id: Option<String>,
}

impl Default for CdnConfig {
    fn default() -> Self {
        Self {
            backend: CdnBackend::S3,
            bucket: String::new(),
            region: "us-east-1".to_string(),
            access_key: String::new(),
            secret_key: String::new(),
            base_path: "live".to_string(),
            public: true,
            enable_cdn: false,
            cdn_domain: None,
            project_id: None,
        }
    }
}

impl CdnConfig {
    /// Validates that this configuration can address a real bucket/container.
    ///
    /// # Errors
    ///
    /// Returns [`ServerError::Internal`] when a required field is missing.
    pub fn validate(&self) -> ServerResult<()> {
        if self.bucket.trim().is_empty() {
            return Err(ServerError::Internal(
                "CDN configuration is missing a bucket/container name; \
                 refusing to start an uploader that cannot address any object"
                    .to_string(),
            ));
        }
        if self.backend == CdnBackend::S3 && self.region.trim().is_empty() {
            return Err(ServerError::Internal(
                "CDN configuration is missing an S3 region".to_string(),
            ));
        }
        Ok(())
    }
}

/// Upload job.
#[derive(Debug, Clone)]
pub struct UploadJob {
    /// Job ID.
    pub id: Uuid,

    /// Stream key.
    pub stream_key: String,

    /// Object key in bucket.
    pub object_key: String,

    /// Data size.
    pub size: u64,

    /// Upload status.
    pub status: UploadStatus,

    /// Created time.
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Completed time.
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Failure reason, set when [`Self::status`] is [`UploadStatus::Failed`].
    pub error: Option<String>,

    /// Object URL, set when [`Self::status`] is [`UploadStatus::Completed`].
    ///
    /// Never populated for a job that did not complete — a URL here always
    /// corresponds to bytes that reached the backend.
    pub url: Option<String>,
}

/// Upload status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadStatus {
    /// Queued, not yet started.
    Pending,
    /// Currently uploading.
    Uploading,
    /// Uploaded successfully.
    Completed,
    /// Upload failed.
    Failed,
}

/// Bounded ledger of upload jobs.
#[derive(Default)]
struct JobLedger {
    /// Jobs by ID.
    jobs: HashMap<Uuid, UploadJob>,
    /// Insertion order, used to evict the oldest jobs.
    order: VecDeque<Uuid>,
}

impl JobLedger {
    /// Inserts a job, evicting the oldest once [`MAX_TRACKED_JOBS`] is exceeded.
    fn insert(&mut self, job: UploadJob) {
        let id = job.id;
        self.jobs.insert(id, job);
        self.order.push_back(id);
        while self.order.len() > MAX_TRACKED_JOBS {
            if let Some(oldest) = self.order.pop_front() {
                self.jobs.remove(&oldest);
            }
        }
    }

    /// Marks a job as in-flight.
    fn mark_uploading(&mut self, id: Uuid) {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.status = UploadStatus::Uploading;
        }
    }

    /// Marks a job completed with the object URL returned by the backend.
    fn mark_completed(&mut self, id: Uuid, url: String) {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.status = UploadStatus::Completed;
            job.completed_at = Some(chrono::Utc::now());
            job.url = Some(url);
        }
    }

    /// Marks a job failed with the underlying error message.
    fn mark_failed(&mut self, id: Uuid, cause: String) {
        if let Some(job) = self.jobs.get_mut(&id) {
            job.status = UploadStatus::Failed;
            job.completed_at = Some(chrono::Utc::now());
            job.error = Some(cause);
        }
    }
}

/// Dispatch target: one of the typed CDN uploaders.
enum BackendUploader {
    /// Amazon S3.
    S3(S3CdnUploader),
    /// Azure Blob Storage.
    Azure(AzureCdnUploader),
    /// Google Cloud Storage.
    Gcs(GcsCdnUploader),
}

impl BackendUploader {
    /// Uploads `data` under `key`, returning the object URL on success.
    async fn upload_bytes(&self, key: &str, data: &[u8]) -> std::result::Result<String, CdnError> {
        match self {
            Self::S3(u) => u.upload_bytes(data, key).await,
            Self::Azure(u) => u.upload_bytes(data, key).await,
            Self::Gcs(u) => u.upload_bytes(data, key).await,
        }
    }

    /// Whether the underlying typed uploader has a real backend.
    const fn is_live(&self) -> bool {
        match self {
            Self::S3(u) => u.is_live(),
            Self::Azure(u) => u.is_live(),
            Self::Gcs(u) => u.is_live(),
        }
    }
}

/// Upload task handed to the worker.
struct UploadTask {
    /// Ledger entry this task belongs to.
    job_id: Uuid,

    /// Object key.
    object_key: String,

    /// Data to upload.
    data: bytes::Bytes,
}

/// CDN uploader.
///
/// See this module's documentation for the honesty contract.
pub struct CdnUploader {
    /// Configuration.
    config: CdnConfig,

    /// Dispatch target.
    backend: Arc<BackendUploader>,

    /// Bounded ledger of upload jobs.
    jobs: Arc<RwLock<JobLedger>>,

    /// Upload queue.
    upload_tx: mpsc::UnboundedSender<UploadTask>,

    /// Optional metrics sink.
    metrics: Option<Arc<MetricsCollector>>,
}

impl std::fmt::Debug for CdnUploader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdnUploader")
            .field("backend", &self.config.backend)
            .field("bucket", &self.config.bucket)
            .field("base_path", &self.config.base_path)
            .field("tracked_jobs", &self.jobs.read().jobs.len())
            .finish()
    }
}

impl CdnUploader {
    /// Creates a CDN uploader from configuration.
    ///
    /// # Errors
    ///
    /// Returns an error when the configuration cannot address a real object
    /// (empty bucket), when the Cargo feature carrying the selected backend is
    /// disabled (in which case nothing could ever be uploaded), or when the
    /// underlying storage client cannot be created.
    pub async fn with_config(config: CdnConfig) -> ServerResult<Self> {
        Self::build(config, None).await
    }

    /// Like [`with_config`](Self::with_config) but records `cdn_upload_*`
    /// counters into `metrics`.
    ///
    /// # Errors
    ///
    /// Same as [`with_config`](Self::with_config).
    pub async fn with_config_and_metrics(
        config: CdnConfig,
        metrics: Arc<MetricsCollector>,
    ) -> ServerResult<Self> {
        Self::build(config, Some(metrics)).await
    }

    /// Shared constructor.
    async fn build(
        config: CdnConfig,
        metrics: Option<Arc<MetricsCollector>>,
    ) -> ServerResult<Self> {
        config.validate()?;

        if !config.backend.is_enabled() {
            return Err(ServerError::Internal(format!(
                "CDN upload requires the `{}` Cargo feature of oximedia-server; \
                 the feature is disabled, so no segment would ever be uploaded. \
                 Rebuild with `--features {}` or disable CDN upload.",
                config.backend.required_feature(),
                config.backend.required_feature()
            )));
        }

        let backend = Arc::new(match config.backend {
            CdnBackend::S3 => BackendUploader::S3(S3CdnUploader::new(&config).await?),
            CdnBackend::Azure => BackendUploader::Azure(AzureCdnUploader::new(&config).await?),
            CdnBackend::Gcs => BackendUploader::Gcs(GcsCdnUploader::new(&config).await?),
        });

        // Defence in depth: the feature check above should make this
        // unreachable, but never run a queue whose uploads cannot land.
        if !backend.is_live() {
            return Err(ServerError::Internal(format!(
                "CDN backend {:?} reported no live client despite feature `{}`",
                config.backend,
                config.backend.required_feature()
            )));
        }

        let (upload_tx, mut upload_rx) = mpsc::unbounded_channel::<UploadTask>();
        let jobs = Arc::new(RwLock::new(JobLedger::default()));

        // Upload worker: drains the queue, performs the real upload, and
        // records the outcome. It logs failures and keeps going so a CDN
        // outage never tears down the ingest loop.
        let worker_backend = Arc::clone(&backend);
        let worker_jobs = Arc::clone(&jobs);
        let worker_metrics = metrics.clone();
        tokio::spawn(async move {
            while let Some(task) = upload_rx.recv().await {
                worker_jobs.write().mark_uploading(task.job_id);
                let size = task.data.len() as u64;

                match worker_backend
                    .upload_bytes(&task.object_key, &task.data)
                    .await
                {
                    Ok(url) => {
                        worker_jobs.write().mark_completed(task.job_id, url);
                        if let Some(ref m) = worker_metrics {
                            m.increment_counter(METRIC_COMPLETED, 1.0);
                            m.increment_counter(METRIC_BYTES, size as f64);
                        }
                    }
                    Err(e) => {
                        error!(
                            key = %task.object_key,
                            "CDN upload failed: {e}"
                        );
                        worker_jobs.write().mark_failed(task.job_id, e.to_string());
                        if let Some(ref m) = worker_metrics {
                            m.increment_counter(METRIC_FAILED, 1.0);
                        }
                    }
                }
            }
            info!("CDN upload worker stopped");
        });

        Ok(Self {
            config,
            backend,
            jobs,
            upload_tx,
            metrics,
        })
    }

    /// Returns the configuration this uploader was built from.
    #[must_use]
    pub const fn config(&self) -> &CdnConfig {
        &self.config
    }

    /// Builds the object key for a stream-scoped resource, **relative to the
    /// configured base path**.
    ///
    /// The base path is applied once, by the typed uploader
    /// (`S3CdnUploader::object_url` and friends); prefixing it here as well
    /// would produce `live/live/<stream>/…`.
    fn stream_object_key(stream_key: &str, name: &str) -> String {
        format!("{}/{}", stream_key.replace('/', "_"), name)
    }

    /// Queues an upload and records a [`UploadJob`] for it.
    ///
    /// # Errors
    ///
    /// Returns an error if the worker queue is closed.
    fn enqueue(
        &self,
        stream_key: &str,
        object_key: String,
        data: bytes::Bytes,
    ) -> ServerResult<Uuid> {
        let id = Uuid::new_v4();
        self.jobs.write().insert(UploadJob {
            id,
            stream_key: stream_key.to_string(),
            object_key: object_key.clone(),
            size: data.len() as u64,
            status: UploadStatus::Pending,
            created_at: chrono::Utc::now(),
            completed_at: None,
            error: None,
            url: None,
        });

        if let Some(ref m) = self.metrics {
            m.increment_counter(METRIC_QUEUED, 1.0);
        }

        if self
            .upload_tx
            .send(UploadTask {
                job_id: id,
                object_key,
                data,
            })
            .is_err()
        {
            let cause = "CDN upload worker is no longer running".to_string();
            self.jobs.write().mark_failed(id, cause.clone());
            if let Some(ref m) = self.metrics {
                m.increment_counter(METRIC_FAILED, 1.0);
            }
            return Err(ServerError::Internal(format!(
                "Failed to queue CDN upload: {cause}"
            )));
        }

        Ok(id)
    }

    /// Queues a media packet for upload.
    ///
    /// Returns once the packet is queued; the upload itself happens on the
    /// worker task.  Inspect [`get_job`](Self::get_job) or the
    /// `cdn_upload_completed_total` / `cdn_upload_failed_total` metrics for the
    /// outcome, or use [`upload_bytes_now`](Self::upload_bytes_now) when the
    /// caller needs the real result.
    ///
    /// # Errors
    ///
    /// Returns an error if the upload could not be queued.
    pub fn upload_packet(&self, stream_key: &str, packet: &MediaPacket) -> ServerResult<Uuid> {
        let object_key =
            Self::stream_object_key(stream_key, &format!("packet_{}.bin", packet.timestamp));
        self.enqueue(stream_key, object_key, packet.data.clone())
    }

    /// Queues a segment for upload.
    ///
    /// Same queueing semantics as [`upload_packet`](Self::upload_packet).
    ///
    /// # Errors
    ///
    /// Returns an error if the upload could not be queued.
    pub fn upload_segment(
        &self,
        stream_key: &str,
        segment_name: &str,
        data: bytes::Bytes,
    ) -> ServerResult<Uuid> {
        let object_key = Self::stream_object_key(stream_key, segment_name);
        self.enqueue(stream_key, object_key, data)
    }

    /// Uploads bytes immediately, awaiting the real backend call.
    ///
    /// Returns the object URL of the uploaded object, or the backend error.
    /// Unlike the queueing methods this never reports success before the bytes
    /// have landed.
    ///
    /// `key` is resolved **relative to [`CdnConfig::base_path`]**, exactly like
    /// the keys built by [`upload_packet`](Self::upload_packet) /
    /// [`upload_segment`](Self::upload_segment): the typed uploader prefixes the
    /// base path once, so `"clips/a.ts"` with `base_path = "live"` lands at
    /// `live/clips/a.ts`.  The key is validated by the typed uploader (empty
    /// keys and path traversal are rejected).
    ///
    /// # Errors
    ///
    /// Propagates the backend error.
    pub async fn upload_bytes_now(&self, key: &str, data: &[u8]) -> ServerResult<String> {
        let url = self.backend.upload_bytes(key, data).await.map_err(|e| {
            if let Some(ref m) = self.metrics {
                m.increment_counter(METRIC_FAILED, 1.0);
            }
            ServerError::from(e)
        })?;
        if let Some(ref m) = self.metrics {
            m.increment_counter(METRIC_COMPLETED, 1.0);
            m.increment_counter(METRIC_BYTES, data.len() as f64);
        }
        Ok(url)
    }

    /// Gets an upload job by ID.
    ///
    /// Returns `None` once the job has been evicted from the bounded ledger
    /// (see `MAX_TRACKED_JOBS`).
    #[must_use]
    pub fn get_job(&self, id: Uuid) -> Option<UploadJob> {
        self.jobs.read().jobs.get(&id).cloned()
    }

    /// Lists the retained jobs, most recently queued last.
    #[must_use]
    pub fn list_jobs(&self) -> Vec<UploadJob> {
        let ledger = self.jobs.read();
        ledger
            .order
            .iter()
            .filter_map(|id| ledger.jobs.get(id).cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_for(backend: CdnBackend) -> CdnConfig {
        CdnConfig {
            backend,
            bucket: "live-bucket".to_string(),
            region: "us-east-1".to_string(),
            access_key: "key".to_string(),
            secret_key: "secret".to_string(),
            base_path: "live".to_string(),
            public: true,
            enable_cdn: false,
            cdn_domain: None,
            project_id: Some("proj".to_string()),
        }
    }

    #[test]
    fn required_feature_names_match_cargo_features() {
        assert_eq!(CdnBackend::S3.required_feature(), "cdn-aws");
        assert_eq!(CdnBackend::Azure.required_feature(), "cdn-azure");
        assert_eq!(CdnBackend::Gcs.required_feature(), "cdn-gcs");
    }

    #[test]
    fn stream_object_key_is_relative_to_the_base_path() {
        // The typed uploader prefixes the base path; doing it here as well
        // would produce `live/live/app_stream/...`.
        let key = CdnUploader::stream_object_key("app/stream", "packet_42.bin");
        assert_eq!(key, "app_stream/packet_42.bin");
        assert!(
            !key.contains("live"),
            "base path must not be composed twice: {key}"
        );
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn composed_object_url_carries_the_base_path_exactly_once() {
        let uploader = S3CdnUploader::new(&config_for(CdnBackend::S3))
            .await
            .expect("init");
        let key = CdnUploader::stream_object_key("app/stream", "packet_1.bin");
        let url = uploader.object_url(&key);
        assert_eq!(
            url,
            "https://live-bucket.s3.us-east-1.amazonaws.com/live/app_stream/packet_1.bin"
        );
        assert_eq!(url.matches("/live/").count(), 1, "{url}");
    }

    #[test]
    fn fully_specified_config_passes_validation() {
        for backend in [CdnBackend::S3, CdnBackend::Azure, CdnBackend::Gcs] {
            config_for(backend)
                .validate()
                .expect("fully specified config must validate");
        }
    }

    #[tokio::test]
    async fn empty_bucket_is_rejected() {
        let config = CdnConfig::default();
        let err = CdnUploader::with_config(config)
            .await
            .expect_err("empty bucket must not build an uploader");
        assert!(err.to_string().contains("bucket"), "{err}");
    }

    #[cfg(not(feature = "cdn-aws"))]
    #[tokio::test]
    async fn s3_backend_requires_feature() {
        let err = CdnUploader::with_config(config_for(CdnBackend::S3))
            .await
            .expect_err("must not build a live-path uploader that cannot upload");
        let msg = err.to_string();
        assert!(
            msg.contains("cdn-aws"),
            "error must name the feature: {msg}"
        );
    }

    #[cfg(not(feature = "cdn-azure"))]
    #[tokio::test]
    async fn azure_backend_requires_feature() {
        let err = CdnUploader::with_config(config_for(CdnBackend::Azure))
            .await
            .expect_err("must not build a live-path uploader that cannot upload");
        assert!(err.to_string().contains("cdn-azure"), "{err}");
    }

    #[cfg(not(feature = "cdn-gcs"))]
    #[tokio::test]
    async fn gcs_backend_requires_feature() {
        let err = CdnUploader::with_config(config_for(CdnBackend::Gcs))
            .await
            .expect_err("must not build a live-path uploader that cannot upload");
        assert!(err.to_string().contains("cdn-gcs"), "{err}");
    }

    #[test]
    fn job_ledger_is_bounded() {
        let mut ledger = JobLedger::default();
        let mut first = None;
        for i in 0..(MAX_TRACKED_JOBS + 10) {
            let id = Uuid::new_v4();
            if i == 0 {
                first = Some(id);
            }
            ledger.insert(UploadJob {
                id,
                stream_key: "s".to_string(),
                object_key: format!("k{i}"),
                size: 0,
                status: UploadStatus::Pending,
                created_at: chrono::Utc::now(),
                completed_at: None,
                error: None,
                url: None,
            });
        }
        assert_eq!(ledger.jobs.len(), MAX_TRACKED_JOBS);
        assert_eq!(ledger.order.len(), MAX_TRACKED_JOBS);
        let evicted = first.expect("first id recorded");
        assert!(
            !ledger.jobs.contains_key(&evicted),
            "oldest job must be evicted"
        );
    }

    #[test]
    fn failed_job_never_carries_a_url() {
        let mut ledger = JobLedger::default();
        let id = Uuid::new_v4();
        ledger.insert(UploadJob {
            id,
            stream_key: "s".to_string(),
            object_key: "k".to_string(),
            size: 4,
            status: UploadStatus::Pending,
            created_at: chrono::Utc::now(),
            completed_at: None,
            error: None,
            url: None,
        });
        ledger.mark_uploading(id);
        ledger.mark_failed(id, "backend down".to_string());
        let job = ledger.jobs.get(&id).expect("job present");
        assert_eq!(job.status, UploadStatus::Failed);
        assert!(job.url.is_none(), "a failed upload must have no object URL");
        assert_eq!(job.error.as_deref(), Some("backend down"));
    }
}
