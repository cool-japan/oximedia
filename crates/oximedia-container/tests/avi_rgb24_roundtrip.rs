// Copyright 2025 COOLJAPAN OU (Team KitaSan)
// Licensed under the Apache License, Version 2.0

//! RGB24 AVI roundtrip test.
//!
//! Muxes a 32×32 gradient BGR24 image across 5 frames and verifies demux.

use oximedia_container::demux::avi::AviMjpegReader;
use oximedia_container::mux::avi::{AviMjpegWriter, VideoCodec};

const WIDTH: u32 = 32;
const HEIGHT: u32 = 32;

/// Generate a 32×32 bottom-up BGR24 gradient image (3072 bytes).
///
/// AVI convention: bottom row first.
/// Each pixel: B = column_idx, G = row_idx, R = tag.
fn bgr24_frame(tag: u8) -> Vec<u8> {
    let mut frame = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    // Bottom-up: row 0 is the bottom-most scanline.
    for row in (0..HEIGHT).rev() {
        for col in 0..WIDTH {
            let b = (col & 0xFF) as u8;
            let g = (row & 0xFF) as u8;
            let r = tag;
            frame.push(b);
            frame.push(g);
            frame.push(r);
        }
    }
    debug_assert_eq!(frame.len(), (WIDTH * HEIGHT * 3) as usize);
    frame
}

#[test]
fn avi_rgb24_roundtrip_5_frames() {
    const FRAMES: usize = 5;

    let mut writer = AviMjpegWriter::new(WIDTH, HEIGHT, 30, 1).with_video_codec(VideoCodec::Rgb24);

    let mut frames_written: Vec<Vec<u8>> = Vec::new();
    for i in 0u8..FRAMES as u8 {
        let frame = bgr24_frame(i * 17);
        frames_written.push(frame.clone());
        writer.write_frame(frame).expect("write_frame");
    }

    let avi_bytes = writer.finish().expect("finish");

    // BI_RGB = 0 so we check for the width/height in the header instead.
    assert!(avi_bytes.len() > 12, "output must be non-trivial");

    // Demux and verify.
    let reader = AviMjpegReader::new(avi_bytes).expect("reader");
    let frames = reader.frames().expect("frames");

    assert_eq!(
        frames.len(),
        FRAMES,
        "must recover exactly {FRAMES} frames; got {}",
        frames.len()
    );

    for (i, (got, want)) in frames.iter().zip(frames_written.iter()).enumerate() {
        assert_eq!(got.len(), want.len(), "frame {i} length mismatch");
        assert_eq!(got, want, "frame {i} pixel content mismatch");
    }
}

#[test]
fn rgb24_bitmapinfoheader_bi_rgb() {
    // BI_RGB compression = 0 (four zero bytes where biCompression lives).
    let writer = AviMjpegWriter::new(WIDTH, HEIGHT, 25, 1).with_video_codec(VideoCodec::Rgb24);
    let bytes = writer.finish().expect("finish");

    // Find "strf" chunk and check biCompression = 0 at offset 16 within payload.
    let strf_pos = bytes
        .windows(4)
        .enumerate()
        .find(|(_, w)| *w == b"strf")
        .map(|(i, _)| i)
        .expect("strf chunk must be present");

    // strf payload starts at strf_pos + 8 (fourcc + size).
    // biCompression is at BITMAPINFOHEADER offset 16.
    let bi_compression_pos = strf_pos + 8 + 16;
    let bi_compression = u32::from_le_bytes([
        bytes[bi_compression_pos],
        bytes[bi_compression_pos + 1],
        bytes[bi_compression_pos + 2],
        bytes[bi_compression_pos + 3],
    ]);
    assert_eq!(bi_compression, 0, "BI_RGB must be 0 in BITMAPINFOHEADER");
}

#[test]
fn rgb24_frame_size() {
    // Verify that the expected frame byte count is correct.
    let frame = bgr24_frame(42);
    assert_eq!(frame.len(), (WIDTH * HEIGHT * 3) as usize);
}
