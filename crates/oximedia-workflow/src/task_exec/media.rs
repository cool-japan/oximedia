//! Media decoding helpers shared by the real QC and analysis back-ends.
//!
//! The workflow crate does not implement any codec of its own: it drives the
//! `oximedia-container` demuxers and the `oximedia-codec` PCM decoder, and
//! reports an honest error for every format it cannot actually decode.
//!
//! Two ingest paths exist today, both fully real:
//!
//! - **WAV / RIFF** — [`WavDemuxer`] + [`PcmDecoder`] → interleaved `f32`
//!   samples normalised to ±1.0 (all standard PCM sub-formats: 8/16/24/32-bit
//!   integer, 32/64-bit float, and `WAVE_FORMAT_EXTENSIBLE` wrapping those).
//! - **Y4M / YUV4MPEG2** — [`Y4mDemuxer`] → raw planar YUV frames with the
//!   chroma planes sliced according to the stream's subsampling mode.
//!
//! Any other container returns [`MediaKind::Unsupported`] from
//! [`detect_media_kind`], which callers turn into a task failure naming the
//! detected magic bytes. Nothing is guessed from the file extension.

use std::io::Read;
use std::path::{Path, PathBuf};

use tracing::debug;

use oximedia_container::demux::y4m::Y4mChroma;
use oximedia_container::demux::{WavDemuxer, Y4mDemuxer};
use oximedia_container::Demuxer;
use oximedia_core::OxiError;
use oximedia_io::source::MemorySource;

use crate::error::{Result, WorkflowError};

/// Default cap on the number of video frames decoded for analysis.
pub const DEFAULT_MAX_FRAMES: usize = 600;

/// Default cap on the number of decoded video bytes held in memory (256 MiB).
pub const DEFAULT_MAX_BYTES: usize = 256 * 1024 * 1024;

/// Container kinds this crate can decode directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaKind {
    /// RIFF/RF64 WAVE audio.
    Wav,
    /// YUV4MPEG2 raw video.
    Y4m,
    /// Something else — carries a printable rendition of the leading bytes.
    Unsupported(String),
}

/// Reads the leading bytes of `path` for magic-number detection.
fn read_magic(path: &Path) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        WorkflowError::generic(format!("Cannot open {} for probing: {e}", path.display()))
    })?;
    let mut buf = [0u8; 16];
    let read = file.read(&mut buf).map_err(|e| {
        WorkflowError::generic(format!("Cannot read {} for probing: {e}", path.display()))
    })?;
    Ok(buf[..read].to_vec())
}

/// Renders leading bytes as a short printable description for error messages.
fn describe_magic(magic: &[u8]) -> String {
    let printable: String = magic
        .iter()
        .take(8)
        .map(|&b| {
            if b.is_ascii_graphic() {
                char::from(b)
            } else {
                '.'
            }
        })
        .collect();
    let hex: Vec<String> = magic.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("`{printable}` ({})", hex.join(" "))
}

/// Detects which of the directly decodable containers `path` holds.
///
/// # Errors
///
/// Returns an error when the file cannot be opened or read.
pub fn detect_media_kind(path: &Path) -> Result<MediaKind> {
    let magic = read_magic(path)?;
    if magic.starts_with(b"RIFF") || magic.starts_with(b"RF64") {
        return Ok(MediaKind::Wav);
    }
    if magic.starts_with(b"YUV4MPEG2") {
        return Ok(MediaKind::Y4m);
    }
    Ok(MediaKind::Unsupported(describe_magic(&magic)))
}

// ---------------------------------------------------------------------------
// Audio
// ---------------------------------------------------------------------------

/// Decoded interleaved audio.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// Interleaved `f32` samples normalised to ±1.0.
    pub samples: Vec<f32>,
    /// Channel count.
    pub channels: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl DecodedAudio {
    /// Number of sample frames (samples per channel).
    #[must_use]
    pub fn frame_count(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    /// Content duration in seconds.
    #[must_use]
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frame_count() as f64 / f64::from(self.sample_rate)
        }
    }

    /// Downmixes to mono by averaging channels (a no-op for mono input).
    #[must_use]
    pub fn to_mono(&self) -> Vec<f32> {
        let channels = self.channels.max(1) as usize;
        if channels == 1 {
            return self.samples.clone();
        }
        self.samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / frame.len() as f32)
            .collect()
    }
}

