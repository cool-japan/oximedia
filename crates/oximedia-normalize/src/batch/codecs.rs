//! Format sniffing and real per-format decode/encode helpers for [`super::BatchProcessor`].
//!
//! Kept separate from `batch.rs` so the orchestration logic in the parent module
//! (analyze → gain → process) stays readable; everything here is plain byte-level
//! codec plumbing with no loudness-domain logic of its own.
//!
//! # Supported containers
//!
//! | Container | Decode | Encode | Notes |
//! |---|---|---|---|
//! | WAV | [`super::decode_wav`] (`oximedia_audio::wav`) | [`super::encode_wav`] | Unchanged from before this module existed. |
//! | FLAC | [`decode_flac`] (`oximedia_codec::flac::FlacDecoder`) | [`encode_flac_output`] (`oximedia_codec::flac::FlacEncoder`) | RFC 9639, bit-exact vs libFLAC/ffmpeg per `oximedia-codec`'s own docs. |
//! | Opus (Ogg) | honest [`NormalizeError::UnsupportedFormat`] | — | The in-tree Opus decoder was empirically found to decode real CELT packets to silence this session; refusing beats fabricating a measurement of silence. |
//! | MP3 | honest [`NormalizeError::UnsupportedFormat`] | — | MP3 decoding is not wired into this crate's dependency graph. |
//!
//! FLAC is decoded via `oximedia_codec::flac::FlacDecoder::decode_stream` directly
//! rather than through `oximedia-container`'s `FlacDemuxer`. That demuxer's
//! `read_packet` estimates each packet's length from `STREAMINFO.max_frame_size`
//! (its own doc comment: "In a full implementation, we'd parse subframes and find
//! CRC-16" — it does not) and is `async`, requiring a Tokio runtime this otherwise
//! synchronous crate does not otherwise depend on. `FlacDecoder::decode_stream`
//! parses metadata and every frame with real CRC-16 verification and an exact
//! `bytes_consumed`, so it is both more correct and dependency-lighter here.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use oximedia_audio::wav::WavSpec;
use oximedia_codec::flac::{FlacConfig, FlacDecoder, FlacEncoder};
use oximedia_metadata::{Metadata, MetadataFormat, MetadataValue};

use crate::{NormalizeError, NormalizeResult};

/// A real, supported audio container this module can decode and re-encode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Container {
    /// RIFF/WAVE PCM or IEEE-float, via `oximedia_audio::wav`.
    Wav,
    /// Native FLAC (RFC 9639), via `oximedia_codec::flac`.
    Flac,
}

impl Container {
    /// Parse a case-insensitive `output_format` override string.
    fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("wav") {
            Some(Self::Wav)
        } else if name.eq_ignore_ascii_case("flac") {
            Some(Self::Flac)
        } else {
            None
        }
    }
}

/// Number of leading bytes read to sniff a file's container/codec.
///
/// Large enough to find the `OpusHead` packet, which sits after an Ogg page header
/// (27 bytes) plus a small segment table — 128 bytes gives comfortable margin.
const SNIFF_LEN: usize = 128;

/// Sniff `path`'s leading bytes and decode it via the appropriate real codec.
///
/// Returns interleaved `f32` samples in `[-1.0, 1.0]`, a [`WavSpec`] describing the
/// decoded channel count / sample rate / bit depth (reused as this crate's generic
/// decoded-audio descriptor regardless of the source container — [`super::process_decoded`]
/// only ever reads `channels`/`sample_rate` off it), and the [`Container`] that was
/// decoded.
///
/// # Errors
///
/// FLAC input that fails to parse/decode surfaces a real [`NormalizeError::Codec`].
/// Recognized-but-unsupported input (Opus, MP3) surfaces an honest
/// [`NormalizeError::UnsupportedFormat`] explaining why, rather than fabricating a
/// measurement of silence or garbage. Anything else (including real WAV, and
/// non-audio garbage) falls through to [`super::decode_wav`], which reports its own
/// precise error for malformed input.
pub(super) fn decode_input(path: &Path) -> NormalizeResult<(Vec<f32>, WavSpec, Container)> {
    let head = {
        let file = File::open(path)?;
        let mut buf = Vec::with_capacity(SNIFF_LEN);
        // `take` is unambiguously `Read::take` (moves `file`, unused afterward) —
        // `by_ref` would be ambiguous here since `File` also implements `Write`.
        file.take(SNIFF_LEN as u64).read_to_end(&mut buf)?;
        buf
    };

    if head.len() >= 4 && &head[..4] == b"fLaC" {
        let (samples, spec) = decode_flac(path)?;
        return Ok((samples, spec, Container::Flac));
    }

    if head.len() >= 4 && &head[..4] == b"OggS" && contains(&head, b"OpusHead") {
        return Err(NormalizeError::UnsupportedFormat(format!(
            "{}: Ogg Opus input. The in-tree Opus decoder was empirically found \
             untrustworthy this session (real CELT packets decode to silence), so batch \
             normalize refuses to measure/gain it rather than report a fabricated success \
             over silence.",
            path.display()
        )));
    }

    if is_mp3_signature(&head) {
        return Err(NormalizeError::UnsupportedFormat(format!(
            "{}: MP3 input. MP3 decoding is not wired into oximedia-normalize's batch \
             pipeline — this crate depends only on oximedia-audio's WAV codec and \
             oximedia-codec's FLAC codec for real decode.",
            path.display()
        )));
    }

    // RIFF/WAVE, or anything unrecognized (including garbage): the existing WAV path
    // owns this case and reports its own honest, specific parse error.
    let (samples, spec) = super::decode_wav(path)?;
    Ok((samples, spec, Container::Wav))
}

