// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Integration test: reading a **fragmented** MP4 back with [`Mp4Demuxer`].
//!
//! Before `demux/mp4/fragments.rs` existed the demuxer had no `moof` handling
//! at all — a `moof` box fell into the "skip unknown top-level box" arm, so a
//! perfectly valid fMP4 file demuxed to **zero packets** without any error.
//!
//! The reference input is produced in-tree: the workspace's own
//! [`Mp4Muxer`] in `Mp4FragmentMode::Fragmented` mode, fed the exact same
//! AV1 + Opus packet list as the progressive path. The two demux results must
//! be packet-for-packet identical.

use bytes::Bytes;
use oximedia_container::{
    demux::Mp4Demuxer,
    mux::mp4::{Mp4Config, Mp4FragmentMode, Mp4Muxer},
    CodecParams, Demuxer, Packet, PacketFlags, StreamInfo,
};
use oximedia_core::{CodecId, OxiError, Rational, Timestamp};
use oximedia_io::source::MemorySource;

const VIDEO_TIMESCALE: i64 = 90_000;
const AUDIO_TIMESCALE: i64 = 48_000;
const VIDEO_FRAME_DURATION: u32 = 3_000; // ~33 ms at 90 kHz
const AUDIO_FRAME_DURATION: u32 = 960; // 20 ms at 48 kHz
const NUM_VIDEO_FRAMES: i64 = 24;
const NUM_AUDIO_FRAMES: i64 = 24;
const KEYFRAME_INTERVAL: i64 = 6;

fn video_stream() -> StreamInfo {
    let mut info = StreamInfo::new(0, CodecId::Av1, Rational::new(1, VIDEO_TIMESCALE));
    info.codec_params = CodecParams::video(1280, 720);
    info
}

fn audio_stream() -> StreamInfo {
    let mut info = StreamInfo::new(1, CodecId::Opus, Rational::new(1, AUDIO_TIMESCALE));
    info.codec_params = CodecParams::audio(48_000, 2);
    info
}

fn video_packet(frame: i64) -> Packet {
    // Distinct, variable-length payload so a mis-computed offset or size shows
    // up as a data mismatch rather than accidentally comparing equal.
    let len = 64 + (frame as usize % 11);
    let mut payload = vec![0xA0_u8; len];
    payload[0] = (frame & 0xFF) as u8;
    payload[1] = 0x5A;
    let mut ts = Timestamp::new(
        frame * i64::from(VIDEO_FRAME_DURATION),
        Rational::new(1, VIDEO_TIMESCALE),
    );
    ts.duration = Some(i64::from(VIDEO_FRAME_DURATION));
    Packet::new(
        0,
        Bytes::from(payload),
        ts,
        if frame % KEYFRAME_INTERVAL == 0 {
            PacketFlags::KEYFRAME
        } else {
            PacketFlags::empty()
        },
    )
}

fn audio_packet(frame: i64) -> Packet {
    let len = 40 + (frame as usize % 7);
    let mut payload = vec![0xC0_u8; len];
    payload[0] = (frame & 0xFF) as u8;
    let mut ts = Timestamp::new(
        frame * i64::from(AUDIO_FRAME_DURATION),
        Rational::new(1, AUDIO_TIMESCALE),
    );
    ts.duration = Some(i64::from(AUDIO_FRAME_DURATION));
    Packet::new(1, Bytes::from(payload), ts, PacketFlags::KEYFRAME)
}

fn source_packets() -> Vec<Packet> {
    let mut packets = Vec::new();
    for frame in 0..NUM_VIDEO_FRAMES.max(NUM_AUDIO_FRAMES) {
        if frame < NUM_VIDEO_FRAMES {
            packets.push(video_packet(frame));
        }
        if frame < NUM_AUDIO_FRAMES {
            packets.push(audio_packet(frame));
        }
    }
    packets
}

fn mux(mode: Mp4FragmentMode, packets: &[Packet]) -> Vec<u8> {
    let mut muxer = Mp4Muxer::new(Mp4Config::new().with_mode(mode));
    muxer.add_stream(video_stream()).expect("video stream");
    muxer.add_stream(audio_stream()).expect("audio stream");
    muxer.write_header().expect("write header");
    for packet in packets {
        muxer.write_packet(packet).expect("write packet");
    }
    muxer.finalize().expect("finalize")
}

async fn demux_all(data: Vec<u8>) -> Vec<Packet> {
    let mut demuxer = Mp4Demuxer::new(MemorySource::from_vec(data));
    demuxer.probe().await.expect("probe");
    let mut out = Vec::new();
    loop {
        match demuxer.read_packet().await {
            Ok(p) => out.push(p),
            Err(OxiError::Eof) => break,
            Err(e) => panic!("demux error: {e:?}"),
        }
    }
    out
}

