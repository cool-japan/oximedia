// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Output sinks for codecs the container crate has no (correct) muxer for.
//!
//! All types here implement the async [`Muxer`] trait so they can drive a
//! [`crate::multi_track::MultiTrackExecutor`] like any container muxer:
//!
//! - [`RawEsFileMuxer`] — concatenates encoded packets into a raw
//!   elementary-stream file (valid for intra-only MPEG-2 video, where each
//!   packet carries its own sequence header — the classic `.m2v` file).
//! - [`CafAlacFileMuxer`] — writes a standards-compliant CAF (Core Audio
//!   Format) file around raw ALAC packets: `desc` + `kuki` (magic cookie) +
//!   `pakt` (byte-count packet table per the CAF spec for
//!   constant-frames/variable-bytes formats) + `data`.
//! - [`Y4mFileMuxer`] — wraps the container crate's `Y4mMuxer` for raw
//!   (uncompressed) YUV 4:2:0 output.
//! - [`Ffv1RawFileMuxer`] — writes FFV1 packets into a small
//!   length-prefixed private framing (**not** a standard bitstream: FFV1
//!   packets carry no self-delimiting boundary of their own, unlike
//!   intra-only MPEG-2, and no container crate here can yet carry `V_FFV1`
//!   + `CodecPrivate`). Round-trips with [`crate::frame_adapters::Ffv1FrameDecoder`]
//!   but is not readable by ffmpeg/mkvtoolnix/any other tool.

#![allow(clippy::module_name_repetitions)]

use std::io::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use oximedia_container::mux::Y4mMuxerBuilder;
use oximedia_container::{Muxer, MuxerConfig, Packet, StreamInfo};
use oximedia_core::{OxiError, OxiResult};

// ─── RawEsFileMuxer ───────────────────────────────────────────────────────────

/// Writes every packet payload sequentially to `path` — a raw elementary
/// stream. Suitable for self-delimiting intra-only bitstreams (MPEG-2 video
/// with per-frame sequence headers).
pub struct RawEsFileMuxer {
    path: PathBuf,
    buf: Vec<u8>,
    streams: Vec<StreamInfo>,
    config: MuxerConfig,
}

impl RawEsFileMuxer {
    /// Creates a raw elementary-stream sink writing to `path`.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            buf: Vec::new(),
            streams: Vec::new(),
            config: MuxerConfig::new(),
        }
    }
}

#[async_trait]
impl Muxer for RawEsFileMuxer {
    fn add_stream(&mut self, info: StreamInfo) -> OxiResult<usize> {
        if !self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "raw elementary-stream output supports exactly one stream".into(),
            ));
        }
        self.streams.push(info);
        Ok(0)
    }

    async fn write_header(&mut self) -> OxiResult<()> {
        if self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "add_stream must be called before write_header".into(),
            ));
        }
        Ok(())
    }

    async fn write_packet(&mut self, packet: &Packet) -> OxiResult<()> {
        self.buf.extend_from_slice(&packet.data);
        Ok(())
    }

    async fn write_trailer(&mut self) -> OxiResult<()> {
        tokio::fs::write(&self.path, &self.buf).await?;
        Ok(())
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn config(&self) -> &MuxerConfig {
        &self.config
    }
}

// ─── FlacFileMuxer ────────────────────────────────────────────────────────────

/// Writes a raw FLAC file: `fLaC` magic + STREAMINFO + frames, with an
/// exact total-samples figure taken from the encoder's shared counter at
/// trailer time (the container crate's `FlacMuxer` can only estimate a
/// fixed block size per packet, which reference decoders reject as a
/// STREAMINFO mismatch).
pub struct FlacFileMuxer {
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
    frames: Vec<u8>,
    total_samples: crate::audio_adapters::SharedSampleCounter,
    streams: Vec<StreamInfo>,
    config: MuxerConfig,
}

impl FlacFileMuxer {
    /// Creates a FLAC sink writing to `path`. `total_samples` must be the
    /// encoder's [`crate::audio_adapters::FlacFrameEncoder::sample_counter`]
    /// handle so the STREAMINFO total is exact.
    #[must_use]
    pub fn new(
        path: PathBuf,
        sample_rate: u32,
        channels: u16,
        total_samples: crate::audio_adapters::SharedSampleCounter,
    ) -> Self {
        Self {
            path,
            sample_rate,
            channels,
            frames: Vec::new(),
            total_samples,
            streams: Vec::new(),
            config: MuxerConfig::new(),
        }
    }
}

