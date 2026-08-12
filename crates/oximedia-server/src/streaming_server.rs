//! Main streaming server orchestration.

use crate::{
    auth::{WebhookConfig, WebhookNotifier},
    cdn::CdnConfig,
    dash::{DashConfig, DashPackager},
    dvr::{DvrConfig, DvrManager},
    error::ServerResult,
    hls::{HlsConfig, HlsPackager},
    metrics::MetricsCollector,
    record::StreamRecorder,
    rtmp::{RtmpIngestConfig, RtmpIngestServer},
    transcode::TranscodeEngine,
};
use oximedia_net::rtmp::MediaPacket;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info};

/// Streaming server configuration.
#[derive(Debug, Clone)]
pub struct StreamingServerConfig {
    /// RTMP ingest bind address.
    pub rtmp_bind_addr: SocketAddr,

    /// HTTP server bind address (for HLS/DASH).
    pub http_bind_addr: SocketAddr,

    /// HLS configuration.
    pub hls: HlsConfig,

    /// DASH configuration.
    pub dash: DashConfig,

    /// DVR configuration.
    pub dvr: DvrConfig,

    /// Webhook configuration.
    pub webhook: WebhookConfig,

    /// Enable transcoding.
    pub enable_transcoding: bool,

    /// Enable recording.
    pub enable_recording: bool,

    /// Enable CDN upload.
    ///
    /// Forwarded to the RTMP ingest server, which owns the single
    /// [`CdnUploader`](crate::cdn::CdnUploader) instance. Requires
    /// [`Self::cdn`]; server construction fails otherwise.
    pub enable_cdn_upload: bool,

    /// CDN destination used when [`Self::enable_cdn_upload`] is set.
    pub cdn: Option<CdnConfig>,

    /// Enable DVR.
    pub enable_dvr: bool,

    /// Enable webhooks.
    pub enable_webhooks: bool,

    /// Recording directory.
    pub record_dir: String,

    /// DVR storage directory.
    pub dvr_storage_dir: String,
}

impl Default for StreamingServerConfig {
    fn default() -> Self {
        Self {
            // Built directly from octets/port rather than `str::parse`, so
            // this is infallible by construction (no `.expect()` needed).
            rtmp_bind_addr: SocketAddr::from(([0, 0, 0, 0], 1935)),
            http_bind_addr: SocketAddr::from(([0, 0, 0, 0], 8080)),
            hls: HlsConfig::default(),
            dash: DashConfig::default(),
            dvr: DvrConfig::default(),
            webhook: WebhookConfig::default(),
            enable_transcoding: true,
            enable_recording: false,
            enable_cdn_upload: false,
            cdn: None,
            enable_dvr: true,
            enable_webhooks: true,
            record_dir: "./recordings".to_string(),
            dvr_storage_dir: "./dvr".to_string(),
        }
    }
}

/// Active stream coordinator.
#[allow(dead_code)]
struct StreamCoordinator {
    /// Stream key.
    stream_key: String,

    /// Application name.
    app_name: String,

    /// Packet broadcaster.
    packet_tx: broadcast::Sender<MediaPacket>,

    /// Is active.
    active: RwLock<bool>,
}

impl StreamCoordinator {
    /// Creates a new stream coordinator.
    fn new(app_name: impl Into<String>, stream_key: impl Into<String>) -> Self {
        let (packet_tx, _) = broadcast::channel(1000);

        Self {
            stream_key: stream_key.into(),
            app_name: app_name.into(),
            packet_tx,
            active: RwLock::new(true),
        }
    }

    /// Broadcasts a packet to all consumers.
    fn broadcast_packet(&self, packet: MediaPacket) {
        let _ = self.packet_tx.send(packet);
    }

    /// Subscribes to packets.
    fn subscribe(&self) -> broadcast::Receiver<MediaPacket> {
        self.packet_tx.subscribe()
    }

    /// Marks stream as inactive.
    fn deactivate(&self) {
        *self.active.write() = false;
    }

    /// Checks if stream is active.
    #[must_use]
    fn is_active(&self) -> bool {
        *self.active.read()
    }
}

/// Main streaming server.
#[allow(dead_code)]
pub struct StreamingServer {
    /// Configuration.
    config: StreamingServerConfig,

    /// RTMP ingest server.
    rtmp_server: Arc<RtmpIngestServer>,

    /// HLS packager.
    hls_packager: Arc<HlsPackager>,

    /// DASH packager.
    dash_packager: Arc<DashPackager>,