/// Resolve the container batch output should be written in.
///
/// `override_format` is [`super::BatchConfig::output_format`]: `None` means "same as
/// input"; `Some("wav")`/`Some("flac")` (case-insensitive) force that container.
///
/// # Errors
///
/// Returns [`NormalizeError::InvalidConfig`] for any other `override_format` value —
/// silently falling back to "same as input" for a typo'd format string would be a
/// worse surprise than a clear error.
pub(super) fn resolve_output_container(
    source: Container,
    override_format: Option<&str>,
) -> NormalizeResult<Container> {
    match override_format {
        None => Ok(source),
        Some(name) => Container::parse(name).ok_or_else(|| {
            NormalizeError::InvalidConfig(format!(
                "unsupported output_format {name:?}: batch normalize supports \"wav\" or \
                 \"flac\" (or None for \"same as input\")"
            ))
        }),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Best-effort MP3 sniff: an ID3v2 tag header, or an MPEG audio frame sync (11 set
/// high bits: `0xFF` followed by a byte whose top 3 bits are all set).
fn is_mp3_signature(head: &[u8]) -> bool {
    if head.len() >= 3 && &head[..3] == b"ID3" {
        return true;
    }
    head.len() >= 2 && head[0] == 0xFF && (head[1] & 0xE0) == 0xE0
}

// =============================================================================
// FLAC
// =============================================================================

/// Decode a FLAC file's samples (interleaved `f32` in `[-1.0, 1.0]`) and a [`WavSpec`]
/// describing its channel/rate/bit-depth layout.
fn decode_flac(path: &Path) -> NormalizeResult<(Vec<f32>, WavSpec)> {
    let data = std::fs::read(path)?;
    let mut decoder = FlacDecoder::new();
    let pcm = decoder.decode_stream(&data)?;
    let info = decoder.stream_info().ok_or_else(|| {
        NormalizeError::ProcessingError(format!(
            "{}: FLAC decode produced no STREAMINFO",
            path.display()
        ))
    })?;

    let spec = WavSpec {
        channels: u16::from(info.channels),
        sample_rate: info.sample_rate,
        bits_per_sample: u16::from(info.bits_per_sample),
        float: false,
    };
    Ok((pcm_i32_to_f32(&pcm, info.bits_per_sample), spec))
}

/// Re-encode interleaved `f32` samples to a real FLAC file at `path`, at `spec`'s
/// channel count / sample rate / bit depth, via [`FlacEncoder`] (RFC 9639, bit-exact
/// vs libFLAC/ffmpeg per `oximedia-codec`'s own module docs).
///
/// When `vorbis_tags` is `Some`, a VORBIS_COMMENT metadata block carrying those tags
/// is inserted between STREAMINFO and the audio frames: STREAMINFO's
/// last-metadata-block flag is cleared and moved onto the new block — the same
/// block-insertion shape `oximedia_codec::flac::decoder`'s own
/// `test_parse_metadata_with_vorbis_comment_block` test exercises. When `None`, the
/// stream is unchanged from before `write_metadata` support existed: STREAMINFO
/// followed directly by frames.
///
/// # Errors
///
/// Returns [`NormalizeError::InvalidConfig`] if `spec` is outside FLAC's supported
/// range (1..=8 channels, 4..=32 bits per sample), or [`NormalizeError::Codec`] /
/// [`NormalizeError::MetadataError`] if encoding fails.
pub(super) fn encode_flac_output(
    path: &Path,
    samples: &[f32],
    spec: WavSpec,
    vorbis_tags: Option<&[(String, String)]>,
) -> NormalizeResult<()> {
    if !(1..=8).contains(&spec.channels) {
        return Err(NormalizeError::InvalidConfig(format!(
            "cannot re-encode to FLAC: {} channel(s), FLAC supports 1..=8",
            spec.channels
        )));
    }
    if !(4..=32).contains(&spec.bits_per_sample) {
        return Err(NormalizeError::InvalidConfig(format!(
            "cannot re-encode to FLAC: {}-bit samples, FLAC supports 4..=32 bits per sample",
            spec.bits_per_sample
        )));
    }

    let config = FlacConfig {
        sample_rate: spec.sample_rate,
        channels: spec.channels as u8,
        bits_per_sample: spec.bits_per_sample as u8,
    };
    let pcm = pcm_f32_to_i32(samples, spec.bits_per_sample);

    let mut encoder = FlacEncoder::new(config);
    let (_provisional_header, frames) = encoder.encode(&pcm)?;
    // Discard the provisional header `encode()` returned and use the finalized one
    // (real total sample count, frame sizes, MD5) now that the whole buffer — the
    // complete file, batch normalize never streams FLAC in chunks — has been
    // encoded in the single call above.
    let mut stream = encoder.finalized_stream_header();

    if let Some(tags) = vorbis_tags {
        let body = encode_vorbis_comment_body(tags)?;
        // FLAC metadata block lengths are 24-bit; loudness tag lists are always a
        // handful of short strings and will never come close, but truncating the
        // length field instead of erroring would silently desync the frame stream
        // that follows, so refuse rather than emit a corrupt file.
        if body.len() > 0x00FF_FFFF {
            return Err(NormalizeError::MetadataError(format!(
                "VORBIS_COMMENT body ({} bytes) exceeds FLAC's 24-bit metadata block length",
                body.len()
            )));
        }
        stream[4] &= 0x7F; // clear STREAMINFO's last-metadata-block flag
        stream.push(0x84); // last-metadata-block flag | block type 4 (VORBIS_COMMENT)
        let len = body.len() as u32;
        stream.extend_from_slice(&len.to_be_bytes()[1..]); // 24-bit big-endian length
        stream.extend_from_slice(&body);
    }

    for frame in &frames {
        stream.extend_from_slice(&frame.data);
    }

    std::fs::write(path, &stream)?;
    Ok(())
}

/// Build a VORBIS_COMMENT metadata block *body* (vendor string + tag list) from
/// `tags`, via `oximedia_metadata::vorbis::write` — the same wire format FLAC, Ogg
/// Vorbis and Ogg Opus all share.
fn encode_vorbis_comment_body(tags: &[(String, String)]) -> NormalizeResult<Vec<u8>> {
    let mut metadata = Metadata::new(MetadataFormat::VorbisComments);
    for (key, value) in tags {
        metadata.insert(key.clone(), MetadataValue::Text(value.clone()));
    }
    oximedia_metadata::vorbis::write(&metadata)
        .map_err(|e| NormalizeError::MetadataError(format!("VORBIS_COMMENT encode: {e}")))
}

/// Convert interleaved signed PCM at `bits_per_sample` to interleaved `f32` in
/// `[-1.0, 1.0]`, dividing by `2^(bits-1)` — the same asymmetric int↔float
/// convention `oximedia_audio::wav` uses (encode multiplies by `2^(bits-1) - 1`,
/// decode divides by `2^(bits-1)`), matched here so round-trip behavior is
/// consistent across the workspace regardless of which codec produced the samples.
fn pcm_i32_to_f32(pcm: &[i32], bits_per_sample: u8) -> Vec<f32> {
    let scale = 2f64.powi(i32::from(bits_per_sample.max(1)) - 1);
    pcm.iter().map(|&s| (f64::from(s) / scale) as f32).collect()
}

/// Convert interleaved `f32` samples in `[-1.0, 1.0]` to interleaved signed PCM at
/// `bits_per_sample`, clamping first: a limiter/DRC overshoot fed in at slightly
/// above unity would otherwise scale to a value [`FlacEncoder::encode`] rejects
/// outright (it validates every sample against the declared bit depth).
fn pcm_f32_to_i32(samples: &[f32], bits_per_sample: u16) -> Vec<i32> {
    let max_positive = 2f64.powi(i32::from(bits_per_sample.max(1)) - 1) - 1.0;
    samples
        .iter()
        .map(|&s| (f64::from(s.clamp(-1.0, 1.0)) * max_positive).round() as i32)
        .collect()
}

// =============================================================================
// WAV — BWF `bext` (Broadcast Audio Extension) loudness chunk
// =============================================================================

/// Real measured loudness fields to embed in a BWF `bext` chunk.
pub(super) struct LoudnessFields {
    /// Integrated loudness, LUFS.
    pub integrated_lufs: f64,
    /// Loudness range, LU.
    pub loudness_range: f64,
    /// Maximum true peak, dBTP.
    pub true_peak_dbtp: f64,
    /// Highest momentary loudness, LUFS.
    pub max_momentary_lufs: f64,
    /// Highest short-term loudness, LUFS.
    pub max_short_term_lufs: f64,
}

/// BWF `bext` fixed-size body length in bytes, per EBU – Tech 3285 v2 (Geneva, May
/// 2011) §2.3: `Description[256] + Originator[32] + OriginatorReference[32] +
/// OriginationDate[10] + OriginationTime[8] + TimeReferenceLow[4] +
/// TimeReferenceHigh[4] + Version[2] + UMID[64] + LoudnessValue[2] +
/// LoudnessRange[2] + MaxTruePeakLevel[2] + MaxMomentaryLoudness[2] +
/// MaxShortTermLoudness[2] + Reserved[180]`, before the variable-length
/// `CodingHistory` tail.
const BEXT_FIXED_LEN: usize = 602;

/// Append a real BWF `bext` chunk to an already-finalized WAV file at `path`,
/// carrying `loudness` in the chunk's Version-2 loudness fields (EBU – Tech 3285 v2,
/// which brings BWF in line with EBU R128), and patch the top-level RIFF size field
/// to include it.
///
/// `oximedia_audio::wav::WavWriter` has no facility to emit extra chunks (only
/// `RIFF/WAVE + fmt + data`) and this crate does not modify `oximedia-audio`, so this
/// performs the append as a second pass over the file `WavWriter::finalize` already
/// wrote: append the `bext` chunk after `data`, then repatch the RIFF size.
///
/// This places `bext` after `data` rather than the conventional position immediately
/// following the RIFF/WAVE header (before `fmt `). RIFF chunks are order-independent
/// for any full/random-access reader — only `fmt` must precede `data`, and that is
/// unaffected here — so this remains a well-formed WAV/BWF file;
/// [`oximedia_audio::wav::WavReader`] (used as the round-trip check in this module's
/// tests) walks every chunk by id regardless of position.
///
/// # Errors
///
/// Returns [`NormalizeError::IoError`] if `path` cannot be reopened for read+write.
pub(super) fn write_wav_bext(
    path: &Path,
    spec: WavSpec,
    loudness: &LoudnessFields,
) -> NormalizeResult<()> {
    let body = build_bext_body(spec, loudness);

    let mut file = OpenOptions::new().read(true).write(true).open(path)?;

    let riff_size = {
        let mut bytes = [0u8; 4];
        file.seek(SeekFrom::Start(4))?;
        file.read_exact(&mut bytes)?;
        u32::from_le_bytes(bytes)
    };

    file.seek(SeekFrom::End(0))?;
    file.write_all(b"bext")?;
    file.write_all(&(body.len() as u32).to_le_bytes())?;
    file.write_all(&body)?;
    // RIFF chunks are word-aligned: pad with one zero byte if the body is odd-length.
    // The pad byte is NOT counted in the chunk's own declared length (above), only
    // in the total bytes appended / the RIFF size below — exactly what
    // `oximedia_audio::wav::WavReader`'s `padded_size = chunk_size + (chunk_size &
    // 1)` expects when it walks chunks back.
    let pad: u32 = u32::from(body.len() % 2 == 1);
    if pad == 1 {
        file.write_all(&[0u8])?;
    }

    let new_riff_size = riff_size
        .saturating_add(8) // "bext" id + 4-byte size field
        .saturating_add(body.len() as u32)
        .saturating_add(pad);
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&new_riff_size.to_le_bytes())?;
    file.flush()?;
    Ok(())
}

fn build_bext_body(spec: WavSpec, loudness: &LoudnessFields) -> Vec<u8> {
    let mut body = Vec::with_capacity(BEXT_FIXED_LEN + 64);

    push_fixed_ascii(&mut body, b"OxiMedia batch normalize", 256); // Description
    push_fixed_ascii(&mut body, b"OxiMedia", 32); // Originator
    push_fixed_ascii(&mut body, b"", 32); // OriginatorReference (unknown)
                                          // OriginationDate/Time (unknown): left empty (all-NULL) rather than a
                                          // zero-value date/time string. "0000-00-00"/"00:00:00" would parse as a
                                          // 10/8-byte ASCII field but describes a month=0/day=0 date the spec defines
                                          // as out of range (Month 1..=12, Day 1..=31) — strict BWF validators flag
                                          // that. Empty matches the "absent" convention already used for
                                          // OriginatorReference/UMID above.
    push_fixed_ascii(&mut body, b"", 10); // OriginationDate (unknown)
    push_fixed_ascii(&mut body, b"", 8); // OriginationTime (unknown)
    body.extend_from_slice(&0u32.to_le_bytes()); // TimeReferenceLow (unknown)
    body.extend_from_slice(&0u32.to_le_bytes()); // TimeReferenceHigh (unknown)
    body.extend_from_slice(&2u16.to_le_bytes()); // Version 2: loudness fields present
    body.extend_from_slice(&[0u8; 64]); // UMID (none — an all-zero UMID is defined as "absent")

    for value in [
        loudness.integrated_lufs,
        loudness.loudness_range,
        loudness.true_peak_dbtp,
        loudness.max_momentary_lufs,
        loudness.max_short_term_lufs,
    ] {
        body.extend_from_slice(&bext_loudness_word(value).to_le_bytes());
    }

    body.extend_from_slice(&[0u8; 180]); // Reserved (must be NULL per spec when Version=2)

    debug_assert_eq!(
        body.len(),
        BEXT_FIXED_LEN,
        "bext fixed-size body must be exactly 602 bytes per EBU Tech 3285 v2"
    );

    let channel_mode = match spec.channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n}-channel"),
    };
    body.extend_from_slice(
        format!(
            "A=PCM,F={},W={},M={channel_mode},T=OxiMedia batch normalize\r\n",
            spec.sample_rate, spec.bits_per_sample,
        )
        .as_bytes(),
    ); // CodingHistory

    body
}

