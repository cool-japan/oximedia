//! Shared test helpers for oximedia-cli integration tests.

use std::path::PathBuf;
use tempfile::TempDir;

/// Build a minimal valid WAV file in memory (44 bytes header + PCM data).
///
/// Generates a pure sine wave at `freq_hz` with the given sample rate,
/// channel count, and duration.
pub fn make_sine_wav(freq_hz: f32, sample_rate: u32, channels: u16, duration_secs: f32) -> Vec<u8> {
    let num_samples = (sample_rate as f32 * duration_secs) as u32;
    let num_channels = u32::from(channels);
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * num_channels * u32::from(bits_per_sample / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_size = num_samples * num_channels * u32::from(bits_per_sample / 8);
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);
    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());
    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    // PCM samples — sine wave
    for i in 0..num_samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (2.0 * std::f32::consts::PI * freq_hz * t).sin();
        let pcm = (sample * 32767.0) as i16;
        for _ch in 0..channels {
            buf.extend_from_slice(&pcm.to_le_bytes());
        }
    }
    buf
}

/// Write a WAV file to a temp dir and return `(TempDir, path)`.
///
/// The `TempDir` must be kept alive for the duration of the test.
pub fn write_wav_fixture(
    freq_hz: f32,
    sample_rate: u32,
    channels: u16,
    duration_secs: f32,
) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create TempDir");
    let path = dir.path().join("test.wav");
    let data = make_sine_wav(freq_hz, sample_rate, channels, duration_secs);
    std::fs::write(&path, data).expect("failed to write WAV fixture");
    (dir, path)
}

/// Build a minimal single-frame 4:2:0 Y4M clip in memory with every luma
/// byte set to `luma` and every chroma byte set to `chroma`.
///
/// Only `validate_loudness_check.rs` uses this today; each `tests/*.rs` file
/// is its own crate, so an unused-by-this-particular-binary helper here
/// would otherwise be flagged as dead code in the other test binaries.
#[allow(dead_code)]
pub fn make_solid_y4m(width: u32, height: u32, luma: u8, chroma: u8) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(format!("YUV4MPEG2 W{width} H{height} C420jpeg\n").as_bytes());
    buf.extend_from_slice(b"FRAME\n");
    buf.extend(std::iter::repeat_n(luma, (width * height) as usize));
    let chroma_w = width.div_ceil(2);
    let chroma_h = height.div_ceil(2);
    buf.extend(std::iter::repeat_n(
        chroma,
        (chroma_w * chroma_h) as usize * 2,
    ));
    buf
}

/// Write a solid-color Y4M file to a temp dir and return `(TempDir, path)`.
///
/// The `TempDir` must be kept alive for the duration of the test.
#[allow(dead_code)]
pub fn write_y4m_fixture(width: u32, height: u32, luma: u8, chroma: u8) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("failed to create TempDir");
    let path = dir.path().join("test.y4m");
    let data = make_solid_y4m(width, height, luma, chroma);
    std::fs::write(&path, data).expect("failed to write Y4M fixture");
    (dir, path)
}

/// Build a minimal valid FLAC file in memory (a short ramp signal) using the
/// real `oximedia_codec::flac::FlacEncoder` — header plus every encoded
/// frame concatenated, exactly as the codec crate's own round-trip tests
/// assemble a complete stream.
#[allow(dead_code)]
pub fn make_ramp_flac(channels: u8, sample_rate: u32) -> Vec<u8> {
    use oximedia_codec::flac::{FlacConfig, FlacEncoder};

    let mut enc = FlacEncoder::new(FlacConfig {
        sample_rate,
        channels,
        bits_per_sample: 16,
    });
    let samples_per_channel = 256usize;
    let ramp: Vec<i32> = (0..samples_per_channel * channels as usize)
        .map(|i| ((i / channels as usize) as i32 % 2000) - 1000)
        .collect();
    let (header, frames) = enc.encode(&ramp).expect("encode ramp");

    let mut stream = header;
    for f in &frames {
        stream.extend_from_slice(&f.data);
    }
    stream
}
