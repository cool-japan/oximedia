//! Conversions from `oximedia_container`'s real demux types to this crate's
//! local `src/container.rs` mirror types.
//!
//! `src/container.rs` predates this crate depending on `oximedia-container`
//! and is kept as a deliberately independent, WASM-local type surface (five
//! files build on it: `types.rs`, `demuxer.rs`, `streaming_demuxer.rs`, and
//! their tests) rather than replaced outright. This module is the one place
//! that knows how to translate a real, fully-parsed `oximedia_container`
//! value into the local mirror -- every field copied here reflects data
//! [`oximedia_container`]'s demuxers actually parsed from the container,
//! never a fabricated default.
//!
//! Fields that exist on the real type but not the local mirror (MP4/MKV
//! display-rotation matrices, Matroska `BlockAdditionMapping`s) are
//! intentionally dropped -- the mirror is a deliberate subset, not a lossy
//! bug.

use oximedia_container::demux::{FlacDemuxer, MatroskaDemuxer, Mp4Demuxer, OggDemuxer, WavDemuxer};
use oximedia_container::Demuxer as ContainerDemuxer;
use oximedia_core::OxiResult;
use oximedia_io::source::MemorySource;

use crate::container::{
    CodecParams as LocalCodecParams, ContainerFormat, Metadata as LocalMetadata,
    Packet as LocalPacket, PacketFlags as LocalPacketFlags, StreamInfo as LocalStreamInfo,
};

/// Converts a real, parsed [`oximedia_container::CodecParams`] into the
/// local mirror ([`crate::container::CodecParams`] -- a plain, non-`wasm_bindgen`
/// type; see [`convert_stream_info`] for the further conversion into the
/// public [`crate::types::WasmStreamInfo`]).
pub fn convert_codec_params(params: &oximedia_container::CodecParams) -> LocalCodecParams {
    LocalCodecParams {
        width: params.width,
        height: params.height,
        sample_rate: params.sample_rate,
        channels: params.channels,
        extradata: params.extradata.clone(),
    }
}

/// Converts a real, parsed [`oximedia_container::Metadata`] into the local
/// mirror.
pub fn convert_metadata(metadata: &oximedia_container::Metadata) -> LocalMetadata {
    LocalMetadata {
        title: metadata.title.clone(),
        artist: metadata.artist.clone(),
        album: metadata.album.clone(),
        entries: metadata.entries.clone(),
    }
}

/// Converts a real, parsed [`oximedia_container::StreamInfo`] into the
/// local mirror type ([`crate::container::StreamInfo`]).
///
/// Callers that need the public `wasm_bindgen`-exposed
/// [`crate::types::WasmStreamInfo`] wrap this with
/// `WasmStreamInfo::from(...)` (see its `impl From<container::StreamInfo>`).
///
/// Drops `rotation`/`display_matrix` (no equivalent field on the local
/// mirror -- see module docs).
pub fn convert_stream_info(stream: &oximedia_container::StreamInfo) -> LocalStreamInfo {
    let mut converted = LocalStreamInfo::new(stream.index, stream.codec, stream.timebase);
    converted.duration = stream.duration;
    converted.codec_params = convert_codec_params(&stream.codec_params);
    converted.metadata = convert_metadata(&stream.metadata);
    converted
}

/// Converts a slice of real, parsed stream info into the local mirror type.
pub fn convert_streams(streams: &[oximedia_container::StreamInfo]) -> Vec<LocalStreamInfo> {
    streams.iter().map(convert_stream_info).collect()
}

/// Converts real [`oximedia_container::PacketFlags`] into the local mirror
/// flags, field by field (the two types are not bit-compatible by
/// construction guarantee, so this does not rely on their numeric layout
/// matching).
pub fn convert_packet_flags(flags: oximedia_container::PacketFlags) -> LocalPacketFlags {
    let mut converted = LocalPacketFlags::empty();
    if flags.contains(oximedia_container::PacketFlags::KEYFRAME) {
        converted |= LocalPacketFlags::KEYFRAME;
    }
    if flags.contains(oximedia_container::PacketFlags::CORRUPT) {
        converted |= LocalPacketFlags::CORRUPT;
    }
    if flags.contains(oximedia_container::PacketFlags::DISCARD) {
        converted |= LocalPacketFlags::DISCARD;
    }
    converted
}

/// Converts a real, demuxed [`oximedia_container::Packet`] into the local
/// mirror type ([`crate::container::Packet`]). Callers needing the public
/// [`crate::types::WasmPacket`] wrap this with `WasmPacket::from(...)`.
pub fn convert_packet(packet: oximedia_container::Packet) -> LocalPacket {
    LocalPacket::new(
        packet.stream_index,
        packet.data,
        packet.timestamp,
        convert_packet_flags(packet.flags),
    )
}

