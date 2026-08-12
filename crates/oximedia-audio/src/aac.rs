//! AAC (Advanced Audio Coding) — ADTS parsing only; **decoding is not
//! implemented**.
//!
//! # Status
//!
//! `oximedia-audio` cannot decode AAC. This module provides:
//!
//! - [`AdtsHeader`] — a real ADTS (Audio Data Transport Stream) transport
//!   header parser (sync word, profile, sample rate index, channel
//!   configuration, frame length, CRC flag).
//! - [`AacObjectType`] — ISO 14496-3 audio object type identifiers.
//! - [`AacDecoder`] — a stream *inspector* whose decode entry points fail
//!   closed with [`AudioError::UnsupportedFormat`]. It implements
//!   [`AudioDecoder`] and reports
//!   [`CodecId::Aac`], so it can sit in a decoder
//!   registry and be rejected honestly instead of being mistaken for a codec
//!   OxiMedia does implement.
//!
//! There is no spectral (Huffman) decode, no inverse quantisation, no
//! filterbank, and no SBR/PS. An earlier revision of this module returned an
//! [`AudioFrame`] whose samples were derived from raw payload bit-patterns —
//! a fabricated signal unrelated to the encoded audio — and reported
//! `CodecId::Mp3` as its codec. Both behaviours have been removed; see
//! `docs/codec_status.md`.
//!
//! Real AAC decode is expected to arrive through a separate platform
//! extension (for example AudioToolbox on macOS), not through this module.
//!
//! # Patents
//! The Fraunhofer/Via Licensing AAC patents expired in April 2023, so an
//! implementation would be patent-free; the gap here is engineering effort,
//! not licensing.

#![allow(clippy::cast_precision_loss)]
#![allow(clippy::cast_possible_truncation)]

use crate::traits::AudioDecoder;
use crate::{AudioError, AudioFrame, AudioResult, ChannelLayout};
use oximedia_core::{CodecId, SampleFormat};

/// AAC object type (ISO 14496-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AacObjectType {
    /// AAC-LC Low Complexity profile.
    AacLc,
    /// HE-AAC v1 with Spectral Band Replication.
    HeAacV1,
    /// HE-AAC v2 with Parametric Stereo.
    HeAacV2,
}

impl AacObjectType {
    /// ISO 14496-3 audio object type identifier.
    #[must_use]
    pub fn object_type_id(self) -> u8 {
        match self {
            Self::AacLc => 2,
            Self::HeAacV1 => 5,
            Self::HeAacV2 => 29,
        }
    }
}

/// ADTS frame header (7 or 9 bytes).
#[derive(Debug, Clone, Copy)]
pub struct AdtsHeader {
    /// MPEG version (0 = MPEG-4, 1 = MPEG-2).
    pub mpeg_version: u8,
    /// Audio Object Type (minus 1 in the bitstream).
    pub profile: u8,
    /// Sampling frequency index (0-12, maps to sample rates).
    pub sampling_freq_index: u8,
    /// Channel configuration.
    pub channel_config: u8,
    /// Frame length in bytes including header.
    pub frame_length: u16,
    /// Buffer fullness (0x7FF = VBR).
    pub buffer_fullness: u16,
    /// Number of AAC frames per ADTS frame minus 1.
    pub num_aac_frames: u8,
    /// Whether CRC protection is present.
    pub has_crc: bool,
}

/// Standard AAC sampling frequency table (ISO 14496-3 Table 4.82).
pub const AAC_SAMPLE_RATES: [u32; 13] = [
    96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
];

