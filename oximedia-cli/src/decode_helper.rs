//! Shared media decoding helpers for the OxiMedia CLI.
//!
//! Provides WAV and FLAC audio decode helpers.  [`decode_wav_f32`] is used
//! by `normalize_cmd` to produce real f32 sample data instead of synthetic
//! silence; [`decode_audio_f32`] (WAV or FLAC) backs `validate`'s
//! `--loudness-check`.  Future expansion can add video decode paths here.
//!
//! # Design
//!
//! The helpers use the OxiMedia container/codec stack:
//!
//! - [`oximedia_container::demux::WavDemuxer`] for WAV/RIFF demuxing
//! - [`oximedia_codec::pcm::PcmDecoder`] for PCM → f32 decoding
//! - [`oximedia_codec::flac::FlacDecoder`] for whole-stream FLAC → i32 decoding
//!   (RFC 9639; the same decoder the codec crate's own conformance tests use)
//! - [`oximedia_io::source::MemorySource`] as the in-memory media source
//!
//! All demuxer operations are async; callers must be inside a Tokio context.
//! Unrecognised formats return [`DecodeError::UnsupportedFormat`]; callers in
//! `normalize_cmd` fall back to synthetic silence on that error.

use anyhow::Result;
use oximedia_codec::pcm::{ByteOrder, PcmConfig, PcmDecoder, PcmFormat};
use oximedia_container::demux::WavDemuxer;
use oximedia_container::Demuxer;
use oximedia_core::OxiError;
use oximedia_io::source::MemorySource;
use std::path::Path;

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

/// Errors that can occur during media decoding.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The container/codec combination is not supported.
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    /// Container probing or reading failed.
    #[error("container error: {0}")]
    Container(String),

    /// File I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// PCM decoding failed.
    #[error("pcm decode error: {0}")]
    PcmDecode(String),

    /// FLAC decoding failed.
    #[error("flac decode error: {0}")]
    FlacDecode(String),
}

// ---------------------------------------------------------------------------
// Public output type
// ---------------------------------------------------------------------------

/// Decoded audio samples (interleaved f32, normalised to ±1.0).
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// Interleaved f32 samples, normalised to ±1.0.
    pub samples: Vec<f32>,
    /// Number of audio channels.
    pub channels: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl DecodedAudio {
    /// Returns the total number of frames (samples per channel).
    #[must_use]
    #[cfg(test)]
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }
}

// ---------------------------------------------------------------------------
// WAV decode helper
// ---------------------------------------------------------------------------