/// Converts a decoded PCM [`AudioFrame`](oximedia_codec::audio::AudioFrame)
/// into normalised `f32` samples.
fn audio_frame_to_f32(frame: &oximedia_codec::audio::AudioFrame) -> Result<Vec<f32>> {
    use oximedia_codec::audio::SampleFormat;

    let raw = &frame.samples;
    let bad_len = |unit: &str| {
        WorkflowError::generic(format!(
            "PCM decode produced a truncated {unit} frame ({} bytes)",
            raw.len()
        ))
    };

    match frame.format {
        SampleFormat::F32 => {
            if raw.len() % 4 != 0 {
                return Err(bad_len("f32"));
            }
            Ok(raw
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect())
        }
        SampleFormat::I16 => {
            if raw.len() % 2 != 0 {
                return Err(bad_len("i16"));
            }
            Ok(raw
                .chunks_exact(2)
                .map(|c| f32::from(i16::from_le_bytes([c[0], c[1]])) / 32_768.0)
                .collect())
        }
        SampleFormat::I32 => {
            if raw.len() % 4 != 0 {
                return Err(bad_len("i32"));
            }
            #[allow(clippy::cast_precision_loss)]
            Ok(raw
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0)
                .collect())
        }
        SampleFormat::U8 => Ok(raw
            .iter()
            .map(|&b| (f32::from(b) - 128.0) / 128.0)
            .collect()),
    }
}

/// Decodes a WAV/RIFF file into interleaved `f32` samples.
///
/// # Errors
///
/// Returns an error when the file is not WAV, the `fmt ` chunk describes a
/// sub-format with no PCM decoder, or demuxing/decoding fails.
pub async fn decode_wav(path: &Path) -> Result<DecodedAudio> {
    use oximedia_codec::pcm::{ByteOrder, PcmConfig, PcmDecoder, PcmFormat};
    use oximedia_container::demux::wav::WavFormat;

    if detect_media_kind(path)? != MediaKind::Wav {
        return Err(WorkflowError::generic(format!(
            "{} is not a RIFF/RF64 WAVE file",
            path.display()
        )));
    }

    let raw = std::fs::read(path)
        .map_err(|e| WorkflowError::generic(format!("Cannot read {}: {e}", path.display())))?;
    let mut demuxer = WavDemuxer::new(MemorySource::from_vec(raw));
    demuxer.probe().await.map_err(|e| {
        WorkflowError::generic(format!("WAV probe failed for {}: {e}", path.display()))
    })?;

    let (pcm_format, byte_order, channels, sample_rate) = {
        let info = demuxer.format_info().ok_or_else(|| {
            WorkflowError::generic(format!(
                "WAV {} has no `fmt ` chunk after probing",
                path.display()
            ))
        })?;
        let bits = info.bits_per_sample;
        let unsupported = |detail: String| {
            WorkflowError::generic(format!(
                "WAV {} uses an unsupported sample format: {detail}",
                path.display()
            ))
        };

        let (format, order) = match (&info.format, bits) {
            (WavFormat::Pcm, 8) => (PcmFormat::U8, ByteOrder::Little),
            (WavFormat::Pcm, 16) => (PcmFormat::I16, ByteOrder::Little),
            (WavFormat::Pcm, 24) => (PcmFormat::I24, ByteOrder::Little),
            (WavFormat::Pcm, 32) => (PcmFormat::I32, ByteOrder::Little),
            (WavFormat::IeeeFloat, 32) => (PcmFormat::F32, ByteOrder::Little),
            (WavFormat::IeeeFloat, 64) => (PcmFormat::F64, ByteOrder::Little),
            (WavFormat::Extensible, bps) => {
                let extension = info.extension.as_ref().ok_or_else(|| {
                    unsupported("WAVE_FORMAT_EXTENSIBLE without extension data".to_string())
                })?;
                let sub_code =
                    u16::from_le_bytes([extension.sub_format[0], extension.sub_format[1]]);
                match (sub_code, bps) {
                    (0x0001, 16) => (PcmFormat::I16, ByteOrder::Little),
                    (0x0001, 24) => (PcmFormat::I24, ByteOrder::Little),
                    (0x0001, 32) => (PcmFormat::I32, ByteOrder::Little),
                    (0x0003, 32) => (PcmFormat::F32, ByteOrder::Little),
                    (0x0003, 64) => (PcmFormat::F64, ByteOrder::Little),
                    _ => {
                        return Err(unsupported(format!(
                            "WAVE_FORMAT_EXTENSIBLE sub-format {sub_code:#06x} at {bps} bits"
                        )));
                    }
                }
            }
            (other, bps) => {
                return Err(unsupported(format!("{other:?} at {bps} bits")));
            }
        };

        (format, order, info.channels, info.sample_rate)
    };

    let decoder = PcmDecoder::new(PcmConfig {
        format: pcm_format,
        byte_order,
        sample_rate,
        channels: u8::try_from(channels).unwrap_or(u8::MAX),
    });

    let mut samples = Vec::new();
    loop {
        match demuxer.read_packet().await {
            Ok(packet) => {
                let frame = decoder.decode_bytes(&packet.data).map_err(|e| {
                    WorkflowError::generic(format!("PCM decode failed for {}: {e}", path.display()))
                })?;
                samples.extend_from_slice(&audio_frame_to_f32(&frame)?);
            }
            Err(OxiError::Eof) => break,
            Err(e) => {
                return Err(WorkflowError::generic(format!(
                    "WAV read failed for {}: {e}",
                    path.display()
                )));
            }
        }
    }

    if samples.is_empty() {
        return Err(WorkflowError::generic(format!(
            "WAV {} decoded to zero samples",
            path.display()
        )));
    }

    debug!(
        "Decoded {} : {} samples, {} ch @ {} Hz",
        path.display(),
        samples.len(),
        channels,
        sample_rate
    );

    Ok(DecodedAudio {
        samples,
        channels: u32::from(channels),
        sample_rate,
    })
}

