//! End-to-end proof that [`oximedia_metadata::embed`] splices metadata into
//! *real* container files rather than merely producing bytes that its own
//! parsers happen to accept.
//!
//! Every fixture here is muxed by `oximedia-container`'s own muxers and read
//! back by its own demuxers/readers, so a defect in the splice shows up as a
//! demux failure, a lost packet or a stale seek index — not as a green test.
//!
//! Coverage:
//!
//! | Test | Container | What it proves |
//! |---|---|---|
//! | [`test_matroska_tags_are_readable_and_packets_survive`] | MKV/WebM | The `Tags` element is where the demuxer looks, and every frame still demuxes byte-identically |
//! | [`test_matroska_seek_index_still_resolves_after_embedding`] | MKV/WebM | `SeekHead`/`Cues` positions were recomputed: seeking still lands on the right frame |
//! | [`test_ogg_opus_comments_round_trip`] | Ogg/Opus | Pages re-lace with valid CRCs (checked by the container's own page parser) and packets survive |
//! | [`test_flac_comments_read_back_by_the_container_reader`] | FLAC | The rewritten `VORBIS_COMMENT` block is what `FlacMetadataReader` reads |
//! | [`test_mp4_itunes_tags_and_packet_offsets_survive`] | MP4 | `moov` grew, chunk offsets were patched, and every sample still demuxes correctly |

use bytes::Bytes;
use oximedia_container::demux::{Demuxer, MatroskaDemuxer, Mp4Demuxer, OggDemuxer};
use oximedia_container::metadata::reader::{FlacMetadataReader, MetadataReader};
use oximedia_container::mux::mp4::{Mp4Config, Mp4Muxer};
use oximedia_container::mux::{
    FlacMuxer, MatroskaMuxer, Muxer, MuxerConfig, OggMuxer, OutputFormat,
};
use oximedia_container::{CodecParams, Packet, PacketFlags, StreamInfo};
use oximedia_core::{CodecId, OxiError, Rational, Timestamp};
use oximedia_io::MemorySource;
use oximedia_metadata::{Metadata, MetadataFormat, MetadataValue};

/// Frames written into every video fixture.
const FRAME_COUNT: i64 = 8;
/// Spacing between video frames, in milliseconds.
const FRAME_STEP_MS: i64 = 1000;
/// Forces one cluster per frame so the MKV fixture is genuinely multi-cluster.
const MAX_CLUSTER_DURATION_MS: u32 = 500;

// ── Shared fixtures ──────────────────────────────────────────────────────────

fn video_stream() -> StreamInfo {
    let mut info = StreamInfo::new(0, CodecId::Vp9, Rational::new(1, 1000));
    info.codec_params.width = Some(320);
    info.codec_params.height = Some(240);
    info
}

fn video_packet(index: i64) -> Packet {
    // A distinct payload per frame, so a lost or reordered frame is visible.
    let payload = vec![
        0x56,
        (index & 0xFF) as u8,
        ((index >> 8) & 0xFF) as u8,
        0x01,
    ];
    Packet::new(
        0,
        Bytes::from(payload),
        Timestamp::new(index * FRAME_STEP_MS, Rational::new(1, 1000)),
        PacketFlags::KEYFRAME,
    )
}

/// The metadata every test embeds.
fn tags(format: MetadataFormat) -> Metadata {
    let mut metadata = Metadata::new(format);
    let (title_key, artist_key) = match format {
        MetadataFormat::iTunes | MetadataFormat::QuickTime => ("desc", "cprt"),
        _ => ("TITLE", "ARTIST"),
    };
    metadata.insert(
        title_key.to_string(),
        MetadataValue::Text("Embedded By OxiMedia".to_string()),
    );
    metadata.insert(
        artist_key.to_string(),
        MetadataValue::Text("Slice 3G".to_string()),
    );
    metadata
}

/// Writes `file` to a uniquely-named path under the system temp directory and
/// reads it straight back, proving the bytes survive a real filesystem trip.
fn through_temp_file(tag: &str, file: &[u8]) -> Vec<u8> {
    let path = std::env::temp_dir().join(format!(
        "oximedia_metadata_embed_{tag}_{}.bin",
        std::process::id()
    ));
    std::fs::write(&path, file).expect("write fixture to temp_dir");
    let read_back = std::fs::read(&path).expect("read fixture back");
    let _ = std::fs::remove_file(&path);
    read_back
}