#[async_trait]
impl Muxer for FlacFileMuxer {
    fn add_stream(&mut self, info: StreamInfo) -> OxiResult<usize> {
        if !self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "FLAC output supports exactly one audio stream".into(),
            ));
        }
        self.streams.push(info);
        Ok(0)
    }

    async fn write_header(&mut self) -> OxiResult<()> {
        if self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "add_stream must be called before write_header".into(),
            ));
        }
        Ok(())
    }

    async fn write_packet(&mut self, packet: &Packet) -> OxiResult<()> {
        self.frames.extend_from_slice(&packet.data);
        Ok(())
    }

    async fn write_trailer(&mut self) -> OxiResult<()> {
        let total = self
            .total_samples
            .load(std::sync::atomic::Ordering::Relaxed);
        // Fixed 4096-sample blocks; a shorter final block is excluded from
        // the STREAMINFO min/max block size per the FLAC specification.
        let mut out = crate::flac_bitstream::stream_info_block(
            self.sample_rate,
            self.channels,
            total,
            4_096,
            4_096,
        );
        out.extend_from_slice(&self.frames);
        tokio::fs::write(&self.path, &out).await?;
        Ok(())
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn config(&self) -> &MuxerConfig {
        &self.config
    }
}

// ─── CafAlacFileMuxer ─────────────────────────────────────────────────────────

/// CAF `frames per packet` used by the ALAC pipeline (matches the encoder's
/// `frame_length`).
pub const CAF_ALAC_FRAMES_PER_PACKET: u32 = 4096;

/// Writes ALAC packets into a standards-compliant CAF file.
///
/// Each `write_packet` payload must be exactly one ALAC packet (the audio
/// adapter guarantees this: one 4096-frame block per packet, plus one final
/// short block). The packet table (`pakt`) stores byte counts only, per the
/// CAF specification for constant-frames-per-packet formats.
pub struct CafAlacFileMuxer {
    path: PathBuf,
    sample_rate: u32,
    channels: u16,
    /// The ALAC magic cookie (decoder configuration) for the `kuki` chunk.
    magic_cookie: Vec<u8>,
    /// One entry per ALAC packet.
    packets: Vec<Vec<u8>>,
    /// Total valid sample-frames across all packets. Set directly for
    /// tests, or read from a shared encoder counter at trailer time (the
    /// muxer cannot derive it from opaque ALAC payloads).
    total_frames: u64,
    /// Exact counter shared with the encoder; preferred over
    /// `total_frames` when present.
    counter: Option<crate::audio_adapters::SharedSampleCounter>,
    streams: Vec<StreamInfo>,
    config: MuxerConfig,
}

impl CafAlacFileMuxer {
    /// Creates a CAF-ALAC sink writing to `path`.
    #[must_use]
    pub fn new(path: PathBuf, sample_rate: u32, channels: u16, magic_cookie: Vec<u8>) -> Self {
        Self {
            path,
            sample_rate,
            channels,
            magic_cookie,
            packets: Vec::new(),
            total_frames: 0,
            counter: None,
            streams: Vec::new(),
            config: MuxerConfig::new(),
        }
    }

    /// Records the total number of valid sample-frames (per channel) that
    /// the packet stream encodes; required for the `pakt` header.
    pub fn set_total_frames(&mut self, frames: u64) {
        self.total_frames = frames;
    }

    /// Reads the exact total from the encoder's shared counter at trailer
    /// time (takes precedence over [`set_total_frames`](Self::set_total_frames)).
    pub fn set_sample_counter(&mut self, counter: crate::audio_adapters::SharedSampleCounter) {
        self.counter = Some(counter);
    }

    /// Encode a CAF variable-length integer (BER style: 7 bits per byte,
    /// high bit set on all but the last byte).
    fn encode_vint(mut value: u64, out: &mut Vec<u8>) {
        let mut groups = [0u8; 10];
        let mut n = 0;
        loop {
            groups[n] = (value & 0x7F) as u8;
            n += 1;
            value >>= 7;
            if value == 0 {
                break;
            }
        }
        for i in (0..n).rev() {
            let mut byte = groups[i];
            if i != 0 {
                byte |= 0x80;
            }
            out.push(byte);
        }
    }

