// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Real Matroska/WebM (`.mkv`/`.webm`) and Ogg (`.ogg`/`.oga`) in-container
//! **audio** decode for the frame-level transcode path.
//!
//! Both demuxers already carry real codec-id mapping for FLAC and PCM
//! (`oximedia_container::demux::matroska::parser::map_codec_id` maps
//! `A_FLAC`/`A_PCM/*`; `oximedia_container::demux::ogg` identifies FLAC by
//! its Ogg mapping magic) — the gap closed here is entirely on the
//! `oximedia-transcode` side: reconstructing a decodable byte stream from
//! each container's packet/header layout and feeding it to
//! [`crate::flac_decode::decode_flac_to_i16`] (the same spec-compliant FLAC
//! decoder the WAV/FLAC-file path already uses).
//!
//! # Matroska/WebM
//!
//! For an `A_FLAC` track, `CodecPrivate` (surfaced as
//! [`StreamInfo::codec_params::extradata`](oximedia_container::CodecParams))
//! is, by convention (and by what [`crate::raw_sinks::FlacFileMuxer`]/
//! [`crate::flac_bitstream::stream_info_block`] write), the `fLaC` magic
//! plus the STREAMINFO metadata block — i.e. exactly a bare `.flac` file's
//! header. Each packet on the track is one raw FLAC frame with no framing
//! of its own, so `extradata ++ packet_1 ++ packet_2 ++ …` reconstructs a
//! complete, decodable FLAC stream byte-for-byte.
//!
//! For an `A_PCM/*` track, packets are raw interleaved PCM with no header.
//! [`oximedia_container::CodecParams`] does not carry a bit-depth field (a
//! container-crate gap, out of this crate's scope), so this path assumes
//! 16-bit — the only PCM width this pipeline ever produces or consumes
//! elsewhere. A track actually encoded at another bit depth will decode to
//! wrong-sounding (but not out-of-bounds or panicking) audio; there is no
//! way to detect that case from the data this crate can see.
//!
//! # Ogg
//!
//! `OggDemuxer` packs every codec header packet it collected into
//! `extradata` as a `[u16 LE length][bytes]` sequence (see
//! `oximedia_container::demux::ogg::mod::update_stream_info`). For FLAC the
//! first such packet is the Ogg-FLAC mapping header: `0x7F` + `"FLAC"` +
//! mapping version + header count, followed by the normal `fLaC` magic +
//! STREAMINFO block. This module locates the `fLaC` marker inside that
//! packet (rather than assuming a fixed offset) and slices from there,
//! giving the same `fLaC`-prefixed buffer the Matroska path produces.
//!
//! Vorbis/Opus audio (and any other codec) in either container returns a
//! precise [`TranscodeError::Unsupported`] naming the codec — this crate
//! has no verified Vorbis or Opus decoder (see `audio_adapters.rs`).

use std::path::Path;

use bytes::Bytes;

use crate::frame_level::DecodedPcm;
use crate::{Result, TranscodeError};

use oximedia_container::{
    demux::{Demuxer, MatroskaDemuxer, OggDemuxer},
    ContainerFormat, StreamInfo,
};
use oximedia_core::CodecId;
use oximedia_io::FileSource;

/// The audio stream this module selected from a container, plus the parts
/// of its [`StreamInfo`] the decode step needs (kept as an owned snapshot
/// so the borrow on the demuxer's stream list can end before packets are
/// read from it).
struct PickedStream {
    index: usize,
    codec: CodecId,
    extradata: Option<Bytes>,
    sample_rate: Option<u32>,
    channels: Option<u8>,
}

/// Selects the audio stream to decode: the first FLAC or PCM stream, or
/// (for an honest error) the first audio-shaped stream, or the first
/// stream at all.
fn pick_audio_stream(streams: &[StreamInfo], container_name: &str) -> Result<PickedStream> {
    if let Some(s) = streams
        .iter()
        .find(|s| matches!(s.codec, CodecId::Flac | CodecId::Pcm))
    {
        return Ok(PickedStream {
            index: s.index,
            codec: s.codec,
            extradata: s.codec_params.extradata.clone(),
            sample_rate: s.codec_params.sample_rate,
            channels: s.codec_params.channels,
        });
    }
    // No decodable audio stream: report the most audio-shaped stream we
    // did find (sample_rate implies audio), falling back to the first
    // stream of any kind, so the error names a real codec instead of
    // guessing.
    let reported = streams
        .iter()
        .find(|s| s.codec_params.sample_rate.is_some())
        .or_else(|| streams.first());
    match reported {
        Some(s) => Err(TranscodeError::Unsupported(format!(
            "{:?} audio in {container_name} is not supported for transcode; \
             supported audio codecs from {container_name}: flac, pcm",
            s.codec
        ))),
        None => Err(TranscodeError::InvalidInput(format!(
            "no stream with a supported codec found in this {container_name} file"
        ))),
    }
}

