//! Adobe FLV (Flash Video) demuxer.
//!
//! FLV is a simple tag-based container originally designed for Adobe
//! Flash and still widely produced by RTMP livestreaming pipelines.
//! The file format is specified in Adobe's *Video File Format
//! Specification Version 10* (2008).
//!
//! # File layout
//!
//! ```text
//!  ┌──────────────────────────┐
//!  │ FLV header (9 bytes)     │
//!  │   F L V signature        │
//!  │   version (1 byte)       │
//!  │   flags   (1 byte)       │
//!  │   header size (4 B BE)   │
//!  ├──────────────────────────┤
//!  │ PreviousTagSize0 (4 B BE)│  = 0
//!  ├──────────────────────────┤
//!  │ Tag                      │  ┐
//!  │   tag header (11 bytes)  │  │ repeats until
//!  │   tag body  (DataSize B) │  │ EOF
//!  ├──────────────────────────┤  │
//!  │ PreviousTagSize (4 B BE) │  │ = 11 + DataSize of previous tag
//!  ├──────────────────────────┤  │
//!  │ Tag …                    │  ┘
//!  └──────────────────────────┘
//! ```
//!
//! Tag types: **8** = audio, **9** = video, **18** = script (metadata).
//!
//! # Scope of this demuxer
//!
//! This module is a **pure container parser**: it yields raw tag bodies
//! and per-tag codec metadata, but does not decode any video or audio
//! payload. Callers that want to play out the elementary streams must
//! feed them to the appropriate codec decoder (which may itself refuse
//! patent-encumbered payloads per the workspace's codec policy).
//!
//! The format itself, and the parsing thereof, is unencumbered —
//! Adobe placed the FLV spec under a license that explicitly permits
//! implementation.

use std::io::{self, Read};

use thiserror::Error;

/// Errors produced by the FLV parser.
#[derive(Debug, Error)]
pub enum FlvError {
    /// I/O failure while reading the input stream.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// The file header didn't start with the `'FLV'` signature.
    #[error("not an FLV file: signature mismatch (got {0:02X?})")]
    BadSignature([u8; 3]),

    /// The header declared a version this parser doesn't recognize.
    #[error("unsupported FLV version: {0}")]
    UnsupportedVersion(u8),

    /// A tag declared a size larger than what's plausible for FLV
    /// (anything above 16 MiB is almost certainly a parse-resync issue).
    #[error("FLV tag size too large: {0} bytes")]
    TagTooLarge(u32),

    /// A tag's `TagType` byte wasn't one of 8 / 9 / 18.
    #[error("unknown FLV tag type: {0}")]
    UnknownTagType(u8),

    /// An audio tag body's first byte declared a `SoundFormat` value
    /// not enumerated in the FLV spec.
    #[error("invalid FLV audio format code: {0}")]
    InvalidAudioFormat(u8),

    /// A video tag body's first byte declared a `CodecID` value not
    /// enumerated in the FLV spec.
    #[error("invalid FLV video codec id: {0}")]
    InvalidVideoCodec(u8),