// ── Matroska ─────────────────────────────────────────────────────────────────

/// Muxes a multi-cluster `WebM` file with this workspace's own Matroska muxer.
async fn mux_matroska() -> Vec<u8> {
    let sink = MemorySource::new_writable(256 * 1024);
    let config = MuxerConfig::new().with_output_format(
        OutputFormat::new().with_max_cluster_duration_ms(MAX_CLUSTER_DURATION_MS),
    );
    let mut muxer = MatroskaMuxer::new(sink, config);
    muxer.add_stream(video_stream()).expect("add video stream");
    muxer.write_header().await.expect("write header");
    for index in 0..FRAME_COUNT {
        muxer
            .write_packet(&video_packet(index))
            .await
            .expect("write packet");
    }
    muxer.write_trailer().await.expect("write trailer");
    muxer.into_sink().written_data().to_vec()
}

/// Demuxes every packet, returning `(payload, pts)` pairs.
async fn demux_matroska(file: Vec<u8>) -> (Vec<(Vec<u8>, i64)>, Vec<(String, String)>) {
    let mut demuxer = MatroskaDemuxer::new(MemorySource::from_vec(file));
    demuxer.probe().await.expect("probe matroska");
    let tags: Vec<(String, String)> = demuxer
        .tags()
        .iter()
        .flat_map(|tag| {
            tag.simple_tags.iter().map(|simple| {
                (
                    simple.name.clone(),
                    simple.string.clone().unwrap_or_default(),
                )
            })
        })
        .collect();
    let mut packets = Vec::new();
    loop {
        match demuxer.read_packet().await {
            Ok(packet) => packets.push((packet.data.to_vec(), packet.pts())),
            Err(OxiError::Eof) => break,
            Err(error) => panic!("matroska demux error: {error:?}"),
        }
    }
    (packets, tags)
}

#[tokio::test]
async fn test_matroska_tags_are_readable_and_packets_survive() {
    let original = mux_matroska().await;
    let (original_packets, original_tags) = demux_matroska(original.clone()).await;
    assert_eq!(original_packets.len(), FRAME_COUNT as usize);
    assert!(
        original_tags.is_empty(),
        "the fixture must start without tags, or this test proves nothing"
    );

    let embedded = oximedia_metadata::embed::embed(&original, &tags(MetadataFormat::Matroska))
        .expect("embed Matroska tags");
    let embedded = through_temp_file("matroska", &embedded);

    let (packets, mut tag_pairs) = demux_matroska(embedded).await;
    tag_pairs.sort();
    assert_eq!(
        tag_pairs,
        vec![
            ("ARTIST".to_string(), "Slice 3G".to_string()),
            ("TITLE".to_string(), "Embedded By OxiMedia".to_string()),
        ],
        "the container's own demuxer must find the embedded tags"
    );
    assert_eq!(
        packets, original_packets,
        "every frame must survive the Tags splice byte-for-byte, with its timestamp"
    );
}

#[tokio::test]
async fn test_matroska_seek_index_still_resolves_after_embedding() {
    let original = mux_matroska().await;
    let embedded = oximedia_metadata::embed::embed(&original, &tags(MetadataFormat::Matroska))
        .expect("embed Matroska tags");

    // Seeking exercises the SeekHead → Cues pointer *and* every
    // CueClusterPosition, all of which the splice had to recompute.
    let target_index = 5i64;
    let target_ms = target_index * FRAME_STEP_MS;

    let mut demuxer = MatroskaDemuxer::new(MemorySource::from_vec(embedded));
    demuxer.probe().await.expect("probe embedded matroska");
    assert!(
        !demuxer.cues().is_empty(),
        "the rewritten file must still expose a Cues index"
    );
    demuxer
        .seek_to_time(target_ms as f64 / 1000.0)
        .await
        .expect("seek in the embedded file");

    let packet = demuxer.read_packet().await.expect("read after seek");
    assert_eq!(
        packet.pts(),
        target_ms,
        "seeking must land on the requested frame, which only works if the cue cluster \
         positions were recomputed for the new layout"
    );
    assert_eq!(
        packet.data.to_vec(),
        video_packet(target_index).data.to_vec(),
        "the frame reached by seeking must be the right one"
    );
}

