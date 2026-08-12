//! DASH packager for creating DASH streams.

use crate::dash::{DashSegmentWriter, MpdGenerator};
use crate::error::ServerResult;
use oximedia_net::rtmp::MediaPacket;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// DASH configuration.
#[derive(Debug, Clone)]
pub struct DashConfig {
    /// Output directory.
    pub output_dir: PathBuf,

    /// Segment duration.
    pub segment_duration: Duration,

    /// Minimum buffer time.
    pub min_buffer_time: Duration,

    /// Time shift buffer depth (for live DVR).
    pub time_shift_buffer_depth: Option<Duration>,

    /// Enable low-latency DASH.
    pub low_latency: bool,

    /// Suggested presentation delay.
    pub suggested_presentation_delay: Duration,
}

impl Default for DashConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./dash"),
            segment_duration: Duration::from_secs(2),
            min_buffer_time: Duration::from_secs(2),
            time_shift_buffer_depth: Some(Duration::from_secs(60)),
            low_latency: false,
            suggested_presentation_delay: Duration::from_secs(6),
        }
    }
}

/// DASH stream packager.
#[allow(dead_code)]
struct StreamPackager {
    /// Stream key.
    stream_key: String,

    /// Configuration.
    config: DashConfig,

    /// MPD generator.
    mpd_gen: MpdGenerator,

    /// Segment writer.
    segment_writer: DashSegmentWriter,

    /// Current segment index.
    segment_index: RwLock<u64>,

    /// Packets buffered for current segment.
    packet_buffer: RwLock<Vec<MediaPacket>>,

    /// Whether a real `ftyp`+`moov` initialization segment has been written.
    init_written: RwLock<bool>,

    /// Set once we have logged that segment muxing failed for this stream, so
    /// we warn a single time instead of once per segment. Reserved for genuine
    /// failures (unsupported codec, muxer error) — "no sequence header yet" is
    /// transient and logged at debug level instead.
    mux_unsupported_logged: AtomicBool,
}

impl StreamPackager {
    /// Creates a new stream packager.
    fn new(stream_key: String, config: DashConfig) -> ServerResult<Self> {
        let mpd_gen = MpdGenerator::new(config.clone());
        let segment_writer = DashSegmentWriter::new(&config.output_dir)?;

        Ok(Self {
            stream_key,
            config,
            mpd_gen,
            segment_writer,
            segment_index: RwLock::new(1),
            packet_buffer: RwLock::new(Vec::new()),
            init_written: RwLock::new(false),
            mux_unsupported_logged: AtomicBool::new(false),
        })
    }

    /// Processes a media packet.
    async fn process_packet(&self, packet: MediaPacket) -> ServerResult<()> {
        // Learn the codec configuration as packets arrive. Sequence headers
        // carry no elementary samples, so the same packets are still muxed in
        // full when the segment is finalized.
        if let Err(e) = self.segment_writer.observe_packet(&packet) {
            self.warn_mux_unsupported_once(&e);
        }

        let should_finalize = {
            let mut buffer = self.packet_buffer.write();
            buffer.push(packet);
            // Check if we should finalize the segment
            buffer.len() >= 100
        };

        if should_finalize {
            // Retry the initialization segment until it genuinely succeeds. The
            // first packets of a publish routinely arrive before the codec
            // sequence headers, and marking the attempt "done" on that
            // transient failure would leave the stream permanently without an
            // init segment.
            //
            // This runs immediately before the first fragment rather than on
            // every packet, and deliberately so: a CMAF track set is frozen by
            // its initialization segment. Writing init after the very first
            // packet would freeze it around whichever stream's sequence header
            // arrived first (video, in practice), and the audio track would
            // never exist — its samples would be dropped by the muxer with no
            // error. Waiting until the segment boundary means both sequence
            // headers are long past.
            if !*self.init_written.read() && self.write_init_segment().await? {
                *self.init_written.write() = true;
            }
            self.finalize_segment().await?;
        }

        Ok(())
    }