impl AdtsHeader {
    /// Parse an ADTS header from a byte slice.
    ///
    /// ADTS sync word is 12 bits of '1' (`0xFFF`).
    ///
    /// # Errors
    ///
    /// Returns error if the sync word is missing or the data is too short.
    pub fn parse(data: &[u8]) -> AudioResult<Self> {
        if data.len() < 7 {
            return Err(AudioError::InvalidData(
                "ADTS header requires at least 7 bytes".into(),
            ));
        }

        // Check sync word: first 12 bits must be all 1s
        if data[0] != 0xFF || (data[1] & 0xF0) != 0xF0 {
            return Err(AudioError::InvalidData(format!(
                "ADTS sync word not found: {:02X} {:02X}",
                data[0], data[1]
            )));
        }

        let mpeg_version = (data[1] >> 3) & 0x01;
        let has_crc = (data[1] & 0x01) == 0; // 0 = has CRC, 1 = no CRC
        let profile = ((data[2] >> 6) & 0x03) + 1; // +1 for object type
        let sampling_freq_index = (data[2] >> 2) & 0x0F;
        let channel_config = ((data[2] & 0x01) << 2) | (data[3] >> 6);
        let frame_length = ((u16::from(data[3] & 0x03) << 11)
            | (u16::from(data[4]) << 3)
            | (u16::from(data[5]) >> 5)) as u16;
        let buffer_fullness = ((u16::from(data[5] & 0x1F) << 6) | u16::from(data[6] >> 2)) as u16;
        let num_aac_frames = (data[6] & 0x03) + 1;

        if sampling_freq_index >= 13 {
            return Err(AudioError::InvalidData(format!(
                "Invalid sampling frequency index: {sampling_freq_index}"
            )));
        }

        Ok(Self {
            mpeg_version,
            profile,
            sampling_freq_index,
            channel_config,
            frame_length,
            buffer_fullness,
            num_aac_frames,
            has_crc,
        })
    }

    /// Get the sample rate for this header.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        AAC_SAMPLE_RATES
            .get(self.sampling_freq_index as usize)
            .copied()
            .unwrap_or(44100)
    }

    /// Get the channel count.
    #[must_use]
    pub fn channels(&self) -> u8 {
        match self.channel_config {
            0 => 2, // defined in AOT-specific configuration
            1 => 1, // center front speaker
            2 => 2, // left/right front speakers
            3 => 3, // C + L + R
            4 => 4, // C + L + R + rear center
            5 => 5, // C + L + R + Ls + Rs
            6 => 6, // 5.1
            7 => 8, // 7.1
            _ => 2,
        }
    }

    /// Header size in bytes (7 without CRC, 9 with CRC).
    #[must_use]
    pub fn header_size(&self) -> usize {
        if self.has_crc {
            9
        } else {
            7
        }
    }
}

/// Error returned by every AAC decode entry point.
///
/// AAC decoding is **not implemented** in `oximedia-audio`. Rather than
/// fabricating PCM, all decode paths fail closed with this error.
fn not_implemented() -> AudioError {
    AudioError::UnsupportedFormat(
        "AAC decoding is not implemented in oximedia-audio: no spectral (Huffman) \
         decode, no filterbank, no SBR/PS. Real AAC decode is planned via an \
         external platform extension (e.g. AudioToolbox on macOS). Use ADTS \
         header parsing (`AdtsHeader::parse`) for stream inspection only."
            .to_string(),
    )
}

/// AAC-LC stream inspector — **decoding is not implemented**.
///
/// # Status
///
/// This type deliberately does **not** decode AAC. Earlier revisions returned
/// an [`AudioFrame`] filled with values derived from raw bit-patterns of the
/// payload (a fabricated signal that was not the encoded audio) and reported
/// [`CodecId::Mp3`] as its codec. Both behaviours were dishonest and have been
/// removed:
///
/// * [`AacDecoder::send_packet`] and [`AacDecoder::receive_frame`] return
///   [`AudioError::UnsupportedFormat`]; they never produce PCM.
/// * The [`AudioDecoder`] implementation reports [`CodecId::Aac`] — its real
///   identity — rather than borrowing another codec's. Every decode entry
///   point on that trait still fails closed.
///
/// What *is* real here is ADTS framing: [`AdtsHeader::parse`] parses the
/// transport header (sample rate, channel configuration, frame length), and
/// [`AacDecoder::send_packet`] updates [`AacDecoder::sample_rate`] /
/// [`AacDecoder::channel_layout`] from the first valid header it sees before
/// failing, so stream inspection still works.
///
/// Real AAC decode is expected to arrive through a separate FFI extension
/// crate; see `docs/codec_status.md`.
pub struct AacDecoder {
    /// Sample rate observed in the most recent valid ADTS header.
    sample_rate: Option<u32>,
    /// Channel count observed in the most recent valid ADTS header.
    channels: Option<u8>,
    /// Number of packets rejected because decoding is not implemented.
    decode_errors: u32,
}