// ---------------------------------------------------------------------------
// Video
// ---------------------------------------------------------------------------

/// One decoded planar YUV frame.
#[derive(Debug, Clone)]
pub struct YuvFrame {
    /// Luma plane, `width * height` bytes.
    pub y: Vec<u8>,
    /// Cb plane (absent for monochrome streams).
    pub u: Option<Vec<u8>>,
    /// Cr plane (absent for monochrome streams).
    pub v: Option<Vec<u8>>,
}

/// A decoded Y4M sequence.
#[derive(Debug, Clone)]
pub struct DecodedVideo {
    /// Frame width in pixels.
    pub width: usize,
    /// Frame height in pixels.
    pub height: usize,
    /// Chroma-plane width in pixels.
    pub chroma_width: usize,
    /// Chroma-plane height in pixels.
    pub chroma_height: usize,
    /// Frame rate numerator.
    pub fps_num: u32,
    /// Frame rate denominator.
    pub fps_den: u32,
    /// Chroma subsampling mode, as written in the Y4M header.
    pub chroma: String,
    /// Decoded frames, in presentation order.
    pub frames: Vec<YuvFrame>,
    /// `true` when decoding stopped at a limit before the end of the stream.
    pub truncated: bool,
}

impl DecodedVideo {
    /// Frame rate as frames per second.
    #[must_use]
    pub fn fps(&self) -> f64 {
        if self.fps_den == 0 {
            0.0
        } else {
            f64::from(self.fps_num) / f64::from(self.fps_den)
        }
    }

    /// `true` when the stream carries chroma planes.
    #[must_use]
    pub fn has_chroma(&self) -> bool {
        self.frames.first().is_some_and(|f| f.u.is_some())
    }
}

