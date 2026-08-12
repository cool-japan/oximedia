//! HLS segment writer for MPEG-TS segments.
//!
//! Segments are produced by a **real** [`MpegTsMuxer`]: RTMP ingest packets are
//! depacketized by [`Depacketizer`] into elementary samples plus a decoder
//! configuration record, the resulting streams are declared to the muxer, and
//! the muxer emits PAT/PMT plus PES-packetized transport packets.
//!
//! Only the patent-free codecs OxiMedia supports on ingest reach this point —
//! AV1 and VP9 video, Opus and FLAC audio. Legacy FLV H.264/AAC ingest is
//! refused by the depacketizer, naming the codec and the Red List, so no
//! fabricated `.ts` file is ever written.

use crate::error::{ServerError, ServerResult};
use crate::ingest_depacketizer::{
    measured_span_seconds, Depacketizer, ElementarySample, SampleKind, StreamConfig,
};
use async_trait::async_trait;
use oximedia_container::mux::{MpegTsMuxer, Muxer, MuxerConfig};
use oximedia_container::{CodecParams, Packet, PacketFlags, StreamInfo};
use oximedia_core::{OxiError, OxiResult, Rational, Timestamp};
use oximedia_io::MediaSource;
use oximedia_net::rtmp::MediaPacket;
use parking_lot::Mutex;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;

/// MPEG-TS timebase: the 90 kHz MPEG system clock.
const TS_TIMEBASE: Rational = Rational {
    num: 1,
    den: 90_000,
};

/// Transport-stream index assigned to the video elementary stream.
const VIDEO_STREAM_INDEX: usize = 0;

/// Transport-stream index assigned to the audio elementary stream.
const AUDIO_STREAM_INDEX: usize = 1;

/// Converts a millisecond RTMP timestamp into 90 kHz MPEG ticks.
const fn ms_to_90khz(ms: i64) -> i64 {
    ms * 90
}

// ─── In-memory muxer sink ─────────────────────────────────────────────────────

/// Write-only [`MediaSource`] that accumulates everything a muxer writes.
///
/// [`MpegTsMuxer`] takes its sink **by value** and exposes no accessor to get
/// it back, so the emitted bytes are captured through a shared buffer handle
/// retained by the caller before the sink is handed over.
#[derive(Clone, Debug, Default)]
struct SegmentSink {
    /// Shared buffer holding everything written so far.
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl SegmentSink {
    /// Returns a handle onto the shared buffer.
    fn handle(&self) -> Arc<Mutex<Vec<u8>>> {
        Arc::clone(&self.buffer)
    }
}

#[async_trait]
impl MediaSource for SegmentSink {
    async fn read(&mut self, _buf: &mut [u8]) -> OxiResult<usize> {
        Err(OxiError::unsupported(
            "HLS segment sink is write-only and cannot be read back",
        ))
    }

    async fn write_all(&mut self, buf: &[u8]) -> OxiResult<()> {
        self.buffer.lock().extend_from_slice(buf);
        Ok(())
    }

    async fn seek(&mut self, _pos: SeekFrom) -> OxiResult<u64> {
        Err(OxiError::unsupported(
            "HLS segment sink is an append-only stream and cannot seek",
        ))
    }

    fn len(&self) -> Option<u64> {
        Some(self.buffer.lock().len() as u64)
    }

    fn is_seekable(&self) -> bool {
        false
    }

    fn position(&self) -> u64 {
        self.buffer.lock().len() as u64
    }

    fn is_writable(&self) -> bool {
        true
    }
}

// ─── Public types ─────────────────────────────────────────────────────────────

/// MPEG-TS segment.
#[derive(Debug, Clone)]
pub struct TsSegment {
    /// Segment filename.
    pub filename: String,

    /// Segment duration.
    pub duration: f64,

    /// Segment data.
    pub data: Vec<u8>,
}

/// Segment writer.
///
/// Holds the per-stream [`Depacketizer`] so codec configuration captured from
/// one segment's sequence headers remains available for every later segment.
pub struct SegmentWriter {
    /// Output directory.
    output_dir: PathBuf,

