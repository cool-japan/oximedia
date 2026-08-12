//! DASH segment writer for CMAF/fMP4 segments.
//!
//! Segments are produced by a **real** [`CmafMuxer`]: RTMP ingest packets are
//! depacketized by [`Depacketizer`] into elementary samples plus a
//! container-ready decoder-configuration record, one CMAF track is registered
//! per elementary stream, and the muxer emits a genuine `ftyp`+`moov`
//! initialization segment and `moof`+`mdat` media fragments.
//!
//! The configuration record handed to the muxer is already in ISO-BMFF form —
//! `av1C` / `vpcC` / `dOps` / `dfLa` payloads, including the `FullBox`
//! version/flags prefix for the two boxes that need one. See
//! [`crate::ingest_depacketizer`] for the per-codec conversion rules; the
//! Enhanced-RTMP wire format is *not* byte-identical to the ISO-BMFF form for
//! Opus or FLAC, so this is a real transcription rather than a copy.

use crate::error::{ServerError, ServerResult};
use crate::ingest_depacketizer::{
    measured_span_seconds, Depacketizer, ElementarySample, SampleKind, StreamConfig,
};
use oximedia_container::mux::{CmafBrand, CmafConfig, CmafMuxer, CmafSample, CmafTrack, TrackType};
use oximedia_net::rtmp::MediaPacket;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use tokio::fs;

/// ISO-BMFF track ID of the video track.
const VIDEO_TRACK_ID: u32 = 1;

/// ISO-BMFF track ID of the audio track.
const AUDIO_TRACK_ID: u32 = 2;

/// Initialization segment.
#[derive(Debug, Clone)]
pub struct InitSegment {
    /// Segment data.
    pub data: Vec<u8>,
}

/// Media segment.
#[derive(Debug, Clone)]
pub struct MediaSegment {
    /// Segment number.
    pub number: u64,

    /// Duration.
    pub duration: f64,

    /// Segment data.
    pub data: Vec<u8>,
}

/// Muxing state shared by the init and media segment paths.
///
/// The [`CmafMuxer`] is created lazily, once the ingest codec is known, so its
/// `ftyp` brand can reflect the actual video codec instead of a placeholder.
struct DashState {
    /// Ingest depacketizer.
    depacketizer: Depacketizer,
    /// CMAF muxer, created when the first track is registered.
    muxer: Option<CmafMuxer>,
    /// Whether tracks have been registered on the muxer.
    tracks_registered: bool,
    /// Whether a video track exists on the muxer.
    video_registered: bool,
    /// Whether an audio track exists on the muxer.
    audio_registered: bool,
}

impl DashState {
    fn new() -> Self {
        Self {
            depacketizer: Depacketizer::new(),
            muxer: None,
            tracks_registered: false,
            video_registered: false,
            audio_registered: false,
        }
    }