/// Decodes a FLAC track (Matroska `A_FLAC` CodecPrivate, or an Ogg-FLAC
/// header packet already sliced to start at `fLaC`) plus its concatenated
/// raw-frame payload to interleaved i16 PCM.
fn decode_flac_track(flac_prefixed_extradata: &[u8], payload: &[u8]) -> Result<DecodedPcm> {
    let mut full = Vec::with_capacity(flac_prefixed_extradata.len() + payload.len());
    full.extend_from_slice(flac_prefixed_extradata);
    full.extend_from_slice(payload);
    let (params, samples) = crate::flac_decode::decode_flac_to_i16(&full)?;
    let mut data = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        data.extend_from_slice(&s.to_le_bytes());
    }
    Ok(DecodedPcm {
        data,
        sample_rate: params.sample_rate,
        channels: params.channels,
    })
}

/// Decodes a raw PCM track's concatenated payload to [`DecodedPcm`].
/// Assumes 16-bit interleaved LE samples — see the module doc for why.
fn decode_pcm_track(
    sample_rate: Option<u32>,
    channels: Option<u8>,
    payload: &[u8],
    container_name: &str,
) -> Result<DecodedPcm> {
    let sample_rate = sample_rate.ok_or_else(|| {
        TranscodeError::ContainerError(format!("{container_name} PCM track carries no sample rate"))
    })?;
    let channels = channels.ok_or_else(|| {
        TranscodeError::ContainerError(format!(
            "{container_name} PCM track carries no channel count"
        ))
    })?;
    if channels == 0 {
        return Err(TranscodeError::ContainerError(format!(
            "{container_name} PCM track reports zero channels"
        )));
    }
    let bytes_per_frame = usize::from(channels) * 2;
    if payload.len() % bytes_per_frame != 0 {
        return Err(TranscodeError::ContainerError(format!(
            "{container_name} PCM payload ({} bytes) is not a whole number of \
             16-bit {channels}-channel sample-frames; this path assumes 16-bit \
             PCM because the container crate does not expose bit depth",
            payload.len()
        )));
    }
    Ok(DecodedPcm {
        data: payload.to_vec(),
        sample_rate,
        channels: u16::from(channels),
    })
}

/// Extracts the first `[u16 LE length][bytes]`-packed header packet from
/// an Ogg stream's `extradata` (see `OggDemuxer::update_stream_info`).
fn ogg_first_header_packet(extradata: &[u8]) -> Result<&[u8]> {
    if extradata.len() < 2 {
        return Err(TranscodeError::ContainerError(
            "Ogg codec header data is too short to contain a packet length".into(),
        ));
    }
    let len = usize::from(u16::from_le_bytes([extradata[0], extradata[1]]));
    let start: usize = 2;
    let end = start
        .checked_add(len)
        .filter(|&e| e <= extradata.len())
        .ok_or_else(|| {
            TranscodeError::ContainerError(
                "Ogg codec header data is truncated (declared length exceeds buffer)".into(),
            )
        })?;
    Ok(&extradata[start..end])
}

/// Locates the `fLaC` marker inside an Ogg-FLAC mapping header packet and
/// returns the slice starting there (the `fLaC` magic + STREAMINFO block,
/// exactly matching the Matroska `CodecPrivate` convention).
fn slice_from_flac_marker(packet: &[u8]) -> Result<&[u8]> {
    packet
        .windows(4)
        .position(|w| w == b"fLaC")
        .map(|pos| &packet[pos..])
        .ok_or_else(|| {
            TranscodeError::ContainerError(
                "Ogg FLAC header packet does not contain the 'fLaC' stream marker; \
                 cannot reconstruct a decodable FLAC stream from it"
                    .into(),
            )
        })
}

