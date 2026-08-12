//! Real container/codec plumbing for worker QC, analysis, fingerprint,
//! transcode and thumbnail tasks.
//!
//! `oximedia-farm` does not implement any codec of its own here — it drives
//! `oximedia-container`'s WAV/Y4M demuxers and `oximedia-codec`'s PCM
//! decoder, and reports an honest [`FarmError::Task`] for every format it
//! cannot actually decode rather than fabricating a result.
//!
//! # Why containers are magic-byte-checked before trusting `oximedia-qc`
//!
//! `oximedia-qc`'s file probe ([`oximedia_qc::QualityControl::validate`])
//! only reads real stream metadata for the containers it recognises by magic
//! bytes (ISO-BMFF, Matroska/WebM, AVI, WAV, FLAC, Ogg, MXF). For anything
//! else it falls back to *synthesised* stream information (a default AV1
//! video stream), so a "passed" verdict on such a file would describe
//! invented metadata rather than the file itself. [`assert_qc_probeable`]
//! rejects such inputs up front with an honest error instead of reporting a
//! QC result that was never really measured.

use std::path::Path;

use tokio::io::AsyncReadExt;

use crate::{FarmError, Result};

// ---------------------------------------------------------------------------
// Magic-byte detection
// ---------------------------------------------------------------------------

/// Reads the leading bytes of `path` for magic-number detection.
///
/// # Errors
///
/// Returns [`FarmError::NotFound`] when the file does not exist, or
/// [`FarmError::Task`] for any other I/O failure while opening/reading it.
async fn read_magic(path: &str) -> Result<Vec<u8>> {
    let mut file = tokio::fs::File::open(path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FarmError::NotFound(format!("input not found: {path}"))
        } else {
            FarmError::Task(format!("cannot open '{path}': {e}"))
        }
    })?;
    let mut buf = [0u8; 16];
    let read = file
        .read(&mut buf)
        .await
        .map_err(|e| FarmError::Task(format!("cannot read '{path}': {e}")))?;
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

/// Container formats whose stream metadata `oximedia-qc` really parses (see
/// the magic-byte dispatch in `oximedia_qc::QualityControl::probe_file`).
///
/// Caveat: for `"FLAC"` and `"Ogg"` this only means the *container* and
/// codec identity are read from real bytes — `probe_file`'s
/// `probe_audio_container` still hardcodes the sample rate (44100 Hz for
/// FLAC, 48000 Hz for Ogg/Opus) instead of parsing STREAMINFO/OpusHead. QC
/// presets that never read `sample_rate` (e.g. the default [`Basic`
/// preset](oximedia_qc::QcPreset::Basic)) are unaffected; presets that add
/// `SampleRateValidation` (`Streaming`/`Broadcast`/`Comprehensive`/
/// `YouTube`/`Vimeo`) could report a real FLAC/Ogg file's sample rate as
/// passing or failing against that invented value rather than its own. This
/// is a gap in `oximedia-qc` itself (out of this crate's scope), not
/// something this gate can close by rejecting the container outright — the
/// container and codec fields it reports for FLAC/Ogg are still genuine.
fn qc_probeable_container(magic: &[u8]) -> Option<&'static str> {
    if magic.len() >= 8 && &magic[4..8] == b"ftyp" {
        return Some("ISO-BMFF (MP4/MOV)");
    }
    if magic.len() >= 4 {
        if magic[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
            return Some("Matroska/WebM");
        }
        if &magic[..4] == b"fLaC" {
            return Some("FLAC");
        }
        if &magic[..4] == b"OggS" {
            return Some("Ogg");
        }
        if magic[..4] == [0x06, 0x0E, 0x2B, 0x34] {
            return Some("MXF");
        }
    }
    if magic.len() >= 12 && &magic[..4] == b"RIFF" {
        return match &magic[8..12] {
            b"AVI " => Some("AVI"),
            b"WAVE" => Some("WAV"),
            _ => None,
        };
    }
    None
}

/// `true` when the leading bytes identify a RIFF/WAVE container.
///
/// Note: `oximedia_container::demux::wav::WavDemuxer::probe` only accepts
/// the `RIFF` magic (its `RF64_MAGIC` constant for >4 GiB WAVE files is
/// currently unused by `parse_header`), so RF64 is deliberately not
/// accepted here — claiming support this crate cannot deliver would just
/// swap an honest "unsupported format" error for a confusing "WAV probe
/// failed: Not a RIFF file" one.
fn is_wav(magic: &[u8]) -> bool {
    magic.len() >= 12 && &magic[..4] == b"RIFF" && &magic[8..12] == b"WAVE"
}