    /// Ingest depacketizer, shared across segments.
    depacketizer: Mutex<Depacketizer>,
}

impl SegmentWriter {
    /// Creates a new segment writer.
    ///
    /// # Errors
    ///
    /// Returns an error if directory creation fails.
    pub fn new(output_dir: impl AsRef<Path>) -> ServerResult<Self> {
        let output_dir = output_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&output_dir)?;

        Ok(Self {
            output_dir,
            depacketizer: Mutex::new(Depacketizer::new()),
        })
    }

    /// Returns `true` once a codec configuration has been captured from ingest.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.depacketizer.lock().ready()
    }

    /// Absorbs a packet's codec configuration without consuming coded frames.
    ///
    /// Lets a caller that buffers packets for later muxing learn the stream
    /// configuration as packets arrive, so a segment whose buffer happens to
    /// begin with a coded frame still muxes instead of being dropped whole.
    /// Sequence headers carry no elementary samples, so replaying the same
    /// packets through [`Self::write_segment`] afterwards is unaffected.
    ///
    /// # Errors
    ///
    /// Returns an honest error for Red List or non-patent-free sequence
    /// headers (see [`Depacketizer::observe_config`]).
    pub fn observe_packet(&self, packet: &MediaPacket) -> ServerResult<()> {
        self.depacketizer.lock().observe_config(packet)
    }

    /// Writes an MPEG-TS segment for the given packets.
    ///
    /// Returns the segment's **measured** duration in seconds, so the playlist
    /// advertises an `EXTINF` matching what the segment actually contains
    /// rather than the packager's nominal target.
    ///
    /// # Errors
    ///
    /// Returns an error when the ingest stream cannot be depacketized — an
    /// unsupported (Red List) codec, a malformed tag body, or coded frames
    /// arriving before their sequence header — and when no elementary sample
    /// could be produced. Nothing is written to disk on any error path.
    pub async fn write_segment(
        &self,
        filename: &str,
        packets: &[MediaPacket],
    ) -> ServerResult<f64> {
        let segment = self.create_segment(filename, packets).await?;
        let path = self.output_dir.join(filename);
        fs::write(&path, &segment.data).await?;
        Ok(segment.duration)
    }

    /// Creates an in-memory MPEG-TS segment from packets.
    ///
    /// # Errors
    ///
    /// See [`Self::write_segment`].
    pub async fn create_segment(
        &self,
        filename: &str,
        packets: &[MediaPacket],
    ) -> ServerResult<TsSegment> {
        // Depacketize under the lock, then release it before any `.await`.
        let (samples, video, audio) = {
            let mut depack = self.depacketizer.lock();
            let mut samples = Vec::with_capacity(packets.len());
            for packet in packets {
                if let Some(sample) = depack.push(packet)? {
                    samples.push(sample);
                }
            }
            (
                samples,
                depack.video_config().cloned(),
                depack.audio_config().cloned(),
            )
        };

        if video.is_none() && audio.is_none() {
            return Err(ServerError::BadRequest(
                "cannot mux an MPEG-TS segment: no Enhanced-RTMP sequence header has been \
                 received yet, so no decoder configuration record is available"
                    .to_string(),
            ));
        }
        if samples.is_empty() {
            return Err(ServerError::BadRequest(format!(
                "cannot mux MPEG-TS segment '{filename}': the {} ingest packets carried no \
                 elementary samples (sequence headers only)",
                packets.len()
            )));
        }

        let data = mux_mpegts(&samples, video.as_ref(), audio.as_ref()).await?;
        let duration = measured_span_seconds(&samples);

        Ok(TsSegment {
            filename: filename.to_string(),
            duration,
            data,
        })
    }

    /// Deletes old segments.
    pub async fn cleanup_old_segments(&self, keep_count: usize) -> ServerResult<()> {
        let mut entries = fs::read_dir(&self.output_dir).await?;
        let mut files = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().and_then(|s| s.to_str()) == Some("ts") {
                files.push(entry.path());
            }
        }

        // Sort by modification time
        files.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());

        // Delete oldest files if we have too many
        if files.len() > keep_count {
            for file in &files[..files.len() - keep_count] {
                fs::remove_file(file).await?;
            }
        }

        Ok(())
    }
}