impl AacDecoder {
    /// Create a new AAC stream inspector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sample_rate: None,
            channels: None,
            decode_errors: 0,
        }
    }

    /// Honest codec identity of this module: `"aac"`.
    ///
    /// Equivalent to [`CodecId::Aac.name()`](oximedia_core::CodecId::name);
    /// kept as an inherent associated function so callers can ask without
    /// constructing a decoder or importing the trait.
    #[must_use]
    pub fn codec_name() -> &'static str {
        CodecId::Aac.name()
    }

    /// Number of packets rejected because AAC decoding is not implemented.
    #[must_use]
    pub fn decode_errors(&self) -> u32 {
        self.decode_errors
    }

    /// Accept a compressed AAC packet — always fails.
    ///
    /// Any ADTS headers found in `data` are parsed first so that
    /// [`Self::sample_rate`] and [`Self::channel_layout`] can report the
    /// stream's parameters, then the call fails with
    /// [`AudioError::UnsupportedFormat`].
    ///
    /// # Errors
    ///
    /// Always returns [`AudioError::UnsupportedFormat`]: AAC decoding is not
    /// implemented.
    pub fn send_packet(&mut self, data: &[u8], _pts: i64) -> AudioResult<()> {
        self.scan_adts_metadata(data);
        self.decode_errors = self.decode_errors.saturating_add(1);
        Err(not_implemented())
    }

    /// Retrieve a decoded frame — always fails.
    ///
    /// # Errors
    ///
    /// Always returns [`AudioError::UnsupportedFormat`]: there is no decoder
    /// behind this type, so a frame can never become available.
    pub fn receive_frame(&mut self) -> AudioResult<Option<AudioFrame>> {
        Err(not_implemented())
    }

    /// Drop any inspection state.
    ///
    /// # Errors
    ///
    /// Never fails; the signature mirrors the decoder convention.
    pub fn flush(&mut self) -> AudioResult<()> {
        Ok(())
    }

    /// Reset the inspector to its initial state.
    pub fn reset(&mut self) {
        self.sample_rate = None;
        self.channels = None;
        self.decode_errors = 0;
    }

    /// Output sample format — always `None`, because nothing is ever decoded.
    #[must_use]
    pub fn output_format(&self) -> Option<SampleFormat> {
        None
    }

    /// Sample rate from the most recently parsed ADTS header, if any.
    #[must_use]
    pub fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    /// Channel layout from the most recently parsed ADTS header, if any.
    #[must_use]
    pub fn channel_layout(&self) -> Option<ChannelLayout> {
        self.channels.map(Self::channel_config_to_layout)
    }

    /// Scan `data` for the first parsable ADTS header and record its stream
    /// parameters. Purely informational; no audio is produced.
    fn scan_adts_metadata(&mut self, data: &[u8]) {
        if data.len() < 7 {
            return;
        }
        for i in 0..=(data.len() - 7) {
            if data[i] == 0xFF && (data[i + 1] & 0xF0) == 0xF0 {
                if let Ok(header) = AdtsHeader::parse(&data[i..]) {
                    self.sample_rate = Some(header.sample_rate());
                    self.channels = Some(header.channels());
                    return;
                }
            }
        }
    }

    /// Map an AAC channel configuration to a [`ChannelLayout`].
    fn channel_config_to_layout(channels: u8) -> ChannelLayout {
        match channels {
            1 => ChannelLayout::Mono,
            2 => ChannelLayout::Stereo,
            6 => ChannelLayout::Surround51,
            _ => ChannelLayout::Stereo,
        }
    }
}

impl Default for AacDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// [`AudioDecoder`] implementation that fails closed.
///
/// Every method delegates to the inherent implementation above, so the trait
/// object behaves exactly like the concrete type: it reports
/// [`CodecId::Aac`], surfaces ADTS stream parameters, and refuses to produce
/// a single PCM sample.
impl AudioDecoder for AacDecoder {
    /// The honest codec identity: [`CodecId::Aac`].
    ///
    /// Reporting `Aac` here is what makes the failure legible — a caller that
    /// receives [`AudioError::UnsupportedFormat`] knows *which* codec is
    /// missing instead of being told a different codec failed.
    fn codec(&self) -> CodecId {
        CodecId::Aac
    }

    /// Always fails; see [`AacDecoder::send_packet`].
    fn send_packet(&mut self, data: &[u8], pts: i64) -> AudioResult<()> {
        Self::send_packet(self, data, pts)
    }

    /// Always fails; see [`AacDecoder::receive_frame`].
    fn receive_frame(&mut self) -> AudioResult<Option<AudioFrame>> {
        Self::receive_frame(self)
    }

    fn flush(&mut self) -> AudioResult<()> {
        Self::flush(self)
    }

    fn reset(&mut self) {
        Self::reset(self);
    }

    /// Always `None`: nothing is ever decoded, so there is no output format.
    fn output_format(&self) -> Option<SampleFormat> {
        Self::output_format(self)
    }

