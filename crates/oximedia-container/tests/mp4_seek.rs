// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Integration test: real repositioning `Demuxer::seek` on [`Mp4Demuxer`].
//!
//! Previously `Mp4Demuxer` inherited the trait's default `seek`, which just
//! returned "Seeking not supported"; the only random-access API was
//! `seek_sample_accurate`, and that merely *planned* a cursor without moving
//! anything, so `read_packet` kept walking from wherever it already was.
//!
//! These tests mux the AV1 + Opus fixtures used by `it_mp4_roundtrip.rs`, seek
//! to a mid-stream PTS, and assert that the demuxer resumes at the preceding
//! `stss` sync sample and that decode order continues correctly from there.

use bytes::Bytes;
use oximedia_container::{
    demux::Mp4Demuxer,
    mux::mp4::{Mp4Config, Mp4FragmentMode, Mp4Muxer},
    CodecParams, Demuxer, Packet, PacketFlags, SeekFlags, SeekTarget, StreamInfo,
};
use oximedia_core::{CodecId, OxiError, Rational, Timestamp};
use oximedia_io::source::MemorySource;

const VIDEO_TIMESCALE: i64 = 90_000;
const AUDIO_TIMESCALE: i64 = 48_000;
const VIDEO_FRAME_DURATION: i64 = 3_000; // ~33 ms at 90 kHz
const AUDIO_FRAME_DURATION: i64 = 960; // 20 ms at 48 kHz
const NUM_VIDEO_FRAMES: i64 = 16;
const NUM_AUDIO_FRAMES: i64 = 32;
const KEYFRAME_INTERVAL: i64 = 4; // sync samples at frames 0, 4, 8, 12

fn video_stream() -> StreamInfo {
    let mut info = StreamInfo::new(0, CodecId::Av1, Rational::new(1, VIDEO_TIMESCALE));
    info.codec_params = CodecParams::video(640, 360);
    info
}

fn audio_stream() -> StreamInfo {
    let mut info = StreamInfo::new(1, CodecId::Opus, Rational::new(1, AUDIO_TIMESCALE));
    info.codec_params = CodecParams::audio(48_000, 2);
    info
}

/// Deterministic, frame-identifying payload.
fn video_payload(frame: i64) -> Vec<u8> {
    let mut payload = vec![0xAB_u8; 80];
    payload[0] = (frame & 0xFF) as u8;
    payload[1] = 0xF0;
    payload
}

fn audio_payload(frame: i64) -> Vec<u8> {
    let mut payload = vec![0xCD_u8; 48];
    payload[0] = (frame & 0xFF) as u8;
    payload
}

fn is_keyframe_index(frame: i64) -> bool {
    frame % KEYFRAME_INTERVAL == 0
}

fn video_packet(frame: i64) -> Packet {
    let mut ts = Timestamp::new(
        frame * VIDEO_FRAME_DURATION,
        Rational::new(1, VIDEO_TIMESCALE),
    );
    ts.duration = Some(VIDEO_FRAME_DURATION);
    Packet::new(
        0,
        Bytes::from(video_payload(frame)),
        ts,
        if is_keyframe_index(frame) {
            PacketFlags::KEYFRAME
        } else {
            PacketFlags::empty()
        },
    )
}

fn audio_packet(frame: i64) -> Packet {
    let mut ts = Timestamp::new(
        frame * AUDIO_FRAME_DURATION,
        Rational::new(1, AUDIO_TIMESCALE),
    );
    ts.duration = Some(AUDIO_FRAME_DURATION);
    Packet::new(
        1,
        Bytes::from(audio_payload(frame)),
        ts,
        PacketFlags::KEYFRAME,
    )
}

/// Muxes an AV1 + Opus progressive MP4 with the fixture packet list.
fn mux_av_fixture(mode: Mp4FragmentMode) -> Vec<u8> {
    let mut muxer = Mp4Muxer::new(Mp4Config::new().with_mode(mode));
    muxer.add_stream(video_stream()).expect("video stream");
    muxer.add_stream(audio_stream()).expect("audio stream");
    muxer.write_header().expect("write header");
    for frame in 0..NUM_VIDEO_FRAMES {
        muxer.write_packet(&video_packet(frame)).expect("video");
    }
    for frame in 0..NUM_AUDIO_FRAMES {
        muxer.write_packet(&audio_packet(frame)).expect("audio");
    }
    muxer.finalize().expect("finalize")
}

/// Muxes a video-only MP4, so "the next packet" is unambiguous.
fn mux_video_only_fixture() -> Vec<u8> {
    let mut muxer = Mp4Muxer::new(Mp4Config::new());
    muxer.add_stream(video_stream()).expect("video stream");
    muxer.write_header().expect("write header");
    for frame in 0..NUM_VIDEO_FRAMES {
        muxer.write_packet(&video_packet(frame)).expect("video");
    }
    muxer.finalize().expect("finalize")
}

