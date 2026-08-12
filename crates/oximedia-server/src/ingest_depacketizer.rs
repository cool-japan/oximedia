//! RTMP ingest depacketizer — Enhanced-RTMP tag bodies to elementary samples.
//!
//! Live ingest arrives here as opaque FLV tag bodies inside
//! [`oximedia_net::rtmp::MediaPacket`]s. Before any of it can be handed to a real container muxer
//! (MPEG-TS for HLS, CMAF/fMP4 for DASH) two things have to happen:
//!
//! 1. the tag body has to be parsed into a codec identity plus an *elementary*
//!    payload (the codec bitstream, with the FLV framing stripped), and
//! 2. the codec's decoder-configuration record has to be captured out of the
//!    stream's *sequence header* and converted into the exact byte layout the
//!    target container expects.
//!
//! # Patent policy (Red List)
//!
//! OxiMedia's container muxers accept AV1, VP9, VP8, Opus, FLAC and PCM only.
//! H.264/AVC and AAC are on the **Red List** — never supported, at any layer.
//! Legacy FLV tags carry exactly those two codecs, so the only real ingest path
//! is **Enhanced RTMP** (E-RTMP), whose FourCC-tagged headers can name AV1
//! (`av01`), VP9 (`vp09`), Opus (`Opus`) and FLAC (`fLaC`).
//!
//! Legacy tags and non-patent-free E-RTMP FourCCs are refused with an honest,
//! codec-naming error rather than being silently dropped or transcoded.
//!
//! # Enhanced-RTMP header dispatch (repo-specific)
//!
//! Published Enhanced RTMP places the `isExVideoHeader` marker at **bit 7** of
//! the first tag byte. [`oximedia_net::rtmp::enhanced`] — the decoder this
//! module must feed — instead encodes and decodes that marker at **bit 3**
//! (`0x08`, see `EnhancedVideoTag::encode`/`decode`), and requires the literal
//! byte `0x9F` to open an enhanced *audio* tag. Dispatch here deliberately
//! matches that crate's encoder/decoder pair, because a bit-7 test would route
//! every tag this workspace produces (e.g. `0x19` for an AV1 keyframe) into the
//! legacy branch and misreport it as H.264. See the module-level note in the
//! final report: reconciling `enhanced.rs` with the published bit position is a
//! separate, cross-crate change.

use crate::error::{ServerError, ServerResult};
use bytes::Bytes;
use oximedia_core::CodecId;
use oximedia_net::rtmp::{
    EnhancedAudioPacketType, EnhancedAudioTag, EnhancedVideoPacketType, EnhancedVideoTag, FourCC,
    MediaPacket, MediaPacketType,
};

/// Bit mask selecting the enhanced-header marker in a video tag's first byte,
/// as encoded and decoded by [`oximedia_net::rtmp::enhanced`].
const VIDEO_EX_HEADER_MASK: u8 = 0x08;

/// Literal first byte of an enhanced audio tag, as required by
/// [`EnhancedAudioTag::decode`].
const AUDIO_EX_HEADER_BYTE: u8 = 0x9F;

/// Legacy FLV video codec id for H.264/AVC (Red List).
const LEGACY_VIDEO_CODEC_AVC: u8 = 7;

/// Legacy FLV sound format for AAC (Red List).
const LEGACY_AUDIO_FORMAT_AAC: u8 = 10;

/// Size in bytes of a FLAC `STREAMINFO` metadata block payload.
const FLAC_STREAMINFO_LEN: usize = 34;

/// Minimum length of an RFC 7845 `OpusHead` identification header
/// (magic + version + channels + pre-skip + rate + gain + mapping family).
const OPUS_HEAD_MIN_LEN: usize = 19;

// ─── Codec identity ───────────────────────────────────────────────────────────

/// A patent-free codec that this server accepts on live ingest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IngestCodec {
    /// AV1 video.
    Av1,
    /// VP9 video.
    Vp9,
    /// Opus audio.
    Opus,
    /// FLAC audio.
    Flac,
}

impl IngestCodec {
    /// Returns the container-level codec identifier.
    #[must_use]
    pub const fn codec_id(self) -> CodecId {
        match self {
            Self::Av1 => CodecId::Av1,
            Self::Vp9 => CodecId::Vp9,
            Self::Opus => CodecId::Opus,
            Self::Flac => CodecId::Flac,
        }
    }

    /// Returns the ISO-BMFF sample-entry FourCC for this codec.
    ///
    /// These are the codes [`oximedia_container::mux::CmafMuxer`] maps to the
    /// codec-configuration box names `av1C` / `vpcC` / `dOps` / `dfLa`.
    #[must_use]
    pub const fn sample_entry_fourcc(self) -> [u8; 4] {
        match self {
            Self::Av1 => *b"av01",
            Self::Vp9 => *b"vp09",
            Self::Opus => *b"Opus",
            Self::Flac => *b"fLaC",
        }
    }

    /// Returns the human-readable codec name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Av1 => "AV1",
            Self::Vp9 => "VP9",
            Self::Opus => "Opus",
            Self::Flac => "FLAC",
        }
    }

    /// Returns `true` for video codecs.
    #[must_use]
    pub const fn is_video(self) -> bool {
        matches!(self, Self::Av1 | Self::Vp9)
    }

    /// Maps an Enhanced-RTMP FourCC onto an ingest codec.
    ///
    /// Returns `None` for FourCCs that are not on the patent-free allowlist.
    #[must_use]
    fn from_fourcc(codec: FourCC) -> Option<Self> {
        match codec {
            FourCC::AV1 => Some(Self::Av1),
            FourCC::VP9 => Some(Self::Vp9),
            FourCC::OPUS => Some(Self::Opus),
            FourCC::FLAC => Some(Self::Flac),
            _ => None,
        }
    }
}

/// Whether a sample belongs to the video or the audio elementary stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleKind {
    /// Video elementary stream.
    Video,
    /// Audio elementary stream.
    Audio,
}

// ─── Stream configuration ─────────────────────────────────────────────────────

/// Decoder configuration captured from an ingest stream's sequence header.
///
/// [`Self::config_record`] is **container-ready**: it is exactly the payload
/// that belongs inside the codec-configuration box of an ISO-BMFF sample entry
/// (`av1C`, `vpcC`, `dOps`, `dfLa`), including the `FullBox` version/flags
/// prefix for the two boxes that are full boxes. See
/// [`Depacketizer`] for the per-codec conversion rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamConfig {
    /// Codec this configuration describes.
    pub codec: IngestCodec,
    /// Container-ready decoder-configuration record.
    pub config_record: Vec<u8>,
    /// Coded width in pixels (video only, when derivable).
    pub width: Option<u32>,
    /// Coded height in pixels (video only, when derivable).
    pub height: Option<u32>,
    /// Sample rate in Hz (audio only).
    pub sample_rate: Option<u32>,
    /// Channel count (audio only).
    pub channels: Option<u8>,
}

impl StreamConfig {
    /// Returns the ISO-BMFF track timescale to use for this stream.
    ///
    /// Video uses the 90 kHz MPEG clock; audio uses its own sample rate, which
    /// is what every real fMP4 packager does so that sample durations land on
    /// exact integers.
    #[must_use]
    pub fn timescale(&self) -> u32 {
        if self.codec.is_video() {
            90_000
        } else {
            self.sample_rate.unwrap_or(48_000)
        }
    }
}

/// One depacketized elementary sample, ready for a container muxer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementarySample {
    /// Codec that produced this sample.
    pub codec: IngestCodec,
    /// Which elementary stream this sample belongs to.
    pub kind: SampleKind,
    /// Elementary bitstream payload (FLV framing removed).
    pub data: Bytes,
    /// Decode timestamp in milliseconds, as carried by RTMP.
    pub dts_ms: u32,
    /// Composition-time offset in milliseconds (`pts = dts + this`).
    pub composition_time_ms: i32,
    /// Whether this sample is a sync (key) frame.
    pub keyframe: bool,
}

// ─── Depacketizer ─────────────────────────────────────────────────────────────

/// Stateful RTMP ingest depacketizer.
///
/// Feed every published [`MediaPacket`] through [`Self::push`]. Sequence
/// headers are absorbed into the per-stream [`StreamConfig`] and yield
/// `Ok(None)`; coded frames yield `Ok(Some(sample))`. Anything that is not a
/// patent-free Enhanced-RTMP stream yields an honest, codec-naming error.
#[derive(Debug, Default)]
pub struct Depacketizer {
    /// Video configuration, once its sequence header has been seen.
    video: Option<StreamConfig>,
    /// Audio configuration, once its sequence header has been seen.
    audio: Option<StreamConfig>,
}

impl Depacketizer {
    /// Creates an empty depacketizer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns `true` once at least one elementary stream has been configured.
    ///
    /// A muxer cannot emit anything before this is true: without a decoder
    /// configuration record there is no honest sample entry to write.
    #[must_use]
    pub const fn ready(&self) -> bool {
        self.video.is_some() || self.audio.is_some()
    }