/// Decode a WAV file to interleaved f32 PCM samples.
///
/// Reads the entire file into memory and decodes it using `WavDemuxer` +
/// `PcmDecoder`.  All standard WAV PCM sub-formats (8-, 16-, 24-, 32-bit
/// integer; IEEE 32- and 64-bit float) are handled.
///
/// Returns [`DecodeError::UnsupportedFormat`] for non-WAV files, which is the
/// signal for callers to fall back to synthetic silence.
///
/// # Errors
///
/// - [`DecodeError::Io`] — file read failed
/// - [`DecodeError::UnsupportedFormat`] — not a WAV file or unsupported format
/// - [`DecodeError::Container`] — WAV header malformed
/// - [`DecodeError::PcmDecode`] — PCM data could not be decoded
pub async fn decode_wav_f32(path: &Path) -> Result<DecodedAudio, DecodeError> {
    // ------------------------------------------------------------------
    // 1. Check magic bytes — reject non-WAV early without loading the file
    // ------------------------------------------------------------------
    let magic = read_magic_bytes(path)?;
    let is_wav = magic.starts_with(b"RIFF") || magic.starts_with(b"RF64");
    if !is_wav {
        return Err(DecodeError::UnsupportedFormat(format!(
            "{} does not appear to be a WAV/RIFF file",
            path.display()
        )));
    }

    // ------------------------------------------------------------------
    // 2. Load the whole file and build a MemorySource
    // ------------------------------------------------------------------
    let raw = std::fs::read(path)?;
    let source = MemorySource::from_vec(raw);

    // ------------------------------------------------------------------
    // 3. Probe the WAV header
    // ------------------------------------------------------------------
    let mut demuxer = WavDemuxer::new(source);
    demuxer
        .probe()
        .await
        .map_err(|e| DecodeError::Container(e.to_string()))?;

    // Extract format info we need for PcmConfig
    let (pcm_format, byte_order, channels, sample_rate) = {
        let fmt_info = demuxer
            .format_info()
            .ok_or_else(|| DecodeError::Container("WAV fmt chunk not found after probe".into()))?;

        let channels = fmt_info.channels;
        let sample_rate = fmt_info.sample_rate;
        let bits_per_sample = fmt_info.bits_per_sample;

        use oximedia_container::demux::wav::WavFormat;
        let (pcm_format, byte_order) = match (&fmt_info.format, bits_per_sample) {
            (WavFormat::Pcm, 8) => (PcmFormat::U8, ByteOrder::Little),
            (WavFormat::Pcm, 16) => (PcmFormat::I16, ByteOrder::Little),
            (WavFormat::Pcm, 24) => (PcmFormat::I24, ByteOrder::Little),
            (WavFormat::Pcm, 32) => (PcmFormat::I32, ByteOrder::Little),
            (WavFormat::IeeeFloat, 32) => (PcmFormat::F32, ByteOrder::Little),
            (WavFormat::IeeeFloat, 64) => (PcmFormat::F64, ByteOrder::Little),
            // Extensible: inspect sub-format — if the first 2 bytes of the
            // GUID are 0x0001 (PCM) or 0x0003 (IEEE float), handle them.
            (WavFormat::Extensible, bps) => {
                if let Some(ext) = &fmt_info.extension {
                    let sub_code = u16::from_le_bytes([ext.sub_format[0], ext.sub_format[1]]);
                    match (sub_code, bps) {
                        (0x0001, 16) => (PcmFormat::I16, ByteOrder::Little),
                        (0x0001, 24) => (PcmFormat::I24, ByteOrder::Little),
                        (0x0001, 32) => (PcmFormat::I32, ByteOrder::Little),
                        (0x0003, 32) => (PcmFormat::F32, ByteOrder::Little),
                        (0x0003, 64) => (PcmFormat::F64, ByteOrder::Little),
                        _ => {
                            return Err(DecodeError::UnsupportedFormat(format!(
                                "WAVE_FORMAT_EXTENSIBLE sub_code={sub_code:#06x} bits={bps} not supported"
                            )));
                        }
                    }
                } else {
                    return Err(DecodeError::UnsupportedFormat(
                        "WAVE_FORMAT_EXTENSIBLE without extension data".into(),
                    ));
                }
            }
            (fmt, bps) => {
                return Err(DecodeError::UnsupportedFormat(format!(
                    "WAV format {:?} {bps}-bit not supported",
                    fmt
                )));
            }
        };

        (pcm_format, byte_order, channels, sample_rate)
    };

    let pcm_config = PcmConfig {
        format: pcm_format,
        byte_order,
        sample_rate,
        channels: channels.min(u8::MAX as u16) as u8,
    };
    let decoder = PcmDecoder::new(pcm_config);

    // ------------------------------------------------------------------
    // 4. Read packets and decode
    // ------------------------------------------------------------------
    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        match demuxer.read_packet().await {
            Ok(packet) => {
                let frame = decoder
                    .decode_bytes(&packet.data)
                    .map_err(|e| DecodeError::PcmDecode(e.to_string()))?;

                // The AudioFrame stores samples as raw bytes in F32 format
                // after decoding — extract them.
                let f32_samples = audio_frame_to_f32_samples(&frame)?;
                all_samples.extend_from_slice(&f32_samples);
            }
            Err(OxiError::Eof) => break,
            Err(e) => {
                return Err(DecodeError::Container(format!("read_packet error: {e}")));
            }
        }
    }

    Ok(DecodedAudio {
        samples: all_samples,
        channels: u32::from(channels),
        sample_rate,
    })
}