// ─── Muxing ───────────────────────────────────────────────────────────────────

/// Builds the [`StreamInfo`] for one depacketized elementary stream.
fn stream_info(index: usize, config: &StreamConfig) -> StreamInfo {
    let mut info = StreamInfo::new(index, config.codec.codec_id(), TS_TIMEBASE);
    info.codec_params = if config.codec.is_video() {
        CodecParams {
            width: config.width,
            height: config.height,
            ..CodecParams::default()
        }
    } else {
        CodecParams {
            sample_rate: config.sample_rate,
            channels: config.channels,
            ..CodecParams::default()
        }
    };
    info
}

/// Muxes elementary samples into a complete MPEG-TS segment.
async fn mux_mpegts(
    samples: &[ElementarySample],
    video: Option<&StreamConfig>,
    audio: Option<&StreamConfig>,
) -> ServerResult<Vec<u8>> {
    let sink = SegmentSink::default();
    let buffer = sink.handle();
    let mut muxer = MpegTsMuxer::new(sink, MuxerConfig::new());

    if let Some(config) = video {
        muxer.add_stream(stream_info(VIDEO_STREAM_INDEX, config))?;
    }
    if let Some(config) = audio {
        muxer.add_stream(stream_info(AUDIO_STREAM_INDEX, config))?;
    }

    muxer.write_header().await?;

    for sample in samples {
        let stream_index = match sample.kind {
            SampleKind::Video => VIDEO_STREAM_INDEX,
            SampleKind::Audio => AUDIO_STREAM_INDEX,
        };
        // Skip samples whose stream was never configured rather than handing
        // the muxer an index it does not know.
        let configured = match sample.kind {
            SampleKind::Video => video.is_some(),
            SampleKind::Audio => audio.is_some(),
        };
        if !configured {
            continue;
        }

        let dts = ms_to_90khz(i64::from(sample.dts_ms));
        let pts = dts + ms_to_90khz(i64::from(sample.composition_time_ms));
        let flags = if sample.keyframe {
            PacketFlags::KEYFRAME
        } else {
            PacketFlags::empty()
        };
        let packet = Packet::new(
            stream_index,
            sample.data.clone(),
            Timestamp::with_dts(pts, Some(dts), TS_TIMEBASE, None),
            flags,
        );
        muxer.write_packet(&packet).await?;
    }

    muxer.write_trailer().await?;

    let data = std::mem::take(&mut *buffer.lock());
    if data.is_empty() {
        return Err(ServerError::Internal(
            "MPEG-TS muxer produced no output for a non-empty sample set".to_string(),
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use oximedia_net::rtmp::{
        EnhancedAudioTag, EnhancedVideoTag, FourCC, MediaPacket, MediaPacketType,
    };

    fn tmp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia-hls-seg-{name}-{}", std::process::id()))
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

    /// Minimal valid `AV1CodecConfigurationRecord`.
    fn av1_config() -> Bytes {
        Bytes::from_static(&[0x81, 0x00, 0x00, 0x00])
    }

    /// A canonical stereo 48 kHz `OpusHead`.
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

    /// A synthetic AV1 Enhanced-RTMP publish: sequence header + two keyframes.
    fn av1_ingest() -> Vec<MediaPacket> {
        vec![
            video_packet(
                EnhancedVideoTag::sequence_start(FourCC::AV1, av1_config()).encode(),
                0,
            ),
            video_packet(
                EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0xDE, 0xAD, 0xBE, 0xEF]))
                    .encode(),
                0,
            ),
            video_packet(
                EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0xCA, 0xFE, 0xBA, 0xBE]))
                    .encode(),
                40,
            ),
        ]
    }

    /// A legacy H.264 publish (Red List) used by the no-fabrication tests.
    fn h264_ingest() -> Vec<MediaPacket> {
        vec![video_packet(
            Bytes::from_static(&[0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x42]),
            0,
        )]
    }

    // 1. A real AV1 segment is produced and starts with the TS sync byte.
    #[tokio::test]
    async fn write_segment_produces_real_mpegts() {
        let dir = tmp_dir("write-real");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        writer
            .write_segment("segment0.ts", &av1_ingest())
            .await
            .expect("real MPEG-TS segment must be produced");

        let path = dir.join("segment0.ts");
        let data = std::fs::read(&path).expect("segment written");
        assert!(!data.is_empty());
        assert_eq!(
            data.len() % 188,
            0,
            "MPEG-TS is a stream of 188-byte packets"
        );
        assert_eq!(data[0], 0x47, "first byte must be the TS sync byte");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 2. The segment re-parses: PAT/PMT are present and the stream type is AV1.
    #[tokio::test]
    async fn segment_reparses_with_correct_stream_type() {
        use oximedia_container::demux::Demuxer;
        use oximedia_container::demux::MpegTsDemuxer;
        use oximedia_core::CodecId;
        use oximedia_io::MemorySource;

        let dir = tmp_dir("reparse");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        let segment = writer
            .create_segment("segment0.ts", &av1_ingest())
            .await
            .expect("segment");

        let mut demuxer = MpegTsDemuxer::new(MemorySource::from_vec(segment.data.clone()));
        // `probe` only succeeds once PAT *and* PMT have been parsed and an
        // elementary stream registered — so this is the PAT/PMT assertion.
        demuxer.probe().await.expect("PAT/PMT must parse");
        let streams = demuxer.streams();
        assert_eq!(streams.len(), 1, "one elementary stream (video only)");
        assert_eq!(
            streams[0].codec,
            CodecId::Av1,
            "PMT stream_type 0x85 must map back to AV1"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3. The elementary payload survives the PES round trip.
    #[tokio::test]
    async fn segment_payload_round_trips() {
        use oximedia_container::demux::Demuxer;
        use oximedia_container::demux::MpegTsDemuxer;
        use oximedia_io::MemorySource;

        let dir = tmp_dir("roundtrip");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        let segment = writer
            .create_segment("segment0.ts", &av1_ingest())
            .await
            .expect("segment");

        let mut demuxer = MpegTsDemuxer::new(MemorySource::from_vec(segment.data.clone()));
        demuxer.probe().await.expect("probe");

        let mut payloads: Vec<Vec<u8>> = Vec::new();
        while let Ok(packet) = demuxer.read_packet().await {
            payloads.push(packet.data.to_vec());
        }

        assert!(
            !payloads.is_empty(),
            "the demuxer must recover at least one elementary payload"
        );
        // Prefix, not equality, and deliberately so: `oximedia_container`'s
        // MPEG-TS demuxer does not trim the reassembled PES payload down to
        // `PES_packet_length`, so it hands back the muxer's transport-packet
        // stuffing (`0xFF`) as trailing bytes. The elementary data itself is
        // byte-exact and correctly positioned; only trailing padding is extra.
        // Tightening this to `p == &[0xDE, 0xAD, 0xBE, 0xEF]` fails today. That
        // is a container-level conformance gap, reported rather than patched
        // here.
        assert!(
            payloads
                .iter()
                .any(|p| p.starts_with(&[0xDE, 0xAD, 0xBE, 0xEF])),
            "the first keyframe's elementary bytes must survive PES packetization; \
             got {payloads:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 4. Audio-only Opus ingest muxes to a real segment too.
    #[tokio::test]
    async fn opus_ingest_produces_real_segment() {
        let dir = tmp_dir("opus");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        let packets = vec![
            audio_packet(
                EnhancedAudioTag::sequence_start(FourCC::OPUS, opus_head()).encode(),
                0,
            ),
            audio_packet(
                EnhancedAudioTag::opus(Bytes::from_static(&[0xFC, 0x01, 0x02])).encode(),
                20,
            ),
        ];
        let segment = writer
            .create_segment("segment0.ts", &packets)
            .await
            .expect("Opus segment");
        assert_eq!(segment.data[0], 0x47);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 5. NO-FABRICATION: legacy H.264 ingest is refused and leaves no file.
    #[tokio::test]
    async fn write_segment_refuses_h264_and_writes_nothing() {
        let dir = tmp_dir("h264-write");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        let filename = "segment0.ts";

        let err = writer
            .write_segment(filename, &h264_ingest())
            .await
            .expect_err("H.264 ingest must not fabricate a .ts segment");
        let msg = err.to_string();
        assert!(msg.contains("H.264"), "error must name H.264: {msg}");
        assert!(
            msg.contains("Red List"),
            "error must cite the Red List: {msg}"
        );

        assert!(
            !dir.join(filename).exists(),
            "no fabricated segment file may be written on the honest-error path"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 6. NO-FABRICATION: in-memory creation refuses H.264 too.
    #[tokio::test]
    async fn create_segment_refuses_h264() {
        let dir = tmp_dir("h264-create");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        let err = writer
            .create_segment("segment0.ts", &h264_ingest())
            .await
            .expect_err("H.264 ingest must not fabricate an in-memory .ts segment");
        assert!(err.to_string().contains("Red List"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 7. NO-FABRICATION: coded frames without a sequence header are refused.
    #[tokio::test]
    async fn segment_without_sequence_header_is_refused() {
        let dir = tmp_dir("no-config");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        let packets = vec![video_packet(
            EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x01, 0x02])).encode(),
            0,
        )];
        let err = writer
            .create_segment("segment0.ts", &packets)
            .await
            .expect_err("no config means no honest segment");
        assert!(err.to_string().contains("sequence header"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // 8. Configuration persists across segments (the writer is stateful).
    #[tokio::test]
    async fn config_persists_across_segments() {
        let dir = tmp_dir("persist");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        assert!(!writer.is_ready());

        writer
            .create_segment("segment0.ts", &av1_ingest())
            .await
            .expect("first segment");
        assert!(writer.is_ready());

        // A later segment carrying only coded frames still muxes.
        let packets = vec![video_packet(
            EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x11, 0x22])).encode(),
            80,
        )];
        let segment = writer
            .create_segment("segment1.ts", &packets)
            .await
            .expect("second segment reuses the captured configuration");
        assert_eq!(segment.data[0], 0x47);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 9. Segment duration is measured, never invented, and includes the last
    //    sample's own span so the playlist does not lose a frame per segment.
    #[test]
    fn segment_duration_is_measured() {
        use crate::ingest_depacketizer::IngestCodec;

        let sample = |dts_ms| ElementarySample {
            codec: IngestCodec::Av1,
            kind: SampleKind::Video,
            data: Bytes::from_static(&[0]),
            dts_ms,
            composition_time_ms: 0,
            keyframe: true,
        };
        // Three 40 ms frames occupy 120 ms, not the 80 ms between first and last.
        let three = [sample(0), sample(40), sample(80)];
        assert!((measured_span_seconds(&three) - 0.120).abs() < 1e-9);
        // Fewer than two samples give no measurable delta at all.
        assert!(measured_span_seconds(&[sample(500)]).abs() < f64::EPSILON);
        assert!(measured_span_seconds(&[]).abs() < f64::EPSILON);
    }

    // 10. write_segment reports the measured duration to its caller.
    #[tokio::test]
    async fn write_segment_returns_the_measured_duration() {
        let dir = tmp_dir("duration");
        let writer = SegmentWriter::new(&dir).expect("create writer");
        // Two keyframes 40 ms apart occupy 80 ms.
        let duration = writer
            .write_segment("segment0.ts", &av1_ingest())
            .await
            .expect("segment");
        assert!(
            (duration - 0.080).abs() < 1e-9,
            "expected the measured 80 ms span, got {duration}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
