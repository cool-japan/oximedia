//! RTMP ingest server implementation.

use crate::cdn::{CdnConfig, CdnUploader};
use crate::error::{ServerError, ServerResult};
use crate::metrics::MetricsCollector;
use crate::record::StreamRecorder;
use crate::transcode::TranscodeEngine;
use async_trait::async_trait;
use oximedia_net::rtmp::{
    AuthHandler, AuthResult, MediaPacket as NetMediaPacket, PublishType, RtmpServer,
    RtmpServerBuilder, StreamMetadata, StreamRegistry,
};
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Emitted by [`StreamKeyValidator`] the moment a publish is authorized, so the
/// ingest bridge can subscribe to the corresponding net stream once the
/// oximedia-net RTMP server registers it.
#[derive(Debug, Clone)]
struct PublishEvent {
    /// RTMP application name.
    app: String,
    /// RTMP stream key.
    stream_key: String,
}

/// RTMP ingest server configuration.
#[derive(Debug, Clone)]
pub struct RtmpIngestConfig {
    /// Bind address for RTMP server.
    pub bind_addr: SocketAddr,

    /// Maximum number of concurrent streams.
    pub max_streams: usize,

    /// Enable authentication.
    pub enable_auth: bool,

    /// Enable transcoding.
    pub enable_transcoding: bool,

    /// Enable recording.
    pub enable_recording: bool,

    /// Enable CDN upload.
    ///
    /// Requires [`Self::cdn`] to be set; server construction fails otherwise
    /// rather than starting an ingest that silently discards every upload.
    pub enable_cdn_upload: bool,

    /// CDN destination used when [`Self::enable_cdn_upload`] is set.
    pub cdn: Option<CdnConfig>,

    /// Recording directory.
    pub record_dir: String,

    /// Chunk size for RTMP.
    pub chunk_size: usize,

    /// Max chunk size.
    pub max_chunk_size: usize,

    /// Window acknowledgement size.
    pub window_ack_size: u32,
}

impl Default for RtmpIngestConfig {
    fn default() -> Self {
        Self {
            // Built directly from octets/port rather than `str::parse`, so
            // this is infallible by construction (no `.expect()` needed).
            bind_addr: SocketAddr::from(([0, 0, 0, 0], 1935)),
            max_streams: 1000,
            enable_auth: true,
            enable_transcoding: true,
            enable_recording: false,
            enable_cdn_upload: false,
            cdn: None,
            record_dir: "./recordings".to_string(),
            chunk_size: 4096,
            max_chunk_size: 65536,
            window_ack_size: 2_500_000,
        }
    }
}

/// Stream key validator.
pub struct StreamKeyValidator {
    valid_keys: RwLock<HashMap<String, StreamKeyInfo>>,

    /// Optional notifier: emits a [`PublishEvent`] whenever a publish is
    /// authorized, so the ingest bridge can subscribe to the newly registered
    /// net stream. `None` for standalone use (e.g. unit tests via `new`).
    publish_notifier: RwLock<Option<mpsc::UnboundedSender<PublishEvent>>>,
}

/// Stream key information.
#[derive(Debug, Clone)]
pub struct StreamKeyInfo {
    /// Stream key.
    pub key: String,

    /// Application name.
    pub app_name: String,

    /// User ID.
    pub user_id: Option<String>,

    /// Maximum bitrate (bits per second).
    pub max_bitrate: Option<u64>,

    /// Allowed codecs.
    pub allowed_codecs: Vec<String>,

    /// Is active.
    pub active: bool,
}