    /// Returns the captured video configuration, if any.
    #[must_use]
    pub const fn video_config(&self) -> Option<&StreamConfig> {
        self.video.as_ref()
    }

    /// Returns the captured audio configuration, if any.
    #[must_use]
    pub const fn audio_config(&self) -> Option<&StreamConfig> {
        self.audio.as_ref()
    }

    /// Absorbs *only* sequence-header packets, ignoring everything else.
    ///
    /// This exists so a caller that buffers packets for later muxing can learn
    /// the stream configuration as packets arrive, without consuming coded
    /// frames. Because sequence headers produce no samples, replaying the same
    /// packet through [`Self::push`] afterwards is idempotent.
    ///
    /// # Errors
    ///
    /// Returns the same honest errors as [`Self::push`] when the packet is a
    /// sequence header for a Red List or non-patent-free codec. Coded frames
    /// and undecodable bodies are ignored (`Ok(())`) — [`Self::push`] is the
    /// place that reports on those.
    pub fn observe_config(&mut self, packet: &MediaPacket) -> ServerResult<()> {
        match packet.packet_type {
            MediaPacketType::Video => {
                if !is_enhanced_video(&packet.data) {
                    // Legacy tags are still worth refusing loudly: a publisher
                    // sending AVC will never become decodable.
                    return legacy_video_check(&packet.data);
                }
                let tag = decode_enhanced_video(&packet.data)?;
                if tag.packet_type == EnhancedVideoPacketType::SequenceStart {
                    self.absorb_video_sequence_start(&tag)?;
                }
                Ok(())
            }
            MediaPacketType::Audio => {
                if !is_enhanced_audio(&packet.data) {
                    return legacy_audio_check(&packet.data);
                }
                let tag = decode_enhanced_audio(&packet.data)?;
                if tag.packet_type == EnhancedAudioPacketType::SequenceStart {
                    self.absorb_audio_sequence_start(&tag)?;
                }
                Ok(())
            }
            MediaPacketType::Data => Ok(()),
        }
    }

    /// Depacketizes one ingest packet.
    ///
    /// Returns `Ok(None)` when the packet carried no elementary sample — a
    /// sequence header (absorbed into the configuration), a sequence end, a
    /// metadata tag, or an AMF data packet.
    ///
    /// # Errors
    ///
    /// * legacy FLV H.264/AAC tags — refused, naming the codec and the Red List
    /// * Enhanced-RTMP FourCCs outside the patent-free set (e.g. `hvc1`,
    ///   `mp4a`) — refused, naming the codec
    /// * malformed tag bodies, or coded frames arriving before their sequence
    ///   header — refused with a precise description
    pub fn push(&mut self, packet: &MediaPacket) -> ServerResult<Option<ElementarySample>> {
        match packet.packet_type {
            MediaPacketType::Video => self.push_video(packet),
            MediaPacketType::Audio => self.push_audio(packet),
            // AMF `onMetaData` / `@setDataFrame` carry no elementary bitstream.
            MediaPacketType::Data => Ok(None),
        }
    }

    /// Depacketizes a video tag body.
    fn push_video(&mut self, packet: &MediaPacket) -> ServerResult<Option<ElementarySample>> {
        if !is_enhanced_video(&packet.data) {
            legacy_video_check(&packet.data)?;
            // `legacy_video_check` only returns `Ok` for bodies it could not
            // identify at all; those are equally unusable.
            return Err(ServerError::UnsupportedMediaType(format!(
                "unrecognised FLV video tag body (first byte {:#04x}): not an Enhanced-RTMP \
                 tag and not a known legacy codec id; OxiMedia ingest accepts Enhanced RTMP \
                 av01/vp09 only",
                packet.data.first().copied().unwrap_or(0)
            )));
        }

        let tag = decode_enhanced_video(&packet.data)?;

        match tag.packet_type {
            EnhancedVideoPacketType::SequenceStart => {
                self.absorb_video_sequence_start(&tag)?;
                Ok(None)
            }
            EnhancedVideoPacketType::CodedFrames | EnhancedVideoPacketType::CodedFramesX => {
                let codec = require_patent_free_video(tag.codec)?;
                let config = self.video.as_ref().ok_or_else(|| {
                    ServerError::BadRequest(format!(
                        "{} coded frame arrived before its Enhanced-RTMP sequence header; \
                         no decoder configuration record is available to build a sample entry",
                        codec.name()
                    ))
                })?;
                if config.codec != codec {
                    return Err(ServerError::BadRequest(format!(
                        "video codec changed mid-stream from {} to {}; \
                         mid-stream codec switching is not supported on ingest",
                        config.codec.name(),
                        codec.name()
                    )));
                }
                let keyframe = tag.frame_type.is_keyframe();
                // VP9 carries its frame size in the keyframe's uncompressed
                // header, not in `vpcC`; AV1 may repeat its sequence header in
                // the temporal unit. Learn real dimensions from the first
                // keyframe that yields them, and never invent any.
                if keyframe {
                    self.learn_video_dimensions(codec, &tag.data);
                }
                Ok(Some(ElementarySample {
                    codec,
                    kind: SampleKind::Video,
                    data: tag.data.clone(),
                    dts_ms: packet.timestamp,
                    composition_time_ms: if tag.packet_type == EnhancedVideoPacketType::CodedFramesX
                    {
                        // `CodedFramesX` is defined precisely as "no composition
                        // time offset present"; it is always zero.
                        0
                    } else {
                        tag.composition_time_ms
                    },
                    keyframe,
                }))
            }
            // Sequence end and HDR/colour metadata carry no coded samples.
            EnhancedVideoPacketType::SequenceEnd | EnhancedVideoPacketType::Metadata => Ok(None),
            EnhancedVideoPacketType::Mpeg2TsSequenceStart => {
                Err(ServerError::UnsupportedMediaType(format!(
                    "Enhanced-RTMP MPEG-2 TS sequence start for codec '{}' is not supported \
                     on ingest; publish a plain sequence start instead",
                    tag.codec
                )))
            }
        }
    }

    /// Depacketizes an audio tag body.
    fn push_audio(&mut self, packet: &MediaPacket) -> ServerResult<Option<ElementarySample>> {
        if !is_enhanced_audio(&packet.data) {
            legacy_audio_check(&packet.data)?;
            return Err(ServerError::UnsupportedMediaType(format!(
                "unrecognised FLV audio tag body (first byte {:#04x}): not an Enhanced-RTMP \
                 tag and not a known legacy sound format; OxiMedia ingest accepts Enhanced \
                 RTMP Opus/fLaC only",
                packet.data.first().copied().unwrap_or(0)
            )));
        }

        let tag = decode_enhanced_audio(&packet.data)?;

        match tag.packet_type {
            EnhancedAudioPacketType::SequenceStart => {
                self.absorb_audio_sequence_start(&tag)?;
                Ok(None)
            }
            EnhancedAudioPacketType::CodedFrames => {
                let codec = require_patent_free_audio(tag.codec)?;
                let config = self.audio.as_ref().ok_or_else(|| {
                    ServerError::BadRequest(format!(
                        "{} coded frame arrived before its Enhanced-RTMP sequence header; \
                         no decoder configuration record is available to build a sample entry",
                        codec.name()
                    ))
                })?;
                if config.codec != codec {
                    return Err(ServerError::BadRequest(format!(
                        "audio codec changed mid-stream from {} to {}; \
                         mid-stream codec switching is not supported on ingest",
                        config.codec.name(),
                        codec.name()
                    )));
                }
                Ok(Some(ElementarySample {
                    codec,
                    kind: SampleKind::Audio,
                    data: tag.data.clone(),
                    dts_ms: packet.timestamp,
                    composition_time_ms: 0,
                    // Every Opus/FLAC packet is independently decodable.
                    keyframe: true,
                }))
            }
            EnhancedAudioPacketType::SequenceEnd => Ok(None),
            EnhancedAudioPacketType::MultichannelConfig => {
                Err(ServerError::UnsupportedMediaType(format!(
                    "Enhanced-RTMP multichannel configuration for codec '{}' is not supported \
                     on ingest; the channel layout must be carried in the codec's own \
                     sequence header",
                    tag.codec
                )))
            }
            EnhancedAudioPacketType::Multitrack => Err(ServerError::UnsupportedMediaType(format!(
                "Enhanced-RTMP multitrack audio (codec '{}') is not supported on ingest; \
                     publish a single audio track per stream",
                tag.codec
            ))),
        }
    }

    /// Captures a video sequence header into [`Self::video`].
    fn absorb_video_sequence_start(&mut self, tag: &EnhancedVideoTag) -> ServerResult<()> {
        let codec = require_patent_free_video(tag.codec)?;
        let config_record = match codec {
            IngestCodec::Av1 => av1_config_record(&tag.data)?,
            IngestCodec::Vp9 => vp9_config_record(&tag.data)?,
            // `require_patent_free_video` only yields video codecs.
            IngestCodec::Opus | IngestCodec::Flac => {
                return Err(ServerError::BadRequest(format!(
                    "audio codec {} announced in a video sequence header",
                    codec.name()
                )))
            }
        };

        let (width, height) = match codec {
            IngestCodec::Av1 => av1_dimensions_from_config(&tag.data).unzip_or_none(),
            // `vpcC` genuinely does not carry frame dimensions; they come from
            // the first keyframe's uncompressed header.
            _ => (None, None),
        };

        self.video = Some(StreamConfig {
            codec,
            config_record,
            width,
            height,
            sample_rate: None,
            channels: None,
        });
        Ok(())
    }