    /// Total valid frames, preferring the live encoder counter.
    fn effective_total_frames(&self) -> u64 {
        self.counter.as_ref().map_or(self.total_frames, |c| {
            c.load(std::sync::atomic::Ordering::Relaxed)
        })
    }

    /// Assemble the complete CAF byte stream.
    fn assemble(&self) -> Vec<u8> {
        let mut out = Vec::new();

        // File header: 'caff' + version 1 + flags 0.
        out.extend_from_slice(b"caff");
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());

        // desc chunk: AudioStreamBasicDescription (32 bytes).
        out.extend_from_slice(b"desc");
        out.extend_from_slice(&32i64.to_be_bytes());
        out.extend_from_slice(&f64::from(self.sample_rate).to_be_bytes());
        out.extend_from_slice(b"alac"); // format id
        out.extend_from_slice(&0u32.to_be_bytes()); // format flags
        out.extend_from_slice(&0u32.to_be_bytes()); // bytes per packet (variable)
        out.extend_from_slice(&CAF_ALAC_FRAMES_PER_PACKET.to_be_bytes());
        out.extend_from_slice(&u32::from(self.channels).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes()); // bits per channel (compressed)

        // kuki chunk: the ALAC magic cookie.
        out.extend_from_slice(b"kuki");
        out.extend_from_slice(&(self.magic_cookie.len() as i64).to_be_bytes());
        out.extend_from_slice(&self.magic_cookie);

        // pakt chunk: header + byte-count VInts (constant frames/packet).
        let total_frames = self.effective_total_frames();
        let mut pakt = Vec::new();
        let num_packets = self.packets.len() as i64;
        let capacity =
            (self.packets.len() as u64).saturating_mul(u64::from(CAF_ALAC_FRAMES_PER_PACKET));
        let remainder = capacity.saturating_sub(total_frames);
        pakt.extend_from_slice(&num_packets.to_be_bytes());
        pakt.extend_from_slice(&(total_frames as i64).to_be_bytes());
        pakt.extend_from_slice(&0i32.to_be_bytes()); // priming frames
        pakt.extend_from_slice(&(remainder.min(i64::MAX as u64) as i32).to_be_bytes());
        for pkt in &self.packets {
            Self::encode_vint(pkt.len() as u64, &mut pakt);
        }
        out.extend_from_slice(b"pakt");
        out.extend_from_slice(&(pakt.len() as i64).to_be_bytes());
        out.extend_from_slice(&pakt);

        // data chunk: edit count + concatenated ALAC packets.
        let payload_len: usize = self.packets.iter().map(Vec::len).sum();
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(4 + payload_len as i64).to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes()); // edit count
        for pkt in &self.packets {
            out.extend_from_slice(pkt);
        }

        out
    }
}

#[async_trait]
impl Muxer for CafAlacFileMuxer {
    fn add_stream(&mut self, info: StreamInfo) -> OxiResult<usize> {
        if !self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "CAF output supports exactly one audio stream".into(),
            ));
        }
        self.streams.push(info);
        Ok(0)
    }

    async fn write_header(&mut self) -> OxiResult<()> {
        if self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "add_stream must be called before write_header".into(),
            ));
        }
        Ok(())
    }

    async fn write_packet(&mut self, packet: &Packet) -> OxiResult<()> {
        if packet.data.is_empty() {
            return Ok(());
        }
        self.packets.push(packet.data.to_vec());
        Ok(())
    }

    async fn write_trailer(&mut self) -> OxiResult<()> {
        let bytes = self.assemble();
        tokio::fs::write(&self.path, &bytes).await?;
        Ok(())
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn config(&self) -> &MuxerConfig {
        &self.config
    }
}

// ─── Y4mFileMuxer ─────────────────────────────────────────────────────────────

/// Raw YUV 4:2:0 output via the container crate's Y4M writer.
///
/// Every packet payload must be one complete flat 4:2:0 frame of the
/// configured dimensions.
pub struct Y4mFileMuxer {
    inner: Option<oximedia_container::mux::Y4mMuxer<std::io::BufWriter<std::fs::File>>>,
    path: PathBuf,
    width: u32,
    height: u32,
    fps: (u32, u32),
    streams: Vec<StreamInfo>,
    config: MuxerConfig,
}