impl StreamKeyValidator {
    /// Creates a new stream key validator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            valid_keys: RwLock::new(HashMap::new()),
            publish_notifier: RwLock::new(None),
        }
    }

    /// Installs the publish notifier used by the ingest bridge.
    ///
    /// Called by [`RtmpIngestServer::new`] so that each authorized publish is
    /// forwarded to the bridge loop. Uses interior mutability so it can be set
    /// on an `Arc<StreamKeyValidator>` already shared with the net server.
    fn set_publish_notifier(&self, tx: mpsc::UnboundedSender<PublishEvent>) {
        *self.publish_notifier.write() = Some(tx);
    }

    /// Adds a valid stream key.
    pub fn add_key(&self, info: StreamKeyInfo) {
        let mut keys = self.valid_keys.write();
        keys.insert(info.key.clone(), info);
    }

    /// Removes a stream key.
    pub fn remove_key(&self, key: &str) {
        let mut keys = self.valid_keys.write();
        keys.remove(key);
    }

    /// Validates a stream key.
    #[must_use]
    pub fn validate(&self, app: &str, stream_key: &str) -> bool {
        let keys = self.valid_keys.read();
        if let Some(info) = keys.get(stream_key) {
            info.active && info.app_name == app
        } else {
            // For development, allow all keys
            true
        }
    }

    /// Gets stream key info.
    #[must_use]
    pub fn get_key_info(&self, stream_key: &str) -> Option<StreamKeyInfo> {
        let keys = self.valid_keys.read();
        keys.get(stream_key).cloned()
    }
}

impl Default for StreamKeyValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl AuthHandler for StreamKeyValidator {
    async fn authenticate_connect(
        &self,
        _app: &str,
        _tc_url: &str,
        _params: &std::collections::HashMap<String, String>,
    ) -> AuthResult {
        AuthResult::Success
    }

    async fn authenticate_publish(
        &self,
        app: &str,
        stream_key: &str,
        _publish_type: PublishType,
    ) -> AuthResult {
        if self.validate(app, stream_key) {
            info!("Authenticated publish: {}/{}", app, stream_key);
            // Notify the ingest bridge (if wired) so it can subscribe to this
            // stream once the net server registers it. Clone the sender out of
            // the guard so nothing is held across the send.
            let notifier = self.publish_notifier.read().clone();
            if let Some(tx) = notifier {
                let _ = tx.send(PublishEvent {
                    app: app.to_string(),
                    stream_key: stream_key.to_string(),
                });
            }
            AuthResult::Success
        } else {
            warn!("Rejected publish: {}/{}", app, stream_key);
            AuthResult::Failed("Invalid stream key".to_string())
        }
    }

    async fn authenticate_play(&self, app: &str, stream_key: &str) -> AuthResult {
        if self.validate(app, stream_key) {
            AuthResult::Success
        } else {
            AuthResult::Failed("Invalid stream key".to_string())
        }
    }
}

/// Active ingest stream.
pub struct IngestStream {
    /// Stream ID.
    pub id: Uuid,

    /// Stream key.
    pub stream_key: String,

    /// Application name.
    pub app_name: String,

    /// Metadata.
    pub metadata: StreamMetadata,

    /// Packet sender.
    pub packet_tx: mpsc::UnboundedSender<NetMediaPacket>,

    /// Bytes received.
    pub bytes_received: Arc<RwLock<u64>>,

    /// Packets received.
    pub packets_received: Arc<RwLock<u64>>,

    /// Start time.
    pub start_time: std::time::Instant,
}

/// RTMP ingest server.
pub struct RtmpIngestServer {
    /// Configuration.
    config: RtmpIngestConfig,

    /// RTMP server.
    rtmp_server: Arc<RtmpServer>,

    /// Stream key validator.
    validator: Arc<StreamKeyValidator>,

    /// Active streams.
    streams: Arc<RwLock<HashMap<String, Arc<IngestStream>>>>,

    /// Transcode engine.
    transcode_engine: Option<Arc<TranscodeEngine>>,

    /// Stream recorder.
    recorder: Option<Arc<StreamRecorder>>,

    /// CDN uploader.
    cdn_uploader: Option<Arc<CdnUploader>>,

    /// Metrics collector.
    metrics: Arc<MetricsCollector>,

    /// Receiver for [`PublishEvent`]s from the validator, consumed once by
    /// [`Self::start`] to drive the ingest bridge. Wrapped so the `&self`
    /// `start` can move it out exactly once.
    publish_rx: Mutex<Option<mpsc::UnboundedReceiver<PublishEvent>>>,
}