    /// Logs the "segment muxing failed" warning at most once per stream.
    fn warn_mux_unsupported_once(&self, err: &crate::error::ServerError) {
        if !self.mux_unsupported_logged.swap(true, Ordering::Relaxed) {
            warn!(
                "DASH segment muxing failed for stream '{}': {}; \
                 dropping segments (no fabricated output produced)",
                self.stream_key, err
            );
        }
    }

    /// Attempts to write the fMP4 initialization segment.
    ///
    /// Returns `Ok(true)` only when a real `ftyp`+`moov` segment was written.
    /// Two failure modes are distinguished so a transient one is never
    /// mistaken for a permanent one:
    ///
    /// * **not ready** — no Enhanced-RTMP sequence header has arrived yet.
    ///   Expected at the start of every publish; logged at debug level and
    ///   retried on the next packet.
    /// * **muxing failure** — an unsupported codec or a real error. Warned
    ///   about once per stream; nothing is written.
    async fn write_init_segment(&self) -> ServerResult<bool> {
        if !self.segment_writer.is_ready() {
            debug!(
                "DASH initialization segment for stream '{}' is not ready yet: \
                 awaiting the Enhanced-RTMP sequence header",
                self.stream_key
            );
            return Ok(false);
        }

        let init_name = "init.mp4";
        match self.segment_writer.write_init_segment(init_name).await {
            Ok(()) => {
                info!("Wrote DASH initialization segment: {}", init_name);
                Ok(true)
            }
            Err(e) => {
                self.warn_mux_unsupported_once(&e);
                Ok(false)
            }
        }
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
            (idx, format!("segment{}.m4s", idx))
        };

        // Write a real `moof`+`mdat` fragment via the CMAF muxer. When the
        // ingest cannot be depacketized (Red List codec, no sequence header
        // yet) this fails honestly: we drop the segment rather than writing a
        // fabricated file or advertising a segment that does not exist in the
        // MPD.
        match self
            .segment_writer
            .write_media_segment(index_val, &segment_name, &packets)
            .await
        {
            Ok(measured_duration) => {
                // Only advertise the segment once it was genuinely produced,
                // and advertise the duration the samples actually span rather
                // than the configured target — an MPD duration that disagrees
                // with the fragment drifts a player's timeline. A segment too
                // short to measure falls back to the configured target.
                let duration = if measured_duration > 0.0 {
                    measured_duration
                } else {
                    self.config.segment_duration.as_secs_f64()
                };
                self.mpd_gen.add_segment(index_val, duration)?;
                info!(
                    "Finalized DASH segment: {} ({:.3}s)",
                    segment_name, duration
                );
            }
            Err(e) => {
                self.warn_mux_unsupported_once(&e);
            }
        }

        Ok(())
    }
}

/// DASH packager.
pub struct DashPackager {
    /// Configuration.
    config: DashConfig,

    /// Active stream packagers.
    packagers: Arc<RwLock<HashMap<String, Arc<StreamPackager>>>>,
}