    /// Captures an audio sequence header into [`Self::audio`].
    fn absorb_audio_sequence_start(&mut self, tag: &EnhancedAudioTag) -> ServerResult<()> {
        let codec = require_patent_free_audio(tag.codec)?;
        let (config_record, sample_rate, channels) = match codec {
            IngestCodec::Opus => opus_head_to_dops(&tag.data)?,
            IngestCodec::Flac => flac_config_to_dfla(&tag.data)?,
            IngestCodec::Av1 | IngestCodec::Vp9 => {
                return Err(ServerError::BadRequest(format!(
                    "video codec {} announced in an audio sequence header",
                    codec.name()
                )))
            }
        };

        self.audio = Some(StreamConfig {
            codec,
            config_record,
            width: None,
            height: None,
            sample_rate: Some(sample_rate),
            channels: Some(channels),
        });
        Ok(())
    }

    /// Fills in real frame dimensions from a keyframe, if they are still unknown.
    ///
    /// Never fabricates: a parse failure simply leaves the dimensions `None`,
    /// and the sample itself is still emitted.
    fn learn_video_dimensions(&mut self, codec: IngestCodec, frame: &[u8]) {
        let Some(config) = self.video.as_mut() else {
            return;
        };
        if config.width.is_some() && config.height.is_some() {
            return;
        }
        let dims = match codec {
            IngestCodec::Av1 => av1_dimensions_from_temporal_unit(frame),
            IngestCodec::Vp9 => vp9_dimensions_from_keyframe(frame),
            IngestCodec::Opus | IngestCodec::Flac => None,
        };
        if let Some((w, h)) = dims {
            config.width = Some(w);
            config.height = Some(h);
        }
    }
}

/// Small helper so `Option<(u32, u32)>` can be spread into two `Option`s.
trait UnzipOrNone {
    /// Splits `Some((a, b))` into `(Some(a), Some(b))`, `None` into `(None, None)`.
    fn unzip_or_none(self) -> (Option<u32>, Option<u32>);
}

impl UnzipOrNone for Option<(u32, u32)> {
    fn unzip_or_none(self) -> (Option<u32>, Option<u32>) {
        match self {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        }
    }
}

// ─── Measured segment duration ────────────────────────────────────────────────

/// Measures the presentation span a batch of elementary samples covers, in
/// seconds.
///
/// This is what an HLS `EXTINF` or a DASH segment duration must advertise:
/// a *measured* value derived from the samples actually in the segment, never
/// the packager's nominal target. Advertising the nominal value against real
/// media makes a player's timeline drift away from the media it is decoding.
///
/// The span is `last_dts - first_dts` plus the final sample's own duration,
/// which is estimated from the last measured inter-sample delta — a segment's
/// final sample has no successor to measure against. Video samples define the
/// span when present, since they set the segment cadence; an audio-only
/// segment is measured from its audio samples.
///
/// A batch with fewer than two samples on the measured track has no measurable
/// delta and reports `0.0` rather than inventing a duration.
#[must_use]
pub fn measured_span_seconds(samples: &[ElementarySample]) -> f64 {
    let mut dts: Vec<u32> = samples
        .iter()
        .filter(|s| s.kind == SampleKind::Video)
        .map(|s| s.dts_ms)
        .collect();
    if dts.is_empty() {
        dts = samples
            .iter()
            .filter(|s| s.kind == SampleKind::Audio)
            .map(|s| s.dts_ms)
            .collect();
    }
    if dts.len() < 2 {
        return 0.0;
    }
    dts.sort_unstable();
    let span = dts[dts.len() - 1].saturating_sub(dts[0]);
    // The last sample occupies time too; its duration is unknown, so reuse the
    // last measured delta rather than truncating the segment by one frame.
    let last_delta = dts[dts.len() - 1].saturating_sub(dts[dts.len() - 2]);
    f64::from(span.saturating_add(last_delta)) / 1000.0
}

// ─── Stateless codec sniffing ─────────────────────────────────────────────────

/// Resolves the patent-free ingest codec a packet carries, without any state.
///
/// Intended for callers that need to name the codec in a diagnostic (e.g. the
/// transcode engine's honest per-codec refusal) rather than depacketize.
///
/// # Errors
///
/// Returns the same honest, codec-naming errors as [`Depacketizer::push`] for
/// Red List and non-patent-free inputs, and for bodies that carry no codec at
/// all (AMF data packets).
pub fn sniff_ingest_codec(packet: &MediaPacket) -> ServerResult<IngestCodec> {
    match packet.packet_type {
        MediaPacketType::Video => {
            if !is_enhanced_video(&packet.data) {
                legacy_video_check(&packet.data)?;
                return Err(ServerError::UnsupportedMediaType(format!(
                    "unrecognised FLV video tag body (first byte {:#04x})",
                    packet.data.first().copied().unwrap_or(0)
                )));
            }
            let tag = decode_enhanced_video(&packet.data)?;
            require_patent_free_video(tag.codec)
        }
        MediaPacketType::Audio => {
            if !is_enhanced_audio(&packet.data) {
                legacy_audio_check(&packet.data)?;
                return Err(ServerError::UnsupportedMediaType(format!(
                    "unrecognised FLV audio tag body (first byte {:#04x})",
                    packet.data.first().copied().unwrap_or(0)
                )));
            }
            let tag = decode_enhanced_audio(&packet.data)?;
            require_patent_free_audio(tag.codec)
        }
        MediaPacketType::Data => Err(ServerError::BadRequest(
            "AMF data packet carries no codec".to_string(),
        )),
    }
}

/// Returns `true` when a video tag body opens an Enhanced-RTMP (ex) header.
///
/// Matches [`EnhancedVideoTag::decode`]'s marker position — see the module
/// documentation for why this is bit 3 and not the published bit 7.
#[must_use]
fn is_enhanced_video(body: &[u8]) -> bool {
    body.first().is_some_and(|b| b & VIDEO_EX_HEADER_MASK != 0)
}

/// Returns `true` when an audio tag body opens an Enhanced-RTMP header.
#[must_use]
fn is_enhanced_audio(body: &[u8]) -> bool {
    body.first() == Some(&AUDIO_EX_HEADER_BYTE)
}

/// Decodes an enhanced video tag, or reports precisely why it could not.
fn decode_enhanced_video(body: &[u8]) -> ServerResult<EnhancedVideoTag> {
    EnhancedVideoTag::decode(body).ok_or_else(|| {
        ServerError::BadRequest(format!(
            "malformed Enhanced-RTMP video tag ({} bytes): frame type, packet type or \
             FourCC could not be decoded",
            body.len()
        ))
    })
}

/// Decodes an enhanced audio tag, or reports precisely why it could not.
fn decode_enhanced_audio(body: &[u8]) -> ServerResult<EnhancedAudioTag> {
    EnhancedAudioTag::decode(body).ok_or_else(|| {
        ServerError::BadRequest(format!(
            "malformed Enhanced-RTMP audio tag ({} bytes): packet type or FourCC could \
             not be decoded",
            body.len()
        ))
    })
}

/// Refuses a legacy FLV video tag body, naming its codec.
///
/// Returns `Ok(())` only when the body's codec id is not one this server can
/// name — the caller then reports it as unrecognised.
fn legacy_video_check(body: &[u8]) -> ServerResult<()> {
    let Some(&first) = body.first() else {
        return Err(ServerError::BadRequest(
            "empty FLV video tag body".to_string(),
        ));
    };
    let codec_id = first & 0x0F;
    if codec_id == LEGACY_VIDEO_CODEC_AVC {
        return Err(ServerError::UnsupportedMediaType(
            "legacy FLV video codec id 7 is H.264/AVC, which is on the OxiMedia Red List \
             (NEVER supported: patent-encumbered). Publish AV1 (`av01`) or VP9 (`vp09`) \
             over Enhanced RTMP instead."
                .to_string(),
        ));
    }
    Err(ServerError::UnsupportedMediaType(format!(
        "legacy FLV video codec id {codec_id} is not supported by OxiMedia ingest; the \
         only real path is Enhanced RTMP with a patent-free FourCC (`av01`, `vp09`)"
    )))
}