/// Limits applied while decoding video for analysis.
#[derive(Debug, Clone, Copy)]
pub struct DecodeLimits {
    /// Maximum number of frames decoded.
    pub max_frames: usize,
    /// Maximum number of decoded bytes held in memory.
    pub max_bytes: usize,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            max_frames: DEFAULT_MAX_FRAMES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// Chroma-plane dimensions for a Y4M subsampling mode.
///
/// Returns `None` for monochrome (no chroma planes at all).
fn chroma_dimensions(chroma: Y4mChroma, width: usize, height: usize) -> Option<(usize, usize)> {
    match chroma {
        Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv => {
            Some((width.div_ceil(2), height.div_ceil(2)))
        }
        Y4mChroma::C422 => Some((width.div_ceil(2), height)),
        Y4mChroma::C444 | Y4mChroma::C444alpha => Some((width, height)),
        Y4mChroma::Mono => None,
    }
}

/// Decodes a Y4M file into planar YUV frames (blocking; call via
/// [`decode_y4m`]).
fn decode_y4m_blocking(path: PathBuf, limits: DecodeLimits) -> Result<DecodedVideo> {
    let file = std::fs::File::open(&path)
        .map_err(|e| WorkflowError::generic(format!("Cannot open {}: {e}", path.display())))?;
    let mut demuxer = Y4mDemuxer::new(file).map_err(|e| {
        WorkflowError::generic(format!("Y4M header invalid in {}: {e}", path.display()))
    })?;

    let width = demuxer.width() as usize;
    let height = demuxer.height() as usize;
    if width == 0 || height == 0 {
        return Err(WorkflowError::generic(format!(
            "Y4M {} declares a zero-sized frame ({width}x{height})",
            path.display()
        )));
    }
    let chroma = demuxer.chroma();
    let (fps_num, fps_den) = demuxer.fps();
    let luma_len = width.saturating_mul(height);
    let chroma_dims = chroma_dimensions(chroma, width, height);
    let chroma_len = chroma_dims.map_or(0, |(w, h)| w.saturating_mul(h));

    let mut frames = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;

    loop {
        if frames.len() >= limits.max_frames {
            truncated = !demuxer.is_eof();
            break;
        }
        let raw = match demuxer.read_frame() {
            Ok(Some(raw)) => raw,
            Ok(None) => break,
            Err(e) => {
                return Err(WorkflowError::generic(format!(
                    "Y4M frame {} unreadable in {}: {e}",
                    frames.len(),
                    path.display()
                )));
            }
        };

        if raw.len() < luma_len + 2 * chroma_len {
            return Err(WorkflowError::generic(format!(
                "Y4M {} frame {} is short: {} bytes for a {width}x{height} {chroma:?} frame \
                 needing {}",
                path.display(),
                frames.len(),
                raw.len(),
                luma_len + 2 * chroma_len
            )));
        }

        bytes = bytes.saturating_add(raw.len());
        if bytes > limits.max_bytes {
            truncated = true;
            break;
        }

        let y = raw[..luma_len].to_vec();
        let (u, v) = if chroma_len == 0 {
            (None, None)
        } else {
            let u_start = luma_len;
            let v_start = luma_len + chroma_len;
            (
                Some(raw[u_start..u_start + chroma_len].to_vec()),
                Some(raw[v_start..v_start + chroma_len].to_vec()),
            )
        };
        frames.push(YuvFrame { y, u, v });
    }

    if frames.is_empty() {
        return Err(WorkflowError::generic(format!(
            "Y4M {} contains no frames",
            path.display()
        )));
    }

    let (chroma_width, chroma_height) = chroma_dims.unwrap_or((0, 0));

    debug!(
        "Decoded {} : {} frames of {}x{} {:?}",
        path.display(),
        frames.len(),
        width,
        height,
        chroma
    );

    Ok(DecodedVideo {
        width,
        height,
        chroma_width,
        chroma_height,
        fps_num,
        fps_den,
        chroma: format!("{chroma:?}"),
        frames,
        truncated,
    })
}

/// Decodes a Y4M file into planar YUV frames, bounded by `limits`.
///
/// The blocking demuxer runs on a Tokio blocking thread so the async executor
/// is never stalled by file I/O.
///
/// # Errors
///
/// Returns an error when the header is malformed, a frame is truncated, the
/// stream has no frames, or the blocking task cannot be joined.
pub async fn decode_y4m(path: &Path, limits: DecodeLimits) -> Result<DecodedVideo> {
    let owned = path.to_path_buf();
    tokio::task::spawn_blocking(move || decode_y4m_blocking(owned, limits))
        .await
        .map_err(|e| WorkflowError::generic(format!("Y4M decode task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("oximedia_wf_media_{}_{name}", std::process::id()))
    }

    fn wav_bytes(samples: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
        let data_size = (samples.len() * 2) as u32;
        let byte_rate = sample_rate * u32::from(channels) * 2;
        let mut buf = Vec::with_capacity(44 + data_size as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_size).to_le_bytes());
        buf.extend_from_slice(b"WAVEfmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes());
        buf.extend_from_slice(&channels.to_le_bytes());
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&byte_rate.to_le_bytes());
        buf.extend_from_slice(&(channels * 2).to_le_bytes());
        buf.extend_from_slice(&16u16.to_le_bytes());
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_size.to_le_bytes());
        for s in samples {
            buf.extend_from_slice(&s.to_le_bytes());
        }
        buf
    }

    fn y4m_bytes(width: usize, height: usize, frames: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(
            format!("YUV4MPEG2 W{width} H{height} F25:1 Ip A1:1 C420jpeg\n").as_bytes(),
        );
        for t in 0..frames {
            buf.extend_from_slice(b"FRAME\n");
            for y in 0..height {
                for x in 0..width {
                    buf.push(((x * 3 + y * 5 + t * 7) % 256) as u8);
                }
            }
            for _ in 0..2 {
                for _ in 0..height.div_ceil(2) {
                    for x in 0..width.div_ceil(2) {
                        buf.push(((x * 2 + t) % 256) as u8);
                    }
                }
            }
        }
        buf
    }

    #[test]
    fn detects_wav_and_y4m_and_rejects_others() {
        let wav = temp_path("detect.wav");
        std::fs::write(&wav, wav_bytes(&[0, 1, -1, 2], 8_000, 1)).expect("write wav");
        assert_eq!(detect_media_kind(&wav).expect("probe"), MediaKind::Wav);

        let y4m = temp_path("detect.y4m");
        std::fs::write(&y4m, y4m_bytes(16, 16, 1)).expect("write y4m");
        assert_eq!(detect_media_kind(&y4m).expect("probe"), MediaKind::Y4m);

        let other = temp_path("detect.bin");
        std::fs::write(&other, b"\x1a\x45\xdf\xa3not-a-media-file").expect("write bin");
        assert!(matches!(
            detect_media_kind(&other).expect("probe"),
            MediaKind::Unsupported(_)
        ));

        for path in [wav, y4m, other] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[tokio::test]
    async fn decodes_wav_to_normalised_samples() {
        let path = temp_path("decode.wav");
        let samples: Vec<i16> = (0..2_000)
            .map(|i| ((f64::from(i) * 0.05).sin() * 20_000.0) as i16)
            .collect();
        std::fs::write(&path, wav_bytes(&samples, 16_000, 1)).expect("write wav");

        let audio = decode_wav(&path).await.expect("decode");
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.frame_count(), samples.len());
        assert!(audio.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
        assert!((audio.duration_secs() - 0.125).abs() < 1e-6);

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn decode_wav_rejects_non_wav() {
        let path = temp_path("notawav.y4m");
        std::fs::write(&path, y4m_bytes(8, 8, 1)).expect("write y4m");
        let err = decode_wav(&path).await.expect_err("must reject");
        assert!(err.to_string().contains("not a RIFF"));
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn decodes_y4m_planes() {
        let path = temp_path("decode.y4m");
        std::fs::write(&path, y4m_bytes(32, 16, 4)).expect("write y4m");

        let video = decode_y4m(&path, DecodeLimits::default())
            .await
            .expect("decode");
        assert_eq!(video.width, 32);
        assert_eq!(video.height, 16);
        assert_eq!(video.chroma_width, 16);
        assert_eq!(video.chroma_height, 8);
        assert_eq!(video.frames.len(), 4);
        assert!(!video.truncated);
        assert!((video.fps() - 25.0).abs() < f64::EPSILON);
        for frame in &video.frames {
            assert_eq!(frame.y.len(), 32 * 16);
            assert_eq!(frame.u.as_ref().map(Vec::len), Some(16 * 8));
            assert_eq!(frame.v.as_ref().map(Vec::len), Some(16 * 8));
        }

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn y4m_frame_limit_marks_truncation() {
        let path = temp_path("limit.y4m");
        std::fs::write(&path, y4m_bytes(16, 16, 10)).expect("write y4m");

        let video = decode_y4m(
            &path,
            DecodeLimits {
                max_frames: 3,
                max_bytes: DEFAULT_MAX_BYTES,
            },
        )
        .await
        .expect("decode");
        assert_eq!(video.frames.len(), 3);
        assert!(video.truncated, "hitting the frame cap must be reported");

        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn y4m_with_no_frames_is_an_error() {
        let path = temp_path("empty.y4m");
        std::fs::write(&path, b"YUV4MPEG2 W16 H16 F25:1 Ip A1:1 C420jpeg\n").expect("write y4m");
        let err = decode_y4m(&path, DecodeLimits::default())
            .await
            .expect_err("no frames");
        assert!(err.to_string().contains("no frames"));
        let _ = std::fs::remove_file(path);
    }
}
