// Copyright 2025 COOLJAPAN OU (Team KitaSan)
// Licensed under the Apache License, Version 2.0

//! AVI audio roundtrip test.
//!
//! Muxes 30 MJPEG frames with 48000 Hz 16-bit stereo PCM audio
//! (1024 samples per frame × 30 = 30720 samples) and verifies demux.

use oximedia_container::demux::avi::AviMjpegReader;
use oximedia_container::mux::avi::{AudioConfig, AviMjpegWriter};

fn fake_jpeg(tag: u8) -> Vec<u8> {
    vec![0xFF, 0xD8, tag, 0xFF, 0xD9]
}

/// Generate 1024 stereo 16-bit PCM samples (4 bytes each = 4096 bytes).
fn pcm_chunk(counter: u16) -> Vec<u8> {
    // 1024 samples × 2 channels × 2 bytes = 4096 bytes
    let mut chunk = Vec::with_capacity(4096);
    for s in 0u16..1024 {
        let val = s.wrapping_add(counter);
        // Left channel
        chunk.extend_from_slice(&val.to_le_bytes());
        // Right channel
        chunk.extend_from_slice(&val.wrapping_add(1).to_le_bytes());
    }
    debug_assert_eq!(chunk.len(), 4096);
    chunk
}

#[test]
fn avi_audio_roundtrip_30_frames() {
    const FRAMES: u32 = 30;
    const SAMPLE_RATE: u32 = 48_000;
    const CHANNELS: u16 = 2;
    const BITS: u16 = 16;
    const SAMPLES_PER_FRAME: u64 = 1024;

    let audio_cfg = AudioConfig {
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        bits_per_sample: BITS,
    };

    let mut writer = AviMjpegWriter::new(320, 240, 30, 1).with_audio(audio_cfg);

    let mut all_pcm: Vec<u8> = Vec::new();
    for i in 0u8..FRAMES as u8 {
        writer.write_frame(fake_jpeg(i)).expect("write_frame");
        let pcm = pcm_chunk(i as u16);
        all_pcm.extend_from_slice(&pcm);
        writer.write_audio_chunk(pcm);
    }

    let avi_bytes = writer.finish().expect("finish");

    // Verify file has both 00dc and 01wb chunks.
    let has_00dc = avi_bytes.windows(4).any(|w| w == b"00dc");
    let has_01wb = avi_bytes.windows(4).any(|w| w == b"01wb");
    assert!(has_00dc, "file must contain 00dc video chunks");
    assert!(has_01wb, "file must contain 01wb audio chunks");

    // Demux and verify.
    let reader = AviMjpegReader::new(avi_bytes).expect("reader");

    // Audio format.
    let fmt = reader.audio_format().expect("audio format must be present");
    assert_eq!(fmt.sample_rate, SAMPLE_RATE);
    assert_eq!(fmt.channels, CHANNELS);
    assert_eq!(fmt.bits_per_sample, BITS);

    // Video frames.
    let frames = reader.frames().expect("frames");
    assert_eq!(
        frames.len(),
        FRAMES as usize,
        "must recover {FRAMES} video frames"
    );
    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(frame, &fake_jpeg(i as u8), "video frame {i} mismatch");
    }

    // Audio data.
    let audio = reader.audio_data().expect("audio data must be present");
    let bytes_per_sample = u64::from(CHANNELS) * u64::from(BITS / 8);
    let total_samples = audio.len() as u64 / bytes_per_sample;
    let expected_samples = SAMPLES_PER_FRAME * FRAMES as u64;
    assert_eq!(
        total_samples, expected_samples,
        "expected {expected_samples} samples, got {total_samples}"
    );

    // Verify PCM content matches what was written.
    assert_eq!(audio, all_pcm, "audio byte content must match exactly");
}

#[test]
fn avi_audio_format_wfx_in_output() {
    let audio_cfg = AudioConfig {
        sample_rate: 44_100,
        channels: 1,
        bits_per_sample: 16,
    };
    let writer = AviMjpegWriter::new(8, 8, 25, 1).with_audio(audio_cfg);
    let bytes = writer.finish().expect("finish");

    let reader = AviMjpegReader::new(bytes).expect("reader");
    let fmt = reader.audio_format().expect("audio format");
    assert_eq!(fmt.sample_rate, 44_100);
    assert_eq!(fmt.channels, 1);
    assert_eq!(fmt.bits_per_sample, 16);
}
