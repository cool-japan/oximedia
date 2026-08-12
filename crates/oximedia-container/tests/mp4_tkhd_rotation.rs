// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Integration test: `tkhd` rotation reaches the demuxed [`StreamInfo`].
//!
//! `TkhdBox::parse` used to skip the 3x3 transform matrix outright, so a
//! portrait phone capture — landscape pixels plus a 90-degree display rotation —
//! was indistinguishable from real landscape footage. Any 9:16 reframing stage
//! built on `codec_params.width/height` alone would then crop the wrong axis.
//!
//! The workspace muxer always writes the unity matrix, so these tests patch the
//! matrix bytes of a real muxed file in place and re-demux it.

use bytes::Bytes;
use oximedia_container::{
    demux::Mp4Demuxer,
    mux::mp4::{Mp4Config, Mp4Muxer},
    CodecParams, Demuxer, Packet, PacketFlags, StreamInfo,
};
use oximedia_core::{CodecId, Rational, Timestamp};
use oximedia_io::source::MemorySource;

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const FP_ONE: i32 = 0x0001_0000;
const FP_W_ONE: i32 = 0x4000_0000;

fn video_stream() -> StreamInfo {
    let mut info = StreamInfo::new(0, CodecId::Av1, Rational::new(1, 90_000));
    info.codec_params = CodecParams::video(WIDTH, HEIGHT);
    info
}

fn video_packet(frame: i64) -> Packet {
    let mut ts = Timestamp::new(frame * 3000, Rational::new(1, 90_000));
    ts.duration = Some(3000);
    Packet::new(0, Bytes::from(vec![0x5A_u8; 32]), ts, PacketFlags::KEYFRAME)
}

fn mux_landscape() -> Vec<u8> {
    let mut muxer = Mp4Muxer::new(Mp4Config::new());
    muxer.add_stream(video_stream()).expect("add stream");
    muxer.write_header().expect("write header");
    for frame in 0..4 {
        muxer.write_packet(&video_packet(frame)).expect("packet");
    }
    muxer.finalize().expect("finalize")
}

/// The four quarter-turn display matrices, in ISOBMFF storage order
/// `[a, b, u, c, d, v, x, y, w]`.
fn rotation_matrix(degrees: u16) -> [i32; 9] {
    #[allow(clippy::cast_possible_wrap)]
    let (w, h) = ((WIDTH as i32) << 16, (HEIGHT as i32) << 16);
    match degrees {
        90 => [0, FP_ONE, 0, -FP_ONE, 0, 0, h, 0, FP_W_ONE],
        180 => [-FP_ONE, 0, 0, 0, -FP_ONE, 0, w, h, FP_W_ONE],
        270 => [0, -FP_ONE, 0, FP_ONE, 0, 0, 0, w, FP_W_ONE],
        _ => [FP_ONE, 0, 0, 0, FP_ONE, 0, 0, 0, FP_W_ONE],
    }
}

/// Overwrites the matrix of the first version-0 `tkhd` box in `data`.
///
/// Layout after the 4-byte `tkhd` fourcc (version 0):
/// `version+flags(4) creation(4) modification(4) track_id(4) reserved(4)
///  duration(4) reserved(8) layer(2) alternate_group(2) volume(2) reserved(2)`
/// = 40 bytes, then the 36-byte matrix.
fn patch_tkhd_matrix(data: &mut [u8], matrix: [i32; 9]) {
    let tag = data
        .windows(4)
        .position(|w| w == b"tkhd")
        .expect("muxed file contains a tkhd box");
    let matrix_start = tag + 4 + 40;
    assert!(
        matrix_start + 36 <= data.len(),
        "tkhd box is long enough to hold a matrix"
    );
    for (index, value) in matrix.iter().enumerate() {
        let at = matrix_start + index * 4;
        data[at..at + 4].copy_from_slice(&value.to_be_bytes());
    }
}

async fn demux_stream_zero(data: Vec<u8>) -> StreamInfo {
    let mut demuxer = Mp4Demuxer::new(MemorySource::from_vec(data));
    demuxer.probe().await.expect("probe");
    demuxer.streams()[0].clone()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn unity_matrix_reports_zero_rotation() {
    let stream = demux_stream_zero(mux_landscape()).await;
    assert_eq!(stream.rotation, Some(0));
    assert!(!stream.is_quarter_turned());
    assert_eq!(stream.display_dimensions(), Some((WIDTH, HEIGHT)));
    let matrix = stream.display_matrix.expect("matrix is surfaced");
    assert!(matrix.is_identity());
}

#[tokio::test]
async fn ninety_degree_rotation_swaps_display_dimensions() {
    let mut data = mux_landscape();
    patch_tkhd_matrix(&mut data, rotation_matrix(90));
    let stream = demux_stream_zero(data).await;

    assert_eq!(stream.rotation, Some(90));
    assert!(stream.is_quarter_turned());
    assert_eq!(
        stream.codec_params.width,
        Some(WIDTH),
        "coded dimensions stay as stored"
    );
    assert_eq!(
        stream.display_dimensions(),
        Some((HEIGHT, WIDTH)),
        "portrait capture must present as 1080x1920"
    );

    let matrix = stream.display_matrix.expect("matrix is surfaced");
    assert!(!matrix.is_identity());
    let (tx, _ty) = matrix.translation_xy();
    #[allow(clippy::cast_precision_loss)]
    let expected_tx = f64::from(HEIGHT);
    assert!(
        (tx - expected_tx).abs() < 1e-6,
        "translation decodes as 16.16"
    );
}

#[tokio::test]
async fn one_eighty_rotation_keeps_display_dimensions() {
    let mut data = mux_landscape();
    patch_tkhd_matrix(&mut data, rotation_matrix(180));
    let stream = demux_stream_zero(data).await;

    assert_eq!(stream.rotation, Some(180));
    assert!(!stream.is_quarter_turned());
    assert_eq!(stream.display_dimensions(), Some((WIDTH, HEIGHT)));
}

#[tokio::test]
async fn two_seventy_degree_rotation_swaps_display_dimensions() {
    let mut data = mux_landscape();
    patch_tkhd_matrix(&mut data, rotation_matrix(270));
    let stream = demux_stream_zero(data).await;

    assert_eq!(stream.rotation, Some(270));
    assert!(stream.is_quarter_turned());
    assert_eq!(stream.display_dimensions(), Some((HEIGHT, WIDTH)));
}

/// Rotation metadata must not disturb packet reading.
#[tokio::test]
async fn rotated_file_still_demuxes_every_packet() {
    let mut data = mux_landscape();
    patch_tkhd_matrix(&mut data, rotation_matrix(90));

    let mut demuxer = Mp4Demuxer::new(MemorySource::from_vec(data));
    demuxer.probe().await.expect("probe");
    let mut count = 0;
    while demuxer.read_packet().await.is_ok() {
        count += 1;
    }
    assert_eq!(count, 4);
}