    /// Registers one CMAF track per configured elementary stream.
    ///
    /// The track set is fixed by the initialization segment and cannot change
    /// afterwards, so this runs exactly once. If a *second* elementary stream's
    /// sequence header arrives after that point, its samples can never appear
    /// in any fragment — [`CmafMuxer`] only emits track runs for tracks it
    /// knows — so this reports an honest error naming the codec instead of
    /// letting that stream vanish silently.
    fn register_tracks(&mut self) -> ServerResult<()> {
        let video = self.depacketizer.video_config().cloned();
        let audio = self.depacketizer.audio_config().cloned();

        if self.tracks_registered {
            if let Some(late) = self.late_track(video.as_ref(), audio.as_ref()) {
                return Err(ServerError::BadRequest(format!(
                    "{late} sequence header arrived after the CMAF initialization segment \
                     was written; a CMAF track set is fixed by its init segment and this \
                     stream's samples would be silently dropped. Restart the publish so \
                     both sequence headers precede the first segment."
                )));
            }
            return Ok(());
        }

        if video.is_none() && audio.is_none() {
            return Err(ServerError::BadRequest(
                "cannot build a CMAF track: no Enhanced-RTMP sequence header has been \
                 received yet, so no decoder configuration record is available"
                    .to_string(),
            ));
        }

        // `cav1` signals an AV1 CMAF track; anything else uses the generic
        // CMAF brand rather than claiming a profile the media does not match.
        let brand = match video.as_ref().map(|c| c.codec) {
            Some(crate::ingest_depacketizer::IngestCodec::Av1) => CmafBrand::CmafCmav1,
            _ => CmafBrand::CmafCm,
        };
        let mut muxer = CmafMuxer::new(CmafConfig {
            brand,
            ..CmafConfig::default()
        });

        if let Some(config) = video.as_ref() {
            muxer.add_track(cmaf_track(VIDEO_TRACK_ID, TrackType::Video, config));
        }
        if let Some(config) = audio.as_ref() {
            muxer.add_track(cmaf_track(AUDIO_TRACK_ID, TrackType::Audio, config));
        }

        self.video_registered = video.is_some();
        self.audio_registered = audio.is_some();
        self.muxer = Some(muxer);
        self.tracks_registered = true;
        Ok(())
    }

    /// Names an elementary stream that is configured but was never registered.
    fn late_track(
        &self,
        video: Option<&StreamConfig>,
        audio: Option<&StreamConfig>,
    ) -> Option<&'static str> {
        match (video, audio) {
            (Some(_), _) if !self.video_registered => Some("video"),
            (_, Some(_)) if !self.audio_registered => Some("audio"),
            _ => None,
        }
    }
}

/// Builds a [`CmafTrack`] from a depacketized stream configuration.
fn cmaf_track(track_id: u32, track_type: TrackType, config: &StreamConfig) -> CmafTrack {
    CmafTrack {
        track_id,
        track_type,
        codec_fourcc: config.codec.sample_entry_fourcc(),
        timescale: config.timescale(),
        width: config.width,
        height: config.height,
        sample_rate: config.sample_rate,
        channels: config.channels,
        // Already an `av1C` / `vpcC` / `dOps` / `dfLa` box payload.
        extradata: config.config_record.clone(),
    }
}

/// DASH segment writer.
pub struct DashSegmentWriter {
    /// Output directory.
    output_dir: PathBuf,

    /// Depacketizer + CMAF muxer, shared across segments.
    state: Mutex<DashState>,
}