// ── Ogg ──────────────────────────────────────────────────────────────────────

fn opus_stream() -> StreamInfo {
    let mut info = StreamInfo::new(0, CodecId::Opus, Rational::new(1, 48000));
    info.codec_params = CodecParams::audio(48000, 2);
    info
}

/// Muxes an Ogg/Opus bitstream with this workspace's own Ogg muxer.
async fn mux_ogg() -> Vec<u8> {
    let sink = MemorySource::new_writable(128 * 1024);
    let mut muxer = OggMuxer::new(sink, MuxerConfig::new());
    muxer.add_stream(opus_stream()).expect("add opus stream");
    muxer.write_header().await.expect("write header");
    for index in 0..4i64 {
        let payload = vec![index as u8 + 1; 64];
        muxer
            .write_packet(&Packet::new(
                0,
                Bytes::from(payload),
                Timestamp::new(index * 960, Rational::new(1, 48000)),
                PacketFlags::KEYFRAME,
            ))
            .await
            .expect("write packet");
    }
    muxer.write_trailer().await.expect("write trailer");
    muxer.into_sink().written_data().to_vec()
}

#[tokio::test]
async fn test_ogg_opus_comments_round_trip() {
    let original = mux_ogg().await;
    let embedded =
        oximedia_metadata::embed::embed(&original, &tags(MetadataFormat::VorbisComments))
            .expect("embed Vorbis comments into Ogg");
    let embedded = through_temp_file("ogg", &embedded);

    // 1. Every page must parse *and* pass the container's own CRC check.
    let mut offset = 0usize;
    let mut page_count = 0usize;
    while offset < embedded.len() {
        let (_, consumed) = oximedia_container::ogg_page::parse_ogg_page(&embedded[offset..])
            .expect("every rewritten page must parse with a valid CRC-32");
        offset += consumed;
        page_count += 1;
    }
    assert!(page_count >= 3, "expected a multi-page bitstream");

    // 2. The comment packet must carry the embedded tags.
    let comments =
        oximedia_metadata::embed::ogg::extract_comments(&embedded).expect("extract comments");
    let parsed = oximedia_metadata::vorbis::parse(&comments).expect("parse comment block");
    assert_eq!(
        parsed.get("TITLE").and_then(MetadataValue::as_text),
        Some("Embedded By OxiMedia")
    );
    assert_eq!(
        parsed.get("ARTIST").and_then(MetadataValue::as_text),
        Some("Slice 3G")
    );

    // 3. The container's demuxer must still probe the rewritten bitstream.
    let mut demuxer = OggDemuxer::new(MemorySource::from_vec(embedded));
    demuxer.probe().await.expect("probe the rewritten Ogg file");
}

// ── FLAC ─────────────────────────────────────────────────────────────────────

/// Muxes a FLAC stream with this workspace's own FLAC muxer.
async fn mux_flac() -> Vec<u8> {
    let sink = MemorySource::new_writable(128 * 1024);
    let mut muxer = FlacMuxer::new(sink, MuxerConfig::new());
    let mut info = StreamInfo::new(0, CodecId::Flac, Rational::new(1, 44100));
    info.codec_params = CodecParams::audio(44100, 2);
    muxer.add_stream(info).expect("add flac stream");
    muxer.write_header().await.expect("write header");
    for index in 0..3i64 {
        muxer
            .write_packet(&Packet::new(
                0,
                Bytes::from(vec![0xF8, 0x00, index as u8, 0x11]),
                Timestamp::new(index * 4096, Rational::new(1, 44100)),
                PacketFlags::KEYFRAME,
            ))
            .await
            .expect("write packet");
    }
    muxer.write_trailer().await.expect("write trailer");
    muxer.into_sink().written_data().to_vec()
}

