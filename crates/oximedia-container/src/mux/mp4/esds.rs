//! MPEG-4 `esds` (`ES_Descriptor`) construction for MP4 audio tracks.
//!
//! An `mp4a` sample entry is meaningless without an `esds` box: it is what
//! carries the `AudioSpecificConfig` a decoder needs (profile, sample rate,
//! channel configuration). Nothing in the workspace could build one before, so
//! [`SimpleMp4Muxer`](super::simple::SimpleMp4Muxer) — the only writer that
//! accepts an arbitrary sample-entry fourcc — could emit an `mp4a` track that
//! no player could decode.
//!
//! The descriptor tree emitted here is the standard AAC layout:
//!
//! ```text
//! esds (FullBox v0)
//! └─ ES_Descriptor                (tag 0x03)
//!    ├─ DecoderConfigDescriptor   (tag 0x04)  objectTypeIndication = 0x40
//!    │  └─ DecoderSpecificInfo    (tag 0x05)  ← AudioSpecificConfig
//!    └─ SLConfigDescriptor        (tag 0x06)  predefined = 2 (MP4 framing)
//! ```
//!
//! Descriptor lengths use the expandable 7-bits-per-byte encoding of
//! ISO/IEC 14496-1 §8.3.3, emitted in minimal form.

#![forbid(unsafe_code)]
#![allow(clippy::cast_possible_truncation)]

use super::simple::encode_box;

// ─── esds (MPEG-4 ES_Descriptor) ─────────────────────────────────────────────

/// Tuning knobs for [`build_esds_payload`] / [`build_esds_box`].
#[derive(Debug, Clone, Copy)]
pub struct EsdsParams {
    /// Elementary-stream ID. Conventionally the track ID; `0` is also accepted
    /// by every player in practice.
    pub es_id: u16,
    /// Decoding buffer size in bytes (24-bit field).
    pub buffer_size_db: u32,
    /// Maximum bitrate in bits/second (`0` = unknown).
    pub max_bitrate: u32,
    /// Average bitrate in bits/second (`0` = unknown).
    pub avg_bitrate: u32,
}

impl Default for EsdsParams {
    fn default() -> Self {
        Self {
            es_id: 1,
            buffer_size_db: 0,
            max_bitrate: 0,
            avg_bitrate: 0,
        }
    }
}

/// MPEG-4 `objectTypeIndication` for MPEG-4 Audio (ISO/IEC 14496-3).
const OTI_MPEG4_AUDIO: u8 = 0x40;
/// MPEG-4 `streamType` for an audio stream.
const STREAM_TYPE_AUDIO: u8 = 0x05;

/// Encodes an MPEG-4 descriptor length using the expandable 7-bits-per-byte
/// form defined in ISO/IEC 14496-1 §8.3.3.
fn encode_descriptor_length(mut length: u32, out: &mut Vec<u8>) {
    // Emit the 7-bit groups most-significant first, each but the last with the
    // continuation bit (0x80) set.
    let mut groups = [0u8; 5];
    let mut count = 0usize;
    loop {
        groups[count] = (length & 0x7F) as u8;
        count += 1;
        length >>= 7;
        if length == 0 || count == groups.len() {
            break;
        }
    }
    for i in (0..count).rev() {
        let last = i == 0;
        out.push(if last { groups[i] } else { groups[i] | 0x80 });
    }
}

/// Wraps `payload` in an MPEG-4 descriptor with the given tag.
fn encode_descriptor(tag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 6);
    out.push(tag);
    encode_descriptor_length(u32::try_from(payload.len()).unwrap_or(u32::MAX), &mut out);
    out.extend_from_slice(payload);
    out
}