/// Loads and fully decodes the audio track of a Matroska/WebM (`.mkv`,
/// `.webm`) file to interleaved i16 PCM.
///
/// # Errors
///
/// Returns [`TranscodeError::Unsupported`] if no FLAC/PCM audio stream is
/// present, or a container/codec error if the stream is malformed.
async fn load_matroska_audio_pcm(path: &Path) -> Result<DecodedPcm> {
    let source = FileSource::open(path)
        .await
        .map_err(|e| TranscodeError::IoError(e.to_string()))?;
    let mut demuxer = MatroskaDemuxer::new(source);
    demuxer
        .probe()
        .await
        .map_err(|e| TranscodeError::ContainerError(format!("Matroska probe failed: {e}")))?;

    let picked = pick_audio_stream(demuxer.streams(), "Matroska")?;

    let mut payload = Vec::new();
    loop {
        match demuxer.read_packet().await {
            Ok(pkt) if pkt.stream_index == picked.index => payload.extend_from_slice(&pkt.data),
            Ok(_) => {} // a different track (e.g. video); not part of this job
            Err(e) if e.is_eof() => break,
            Err(e) => {
                return Err(TranscodeError::ContainerError(format!(
                    "Matroska read failed: {e}"
                )))
            }
        }
    }

    match picked.codec {
        CodecId::Flac => {
            let extradata = picked.extradata.ok_or_else(|| {
                TranscodeError::ContainerError(
                    "Matroska FLAC track carries no CodecPrivate (STREAMINFO); cannot decode"
                        .into(),
                )
            })?;
            if extradata.len() < 4 || &extradata[0..4] != b"fLaC" {
                return Err(TranscodeError::ContainerError(format!(
                    "Matroska FLAC track's CodecPrivate ({} bytes) does not start with \
                     the 'fLaC' stream marker; cannot reconstruct a decodable FLAC \
                     stream from it",
                    extradata.len()
                )));
            }
            decode_flac_track(&extradata, &payload)
        }
        CodecId::Pcm => decode_pcm_track(picked.sample_rate, picked.channels, &payload, "Matroska"),
        other => Err(TranscodeError::Unsupported(format!(
            "{other:?} audio in Matroska is not supported for transcode; \
             supported audio codecs from Matroska: flac, pcm"
        ))),
    }
}

/// Loads and fully decodes the audio track of an Ogg (`.ogg`, `.oga`) file
/// to interleaved i16 PCM.
///
/// # Errors
///
/// Returns [`TranscodeError::Unsupported`] if no FLAC audio stream is
/// present (Vorbis/Opus are real Ogg codecs but have no verified decoder in
/// this build), or a container/codec error if the stream is malformed.
async fn load_ogg_audio_pcm(path: &Path) -> Result<DecodedPcm> {
    let source = FileSource::open(path)
        .await
        .map_err(|e| TranscodeError::IoError(e.to_string()))?;
    let mut demuxer = OggDemuxer::new(source);
    demuxer
        .probe()
        .await
        .map_err(|e| TranscodeError::ContainerError(format!("Ogg probe failed: {e}")))?;

    let picked = pick_audio_stream(demuxer.streams(), "Ogg")?;

    let mut payload = Vec::new();
    loop {
        match demuxer.read_packet().await {
            Ok(pkt) if pkt.stream_index == picked.index => payload.extend_from_slice(&pkt.data),
            Ok(_) => {}
            Err(e) if e.is_eof() => break,
            Err(e) => {
                return Err(TranscodeError::ContainerError(format!(
                    "Ogg read failed: {e}"
                )))
            }
        }
    }

    match picked.codec {
        CodecId::Flac => {
            let extradata = picked.extradata.ok_or_else(|| {
                TranscodeError::ContainerError("Ogg FLAC stream carries no header packets".into())
            })?;
            let first_header = ogg_first_header_packet(&extradata)?;
            let flac_prefixed = slice_from_flac_marker(first_header)?;
            decode_flac_track(flac_prefixed, &payload)
        }
        // PCM has no Ogg mapping in the wild; oximedia_container's Ogg
        // demuxer never reports CodecId::Pcm, so that arm is unreachable
        // here and is folded into the honest-Err default below.
        other => Err(TranscodeError::Unsupported(format!(
            "{other:?} audio in Ogg is not supported for transcode; \
             supported audio codecs from Ogg: flac"
        ))),
    }
}

