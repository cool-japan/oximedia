//! HLS packager for creating HLS streams.

use crate::error::ServerResult;
use crate::hls::{PlaylistGenerator, SegmentWriter};
use oximedia_net::rtmp::MediaPacket;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// HLS configuration.
#[derive(Debug, Clone)]
pub struct HlsConfig {
    /// Output directory.
    pub output_dir: PathBuf,

    /// Segment duration.
    pub segment_duration: Duration,

    /// Playlist length (number of segments).
    pub playlist_length: usize,

    /// Enable low-latency HLS.
    pub low_latency: bool,

    /// Part duration for LL-HLS.
    pub part_duration: Duration,

    /// DVR window duration.
    pub dvr_window: Option<Duration>,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./hls"),
            segment_duration: Duration::from_secs(2),
            playlist_length: 6,
            low_latency: false,
            part_duration: Duration::from_millis(500),
            dvr_window: None,
        }
    }
}

/// HLS stream packager.
#[allow(dead_code)]
struct StreamPackager {
    /// Stream key.
    stream_key: String,

    /// Configuration.
    config: HlsConfig,

    /// Playlist generator.
    playlist_gen: PlaylistGenerator,

    /// Segment writer.
    segment_writer: SegmentWriter,

    /// Current segment index.
    segment_index: RwLock<u64>,

    /// Packets buffered for current segment.
    packet_buffer: RwLock<Vec<MediaPacket>>,

    /// Set once we have logged that real segment muxing is unavailable, so we
    /// warn a single time per stream instead of once per segment.
    mux_unsupported_logged: AtomicBool,
}

impl StreamPackager {
    /// Creates a new stream packager.
    fn new(stream_key: String, config: HlsConfig) -> ServerResult<Self> {
        let playlist_gen = PlaylistGenerator::new(config.clone());
        let segment_writer = SegmentWriter::new(&config.output_dir)?;

        Ok(Self {
            stream_key,
            config,
            playlist_gen,
            segment_writer,
            segment_index: RwLock::new(0),
            packet_buffer: RwLock::new(Vec::new()),
            mux_unsupported_logged: AtomicBool::new(false),
        })
    }

    /// Processes a media packet.
    async fn process_packet(&self, packet: MediaPacket) -> ServerResult<()> {
        // Learn the codec configuration as packets arrive, so a segment whose
        // buffer happens to open with a coded frame still muxes rather than
        // being dropped whole. Sequence headers carry no elementary samples,
        // so the same packets are still muxed in full at finalize time.
        if let Err(e) = self.segment_writer.observe_packet(&packet) {
            if !self.mux_unsupported_logged.swap(true, Ordering::Relaxed) {
                warn!(
                    "HLS ingest for stream '{}' is not muxable: {}; \
                     dropping segments (no fabricated output produced)",
                    self.stream_key, e
                );
            }
        }

        let should_finalize = {
            let mut buffer = self.packet_buffer.write();
            buffer.push(packet);
            // Check if we should finalize the segment
            // (simplified logic - in real implementation, check timestamp and keyframes)
            buffer.len() >= 100
        };

        if should_finalize {
            self.finalize_segment().await?;
        }

        Ok(())
    }

    /// Finalizes the current segment.
    async fn finalize_segment(&self) -> ServerResult<()> {
        let packets = {
            let mut buffer = self.packet_buffer.write();
            std::mem::take(&mut *buffer)
        };

        if packets.is_empty() {
            return Ok(());
        }

        let (index_val, segment_name) = {
            let mut index = self.segment_index.write();
            let idx = *index;
            *index += 1;
            (idx, format!("segment{}.ts", idx))
        };

        // Write a real MPEG-TS segment via the transport-stream muxer. When
        // the ingest cannot be depacketized (Red List codec, no Enhanced-RTMP
        // sequence header yet) this fails honestly. We degrade by dropping the
        // segment instead of writing a fabricated file or advertising a segment
        // that does not exist in the playlist. Warn once per stream to avoid
        // flooding the log every segment.
        match self
            .segment_writer
            .write_segment(&segment_name, &packets)
            .await
        {
            Ok(measured_duration) => {
                // Only advertise the segment once it was genuinely produced,
                // and advertise the duration its samples actually span: an
                // EXTINF that disagrees with the segment drifts a player's
                // timeline. A segment too short to measure (a single sample has
                // no measurable inter-sample delta) falls back to the
                // configured target.
                let duration = if measured_duration > 0.0 {
                    measured_duration
                } else {
                    self.config.segment_duration.as_secs_f64()
                };
                self.playlist_gen.add_segment(&segment_name, duration)?;
                info!("Finalized HLS segment: {} ({:.3}s)", segment_name, duration);
            }
            Err(e) => {
                if !self.mux_unsupported_logged.swap(true, Ordering::Relaxed) {
                    warn!(
                        "HLS segment muxing unavailable for stream '{}': {}; \
                         dropping segments (no fabricated output produced)",
                        self.stream_key, e
                    );
                }
            }
        }

        let _ = index_val; // reserved above; kept monotonic even when dropped

        Ok(())
    }
}

/// HLS packager.
pub struct HlsPackager {
    /// Configuration.
    config: HlsConfig,

    /// Active stream packagers.
    packagers: Arc<RwLock<HashMap<String, Arc<StreamPackager>>>>,
}

impl HlsPackager {
    /// Creates a new HLS packager.
    pub fn new(config: HlsConfig) -> Self {
        Self {
            config,
            packagers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Starts packaging for a stream.
    pub fn start_stream(&self, stream_key: impl Into<String>) -> ServerResult<()> {
        let stream_key = stream_key.into();

        let packager = Arc::new(StreamPackager::new(
            stream_key.clone(),
            self.config.clone(),
        )?);

        let mut packagers = self.packagers.write();
        packagers.insert(stream_key.clone(), packager);

        info!("Started HLS packaging for stream: {}", stream_key);

        Ok(())
    }

    /// Stops packaging for a stream.
    pub async fn stop_stream(&self, stream_key: &str) -> ServerResult<()> {
        let packager = {
            let mut packagers = self.packagers.write();
            packagers.remove(stream_key)
        };

        if let Some(packager) = packager {
            // Finalize any remaining segment
            packager.finalize_segment().await?;
        }

        info!("Stopped HLS packaging for stream: {}", stream_key);

        Ok(())
    }

    /// Processes a media packet.
    pub async fn process_packet(&self, stream_key: &str, packet: MediaPacket) -> ServerResult<()> {
        let packager = {
            let packagers = self.packagers.read();
            packagers.get(stream_key).map(Arc::clone)
        };

        if let Some(packager) = packager {
            packager.process_packet(packet).await?;
        }

        Ok(())
    }

    /// Gets the playlist path for a stream.
    #[must_use]
    pub fn get_playlist_path(&self, stream_key: &str) -> PathBuf {
        self.config
            .output_dir
            .join(stream_key)
            .join("playlist.m3u8")
    }
}