/// Refuses a legacy FLV audio tag body, naming its sound format.
fn legacy_audio_check(body: &[u8]) -> ServerResult<()> {
    let Some(&first) = body.first() else {
        return Err(ServerError::BadRequest(
            "empty FLV audio tag body".to_string(),
        ));
    };
    let sound_format = first >> 4;
    if sound_format == LEGACY_AUDIO_FORMAT_AAC {
        return Err(ServerError::UnsupportedMediaType(
            "legacy FLV sound format 10 is AAC, which is on the OxiMedia Red List \
             (NEVER supported: patent-encumbered). Publish Opus (`Opus`) or FLAC \
             (`fLaC`) over Enhanced RTMP instead."
                .to_string(),
        ));
    }
    Err(ServerError::UnsupportedMediaType(format!(
        "legacy FLV sound format {sound_format} is not supported by OxiMedia ingest; the \
         only real path is Enhanced RTMP with a patent-free FourCC (`Opus`, `fLaC`)"
    )))
}

/// Maps a video FourCC onto an ingest codec, or refuses it by name.
fn require_patent_free_video(codec: FourCC) -> ServerResult<IngestCodec> {
    match IngestCodec::from_fourcc(codec) {
        Some(c) if c.is_video() => Ok(c),
        Some(c) => Err(ServerError::BadRequest(format!(
            "audio codec {} ('{}') announced on a video tag",
            c.name(),
            codec
        ))),
        None => Err(ServerError::UnsupportedMediaType(format!(
            "Enhanced-RTMP video FourCC '{}' ({}) is not patent-free and is not supported \
             by OxiMedia; H.264/AVC and HEVC are on the Red List (NEVER supported). \
             Publish AV1 (`av01`) or VP9 (`vp09`).",
            codec,
            codec.codec_name()
        ))),
    }
}

/// Maps an audio FourCC onto an ingest codec, or refuses it by name.
fn require_patent_free_audio(codec: FourCC) -> ServerResult<IngestCodec> {
    match IngestCodec::from_fourcc(codec) {
        Some(c) if !c.is_video() => Ok(c),
        Some(c) => Err(ServerError::BadRequest(format!(
            "video codec {} ('{}') announced on an audio tag",
            c.name(),
            codec
        ))),
        None => Err(ServerError::UnsupportedMediaType(format!(
            "Enhanced-RTMP audio FourCC '{}' ({}) is not patent-free and is not supported \
             by OxiMedia; AAC is on the Red List (NEVER supported). Publish Opus (`Opus`) \
             or FLAC (`fLaC`).",
            codec,
            codec.codec_name()
        ))),
    }
}

// ─── AV1: av1C ────────────────────────────────────────────────────────────────

/// Validates an Enhanced-RTMP AV1 sequence-header payload as an `av1C` record.
///
/// `AV1CodecConfigurationBox` is a plain ISO-BMFF `Box` (AV1 Codec ISOBMFF
/// Binding v1.2.0 §2.3), so the E-RTMP payload — an
/// `AV1CodecConfigurationRecord` — is exactly the box payload and passes
/// through verbatim. Only the record's fixed marker/version prefix is checked,
/// so a garbage blob cannot masquerade as a configuration record.
fn av1_config_record(payload: &[u8]) -> ServerResult<Vec<u8>> {
    // marker(1)=1, version(7)=1, seq_profile/level, flags, delay byte.
    if payload.len() < 4 {
        return Err(ServerError::BadRequest(format!(
            "AV1 sequence header is {} bytes; an AV1CodecConfigurationRecord needs at \
             least 4",
            payload.len()
        )));
    }
    let marker = payload[0] >> 7;
    let version = payload[0] & 0x7F;
    if marker != 1 || version != 1 {
        return Err(ServerError::BadRequest(format!(
            "AV1 sequence header is not an AV1CodecConfigurationRecord: marker={marker} \
             version={version} (expected marker=1, version=1)"
        )));
    }
    Ok(payload.to_vec())
}

/// Extracts real frame dimensions from an `av1C` record's `configOBUs`.
fn av1_dimensions_from_config(config: &[u8]) -> Option<(u32, u32)> {
    // The first 4 bytes are the fixed record header; `configOBUs` follow.
    config.get(4..).and_then(av1_dimensions_from_temporal_unit)
}

/// Extracts real frame dimensions from a run of AV1 OBUs.
///
/// Returns `None` whenever no sequence header OBU is present or it does not
/// parse — dimensions are never guessed.
fn av1_dimensions_from_temporal_unit(obus: &[u8]) -> Option<(u32, u32)> {
    use oximedia_codec::av1::SequenceHeader;
    use oximedia_codec::av1_obu::parse_obu_headers;

    for unit in parse_obu_headers(obus) {
        if !unit.is_sequence_header() {
            continue;
        }
        let start = unit.payload_offset as usize;
        let end = start.checked_add(unit.size as usize)?;
        let payload = obus.get(start..end)?;
        if let Ok(seq) = SequenceHeader::parse(payload) {
            return Some((seq.max_frame_width(), seq.max_frame_height()));
        }
    }
    None
}

// ─── VP9: vpcC ────────────────────────────────────────────────────────────────

/// Normalises an Enhanced-RTMP VP9 sequence-header payload into a `vpcC` payload.
///
/// `VPCodecConfigurationBox` is a **`FullBox`** (VP Codec ISO Media File Format
/// Binding, `vpcC` version 1), so the box payload is `version(1) + flags(3) +
/// VPCodecConfigurationRecord`. Publishers differ on whether the E-RTMP payload
/// already carries that 4-byte prefix, so the shape is resolved from the
/// record's own `codecInitializationDataSize` field rather than guessed:
///
/// * bare record — `8 + codecInitializationDataSize == len` → prefix is added
/// * prefixed record — `12 + codecInitializationDataSize == len` → passed through
///
/// Anything else is refused rather than silently reshaped.
fn vp9_config_record(payload: &[u8]) -> ServerResult<Vec<u8>> {
    /// Byte length of a `VPCodecConfigurationRecord` before its
    /// variable-length `codecInitializationData`.
    const RECORD_FIXED_LEN: usize = 8;
    /// Byte length of the `FullBox` version + flags prefix.
    const FULLBOX_PREFIX_LEN: usize = 4;

    // Bare record: codecInitializationDataSize is the last two fixed bytes.
    if payload.len() >= RECORD_FIXED_LEN {
        let size = u16::from_be_bytes([payload[6], payload[7]]) as usize;
        if RECORD_FIXED_LEN + size == payload.len() {
            let mut out = Vec::with_capacity(FULLBOX_PREFIX_LEN + payload.len());
            // vpcC is defined at version 1; flags are always zero.
            out.extend_from_slice(&[1, 0, 0, 0]);
            out.extend_from_slice(payload);
            return Ok(out);
        }
    }

    // Already carries the FullBox prefix.
    if payload.len() >= FULLBOX_PREFIX_LEN + RECORD_FIXED_LEN {
        let size = u16::from_be_bytes([payload[10], payload[11]]) as usize;
        if FULLBOX_PREFIX_LEN + RECORD_FIXED_LEN + size == payload.len() {
            return Ok(payload.to_vec());
        }
    }

    Err(ServerError::BadRequest(format!(
        "VP9 sequence header ({} bytes) is not a VPCodecConfigurationRecord: its \
         codecInitializationDataSize matches neither a bare record nor one carrying the \
         vpcC FullBox version/flags prefix",
        payload.len()
    )))
}

/// Extracts real frame dimensions from a VP9 keyframe's uncompressed header.
///
/// Returns `None` unless the header actually carries a frame size. A
/// `show_existing_frame` header, for instance, parses successfully but encodes
/// no dimensions at all — reporting its zeroed defaults would fabricate a 0×0
/// track.
fn vp9_dimensions_from_keyframe(frame: &[u8]) -> Option<(u32, u32)> {
    use oximedia_codec::vp9::UncompressedHeader;

    let header = UncompressedHeader::parse(frame).ok()?;
    if header.width == 0 || header.height == 0 {
        return None;
    }
    Some((header.width, header.height))
}

// ─── Opus: OpusHead → dOps ────────────────────────────────────────────────────