    /// A tag body was shorter than the codec-header bytes its tag type
    /// requires.
    #[error("FLV tag body truncated: {0}")]
    BodyTruncated(&'static str),
}

/// Parsed FLV file header (the 9 bytes at offset 0).
#[derive(Debug, Clone, Copy)]
pub struct FlvHeader {
    /// File format version. The spec defines only version `1`.
    pub version: u8,
    /// True if the file contains at least one audio stream.
    pub has_audio: bool,
    /// True if the file contains at least one video stream.
    pub has_video: bool,
}

/// One FLV tag — the unit of container parsing.
#[derive(Debug, Clone)]
pub enum FlvTag {
    /// An audio tag.
    Audio(AudioTag),
    /// A video tag.
    Video(VideoTag),
    /// A script (metadata) tag — typically the `onMetaData` event.
    Script(ScriptTag),
}

impl FlvTag {
    /// Timestamp in milliseconds, common to every tag kind.
    #[must_use]
    pub fn timestamp_ms(&self) -> u32 {
        match self {
            Self::Audio(t) => t.timestamp_ms,
            Self::Video(t) => t.timestamp_ms,
            Self::Script(t) => t.timestamp_ms,
        }
    }
}

/// Decoded audio tag.
#[derive(Debug, Clone)]
pub struct AudioTag {
    /// Presentation timestamp in milliseconds.
    pub timestamp_ms: u32,
    /// Sound codec.
    pub format: AudioFormat,
    /// Nominal sample rate.
    pub rate: SampleRate,
    /// Bit depth per sample.
    pub sample_size: SampleSize,
    /// Mono vs stereo (per the spec — high-resolution codecs override
    /// this in practice).
    pub channels: AudioChannels,
    /// For AAC tags: the AAC packet type byte (0 = sequence header,
    /// 1 = AAC raw). `None` for non-AAC formats.
    pub aac_packet_type: Option<u8>,
    /// Raw codec payload (i.e. the tag body after the audio-header
    /// byte(s)). For AAC: the AudioSpecificConfig (sequence header)
    /// or one raw ADTS-less AAC access unit (raw).
    pub payload: Vec<u8>,
}

/// Decoded video tag.
#[derive(Debug, Clone)]
pub struct VideoTag {
    /// Decode timestamp in milliseconds (DTS).
    pub timestamp_ms: u32,
    /// I / P / B / etc.
    pub frame_type: FrameType,
    /// Video codec ID.
    pub codec: VideoCodec,
    /// For AVC tags: the AVCPacketType byte (0 = AVCDecoderConfigurationRecord,
    /// 1 = NALU, 2 = end-of-sequence). `None` for non-AVC formats.
    pub avc_packet_type: Option<u8>,
    /// Composition-time offset in milliseconds: `pts = dts + composition_time`.
    /// Only non-zero for AVC streams with B-frames; `0` elsewhere.
    pub composition_time_ms: i32,
    /// Raw codec payload (after the codec header byte(s)). For AVC NALU
    /// tags this is the AVCC-framed access unit (4-byte BE length
    /// prefix + NAL data, repeated).
    pub payload: Vec<u8>,
}

/// Script (metadata) tag — raw AMF0/AMF3 bytes.
#[derive(Debug, Clone)]
pub struct ScriptTag {
    /// Presentation timestamp in milliseconds (almost always 0 — script
    /// tags are out of band).
    pub timestamp_ms: u32,
    /// Raw AMF-encoded payload. Parsing AMF is out of scope here;
    /// callers that need `onMetaData` event details should pull
    /// `oximedia-net::rtmp::amf` or an external AMF parser.
    pub data: Vec<u8>,
}

/// FLV `SoundFormat` enumeration (4-bit field, low-order of audio-tag
/// header byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    /// Linear PCM, platform-endian.
    PcmPlatform,
    /// ADPCM.
    Adpcm,
    /// MP3.
    Mp3,
    /// Linear PCM, little-endian.
    PcmLittleEndian,
    /// Nellymoser 16 kHz mono.
    Nellymoser16Mono,
    /// Nellymoser 8 kHz mono.
    Nellymoser8Mono,
    /// Nellymoser.
    Nellymoser,
    /// G.711 A-law.
    G711ALaw,
    /// G.711 mu-law.
    G711MuLaw,
    /// AAC.
    Aac,
    /// Speex.
    Speex,
    /// MP3, 8 kHz.
    Mp38kHz,
    /// Device-specific sound.
    DeviceSpecific,
}

impl AudioFormat {
    fn from_code(code: u8) -> Result<Self, FlvError> {
        Ok(match code {
            0 => Self::PcmPlatform,
            1 => Self::Adpcm,
            2 => Self::Mp3,
            3 => Self::PcmLittleEndian,
            4 => Self::Nellymoser16Mono,
            5 => Self::Nellymoser8Mono,
            6 => Self::Nellymoser,
            7 => Self::G711ALaw,
            8 => Self::G711MuLaw,
            10 => Self::Aac,
            11 => Self::Speex,
            14 => Self::Mp38kHz,
            15 => Self::DeviceSpecific,
            other => return Err(FlvError::InvalidAudioFormat(other)),
        })
    }
}

/// FLV audio nominal sample rate (2-bit field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleRate {
    /// 5.5 kHz.
    Rate5512,
    /// 11 kHz.
    Rate11025,
    /// 22 kHz.
    Rate22050,
    /// 44 kHz.
    Rate44100,
}

impl SampleRate {
    fn from_bits(bits: u8) -> Self {
        match bits & 0x3 {
            0 => Self::Rate5512,
            1 => Self::Rate11025,
            2 => Self::Rate22050,
            _ => Self::Rate44100,
        }
    }

    /// Sample rate in Hz.
    #[must_use]
    pub fn hz(self) -> u32 {
        match self {
            Self::Rate5512 => 5512,
            Self::Rate11025 => 11_025,
            Self::Rate22050 => 22_050,
            Self::Rate44100 => 44_100,
        }
    }
}