impl RtmpIngestServer {
    /// Creates a new RTMP ingest server.
    ///
    /// # Errors
    ///
    /// Returns an error if server initialization fails.
    pub async fn new(
        config: RtmpIngestConfig,
        metrics: Arc<MetricsCollector>,
    ) -> ServerResult<Self> {
        let validator = Arc::new(StreamKeyValidator::new());

        // Wire the validator to notify the ingest bridge on each authorized
        // publish. The receiver is consumed once by `start`.
        let (publish_tx, publish_rx) = mpsc::unbounded_channel::<PublishEvent>();
        validator.set_publish_notifier(publish_tx);

        // Build the REAL oximedia-net RTMP server: bind the configured address,
        // apply chunk/window/connection limits, and use our validator as the
        // auth handler (which also feeds the publish notifier above).
        let rtmp_server = RtmpServerBuilder::new()
            .bind_address(config.bind_addr.to_string())
            .chunk_size(config.chunk_size as u32)
            .window_ack_size(config.window_ack_size)
            .max_connections(config.max_streams)
            .auth_handler(Arc::clone(&validator) as Arc<dyn AuthHandler>)
            .build();

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

        // CDN upload is either fully wired (destination + a build that can
        // reach it) or refused at construction. Starting an ingest whose CDN
        // uploads silently vanish would be worse than not starting at all.
        let cdn_uploader = if config.enable_cdn_upload {
            let cdn_config = config.cdn.clone().ok_or_else(|| {
                ServerError::Internal(
                    "enable_cdn_upload = true but RtmpIngestConfig::cdn is None; \
                     supply a CdnConfig or disable CDN upload"
                        .to_string(),
                )
            })?;
            Some(Arc::new(
                CdnUploader::with_config_and_metrics(cdn_config, Arc::clone(&metrics)).await?,
            ))
        } else {
            None
        };

        Ok(Self {
            config,
            rtmp_server: Arc::new(rtmp_server),
            validator,
            streams: Arc::new(RwLock::new(HashMap::new())),
            transcode_engine,
            recorder,
            cdn_uploader,
            metrics,
            publish_rx: Mutex::new(Some(publish_rx)),
        })
    }

    /// Starts the RTMP ingest server.
    ///
    /// # Errors
    ///
    /// Returns an error if the server fails to start.
    pub async fn start(&self) -> ServerResult<()> {
        info!("Starting RTMP ingest server on {}", self.config.bind_addr);

        // 1. Drive the REAL oximedia-net accept loop. `RtmpServer::run` binds
        //    the TCP socket at `config.bind_addr` and accepts publish/play
        //    connections, registering each published stream into
        //    `stream_registry()`. Without this the server never accepts a
        //    single connection.
        let accept_server = Arc::clone(&self.rtmp_server);
        tokio::spawn(async move {
            if let Err(e) = accept_server.run().await {
                error!("RTMP accept loop error: {}", e);
            }
        });

        // 2. Bridge accepted net streams into this server's ingest map so that
        //    transcoding / recording / CDN upload actually observe the media.
        //    The bridge is driven by publish notifications from the validator.
        let bridge_rx = self.publish_rx.lock().take();
        if let Some(publish_rx) = bridge_rx {
            let registry = Arc::clone(self.rtmp_server.stream_registry());
            let streams = Arc::clone(&self.streams);
            let metrics = Arc::clone(&self.metrics);
            let transcode = self.transcode_engine.clone();
            let recorder = self.recorder.clone();
            let cdn = self.cdn_uploader.clone();
            tokio::spawn(async move {
                Self::run_bridge(
                    registry, streams, metrics, transcode, recorder, cdn, publish_rx,
                )
                .await;
            });
        } else {
            warn!("RTMP ingest bridge already started; not starting a second bridge");
        }

        Ok(())
    }

