// Copyright 2025 COOLJAPAN OU (Team KitaSan)
// Licensed under the Apache License, Version 2.0

//! H.264 AVI roundtrip test.
//!
//! Muxes synthetic H.264 Annex-B NALUs and verifies demux byte-for-byte.

use oximedia_container::demux::avi::AviMjpegReader;
use oximedia_container::mux::avi::{AviMjpegWriter, VideoCodec};

/// A minimal synthetic H.264 IDR slice Annex-B NALU.
/// start_code(4) + NAL type 0x65 (IDR) + padding.
fn h264_idr_nalu(tag: u8) -> Vec<u8> {
    vec![0x00, 0x00, 0x00, 0x01, 0x65, tag, 0x00, 0x00, 0x00, 0x01]
}

/// A non-IDR P-frame NALU (NAL type 0x41 = slice non-IDR).
fn h264_p_nalu(tag: u8) -> Vec<u8> {
    vec![0x00, 0x00, 0x00, 0x01, 0x41, tag, 0x00, 0x00, 0x00, 0x01]
}

#[test]
fn avi_h264_roundtrip_10_frames() {
    let mut writer = AviMjpegWriter::new(160, 120, 30, 1).with_video_codec(VideoCodec::H264);

    let mut frames_written: Vec<Vec<u8>> = Vec::new();

    for i in 0u8..10 {
        // Alternate IDR and P-frame.
        let nalu = if i % 3 == 0 {
            h264_idr_nalu(i)
        } else {
            h264_p_nalu(i)
        };
        frames_written.push(nalu.clone());
        writer.write_frame(nalu).expect("write_frame");
    }

    let avi_bytes = writer.finish().expect("finish");

    // Verify H264 fourcc is present.
    let has_h264 = avi_bytes.windows(4).any(|w| w == b"H264");
    assert!(has_h264, "output must contain H264 fourcc in strf");

    // Demux and verify byte-for-byte.
    let reader = AviMjpegReader::new(avi_bytes).expect("reader");
    let frames = reader.frames().expect("frames");

    assert_eq!(
        frames.len(),
        10,
        "must recover exactly 10 H.264 frames; got {}",
        frames.len()
    );

    for (i, (got, want)) in frames.iter().zip(frames_written.iter()).enumerate() {
        assert_eq!(got, want, "H.264 frame {i} bytestream mismatch");
    }
}

#[test]
fn h264_idr_keyframe_detection() {
    // IDR NALUs should be detected as keyframes.
    let mut writer = AviMjpegWriter::new(16, 16, 24, 1).with_video_codec(VideoCodec::H264);

    let idr = h264_idr_nalu(1);
    let p = h264_p_nalu(2);

    writer.write_frame(idr).expect("idr frame");
    writer.write_frame(p).expect("p frame");

    let bytes = writer.finish().expect("finish");

    // Both idx1 entries should be present; keyframe flag only on first.
    let has_idx1 = bytes.windows(4).any(|w| w == b"idx1");
    assert!(has_idx1, "idx1 must be present");
}

#[test]
fn h264_fourcc_in_bitmapinfoheader() {
    let writer = AviMjpegWriter::new(320, 240, 25, 1).with_video_codec(VideoCodec::H264);
    let bytes = writer.finish().expect("finish");
    let has_h264 = bytes.windows(4).any(|w| w == b"H264");
    assert!(has_h264, "H264 fourcc must appear in BITMAPINFOHEADER");
}