// ---------------------------------------------------------------------------
// FLAC decode helper
// ---------------------------------------------------------------------------

/// Decode a FLAC file to interleaved f32 PCM samples.
///
/// Reads the entire file into memory and decodes it with
/// [`oximedia_codec::flac::FlacDecoder`] — a from-scratch RFC 9639 decoder
/// (frame header + CRC-8/CRC-16, fixed/LPC subframes, partitioned Rice
/// residuals), not the older simplified `flac_codec` module.
///
/// Returns [`DecodeError::UnsupportedFormat`] for non-FLAC files, which is
/// the signal [`decode_audio_f32`] uses to try the next decoder.
///
/// # Errors
///
/// - [`DecodeError::Io`] — file read failed
/// - [`DecodeError::UnsupportedFormat`] — not a FLAC file
/// - [`DecodeError::FlacDecode`] — the FLAC stream is malformed, or declares
///   a bits-per-sample the f32 normalisation below cannot handle (0 or >32)
pub async fn decode_flac_f32(path: &Path) -> Result<DecodedAudio, DecodeError> {
    // ------------------------------------------------------------------
    // 1. Check magic bytes — reject non-FLAC early without decoding
    // ------------------------------------------------------------------
    let magic = read_magic_bytes(path)?;
    if !magic.starts_with(b"fLaC") {
        return Err(DecodeError::UnsupportedFormat(format!(
            "{} does not appear to be a FLAC file",
            path.display()
        )));
    }

    // ------------------------------------------------------------------
    // 2. Load the whole file and decode every frame
    // ------------------------------------------------------------------
    let raw = std::fs::read(path)?;

    let mut decoder = oximedia_codec::flac::FlacDecoder::new();
    let samples_i32 = decoder
        .decode_stream(&raw)
        .map_err(|e| DecodeError::FlacDecode(e.to_string()))?;

    let info = decoder.stream_info().ok_or_else(|| {
        DecodeError::FlacDecode("FLAC stream has no STREAMINFO after decode".to_string())
    })?;
    let channels = u32::from(info.channels);
    let sample_rate = info.sample_rate;
    let bits = info.bits_per_sample;
    if bits == 0 || bits > 32 {
        return Err(DecodeError::FlacDecode(format!(
            "unsupported bits-per-sample: {bits}"
        )));
    }

    // ------------------------------------------------------------------
    // 3. Normalise signed PCM samples to f32 in [-1.0, 1.0] using the
    //    stream's own bit depth (not a hardcoded 16-bit assumption).
    // ------------------------------------------------------------------
    let scale = (1i64 << (bits - 1)) as f32;
    let samples: Vec<f32> = samples_i32.into_iter().map(|s| s as f32 / scale).collect();

    Ok(DecodedAudio {
        samples,
        channels,
        sample_rate,
    })
}

// ---------------------------------------------------------------------------
// Combined dispatcher
// ---------------------------------------------------------------------------