    /// Bridges published net streams into this server's ingest map.
    ///
    /// For every authorized publish (delivered on `publish_rx`), waits for the
    /// oximedia-net server to register the stream in `registry`, then creates
    /// an [`IngestStream`] and forwards the stream's broadcast media into it so
    /// the transcode / record / CDN pipeline runs on the real bytes.
    async fn run_bridge(
        registry: Arc<StreamRegistry>,
        streams: Arc<RwLock<HashMap<String, Arc<IngestStream>>>>,
        metrics: Arc<MetricsCollector>,
        transcode: Option<Arc<TranscodeEngine>>,
        recorder: Option<Arc<StreamRecorder>>,
        cdn: Option<Arc<CdnUploader>>,
        mut publish_rx: mpsc::UnboundedReceiver<PublishEvent>,
    ) {
        while let Some(event) = publish_rx.recv().await {
            let key = format!("{}/{}", event.app, event.stream_key);

            // Idempotent against duplicate/re-published keys.
            if streams.read().contains_key(&key) {
                continue;
            }

            // The net connection handler registers the stream into `registry`
            // *after* `authenticate_publish` returns, so poll briefly until it
            // appears. Bounded (≈2s) so a rejected/aborted publish cannot wedge
            // the bridge.
            let mut active = None;
            for _ in 0..100 {
                if let Some(stream) = registry.get_stream(&key).await {
                    active = Some(stream);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let active = match active {
                Some(stream) => stream,
                None => {
                    warn!(
                        "RTMP publish '{}' was authorized but never registered; not bridged",
                        key
                    );
                    continue;
                }
            };

            // Register the ingest stream (spawns the transcode/record/CDN task)
            // and record it as active.
            let ingest = Self::spawn_ingest_stream(
                &streams,
                &metrics,
                &transcode,
                &recorder,
                &cdn,
                event.app.clone(),
                event.stream_key.clone(),
                active.metadata.clone(),
            );
            metrics.record_stream_active(&event.app, &event.stream_key);

            // Forward media from the net broadcast channel into the ingest
            // packet task until the publisher ends.
            let mut media_rx = active.media_tx.subscribe();
            let seq_headers = Arc::clone(&active.seq_headers);
            let packet_tx = ingest.packet_tx.clone();
            let streams_for_cleanup = Arc::clone(&streams);
            let cleanup_key = key.clone();
            tokio::spawn(async move {
                // `subscribe()` above only observes packets sent after that
                // point, so on a fast loopback the publisher's codec sequence
                // headers can already be gone. Replay them from the registry's
                // cache before forwarding anything live — subscribe first, then
                // read the cache, so a header racing in between is at worst
                // duplicated (the depacketizer absorbs duplicates) instead of
                // lost.
                for packet in seq_headers.read().await.replay_packets() {
                    if packet_tx.send(packet).is_err() {
                        return; // ingest task gone before it started
                    }
                }
                loop {
                    match media_rx.recv().await {
                        Ok(packet) => {
                            if packet_tx.send(packet).is_err() {
                                break; // ingest task gone
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(dropped)) => {
                            warn!(
                                "RTMP ingest bridge lagged on '{}'; dropped {} packets",
                                cleanup_key, dropped
                            );
                        }
                        Err(broadcast::error::RecvError::Closed) => break, // publisher ended
                    }
                }
                // Net stream ended: drop the ingest stream so state stays honest.
                streams_for_cleanup.write().remove(&cleanup_key);
                info!("RTMP ingest bridge closed for stream: {}", cleanup_key);
            });

            info!("Bridged RTMP ingest stream: {}", key);
        }
    }

    /// Creates an [`IngestStream`], inserts it into `streams`, and spawns its
    /// packet-processing task (stats, metrics, transcode, record, CDN).
    ///
    /// Shared by the live bridge ([`Self::run_bridge`]) and the direct
    /// [`Self::register_stream`] entry point.
    fn spawn_ingest_stream(
        streams: &Arc<RwLock<HashMap<String, Arc<IngestStream>>>>,
        metrics: &Arc<MetricsCollector>,
        transcode: &Option<Arc<TranscodeEngine>>,
        recorder: &Option<Arc<StreamRecorder>>,
        cdn: &Option<Arc<CdnUploader>>,
        app_name: String,
        stream_key: String,
        metadata: StreamMetadata,
    ) -> Arc<IngestStream> {
        let id = Uuid::new_v4();
        let (packet_tx, mut packet_rx) = mpsc::unbounded_channel();

        let stream = Arc::new(IngestStream {
            id,
            stream_key: stream_key.clone(),
            app_name: app_name.clone(),
            metadata,
            packet_tx,
            bytes_received: Arc::new(RwLock::new(0)),
            packets_received: Arc::new(RwLock::new(0)),
            start_time: std::time::Instant::now(),
        });

        let key = format!("{}/{}", app_name, stream_key);
        {
            let mut map = streams.write();
            map.insert(key.clone(), Arc::clone(&stream));
        }

        // Spawn packet processing task.
        let stream_clone = Arc::clone(&stream);
        let metrics = Arc::clone(metrics);
        let transcode = transcode.clone();
        let recorder = recorder.clone();
        let cdn = cdn.clone();

        tokio::spawn(async move {
            // Logged once per stream if transcoding is unavailable, so we do
            // not flood the log on every packet.
            let mut transcode_passthrough_logged = false;

            while let Some(packet) = packet_rx.recv().await {
                // Update stats.
                let data_len = packet.data.len() as u64;
                *stream_clone.bytes_received.write() += data_len;
                *stream_clone.packets_received.write() += 1;

                // Record metrics.
                metrics.record_bytes_received(data_len);
                metrics.record_packet_received();

                // Transcode when enabled. `process_packet` honestly reports
                // that real-time ingest transcoding is unimplemented; on that
                // error we degrade to stream-copy (pass-through) — the same
                // packet still flows to the recorder / CDN below. We never
                // claim a transcode happened.
                if let Some(ref engine) = transcode {
                    if let Err(e) = engine.process_packet(&packet).await {
                        if !transcode_passthrough_logged {
                            warn!(
                                "Transcoding unavailable for stream '{}' ({}); \
                                 passing through (stream-copy)",
                                stream_clone.stream_key, e
                            );
                            transcode_passthrough_logged = true;
                        }
                    }
                }

                // Record stream.
                if let Some(ref rec) = recorder {
                    if let Err(e) = rec.write_packet(&stream_clone.stream_key, &packet).await {
                        error!("Recording error: {}", e);
                    }
                }

                // Queue the packet for CDN upload. A queueing failure is
                // logged here and counted by the uploader itself (metric
                // `cdn_upload_failed_total`, along with the per-upload
                // outcome recorded by its worker), so a CDN outage degrades
                // to "no CDN" instead of tearing down the ingest loop: the
                // stream keeps being received, recorded and transcoded.
                if let Some(ref uploader) = cdn {
                    if let Err(e) = uploader.upload_packet(&stream_clone.stream_key, &packet) {
                        error!(
                            stream = %stream_clone.stream_key,
                            "CDN upload could not be queued: {e}"
                        );
                    }
                }
            }

            info!("Stream ended: {}", key);
        });

        stream
    }

    /// Registers a new stream directly (used by tests and integrations that
    /// drive the media lifecycle themselves rather than via the live bridge).
    pub fn register_stream(
        &self,
        app_name: impl Into<String>,
        stream_key: impl Into<String>,
        metadata: StreamMetadata,
    ) -> Arc<IngestStream> {
        Self::spawn_ingest_stream(
            &self.streams,
            &self.metrics,
            &self.transcode_engine,
            &self.recorder,
            &self.cdn_uploader,
            app_name.into(),
            stream_key.into(),
            metadata,
        )
    }

    /// Unregisters a stream.
    pub fn unregister_stream(&self, app_name: &str, stream_key: &str) {
        let key = format!("{}/{}", app_name, stream_key);
        let mut streams = self.streams.write();
        streams.remove(&key);
        info!("Unregistered stream: {}", key);
    }

    /// Gets an active stream.
    #[must_use]
    pub fn get_stream(&self, app_name: &str, stream_key: &str) -> Option<Arc<IngestStream>> {
        let key = format!("{}/{}", app_name, stream_key);
        let streams = self.streams.read();
        streams.get(&key).cloned()
    }

    /// Lists all active streams.
    #[must_use]
    pub fn list_streams(&self) -> Vec<Arc<IngestStream>> {
        let streams = self.streams.read();
        streams.values().cloned().collect()
    }

    /// Gets the stream key validator.
    #[must_use]
    pub fn validator(&self) -> &Arc<StreamKeyValidator> {
        &self.validator
    }

    /// Shuts down the server.
    pub async fn shutdown(&self) -> ServerResult<()> {
        info!("Shutting down RTMP ingest server");

        // Stop all streams
        let streams = {
            let s = self.streams.write();
            s.keys().cloned().collect::<Vec<_>>()
        };

        for key in streams {
            let parts: Vec<&str> = key.split('/').collect();
            if parts.len() == 2 {
                self.unregister_stream(parts[0], parts[1]);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod bridge_tests {
    use super::*;
    use bytes::Bytes;
    use oximedia_net::rtmp::MediaPacketType;

    fn packet(packet_type: MediaPacketType, body: &'static [u8]) -> NetMediaPacket {
        NetMediaPacket {
            packet_type,
            timestamp: 0,
            stream_id: 1,
            data: Bytes::from_static(body),
        }
    }

    /// The ingest bridge subscribes to a stream that is already publishing, so
    /// the publisher's codec sequence headers were broadcast before the
    /// subscription existed. They must be replayed from the registry cache,
    /// otherwise the packaging layer only ever sees coded frames it has no
    /// decoder configuration for.
    #[tokio::test]
    async fn bridge_replays_cached_sequence_headers() {
        let registry = Arc::new(StreamRegistry::new());
        registry
            .register_stream(
                "live/cam".to_string(),
                StreamMetadata::new("cam", "live"),
                1,
            )
            .await
            .expect("register stream");

        // Publisher's sequence headers, sent before the bridge subscribes.
        let cache = registry
            .seq_headers("live/cam")
            .await
            .expect("sequence-header cache");
        {
            let mut guard = cache.write().await;
            guard.capture(&packet(
                MediaPacketType::Video,
                &[0x18, b'a', b'v', b'0', b'1', 0x81, 0x00, 0x00, 0x00],
            ));
            guard.capture(&packet(
                MediaPacketType::Audio,
                &[0x9F, 0x00, b'O', b'p', b'u', b's', 0x01],
            ));
        }

        let streams: Arc<RwLock<HashMap<String, Arc<IngestStream>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let metrics = Arc::new(MetricsCollector::new());
        let (publish_tx, publish_rx) = mpsc::unbounded_channel();
        publish_tx
            .send(PublishEvent {
                app: "live".to_string(),
                stream_key: "cam".to_string(),
            })
            .expect("queue publish event");
        drop(publish_tx);

        tokio::spawn(RtmpIngestServer::run_bridge(
            Arc::clone(&registry),
            Arc::clone(&streams),
            Arc::clone(&metrics),
            None,
            None,
            None,
            publish_rx,
        ));

        // The bridge registers the ingest stream, then replays both cached
        // headers into it — with no live packet ever published.
        let mut received = 0u64;
        for _ in 0..200 {
            if let Some(stream) = streams.read().get("live/cam").map(Arc::clone) {
                received = *stream.packets_received.read();
                if received >= 2 {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(
            received, 2,
            "the bridge must replay both cached sequence headers to a late subscriber"
        );
    }

    /// With nothing cached, the bridge must not invent any packet.
    #[tokio::test]
    async fn bridge_replays_nothing_when_cache_is_empty() {
        let registry = Arc::new(StreamRegistry::new());
        registry
            .register_stream(
                "live/quiet".to_string(),
                StreamMetadata::new("quiet", "live"),
                1,
            )
            .await
            .expect("register stream");

        let streams: Arc<RwLock<HashMap<String, Arc<IngestStream>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let metrics = Arc::new(MetricsCollector::new());
        let (publish_tx, publish_rx) = mpsc::unbounded_channel();
        publish_tx
            .send(PublishEvent {
                app: "live".to_string(),
                stream_key: "quiet".to_string(),
            })
            .expect("queue publish event");
        drop(publish_tx);

        tokio::spawn(RtmpIngestServer::run_bridge(
            Arc::clone(&registry),
            Arc::clone(&streams),
            Arc::clone(&metrics),
            None,
            None,
            None,
            publish_rx,
        ));

        // Wait for the bridge to register the stream, then confirm it is idle.
        let mut stream = None;
        for _ in 0..200 {
            if let Some(s) = streams.read().get("live/quiet").map(Arc::clone) {
                stream = Some(s);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let stream = stream.expect("the bridge must register the ingest stream");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            *stream.packets_received.read(),
            0,
            "no packet may be fabricated when nothing was cached"
        );
    }
}