impl Y4mFileMuxer {
    /// Creates a Y4M sink for `width`×`height` frames at `fps`.
    #[must_use]
    pub fn new(path: PathBuf, width: u32, height: u32, fps: (u32, u32)) -> Self {
        Self {
            inner: None,
            path,
            width,
            height,
            fps,
            streams: Vec::new(),
            config: MuxerConfig::new(),
        }
    }
}

#[async_trait]
impl Muxer for Y4mFileMuxer {
    fn add_stream(&mut self, info: StreamInfo) -> OxiResult<usize> {
        if !self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "Y4M output supports exactly one video stream".into(),
            ));
        }
        self.streams.push(info);
        Ok(0)
    }

    async fn write_header(&mut self) -> OxiResult<()> {
        let file = std::fs::File::create(&self.path)?;
        let muxer = Y4mMuxerBuilder::new(self.width, self.height)
            .fps(self.fps.0.max(1), self.fps.1.max(1))
            .build(std::io::BufWriter::new(file))?;
        self.inner = Some(muxer);
        Ok(())
    }

    async fn write_packet(&mut self, packet: &Packet) -> OxiResult<()> {
        match self.inner.as_mut() {
            Some(muxer) => muxer.write_frame(&packet.data),
            None => Err(OxiError::Unsupported(
                "write_header must be called before write_packet".into(),
            )),
        }
    }

    async fn write_trailer(&mut self) -> OxiResult<()> {
        if let Some(mut muxer) = self.inner.take() {
            muxer.finish()?;
            let mut writer = muxer.into_writer()?;
            writer.flush()?;
        }
        Ok(())
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn config(&self) -> &MuxerConfig {
        &self.config
    }
}

// ─── Ffv1RawFileMuxer ─────────────────────────────────────────────────────────

/// Magic bytes identifying this crate's private raw-FFV1 framing.
pub const FFV1_RAW_MAGIC: &[u8; 8] = b"OXIFFV1\0";

/// Writes FFV1 packets into a small length-prefixed private file:
///
/// ```text
/// [8B magic "OXIFFV1\0"] [1B format version = 1]
/// [4B width LE] [4B height LE] [4B fps_num LE] [4B fps_den LE]
/// [4B extradata_len LE] [extradata_len B extradata]
/// repeated: [4B frame_len LE] [frame_len B frame payload]
/// ```
///
/// `extradata` must be the encoder's
/// [`Ffv1Encoder::extradata`](oximedia_codec::Ffv1Encoder::extradata)
/// output — [`crate::frame_adapters::Ffv1FrameDecoder`] passes it straight
/// to `Ffv1Decoder::with_extradata`. Every FFV1 packet here is a complete,
/// independently-decodable intra frame (the frame-level encoder always
/// configures `keyint = 1`), so frames need no inter-frame ordering
/// guarantee beyond append order.
pub struct Ffv1RawFileMuxer {
    path: PathBuf,
    width: u32,
    height: u32,
    fps: (u32, u32),
    extradata: Vec<u8>,
    frames: Vec<Vec<u8>>,
    streams: Vec<StreamInfo>,
    config: MuxerConfig,
}

impl Ffv1RawFileMuxer {
    /// Creates a raw-FFV1 sink writing to `path`.
    #[must_use]
    pub fn new(
        path: PathBuf,
        width: u32,
        height: u32,
        fps: (u32, u32),
        extradata: Vec<u8>,
    ) -> Self {
        Self {
            path,
            width,
            height,
            fps,
            extradata,
            frames: Vec::new(),
            streams: Vec::new(),
            config: MuxerConfig::new(),
        }
    }

    fn assemble(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(FFV1_RAW_MAGIC);
        out.push(1); // format version
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.fps.0.to_le_bytes());
        out.extend_from_slice(&self.fps.1.to_le_bytes());
        out.extend_from_slice(&(self.extradata.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.extradata);
        for frame in &self.frames {
            out.extend_from_slice(&(frame.len() as u32).to_le_bytes());
            out.extend_from_slice(frame);
        }
        out
    }
}