/// FLV audio sample-size flag (1-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleSize {
    /// 8-bit samples.
    Bits8,
    /// 16-bit samples.
    Bits16,
}

/// FLV audio channel layout (1-bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioChannels {
    /// Mono.
    Mono,
    /// Stereo.
    Stereo,
}

/// FLV video frame type (high 4 bits of video-tag header byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Keyframe / IDR (1).
    Key,
    /// Inter-coded frame (2).
    Inter,
    /// Disposable (H.263 only, type 3).
    Disposable,
    /// Generated keyframe (type 4).
    GeneratedKey,
    /// Video info / command frame (type 5).
    Info,
}

impl FrameType {
    fn from_bits(bits: u8) -> Self {
        match bits & 0xF {
            1 => Self::Key,
            2 => Self::Inter,
            3 => Self::Disposable,
            4 => Self::GeneratedKey,
            _ => Self::Info,
        }
    }
}

/// FLV video codec id (low 4 bits of video-tag header byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// JPEG (codec id 1) — reserved in spec, never produced in practice.
    Jpeg,
    /// Sorenson H.263 (codec id 2).
    SorensonH263,
    /// Screen video (codec id 3).
    ScreenVideo,
    /// On2 VP6 (codec id 4).
    Vp6,
    /// On2 VP6 with alpha channel (codec id 5).
    Vp6Alpha,
    /// Screen video v2 (codec id 6).
    ScreenVideo2,
    /// AVC / H.264 (codec id 7).
    Avc,
}

impl VideoCodec {
    fn from_code(code: u8) -> Result<Self, FlvError> {
        Ok(match code {
            1 => Self::Jpeg,
            2 => Self::SorensonH263,
            3 => Self::ScreenVideo,
            4 => Self::Vp6,
            5 => Self::Vp6Alpha,
            6 => Self::ScreenVideo2,
            7 => Self::Avc,
            other => return Err(FlvError::InvalidVideoCodec(other)),
        })
    }
}

/// Streaming FLV demuxer over any `Read` source.
///
/// Construct with [`Self::new`], call [`Self::read_header`] once to pick
/// up the 9-byte file header, then call [`Self::next_tag`] in a loop to
/// pull tags out until it returns `Ok(None)` (EOF).
pub struct FlvDemuxer<R: Read> {
    src: R,
    header_read: bool,
}

impl<R: Read> FlvDemuxer<R> {
    /// Wrap a `Read` source. Does no I/O yet.
    pub fn new(src: R) -> Self {
        Self {
            src,
            header_read: false,
        }
    }

    /// Read and parse the 9-byte file header plus the leading
    /// `PreviousTagSize0` word. Must be called exactly once before the
    /// first `next_tag`.
    pub fn read_header(&mut self) -> Result<FlvHeader, FlvError> {
        if self.header_read {
            return Err(FlvError::BodyTruncated("header already consumed"));
        }
        let mut buf = [0u8; 9];
        self.src.read_exact(&mut buf)?;
        if &buf[..3] != b"FLV" {
            return Err(FlvError::BadSignature([buf[0], buf[1], buf[2]]));
        }
        let version = buf[3];
        if version != 1 {
            return Err(FlvError::UnsupportedVersion(version));
        }
        let flags = buf[4];
        // Per spec: bit 0 = has video, bit 2 = has audio.
        let has_video = flags & 0x01 != 0;
        let has_audio = flags & 0x04 != 0;
        let header_size = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]);
        if header_size != 9 {
            return Err(FlvError::BodyTruncated(
                "FLV header size must be 9 in version-1 files",
            ));
        }
        // Skip PreviousTagSize0.
        let mut prev = [0u8; 4];
        self.src.read_exact(&mut prev)?;
        self.header_read = true;
        Ok(FlvHeader {
            version,
            has_audio,
            has_video,
        })
    }

    /// Read the next tag from the stream.
    ///
    /// Returns `Ok(None)` at clean EOF. Bubbles `Ok(Some(_))` for every
    /// successfully parsed tag (including unknown sound formats — those
    /// surface as `Err` instead).
    pub fn next_tag(&mut self) -> Result<Option<FlvTag>, FlvError> {
        if !self.header_read {
            return Err(FlvError::BodyTruncated(
                "must call read_header before next_tag",
            ));
        }
        // Try to read the 11-byte tag header. A 0-byte read at this
        // boundary is clean EOF; a short read is corruption.
        let mut hdr = [0u8; 11];
        match self.src.read(&mut hdr[..1])? {
            0 => return Ok(None),
            _ => {}
        }
        self.src.read_exact(&mut hdr[1..])?;

        let tag_type = hdr[0];
        let data_size = u32::from_be_bytes([0, hdr[1], hdr[2], hdr[3]]);
        let timestamp_lo = u32::from_be_bytes([0, hdr[4], hdr[5], hdr[6]]);
        let timestamp_ext = u32::from(hdr[7]);
        let timestamp_ms = (timestamp_ext << 24) | timestamp_lo;
        // hdr[8..11] is StreamID, always 0 — skipped.

        // The 24-bit DataSize field maxes at 16 MiB - 1. A real-world tag
        // larger than ~8 MiB almost certainly indicates we've lost frame
        // sync; treat that as a hard error.
        if data_size > 8 * 1024 * 1024 {
            return Err(FlvError::TagTooLarge(data_size));
        }

        let mut body = vec![0u8; data_size as usize];
        self.src.read_exact(&mut body)?;

        // Skip the trailing PreviousTagSize word.
        let mut trailer = [0u8; 4];
        self.src.read_exact(&mut trailer)?;

        let tag = match tag_type {
            8 => FlvTag::Audio(parse_audio_tag(timestamp_ms, &body)?),
            9 => FlvTag::Video(parse_video_tag(timestamp_ms, &body)?),
            18 => FlvTag::Script(ScriptTag {
                timestamp_ms,
                data: body,
            }),
            other => return Err(FlvError::UnknownTagType(other)),
        };
        Ok(Some(tag))
    }
}