#[tokio::test]
async fn test_flac_comments_read_back_by_the_container_reader() {
    let original = mux_flac().await;
    assert_eq!(&original[..4], b"fLaC");

    let embedded =
        oximedia_metadata::embed::embed(&original, &tags(MetadataFormat::VorbisComments))
            .expect("embed Vorbis comments into FLAC");
    let embedded = through_temp_file("flac", &embedded);

    // The container's own FLAC metadata reader must see the new tags.
    let read_back = FlacMetadataReader::read(MemorySource::from_vec(embedded.clone()))
        .await
        .expect("read FLAC metadata with the container's reader");
    assert_eq!(
        read_back.get_text("TITLE"),
        Some("Embedded By OxiMedia"),
        "the container's reader must see the rewritten VORBIS_COMMENT block"
    );
    assert_eq!(read_back.get_text("ARTIST"), Some("Slice 3G"));

    // The audio frames must be preserved verbatim.
    let original_audio = flac_audio_tail(&original);
    let embedded_audio = flac_audio_tail(&embedded);
    assert_eq!(
        original_audio, embedded_audio,
        "audio frames must survive the metadata-block rewrite byte-for-byte"
    );
}

/// Returns everything after a FLAC file's metadata-block chain.
fn flac_audio_tail(file: &[u8]) -> Vec<u8> {
    let mut pos = 4usize;
    loop {
        let is_last = file[pos] & 0x80 != 0;
        let len = (usize::from(file[pos + 1]) << 16)
            | (usize::from(file[pos + 2]) << 8)
            | usize::from(file[pos + 3]);
        pos += 4 + len;
        if is_last {
            break;
        }
    }
    file[pos..].to_vec()
}

// ── MP4 ──────────────────────────────────────────────────────────────────────

/// Muxes a progressive MP4 with this workspace's own MP4 muxer.
fn mux_mp4() -> Vec<u8> {
    let mut muxer = Mp4Muxer::new(Mp4Config::new());
    let mut info = StreamInfo::new(0, CodecId::Av1, Rational::new(1, 90000));
    info.codec_params = CodecParams::video(320, 240);
    muxer.add_stream(info).expect("add video stream");
    muxer.write_header().expect("write header");
    for index in 0..FRAME_COUNT {
        let mut payload = vec![0xAB_u8; 64];
        payload[0] = (index & 0xFF) as u8;
        let mut timestamp = Timestamp::new(index * 3000, Rational::new(1, 90000));
        timestamp.duration = Some(3000);
        muxer
            .write_packet(&Packet::new(
                0,
                Bytes::from(payload),
                timestamp,
                PacketFlags::KEYFRAME,
            ))
            .expect("write packet");
    }
    muxer.finalize().expect("finalize")
}

/// Demuxes every packet payload from an MP4 file.
async fn demux_mp4(file: Vec<u8>) -> Vec<Vec<u8>> {
    let mut demuxer = Mp4Demuxer::new(MemorySource::from_vec(file));
    demuxer.probe().await.expect("probe mp4");
    let mut packets = Vec::new();
    loop {
        match demuxer.read_packet().await {
            Ok(packet) => packets.push(packet.data.to_vec()),
            Err(OxiError::Eof) => break,
            Err(error) => panic!("mp4 demux error: {error:?}"),
        }
    }
    packets
}

#[tokio::test]
async fn test_mp4_itunes_tags_and_packet_offsets_survive() {
    let original = mux_mp4();
    let original_packets = demux_mp4(original.clone()).await;
    assert_eq!(original_packets.len(), FRAME_COUNT as usize);

    let embedded = oximedia_metadata::embed::embed(&original, &tags(MetadataFormat::iTunes))
        .expect("embed iTunes metadata into MP4");
    let embedded = through_temp_file("mp4", &embedded);
    assert_ne!(
        embedded.len(),
        original.len(),
        "the fixture must actually grow, so the chunk-offset fix-up is exercised"
    );

    // Every sample must still demux — which only holds if stco was patched.
    let packets = demux_mp4(embedded.clone()).await;
    assert_eq!(
        packets, original_packets,
        "chunk offsets must have been fixed up: every sample must still read back identically"
    );

    // And the tags must be there.
    let ilst = oximedia_metadata::embed::mp4::extract(&embedded, MetadataFormat::iTunes)
        .expect("extract ilst")
        .expect("ilst present");
    let parsed = oximedia_metadata::itunes::parse(&ilst).expect("parse ilst");
    assert_eq!(
        parsed.get("desc").and_then(MetadataValue::as_text),
        Some("Embedded By OxiMedia")
    );
    assert_eq!(
        parsed.get("cprt").and_then(MetadataValue::as_text),
        Some("Slice 3G")
    );
}
