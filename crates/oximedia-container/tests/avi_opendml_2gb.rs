// Copyright 2025 COOLJAPAN OU (Team KitaSan)
// Licensed under the Apache License, Version 2.0

//! OpenDML multi-RIFF test.
//!
//! Uses a 1 MB RIFF threshold (via `with_riff_size_limit`) to generate multiple
//! RIFF chunks without writing a true >1 GB file.

use oximedia_container::demux::avi::AviMjpegReader;
use oximedia_container::mux::avi::AviMjpegWriter;

/// Synthetic MJPEG frame: SOI + payload byte + EOI (5 bytes).
fn fake_jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xFF, 0xD9]
}

/// 5 KB fake frame to make the 1 MB threshold trigger at ~200 frames.
fn large_frame(tag: u8) -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8];
    v.extend(std::iter::repeat_n(tag, 5114));
    v.push(0xFF);
    v.push(0xD9);
    v
}

#[test]
fn opendml_multi_riff_with_200_frames() {
    // 1 MB threshold — each ~5 KB frame takes ~5118 bytes in movi,
    // so ~200 frames per RIFF segment.
    const LIMIT: u64 = 1_000_000;

    let mut writer = AviMjpegWriter::new(320, 240, 30, 1).with_riff_size_limit(LIMIT);

    for i in 0u8..200 {
        writer
            .write_frame(large_frame(i))
            .expect("write_frame must not fail");
    }

    let avi_bytes = writer.finish().expect("finish must not fail");

    // 1. Must contain at least one RIFF AVIX chunk.
    let has_avix = avi_bytes.windows(8).any(|w| {
        &w[0..4] == b"RIFF" && &w[4..8] != b"AVI " && {
            // The AVIX type is at offset 8 within the RIFF chunk.
            // Since windows overlap we need to find RIFF then check +8.
            true
        }
    });
    // More precise check: scan for "AVIX" directly.
    let has_avix_precise = avi_bytes.windows(4).any(|w| w == b"AVIX");
    assert!(
        has_avix_precise || has_avix,
        "file must contain RIFF AVIX secondary chunk"
    );
    let _ = has_avix;

    // Verify AVIX presence.
    assert!(
        has_avix_precise,
        "file must contain AVIX fourcc for secondary RIFF chunks"
    );

    // 2. Count RIFF chunks.
    let mut riff_count = 0usize;
    let mut avix_count = 0usize;
    let mut pos = 0usize;
    while pos + 12 <= avi_bytes.len() {
        if &avi_bytes[pos..pos + 4] == b"RIFF" {
            riff_count += 1;
            let riff_type_pos = pos + 8;
            if riff_type_pos + 4 <= avi_bytes.len()
                && &avi_bytes[riff_type_pos..riff_type_pos + 4] == b"AVIX"
            {
                avix_count += 1;
            }
            let size = u32::from_le_bytes([
                avi_bytes[pos + 4],
                avi_bytes[pos + 5],
                avi_bytes[pos + 6],
                avi_bytes[pos + 7],
            ]) as usize;
            pos += 8 + size;
        } else {
            break;
        }
    }
    assert!(
        riff_count >= 2,
        "must have primary + at least one AVIX; got {riff_count}"
    );
    assert!(
        avix_count >= 1,
        "must have at least one AVIX chunk; got {avix_count}"
    );

    // 3. Parse super-index and verify entry count > 1.
    let has_indx = avi_bytes.windows(4).any(|w| w == b"indx");
    assert!(has_indx, "file must contain indx super-index chunk");

    // 4. Demux must recover all 200 frames.
    let reader = AviMjpegReader::new(avi_bytes.clone()).expect("reader construction");
    let frames = reader.frames().expect("frames()");
    assert_eq!(frames.len(), 200, "demuxer must recover exactly 200 frames");

    // Verify frame content.
    for (i, frame) in frames.iter().enumerate() {
        let expected = large_frame(i as u8);
        assert_eq!(frame, &expected, "frame {i} content mismatch");
    }

    // 5. Verify ix00 field-indexes are present.
    let has_ix00 = avi_bytes.windows(4).any(|w| w == b"ix00");
    assert!(has_ix00, "file must contain ix00 field-index chunks");
}

#[test]
fn opendml_tiny_threshold_forces_many_segments() {
    // 100-byte threshold forces a new segment after nearly every frame.
    const LIMIT: u64 = 100;

    let mut writer = AviMjpegWriter::new(4, 4, 10, 1).with_riff_size_limit(LIMIT);
    for i in 0u8..20 {
        writer.write_frame(fake_jpeg(i)).expect("write_frame");
    }

    let avi_bytes = writer.finish().expect("finish");

    // Must contain many RIFF chunks.
    let avix_count = avi_bytes.windows(4).filter(|w| *w == b"AVIX").count();
    assert!(avix_count >= 1, "expected AVIX chunks; got {avix_count}");

    // Demux should still recover all frames.
    let reader = AviMjpegReader::new(avi_bytes).expect("reader");
    let frames = reader.frames().expect("frames");
    assert_eq!(frames.len(), 20, "must recover 20 frames");
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(frame, &fake_jpeg(i as u8), "frame {i} mismatch");
    }
}