/// Converts an RFC 7845 `OpusHead` identification header into a `dOps` payload.
///
/// Returns `(dops_payload, sample_rate, channels)`.
///
/// Enhanced RTMP carries the Opus decoder configuration as an `OpusHead`-style
/// blob, but ISO-BMFF wants an `OpusSpecificBox`. The two describe the same
/// fields in **different byte orders and with a different version number**, so
/// this is a real transcription, not a copy:
///
/// | field | `OpusHead` (RFC 7845 §5.1) | `dOps` (Opus-in-ISOBMFF §4.3.2) |
/// |---|---|---|
/// | magic `"OpusHead"` | 8 bytes, present | **absent** |
/// | `Version` | `1` | `0` |
/// | channel count | 1 byte | 1 byte (`OutputChannelCount`) |
/// | `PreSkip` | u16 **little**-endian | u16 **big**-endian |
/// | `InputSampleRate` | u32 **little**-endian | u32 **big**-endian |
/// | `OutputGain` | i16 **little**-endian | i16 **big**-endian |
/// | `ChannelMappingFamily` | 1 byte | 1 byte |
/// | channel mapping table | present iff family ≠ 0 | present iff family ≠ 0 |
///
/// `OpusSpecificBox` is a plain `Box`, so no `FullBox` version/flags prefix is
/// added — the leading `Version = 0` byte above *is* the record's own field.
///
/// The returned sample rate is fixed at **48 000 Hz**: Opus always decodes at
/// 48 kHz, and that — not `InputSampleRate`, which merely records the rate of
/// the material before encoding — is what the ISO-BMFF `AudioSampleEntry` and
/// the track timescale must carry.
fn opus_head_to_dops(head: &[u8]) -> ServerResult<(Vec<u8>, u32, u8)> {
    if head.len() < OPUS_HEAD_MIN_LEN {
        return Err(ServerError::BadRequest(format!(
            "Opus sequence header is {} bytes; an OpusHead identification header needs at \
             least {OPUS_HEAD_MIN_LEN}",
            head.len()
        )));
    }
    if &head[0..8] != b"OpusHead" {
        return Err(ServerError::BadRequest(
            "Opus sequence header does not start with the RFC 7845 `OpusHead` magic; \
             refusing to reinterpret an unknown blob as an OpusSpecificBox"
                .to_string(),
        ));
    }
    // RFC 7845 §5.1: the major version is the top 4 bits and must be 0.
    let major_version = head[8] >> 4;
    if major_version != 0 {
        return Err(ServerError::BadRequest(format!(
            "OpusHead major version {major_version} is not understood (RFC 7845 requires 0)"
        )));
    }

    let channels = head[9];
    if channels == 0 {
        return Err(ServerError::BadRequest(
            "OpusHead declares 0 channels".to_string(),
        ));
    }
    let pre_skip = u16::from_le_bytes([head[10], head[11]]);
    let input_sample_rate = u32::from_le_bytes([head[12], head[13], head[14], head[15]]);
    let output_gain = i16::from_le_bytes([head[16], head[17]]);
    let mapping_family = head[18];

    let mut out = Vec::with_capacity(OPUS_HEAD_MIN_LEN);
    // OpusSpecificBox.Version — always 0, *not* OpusHead's 1.
    out.push(0);
    out.push(channels);
    out.extend_from_slice(&pre_skip.to_be_bytes());
    out.extend_from_slice(&input_sample_rate.to_be_bytes());
    out.extend_from_slice(&output_gain.to_be_bytes());
    out.push(mapping_family);

    if mapping_family != 0 {
        // ChannelMappingTable: StreamCount, CoupledCount, ChannelMapping[channels].
        let table_len = 2 + channels as usize;
        let table = head.get(19..19 + table_len).ok_or_else(|| {
            ServerError::BadRequest(format!(
                "OpusHead declares channel mapping family {mapping_family} but carries no \
                 {table_len}-byte channel mapping table for {channels} channels"
            ))
        })?;
        out.extend_from_slice(table);
    }

    // Opus is always decoded at 48 kHz regardless of InputSampleRate.
    Ok((out, 48_000, channels))
}

// ─── FLAC: STREAMINFO → dfLa ──────────────────────────────────────────────────

/// Converts a FLAC sequence header into a `dfLa` payload.
///
/// Returns `(dfla_payload, sample_rate, channels)`.
///
/// `FLACSpecificBox` is a **`FullBox`** (FLAC-in-ISOBMFF §3.3.2), and
/// [`oximedia_container::mux::CmafMuxer`] writes the configuration box as a
/// *plain* box around whatever payload it is handed. The 4-byte
/// `version = 0` + `flags = 0` prefix therefore has to be produced **here** —
/// omitting it yields a box that still "muxes successfully" while parsing as
/// garbage downstream.
///
/// Three input shapes are accepted, because publishers differ:
///
/// 1. a native FLAC stream head — `fLaC` magic followed by metadata blocks
/// 2. a metadata block chain starting with the `STREAMINFO` block header
/// 3. a bare 34-byte `STREAMINFO` payload with no block header
///
/// All metadata blocks present are transcribed verbatim (nothing is dropped),
/// with the last-metadata-block flag recomputed so the emitted chain
/// terminates correctly.
fn flac_config_to_dfla(config: &[u8]) -> ServerResult<(Vec<u8>, u32, u8)> {
    // Shape 1: strip the native-stream magic.
    let blocks = if config.len() >= 4 && &config[0..4] == b"fLaC" {
        &config[4..]
    } else {
        config
    };

    // Shape 3: a bare STREAMINFO payload, with no metadata block header.
    if blocks.len() == FLAC_STREAMINFO_LEN {
        let (sample_rate, channels) = parse_flac_streaminfo(blocks)?;
        let mut out = fullbox_prefix();
        push_flac_metadata_block(&mut out, 0, true, blocks);
        return Ok((out, sample_rate, channels));
    }

    // Shape 2: a metadata block chain. Walk it, keeping every block.
    let parsed = parse_flac_metadata_blocks(blocks)?;
    let (block_type, _, first_payload) = parsed.first().ok_or_else(|| {
        ServerError::BadRequest(
            "FLAC sequence header carries no metadata blocks; a STREAMINFO block is \
             required to build a FLACSpecificBox"
                .to_string(),
        )
    })?;
    if *block_type != 0 {
        return Err(ServerError::BadRequest(format!(
            "FLAC sequence header's first metadata block has type {block_type}; \
             FLACSpecificBox requires STREAMINFO (type 0) first"
        )));
    }
    let (sample_rate, channels) = parse_flac_streaminfo(first_payload)?;

    let mut out = fullbox_prefix();
    let last_index = parsed.len() - 1;
    for (index, (block_type, _, payload)) in parsed.iter().enumerate() {
        push_flac_metadata_block(&mut out, *block_type, index == last_index, payload);
    }
    Ok((out, sample_rate, channels))
}

/// Returns the 4-byte `FullBox` header (`version = 0`, `flags = 0`).
fn fullbox_prefix() -> Vec<u8> {
    vec![0, 0, 0, 0]
}

/// Appends one FLAC `METADATA_BLOCK` (header + payload) to `out`.
fn push_flac_metadata_block(out: &mut Vec<u8>, block_type: u8, last: bool, payload: &[u8]) {
    let flag = if last { 0x80 } else { 0x00 };
    out.push(flag | (block_type & 0x7F));
    let len = payload.len() as u32;
    out.push((len >> 16) as u8);
    out.push((len >> 8) as u8);
    out.push(len as u8);
    out.extend_from_slice(payload);
}

/// Walks a FLAC metadata block chain, returning `(block_type, last, payload)`.
#[allow(clippy::type_complexity)]
fn parse_flac_metadata_blocks(data: &[u8]) -> ServerResult<Vec<(u8, bool, &[u8])>> {
    let mut blocks = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= data.len() {
        let header = data[pos];
        let last = header & 0x80 != 0;
        let block_type = header & 0x7F;
        let len = ((data[pos + 1] as usize) << 16)
            | ((data[pos + 2] as usize) << 8)
            | (data[pos + 3] as usize);
        let start = pos + 4;
        let end = start.checked_add(len).ok_or_else(|| {
            ServerError::BadRequest("FLAC metadata block length overflows".to_string())
        })?;
        let payload = data.get(start..end).ok_or_else(|| {
            ServerError::BadRequest(format!(
                "FLAC metadata block of type {block_type} declares {len} bytes but only {} \
                 remain in the sequence header",
                data.len().saturating_sub(start)
            ))
        })?;
        blocks.push((block_type, last, payload));
        pos = end;
        if last {
            break;
        }
    }
    Ok(blocks)
}