impl DashPackager {
    /// Creates a new DASH packager.
    #[must_use]
    pub fn new(config: DashConfig) -> Self {
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

        info!("Started DASH packaging for stream: {}", stream_key);

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

        info!("Stopped DASH packaging for stream: {}", stream_key);

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

    /// Gets the MPD path for a stream.
    #[must_use]
    pub fn get_mpd_path(&self, stream_key: &str) -> PathBuf {
        self.config.output_dir.join(stream_key).join("manifest.mpd")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use oximedia_net::rtmp::{EnhancedAudioTag, EnhancedVideoTag, FourCC, MediaPacketType};

    fn tmp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia-dash-pkg-{name}-{}", std::process::id()))
    }

    fn video_packet(body: Bytes, timestamp: u32) -> MediaPacket {
        MediaPacket {
            packet_type: MediaPacketType::Video,
            timestamp,
            stream_id: 1,
            data: body,
        }
    }

    fn audio_packet(body: Bytes, timestamp: u32) -> MediaPacket {
        MediaPacket {
            packet_type: MediaPacketType::Audio,
            timestamp,
            stream_id: 1,
            data: body,
        }
    }

    fn opus_head() -> Bytes {
        let mut v = b"OpusHead".to_vec();
        v.push(1);
        v.push(2);
        v.extend_from_slice(&312u16.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&0i16.to_le_bytes());
        v.push(0);
        Bytes::from(v)
    }

    /// Counts occurrences of an ISO-BMFF box type anywhere in the buffer.
    fn count_boxes(data: &[u8], want: &[u8; 4]) -> usize {
        let mut count = 0;
        for start in 4..data.len().saturating_sub(4) {
            if &data[start..start + 4] != want {
                continue;
            }
            let header = start - 4;
            let size = u32::from_be_bytes([
                data[header],
                data[header + 1],
                data[header + 2],
                data[header + 3],
            ]) as usize;
            if size >= 8 && header + size <= data.len() {
                count += 1;
            }
        }
        count
    }

    /// REGRESSION: an A/V publish must yield an init segment with both tracks.
    ///
    /// A CMAF track set is frozen by its initialization segment. Attempting
    /// init on the very first packet froze it around the video-only
    /// configuration — real publishers send the video sequence header first —
    /// and every audio sample was then dropped by the muxer with no error and
    /// no log. The init attempt therefore belongs at the first segment
    /// boundary, by which point both sequence headers have arrived.
    #[tokio::test]
    async fn av_publish_init_segment_carries_both_tracks() {
        let dir = tmp_dir("av-init");
        let _ = std::fs::remove_dir_all(&dir);
        let packager = DashPackager::new(DashConfig {
            output_dir: dir.clone(),
            ..DashConfig::default()
        });
        packager.start_stream("live/cam").expect("start stream");

        // Video sequence header first, then audio — the real publisher order.
        packager
            .process_packet(
                "live/cam",
                video_packet(
                    EnhancedVideoTag::sequence_start(
                        FourCC::AV1,
                        Bytes::from_static(&[0x81, 0x00, 0x00, 0x00]),
                    )
                    .encode(),
                    0,
                ),
            )
            .await
            .expect("video sequence header");
        packager
            .process_packet(
                "live/cam",
                audio_packet(
                    EnhancedAudioTag::sequence_start(FourCC::OPUS, opus_head()).encode(),
                    0,
                ),
            )
            .await
            .expect("audio sequence header");

        // Enough coded frames to cross the segment boundary (buffer >= 100).
        for i in 0..98u32 {
            let packet = if i % 2 == 0 {
                video_packet(
                    EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0xDE, 0xAD])).encode(),
                    i * 20,
                )
            } else {
                audio_packet(
                    EnhancedAudioTag::opus(Bytes::from_static(&[0xFC, 0x01])).encode(),
                    i * 20,
                )
            };
            packager
                .process_packet("live/cam", packet)
                .await
                .expect("coded frame");
        }

        let init = std::fs::read(dir.join("init.mp4"))
            .expect("a real initialization segment must have been written");
        assert_eq!(
            count_boxes(&init, b"trak"),
            2,
            "the init segment must declare both the video and the audio track"
        );

        let fragment = std::fs::read(dir.join("segment1.m4s"))
            .expect("a real media fragment must have been written");
        assert_eq!(
            count_boxes(&fragment, b"traf"),
            2,
            "the fragment must carry one traf per track — audio must not vanish"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// NO-FABRICATION: with no sequence header at all, nothing is written.
    #[tokio::test]
    async fn publish_without_sequence_headers_writes_nothing() {
        let dir = tmp_dir("no-config");
        let _ = std::fs::remove_dir_all(&dir);
        let packager = DashPackager::new(DashConfig {
            output_dir: dir.clone(),
            ..DashConfig::default()
        });
        packager.start_stream("live/bare").expect("start stream");

        for i in 0..120u32 {
            packager
                .process_packet(
                    "live/bare",
                    video_packet(
                        EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x01])).encode(),
                        i * 20,
                    ),
                )
                .await
                .expect("ingest must not abort");
        }

        assert!(
            !dir.join("init.mp4").exists(),
            "no placeholder init segment may be written without a sequence header"
        );
        assert!(
            !dir.join("segment1.m4s").exists(),
            "no fabricated fragment may be written without a sequence header"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