/// Builds the **payload** of an `esds` box (an ISOBMFF `FullBox`) describing an
/// AAC-LC elementary stream.
///
/// The result is `version(1) + flags(3)` followed by the `ES_Descriptor`
/// (tag `0x03`) → `DecoderConfigDescriptor` (tag `0x04`) →
/// `DecoderSpecificInfo` (tag `0x05`, carrying `audio_specific_config`) plus an
/// `SLConfigDescriptor` (tag `0x06`, `predefined = 2` — MP4 framing).
/// Descriptor lengths use the expandable 7-bits-per-byte encoding.
///
/// Feed the returned bytes straight into
/// [`AudioCodecInfo::with_config`] with a `b"esds"` fourcc:
///
/// ```
/// use oximedia_container::mux::mp4::simple::{
///     build_esds_payload, AudioCodecInfo, EsdsParams,
/// };
///
/// // AudioSpecificConfig for AAC-LC, 44100 Hz, stereo.
/// let asc = [0x12, 0x10];
/// let esds = build_esds_payload(&asc, &EsdsParams::default());
/// let info = AudioCodecInfo::new(*b"mp4a", 44_100, 2).with_config(*b"esds", esds);
/// assert_eq!(info.config_fourcc, Some(*b"esds"));
/// ```
#[must_use]
pub fn build_esds_payload(audio_specific_config: &[u8], params: &EsdsParams) -> Vec<u8> {
    // DecoderSpecificInfo (tag 0x05) — the raw AudioSpecificConfig.
    let dsi = encode_descriptor(0x05, audio_specific_config);

    // DecoderConfigDescriptor (tag 0x04)
    let mut dcd_payload = Vec::with_capacity(13 + dsi.len());
    dcd_payload.push(OTI_MPEG4_AUDIO);
    // streamType(6) | upStream(1) | reserved(1) == 1
    dcd_payload.push((STREAM_TYPE_AUDIO << 2) | 0x01);
    let buffer_size = params.buffer_size_db & 0x00FF_FFFF;
    dcd_payload.push(((buffer_size >> 16) & 0xFF) as u8);
    dcd_payload.push(((buffer_size >> 8) & 0xFF) as u8);
    dcd_payload.push((buffer_size & 0xFF) as u8);
    dcd_payload.extend_from_slice(&params.max_bitrate.to_be_bytes());
    dcd_payload.extend_from_slice(&params.avg_bitrate.to_be_bytes());
    dcd_payload.extend_from_slice(&dsi);
    let dcd = encode_descriptor(0x04, &dcd_payload);

    // SLConfigDescriptor (tag 0x06) — predefined = 2 (MP4 framing).
    let sl = encode_descriptor(0x06, &[0x02]);

    // ES_Descriptor (tag 0x03)
    let mut es_payload = Vec::with_capacity(3 + dcd.len() + sl.len());
    es_payload.extend_from_slice(&params.es_id.to_be_bytes());
    // streamDependenceFlag(1) URL_Flag(1) OCRstreamFlag(1) streamPriority(5)
    es_payload.push(0x00);
    es_payload.extend_from_slice(&dcd);
    es_payload.extend_from_slice(&sl);
    let es = encode_descriptor(0x03, &es_payload);

    let mut out = Vec::with_capacity(4 + es.len());
    out.extend_from_slice(&[0u8; 4]); // version = 0, flags = 0
    out.extend_from_slice(&es);
    out
}

/// Builds a complete `esds` box (header included) for an AAC-LC stream.
///
/// See [`build_esds_payload`] for the descriptor layout.
#[must_use]
pub fn build_esds_box(audio_specific_config: &[u8], params: &EsdsParams) -> Vec<u8> {
    encode_box(b"esds", &build_esds_payload(audio_specific_config, params))
}

/// Sample-rate table used by `AudioSpecificConfig` (ISO/IEC 14496-3 Table 1.18).
const ASC_SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