async fn open(data: Vec<u8>) -> Mp4Demuxer<MemorySource> {
    let mut demuxer = Mp4Demuxer::new(MemorySource::from_vec(data));
    demuxer.probe().await.expect("probe");
    demuxer
}

/// Drains the demuxer, keeping only packets from `stream`.
async fn drain_stream(demuxer: &mut Mp4Demuxer<MemorySource>, stream: usize) -> Vec<Packet> {
    let mut out = Vec::new();
    loop {
        match demuxer.read_packet().await {
            Ok(p) if p.stream_index == stream => out.push(p),
            Ok(_) => {}
            Err(OxiError::Eof) => break,
            Err(e) => panic!("demux error: {e:?}"),
        }
    }
    out
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn seek_is_advertised_as_supported() {
    let demuxer = open(mux_av_fixture(Mp4FragmentMode::Progressive)).await;
    assert!(
        demuxer.is_seekable(),
        "a seekable source must make the MP4 demuxer seekable"
    );
}

/// The canonical case: seek to a *non*-keyframe PTS and land on the preceding
/// `stss` sync sample. Video-only so that "the next packet" is literal.
#[tokio::test]
async fn seek_lands_on_preceding_sync_sample() {
    let mut demuxer = open(mux_video_only_fixture()).await;

    // Frame 10 is not a sync sample (keyframes are 0, 4, 8, 12).
    let target_pts = 10 * VIDEO_FRAME_DURATION;
    let position = demuxer
        .seek_to_stream_pts(0, target_pts)
        .await
        .expect("seek succeeds");

    assert_eq!(position.stream_index, 0);
    assert_eq!(
        position.sample_index, 8,
        "must rewind to the frame-8 keyframe"
    );
    assert_eq!(position.pts, 8 * VIDEO_FRAME_DURATION);
    assert_eq!(position.dts, 8 * VIDEO_FRAME_DURATION);
    assert!(position.is_sync, "landing sample must be a sync sample");

    let packet = demuxer.read_packet().await.expect("packet after seek");
    assert_eq!(packet.stream_index, 0);
    assert!(
        packet.is_keyframe(),
        "resumed packet must be the sync sample"
    );
    assert_eq!(packet.pts(), 8 * VIDEO_FRAME_DURATION);
    assert_eq!(
        packet.data.as_ref(),
        video_payload(8).as_slice(),
        "resumed packet must carry frame 8's payload"
    );
}

/// After seeking, decode order must continue frame by frame to EOF.
#[tokio::test]
async fn decode_order_continues_after_seek() {
    let mut demuxer = open(mux_video_only_fixture()).await;
    demuxer
        .seek_to_stream_pts(0, 10 * VIDEO_FRAME_DURATION)
        .await
        .expect("seek succeeds");

    let packets = drain_stream(&mut demuxer, 0).await;
    let expected: Vec<i64> = (8..NUM_VIDEO_FRAMES).collect();
    assert_eq!(
        packets.len(),
        expected.len(),
        "must emit every frame from the keyframe to EOF"
    );
    for (packet, &frame) in packets.iter().zip(expected.iter()) {
        assert_eq!(
            packet.pts(),
            frame * VIDEO_FRAME_DURATION,
            "frame {frame} pts"
        );
        assert_eq!(
            packet.data.as_ref(),
            video_payload(frame).as_slice(),
            "frame {frame} payload"
        );
        assert_eq!(
            packet.is_keyframe(),
            is_keyframe_index(frame),
            "frame {frame} keyframe flag"
        );
    }
}

/// `SeekFlags::ANY` opts out of the keyframe rewind.
#[tokio::test]
async fn seek_with_any_flag_lands_on_the_exact_sample() {
    let mut demuxer = open(mux_video_only_fixture()).await;

    let target = SeekTarget::time(10.0 * VIDEO_FRAME_DURATION as f64 / VIDEO_TIMESCALE as f64)
        .with_stream(0)
        .add_flags(SeekFlags::ANY);
    let position = demuxer.seek_position(target).await.expect("seek succeeds");

    assert_eq!(position.sample_index, 10);
    assert!(!position.is_sync, "frame 10 is not a sync sample");

    let packet = demuxer.read_packet().await.expect("packet after seek");
    assert_eq!(packet.pts(), 10 * VIDEO_FRAME_DURATION);
    assert!(!packet.is_keyframe());
}

/// The `Demuxer` trait entry point must reposition, not error out.
#[tokio::test]
async fn demuxer_trait_seek_repositions() {
    let mut demuxer = open(mux_video_only_fixture()).await;

    // Read a couple of packets first so the cursor is not already at 0.
    let _ = demuxer.read_packet().await.expect("packet 0");
    let _ = demuxer.read_packet().await.expect("packet 1");

    // 12 frames * 3000 ticks / 90000 = 0.4 s → keyframe at frame 12.
    demuxer
        .seek(SeekTarget::time(0.4).with_stream(0))
        .await
        .expect("trait seek succeeds");

    let packet = demuxer.read_packet().await.expect("packet after seek");
    assert_eq!(packet.pts(), 12 * VIDEO_FRAME_DURATION);
    assert!(packet.is_keyframe());
}

/// Seeking back to the start must replay the whole stream.
#[tokio::test]
async fn seek_to_zero_replays_everything() {
    let mut demuxer = open(mux_video_only_fixture()).await;
    let first_pass = drain_stream(&mut demuxer, 0).await;
    assert_eq!(first_pass.len(), NUM_VIDEO_FRAMES as usize);

    demuxer.seek_to_stream_pts(0, 0).await.expect("rewind");
    let second_pass = drain_stream(&mut demuxer, 0).await;

    assert_eq!(first_pass.len(), second_pass.len());
    for (a, b) in first_pass.iter().zip(second_pass.iter()) {
        assert_eq!(a.pts(), b.pts());
        assert_eq!(a.data.as_ref(), b.data.as_ref());
    }
}

/// Seeking past the end clamps to the last sync sample rather than erroring.
#[tokio::test]
async fn seek_past_end_clamps_to_last_sync_sample() {
    let mut demuxer = open(mux_video_only_fixture()).await;
    let position = demuxer
        .seek_to_stream_pts(0, 10_000 * VIDEO_FRAME_DURATION)
        .await
        .expect("seek succeeds");
    assert_eq!(position.sample_index, 12, "last keyframe is frame 12");
    assert!(position.is_sync);
}

/// A byte-oriented seek resolves through the sample table's chunk offsets.
#[tokio::test]
async fn byte_seek_resolves_through_the_sample_table() {
    let mut demuxer = open(mux_video_only_fixture()).await;

    let keyframe_offset = demuxer
        .seek_to_stream_pts(0, 8 * VIDEO_FRAME_DURATION)
        .await
        .expect("time seek")
        .byte_offset;

    // Aim a little past the frame-9 payload; the keyframe rewind must bring us
    // back to frame 8.
    #[allow(clippy::cast_precision_loss)]
    let target = SeekTarget::byte(keyframe_offset + 100).with_stream(0);
    let position = demuxer.seek_position(target).await.expect("byte seek");
    assert_eq!(position.sample_index, 8);
    assert_eq!(position.byte_offset, keyframe_offset);
}

/// Both tracks must be repositioned: audio resumes at the same wall-clock time
/// as the video keyframe, not at wherever it happened to be.
#[tokio::test]
async fn seek_realigns_the_other_track() {
    let mut demuxer = open(mux_av_fixture(Mp4FragmentMode::Progressive)).await;

    let position = demuxer
        .seek_to_stream_pts(0, 10 * VIDEO_FRAME_DURATION)
        .await
        .expect("seek succeeds");
    assert_eq!(position.sample_index, 8);

    // Video keyframe at 8 * 3000 / 90000 = 0.26667 s.
    let keyframe_seconds = (8 * VIDEO_FRAME_DURATION) as f64 / VIDEO_TIMESCALE as f64;

    let mut saw_video = false;
    let mut saw_audio = false;
    for _ in 0..4 {
        let packet = demuxer.read_packet().await.expect("packet after seek");
        #[allow(clippy::cast_precision_loss)]
        let seconds = if packet.stream_index == 0 {
            packet.pts() as f64 / VIDEO_TIMESCALE as f64
        } else {
            packet.pts() as f64 / AUDIO_TIMESCALE as f64
        };
        assert!(
            (seconds - keyframe_seconds).abs() < 0.05,
            "stream {} resumed at {seconds:.4}s, expected ≈{keyframe_seconds:.4}s",
            packet.stream_index
        );
        if packet.stream_index == 0 {
            saw_video = true;
        } else {
            saw_audio = true;
        }
    }
    assert!(saw_video && saw_audio, "both tracks must resume");
}

/// Seeking works the same way on a fragmented file, whose sample table is
/// assembled from `moof`/`trun` rather than `stbl`.
#[tokio::test]
async fn seek_works_on_fragmented_input() {
    let data = mux_av_fixture(Mp4FragmentMode::Fragmented {
        fragment_duration_ms: 100,
    });
    let mut demuxer = open(data).await;
    assert!(demuxer.is_fragmented());

    let position = demuxer
        .seek_to_stream_pts(0, 10 * VIDEO_FRAME_DURATION)
        .await
        .expect("seek succeeds");
    assert_eq!(position.sample_index, 8);
    assert_eq!(position.pts, 8 * VIDEO_FRAME_DURATION);
    assert!(position.is_sync);

    let video = drain_stream(&mut demuxer, 0).await;
    assert_eq!(video.len(), (NUM_VIDEO_FRAMES - 8) as usize);
    assert_eq!(video[0].data.as_ref(), video_payload(8).as_slice());
}

#[tokio::test]
async fn seek_rejects_an_out_of_range_stream() {
    let mut demuxer = open(mux_video_only_fixture()).await;
    let result = demuxer.seek(SeekTarget::time(0.1).with_stream(99)).await;
    assert!(result.is_err(), "out-of-range stream index must error");
}