fn assert_packets_equal(label: &str, a: &[Packet], b: &[Packet]) {
    assert_eq!(a.len(), b.len(), "{label}: packet count");
    for (index, (pa, pb)) in a.iter().zip(b.iter()).enumerate() {
        assert_eq!(
            pa.stream_index, pb.stream_index,
            "{label}: packet {index} stream index"
        );
        assert_eq!(
            pa.data.as_ref(),
            pb.data.as_ref(),
            "{label}: packet {index} payload"
        );
        assert_eq!(pa.pts(), pb.pts(), "{label}: packet {index} pts");
        assert_eq!(
            pa.timestamp.dts, pb.timestamp.dts,
            "{label}: packet {index} dts"
        );
        assert_eq!(
            pa.timestamp.duration, pb.timestamp.duration,
            "{label}: packet {index} duration"
        );
        assert_eq!(
            pa.is_keyframe(),
            pb.is_keyframe(),
            "{label}: packet {index} keyframe flag"
        );
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn fmp4_yields_the_same_packets_as_progressive() {
    let packets = source_packets();
    let progressive = demux_all(mux(Mp4FragmentMode::Progressive, &packets)).await;
    let fragmented = demux_all(mux(
        Mp4FragmentMode::Fragmented {
            fragment_duration_ms: 100,
        },
        &packets,
    ))
    .await;

    assert_eq!(
        progressive.len(),
        packets.len(),
        "progressive baseline must round-trip every packet"
    );
    assert!(
        !fragmented.is_empty(),
        "fragmented MP4 must not demux to zero packets"
    );
    assert_packets_equal("fmp4 vs progressive", &progressive, &fragmented);
}

#[tokio::test]
async fn fmp4_payloads_match_the_original_packets() {
    let packets = source_packets();
    let demuxed = demux_all(mux(
        Mp4FragmentMode::Fragmented {
            fragment_duration_ms: 100,
        },
        &packets,
    ))
    .await;

    for stream in 0..2usize {
        let expected: Vec<&Packet> = packets
            .iter()
            .filter(|p| p.stream_index == stream)
            .collect();
        let actual: Vec<&Packet> = demuxed
            .iter()
            .filter(|p| p.stream_index == stream)
            .collect();
        assert_eq!(
            expected.len(),
            actual.len(),
            "stream {stream}: sample count survives fragmentation"
        );
        for (index, (want, got)) in expected.iter().zip(actual.iter()).enumerate() {
            assert_eq!(
                want.data.as_ref(),
                got.data.as_ref(),
                "stream {stream} sample {index}: compressed payload"
            );
            assert_eq!(
                want.pts(),
                got.pts(),
                "stream {stream} sample {index}: presentation timestamp"
            );
            assert_eq!(
                want.is_keyframe(),
                got.is_keyframe(),
                "stream {stream} sample {index}: keyframe flag"
            );
        }
    }
}

#[tokio::test]
async fn fmp4_probe_reports_fragments_and_streams() {
    let packets = source_packets();
    let data = mux(
        Mp4FragmentMode::Fragmented {
            fragment_duration_ms: 100,
        },
        &packets,
    );

    let mut demuxer = Mp4Demuxer::new(MemorySource::from_vec(data));
    demuxer.probe().await.expect("probe");

    assert!(
        demuxer.is_fragmented(),
        "mvex must mark the file fragmented"
    );
    assert!(
        demuxer.fragment_count() >= 2,
        "24 frames with a 100 ms target should produce several moofs; found {}",
        demuxer.fragment_count()
    );

    let streams = demuxer.streams();
    assert_eq!(streams.len(), 2, "video + audio");
    assert_eq!(streams[0].codec, CodecId::Av1);
    assert_eq!(streams[1].codec, CodecId::Opus);

    let video_samples = demuxer.tracks()[0].samples.len() as i64;
    let audio_samples = demuxer.tracks()[1].samples.len() as i64;
    assert_eq!(video_samples, NUM_VIDEO_FRAMES);
    assert_eq!(audio_samples, NUM_AUDIO_FRAMES);
}

/// The fragment path must not disturb plain progressive parsing.
#[tokio::test]
async fn progressive_regression_still_parses_identically() {
    let packets = source_packets();
    let first = demux_all(mux(Mp4FragmentMode::Progressive, &packets)).await;
    let second = demux_all(mux(Mp4FragmentMode::Progressive, &packets)).await;
    assert_packets_equal("progressive determinism", &first, &second);

    let mut demuxer = Mp4Demuxer::new(MemorySource::from_vec(mux(
        Mp4FragmentMode::Progressive,
        &packets,
    )));
    demuxer.probe().await.expect("probe");
    assert!(
        !demuxer.is_fragmented(),
        "a progressive file has no mvex box"
    );
    assert_eq!(demuxer.fragment_count(), 0);
}

/// Sample DTS values must chain across fragment boundaries via `tfdt`.
#[tokio::test]
async fn fmp4_decode_timestamps_are_monotonic_per_track() {
    let packets = source_packets();
    let demuxed = demux_all(mux(
        Mp4FragmentMode::Fragmented {
            fragment_duration_ms: 100,
        },
        &packets,
    ))
    .await;

    for stream in 0..2usize {
        let mut previous: Option<i64> = None;
        for packet in demuxed.iter().filter(|p| p.stream_index == stream) {
            let dts = packet.timestamp.dts.expect("MP4 packets carry a DTS");
            if let Some(prev) = previous {
                assert!(
                    dts > prev,
                    "stream {stream}: DTS must strictly increase ({prev} → {dts})"
                );
            }
            previous = Some(dts);
        }
    }
}