    /// Transcode engine.
    transcode_engine: Option<Arc<TranscodeEngine>>,

    /// Stream recorder.
    recorder: Option<Arc<StreamRecorder>>,

    /// DVR manager.
    dvr_manager: Option<Arc<DvrManager>>,

    /// Webhook notifier.
    webhook_notifier: Option<Arc<WebhookNotifier>>,

    /// Metrics collector.
    metrics: Arc<MetricsCollector>,

    /// Active stream coordinators.
    coordinators: Arc<RwLock<HashMap<String, Arc<StreamCoordinator>>>>,
}

impl StreamingServer {
    /// Creates a new streaming server.
    ///
    /// # Errors
    ///
    /// Returns an error if initialization fails.
    pub async fn new(config: StreamingServerConfig) -> ServerResult<Self> {
        let metrics = Arc::new(MetricsCollector::new());

        // Initialize RTMP server
        let rtmp_config = RtmpIngestConfig {
            bind_addr: config.rtmp_bind_addr,
            enable_transcoding: config.enable_transcoding,
            enable_recording: config.enable_recording,
            enable_cdn_upload: config.enable_cdn_upload,
            cdn: config.cdn.clone(),
            record_dir: config.record_dir.clone(),
            ..Default::default()
        };

        let rtmp_server = Arc::new(RtmpIngestServer::new(rtmp_config, Arc::clone(&metrics)).await?);

        // Initialize packagers
        let hls_packager = Arc::new(HlsPackager::new(config.hls.clone()));
        let dash_packager = Arc::new(DashPackager::new(config.dash.clone()));

        // Initialize optional components
        let transcode_engine = if config.enable_transcoding {
            Some(Arc::new(TranscodeEngine::new().await?))
        } else {
            None
        };

        let recorder = if config.enable_recording {
            Some(Arc::new(StreamRecorder::new(&config.record_dir)?))
        } else {
            None
        };

        // NOTE: the CDN uploader is deliberately NOT duplicated here. The RTMP
        // ingest server constructed above owns the single instance and is the
        // only component that feeds it media; a second uploader built here was
        // never wired to any producer, so it uploaded nothing while suggesting
        // the streaming server had its own CDN path. `RtmpIngestServer::new`
        // already returned an honest error above if CDN upload is enabled
        // without a usable destination.

        let dvr_manager = if config.enable_dvr {
            Some(Arc::new(DvrManager::new(
                config.dvr.clone(),
                &config.dvr_storage_dir,
            )?))
        } else {
            None
        };

        let webhook_notifier = if config.enable_webhooks {
            Some(Arc::new(WebhookNotifier::new(config.webhook.clone())?))
        } else {
            None
        };

        Ok(Self {
            config,
            rtmp_server,
            hls_packager,
            dash_packager,
            transcode_engine,
            recorder,
            dvr_manager,
            webhook_notifier,
            metrics,
            coordinators: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Starts the streaming server.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails to start.
    pub async fn start(&self) -> ServerResult<()> {
        info!("Starting streaming server");

        // Start RTMP ingest
        self.rtmp_server.start().await?;

        info!(
            "Streaming server started - RTMP: {}, HTTP: {}",
            self.config.rtmp_bind_addr, self.config.http_bind_addr
        );

        Ok(())
    }

    /// Starts a new stream.
    pub async fn start_stream(
        &self,
        app_name: impl Into<String>,
        stream_key: impl Into<String>,
    ) -> ServerResult<()> {
        let app_name = app_name.into();
        let stream_key = stream_key.into();
        let key = format!("{}/{}", app_name, stream_key);

        info!("Starting stream: {}", key);

        // Create coordinator
        let coordinator = Arc::new(StreamCoordinator::new(&app_name, &stream_key));

        {
            let mut coordinators = self.coordinators.write();
            coordinators.insert(key.clone(), Arc::clone(&coordinator));
        }

        // Start HLS packaging
        self.hls_packager.start_stream(&stream_key)?;

        // Start DASH packaging
        self.dash_packager.start_stream(&stream_key)?;

        // Start transcoding if enabled
        if let Some(ref engine) = self.transcode_engine {
            let _job = engine.create_job(&stream_key);
            engine.start_transcoding(&stream_key).await?;
        }

        // Start recording if enabled
        if let Some(ref recorder) = self.recorder {
            recorder.start_recording(&stream_key).await?;
        }

        // Create DVR buffer if enabled
        if let Some(ref dvr) = self.dvr_manager {
            dvr.create_buffer(&stream_key);
        }

        // Send webhook notification
        if let Some(ref notifier) = self.webhook_notifier {
            notifier.notify_stream_started(&app_name, &stream_key);
        }

        // Spawn packet processing task
        self.spawn_packet_processor(coordinator, app_name.clone(), stream_key.clone());

        info!("Stream started: {}", key);

        Ok(())
    }

    /// Spawns a packet processor for a stream.
    fn spawn_packet_processor(
        &self,
        coordinator: Arc<StreamCoordinator>,
        app_name: String,
        stream_key: String,
    ) {
        let mut packet_rx = coordinator.subscribe();
        let hls = Arc::clone(&self.hls_packager);
        let dash = Arc::clone(&self.dash_packager);
        let dvr = self.dvr_manager.clone();
        let metrics = Arc::clone(&self.metrics);

        tokio::spawn(async move {
            while let Ok(packet) = packet_rx.recv().await {
                // Update metrics
                metrics.record_packet_received();
                metrics.record_bytes_received(packet.data.len() as u64);

                // Process HLS
                if let Err(e) = hls.process_packet(&stream_key, packet.clone()).await {
                    error!("HLS processing error: {}", e);
                }

                // Process DASH
                if let Err(e) = dash.process_packet(&stream_key, packet.clone()).await {
                    error!("DASH processing error: {}", e);
                }

                // Add to DVR buffer
                if let Some(ref dvr_mgr) = dvr {
                    if let Some(buffer) = dvr_mgr.get_buffer(&stream_key) {
                        buffer.add_packet(packet);
                    }
                }
            }

            info!(
                "Packet processor stopped for stream: {}/{}",
                app_name, stream_key
            );
        });
    }

    /// Stops a stream.
    pub async fn stop_stream(&self, app_name: &str, stream_key: &str) -> ServerResult<()> {
        let key = format!("{}/{}", app_name, stream_key);

        info!("Stopping stream: {}", key);

        // Deactivate coordinator
        if let Some(coordinator) = {
            let mut coordinators = self.coordinators.write();
            coordinators.remove(&key)
        } {
            coordinator.deactivate();
        }

        // Stop HLS packaging
        self.hls_packager.stop_stream(stream_key).await?;

        // Stop DASH packaging
        self.dash_packager.stop_stream(stream_key).await?;

        // Stop transcoding if enabled
        if let Some(ref engine) = self.transcode_engine {
            engine.stop_transcoding(stream_key).await?;
            engine.remove_job(stream_key);
        }

        // Stop recording if enabled
        if let Some(ref recorder) = self.recorder {
            recorder.stop_recording(stream_key).await?;
        }

        // Remove DVR buffer
        if let Some(ref dvr) = self.dvr_manager {
            dvr.remove_buffer(stream_key);
        }

        // Send webhook notification
        if let Some(ref notifier) = self.webhook_notifier {
            notifier.notify_stream_stopped(app_name, stream_key);
        }

        info!("Stream stopped: {}", key);

        Ok(())
    }

    /// Publishes a packet to a stream.
    pub fn publish_packet(&self, app_name: &str, stream_key: &str, packet: MediaPacket) {
        let key = format!("{}/{}", app_name, stream_key);

        let coordinators = self.coordinators.read();
        if let Some(coordinator) = coordinators.get(&key) {
            coordinator.broadcast_packet(packet);
        }
    }

    /// Gets the RTMP server.
    #[must_use]
    pub fn rtmp_server(&self) -> &Arc<RtmpIngestServer> {
        &self.rtmp_server
    }

    /// Gets the metrics collector.
    #[must_use]
    pub fn metrics(&self) -> &Arc<MetricsCollector> {
        &self.metrics
    }

    /// Gets active stream count.
    #[must_use]
    pub fn active_stream_count(&self) -> usize {
        let coordinators = self.coordinators.read();
        coordinators.values().filter(|c| c.is_active()).count()
    }

    /// Shuts down the server.
    pub async fn shutdown(&self) -> ServerResult<()> {
        info!("Shutting down streaming server");

        // Stop all active streams
        let keys: Vec<String> = {
            let coordinators = self.coordinators.read();
            coordinators.keys().cloned().collect()
        };

        for key in keys {
            let parts: Vec<&str> = key.split('/').collect();
            if parts.len() == 2 {
                let _ = self.stop_stream(parts[0], parts[1]).await;
            }
        }

        // Shutdown RTMP server
        self.rtmp_server.shutdown().await?;

        info!("Streaming server shut down");

        Ok(())
    }
}