/// Real per-format demuxer, selected once the container format has been
/// detected from magic bytes. Shared by `demuxer.rs` (whole-buffer demux)
/// and `streaming_demuxer.rs` (rebuild-and-replay over a growing buffer).
///
/// An enum rather than `Box<dyn Demuxer>` -- there are exactly five
/// supported formats, known at construction time, so static dispatch via a
/// `match` is simpler than a trait object here.
pub enum RealDemuxer {
    Matroska(MatroskaDemuxer<MemorySource>),
    Ogg(OggDemuxer<MemorySource>),
    Flac(FlacDemuxer<MemorySource>),
    Wav(WavDemuxer<MemorySource>),
    Mp4(Mp4Demuxer<MemorySource>),
}

impl RealDemuxer {
    /// Builds the demuxer matching `format` over `source`. Does not read
    /// any bytes yet -- call [`probe`](Self::probe) first.
    pub fn new(format: ContainerFormat, source: MemorySource) -> Self {
        match format {
            ContainerFormat::Matroska => Self::Matroska(MatroskaDemuxer::new(source)),
            ContainerFormat::Ogg => Self::Ogg(OggDemuxer::new(source)),
            ContainerFormat::Flac => Self::Flac(FlacDemuxer::new(source)),
            ContainerFormat::Wav => Self::Wav(WavDemuxer::new(source)),
            ContainerFormat::Mp4 => Self::Mp4(Mp4Demuxer::new(source)),
        }
    }

    pub async fn probe(&mut self) -> OxiResult<oximedia_container::ProbeResult> {
        match self {
            Self::Matroska(d) => d.probe().await,
            Self::Ogg(d) => d.probe().await,
            Self::Flac(d) => d.probe().await,
            Self::Wav(d) => d.probe().await,
            Self::Mp4(d) => d.probe().await,
        }
    }

    pub async fn read_packet(&mut self) -> OxiResult<oximedia_container::Packet> {
        match self {
            Self::Matroska(d) => d.read_packet().await,
            Self::Ogg(d) => d.read_packet().await,
            Self::Flac(d) => d.read_packet().await,
            Self::Wav(d) => d.read_packet().await,
            Self::Mp4(d) => d.read_packet().await,
        }
    }

    pub fn streams(&self) -> &[oximedia_container::StreamInfo] {
        match self {
            Self::Matroska(d) => d.streams(),
            Self::Ogg(d) => d.streams(),
            Self::Flac(d) => d.streams(),
            Self::Wav(d) => d.streams(),
            Self::Mp4(d) => d.streams(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_core::{CodecId, Rational, Timestamp};

    #[test]
    fn stream_info_carries_real_fields_through() {
        let mut real = oximedia_container::StreamInfo::new(2, CodecId::Vp9, Rational::new(1, 1000));
        real.duration = Some(48_000);
        real.codec_params = oximedia_container::CodecParams::video(1920, 1080);
        real.metadata = oximedia_container::Metadata::new().with_title("t");

        let converted = convert_stream_info(&real);
        assert_eq!(converted.index, 2);
        assert_eq!(converted.codec, CodecId::Vp9);
        assert_eq!(converted.duration, Some(48_000));
        assert_eq!(converted.codec_params.width, Some(1920));
        assert_eq!(converted.codec_params.height, Some(1080));
        assert_eq!(converted.metadata.title.as_deref(), Some("t"));
    }

    #[test]
    fn packet_flags_convert_bit_by_bit() {
        let real =
            oximedia_container::PacketFlags::KEYFRAME | oximedia_container::PacketFlags::CORRUPT;
        let converted = convert_packet_flags(real);
        assert!(converted.contains(LocalPacketFlags::KEYFRAME));
        assert!(converted.contains(LocalPacketFlags::CORRUPT));
        assert!(!converted.contains(LocalPacketFlags::DISCARD));
    }

    #[test]
    fn packet_carries_real_data_through() {
        let real = oximedia_container::Packet::new(
            1,
            bytes::Bytes::from_static(&[9, 8, 7]),
            Timestamp::new(500, Rational::new(1, 48000)),
            oximedia_container::PacketFlags::KEYFRAME,
        );
        let converted = convert_packet(real);
        assert_eq!(converted.stream_index, 1);
        assert_eq!(&converted.data[..], &[9, 8, 7]);
        assert!(converted.is_keyframe());
        assert_eq!(converted.pts(), 500);
    }
}