/// Push a fixed-width ASCII field: `text` truncated to at most `width` bytes,
/// zero-padded to exactly `width` bytes — the encoding EBU Tech 3285 specifies for
/// `Description`/`Originator`/`OriginatorReference`/`OriginationDate`/`OriginationTime`
/// (null-terminated when shorter than the field width).
fn push_fixed_ascii(out: &mut Vec<u8>, text: &[u8], width: usize) {
    let n = text.len().min(width);
    out.extend_from_slice(&text[..n]);
    out.resize(out.len() + (width - n), 0);
}

/// Encode a loudness value (already in its natural unit — LUFS, LU or dBTP) as a BWF
/// `bext` loudness `WORD` field: a 16-bit signed integer equal to
/// `round_half_away_from_zero(100 * value)` (EBU Tech 3285 v2 §2.4 — `f64::round`
/// already rounds half-away-from-zero, matching the spec's rounding rule exactly).
///
/// Returns the "not present" sentinel `0x7FFF` for non-finite input or a magnitude
/// over 99.99 (outside the field's defined `-99.99..=99.99` range) — the spec's own
/// documented behavior for values a reader "shall... ignore".
fn bext_loudness_word(value: f64) -> u16 {
    const NOT_PRESENT: i16 = 0x7FFF;
    if !value.is_finite() {
        return NOT_PRESENT as u16;
    }
    let scaled = (value * 100.0).round();
    if !(-9999.0..=9999.0).contains(&scaled) {
        return NOT_PRESENT as u16;
    }
    (scaled as i16) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── bext byte-level encoding ────────────────────────────────────────────

    #[test]
    fn test_bext_fixed_body_is_602_bytes_before_coding_history() {
        let spec = WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            float: false,
        };
        let loudness = LoudnessFields {
            integrated_lufs: -23.0,
            loudness_range: 5.0,
            true_peak_dbtp: -1.5,
            max_momentary_lufs: -18.0,
            max_short_term_lufs: -20.0,
        };
        let body = build_bext_body(spec, &loudness);
        assert!(
            body.len() > BEXT_FIXED_LEN,
            "body ({}) must be longer than the fixed part ({BEXT_FIXED_LEN}) once \
             CodingHistory is appended",
            body.len()
        );
        // Reserved[180] ends exactly at BEXT_FIXED_LEN; CodingHistory starts there.
        let coding_history = std::str::from_utf8(&body[BEXT_FIXED_LEN..]).expect("ascii");
        assert!(coding_history.starts_with("A=PCM,F=48000,W=16,M=stereo,T="));
        assert!(coding_history.ends_with("\r\n"));
    }

    #[test]
    fn test_bext_loudness_word_round_trip_examples_from_spec() {
        // Directly from EBU Tech 3285 v2 §2.4's worked examples.
        assert_eq!(bext_loudness_word(-22.644), (-2264i16) as u16);
        assert_eq!(bext_loudness_word(-22.645), (-2265i16) as u16);
        assert_eq!(bext_loudness_word(12.764), 1276);
        assert_eq!(bext_loudness_word(12.765), 1277);
    }

    #[test]
    fn test_bext_loudness_word_not_present_sentinel() {
        assert_eq!(bext_loudness_word(f64::NEG_INFINITY), 0x7FFF);
        assert_eq!(bext_loudness_word(f64::NAN), 0x7FFF);
        assert_eq!(bext_loudness_word(200.0), 0x7FFF, "out of ±99.99 range");
        assert_eq!(bext_loudness_word(-200.0), 0x7FFF, "out of ±99.99 range");
    }

    #[test]
    fn test_push_fixed_ascii_truncates_and_pads() {
        let mut out = Vec::new();
        push_fixed_ascii(&mut out, b"hello", 8);
        assert_eq!(out, b"hello\0\0\0");

        let mut out2 = Vec::new();
        push_fixed_ascii(&mut out2, b"toolongfortwo", 4);
        assert_eq!(out2, b"tool");
    }

    /// End-to-end: write a real WAV via `WavWriter`, append a `bext` chunk, then
    /// re-open with the *independent* `WavReader` (not this module's own logic) and
    /// confirm (a) the audio still parses correctly, (b) `bext` is discoverable via
    /// `extra_chunks()`, (c) decoding the loudness fields back out of that raw
    /// payload recovers the original values within the format's ±0.005 quantization.
    #[test]
    fn test_write_wav_bext_round_trips_through_independent_wav_reader() {
        use oximedia_audio::wav::{WavReader, WavWriter};
        use std::io::BufReader;

        let dir = std::env::temp_dir().join("oximedia_batch_codecs_bext_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("bext.wav");

        let spec = WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            float: false,
        };
        let samples: Vec<f32> = (0..2000).map(|i| 0.2 * (i as f32 * 0.01).sin()).collect();
        {
            let file = File::create(&path).expect("create wav");
            let mut writer = WavWriter::new(std::io::BufWriter::new(file), spec);
            writer.write_samples_f32(&samples).expect("write samples");
            writer.finalize().expect("finalize wav");
        }

        let loudness = LoudnessFields {
            integrated_lufs: -23.05,
            loudness_range: 4.32,
            true_peak_dbtp: -1.23,
            max_momentary_lufs: -19.87,
            max_short_term_lufs: -21.01,
        };
        write_wav_bext(&path, spec, &loudness).expect("write_wav_bext should succeed");

        // (a) Audio must still parse correctly, sample-for-sample, after the
        // appended chunk and repatched RIFF size.
        let file = File::open(&path).expect("open wav");
        let mut reader = WavReader::new(BufReader::new(file))
            .expect("an independent WavReader must still parse the file after bext is appended");
        assert_eq!(reader.spec(), spec);
        let decoded = reader.read_samples_f32().expect("decode samples");
        assert_eq!(decoded.len(), samples.len());

        // (b) `bext` must be discoverable as a generic extra chunk.
        let bext_chunk = reader
            .extra_chunks()
            .iter()
            .find(|c| &c.id == b"bext")
            .expect("WavReader must expose the appended bext chunk via extra_chunks()");
        assert_eq!(
            bext_chunk.data.len(),
            BEXT_FIXED_LEN + "A=PCM,F=44100,W=16,M=stereo,T=OxiMedia batch normalize\r\n".len()
        );

        // (c) Decode the loudness fields back out of the raw bext payload
        // ourselves (independent of `build_bext_body`'s own field order — this
        // indexes by the spec's byte offsets directly) and confirm round-trip.
        let read_i16 = |off: usize| -> i16 {
            i16::from_le_bytes([bext_chunk.data[off], bext_chunk.data[off + 1]])
        };
        let loudness_value_off = 256 + 32 + 32 + 10 + 8 + 4 + 4 + 2 + 64;
        let recovered_integrated = f64::from(read_i16(loudness_value_off)) / 100.0;
        let recovered_range = f64::from(read_i16(loudness_value_off + 2)) / 100.0;
        let recovered_true_peak = f64::from(read_i16(loudness_value_off + 4)) / 100.0;
        let recovered_momentary = f64::from(read_i16(loudness_value_off + 6)) / 100.0;
        let recovered_short_term = f64::from(read_i16(loudness_value_off + 8)) / 100.0;

        assert!((recovered_integrated - loudness.integrated_lufs).abs() < 0.005);
        assert!((recovered_range - loudness.loudness_range).abs() < 0.005);
        assert!((recovered_true_peak - loudness.true_peak_dbtp).abs() < 0.005);
        assert!((recovered_momentary - loudness.max_momentary_lufs).abs() < 0.005);
        assert!((recovered_short_term - loudness.max_short_term_lufs).abs() < 0.005);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Odd-length `CodingHistory` exercises the pad-byte path: a wrong pad
    /// accounting is the classic silent RIFF corruptor (declared chunk length not
    /// matching what was physically written), and `WavReader` will fail outright on
    /// a mis-padded stream since it reads `chunk_size + (chunk_size & 1)` bytes.
    #[test]
    fn test_write_wav_bext_handles_odd_length_coding_history() {
        use oximedia_audio::wav::{WavReader, WavWriter};
        use std::io::BufReader;

        let dir = std::env::temp_dir().join("oximedia_batch_codecs_bext_odd_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("bext_odd.wav");

        // 3-channel -> "3-channel" in CodingHistory, a different length than the
        // "stereo"/"mono" cases above, so between this and the other test at least
        // one exercises an odd total body length.
        let spec = WavSpec {
            channels: 3,
            sample_rate: 48_000,
            bits_per_sample: 24,
            float: false,
        };
        let samples: Vec<f32> = vec![0.1, -0.1, 0.2, 0.15, -0.05, 0.0];
        {
            let file = File::create(&path).expect("create wav");
            let mut writer = WavWriter::new(std::io::BufWriter::new(file), spec);
            writer.write_samples_f32(&samples).expect("write samples");
            writer.finalize().expect("finalize wav");
        }

        let loudness = LoudnessFields {
            integrated_lufs: -30.0,
            loudness_range: 2.0,
            true_peak_dbtp: -6.0,
            max_momentary_lufs: -25.0,
            max_short_term_lufs: -27.0,
        };
        write_wav_bext(&path, spec, &loudness).expect("write_wav_bext should succeed");

        let file = File::open(&path).expect("open wav");
        let mut reader =
            WavReader::new(BufReader::new(file)).expect("must parse regardless of pad parity");
        let decoded = reader.read_samples_f32().expect("decode samples");
        assert_eq!(decoded.len(), samples.len());
        assert!(reader.extra_chunks().iter().any(|c| &c.id == b"bext"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── FLAC decode / encode ────────────────────────────────────────────────

    /// Build a real, in-memory FLAC stream (via `FlacEncoder`) for tests, without
    /// touching any checked-in fixture files.
    fn build_flac_stream(pcm: &[i32], channels: u8, sample_rate: u32) -> Vec<u8> {
        let mut encoder = FlacEncoder::new(FlacConfig {
            sample_rate,
            channels,
            bits_per_sample: 16,
        });
        let (_header, frames) = encoder.encode(pcm).expect("encode should succeed");
        let mut stream = encoder.finalized_stream_header();
        for frame in &frames {
            stream.extend_from_slice(&frame.data);
        }
        stream
    }

    #[test]
    fn test_decode_flac_recovers_real_samples() {
        let dir = std::env::temp_dir().join("oximedia_batch_codecs_flac_decode_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("tone.flac");

        let pcm: Vec<i32> = (0..2000)
            .map(|i| ((i as f64 * 0.05).sin() * 8000.0) as i32)
            .collect();
        std::fs::write(&path, build_flac_stream(&pcm, 1, 44_100)).expect("write flac");

        let (samples, spec) = decode_flac(&path).expect("decode_flac should succeed");
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 44_100);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(samples.len(), pcm.len());
        for (&decoded, &original) in samples.iter().zip(pcm.iter()) {
            let expected = original as f32 / 32768.0;
            assert!(
                (decoded - expected).abs() < 1e-4,
                "decoded {decoded} vs expected {expected}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_encode_flac_output_round_trips_via_flac_decoder() {
        let dir = std::env::temp_dir().join("oximedia_batch_codecs_flac_encode_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("out.flac");

        let spec = WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            float: false,
        };
        let samples: Vec<f32> = (0..4000).map(|i| 0.3 * (i as f32 * 0.02).sin()).collect();

        encode_flac_output(&path, &samples, spec, None).expect("encode_flac_output");

        let data = std::fs::read(&path).expect("read encoded flac");
        let mut decoder = FlacDecoder::new();
        let pcm = decoder.decode_stream(&data).expect("decode_stream");
        let info = decoder.stream_info().expect("stream_info");
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(pcm.len(), samples.len());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_encode_flac_output_with_vorbis_tags_round_trips_via_flac_decoder() {
        let dir = std::env::temp_dir().join("oximedia_batch_codecs_flac_tags_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("tagged.flac");

        let spec = WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 16,
            float: false,
        };
        let samples: Vec<f32> = (0..1000).map(|i| 0.1 * (i as f32 * 0.03).sin()).collect();
        let tags = vec![
            ("REPLAYGAIN_TRACK_GAIN".to_string(), "-3.20 dB".to_string()),
            ("R128_TRACK_LOUDNESS".to_string(), "-23.00 LUFS".to_string()),
        ];

        encode_flac_output(&path, &samples, spec, Some(&tags)).expect("encode_flac_output");

        let data = std::fs::read(&path).expect("read encoded flac");
        let mut decoder = FlacDecoder::new();
        let pcm = decoder
            .decode_stream(&data)
            .expect("decode_stream must still succeed");
        assert_eq!(pcm.len(), samples.len());

        let comments = decoder
            .comment_block()
            .expect("VORBIS_COMMENT block must be present when tags were requested");
        assert_eq!(
            comments
                .comments
                .iter()
                .find(|(k, _)| k == "REPLAYGAIN_TRACK_GAIN")
                .map(|(_, v)| v.as_str()),
            Some("-3.20 dB")
        );
        assert_eq!(
            comments
                .comments
                .iter()
                .find(|(k, _)| k == "R128_TRACK_LOUDNESS")
                .map(|(_, v)| v.as_str()),
            Some("-23.00 LUFS")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_encode_flac_output_rejects_out_of_range_channels() {
        let spec = WavSpec {
            channels: 12,
            sample_rate: 48_000,
            bits_per_sample: 16,
            float: false,
        };
        let result = encode_flac_output(Path::new("/nonexistent/x.flac"), &[0.0; 4], spec, None);
        assert!(matches!(result, Err(NormalizeError::InvalidConfig(_))));
    }

    // ── sniffing / format resolution ────────────────────────────────────────

    #[test]
    fn test_container_parse() {
        assert_eq!(Container::parse("wav"), Some(Container::Wav));
        assert_eq!(Container::parse("WAV"), Some(Container::Wav));
        assert_eq!(Container::parse("flac"), Some(Container::Flac));
        assert_eq!(Container::parse("mp3"), None);
    }

    #[test]
    fn test_resolve_output_container_defaults_to_source() {
        assert_eq!(
            resolve_output_container(Container::Flac, None).expect("ok"),
            Container::Flac
        );
        assert_eq!(
            resolve_output_container(Container::Wav, None).expect("ok"),
            Container::Wav
        );
    }

    #[test]
    fn test_resolve_output_container_honors_override() {
        assert_eq!(
            resolve_output_container(Container::Flac, Some("wav")).expect("ok"),
            Container::Wav
        );
    }

    #[test]
    fn test_resolve_output_container_rejects_unknown_format() {
        let result = resolve_output_container(Container::Wav, Some("mp3"));
        assert!(matches!(result, Err(NormalizeError::InvalidConfig(_))));
    }

    #[test]
    fn test_is_mp3_signature() {
        assert!(is_mp3_signature(b"ID3\x03\x00\x00"));
        assert!(is_mp3_signature(&[0xFF, 0xFB, 0x90, 0x00]));
        assert!(!is_mp3_signature(b"fLaC"));
        assert!(!is_mp3_signature(b"RIFF"));
    }

    #[test]
    fn test_decode_input_errors_honestly_on_opus() {
        let dir = std::env::temp_dir().join("oximedia_batch_codecs_opus_sniff_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("clip.opus");

        // A minimal but realistic Ogg page: "OggS" + a version/flags/granule/serial/
        // seq/crc/segment-table skeleton, then the OpusHead ID header payload.
        let mut data = b"OggS".to_vec();
        data.extend_from_slice(&[0u8; 22]); // rest of the fixed Ogg page header
        data.push(1); // 1 segment
        data.push(19); // segment length
        data.extend_from_slice(b"OpusHead\x01\x02\x00\x00\x80\xbb\x00\x00\x00\x00\x00");
        std::fs::write(&path, &data).expect("write pseudo-opus file");

        let result = decode_input(&path);
        match result {
            Err(NormalizeError::UnsupportedFormat(msg)) => {
                assert!(
                    msg.contains("untrustworthy") && msg.contains("silence"),
                    "Opus error must explain the decoder is untrustworthy \
                     (CELT decodes to silence), got: {msg}"
                );
            }
            other => panic!("expected UnsupportedFormat for Opus input, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decode_input_errors_honestly_on_mp3() {
        let dir = std::env::temp_dir().join("oximedia_batch_codecs_mp3_sniff_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("clip.mp3");
        // ID3v2 header + a couple of bytes; content past the header is irrelevant,
        // decode_input must reject on the signature alone.
        let mut data = b"ID3\x03\x00\x00\x00\x00\x00\x00".to_vec();
        data.extend_from_slice(&[0u8; 32]);
        std::fs::write(&path, &data).expect("write pseudo-mp3 file");

        let result = decode_input(&path);
        match result {
            Err(NormalizeError::UnsupportedFormat(msg)) => {
                assert!(
                    msg.contains("MP3"),
                    "MP3 error message should name the format, got: {msg}"
                );
                assert!(
                    !msg.contains("no decoder in tree"),
                    "must not claim no decoder exists — oximedia-audio has one, it is \
                     simply not wired into this batch path"
                );
            }
            other => panic!("expected UnsupportedFormat for MP3 input, got {other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_decode_input_routes_flac_through_real_decoder() {
        let dir = std::env::temp_dir().join("oximedia_batch_codecs_decode_input_flac_test");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("real.flac");

        let pcm: Vec<i32> = (0..1500).map(|i| ((i % 200) - 100) * 50).collect();
        std::fs::write(&path, build_flac_stream(&pcm, 2, 48_000)).expect("write flac");

        let (samples, spec, container) = decode_input(&path).expect("decode_input should succeed");
        assert_eq!(container, Container::Flac);
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(samples.len(), pcm.len());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