/// Decode a file's audio to interleaved f32 PCM, trying every format this
/// module supports: WAV/PCM first, then FLAC.
///
/// This is the entry point `validate --loudness-check` uses. Returns
/// [`DecodeError::UnsupportedFormat`] naming both formats that were tried
/// when neither recognises the file — never a silent skip.
///
/// # Errors
///
/// Same error set as [`decode_wav_f32`] and [`decode_flac_f32`]; a decode
/// failure from a *recognised* format (bad WAV header, malformed FLAC frame)
/// is returned as-is rather than masked as `UnsupportedFormat`.
pub async fn decode_audio_f32(path: &Path) -> Result<DecodedAudio, DecodeError> {
    match decode_wav_f32(path).await {
        Ok(audio) => return Ok(audio),
        Err(DecodeError::UnsupportedFormat(_)) => {}
        Err(e) => return Err(e),
    }

    match decode_flac_f32(path).await {
        Ok(audio) => Ok(audio),
        Err(DecodeError::UnsupportedFormat(_)) => Err(DecodeError::UnsupportedFormat(format!(
            "{} is neither a WAV/PCM nor a FLAC file; supported: WAV, FLAC",
            path.display()
        ))),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read the first 12 bytes of a file for magic detection.
fn read_magic_bytes(path: &Path) -> Result<Vec<u8>, DecodeError> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = [0u8; 12];
    let n = f.read(&mut buf).map_err(DecodeError::Io)?;
    Ok(buf[..n].to_vec())
}

/// Extract normalised f32 samples from an `AudioFrame`.
///
/// `PcmDecoder::decode_bytes` returns an `AudioFrame` whose `samples` field
/// contains raw bytes in the format specified by `frame.format`.  We convert
/// each sample to f32 here.
fn audio_frame_to_f32_samples(
    frame: &oximedia_codec::audio::AudioFrame,
) -> Result<Vec<f32>, DecodeError> {
    use oximedia_codec::audio::SampleFormat;
    let raw = &frame.samples;

    match frame.format {
        SampleFormat::F32 => {
            if raw.len() % 4 != 0 {
                return Err(DecodeError::PcmDecode(
                    "F32 frame: byte count not a multiple of 4".into(),
                ));
            }
            let mut out = Vec::with_capacity(raw.len() / 4);
            for chunk in raw.chunks_exact(4) {
                let arr = [chunk[0], chunk[1], chunk[2], chunk[3]];
                out.push(f32::from_le_bytes(arr));
            }
            Ok(out)
        }
        SampleFormat::I16 => {
            if raw.len() % 2 != 0 {
                return Err(DecodeError::PcmDecode(
                    "I16 frame: byte count not a multiple of 2".into(),
                ));
            }
            let mut out = Vec::with_capacity(raw.len() / 2);
            for chunk in raw.chunks_exact(2) {
                let v = i16::from_le_bytes([chunk[0], chunk[1]]);
                out.push(v as f32 / 32768.0);
            }
            Ok(out)
        }
        SampleFormat::I32 => {
            if raw.len() % 4 != 0 {
                return Err(DecodeError::PcmDecode(
                    "I32 frame: byte count not a multiple of 4".into(),
                ));
            }
            let mut out = Vec::with_capacity(raw.len() / 4);
            for chunk in raw.chunks_exact(4) {
                let v = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                #[allow(clippy::cast_precision_loss)]
                out.push(v as f32 / 2_147_483_648.0_f64 as f32);
            }
            Ok(out)
        }
        SampleFormat::U8 => {
            let mut out = Vec::with_capacity(raw.len());
            for &b in raw {
                out.push((b as f32 - 128.0) / 128.0);
            }
            Ok(out)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid WAV file in memory: RIFF header + fmt chunk + data chunk.
    fn make_sine_wav(freq_hz: f32, sample_rate: u32, channels: u16, duration_secs: f32) -> Vec<u8> {
        let num_samples = (sample_rate as f32 * duration_secs) as u32;
        let num_channels = u32::from(channels);
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * num_channels * u32::from(bits_per_sample / 8);
        let block_align = channels * (bits_per_sample / 8);
        let data_size = num_samples * num_channels * u32::from(bits_per_sample / 8);
        let file_size = 36 + data_size;

        let mut buf = Vec::with_capacity(44 + data_size as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&file_size.to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&block_align.to_le_bytes());
        buf.extend_from_slice(&bits_per_sample.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());

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

    #[tokio::test]
    async fn decode_wav_produces_samples() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_sine.wav");
        let wav = make_sine_wav(1000.0, 44100, 1, 1.0);
        std::fs::write(&path, &wav).expect("write test WAV");

        let audio = decode_wav_f32(&path).await.expect("decode WAV");
        // 44100 samples, all in [-1.0, 1.0]
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.sample_rate, 44100);
        assert!(!audio.samples.is_empty(), "samples must not be empty");
        for &s in &audio.samples {
            assert!(s >= -1.0 && s <= 1.0, "sample out of range: {s}");
        }

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn decode_non_wav_returns_unsupported() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_notawave.mkv");
        std::fs::write(&path, b"\x1a\x45\xdf\xa3fake_mkv_data").expect("write fake mkv");

        let result = decode_wav_f32(&path).await;
        assert!(
            matches!(result, Err(DecodeError::UnsupportedFormat(_))),
            "expected UnsupportedFormat, got: {result:?}"
        );

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn decoded_audio_frame_count() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_stereo.wav");
        let wav = make_sine_wav(440.0, 48000, 2, 0.5);
        std::fs::write(&path, &wav).expect("write stereo WAV");

        let audio = decode_wav_f32(&path).await.expect("decode stereo WAV");
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.sample_rate, 48000);
        // 48000 * 0.5 = 24000 frames; total interleaved samples = 48000
        let expected_frames: usize = (48000.0 * 0.5) as usize;
        assert!(
            audio.frame_count() >= expected_frames - 100,
            "frame_count {} too low (expected ~{})",
            audio.frame_count(),
            expected_frames
        );

        std::fs::remove_file(&path).ok();
    }

    /// Build a minimal valid FLAC file in memory: `FlacEncoder` header +
    /// encoded frames, exactly as `oximedia-codec`'s own decoder round-trip
    /// tests assemble one.
    fn make_ramp_flac(channels: u8, sample_rate: u32) -> Vec<u8> {
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

    #[tokio::test]
    async fn decode_flac_produces_samples() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_ramp.flac");
        std::fs::write(&path, make_ramp_flac(1, 44100)).expect("write test FLAC");

        let audio = decode_flac_f32(&path).await.expect("decode FLAC");
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.sample_rate, 44100);
        assert!(!audio.samples.is_empty(), "samples must not be empty");
        for &s in &audio.samples {
            assert!(s >= -1.0 && s <= 1.0, "sample out of range: {s}");
        }

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn decode_flac_stereo_reports_channels() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_ramp_stereo.flac");
        std::fs::write(&path, make_ramp_flac(2, 48000)).expect("write test FLAC");

        let audio = decode_flac_f32(&path).await.expect("decode stereo FLAC");
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.sample_rate, 48000);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn decode_non_flac_returns_unsupported() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_notaflac.bin");
        std::fs::write(&path, b"not a flac file at all").expect("write fake file");

        let result = decode_flac_f32(&path).await;
        assert!(
            matches!(result, Err(DecodeError::UnsupportedFormat(_))),
            "expected UnsupportedFormat, got: {result:?}"
        );

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn decode_audio_f32_dispatches_wav() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_dispatch.wav");
        std::fs::write(&path, make_sine_wav(1000.0, 44100, 1, 0.1)).expect("write WAV");

        let audio = decode_audio_f32(&path)
            .await
            .expect("decode via dispatcher");
        assert_eq!(audio.sample_rate, 44100);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn decode_audio_f32_dispatches_flac() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_dispatch.flac");
        std::fs::write(&path, make_ramp_flac(1, 44100)).expect("write FLAC");

        let audio = decode_audio_f32(&path)
            .await
            .expect("decode via dispatcher");
        assert_eq!(audio.sample_rate, 44100);

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn decode_audio_f32_names_both_formats_when_neither_matches() {
        let dir = std::env::temp_dir();
        let path = dir.join("oximedia_dh_test_dispatch_neither.bin");
        std::fs::write(&path, b"neither wav nor flac").expect("write fake file");

        let result = decode_audio_f32(&path).await;
        match result {
            Err(DecodeError::UnsupportedFormat(msg)) => {
                assert!(msg.contains("WAV"), "message should name WAV: {msg}");
                assert!(msg.contains("FLAC"), "message should name FLAC: {msg}");
            }
            other => panic!("expected UnsupportedFormat naming both formats, got: {other:?}"),
        }

        std::fs::remove_file(&path).ok();
    }
}