/// Reads sample rate and channel count out of a 34-byte `STREAMINFO` payload.
///
/// Layout (FLAC format specification, `STREAMINFO`): the 20-bit sample rate,
/// 3-bit `channels - 1` and 5-bit `bits_per_sample - 1` are packed across
/// bytes 10..14.
fn parse_flac_streaminfo(payload: &[u8]) -> ServerResult<(u32, u8)> {
    if payload.len() < FLAC_STREAMINFO_LEN {
        return Err(ServerError::BadRequest(format!(
            "FLAC STREAMINFO block is {} bytes; {FLAC_STREAMINFO_LEN} are required",
            payload.len()
        )));
    }
    let sample_rate = (u32::from(payload[10]) << 12)
        | (u32::from(payload[11]) << 4)
        | (u32::from(payload[12]) >> 4);
    let channels = ((payload[12] >> 1) & 0x07) + 1;
    if sample_rate == 0 {
        return Err(ServerError::BadRequest(
            "FLAC STREAMINFO declares a sample rate of 0".to_string(),
        ));
    }
    Ok((sample_rate, channels))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds an ingest packet out of an already-encoded tag body.
    fn video_packet(body: Bytes, timestamp: u32) -> MediaPacket {
        MediaPacket {
            packet_type: MediaPacketType::Video,
            timestamp,
            stream_id: 1,
            data: body,
        }
    }

    fn audio_packet(body: Bytes, timestamp: u32) -> MediaPacket {
        MediaPacket {
            packet_type: MediaPacketType::Audio,
            timestamp,
            stream_id: 1,
            data: body,
        }
    }

    /// A minimal but structurally valid `AV1CodecConfigurationRecord` with no
    /// `configOBUs` (so dimensions stay honestly unknown).
    fn av1_config_bytes() -> Vec<u8> {
        vec![
            0x81, // marker=1, version=1
            0x00, // seq_profile=0, seq_level_idx_0=0
            0x00, // seq_tier / bitdepth / chroma flags
            0x00, // reserved + initial_presentation_delay
        ]
    }

    /// A bare `VPCodecConfigurationRecord` (no `codecInitializationData`).
    fn vp9_record_bytes() -> Vec<u8> {
        vec![
            0x00, // profile
            0x1F, // level
            0x80, // bitDepth=8, chromaSubsampling=0, fullRange=0
            0x01, // colourPrimaries
            0x01, // transferCharacteristics
            0x01, // matrixCoefficients
            0x00, 0x00, // codecInitializationDataSize = 0
        ]
    }

    /// A canonical stereo 48 kHz `OpusHead` (mapping family 0).
    fn opus_head_bytes() -> Vec<u8> {
        let mut v = b"OpusHead".to_vec();
        v.push(1); // version
        v.push(2); // channels
        v.extend_from_slice(&312u16.to_le_bytes()); // pre-skip
        v.extend_from_slice(&48_000u32.to_le_bytes()); // input sample rate
        v.extend_from_slice(&0i16.to_le_bytes()); // output gain
        v.push(0); // mapping family
        v
    }

    /// A 34-byte `STREAMINFO` payload for 44.1 kHz stereo, 16 bit.
    fn flac_streaminfo_bytes() -> Vec<u8> {
        let mut v = vec![0u8; FLAC_STREAMINFO_LEN];
        v[0..2].copy_from_slice(&4096u16.to_be_bytes()); // min block size
        v[2..4].copy_from_slice(&4096u16.to_be_bytes()); // max block size
                                                         // sample_rate = 44100 (20 bits), channels-1 = 1 (3 bits), bps-1 = 15 (5 bits)
        let sample_rate: u32 = 44_100;
        v[10] = (sample_rate >> 12) as u8;
        v[11] = (sample_rate >> 4) as u8;
        v[12] = (((sample_rate & 0x0F) as u8) << 4) | (1 << 1) | ((15 >> 4) & 0x01);
        v[13] = ((15 & 0x0F) << 4) as u8;
        v
    }

    // ── Dispatch ──────────────────────────────────────────────────────────

    // 1. Enhanced tags produced by this workspace must route to the E-RTMP path.
    #[test]
    fn enhanced_video_dispatch_matches_encoder() {
        let tag = EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0xAA]));
        let body = tag.encode();
        assert!(
            is_enhanced_video(&body),
            "an AV1 keyframe tag encoded by oximedia-net must be recognised as enhanced"
        );
    }

    // 2. A legacy AVC keyframe tag (0x17) must NOT route to the E-RTMP path.
    #[test]
    fn legacy_avc_dispatch_is_not_enhanced() {
        assert!(!is_enhanced_video(&[0x17, 0x00, 0, 0, 0]));
    }

    // 3. Enhanced audio dispatch requires the 0x9F marker exactly.
    #[test]
    fn enhanced_audio_dispatch_matches_encoder() {
        let tag = EnhancedAudioTag::opus(Bytes::from_static(&[0x01]));
        assert!(is_enhanced_audio(&tag.encode()));
        assert!(!is_enhanced_audio(&[0xAF, 0x01]));
    }

    // ── Red List refusals ─────────────────────────────────────────────────

    // 4. Legacy H.264 sequence header is refused, naming H.264 and the Red List.
    #[test]
    fn legacy_avc_is_refused_naming_red_list() {
        let mut depack = Depacketizer::new();
        let packet = video_packet(Bytes::from_static(&[0x17, 0x00, 0x00, 0x00, 0x00]), 0);
        let err = depack
            .push(&packet)
            .expect_err("H.264 ingest must be refused");
        let msg = err.to_string();
        assert!(msg.contains("H.264"), "message must name H.264: {msg}");
        assert!(
            msg.contains("Red List"),
            "message must cite Red List: {msg}"
        );
    }

    // 5. Legacy AAC sequence header is refused, naming AAC and the Red List.
    #[test]
    fn legacy_aac_is_refused_naming_red_list() {
        let mut depack = Depacketizer::new();
        let packet = audio_packet(Bytes::from_static(&[0xAF, 0x00, 0x12, 0x10]), 0);
        let err = depack
            .push(&packet)
            .expect_err("AAC ingest must be refused");
        let msg = err.to_string();
        assert!(msg.contains("AAC"), "message must name AAC: {msg}");
        assert!(
            msg.contains("Red List"),
            "message must cite Red List: {msg}"
        );
    }

    // 6. Non-patent-free E-RTMP FourCC (HEVC) is refused by name.
    #[test]
    fn hevc_fourcc_is_refused_by_name() {
        let mut depack = Depacketizer::new();
        let tag = EnhancedVideoTag::sequence_start(FourCC::HEVC, Bytes::from_static(&[0x01]));
        let err = depack
            .push(&video_packet(tag.encode(), 0))
            .expect_err("HEVC ingest must be refused");
        let msg = err.to_string();
        assert!(msg.contains("hvc1"), "message must name the FourCC: {msg}");
        assert!(
            msg.contains("Red List"),
            "message must cite Red List: {msg}"
        );
    }

    // 7. E-RTMP AAC FourCC is refused by name.
    #[test]
    fn erroneous_aac_fourcc_is_refused() {
        let mut depack = Depacketizer::new();
        let tag = EnhancedAudioTag::sequence_start(FourCC::AAC, Bytes::from_static(&[0x12, 0x10]));
        let err = depack
            .push(&audio_packet(tag.encode(), 0))
            .expect_err("E-RTMP AAC must be refused");
        assert!(err.to_string().contains("mp4a"));
    }

    // ── AV1 ───────────────────────────────────────────────────────────────

    // 8. AV1 sequence start configures the stream; the keyframe yields a sample.
    #[test]
    fn av1_sequence_start_then_keyframe() {
        let mut depack = Depacketizer::new();
        assert!(!depack.ready());

        let seq = EnhancedVideoTag::sequence_start(FourCC::AV1, Bytes::from(av1_config_bytes()));
        assert_eq!(
            depack.push(&video_packet(seq.encode(), 0)).expect("seq ok"),
            None,
            "a sequence header carries no elementary sample"
        );
        assert!(depack.ready());
        let config = depack.video_config().expect("video configured");
        assert_eq!(config.codec, IngestCodec::Av1);
        assert_eq!(config.config_record, av1_config_bytes());

        let frame = EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x12, 0x34, 0x56]));
        let sample = depack
            .push(&video_packet(frame.encode(), 40))
            .expect("frame ok")
            .expect("keyframe yields a sample");
        assert_eq!(sample.codec, IngestCodec::Av1);
        assert_eq!(sample.kind, SampleKind::Video);
        assert_eq!(sample.data, Bytes::from_static(&[0x12, 0x34, 0x56]));
        assert_eq!(sample.dts_ms, 40);
        assert!(sample.keyframe);
    }

    // 9. An AV1 config record with a bad marker/version is refused.
    #[test]
    fn av1_config_record_validates_marker() {
        let mut depack = Depacketizer::new();
        let seq = EnhancedVideoTag::sequence_start(
            FourCC::AV1,
            Bytes::from_static(&[0x00, 0x00, 0x00, 0x00]),
        );
        let err = depack
            .push(&video_packet(seq.encode(), 0))
            .expect_err("bad av1C must be refused");
        assert!(err.to_string().contains("AV1CodecConfigurationRecord"));
    }

    // 10. Coded frames before the sequence header are refused, not fabricated.
    #[test]
    fn av1_frame_before_sequence_header_is_refused() {
        let mut depack = Depacketizer::new();
        let frame = EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x01]));
        let err = depack
            .push(&video_packet(frame.encode(), 0))
            .expect_err("a frame without config must be refused");
        assert!(err
            .to_string()
            .contains("before its Enhanced-RTMP sequence header"));
    }

    // 11. Composition time survives the round trip.
    #[test]
    fn av1_composition_time_is_preserved() {
        let mut depack = Depacketizer::new();
        let seq = EnhancedVideoTag::sequence_start(FourCC::AV1, Bytes::from(av1_config_bytes()));
        depack.push(&video_packet(seq.encode(), 0)).expect("seq ok");

        let mut frame = EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x01]));
        frame.composition_time_ms = 33;
        let sample = depack
            .push(&video_packet(frame.encode(), 100))
            .expect("frame ok")
            .expect("sample");
        assert_eq!(sample.composition_time_ms, 33);
    }

    // 12. Sequence end absorbs silently.
    #[test]
    fn av1_sequence_end_yields_no_sample() {
        let mut depack = Depacketizer::new();
        let end = EnhancedVideoTag::sequence_end(FourCC::AV1);
        assert_eq!(
            depack.push(&video_packet(end.encode(), 0)).expect("ok"),
            None
        );
    }

    // ── VP9 ───────────────────────────────────────────────────────────────

    // 13. A bare VPCodecConfigurationRecord gains the vpcC FullBox prefix.
    #[test]
    fn vp9_bare_record_gains_fullbox_prefix() {
        let record = vp9_record_bytes();
        let out = vp9_config_record(&record).expect("bare record accepted");
        assert_eq!(
            &out[0..4],
            &[1, 0, 0, 0],
            "vpcC is a FullBox at version 1 with zero flags"
        );
        assert_eq!(&out[4..], &record[..]);
    }

    // 14. An already-prefixed record passes through unchanged.
    #[test]
    fn vp9_prefixed_record_passes_through() {
        let mut record = vec![1u8, 0, 0, 0];
        record.extend_from_slice(&vp9_record_bytes());
        let out = vp9_config_record(&record).expect("prefixed record accepted");
        assert_eq!(out, record);
    }

    // 15. A record whose declared size matches neither shape is refused.
    #[test]
    fn vp9_inconsistent_record_is_refused() {
        // codecInitializationDataSize = 99, but no data follows.
        let record = vec![0, 0x1F, 0x80, 1, 1, 1, 0x00, 0x63];
        let err = vp9_config_record(&record).expect_err("inconsistent record must be refused");
        assert!(err.to_string().contains("VPCodecConfigurationRecord"));
    }

    /// MSB-first bit writer used to build synthetic VP9 headers.
    #[derive(Default)]
    struct BitWriter {
        bits: Vec<bool>,
    }

    impl BitWriter {
        fn put(&mut self, value: u32, count: u32) {
            for i in (0..count).rev() {
                self.bits.push((value >> i) & 1 == 1);
            }
        }

        fn finish(self) -> Vec<u8> {
            let mut out = vec![0u8; self.bits.len().div_ceil(8)];
            for (index, bit) in self.bits.iter().enumerate() {
                if *bit {
                    out[index / 8] |= 0x80 >> (index % 8);
                }
            }
            out
        }
    }

    /// A complete VP9 profile-0 keyframe uncompressed header for `width`
    /// × `height`, matching the VP9 bitstream specification's
    /// `uncompressed_header()` for a key frame.
    fn vp9_keyframe_header(width: u32, height: u32) -> Vec<u8> {
        let mut w = BitWriter::default();
        w.put(0b10, 2); // frame_marker
        w.put(0, 1); // profile_low_bit
        w.put(0, 1); // profile_high_bit  -> profile 0
        w.put(0, 1); // show_existing_frame
        w.put(0, 1); // frame_type = KEY_FRAME
        w.put(1, 1); // show_frame
        w.put(0, 1); // error_resilient_mode
        w.put(0x49, 8); // frame_sync_code
        w.put(0x83, 8);
        w.put(0x42, 8);
        // color_config() for profile 0: colour space then range only.
        w.put(2, 3); // CS_BT_709 (not sRGB, so a range bit follows)
        w.put(0, 1); // color_range
                     // frame_size()
        w.put(width - 1, 16);
        w.put(height - 1, 16);
        // render_size()
        w.put(0, 1); // render_and_frame_size_different
                     // error_resilient_mode == 0
        w.put(0, 1); // refresh_frame_context
        w.put(0, 1); // frame_parallel_decoding_mode
        w.put(0, 2); // frame_context_idx
                     // loop_filter_params()
        w.put(0, 6); // filter_level
        w.put(0, 3); // sharpness_level
        w.put(0, 1); // delta_enabled
                     // quantization_params()
        w.put(0, 8); // base_q_idx
        w.put(0, 1); // delta_coded (y_dc)
        w.put(0, 1); // delta_coded (uv_dc)
        w.put(0, 1); // delta_coded (uv_ac)
                     // segmentation_params()
        w.put(0, 1); // segmentation_enabled
                     // tile_info(): 640 px wide gives max_log2 - min_log2 == 1 increment bit.
        w.put(0, 1); // increment_tile_cols_log2
        w.put(0, 1); // tile_rows_log2
        w.put(16, 16); // header_size_in_bytes (must be non-zero)
        w.finish()
    }

    // 16a. VP9 frame dimensions are read from a real keyframe header.
    //
    // `vpcC` genuinely does not encode frame size, so this is the only honest
    // source — and it is what earns oximedia-server's `oximedia-codec/vp9`
    // feature dependency.
    #[test]
    fn vp9_dimensions_come_from_the_keyframe_header() {
        let frame = vp9_keyframe_header(640, 360);
        assert_eq!(
            vp9_dimensions_from_keyframe(&frame),
            Some((640, 360)),
            "the VP9 uncompressed header must yield the real frame size"
        );

        let mut depack = Depacketizer::new();
        let seq = EnhancedVideoTag::sequence_start(FourCC::VP9, Bytes::from(vp9_record_bytes()));
        depack.push(&video_packet(seq.encode(), 0)).expect("seq ok");
        assert_eq!(
            depack.video_config().and_then(|c| c.width),
            None,
            "vpcC alone carries no dimensions"
        );

        let key = EnhancedVideoTag::vp9_keyframe(Bytes::from(frame));
        depack
            .push(&video_packet(key.encode(), 0))
            .expect("frame ok")
            .expect("sample");
        let config = depack.video_config().expect("configured");
        assert_eq!((config.width, config.height), (Some(640), Some(360)));
    }

    // 16b. A header that carries no frame size leaves the dimensions unknown.
    //
    // `[0x9D, 0x01, 0x2A]` parses as a profile-2 `show_existing_frame` header:
    // it succeeds, but encodes no dimensions, so its zeroed defaults must never
    // be reported as a real 0x0 frame size.
    #[test]
    fn vp9_header_without_frame_size_leaves_dimensions_unknown() {
        assert_eq!(vp9_dimensions_from_keyframe(&[0x9D, 0x01, 0x2A]), None);
        // Truncated garbage is refused outright.
        assert_eq!(vp9_dimensions_from_keyframe(&[0x00]), None);
    }

    // 16. VP9 sequence start + coded frame produces a sample.
    #[test]
    fn vp9_sequence_start_then_frame() {
        let mut depack = Depacketizer::new();
        let seq = EnhancedVideoTag::sequence_start(FourCC::VP9, Bytes::from(vp9_record_bytes()));
        depack.push(&video_packet(seq.encode(), 0)).expect("seq ok");
        let config = depack.video_config().expect("configured");
        assert_eq!(config.codec, IngestCodec::Vp9);
        assert_eq!(config.timescale(), 90_000);

        let frame = EnhancedVideoTag::vp9_keyframe(Bytes::from_static(&[0x9D, 0x01, 0x2A]));
        let sample = depack
            .push(&video_packet(frame.encode(), 33))
            .expect("frame ok")
            .expect("sample");
        assert_eq!(sample.codec, IngestCodec::Vp9);
        assert_eq!(sample.dts_ms, 33);
    }

    // ── Opus ──────────────────────────────────────────────────────────────

    // 17. OpusHead → dOps transcribes fields and swaps endianness.
    #[test]
    fn opus_head_to_dops_transcribes_correctly() {
        let (dops, rate, channels) =
            opus_head_to_dops(&opus_head_bytes()).expect("OpusHead accepted");
        assert_eq!(dops[0], 0, "dOps Version must be 0, not OpusHead's 1");
        assert_eq!(dops[1], 2, "OutputChannelCount");
        assert_eq!(
            u16::from_be_bytes([dops[2], dops[3]]),
            312,
            "PreSkip must be big-endian in dOps"
        );
        assert_eq!(
            u32::from_be_bytes([dops[4], dops[5], dops[6], dops[7]]),
            48_000,
            "InputSampleRate must be big-endian in dOps"
        );
        assert_eq!(i16::from_be_bytes([dops[8], dops[9]]), 0, "OutputGain");
        assert_eq!(dops[10], 0, "ChannelMappingFamily");
        assert_eq!(dops.len(), 11, "family 0 carries no mapping table");
        assert_eq!(rate, 48_000, "Opus always decodes at 48 kHz");
        assert_eq!(channels, 2);
    }

    // 18. Endianness swap is observable on a non-palindromic pre-skip.
    #[test]
    fn opus_pre_skip_endianness_is_really_swapped() {
        let mut head = opus_head_bytes();
        head[10..12].copy_from_slice(&0x0102u16.to_le_bytes());
        let (dops, _, _) = opus_head_to_dops(&head).expect("accepted");
        assert_eq!(&dops[2..4], &[0x01, 0x02], "0x0102 big-endian");
    }

    // 19. A mapping family other than 0 requires its table.
    #[test]
    fn opus_mapping_family_requires_table() {
        let mut head = opus_head_bytes();
        head[18] = 1; // family 1, but no table appended
        let err = opus_head_to_dops(&head).expect_err("missing table must be refused");
        assert!(err.to_string().contains("channel mapping table"));

        head.extend_from_slice(&[1, 1, 0, 1]); // stream, coupled, mapping[2]
        let (dops, _, _) = opus_head_to_dops(&head).expect("with table accepted");
        assert_eq!(dops.len(), 15);
        assert_eq!(&dops[11..], &[1, 1, 0, 1]);
    }

    // 20. A blob without the OpusHead magic is refused, not reinterpreted.
    #[test]
    fn opus_without_magic_is_refused() {
        let err = opus_head_to_dops(&[0u8; 24]).expect_err("must be refused");
        assert!(err.to_string().contains("OpusHead"));
    }

    // 21. Full Opus sequence start through the depacketizer.
    #[test]
    fn opus_sequence_start_then_frame() {
        let mut depack = Depacketizer::new();
        let seq = EnhancedAudioTag::sequence_start(FourCC::OPUS, Bytes::from(opus_head_bytes()));
        assert_eq!(
            depack.push(&audio_packet(seq.encode(), 0)).expect("ok"),
            None
        );
        let config = depack.audio_config().expect("audio configured");
        assert_eq!(config.sample_rate, Some(48_000));
        assert_eq!(config.channels, Some(2));
        assert_eq!(config.timescale(), 48_000);
        assert_eq!(config.config_record[0], 0);

        let frame = EnhancedAudioTag::opus(Bytes::from_static(&[0xFC, 0xFF, 0xFE]));
        let sample = depack
            .push(&audio_packet(frame.encode(), 20))
            .expect("frame ok")
            .expect("sample");
        assert_eq!(sample.kind, SampleKind::Audio);
        assert!(
            sample.keyframe,
            "every Opus packet is independently decodable"
        );
    }

    // ── FLAC ──────────────────────────────────────────────────────────────

    // 22. dfLa payload begins with the FullBox version/flags prefix.
    #[test]
    fn flac_dfla_starts_with_fullbox_prefix() {
        let (dfla, rate, channels) =
            flac_config_to_dfla(&flac_streaminfo_bytes()).expect("bare STREAMINFO accepted");
        assert_eq!(
            &dfla[0..4],
            &[0, 0, 0, 0],
            "dfLa is a FullBox: version=0, flags=0 must be present"
        );
        assert_eq!(dfla[4], 0x80, "STREAMINFO block, last-block flag set");
        assert_eq!(
            ((dfla[5] as usize) << 16) | ((dfla[6] as usize) << 8) | dfla[7] as usize,
            FLAC_STREAMINFO_LEN
        );
        assert_eq!(dfla.len(), 4 + 4 + FLAC_STREAMINFO_LEN);
        assert_eq!(rate, 44_100);
        assert_eq!(channels, 2);
    }

    // 23. A native `fLaC` stream head is accepted and its magic stripped.
    #[test]
    fn flac_native_stream_head_is_accepted() {
        let mut config = b"fLaC".to_vec();
        push_flac_metadata_block(&mut config, 0, true, &flac_streaminfo_bytes());
        let (dfla, rate, channels) = flac_config_to_dfla(&config).expect("accepted");
        assert_eq!(&dfla[0..4], &[0, 0, 0, 0]);
        assert_eq!(dfla[4], 0x80);
        assert_eq!(rate, 44_100);
        assert_eq!(channels, 2);
    }

    // 24. Extra metadata blocks are preserved and the last flag recomputed.
    #[test]
    fn flac_extra_metadata_blocks_are_preserved() {
        let mut config = b"fLaC".to_vec();
        push_flac_metadata_block(&mut config, 0, false, &flac_streaminfo_bytes());
        push_flac_metadata_block(&mut config, 4, true, &[0xAB, 0xCD]); // VORBIS_COMMENT
        let (dfla, _, _) = flac_config_to_dfla(&config).expect("accepted");
        // FullBox prefix + STREAMINFO block + comment block
        assert_eq!(dfla.len(), 4 + 4 + FLAC_STREAMINFO_LEN + 4 + 2);
        assert_eq!(dfla[4] & 0x80, 0, "STREAMINFO is no longer the last block");
        let comment_header = 4 + 4 + FLAC_STREAMINFO_LEN;
        assert_eq!(dfla[comment_header], 0x84, "last flag + block type 4");
    }

    // 25. A non-STREAMINFO first block is refused.
    #[test]
    fn flac_without_streaminfo_first_is_refused() {
        let mut config = b"fLaC".to_vec();
        push_flac_metadata_block(&mut config, 4, true, &[0x00, 0x01]);
        let err = flac_config_to_dfla(&config).expect_err("must be refused");
        assert!(err.to_string().contains("STREAMINFO"));
    }

    // 26. Full FLAC sequence start through the depacketizer.
    #[test]
    fn flac_sequence_start_then_frame() {
        let mut depack = Depacketizer::new();
        let seq =
            EnhancedAudioTag::sequence_start(FourCC::FLAC, Bytes::from(flac_streaminfo_bytes()));
        depack.push(&audio_packet(seq.encode(), 0)).expect("ok");
        let config = depack.audio_config().expect("configured");
        assert_eq!(config.codec, IngestCodec::Flac);
        assert_eq!(config.sample_rate, Some(44_100));
        assert_eq!(config.timescale(), 44_100);

        let frame = EnhancedAudioTag::flac(Bytes::from_static(&[0xFF, 0xF8]));
        let sample = depack
            .push(&audio_packet(frame.encode(), 10))
            .expect("frame ok")
            .expect("sample");
        assert_eq!(sample.codec, IngestCodec::Flac);
    }

    // ── observe_config / sniffing ─────────────────────────────────────────

    // 27. `observe_config` learns configuration without consuming frames.
    #[test]
    fn observe_config_absorbs_only_sequence_headers() {
        let mut depack = Depacketizer::new();
        let seq = EnhancedVideoTag::sequence_start(FourCC::AV1, Bytes::from(av1_config_bytes()));
        let seq_packet = video_packet(seq.encode(), 0);
        depack.observe_config(&seq_packet).expect("observed");
        assert!(depack.ready());

        // Replaying the same packet through `push` is idempotent.
        assert_eq!(depack.push(&seq_packet).expect("ok"), None);

        // A coded frame is ignored by `observe_config` but yielded by `push`.
        let frame = EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x07]));
        let frame_packet = video_packet(frame.encode(), 66);
        depack.observe_config(&frame_packet).expect("observed");
        assert!(depack.push(&frame_packet).expect("ok").is_some());
    }

    // 28. `sniff_ingest_codec` names the codec without any state.
    #[test]
    fn sniff_ingest_codec_identifies_streams() {
        let frame = EnhancedVideoTag::av1_keyframe(Bytes::from_static(&[0x01]));
        assert_eq!(
            sniff_ingest_codec(&video_packet(frame.encode(), 0)).expect("sniffed"),
            IngestCodec::Av1
        );

        let audio = EnhancedAudioTag::flac(Bytes::from_static(&[0x01]));
        assert_eq!(
            sniff_ingest_codec(&audio_packet(audio.encode(), 0)).expect("sniffed"),
            IngestCodec::Flac
        );

        let err = sniff_ingest_codec(&video_packet(Bytes::from_static(&[0x17, 0x01, 0, 0, 0]), 0))
            .expect_err("H.264 must be refused");
        assert!(err.to_string().contains("H.264"));
    }

    // 29. Codec identity maps onto the container types the muxers expect.
    #[test]
    fn codec_identity_maps_to_container_types() {
        assert_eq!(IngestCodec::Av1.codec_id(), CodecId::Av1);
        assert_eq!(IngestCodec::Vp9.codec_id(), CodecId::Vp9);
        assert_eq!(IngestCodec::Opus.codec_id(), CodecId::Opus);
        assert_eq!(IngestCodec::Flac.codec_id(), CodecId::Flac);
        assert_eq!(&IngestCodec::Av1.sample_entry_fourcc(), b"av01");
        assert_eq!(&IngestCodec::Vp9.sample_entry_fourcc(), b"vp09");
        assert_eq!(&IngestCodec::Opus.sample_entry_fourcc(), b"Opus");
        assert_eq!(&IngestCodec::Flac.sample_entry_fourcc(), b"fLaC");
    }

    // 30. Multitrack / multichannel-config audio is refused by name.
    #[test]
    fn multitrack_audio_is_refused() {
        let mut depack = Depacketizer::new();
        let body = Bytes::from(vec![0x9F, 5, b'O', b'p', b'u', b's', 0x00]);
        let err = depack
            .push(&audio_packet(body, 0))
            .expect_err("multitrack must be refused");
        assert!(err.to_string().contains("multitrack"));
    }

    // 31. Real AV1 dimensions are read from a sequence header OBU when present.
    #[test]
    fn av1_dimensions_come_from_sequence_header_when_present() {
        // No configOBUs → dimensions must stay honestly unknown.
        let mut depack = Depacketizer::new();
        let seq = EnhancedVideoTag::sequence_start(FourCC::AV1, Bytes::from(av1_config_bytes()));
        depack.push(&video_packet(seq.encode(), 0)).expect("ok");
        let config = depack.video_config().expect("configured");
        assert_eq!(
            (config.width, config.height),
            (None, None),
            "dimensions must never be invented when no sequence header OBU is present"
        );
    }
}