/// `true` when the leading bytes identify a YUV4MPEG2 (Y4M) stream.
fn is_y4m(magic: &[u8]) -> bool {
    magic.starts_with(b"YUV4MPEG2")
}

/// Container kinds the transcode task can genuinely dispatch: exactly the
/// two inputs `oximedia-transcode`'s frame-level path decodes for real (see
/// its module docs) — anything else it would reject with its own honest
/// error anyway, but detecting it here lets the task fail before touching
/// the pipeline at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MediaKind {
    /// RIFF/WAVE audio.
    Wav,
    /// YUV4MPEG2 raw video.
    Y4m,
}

/// Detects whether `path` is a WAV or Y4M container by magic bytes.
///
/// # Errors
///
/// Returns [`FarmError::NotFound`] when the file is missing, or
/// [`FarmError::Task`] naming the detected bytes when the container is
/// neither WAV nor Y4M.
pub(super) async fn detect_media_kind(path: &str) -> Result<MediaKind> {
    let magic = read_magic(path).await?;
    if is_wav(&magic) {
        Ok(MediaKind::Wav)
    } else if is_y4m(&magic) {
        Ok(MediaKind::Y4m)
    } else {
        Err(FarmError::Task(format!(
            "unsupported input container for '{path}': expected RIFF/WAVE (WAV) or YUV4MPEG2 \
             (Y4M) (leading bytes {}); the transcode task currently supports WAV and Y4M \
             inputs only",
            describe_magic(&magic)
        )))
    }
}

/// Rejects inputs for which `oximedia-qc` would synthesise stream metadata
/// instead of genuinely parsing the container.
///
/// # Errors
///
/// Returns [`FarmError::NotFound`] when the file is missing, or
/// [`FarmError::Task`] when it cannot be read or its container is not one
/// `oximedia-qc` genuinely probes.
pub(super) async fn assert_qc_probeable(path: &str) -> Result<&'static str> {
    let magic = read_magic(path).await?;
    qc_probeable_container(&magic).ok_or_else(|| {
        FarmError::Task(format!(
            "QC cannot honestly validate '{path}': oximedia-qc only parses real stream \
             metadata for ISO-BMFF (MP4/MOV), Matroska/WebM, AVI, WAV, FLAC, Ogg and MXF; \
             leading bytes {} match none of those, and oximedia-qc would otherwise \
             synthesise invented stream metadata for it",
            describe_magic(&magic)
        ))
    })
}

// ---------------------------------------------------------------------------
// WAV / PCM decode
// ---------------------------------------------------------------------------

/// Decoded interleaved PCM audio, normalised to `[-1.0, 1.0]`.
#[derive(Debug, Clone)]
pub(super) struct DecodedAudio {
    /// Interleaved `f32` samples normalised to ±1.0.
    pub samples: Vec<f32>,
    /// Channel count.
    pub channels: u32,
    /// Sample rate in Hz.
    pub sample_rate: u32,
}

impl DecodedAudio {
    /// Number of sample frames (samples per channel).
    fn frame_count(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }

    /// Content duration in seconds.
    pub(super) fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 {
            0.0
        } else {
            self.frame_count() as f64 / f64::from(self.sample_rate)
        }
    }

    /// Downmixes to mono by averaging channels (a no-op for mono input).
    pub(super) fn to_mono(&self) -> Vec<f32> {
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

/// Converts a decoded PCM [`AudioFrame`](oximedia_codec::AudioFrame) into
/// normalised `f32` samples. `PcmDecoder::decode_bytes` always stores its
/// output as raw F32-LE / I16-LE / I32-LE / U8 bytes per `frame.format`, so
/// this only re-interprets bytes the decoder already produced — no PCM
/// decoding logic is duplicated here.
fn audio_frame_to_f32(frame: &oximedia_codec::AudioFrame) -> Result<Vec<f32>> {
    use oximedia_codec::SampleFormat;

    let raw = &frame.samples;
    let bad_len = |unit: &str| {
        FarmError::Task(format!(
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
/// Returns [`FarmError::NotFound`] when the file is missing, and
/// [`FarmError::Task`] when the file is not WAV, the `fmt ` chunk describes
/// a sub-format with no PCM decoder, or demuxing/decoding fails.
pub(super) async fn decode_wav(path: &str) -> Result<DecodedAudio> {
    // `oximedia_container::demux::wav::WavFormat` describes the WAV *sample*
    // sub-format (Pcm/IeeeFloat/Alaw/Mulaw/Extensible) parsed from the `fmt `
    // chunk. It intentionally shadows `oximedia_container::mux::wav::WavFormat`
    // (a *writer*-side enum with different variants: Pcm/Float/Extensible) —
    // alias it so the two are never confused.
    use oximedia_codec::{ByteOrder, PcmConfig, PcmDecoder, PcmFormat};
    use oximedia_container::demux::wav::WavFormat as WavSampleFormat;
    use oximedia_container::demux::{Demuxer, WavDemuxer};
    use oximedia_io::MemorySource;

    if !Path::new(path).exists() {
        return Err(FarmError::NotFound(format!(
            "audio input not found: {path}"
        )));
    }

    let raw = tokio::fs::read(path)
        .await
        .map_err(|e| FarmError::Task(format!("cannot read '{path}': {e}")))?;

    let scan = &raw[..raw.len().min(16)];
    if !is_wav(scan) {
        return Err(FarmError::Task(format!(
            "unsupported input format for '{path}': expected a RIFF/WAVE container (leading \
             bytes {}); analysis and fingerprint tasks currently decode WAV only",
            describe_magic(scan)
        )));
    }

    let mut demuxer = WavDemuxer::new(MemorySource::from_vec(raw));
    demuxer
        .probe()
        .await
        .map_err(|e| FarmError::Task(format!("WAV probe failed for '{path}': {e}")))?;

    let (pcm_format, byte_order, channels, sample_rate) = {
        let info = demuxer.format_info().ok_or_else(|| {
            FarmError::Task(format!("WAV '{path}' has no `fmt ` chunk after probing"))
        })?;
        let bits = info.bits_per_sample;
        let unsupported = |detail: String| {
            FarmError::Task(format!(
                "WAV '{path}' uses an unsupported sample format: {detail}"
            ))
        };

        let (format, order) = match (&info.format, bits) {
            (WavSampleFormat::Pcm, 8) => (PcmFormat::U8, ByteOrder::Little),
            (WavSampleFormat::Pcm, 16) => (PcmFormat::I16, ByteOrder::Little),
            (WavSampleFormat::Pcm, 24) => (PcmFormat::I24, ByteOrder::Little),
            (WavSampleFormat::Pcm, 32) => (PcmFormat::I32, ByteOrder::Little),
            (WavSampleFormat::IeeeFloat, 32) => (PcmFormat::F32, ByteOrder::Little),
            (WavSampleFormat::IeeeFloat, 64) => (PcmFormat::F64, ByteOrder::Little),
            (WavSampleFormat::Extensible, bps) => {
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
                let frame = decoder
                    .decode_bytes(&packet.data)
                    .map_err(|e| FarmError::Task(format!("PCM decode failed for '{path}': {e}")))?;
                samples.extend_from_slice(&audio_frame_to_f32(&frame)?);
            }
            Err(oximedia_core::OxiError::Eof) => break,
            Err(e) => {
                return Err(FarmError::Task(format!(
                    "WAV read failed for '{path}': {e}"
                )));
            }
        }
    }

    if samples.is_empty() {
        return Err(FarmError::Task(format!(
            "WAV '{path}' decoded to zero samples"
        )));
    }

    tracing::debug!(
        "Decoded {}: {} samples, {} ch @ {} Hz",
        path,
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
// Y4M / raw video frame extraction
// ---------------------------------------------------------------------------

/// Which frame a thumbnail/extraction request wants.
#[derive(Debug, Clone, Copy)]
pub(super) enum FrameSelector {
    /// 0-based frame index.
    Index(u64),
    /// Presentation timestamp in seconds, converted to a frame index via the
    /// stream's own frame rate.
    TimestampSecs(f64),
}

impl FrameSelector {
    /// Resolves to a concrete 0-based frame index given the stream's frame
    /// rate. A non-positive rate or timestamp treats [`Self::TimestampSecs`]
    /// as index 0 rather than dividing by zero or going negative.
    fn resolve(self, fps_num: u32, fps_den: u32) -> u64 {
        match self {
            Self::Index(i) => i,
            Self::TimestampSecs(t) => {
                if fps_den == 0 || fps_num == 0 || t <= 0.0 {
                    0
                } else {
                    let fps = f64::from(fps_num) / f64::from(fps_den);
                    (t * fps).round().max(0.0) as u64
                }
            }
        }
    }
}

/// One Y4M frame's raw planar pixel data plus the stream metadata needed to
/// interpret it.
#[derive(Debug, Clone)]
pub(super) struct Y4mFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Chroma subsampling mode.
    pub chroma: oximedia_container::demux::y4m::Y4mChroma,
    /// Frame rate numerator.
    pub fps_num: u32,
    /// Frame rate denominator.
    pub fps_den: u32,
    /// The resolved 0-based index of this frame within the stream.
    pub frame_index: u64,
    /// Raw planar data: Y plane, then Cb/Cr (layout per `chroma`).
    pub data: Vec<u8>,
}

impl Y4mFrame {
    /// Chroma plane dimensions for this frame's subsampling mode, or `None`
    /// for [`Y4mChroma::Mono`] (no chroma planes at all).
    pub(super) fn chroma_dims(&self) -> Option<(usize, usize)> {
        y4m_chroma_dims(self.chroma, self.width, self.height)
    }

    /// Splits [`Self::data`] into `(Y, U, V)` plane slices. `U`/`V` are
    /// `None` for [`Y4mChroma::Mono`]; for [`Y4mChroma::C444alpha`] the
    /// trailing alpha plane is present in `data` but not sliced out here (no
    /// caller currently needs it).
    pub(super) fn planes(&self) -> (&[u8], Option<&[u8]>, Option<&[u8]>) {
        let luma_len = (self.width as usize).saturating_mul(self.height as usize);
        let y = &self.data[..luma_len.min(self.data.len())];
        let Some((cw, ch)) = self.chroma_dims() else {
            return (y, None, None);
        };
        let chroma_len = cw.saturating_mul(ch);
        let u_start = luma_len;
        let v_start = luma_len.saturating_add(chroma_len);
        (
            y,
            self.data.get(u_start..u_start + chroma_len),
            self.data.get(v_start..v_start + chroma_len),
        )
    }
}

/// Chroma-plane dimensions for a Y4M subsampling mode at the given luma
/// size. Returns `None` for [`Y4mChroma::Mono`].
fn y4m_chroma_dims(
    chroma: oximedia_container::demux::y4m::Y4mChroma,
    width: u32,
    height: u32,
) -> Option<(usize, usize)> {
    use oximedia_container::demux::y4m::Y4mChroma;

    let w = width as usize;
    let h = height as usize;
    match chroma {
        Y4mChroma::C420jpeg | Y4mChroma::C420mpeg2 | Y4mChroma::C420paldv => {
            Some((w.div_ceil(2), h.div_ceil(2)))
        }
        Y4mChroma::C422 => Some((w.div_ceil(2), h)),
        Y4mChroma::C444 | Y4mChroma::C444alpha => Some((w, h)),
        Y4mChroma::Mono => None,
    }
}

/// Extracts one raw frame from a Y4M file, running the blocking demux on a
/// Tokio blocking thread so the async executor is never stalled by file I/O
/// or the frame-skip loop.
///
/// # Errors
///
/// Returns [`FarmError::NotFound`] when the file is missing, or
/// [`FarmError::Task`] when the file is not Y4M, the header is malformed, a
/// frame is truncated, or the stream ends before reaching the selected
/// frame.
pub(super) async fn extract_y4m_frame(path: &str, selector: FrameSelector) -> Result<Y4mFrame> {
    if !Path::new(path).exists() {
        return Err(FarmError::NotFound(format!(
            "video input not found: {path}"
        )));
    }
    let magic = read_magic(path).await?;
    if !is_y4m(&magic) {
        return Err(FarmError::Task(format!(
            "unsupported input format for '{path}': expected a YUV4MPEG2 (Y4M) stream (leading \
             bytes {}); this worker has no CLI-independent compressed-video decoder for inter \
             frames yet, so thumbnail extraction only supports raw Y4M input",
            describe_magic(&magic)
        )));
    }

    let owned = path.to_string();
    tokio::task::spawn_blocking(move || extract_y4m_frame_blocking(&owned, selector))
        .await
        .map_err(|e| FarmError::Task(format!("Y4M decode task join error: {e}")))?
}

/// Blocking half of [`extract_y4m_frame`].
fn extract_y4m_frame_blocking(path: &str, selector: FrameSelector) -> Result<Y4mFrame> {
    use oximedia_container::demux::Y4mDemuxer;

    let file = std::fs::File::open(path)
        .map_err(|e| FarmError::Task(format!("cannot open '{path}': {e}")))?;
    let mut demuxer = Y4mDemuxer::new(file)
        .map_err(|e| FarmError::Task(format!("Y4M header invalid in '{path}': {e}")))?;

    let width = demuxer.width();
    let height = demuxer.height();
    if width == 0 || height == 0 {
        return Err(FarmError::Task(format!(
            "Y4M '{path}' declares a zero-sized frame ({width}x{height})"
        )));
    }
    let chroma = demuxer.chroma();
    let (fps_num, fps_den) = demuxer.fps();
    let frame_index = selector.resolve(fps_num, fps_den);

    let mut seen = 0u64;
    loop {
        let raw = demuxer.read_frame().map_err(|e| {
            FarmError::Task(format!("Y4M frame {seen} unreadable in '{path}': {e}"))
        })?;
        let Some(raw) = raw else {
            return Err(FarmError::Task(format!(
                "Y4M '{path}' ended after {seen} frame(s) while seeking to frame index \
                 {frame_index}"
            )));
        };
        if seen == frame_index {
            return Ok(Y4mFrame {
                width,
                height,
                chroma,
                fps_num,
                fps_den,
                frame_index,
                data: raw,
            });
        }
        seen += 1;
    }
}

// ---------------------------------------------------------------------------
// Pixel-plane scaling
// ---------------------------------------------------------------------------

/// Downscales (or copies, when `dst == src`) an 8-bit single-channel plane
/// with an area-averaging box filter.
///
/// `src` must contain at least `src_w * src_h` bytes; any excess (e.g. a
/// caller passing a whole multi-plane buffer) is ignored. Each destination
/// pixel is the mean of the axis-aligned source region it covers — computed
/// with plain integer division of the source into `dst_w`/`dst_h` bands, so
/// bands are equal-sized only when dimensions divide evenly, matching how
/// simple area-average resizers commonly behave for non-integer ratios.
///
/// Returns an empty `Vec` if any dimension is zero or `src` is too short.
pub(super) fn box_downscale_plane(
    src: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 || src.len() < src_w * src_h {
        return Vec::new();
    }

    let mut dst = vec![0u8; dst_w * dst_h];
    for dy in 0..dst_h {
        let y0 = dy * src_h / dst_h;
        let y1 = ((dy + 1) * src_h / dst_h).max(y0 + 1).min(src_h);
        for dx in 0..dst_w {
            let x0 = dx * src_w / dst_w;
            let x1 = ((dx + 1) * src_w / dst_w).max(x0 + 1).min(src_w);

            let mut sum: u64 = 0;
            let mut count: u64 = 0;
            for y in y0..y1 {
                let row_start = y * src_w;
                for &px in &src[row_start + x0..row_start + x1] {
                    sum += u64::from(px);
                    count += 1;
                }
            }
            // `sum` accumulates `count` values each in `0..=255`, so
            // `sum / count` (when `count > 0`) is itself always in
            // `0..=255` — the `as u8` truncation is exact, never lossy.
            #[allow(clippy::cast_possible_truncation)]
            let mean = sum.checked_div(count).map_or(0, |v| v as u8);
            dst[dy * dst_w + dx] = mean;
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("oximedia_farm_media_{}_{name}", std::process::id()))
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

    fn sine_i16(freq: f64, sample_rate: u32, duration_secs: f64) -> Vec<i16> {
        let n = (f64::from(sample_rate) * duration_secs) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(sample_rate);
                ((t * freq * std::f64::consts::TAU).sin() * 20_000.0) as i16
            })
            .collect()
    }

    #[test]
    fn qc_probeable_recognises_known_containers() {
        assert_eq!(qc_probeable_container(b"RIFF\0\0\0\0WAVEfmt "), Some("WAV"));
        assert_eq!(qc_probeable_container(b"RIFF\0\0\0\0AVI LIST"), Some("AVI"));
        assert_eq!(
            qc_probeable_container(&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0]),
            Some("Matroska/WebM")
        );
        assert_eq!(
            qc_probeable_container(b"\0\0\0\x20ftypisom"),
            Some("ISO-BMFF (MP4/MOV)")
        );
        assert_eq!(qc_probeable_container(b"fLaC\0\0\0\0"), Some("FLAC"));
        assert_eq!(qc_probeable_container(b"OggS\0\0\0\0"), Some("Ogg"));
        assert_eq!(qc_probeable_container(b"YUV4MPEG2 W16"), None);
    }

    #[test]
    fn is_wav_detects_riff_wave_only() {
        assert!(is_wav(b"RIFF\0\0\0\0WAVEfmt "));
        assert!(!is_wav(b"RIFF\0\0\0\0AVI LIST"));
        assert!(!is_wav(b"YUV4MPEG2 W16 H16"));
        assert!(!is_wav(b"short"));
    }

    #[tokio::test]
    async fn assert_qc_probeable_accepts_wav_and_rejects_unknown() {
        let wav = temp_path("probeable.wav");
        std::fs::write(&wav, wav_bytes(&[0, 1, -1, 2], 8_000, 1)).expect("write wav");
        assert_eq!(
            assert_qc_probeable(&wav.to_string_lossy())
                .await
                .expect("wav is probeable"),
            "WAV"
        );

        let unknown = temp_path("probeable.bin");
        std::fs::write(&unknown, b"not-a-known-container-at-all").expect("write bin");
        let err = assert_qc_probeable(&unknown.to_string_lossy())
            .await
            .expect_err("unknown container must be rejected");
        assert!(err.to_string().contains("synthesise"));

        let _ = std::fs::remove_file(&wav);
        let _ = std::fs::remove_file(&unknown);
    }

    #[tokio::test]
    async fn assert_qc_probeable_missing_file_is_not_found() {
        let path = temp_path("absent.wav");
        let _ = std::fs::remove_file(&path);
        let err = assert_qc_probeable(&path.to_string_lossy())
            .await
            .expect_err("missing file");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn decode_wav_produces_normalised_samples() {
        let path = temp_path("decode.wav");
        let samples = sine_i16(440.0, 16_000, 0.25);
        std::fs::write(&path, wav_bytes(&samples, 16_000, 1)).expect("write wav");

        let audio = decode_wav(&path.to_string_lossy()).await.expect("decode");
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.sample_rate, 16_000);
        assert_eq!(audio.samples.len(), samples.len());
        assert!(audio.samples.iter().all(|s| (-1.0..=1.0).contains(s)));
        assert!((audio.duration_secs() - 0.25).abs() < 1e-6);

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn decode_wav_downmixes_stereo_to_mono() {
        let path = temp_path("stereo.wav");
        // Interleaved L/R: constant +1.0 left, -1.0 right -> mono average 0.0.
        let samples: Vec<i16> = (0..2_000).flat_map(|_| [32_767_i16, -32_768_i16]).collect();
        std::fs::write(&path, wav_bytes(&samples, 8_000, 2)).expect("write wav");

        let audio = decode_wav(&path.to_string_lossy()).await.expect("decode");
        assert_eq!(audio.channels, 2);
        let mono = audio.to_mono();
        assert_eq!(mono.len(), 2_000);
        assert!(mono.iter().all(|s| s.abs() < 0.01));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn decode_wav_rejects_non_wav() {
        let path = temp_path("notawav.bin");
        std::fs::write(&path, b"YUV4MPEG2 W16 H16 F25:1 Ip A1:1 C420jpeg\n").expect("write");
        let err = decode_wav(&path.to_string_lossy())
            .await
            .expect_err("must reject non-WAV");
        assert!(err.to_string().contains("RIFF/WAVE"));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn decode_wav_missing_file_is_not_found() {
        let path = temp_path("absent_decode.wav");
        let _ = std::fs::remove_file(&path);
        let err = decode_wav(&path.to_string_lossy())
            .await
            .expect_err("missing file");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    // -----------------------------------------------------------------
    // Y4M frame extraction / media-kind detection
    // -----------------------------------------------------------------

    /// Builds a Y4M fixture whose Y-plane and (if present) chroma-plane
    /// bytes vary by frame index (`t`), so tests can prove *which* frame was
    /// actually read back.
    fn y4m_bytes(width: usize, height: usize, frames: usize, chroma: &str) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(
            format!("YUV4MPEG2 W{width} H{height} F25:1 Ip A1:1 C{chroma}\n").as_bytes(),
        );
        let (cw, ch) = match chroma {
            "420jpeg" | "420mpeg2" | "420paldv" => (width.div_ceil(2), height.div_ceil(2)),
            "422" => (width.div_ceil(2), height),
            "444" => (width, height),
            _ => (0, 0),
        };
        for t in 0..frames {
            buf.extend_from_slice(b"FRAME\n");
            for y in 0..height {
                for x in 0..width {
                    buf.push(((x + y * 3 + t * 17) % 256) as u8);
                }
            }
            if cw > 0 && ch > 0 {
                for plane in 0..2 {
                    for cy in 0..ch {
                        for cx in 0..cw {
                            buf.push(((cx * 3 + cy * 5 + t * 11 + plane * 40) % 256) as u8);
                        }
                    }
                }
            }
        }
        buf
    }

    #[tokio::test]
    async fn detect_media_kind_recognises_wav_and_y4m_and_rejects_other() {
        let wav = temp_path("kind.wav");
        std::fs::write(&wav, wav_bytes(&[0, 1, -1, 2], 8_000, 1)).expect("write wav");
        assert_eq!(
            detect_media_kind(&wav.to_string_lossy())
                .await
                .expect("wav"),
            MediaKind::Wav
        );

        let y4m = temp_path("kind.y4m");
        std::fs::write(&y4m, y4m_bytes(8, 8, 1, "420jpeg")).expect("write y4m");
        assert_eq!(
            detect_media_kind(&y4m.to_string_lossy())
                .await
                .expect("y4m"),
            MediaKind::Y4m
        );

        let other = temp_path("kind.bin");
        std::fs::write(&other, b"not-a-known-container-at-all").expect("write bin");
        let err = detect_media_kind(&other.to_string_lossy())
            .await
            .expect_err("unknown container must be rejected");
        assert!(err.to_string().contains("WAV and Y4M"), "{err}");

        for p in [wav, y4m, other] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[tokio::test]
    async fn detect_media_kind_missing_file_is_not_found() {
        let path = temp_path("kind_absent.wav");
        let _ = std::fs::remove_file(&path);
        let err = detect_media_kind(&path.to_string_lossy())
            .await
            .expect_err("missing file");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    #[test]
    fn y4m_chroma_dims_matches_known_subsampling() {
        use oximedia_container::demux::y4m::Y4mChroma;

        assert_eq!(y4m_chroma_dims(Y4mChroma::C420jpeg, 16, 10), Some((8, 5)));
        assert_eq!(y4m_chroma_dims(Y4mChroma::C420jpeg, 15, 9), Some((8, 5)));
        assert_eq!(y4m_chroma_dims(Y4mChroma::C422, 16, 10), Some((8, 10)));
        assert_eq!(y4m_chroma_dims(Y4mChroma::C444, 16, 10), Some((16, 10)));
        assert_eq!(y4m_chroma_dims(Y4mChroma::Mono, 16, 10), None);
    }

    #[test]
    fn frame_selector_resolves_index_and_timestamp() {
        assert_eq!(FrameSelector::Index(7).resolve(25, 1), 7);
        // 2.0s @ 25fps -> frame 50.
        assert_eq!(FrameSelector::TimestampSecs(2.0).resolve(25, 1), 50);
        // Degenerate rates fall back to frame 0 rather than dividing by zero.
        assert_eq!(FrameSelector::TimestampSecs(2.0).resolve(0, 1), 0);
        assert_eq!(FrameSelector::TimestampSecs(-1.0).resolve(25, 1), 0);
    }

    #[tokio::test]
    async fn extract_y4m_frame_reads_the_requested_index() {
        let path = temp_path("extract.y4m");
        std::fs::write(&path, y4m_bytes(8, 8, 4, "420jpeg")).expect("write y4m");

        let frame = extract_y4m_frame(&path.to_string_lossy(), FrameSelector::Index(2))
            .await
            .expect("frame 2 must be readable");
        assert_eq!(frame.width, 8);
        assert_eq!(frame.height, 8);
        assert_eq!(frame.frame_index, 2);

        let frame0 = extract_y4m_frame(&path.to_string_lossy(), FrameSelector::Index(0))
            .await
            .expect("frame 0 must be readable");
        assert_ne!(
            frame.data, frame0.data,
            "distinct frame indices must yield distinct content"
        );

        let (y, u, v) = frame.planes();
        assert_eq!(y.len(), 64);
        assert_eq!(u.map(<[u8]>::len), Some(16));
        assert_eq!(v.map(<[u8]>::len), Some(16));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn extract_y4m_frame_missing_file_is_not_found() {
        let path = temp_path("extract_absent.y4m");
        let _ = std::fs::remove_file(&path);
        let err = extract_y4m_frame(&path.to_string_lossy(), FrameSelector::Index(0))
            .await
            .expect_err("missing file");
        assert!(matches!(err, FarmError::NotFound(_)), "{err}");
    }

    #[tokio::test]
    async fn extract_y4m_frame_rejects_non_y4m() {
        let path = temp_path("extract_notay4m.wav");
        std::fs::write(&path, wav_bytes(&[0, 1, -1, 2], 8_000, 1)).expect("write wav");
        let err = extract_y4m_frame(&path.to_string_lossy(), FrameSelector::Index(0))
            .await
            .expect_err("must reject non-Y4M");
        assert!(err.to_string().contains("YUV4MPEG2"), "{err}");
        assert!(
            err.to_string()
                .contains("no CLI-independent compressed-video decode"),
            "{err}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn extract_y4m_frame_out_of_range_is_honest_err() {
        let path = temp_path("extract_short.y4m");
        std::fs::write(&path, y4m_bytes(8, 8, 2, "420jpeg")).expect("write y4m");
        let err = extract_y4m_frame(&path.to_string_lossy(), FrameSelector::Index(9))
            .await
            .expect_err("index beyond stream length must fail honestly");
        assert!(err.to_string().contains("ended after 2 frame"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    // -----------------------------------------------------------------
    // Box-filter plane downscale
    // -----------------------------------------------------------------

    #[test]
    fn box_downscale_plane_identity_when_same_size() {
        let src = [10u8, 20, 30, 40, 50, 60];
        let dst = box_downscale_plane(&src, 3, 2, 3, 2);
        assert_eq!(dst, src);
    }

    #[test]
    fn box_downscale_plane_averages_uniform_regions() {
        // 4x4 split into four 2x2 quadrants of constant value 10/20/30/40.
        #[rustfmt::skip]
        let src = [
            10, 10, 20, 20,
            10, 10, 20, 20,
            30, 30, 40, 40,
            30, 30, 40, 40,
        ];
        let dst = box_downscale_plane(&src, 4, 4, 2, 2);
        assert_eq!(dst, vec![10, 20, 30, 40]);
    }

    #[test]
    fn box_downscale_plane_full_reduction_is_global_mean() {
        let src = [0u8, 10, 20, 30, 40, 50, 60, 70, 80, 90];
        let dst = box_downscale_plane(&src, 10, 1, 1, 1);
        let expected_mean = (0..10).map(|i| i * 10).sum::<u32>() / 10;
        assert_eq!(dst, vec![expected_mean as u8]);
    }

    #[test]
    fn box_downscale_plane_rejects_degenerate_dimensions() {
        let src = [1u8, 2, 3, 4];
        assert!(box_downscale_plane(&src, 0, 2, 1, 1).is_empty());
        assert!(box_downscale_plane(&src, 2, 2, 0, 1).is_empty());
        assert!(
            box_downscale_plane(&src, 4, 4, 2, 2).is_empty(),
            "src too short for 4x4"
        );
    }

    #[test]
    fn box_downscale_plane_non_integer_ratio_stays_in_bounds() {
        // 5 -> 3: bands are not equal-sized, but every output byte must
        // still be a plausible average of in-range source bytes.
        let src: Vec<u8> = (0..25).map(|i| (i * 10) as u8).collect();
        let dst = box_downscale_plane(&src, 5, 5, 3, 3);
        assert_eq!(dst.len(), 9);
    }
}