    fn sample_rate(&self) -> Option<u32> {
        Self::sample_rate(self)
    }

    fn channel_layout(&self) -> Option<ChannelLayout> {
        Self::channel_layout(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adts_header_parse_invalid_sync() {
        let data = [0x00u8; 7];
        assert!(AdtsHeader::parse(&data).is_err());
    }

    #[test]
    fn test_adts_header_parse_too_short() {
        let data = [0xFF, 0xF1, 0x50];
        assert!(AdtsHeader::parse(&data).is_err());
    }

    #[test]
    fn test_adts_header_sample_rate_lookup() {
        // Build a minimal ADTS header with sampling_freq_index = 3 (48000 Hz)
        // Byte 0: 0xFF
        // Byte 1: 0xF1 (MPEG-4, layer=0, no CRC)
        // Byte 2: 0x50 (profile=AAC-LC, sfi=3, private=0, ch_cfg high bit=0)
        // Byte 3-6: frame length = 0 (we'll just check sample rate)
        let mut data = [0u8; 7];
        data[0] = 0xFF;
        data[1] = 0xF1; // MPEG-4, no CRC
        data[2] = 0x50; // profile=1 (LC), sfi=4 (44100Hz)
        data[3] = 0x00;
        data[4] = 0x1C; // frame_length = 7 bytes (just the header)
        data[5] = 0xE0;
        data[6] = 0x00;

        // frame_length from data:
        // bits [30..18]: (data[3] & 0x03) << 11 | data[4] << 3 | data[5] >> 5
        // = 0 | 0x1C << 3 | 0xE0 >> 5 = 0 | 0xE0 | 7 = 0 + 224 + 7 = not right for now
        // Just test sample rate parsing
        if let Ok(h) = AdtsHeader::parse(&data) {
            // sfi = (0x50 >> 2) & 0x0F = 0x14 >> 0 = 20 >> 2 = 5 = 32000 Hz
            let sr = h.sample_rate();
            assert!(AAC_SAMPLE_RATES.contains(&sr) || sr == 44100);
        }
    }

    /// Build a syntactically valid 7-byte ADTS header (MPEG-4, no CRC,
    /// AAC-LC, 48 kHz, stereo) declaring `frame_length` bytes.
    fn adts_header(frame_length: u16) -> [u8; 7] {
        let mut data = [0u8; 7];
        data[0] = 0xFF;
        data[1] = 0xF1; // MPEG-4, layer 0, no CRC
                        // profile(2)=1 (LC) | sfi(4)=3 (48000) | private(1)=0 | ch_cfg high bit
        data[2] = (1 << 6) | (3 << 2);
        data[3] = (2 << 6) | ((frame_length >> 11) & 0x03) as u8;
        data[4] = ((frame_length >> 3) & 0xFF) as u8;
        data[5] = (((frame_length & 0x07) << 5) | 0x1F) as u8;
        data[6] = 0xFC;
        data
    }

    #[test]
    fn test_aac_decoder_new_reports_aac_codec_id() {
        let dec = AacDecoder::new();
        // The honest identity — never `CodecId::Mp3`, as an earlier revision
        // reported.
        assert_eq!(AacDecoder::codec_name(), "aac");
        assert_eq!(AudioDecoder::codec(&dec), CodecId::Aac);
        assert_ne!(AudioDecoder::codec(&dec), CodecId::Mp3);
        assert!(dec.sample_rate().is_none());
        assert!(dec.channel_layout().is_none());
        assert!(dec.output_format().is_none());
        assert_eq!(dec.decode_errors(), 0);
    }

    /// Driving the decoder purely through the [`AudioDecoder`] trait object
    /// must be just as honest as the inherent API: `CodecId::Aac`, no PCM,
    /// `UnsupportedFormat` on every decode entry point.
    #[test]
    fn test_aac_decoder_as_trait_object_fails_closed() {
        let mut dec: Box<dyn AudioDecoder> = Box::new(AacDecoder::new());
        assert_eq!(dec.codec(), CodecId::Aac);

        let mut packet = adts_header(64).to_vec();
        packet.resize(64, 0);

        let err = dec
            .send_packet(&packet, 0)
            .expect_err("AAC decode must not succeed through the trait either");
        assert!(
            matches!(err, AudioError::UnsupportedFormat(_)),
            "expected UnsupportedFormat, got {err:?}"
        );

        // Stream inspection still works through the trait.
        assert_eq!(dec.sample_rate(), Some(48_000));
        assert_eq!(dec.channel_layout(), Some(ChannelLayout::Stereo));
        assert!(dec.output_format().is_none());

        assert!(
            dec.receive_frame().is_err(),
            "no fabricated frame may be handed out"
        );
        dec.flush().expect("flush must succeed");

        dec.reset();
        assert!(
            dec.sample_rate().is_none(),
            "reset must clear inspection state"
        );
    }

    /// `AacDecoder` must satisfy the `Send` supertrait of [`AudioDecoder`].
    #[test]
    fn test_aac_decoder_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AacDecoder>();
    }

    /// Decoding must fail closed, even for an empty packet — the decoder can
    /// never produce PCM.
    #[test]
    fn test_aac_decoder_empty_packet_errors() {
        let mut dec = AacDecoder::new();
        let err = dec
            .send_packet(&[], 0)
            .expect_err("AAC decode must not succeed");
        assert!(
            matches!(err, AudioError::UnsupportedFormat(_)),
            "expected UnsupportedFormat, got {err:?}"
        );
        assert!(
            format!("{err}").contains("not implemented"),
            "error must say decoding is not implemented: {err}"
        );
        assert!(
            dec.receive_frame().is_err(),
            "no frame may ever be produced"
        );
    }

    /// Garbage must not be turned into audio either.
    #[test]
    fn test_aac_decoder_garbage_data_errors() {
        let mut dec = AacDecoder::new();
        let garbage = vec![0x55u8; 100];
        assert!(
            dec.send_packet(&garbage, 0).is_err(),
            "garbage must not decode"
        );
        assert!(dec.receive_frame().is_err());
        assert_eq!(dec.decode_errors(), 1);
    }

    /// A well-formed ADTS packet is still rejected, but its stream parameters
    /// are surfaced for inspection.
    #[test]
    fn test_aac_decoder_valid_adts_errors_but_reports_parameters() {
        let mut dec = AacDecoder::new();
        let mut packet = adts_header(64).to_vec();
        packet.resize(64, 0);

        assert!(
            dec.send_packet(&packet, 0).is_err(),
            "valid ADTS must still fail: decoding is not implemented"
        );
        assert_eq!(dec.sample_rate(), Some(48_000));
        assert_eq!(dec.channel_layout(), Some(ChannelLayout::Stereo));
        assert!(
            dec.receive_frame().is_err(),
            "no fabricated frame may be handed out"
        );
    }

    #[test]
    fn test_aac_decoder_reset() {
        let mut dec = AacDecoder::new();
        let mut packet = adts_header(64).to_vec();
        packet.resize(64, 0);
        let _ = dec.send_packet(&packet, 0);
        assert_eq!(dec.decode_errors(), 1);

        dec.reset();
        assert_eq!(dec.decode_errors(), 0);
        assert!(dec.sample_rate().is_none());
        assert!(dec.channel_layout().is_none());
    }

    #[test]
    fn test_aac_decoder_flush_is_ok() {
        let mut dec = AacDecoder::new();
        dec.flush().expect("flush must succeed");
    }

    #[test]
    fn test_aac_object_type_id() {
        assert_eq!(AacObjectType::AacLc.object_type_id(), 2);
        assert_eq!(AacObjectType::HeAacV1.object_type_id(), 5);
        assert_eq!(AacObjectType::HeAacV2.object_type_id(), 29);
    }

    #[test]
    fn test_adts_channel_config_to_channels() {
        // Build minimal header to test channel_config
        // sfi = 3 (48000 Hz), channel_config = 2 (stereo)
        // data[2] = profile<<6 | sfi<<2 | ch_config>>2
        //         = 1<<6 | 3<<2 | 0 = 0x40 | 0x0C = 0x4C
        // data[3] = ch_config<<6 | ...
        let mut data = [0u8; 7];
        data[0] = 0xFF;
        data[1] = 0xF1;
        data[2] = 0x4C; // profile=LC, sfi=3, channel_config high bit=0
        data[3] = 0x80; // channel_config=2 (bits 7:6 of byte 3 = 10), frame length...
        data[4] = 0x00;
        data[5] = 0x1C; // frame length approximately 7
        data[6] = 0x00;
        if let Ok(h) = AdtsHeader::parse(&data) {
            // channel_config from: ((data[2] & 0x01) << 2) | (data[3] >> 6)
            // = (0x4C & 0x01) << 2 | 0x80 >> 6 = 0 | 2 = 2
            assert_eq!(h.channels(), 2);
        }
    }
}