impl DashSegmentWriter {
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
            state: Mutex::new(DashState::new()),
        })
    }

    /// Returns `true` once a codec configuration has been captured from ingest.
    ///
    /// Callers use this to distinguish "no sequence header yet" (transient —
    /// retry on the next packet) from a genuine muxing failure.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        self.state.lock().depacketizer.ready()
    }

    /// Absorbs a packet's codec configuration without consuming coded frames.
    ///
    /// Lets a caller that buffers packets for later muxing learn the stream
    /// configuration as packets arrive. Replaying the same packets through
    /// [`Self::write_media_segment`] afterwards is unaffected — sequence
    /// headers carry no samples.
    ///
    /// # Errors
    ///
    /// Returns an honest error for Red List or non-patent-free sequence
    /// headers (see [`Depacketizer::observe_config`]).
    pub fn observe_packet(&self, packet: &MediaPacket) -> ServerResult<()> {
        self.state.lock().depacketizer.observe_config(packet)
    }

    /// Writes an fMP4 initialization segment.
    ///
    /// # Errors
    ///
    /// Returns an error when no sequence header has been received yet (check
    /// [`Self::is_ready`] first), or when writing the file fails. Nothing is
    /// written to disk on the error path.
    pub async fn write_init_segment(&self, filename: &str) -> ServerResult<()> {
        let segment = self.create_init_segment()?;
        fs::write(self.output_dir.join(filename), &segment.data).await?;
        Ok(())
    }

    /// Writes an fMP4 media segment for the given packets.
    ///
    /// Returns the segment's **measured** duration in seconds, so the MPD
    /// advertises what the media actually contains rather than the packager's
    /// nominal target.
    ///
    /// # Errors
    ///
    /// Returns an error when the ingest stream cannot be depacketized (Red
    /// List codec, malformed tag, frames before their sequence header), when
    /// no elementary sample could be produced, or when writing fails. Nothing
    /// is written to disk on any error path.
    pub async fn write_media_segment(
        &self,
        number: u64,
        filename: &str,
        packets: &[MediaPacket],
    ) -> ServerResult<f64> {
        let segment = self.create_media_segment(number, packets)?;
        fs::write(self.output_dir.join(filename), &segment.data).await?;
        Ok(segment.duration)
    }

    /// Creates an in-memory fMP4 initialization segment.
    ///
    /// # Errors
    ///
    /// See [`Self::write_init_segment`].
    pub fn create_init_segment(&self) -> ServerResult<InitSegment> {
        let mut state = self.state.lock();
        state.register_tracks()?;
        let muxer = state.muxer.as_ref().ok_or_else(|| {
            ServerError::Internal("CMAF muxer missing after track registration".to_string())
        })?;
        let data = muxer.write_init_segment();
        if data.is_empty() {
            return Err(ServerError::Internal(
                "CMAF muxer produced an empty initialization segment".to_string(),
            ));
        }
        Ok(InitSegment { data })
    }

    /// Creates an in-memory fMP4 media segment from packets.
    ///
    /// # Errors
    ///
    /// See [`Self::write_media_segment`].
    pub fn create_media_segment(
        &self,
        number: u64,
        packets: &[MediaPacket],
    ) -> ServerResult<MediaSegment> {
        let mut state = self.state.lock();

        let mut samples = Vec::with_capacity(packets.len());
        for packet in packets {
            if let Some(sample) = state.depacketizer.push(packet)? {
                samples.push(sample);
            }
        }

        state.register_tracks()?;

        if samples.is_empty() {
            return Err(ServerError::BadRequest(format!(
                "cannot mux CMAF segment {number}: the {} ingest packets carried no \
                 elementary samples (sequence headers only)",
                packets.len()
            )));
        }

        // Only registered tracks may receive samples: `CmafMuxer` emits track
        // runs for the tracks in its init segment and silently ignores any
        // other `track_id`. Gating here keeps that from becoming invisible
        // data loss — `register_tracks` above already errors on a late track.
        let video_timescale = state
            .depacketizer
            .video_config()
            .filter(|_| state.video_registered)
            .map(StreamConfig::timescale);
        let audio_timescale = state
            .depacketizer
            .audio_config()
            .filter(|_| state.audio_registered)
            .map(StreamConfig::timescale);

        let cmaf_samples = build_cmaf_samples(&samples, video_timescale, audio_timescale);
        if cmaf_samples.is_empty() {
            return Err(ServerError::BadRequest(format!(
                "cannot mux CMAF segment {number}: no sample belonged to a registered track"
            )));
        }

        let muxer = state.muxer.as_mut().ok_or_else(|| {
            ServerError::Internal("CMAF muxer missing after track registration".to_string())
        })?;
        let data = muxer.write_media_segment(&cmaf_samples);
        if data.is_empty() {
            return Err(ServerError::Internal(
                "CMAF muxer produced no fragment for a non-empty sample set".to_string(),
            ));
        }

        Ok(MediaSegment {
            number,
            duration: measured_span_seconds(&samples),
            data,
        })
    }

    /// Deletes old segments.
    pub async fn cleanup_old_segments(&self, keep_count: usize) -> ServerResult<()> {
        let mut entries = fs::read_dir(&self.output_dir).await?;
        let mut files = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().and_then(|s| s.to_str()) == Some("m4s") {
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

// ─── Sample mapping ───────────────────────────────────────────────────────────

/// Converts a millisecond timestamp into a track's timescale ticks.
fn ms_to_ticks(ms: i64, timescale: u32) -> i64 {
    ms * i64::from(timescale) / 1000
}

/// Maps depacketized samples onto [`CmafSample`]s in their tracks' timescales.
///
/// Sample durations are derived from the real inter-sample decode-timestamp
/// deltas within the batch. The last sample of a track has no successor, so it
/// reuses the previous delta; a track carrying a single sample has no
/// measurable duration at all and reports `0` rather than inventing one.
fn build_cmaf_samples(
    samples: &[ElementarySample],
    video_timescale: Option<u32>,
    audio_timescale: Option<u32>,
) -> Vec<CmafSample> {
    let mut out = Vec::with_capacity(samples.len());

    for kind in [SampleKind::Video, SampleKind::Audio] {
        let (track_id, timescale) = match kind {
            SampleKind::Video => (VIDEO_TRACK_ID, video_timescale),
            SampleKind::Audio => (AUDIO_TRACK_ID, audio_timescale),
        };
        let Some(timescale) = timescale else {
            continue;
        };

        let track: Vec<&ElementarySample> = samples.iter().filter(|s| s.kind == kind).collect();
        if track.is_empty() {
            continue;
        }

        let dts_ticks: Vec<i64> = track
            .iter()
            .map(|s| ms_to_ticks(i64::from(s.dts_ms), timescale))
            .collect();

        for (index, sample) in track.iter().enumerate() {
            let dts = dts_ticks[index];
            let composition = ms_to_ticks(i64::from(sample.composition_time_ms), timescale);
            let pts = dts.saturating_add(composition).max(0);

            let duration = if index + 1 < dts_ticks.len() {
                (dts_ticks[index + 1] - dts).max(0)
            } else if index > 0 {
                (dts - dts_ticks[index - 1]).max(0)
            } else {
                0
            };

            out.push(CmafSample {
                track_id,
                pts: u64::try_from(pts).unwrap_or(0),
                dts: u64::try_from(dts).unwrap_or(0),
                duration: u32::try_from(duration).unwrap_or(0),
                data: sample.data.to_vec(),
                keyframe: sample.keyframe,
            });
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use oximedia_net::rtmp::{
        EnhancedAudioTag, EnhancedVideoTag, FourCC, MediaPacket, MediaPacketType,
    };

    fn tmp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia-dash-seg-{name}-{}", std::process::id()))
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

    fn av1_config() -> Bytes {
        Bytes::from_static(&[0x81, 0x00, 0x00, 0x00])
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

    fn flac_streaminfo() -> Bytes {
        let mut v = vec![0u8; 34];
        v[0..2].copy_from_slice(&4096u16.to_be_bytes());
        v[2..4].copy_from_slice(&4096u16.to_be_bytes());
        let sample_rate: u32 = 44_100;
        v[10] = (sample_rate >> 12) as u8;
        v[11] = (sample_rate >> 4) as u8;
        v[12] = (((sample_rate & 0x0F) as u8) << 4) | (1 << 1) | ((15 >> 4) & 0x01);
        v[13] = ((15 & 0x0F) << 4) as u8;
        Bytes::from(v)
    }

    fn av1_sequence() -> Vec<MediaPacket> {
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
                EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0xCA, 0xFE])).encode(),
                40,
            ),
        ]
    }

    fn h264_ingest() -> Vec<MediaPacket> {
        vec![video_packet(
            Bytes::from_static(&[0x17, 0x00, 0x00, 0x00, 0x00, 0x01, 0x42]),
            0,
        )]
    }

    /// Locates a top-level ISO-BMFF box by type, returning its payload.
    fn find_box<'a>(data: &'a [u8], want: &[u8; 4]) -> Option<&'a [u8]> {
        let mut pos = 0usize;
        while pos + 8 <= data.len() {
            let size = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]])
                as usize;
            if size < 8 || pos + size > data.len() {
                return None;
            }
            if &data[pos + 4..pos + 8] == want {
                return Some(&data[pos + 8..pos + size]);
            }
            pos += size;
        }
        None
    }

    /// Locates a nested box anywhere in the buffer, returning its payload.
    ///
    /// Scans for a `[size:4][type:4]` header rather than walking the box tree,
    /// because several ISO-BMFF containers on the path to a sample entry
    /// (`stsd`, `meta`, …) are `FullBox`es whose children start after a
    /// version/flags prefix — a naive tree walk desynchronises on those.
    fn find_box_deep<'a>(data: &'a [u8], want: &[u8; 4]) -> Option<&'a [u8]> {
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
            if size < 8 || header + size > data.len() {
                continue;
            }
            return Some(&data[start + 4..header + size]);
        }
        None
    }

    // 1. A real init segment is produced: ftyp + moov.
    #[tokio::test]
    async fn init_segment_is_real_ftyp_moov() {
        let dir = tmp_dir("init-real");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");
        for packet in &av1_sequence() {
            writer.observe_packet(packet).expect("observe");
        }
        assert!(writer.is_ready());

        writer
            .write_init_segment("init.mp4")
            .await
            .expect("real init segment");
        let data = std::fs::read(dir.join("init.mp4")).expect("init written");
        assert!(find_box(&data, b"ftyp").is_some(), "ftyp must be present");
        assert!(find_box(&data, b"moov").is_some(), "moov must be present");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 2. Init + media re-parse through the container crate's fMP4 ingest.
    #[tokio::test]
    async fn init_and_media_reparse_as_fragmented_mp4() {
        use oximedia_container::fragment::mp4::FragmentedMp4Ingest;

        let dir = tmp_dir("reparse");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");
        for packet in &av1_sequence() {
            writer.observe_packet(packet).expect("observe");
        }

        let init = writer.create_init_segment().expect("init");
        let media = writer
            .create_media_segment(1, &av1_sequence())
            .expect("media");

        let mut ingest = FragmentedMp4Ingest::new();
        ingest.ingest(&init.data).expect("init accepted");
        assert!(ingest.init_received(), "init segment must be recognised");

        ingest.ingest(&media.data).expect("fragment accepted");
        assert_eq!(
            ingest.fragments_received(),
            1,
            "exactly one moof fragment must be recognised"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 3. The AV1 sample entry carries a real av1C configuration box.
    #[tokio::test]
    async fn av1_sample_entry_carries_av1c() {
        let dir = tmp_dir("av1c");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");
        for packet in &av1_sequence() {
            writer.observe_packet(packet).expect("observe");
        }
        let init = writer.create_init_segment().expect("init");

        let av1c = find_box_deep(&init.data, b"av1C").expect("av1C must be present");
        assert_eq!(
            av1c,
            &av1_config()[..],
            "av1C payload must be the AV1CodecConfigurationRecord verbatim"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 4. The Opus sample entry carries a real dOps box, converted from OpusHead.
    #[tokio::test]
    async fn opus_sample_entry_carries_dops() {
        let dir = tmp_dir("dops");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");
        writer
            .observe_packet(&audio_packet(
                EnhancedAudioTag::sequence_start(FourCC::OPUS, opus_head()).encode(),
                0,
            ))
            .expect("observe");
        let init = writer.create_init_segment().expect("init");

        let dops = find_box_deep(&init.data, b"dOps").expect("dOps must be present");
        assert_eq!(dops[0], 0, "OpusSpecificBox.Version must be 0");
        assert_eq!(dops[1], 2, "OutputChannelCount");
        assert_eq!(
            u16::from_be_bytes([dops[2], dops[3]]),
            312,
            "PreSkip must be big-endian"
        );
        assert_eq!(
            u32::from_be_bytes([dops[4], dops[5], dops[6], dops[7]]),
            48_000,
            "InputSampleRate must be big-endian"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 5. The FLAC sample entry's dfLa carries the FullBox version/flags prefix.
    #[tokio::test]
    async fn flac_sample_entry_dfla_is_a_fullbox() {
        let dir = tmp_dir("dfla");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");
        writer
            .observe_packet(&audio_packet(
                EnhancedAudioTag::sequence_start(FourCC::FLAC, flac_streaminfo()).encode(),
                0,
            ))
            .expect("observe");
        let init = writer.create_init_segment().expect("init");

        let dfla = find_box_deep(&init.data, b"dfLa").expect("dfLa must be present");
        assert_eq!(
            &dfla[0..4],
            &[0, 0, 0, 0],
            "dfLa is a FullBox: version=0 + 3 flag bytes must lead the payload"
        );
        assert_eq!(dfla[4], 0x80, "STREAMINFO block with the last-block flag");
        assert_eq!(dfla.len(), 4 + 4 + 34);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 6. NO-FABRICATION: the init segment is gated on a sequence header.
    #[tokio::test]
    async fn init_segment_without_config_is_refused() {
        let dir = tmp_dir("init-nocfg");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");
        assert!(!writer.is_ready());

        let err = writer
            .write_init_segment("init.mp4")
            .await
            .expect_err("must not fabricate an init segment");
        assert!(err.to_string().contains("sequence header"));
        assert!(
            !dir.join("init.mp4").exists(),
            "no placeholder init segment may be written"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 7. NO-FABRICATION: legacy H.264 ingest is refused and leaves no file.
    #[tokio::test]
    async fn media_segment_refuses_h264_and_writes_nothing() {
        let dir = tmp_dir("h264");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");
        let filename = "segment1.m4s";

        let err = writer
            .write_media_segment(1, filename, &h264_ingest())
            .await
            .expect_err("H.264 ingest must not fabricate an .m4s fragment");
        let msg = err.to_string();
        assert!(msg.contains("H.264"), "error must name H.264: {msg}");
        assert!(
            msg.contains("Red List"),
            "error must cite the Red List: {msg}"
        );
        assert!(
            !dir.join(filename).exists(),
            "no fabricated fragment file may be written"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Counts top-level-or-nested occurrences of a box type in a buffer.
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

    // 7a. REGRESSION: an A/V publish must produce BOTH tracks in the init
    // segment.
    //
    // A CMAF track set is frozen by its init segment. Writing init after the
    // very first packet froze it around the video-only configuration (real
    // publishers send the video sequence header first), and every audio sample
    // was then silently discarded by the muxer, which only emits track runs for
    // tracks it knows.
    #[tokio::test]
    async fn av_publish_registers_both_tracks() {
        let dir = tmp_dir("av-tracks");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");

        // Video sequence header first, exactly as a real publisher sends it.
        writer
            .observe_packet(&video_packet(
                EnhancedVideoTag::sequence_start(FourCC::AV1, av1_config()).encode(),
                0,
            ))
            .expect("observe video");
        writer
            .observe_packet(&audio_packet(
                EnhancedAudioTag::sequence_start(FourCC::OPUS, opus_head()).encode(),
                0,
            ))
            .expect("observe audio");

        let init = writer.create_init_segment().expect("init");
        assert_eq!(
            count_boxes(&init.data, b"trak"),
            2,
            "both the video and the audio track must be in the init segment"
        );
        assert!(find_box_deep(&init.data, b"av1C").is_some(), "av1C present");
        assert!(find_box_deep(&init.data, b"dOps").is_some(), "dOps present");

        // Both streams' samples must actually reach the fragment.
        let packets = vec![
            video_packet(
                EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0xDE, 0xAD])).encode(),
                0,
            ),
            audio_packet(
                EnhancedAudioTag::opus(Bytes::from_static(&[0xFC, 0x01])).encode(),
                0,
            ),
            video_packet(
                EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0xBE, 0xEF])).encode(),
                40,
            ),
            audio_packet(
                EnhancedAudioTag::opus(Bytes::from_static(&[0xFC, 0x02])).encode(),
                20,
            ),
        ];
        let media = writer.create_media_segment(1, &packets).expect("media");
        assert_eq!(
            count_boxes(&media.data, b"traf"),
            2,
            "the fragment must carry one traf per track — audio must not vanish"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 7b. A track whose sequence header arrives after init is refused loudly.
    #[tokio::test]
    async fn track_appearing_after_init_is_refused_not_dropped() {
        let dir = tmp_dir("late-track");
        let writer = DashSegmentWriter::new(&dir).expect("create writer");

        writer
            .observe_packet(&video_packet(
                EnhancedVideoTag::sequence_start(FourCC::AV1, av1_config()).encode(),
                0,
            ))
            .expect("observe video");
        writer.create_init_segment().expect("video-only init");

        // Audio shows up only now — its samples could never appear in a
        // fragment, so this must be reported rather than silently dropped.
        writer
            .observe_packet(&audio_packet(
                EnhancedAudioTag::sequence_start(FourCC::OPUS, opus_head()).encode(),
                0,
            ))
            .expect("observe audio");

        let err = writer
            .create_media_segment(1, &[])
            .expect_err("a late audio track must be reported");
        let msg = err.to_string();
        assert!(msg.contains("audio"), "message must name the stream: {msg}");
        assert!(
            msg.contains("initialization segment"),
            "message must explain why: {msg}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // 8. Sample durations come from real timestamp deltas.
    #[test]
    fn sample_durations_are_measured_not_invented() {
        use crate::ingest_depacketizer::IngestCodec;

        let sample = |dts_ms| ElementarySample {
            codec: IngestCodec::Av1,
            kind: SampleKind::Video,
            data: Bytes::from_static(&[0x01]),
            dts_ms,
            composition_time_ms: 0,
            keyframe: true,
        };
        let samples = [sample(0), sample(40), sample(80)];
        let mapped = build_cmaf_samples(&samples, Some(90_000), None);
        assert_eq!(mapped.len(), 3);
        assert_eq!(mapped[0].dts, 0);
        assert_eq!(mapped[1].dts, 40 * 90);
        assert_eq!(mapped[0].duration, 40 * 90, "delta to the next sample");
        assert_eq!(
            mapped[2].duration,
            40 * 90,
            "the last sample reuses the previous measured delta"
        );

        // A single sample has no measurable duration.
        let one = [sample(500)];
        let mapped = build_cmaf_samples(&one, Some(90_000), None);
        assert_eq!(mapped[0].duration, 0);
    }

    // 9. Audio samples land in the audio track's own timescale.
    #[test]
    fn audio_samples_use_the_audio_timescale() {
        use crate::ingest_depacketizer::IngestCodec;

        let sample = |dts_ms| ElementarySample {
            codec: IngestCodec::Opus,
            kind: SampleKind::Audio,
            data: Bytes::from_static(&[0x01]),
            dts_ms,
            composition_time_ms: 0,
            keyframe: true,
        };
        let samples = [sample(0), sample(20)];
        let mapped = build_cmaf_samples(&samples, None, Some(48_000));
        assert_eq!(mapped[0].track_id, AUDIO_TRACK_ID);
        assert_eq!(mapped[1].dts, 20 * 48, "20 ms at 48 kHz = 960 ticks");
    }

    // 10. Composition-time offsets shift PTS relative to DTS.
    #[test]
    fn composition_time_offsets_pts() {
        use crate::ingest_depacketizer::IngestCodec;

        let samples = [ElementarySample {
            codec: IngestCodec::Av1,
            kind: SampleKind::Video,
            data: Bytes::from_static(&[0x01]),
            dts_ms: 100,
            composition_time_ms: 33,
            keyframe: true,
        }];
        let mapped = build_cmaf_samples(&samples, Some(90_000), None);
        assert_eq!(mapped[0].dts, 100 * 90);
        assert_eq!(mapped[0].pts, (100 + 33) * 90);
    }
}