#[async_trait]
impl Muxer for Ffv1RawFileMuxer {
    fn add_stream(&mut self, info: StreamInfo) -> OxiResult<usize> {
        if !self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "raw FFV1 output supports exactly one video stream".into(),
            ));
        }
        self.streams.push(info);
        Ok(0)
    }

    async fn write_header(&mut self) -> OxiResult<()> {
        if self.streams.is_empty() {
            return Err(OxiError::Unsupported(
                "add_stream must be called before write_header".into(),
            ));
        }
        Ok(())
    }

    async fn write_packet(&mut self, packet: &Packet) -> OxiResult<()> {
        self.frames.push(packet.data.to_vec());
        Ok(())
    }

    async fn write_trailer(&mut self) -> OxiResult<()> {
        let bytes = self.assemble();
        tokio::fs::write(&self.path, &bytes).await?;
        Ok(())
    }

    fn streams(&self) -> &[StreamInfo] {
        &self.streams
    }

    fn config(&self) -> &MuxerConfig {
        &self.config
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vint_roundtrip_against_container_decoder() {
        // Encode with our writer, decode with a local mirror of the CAF
        // 7-bit BER rule to confirm the wire format.
        fn decode(data: &[u8]) -> (u64, usize) {
            let mut value = 0u64;
            let mut offset = 0;
            loop {
                let byte = data[offset];
                offset += 1;
                value = (value << 7) | u64::from(byte & 0x7F);
                if byte & 0x80 == 0 {
                    break;
                }
            }
            (value, offset)
        }

        for v in [0u64, 1, 127, 128, 300, 16_383, 16_384, 1_000_000] {
            let mut buf = Vec::new();
            CafAlacFileMuxer::encode_vint(v, &mut buf);
            let (decoded, consumed) = decode(&buf);
            assert_eq!(decoded, v, "vint round-trip failed for {v}");
            assert_eq!(consumed, buf.len());
        }
    }

    #[test]
    fn test_caf_assembly_layout() {
        let mut muxer = CafAlacFileMuxer::new(
            std::env::temp_dir().join("oximedia_caf_layout_test.caf"),
            44_100,
            2,
            vec![0xAA; 24],
        );
        muxer.packets.push(vec![1, 2, 3]);
        muxer.packets.push(vec![4, 5]);
        muxer.set_total_frames(4096 + 100);

        let bytes = muxer.assemble();
        assert_eq!(&bytes[..4], b"caff");
        assert_eq!(&bytes[8..12], b"desc");
        // desc payload: sample rate f64 BE at offset 20.
        let sr = f64::from_be_bytes(bytes[20..28].try_into().expect("8 bytes"));
        assert!((sr - 44_100.0).abs() < 1e-9);
        assert_eq!(&bytes[28..32], b"alac");
        // kuki chunk follows the 32-byte desc payload.
        assert_eq!(&bytes[52..56], b"kuki");
        // The file must contain pakt and data chunk markers.
        let hay = bytes.windows(4);
        assert!(hay.clone().any(|w| w == b"pakt"), "missing pakt chunk");
        assert!(hay.clone().any(|w| w == b"data"), "missing data chunk");
        // data payload = concatenated packets at the tail.
        assert_eq!(&bytes[bytes.len() - 5..], &[1, 2, 3, 4, 5]);
    }

    #[tokio::test]
    async fn test_raw_es_muxer_concatenates() {
        use bytes::Bytes;
        use oximedia_container::PacketFlags;
        use oximedia_core::{CodecId, Rational, Timestamp};

        let path =
            std::env::temp_dir().join(format!("oximedia_raw_es_test_{}.m2v", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut muxer = RawEsFileMuxer::new(path.clone());
        let si = StreamInfo::new(0, CodecId::Mpeg2, Rational::new(1, 1_000));
        muxer.add_stream(si).expect("add stream");
        muxer.write_header().await.expect("header");
        for (i, payload) in [&b"AAA"[..], &b"BB"[..]].iter().enumerate() {
            let pkt = Packet::new(
                0,
                Bytes::copy_from_slice(payload),
                Timestamp::new(i as i64 * 40, Rational::new(1, 1_000)),
                PacketFlags::KEYFRAME,
            );
            muxer.write_packet(&pkt).await.expect("packet");
        }
        muxer.write_trailer().await.expect("trailer");

        let written = std::fs::read(&path).expect("read back");
        assert_eq!(written, b"AAABB");
        std::fs::remove_file(&path).ok();
    }
}