fn parse_audio_tag(timestamp_ms: u32, body: &[u8]) -> Result<AudioTag, FlvError> {
    if body.is_empty() {
        return Err(FlvError::BodyTruncated("audio tag header byte"));
    }
    let h = body[0];
    let format = AudioFormat::from_code(h >> 4)?;
    let rate = SampleRate::from_bits((h >> 2) & 0x3);
    let sample_size = if h & 0x02 != 0 {
        SampleSize::Bits16
    } else {
        SampleSize::Bits8
    };
    let channels = if h & 0x01 != 0 {
        AudioChannels::Stereo
    } else {
        AudioChannels::Mono
    };

    let (aac_packet_type, payload_start) = if format == AudioFormat::Aac {
        if body.len() < 2 {
            return Err(FlvError::BodyTruncated("AAC tag packet-type byte"));
        }
        (Some(body[1]), 2)
    } else {
        (None, 1)
    };

    Ok(AudioTag {
        timestamp_ms,
        format,
        rate,
        sample_size,
        channels,
        aac_packet_type,
        payload: body[payload_start..].to_vec(),
    })
}

fn parse_video_tag(timestamp_ms: u32, body: &[u8]) -> Result<VideoTag, FlvError> {
    if body.is_empty() {
        return Err(FlvError::BodyTruncated("video tag header byte"));
    }
    let h = body[0];
    let frame_type = FrameType::from_bits(h >> 4);
    let codec = VideoCodec::from_code(h & 0x0F)?;

    let (avc_packet_type, composition_time_ms, payload_start) = if codec == VideoCodec::Avc {
        if body.len() < 5 {
            return Err(FlvError::BodyTruncated("AVC tag header (packet-type + CTS)"));
        }
        let packet_type = body[1];
        // CompositionTime is a signed 24-bit big-endian integer
        // representing PTS - DTS in milliseconds. Sign-extend manually.
        let raw =
            (u32::from(body[2]) << 16) | (u32::from(body[3]) << 8) | u32::from(body[4]);
        let signed = if raw & 0x0080_0000 != 0 {
            (raw | 0xFF00_0000) as i32
        } else {
            raw as i32
        };
        (Some(packet_type), signed, 5)
    } else {
        (None, 0, 1)
    };

    Ok(VideoTag {
        timestamp_ms,
        frame_type,
        codec,
        avc_packet_type,
        composition_time_ms,
        payload: body[payload_start..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Build an FLV header with the given audio/video flags.
    fn flv_header(has_audio: bool, has_video: bool) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"FLV");
        out.push(1); // version
        let mut flags = 0u8;
        if has_audio {
            flags |= 0x04;
        }
        if has_video {
            flags |= 0x01;
        }
        out.push(flags);
        out.extend_from_slice(&9u32.to_be_bytes()); // header size
        out.extend_from_slice(&0u32.to_be_bytes()); // PreviousTagSize0
        out
    }

    /// Append one tag to the running FLV stream.
    fn append_tag(out: &mut Vec<u8>, tag_type: u8, timestamp_ms: u32, body: &[u8]) {
        let data_size = body.len() as u32;
        out.push(tag_type);
        out.push((data_size >> 16) as u8);
        out.push((data_size >> 8) as u8);
        out.push(data_size as u8);
        // Timestamp split: low 24 + extended high 8.
        out.push((timestamp_ms >> 16) as u8);
        out.push((timestamp_ms >> 8) as u8);
        out.push(timestamp_ms as u8);
        out.push((timestamp_ms >> 24) as u8);
        out.extend_from_slice(&[0u8; 3]); // StreamID
        out.extend_from_slice(body);
        let total = 11 + data_size;
        out.extend_from_slice(&total.to_be_bytes());
    }

    #[test]
    fn header_parses_flags() {
        let bytes = flv_header(true, true);
        let mut d = FlvDemuxer::new(Cursor::new(bytes));
        let h = d.read_header().unwrap();
        assert_eq!(h.version, 1);
        assert!(h.has_audio);
        assert!(h.has_video);
    }

    #[test]
    fn header_audio_only() {
        let bytes = flv_header(true, false);
        let h = FlvDemuxer::new(Cursor::new(bytes))
            .read_header()
            .unwrap();
        assert!(h.has_audio);
        assert!(!h.has_video);
    }

    #[test]
    fn bad_signature_errors() {
        let mut bytes = flv_header(true, true);
        bytes[0] = b'X';
        let err = FlvDemuxer::new(Cursor::new(bytes))
            .read_header()
            .unwrap_err();
        assert!(matches!(err, FlvError::BadSignature(_)));
    }

    #[test]
    fn unsupported_version_errors() {
        let mut bytes = flv_header(true, true);
        bytes[3] = 99;
        let err = FlvDemuxer::new(Cursor::new(bytes))
            .read_header()
            .unwrap_err();
        assert!(matches!(err, FlvError::UnsupportedVersion(99)));
    }

    #[test]
    fn aac_audio_tag_round_trip() {
        let mut stream = flv_header(true, false);
        // AAC tag: format=10, rate=3 (44.1k), size=1 (16-bit), channels=1 (stereo)
        // → header byte = (10<<4) | (3<<2) | (1<<1) | 1 = 0xAF
        // packet_type = 0 (sequence header), payload = b"CFG"
        let mut body = vec![0xAF, 0x00];
        body.extend_from_slice(b"CFG");
        append_tag(&mut stream, 8, 0, &body);

        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        let tag = d.next_tag().unwrap().expect("audio tag");
        match tag {
            FlvTag::Audio(a) => {
                assert_eq!(a.format, AudioFormat::Aac);
                assert_eq!(a.rate, SampleRate::Rate44100);
                assert_eq!(a.sample_size, SampleSize::Bits16);
                assert_eq!(a.channels, AudioChannels::Stereo);
                assert_eq!(a.aac_packet_type, Some(0));
                assert_eq!(a.payload, b"CFG");
                assert_eq!(a.timestamp_ms, 0);
            }
            other => panic!("expected Audio, got {other:?}"),
        }
        // Clean EOF after the one tag.
        assert!(d.next_tag().unwrap().is_none());
    }

    #[test]
    fn avc_video_tag_round_trip_with_composition_time() {
        let mut stream = flv_header(false, true);
        // Video header: frame=1 (key), codec=7 (AVC) → 0x17
        // AVCPacketType=1 (NALU), CompositionTime=42 ms
        let mut body = vec![0x17, 0x01, 0x00, 0x00, 0x2A];
        // AVCC: 4-byte length prefix + minimal NAL data.
        body.extend_from_slice(&5u32.to_be_bytes());
        body.extend_from_slice(b"NALU.");
        append_tag(&mut stream, 9, 3000, &body);

        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        let tag = d.next_tag().unwrap().expect("video tag");
        match tag {
            FlvTag::Video(v) => {
                assert_eq!(v.frame_type, FrameType::Key);
                assert_eq!(v.codec, VideoCodec::Avc);
                assert_eq!(v.avc_packet_type, Some(1));
                assert_eq!(v.composition_time_ms, 42);
                assert_eq!(v.timestamp_ms, 3000);
                assert_eq!(v.payload.len(), 9); // 4-byte length + 5-byte NAL
            }
            other => panic!("expected Video, got {other:?}"),
        }
    }

    #[test]
    fn avc_negative_composition_time_sign_extends() {
        let mut stream = flv_header(false, true);
        // -100 as 24-bit two's complement = 0xFFFF9C.
        let mut body = vec![0x27, 0x01, 0xFF, 0xFF, 0x9C];
        body.push(0xAA);
        append_tag(&mut stream, 9, 0, &body);

        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        if let FlvTag::Video(v) = d.next_tag().unwrap().unwrap() {
            assert_eq!(v.composition_time_ms, -100);
        } else {
            panic!("expected Video");
        }
    }

    #[test]
    fn script_tag_passes_through_payload() {
        let mut stream = flv_header(true, true);
        let amf = b"\x02\x00\x0BonMetaData";
        append_tag(&mut stream, 18, 0, amf);

        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        if let FlvTag::Script(s) = d.next_tag().unwrap().unwrap() {
            assert_eq!(s.data, amf);
        } else {
            panic!("expected Script");
        }
    }

    #[test]
    fn timestamp_extended_byte_takes_effect() {
        let mut stream = flv_header(true, false);
        // Pick a timestamp > 24 bits → 0x0123_4567 (~19 hours).
        let big_ts: u32 = 0x0123_4567;
        let mut body = vec![0xAF, 0x01];
        body.extend_from_slice(b"x");
        append_tag(&mut stream, 8, big_ts, &body);

        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        let t = d.next_tag().unwrap().unwrap();
        assert_eq!(t.timestamp_ms(), big_ts);
    }

    #[test]
    fn multiple_tags_in_sequence() {
        let mut stream = flv_header(true, true);
        // audio, then video, then script.
        append_tag(&mut stream, 8, 10, &[0xAF, 0x01, b'a']);
        append_tag(&mut stream, 9, 20, &[0x17, 0x01, 0, 0, 0, b'v']);
        append_tag(&mut stream, 18, 30, b"meta");

        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        let t1 = d.next_tag().unwrap().unwrap();
        let t2 = d.next_tag().unwrap().unwrap();
        let t3 = d.next_tag().unwrap().unwrap();
        assert!(matches!(t1, FlvTag::Audio(_)));
        assert!(matches!(t2, FlvTag::Video(_)));
        assert!(matches!(t3, FlvTag::Script(_)));
        assert_eq!(t1.timestamp_ms(), 10);
        assert_eq!(t2.timestamp_ms(), 20);
        assert_eq!(t3.timestamp_ms(), 30);
        assert!(d.next_tag().unwrap().is_none());
    }

    #[test]
    fn unknown_tag_type_errors() {
        let mut stream = flv_header(true, true);
        append_tag(&mut stream, 99, 0, b"oops");
        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        let err = d.next_tag().unwrap_err();
        assert!(matches!(err, FlvError::UnknownTagType(99)));
    }

    #[test]
    fn tag_too_large_errors() {
        // Hand-craft a tag header that declares an absurdly large body.
        let mut stream = flv_header(true, false);
        stream.push(8); // audio
        stream.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // data_size = 0xFFFFFF (16 MiB)
        stream.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]); // ts + streamid
        let mut d = FlvDemuxer::new(Cursor::new(stream));
        d.read_header().unwrap();
        let err = d.next_tag().unwrap_err();
        assert!(matches!(err, FlvError::TagTooLarge(_)));
    }

    #[test]
    fn next_tag_before_header_errors() {
        let mut d = FlvDemuxer::new(Cursor::new(Vec::<u8>::new()));
        assert!(d.next_tag().is_err());
    }

    #[test]
    fn audio_format_codes_round_trip() {
        for (code, expected) in [
            (0, AudioFormat::PcmPlatform),
            (2, AudioFormat::Mp3),
            (10, AudioFormat::Aac),
            (11, AudioFormat::Speex),
        ] {
            assert_eq!(AudioFormat::from_code(code).unwrap(), expected);
        }
        assert!(AudioFormat::from_code(13).is_err());
    }

    #[test]
    fn sample_rate_hz_values() {
        assert_eq!(SampleRate::Rate5512.hz(), 5512);
        assert_eq!(SampleRate::Rate11025.hz(), 11_025);
        assert_eq!(SampleRate::Rate22050.hz(), 22_050);
        assert_eq!(SampleRate::Rate44100.hz(), 44_100);
    }
}