/// Builds a two- or five-byte `AudioSpecificConfig` for the given AAC profile,
/// sample rate and channel configuration.
///
/// `audio_object_type` is the raw AOT (2 = AAC-LC, 5 = HE-AAC/SBR, …).
/// Sample rates outside the standard table are emitted using the 4-bit escape
/// index `0x0F` followed by the explicit 24-bit rate, per ISO/IEC 14496-3
/// §1.6.2.1.
///
/// ```
/// use oximedia_container::mux::mp4::simple::build_audio_specific_config;
///
/// // AAC-LC (AOT 2), 44100 Hz (index 4), stereo (config 2) → 0b00010_0100_0010_000
/// assert_eq!(build_audio_specific_config(2, 44_100, 2), vec![0x12, 0x10]);
/// ```
#[must_use]
pub fn build_audio_specific_config(
    audio_object_type: u8,
    sample_rate: u32,
    channel_config: u8,
) -> Vec<u8> {
    let index = ASC_SAMPLE_RATES
        .iter()
        .position(|&r| r == sample_rate)
        .map_or(0x0Fu8, |i| i as u8);

    let mut bits: u64 = 0;
    let mut used: u32 = 0;
    let mut push = |value: u64, width: u32| {
        bits = (bits << width) | (value & ((1u64 << width) - 1));
        used += width;
    };

    push(u64::from(audio_object_type & 0x1F), 5);
    push(u64::from(index), 4);
    if index == 0x0F {
        push(u64::from(sample_rate), 24);
    }
    push(u64::from(channel_config & 0x0F), 4);
    // frameLengthFlag(1) dependsOnCoreCoder(1) extensionFlag(1)
    push(0, 3);

    // Left-align into whole bytes.
    let total_bytes = used.div_ceil(8);
    let padding = total_bytes * 8 - used;
    bits <<= padding;

    let mut out = Vec::with_capacity(total_bytes as usize);
    for i in (0..total_bytes).rev() {
        out.push(((bits >> (i * 8)) & 0xFF) as u8);
    }
    out
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u32_be(buf: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ])
    }

    // ── esds / AudioSpecificConfig ────────────────────────────────────────

    #[test]
    fn descriptor_length_uses_minimal_encoding() {
        let mut out = Vec::new();
        encode_descriptor_length(0, &mut out);
        assert_eq!(out, vec![0x00]);

        out.clear();
        encode_descriptor_length(127, &mut out);
        assert_eq!(out, vec![0x7F]);

        out.clear();
        encode_descriptor_length(128, &mut out);
        assert_eq!(out, vec![0x81, 0x00]);

        out.clear();
        encode_descriptor_length(0x0000_4000, &mut out);
        assert_eq!(out, vec![0x81, 0x80, 0x00]);
    }

    #[test]
    fn audio_specific_config_matches_the_standard_table() {
        // AAC-LC (2), 44100 Hz (index 4), stereo (2).
        assert_eq!(build_audio_specific_config(2, 44_100, 2), vec![0x12, 0x10]);
        // AAC-LC, 48000 Hz (index 3), mono (1).
        assert_eq!(build_audio_specific_config(2, 48_000, 1), vec![0x11, 0x88]);
        // Non-table rate uses the 0x0F escape plus an explicit 24-bit rate.
        let escaped = build_audio_specific_config(2, 12_345, 2);
        assert_eq!(escaped.len(), 5, "5 + 4 + 24 + 4 + 3 = 40 bits");
        assert_eq!(escaped[0] & 0xF8, 0x10, "AOT 2 in the top 5 bits");
        assert_eq!(escaped[0] & 0x07, 0x07, "escape index 0x0F starts here");
    }

    #[test]
    fn esds_payload_has_the_expected_descriptor_tree() {
        let asc = build_audio_specific_config(2, 44_100, 2);
        let params = EsdsParams {
            es_id: 2,
            buffer_size_db: 6144,
            max_bitrate: 128_000,
            avg_bitrate: 128_000,
        };
        let payload = build_esds_payload(&asc, &params);

        // FullBox version + flags
        assert_eq!(&payload[0..4], &[0, 0, 0, 0]);
        // ES_Descriptor
        assert_eq!(payload[4], 0x03, "ES_Descriptor tag");
        let (es_len, es_body) = read_descriptor(&payload[4..]).expect("ES_Descriptor");
        assert_eq!(es_len, es_body.len());
        assert_eq!(u16::from_be_bytes([es_body[0], es_body[1]]), 2, "ES_ID");
        assert_eq!(es_body[2], 0x00, "no dependency / URL / OCR");

        // DecoderConfigDescriptor
        assert_eq!(es_body[3], 0x04, "DecoderConfigDescriptor tag");
        let (_, dcd) = read_descriptor(&es_body[3..]).expect("DecoderConfigDescriptor");
        assert_eq!(dcd[0], 0x40, "objectTypeIndication = MPEG-4 Audio");
        assert_eq!(dcd[1], (0x05 << 2) | 0x01, "streamType = audio, reserved=1");
        assert_eq!(
            u32::from_be_bytes([0, dcd[2], dcd[3], dcd[4]]),
            6144,
            "bufferSizeDB"
        );
        assert_eq!(read_u32_be(dcd, 5), 128_000, "maxBitrate");
        assert_eq!(read_u32_be(dcd, 9), 128_000, "avgBitrate");

        // DecoderSpecificInfo carrying the AudioSpecificConfig verbatim
        assert_eq!(dcd[13], 0x05, "DecoderSpecificInfo tag");
        let (_, dsi) = read_descriptor(&dcd[13..]).expect("DecoderSpecificInfo");
        assert_eq!(dsi, asc.as_slice(), "ASC must round-trip byte for byte");

        // SLConfigDescriptor follows the DecoderConfigDescriptor.
        let dcd_total = descriptor_total_len(&es_body[3..]).expect("dcd length");
        let sl = &es_body[3 + dcd_total..];
        assert_eq!(sl[0], 0x06, "SLConfigDescriptor tag");
        let (_, sl_body) = read_descriptor(sl).expect("SLConfigDescriptor");
        assert_eq!(sl_body, &[0x02], "predefined = MP4 framing");
    }

    #[test]
    fn esds_box_wraps_the_payload() {
        let asc = build_audio_specific_config(2, 48_000, 2);
        let payload = build_esds_payload(&asc, &EsdsParams::default());
        let boxed = build_esds_box(&asc, &EsdsParams::default());
        assert_eq!(&boxed[4..8], b"esds");
        assert_eq!(read_u32_be(&boxed, 0) as usize, boxed.len());
        assert_eq!(&boxed[8..], payload.as_slice());
    }

    #[test]
    fn esds_survives_a_long_audio_specific_config() {
        // > 127 bytes forces the multi-byte descriptor length encoding.
        let asc = vec![0xAB_u8; 200];
        let payload = build_esds_payload(&asc, &EsdsParams::default());
        let (_, es_body) = read_descriptor(&payload[4..]).expect("ES_Descriptor");
        let (_, dcd) = read_descriptor(&es_body[3..]).expect("DecoderConfigDescriptor");
        let (_, dsi) = read_descriptor(&dcd[13..]).expect("DecoderSpecificInfo");
        assert_eq!(dsi, asc.as_slice());
    }

    /// Reads one MPEG-4 descriptor, returning `(declared_length, body)`.
    fn read_descriptor(data: &[u8]) -> Option<(usize, &[u8])> {
        let (length, header_len) = read_descriptor_length(data)?;
        let start = 1 + header_len;
        data.get(start..start + length).map(|body| (length, body))
    }

    /// Total on-the-wire size of the descriptor at `data[0]`.
    fn descriptor_total_len(data: &[u8]) -> Option<usize> {
        let (length, header_len) = read_descriptor_length(data)?;
        Some(1 + header_len + length)
    }

    /// Decodes the expandable length that follows a descriptor tag.
    fn read_descriptor_length(data: &[u8]) -> Option<(usize, usize)> {
        let mut length = 0usize;
        let mut used = 0usize;
        loop {
            let byte = *data.get(1 + used)?;
            length = (length << 7) | usize::from(byte & 0x7F);
            used += 1;
            if byte & 0x80 == 0 || used == 4 {
                break;
            }
        }
        Some((length, used))
    }
}