/// Loads and fully decodes the audio track of a Matroska/WebM or Ogg file
/// to interleaved i16 PCM, dispatching on the already-probed
/// [`ContainerFormat`].
///
/// # Errors
///
/// See [`load_matroska_audio_pcm`] / [`load_ogg_audio_pcm`].
pub(crate) async fn load_container_audio_pcm(
    path: &Path,
    format: ContainerFormat,
) -> Result<DecodedPcm> {
    match format {
        ContainerFormat::Matroska | ContainerFormat::WebM => load_matroska_audio_pcm(path).await,
        ContainerFormat::Ogg => load_ogg_audio_pcm(path).await,
        other => Err(TranscodeError::Unsupported(format!(
            "load_container_audio_pcm called with unsupported format {other:?}"
        ))),
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac_bitstream::{stream_info_block, FlacStreamEncoder};

    /// Deterministic (non-silent) i16 test tone.
    fn test_tone_i16(frames: usize, channels: usize) -> Vec<i16> {
        (0..frames * channels)
            .map(|i| {
                let t = (i / channels) as f64;
                let ch = (i % channels) as f64;
                (7000.0 * (0.03 * t + ch * 0.7).sin()) as i16
            })
            .collect()
    }

    /// Builds one raw Ogg page by hand (RFC 3533 layout). CRC is written as
    /// zero: `oximedia_container`'s `OggDemuxer` does not validate it (its
    /// own test suite's `create_ogg_page` helper relies on the same fact).
    fn build_ogg_page(
        serial: u32,
        sequence: u32,
        flags: u8,
        granule: u64,
        packets: &[&[u8]],
    ) -> Vec<u8> {
        let mut segments = Vec::new();
        let mut data = Vec::new();
        for packet in packets {
            let len = packet.len();
            if len == 0 {
                segments.push(0);
            } else {
                let mut remaining = len;
                while remaining > 255 {
                    segments.push(255);
                    remaining -= 255;
                }
                segments.push(remaining as u8);
            }
            data.extend_from_slice(packet);
        }

        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.push(0); // version
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&serial.to_le_bytes());
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes()); // CRC (unvalidated by this workspace's demuxer)
        page.push(segments.len() as u8);
        page.extend_from_slice(&segments);
        page.extend_from_slice(&data);
        page
    }

    /// Builds a spec-shaped Ogg-FLAC file: BOS page (mapping header only —
    /// `OggDemuxer::handle_bos_page` only reads the BOS page's first
    /// packet), then a page with the `VORBIS_COMMENT` header (completing
    /// FLAC's 2-header-packet requirement), then a page per audio frame.
    fn build_ogg_flac_file(sample_rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        const FLAC_MAPPING_VERSION_MAJOR: u8 = 1;
        const FLAC_MAPPING_VERSION_MINOR: u8 = 0;
        const SERIAL: u32 = 0x5EED_1234;

        let block_frames = u16::try_from(samples.len() / usize::from(channels))
            .expect("test block fits u16 sample-frames");
        let streaminfo = stream_info_block(
            sample_rate,
            channels,
            u64::from(block_frames),
            block_frames,
            block_frames,
        );

        // Header packet 1: the Ogg-FLAC mapping header (0x7F + "FLAC" +
        // version + header-count) followed by "fLaC" + STREAMINFO.
        let mut header1 = Vec::new();
        header1.push(0x7F);
        header1.extend_from_slice(b"FLAC");
        header1.push(FLAC_MAPPING_VERSION_MAJOR);
        header1.push(FLAC_MAPPING_VERSION_MINOR);
        header1.extend_from_slice(&1u16.to_be_bytes()); // 1 more header packet follows
        header1.extend_from_slice(&streaminfo);

        // Header packet 2: a minimal (empty) VORBIS_COMMENT metadata block.
        let mut header2 = Vec::new();
        header2.push(0x84); // last-block flag | type 4 (VORBIS_COMMENT)
        let comment_payload = [0u8; 8]; // vendor_len=0, comment_count=0 (both LE u32)
        header2.extend_from_slice(&(comment_payload.len() as u32).to_be_bytes()[1..]);
        header2.extend_from_slice(&comment_payload);

        let mut flac_enc = FlacStreamEncoder::new(sample_rate, channels).expect("flac encoder");
        let audio_packet = flac_enc.encode_block(samples).expect("encode flac block");

        let mut out = Vec::new();
        out.extend_from_slice(&build_ogg_page(SERIAL, 0, 0x02, 0, &[&header1])); // BOS
        out.extend_from_slice(&build_ogg_page(SERIAL, 1, 0x00, 0, &[&header2]));
        out.extend_from_slice(&build_ogg_page(
            SERIAL,
            2,
            0x04, // EOS
            u64::from(block_frames),
            &[&audio_packet],
        ));
        out
    }

    #[tokio::test]
    async fn test_ogg_flac_decode_pcm_sample_exact() {
        let (sample_rate, channels) = (44_100u32, 2u16);
        let samples = test_tone_i16(1_500, usize::from(channels));
        let bytes = build_ogg_flac_file(sample_rate, channels, &samples);

        let path =
            std::env::temp_dir().join(format!("oximedia_test_ogg_flac_{}.ogg", std::process::id()));
        std::fs::write(&path, &bytes).expect("write ogg fixture");

        let pcm = load_ogg_audio_pcm(&path).await.expect("decode flac-in-ogg");
        let _ = std::fs::remove_file(&path);

        assert_eq!(pcm.sample_rate, sample_rate);
        assert_eq!(pcm.channels, channels);
        let decoded: Vec<i16> = pcm
            .data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(decoded, samples, "FLAC-in-Ogg decode must be sample-exact");
    }

    #[tokio::test]
    async fn test_frame_level_ogg_to_wav_sample_exact() {
        let (sample_rate, channels) = (32_000u32, 1u16);
        let samples = test_tone_i16(2_200, usize::from(channels));
        let bytes = build_ogg_flac_file(sample_rate, channels, &samples);

        let dir = std::env::temp_dir();
        let pid = std::process::id();
        let ogg_path = dir.join(format!("oximedia_test_ogg2wav_{pid}.ogg"));
        let wav_path = dir.join(format!("oximedia_test_ogg2wav_{pid}.wav"));
        std::fs::write(&ogg_path, &bytes).expect("write ogg fixture");

        let config = crate::PipelineConfig {
            input: ogg_path.clone(),
            output: wav_path.clone(),
            video_codec: None,
            audio_codec: None,
            quality: None,
            multipass: None,
            normalization: None,
            track_progress: false,
            hw_accel: false,
            stream_map: Vec::new(),
            start_time_secs: None,
            duration_secs: None,
            video_scale: None,
            audio_gain_db: None,
            output_fps: None,
        };

        let stats = crate::frame_level::execute_frame_level(&config, 0.0)
            .await
            .expect("Ogg -> WAV frame-level transcode");
        assert!(stats.audio_frames > 0);

        // Read the WAV back through the same decode path the WAV/FLAC
        // input branch uses, to compare sample-exact against the source.
        let wav_pcm = crate::frame_level::load_wav_pcm(&wav_path)
            .await
            .expect("read back wav");
        let _ = std::fs::remove_file(&ogg_path);
        let _ = std::fs::remove_file(&wav_path);

        assert_eq!(wav_pcm.sample_rate, sample_rate);
        assert_eq!(wav_pcm.channels, channels);
        let decoded: Vec<i16> = wav_pcm
            .data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(
            decoded, samples,
            "Ogg(FLAC) -> WAV frame-level transcode must be sample-exact"
        );
    }

    #[tokio::test]
    async fn test_ogg_vorbis_names_codec_honestly() {
        // A bare OpusHead/Vorbis-identification BOS page with no matching
        // audio: the important thing is the error names the real codec
        // instead of a generic "unsupported format" message.
        const SERIAL: u32 = 0xABCD_0001;
        let mut vorbis_id = Vec::new();
        vorbis_id.push(0x01);
        vorbis_id.extend_from_slice(b"vorbis");
        vorbis_id.extend_from_slice(&[0u8; 23]); // rest of the identification header, zeroed

        let page = build_ogg_page(SERIAL, 0, 0x02 | 0x04, 0, &[&vorbis_id]);
        let path = std::env::temp_dir().join(format!(
            "oximedia_test_ogg_vorbis_{}.ogg",
            std::process::id()
        ));
        std::fs::write(&path, &page).expect("write fixture");

        let err = load_ogg_audio_pcm(&path)
            .await
            .expect_err("vorbis must be rejected honestly");
        let _ = std::fs::remove_file(&path);
        let msg = err.to_string();
        assert!(msg.contains("Vorbis"), "must name the codec found: {msg}");
    }
}
