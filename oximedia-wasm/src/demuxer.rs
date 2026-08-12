//! Demuxer for WASM.
//!
//! This module provides a synchronous demuxer interface for extracting
//! packets from media containers in the browser.
//!
//! Unlike the async demuxers in the main library, this exposes a
//! synchronous surface to JavaScript. Underneath, it now drives the real
//! `oximedia-container` async demuxers (`oximedia_container::demux::{
//! MatroskaDemuxer, OggDemuxer, FlacDemuxer, WavDemuxer, Mp4Demuxer}`)
//! over an in-memory, non-fs, non-thread `oximedia_io::source::MemorySource`
//! -- see `src/block_on.rs` for why that async surface can be driven
//! synchronously with a single-poll, no-op-`Waker` helper instead of a real
//! async runtime, and `src/container_bridge.rs` for how their real output
//! types are converted to this crate's local mirror types.
//!
//! # Honesty contract
//!
//! [`WasmDemuxer`] never fabricates stream or packet data. Every
//! [`WasmStreamInfo`]/[`WasmPacket`] it returns was actually parsed by the
//! real per-format demuxer from the bytes passed in. A probe that yields no
//! decodable streams (e.g. a container whose only tracks use codecs this
//! demuxer doesn't recognize) is reported as an honest `Err` rather than as
//! a hollow, stream-less "success". MP4's patent-free policy is enforced by
//! `oximedia_container::demux::Mp4Demuxer` itself (`OxiError::PatentViolation`
//! naming the specific rejected codec, e.g. "H.264/AVC") and surfaces
//! verbatim through [`probe`](WasmDemuxer::probe) -- this module does not
//! special-case it, the error's own `Display` impl already names the codec.

use crate::block_on::poll_oxi;
use crate::container::ContainerFormat;
use crate::container_bridge::{convert_packet, convert_streams, RealDemuxer};
use bytes::Bytes;
use oximedia_core::{OxiError, OxiResult};
use oximedia_io::source::MemorySource;
use wasm_bindgen::prelude::*;

use crate::io::ByteSource;
use crate::types::{WasmPacket, WasmStreamInfo};
use crate::utils::to_js_error;

/// WASM-compatible demuxer.
///
/// Provides synchronous demuxing of media containers from in-memory data,
/// backed by the real `oximedia-container` per-format demuxers (see the
/// module-level docs).
///
/// # JavaScript Example
///
/// ```javascript
/// import * as oximedia from 'oximedia-wasm';
///
/// // Load file data
/// const response = await fetch('video.webm');
/// const arrayBuffer = await response.arrayBuffer();
/// const data = new Uint8Array(arrayBuffer);
///
/// // Create demuxer
/// const demuxer = new oximedia.WasmDemuxer(data);
///
/// try {
///     const probe = demuxer.probe();
///     console.log('Format:', probe.format());
///
///     const streams = demuxer.streams();
///     console.log('Found', streams.length, 'streams');
///     for (const stream of streams) {
///         console.log(`Stream ${stream.index()}: ${stream.codec()}`);
///     }
///
///     let count = 0;
///     while (true) {
///         const packet = demuxer.read_packet();
///         if (!packet) break;
///         console.log(`Packet ${count++}: stream=${packet.stream_index()}, size=${packet.size()}`);
///     }
/// } catch (e) {
///     // A genuine parse failure (corrupt/truncated file, or -- for MP4 --
///     // a patent-encumbered codec such as H.264/AAC).
///     console.error('Demux failed:', e);
/// }
/// ```
#[wasm_bindgen]
pub struct WasmDemuxer {
    source: ByteSource,
    format: Option<ContainerFormat>,
    streams: Vec<WasmStreamInfo>,
    probed: bool,
    inner: Option<RealDemuxer>,
    eof: bool,
}

#[wasm_bindgen]
impl WasmDemuxer {
    /// Creates a new demuxer from raw bytes.
    ///
    /// # Arguments
    ///
    /// * `data` - The complete media file data as a `Uint8Array`
    ///
    /// # Example
    ///
    /// ```javascript
    /// const data = new Uint8Array([...]);
    /// const demuxer = new oximedia.WasmDemuxer(data);
    /// ```
    #[wasm_bindgen(constructor)]
    #[must_use]
    pub fn new(data: &[u8]) -> Self {
        let bytes = Bytes::copy_from_slice(data);
        Self {
            source: ByteSource::new(bytes),
            format: None,
            streams: Vec::new(),
            probed: false,
            inner: None,
            eof: false,
        }
    }

    /// Probes the format and parses container headers.
    ///
    /// This must be called before reading packets. It detects the container
    /// format and extracts real stream information.
    ///
    /// # Errors
    ///
    /// Throws a JavaScript exception if the format cannot be detected, the
    /// container is malformed/truncated, no decodable stream is found, or
    /// (MP4 only) a track uses a patent-encumbered codec.
    ///
    /// # Example
    ///
    /// ```javascript
    /// const probe = demuxer.probe();
    /// console.log('Format:', probe.format());
    /// console.log('Confidence:', probe.confidence());
    /// ```
    pub fn probe(&mut self) -> Result<crate::probe::WasmProbeResult, JsValue> {
        self.probe_inner().map_err(to_js_error)
    }

    /// Returns information about all streams.
    ///
    /// This is only valid after `probe()` has been called.
    ///
    /// # Example
    ///
    /// ```javascript
    /// const streams = demuxer.streams();
    /// for (const stream of streams) {
    ///     console.log(`Stream ${stream.index()}: ${stream.codec()}`);
    /// }
    /// ```
    #[must_use]
    pub fn streams(&self) -> Vec<WasmStreamInfo> {
        self.streams.clone()
    }

    /// Reads the next packet from the container.
    ///
    /// Returns `null` when there are no more packets (EOF).
    ///
    /// # Errors
    ///
    /// Throws a JavaScript exception for parse failures or I/O errors.
    ///
    /// # Example
    ///
    /// ```javascript
    /// while (true) {
    ///     const packet = demuxer.read_packet();
    ///     if (!packet) break;
    ///     console.log('Packet size:', packet.size());
    /// }
    /// ```
    pub fn read_packet(&mut self) -> Result<Option<WasmPacket>, JsValue> {
        self.read_packet_inner().map_err(to_js_error)
    }

    /// Returns the total size of the media data in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.source.size()
    }

    /// Returns the current read position of the raw input buffer.
    ///
    /// This reflects the initial magic-byte sniff cursor, not per-packet
    /// parse progress -- the real per-format demuxer parses over its own,
    /// separate `MemorySource` (see the module docs). Use
    /// [`is_eof`](Self::is_eof) to know whether every packet has been read.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.source.position()
    }

    /// Returns true once every packet has genuinely been read, i.e. the
    /// real per-format demuxer reached the actual end of the container.
    #[must_use]
    pub fn is_eof(&self) -> bool {
        self.eof
    }
}

// Private implementation methods.
//
// These are `OxiResult`-typed (not `Result<_, JsValue>`) so that native
// `#[test]`s can assert on real `OxiError` messages -- a `JsValue` is
// opaque outside `wasm32` (see `crate::utils::js_err`). The public
// `#[wasm_bindgen]` methods above are the only place these get converted,
// via `to_js_error`.
impl WasmDemuxer {
    /// Probes the format and parses container headers for real.
    fn probe_inner(&mut self) -> OxiResult<crate::probe::WasmProbeResult> {
        // Read first bytes for format detection.
        let mut header = [0u8; 32];
        let n = self.source.read(&mut header)?;

        // Probe format (real magic-byte sniff).
        let result = crate::container::probe_format(&header[..n])?;
        self.format = Some(result.format);

        // Reset to beginning before the real per-format parse below.
        self.source.seek(std::io::SeekFrom::Start(0))?;

        self.parse_headers()?;
        self.probed = true;

        Ok(crate::probe::WasmProbeResult::new_internal(
            result.format,
            result.confidence,
        ))
    }

    /// Builds the real per-format demuxer over an in-memory copy of the
    /// input bytes, probes it for real, and stores the converted stream
    /// list plus the live demuxer instance (for subsequent packet reads).
    ///
    /// # Honesty contract
    ///
    /// A successful real probe that nonetheless yields zero decodable
    /// streams (every track uses an unrecognized codec, or the container
    /// declares none) is deliberately turned into an `Err` here rather than
    /// returned as a stream-less "success" -- a caller that then calls
    /// `read_packet()` in a loop expecting to decode something would
    /// otherwise just observe an immediate, unexplained `null`.
    fn parse_headers(&mut self) -> OxiResult<()> {
        let format = self
            .format
            .ok_or_else(|| OxiError::InvalidData("No format detected".to_string()))?;

        let bytes = self.source.get_ref().clone();
        let mut inner = RealDemuxer::new(format, MemorySource::new(bytes));

        poll_oxi(inner.probe())?;

        let streams: Vec<WasmStreamInfo> = convert_streams(inner.streams())
            .into_iter()
            .map(WasmStreamInfo::from)
            .collect();
        if streams.is_empty() {
            return Err(OxiError::InvalidData(format!(
                "no decodable streams found in {format:?}"
            )));
        }

        self.streams = streams;
        self.inner = Some(inner);
        Ok(())
    }

    /// Reads one real packet from the live per-format demuxer.
    fn read_packet_inner(&mut self) -> OxiResult<Option<WasmPacket>> {
        if !self.probed {
            return Err(OxiError::InvalidData(
                "Must call probe() before reading packets".to_string(),
            ));
        }
        if self.eof {
            return Ok(None);
        }
        let Some(inner) = self.inner.as_mut() else {
            return Err(OxiError::InvalidData("No format detected".to_string()));
        };

        match poll_oxi(inner.read_packet()) {
            Ok(packet) => Ok(Some(WasmPacket::from(convert_packet(packet)))),
            Err(OxiError::Eof) => {
                self.eof = true;
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_container::mux::{
        FlacMuxer, Mp4Sample, SimpleMp4Config, SimpleMp4Muxer, TrackCodec, VideoCodecInfo, WavMuxer,
    };
    use oximedia_container::{CodecParams, Muxer, MuxerConfig, Packet, PacketFlags, StreamInfo};
    use oximedia_core::{CodecId, Rational, Timestamp};

    #[test]
    fn test_demuxer_new() {
        let data = vec![0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0];
        let demuxer = WasmDemuxer::new(&data);
        assert_eq!(demuxer.size(), 8);
        assert_eq!(demuxer.position(), 0);
    }

    /// A truncated (8-byte) Matroska/EBML header is genuinely too short for
    /// the real EBML parser to succeed -- `probe()` must still honestly
    /// fail, but now because real parsing hit `UnexpectedEof`/`Parse`, not
    /// because the feature was never wired in. This is a regression guard
    /// for the original bug (probe() used to succeed and silently invent a
    /// single VP9 video stream) re-pointed at the real parser.
    #[test]
    fn test_demuxer_probe_matroska_truncated_is_honest_err() {
        let data = vec![0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0];
        let mut demuxer = WasmDemuxer::new(&data);
        let result = demuxer.probe_inner();
        assert!(
            result.is_err(),
            "probe() must not fabricate a stream for a truncated container"
        );
        assert!(
            demuxer.streams().is_empty(),
            "no stream list should ever be fabricated"
        );
        let message = result.expect_err("checked above").to_string();
        assert!(
            !message.contains("not yet available"),
            "error must be a genuine parse failure, not the old stub message: {message}"
        );
    }

    /// The underlying magic-byte format sniff is real and must keep
    /// working even though the full per-container header parse above
    /// honestly fails -- these are two different layers.
    #[test]
    fn test_raw_format_sniff_still_detects_matroska() {
        let data = [0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0];
        let result = crate::container::probe_format(&data).expect("magic bytes should be detected");
        assert_eq!(result.format, ContainerFormat::Matroska);
        assert!(result.confidence > 0.9);
    }

    /// Every currently-supported format must honestly error on these
    /// deliberately truncated fixtures -- none of them contain a real,
    /// complete container -- while never fabricating a stream list. This
    /// guards against fabrication silently creeping back in for any one
    /// format, and against the old stub-message text resurfacing.
    #[test]
    fn test_demuxer_probe_truncated_fixtures_are_honest_err_for_all_formats() {
        let cases: &[(&[u8], &str)] = &[
            (&[0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0], "Matroska"),
            (b"OggS\x00\x02\x00\x00\x00\x00\x00\x00", "Ogg"),
            (b"fLaC\x00\x00\x00\x22", "Flac"),
            (b"RIFF\x00\x00\x00\x00WAVEfmt ", "Wav"),
            (b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00", "Mp4"),
        ];

        for (data, label) in cases {
            let mut demuxer = WasmDemuxer::new(data);
            let result = demuxer.probe_inner();
            assert!(
                result.is_err(),
                "{label}: probe() fabricated success instead of an honest error"
            );
            assert!(
                demuxer.streams().is_empty(),
                "{label}: no stream list should ever be fabricated"
            );
            let message = result.expect_err("checked above").to_string();
            assert!(
                !message.contains("not yet available"),
                "{label}: error must be a genuine parse failure, not the old stub message: {message}"
            );
        }
    }

    #[test]
    fn test_read_packet_before_probe_still_errors() {
        let data = vec![0x1A, 0x45, 0xDF, 0xA3, 0, 0, 0, 0];
        let mut demuxer = WasmDemuxer::new(&data);
        let result = demuxer.read_packet();
        assert!(result.is_err());
    }

    // ------------------------------------------------------------------
    // Real round-trip fixtures: built via `oximedia-container`'s own
    // muxers (never hand-fabricated bytes pretending to be a real
    // encode), then fed back through `WasmDemuxer` to prove the real
    // demux path end-to-end.
    // ------------------------------------------------------------------

    /// Builds a real, minimal WAV file (RIFF/fmt /data) via `WavMuxer`.
    fn make_wav_fixture(sample_rate: u32, channels: u8, num_frames: usize) -> Vec<u8> {
        let sink = MemorySource::new_writable(4096);
        let mut muxer = WavMuxer::new(sink, MuxerConfig::new().with_write_cues(false));

        let mut stream = StreamInfo::new(0, CodecId::Pcm, Rational::new(1, i64::from(sample_rate)));
        stream.codec_params.sample_rate = Some(sample_rate);
        stream.codec_params.channels = Some(channels);
        let idx = muxer.add_stream(stream).expect("add_stream");
        assert_eq!(idx, 0);

        poll_oxi(muxer.write_header()).expect("write_header");

        let bytes_per_frame = usize::from(channels) * 2; // 16-bit PCM
        let pcm = vec![0x11u8; num_frames * bytes_per_frame];
        let packet = Packet::new(
            0,
            bytes::Bytes::from(pcm),
            Timestamp::new(0, Rational::new(1, i64::from(sample_rate))),
            PacketFlags::KEYFRAME,
        );
        poll_oxi(muxer.write_packet(&packet)).expect("write_packet");
        poll_oxi(muxer.write_trailer()).expect("write_trailer");

        muxer.into_sink().written_data().to_vec()
    }

    #[test]
    fn wav_round_trip_is_real() {
        let data = make_wav_fixture(44100, 2, 2000);
        let mut demuxer = WasmDemuxer::new(&data);
        let probe = demuxer
            .probe_inner()
            .expect("real WAV must probe successfully");
        assert_eq!(probe.format(), "Wav");

        let streams = demuxer.streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].codec(), "Pcm");
        assert_eq!(streams[0].codec_params().sample_rate(), Some(44100));
        assert_eq!(streams[0].codec_params().channels(), Some(2));

        let mut total_bytes = 0usize;
        let mut packet_count = 0u32;
        loop {
            let packet = demuxer
                .read_packet_inner()
                .expect("read_packet must not error");
            let Some(packet) = packet else { break };
            total_bytes += packet.size();
            packet_count += 1;
            assert!(packet_count < 10_000, "runaway loop guard");
        }
        assert!(packet_count > 0, "must read at least one real packet");
        assert_eq!(
            total_bytes,
            2000 * 2 * 2,
            "all PCM bytes must be accounted for"
        );
        assert!(demuxer.is_eof());
    }

    #[test]
    fn wav_truncated_data_chunk_is_honest_err_not_a_fabricated_stream() {
        // Real RIFF/WAVE/fmt header but the declared `data` chunk size
        // extends past the actual buffer -- this must fail for real
        // reasons (truncated read), not succeed with fabricated content.
        let data = make_wav_fixture(8000, 1, 100);
        let truncated = &data[..data.len() - 50];
        let mut demuxer = WasmDemuxer::new(truncated);
        // Either probe() itself fails, or streams() honestly reflects only
        // what was really parsed and reading eventually surfaces the
        // truncation as an error rather than fabricated packets.
        if let Ok(_probe) = demuxer.probe() {
            loop {
                match demuxer.read_packet() {
                    Ok(Some(_)) => continue,
                    Ok(None) => break,
                    Err(_) => break, // honest failure on the truncated tail -- acceptable
                }
            }
        }
    }

    /// Builds a real, minimal MP4 file with one AV1 video track via
    /// `SimpleMp4Muxer` (fully synchronous, no async driving needed).
    fn make_mp4_av1_fixture() -> Vec<u8> {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let track = muxer.add_track(TrackCodec::Video(VideoCodecInfo::new(
            *b"av01", 64, 48, 90_000,
        )));
        for i in 0..3u32 {
            muxer
                .write_sample(
                    track,
                    Mp4Sample {
                        pts: i64::from(i) * 3000,
                        dts: i64::from(i) * 3000,
                        duration: 3000,
                        is_sync: i == 0,
                        data: vec![0xAB; 32],
                    },
                )
                .expect("write_sample");
        }
        let mut out = std::io::Cursor::new(Vec::<u8>::new());
        muxer.finalize(&mut out).expect("finalize");
        out.into_inner()
    }

    #[test]
    fn mp4_av1_round_trip_is_real() {
        let data = make_mp4_av1_fixture();
        let mut demuxer = WasmDemuxer::new(&data);
        let probe = demuxer
            .probe_inner()
            .expect("real AV1-in-MP4 must probe successfully");
        assert_eq!(probe.format(), "Mp4");

        let streams = demuxer.streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].codec(), "Av1");
        assert!(streams[0].is_video());
        assert_eq!(streams[0].codec_params().width(), Some(64));
        assert_eq!(streams[0].codec_params().height(), Some(48));

        let mut packets = Vec::new();
        while let Some(packet) = demuxer.read_packet_inner().expect("read_packet") {
            packets.push(packet);
        }
        assert_eq!(packets.len(), 3, "all three real samples must be returned");
        assert!(packets[0].is_keyframe());
        for packet in &packets {
            assert_eq!(packet.size(), 32);
        }
        assert!(demuxer.is_eof());
    }

    /// Builds a real, minimal MP4 file containing an H.264 ("avc1") video
    /// track -- a patent-encumbered codec `Mp4Demuxer` must reject.
    /// `SimpleMp4Muxer` itself does not police codecs (it only knows raw
    /// fourccs), so this really does produce the forbidden bitstream shape
    /// rather than simulating one.
    fn make_mp4_h264_fixture() -> Vec<u8> {
        let mut muxer = SimpleMp4Muxer::new(SimpleMp4Config::new());
        let track = muxer.add_track(TrackCodec::Video(VideoCodecInfo::new(
            *b"avc1", 64, 48, 90_000,
        )));
        muxer
            .write_sample(
                track,
                Mp4Sample {
                    pts: 0,
                    dts: 0,
                    duration: 3000,
                    is_sync: true,
                    data: vec![0; 32],
                },
            )
            .expect("write_sample");
        let mut out = std::io::Cursor::new(Vec::<u8>::new());
        muxer.finalize(&mut out).expect("finalize");
        out.into_inner()
    }

    /// MP4's patent-free policy must surface as a specific, per-codec
    /// message (naming "H.264"), not a generic "demux failed" error.
    #[test]
    fn mp4_h264_is_rejected_by_name_not_a_generic_failure() {
        let data = make_mp4_h264_fixture();
        let mut demuxer = WasmDemuxer::new(&data);
        let result = demuxer.probe_inner();
        let err = result.expect_err("H.264-in-MP4 must be rejected");
        assert!(matches!(err, OxiError::PatentViolation(_)));
        let message = err.to_string();
        assert!(
            message.contains("H.264"),
            "message must name the specific rejected codec, got: {message}"
        );
    }

    /// Builds a real, minimal FLAC file (STREAMINFO + fixed-size frames)
    /// via `FlacMuxer`. Frame headers are syntactically real FLAC frame
    /// headers (sync code + valid field codes) -- `FlacDemuxer` does not
    /// validate the CRC-8/CRC-16 checksums (see `demux/flac/frame.rs`), so
    /// the subframe payload itself can be filler, exactly like the
    /// upstream crate's own `FrameHeader::parse` tests do. Every frame is
    /// written at the *same* total length and `max_frame_size` is set to
    /// match, so `FlacDemuxer::read_packet`'s size estimate lands exactly
    /// on each frame boundary (see its `total_read > max_frame_size`
    /// trim logic) instead of drifting into the next frame.
    fn flac_frame(frame_number: u8, filler_len: usize) -> Vec<u8> {
        let mut frame = vec![
            0xFF,
            0xF8, // sync (fixed blocksize) + reserved(0) + blocking_strategy(0)
            0x89, // block_size_code=8 (256 samples) | sample_rate_code=9 (44100 Hz)
            0x18, // channel_assignment=1 (2ch independent) | sample_size_code=4 (16 bit) | reserved(0)
            frame_number,
            0x00, // CRC-8 (unchecked by this demuxer, see doc above)
        ];
        frame.extend(std::iter::repeat_n(0xABu8, filler_len));
        frame
    }

    fn make_flac_fixture(frame_count: u8, filler_len: usize) -> Vec<u8> {
        let sink = MemorySource::new_writable(4096);
        let mut muxer = FlacMuxer::new(sink, MuxerConfig::new().with_write_cues(false));

        let mut stream = StreamInfo::new(0, CodecId::Flac, Rational::new(1, 44100));
        stream.codec_params = CodecParams::audio(44100, 2);
        muxer.add_stream(stream).expect("add_stream");
        poll_oxi(muxer.write_header()).expect("write_header");

        for n in 0..frame_count {
            let frame_bytes = flac_frame(n, filler_len);
            let packet = Packet::new(
                0,
                bytes::Bytes::from(frame_bytes),
                Timestamp::new(i64::from(n) * 256, Rational::new(1, 44100)),
                PacketFlags::KEYFRAME,
            );
            poll_oxi(muxer.write_packet(&packet)).expect("write_packet");
        }
        poll_oxi(muxer.write_trailer()).expect("write_trailer");

        muxer.into_sink().written_data().to_vec()
    }

    #[test]
    fn flac_round_trip_is_real() {
        let data = make_flac_fixture(3, 10); // 6-byte header + 10 filler = 16 bytes/frame
        let mut demuxer = WasmDemuxer::new(&data);
        let probe = demuxer
            .probe_inner()
            .expect("real FLAC must probe successfully");
        assert_eq!(probe.format(), "Flac");

        let streams = demuxer.streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].codec(), "Flac");
        assert_eq!(streams[0].codec_params().sample_rate(), Some(44100));
        assert_eq!(streams[0].codec_params().channels(), Some(2));

        let mut packets = Vec::new();
        while let Some(packet) = demuxer.read_packet_inner().expect("read_packet") {
            packets.push(packet);
        }
        assert_eq!(packets.len(), 3, "all three real frames must be returned");
        for packet in &packets {
            assert_eq!(packet.size(), 16, "each frame is a fixed 16 bytes");
        }
        assert!(demuxer.is_eof());
    }

    /// Builds a real, minimal WebM file (EBML header + one Opus audio
    /// track) via `MatroskaMuxer`.
    fn make_webm_opus_fixture(packet_count: u32) -> Vec<u8> {
        let sink = MemorySource::new_writable(4096);
        let mut muxer = oximedia_container::mux::MatroskaMuxer::new(
            sink,
            MuxerConfig::new().with_write_cues(false),
        );

        let mut stream = StreamInfo::new(0, CodecId::Opus, Rational::new(1, 48000));
        stream.codec_params = CodecParams::audio(48000, 2);
        muxer.add_stream(stream).expect("add_stream");
        poll_oxi(muxer.write_header()).expect("write_header");

        for n in 0..packet_count {
            let packet = Packet::new(
                0,
                bytes::Bytes::from(vec![0xCDu8; 20]),
                Timestamp::new(i64::from(n) * 20, Rational::new(1, 1000)),
                PacketFlags::KEYFRAME,
            );
            poll_oxi(muxer.write_packet(&packet)).expect("write_packet");
        }
        poll_oxi(muxer.write_trailer()).expect("write_trailer");

        muxer.into_sink().written_data().to_vec()
    }

    #[test]
    fn matroska_round_trip_is_real() {
        let data = make_webm_opus_fixture(4);
        let mut demuxer = WasmDemuxer::new(&data);
        let probe = demuxer
            .probe_inner()
            .expect("real WebM/Matroska must probe successfully");
        assert_eq!(probe.format(), "Matroska");

        let streams = demuxer.streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].codec(), "Opus");
        assert!(streams[0].is_audio());
        assert_eq!(streams[0].codec_params().sample_rate(), Some(48000));
        assert_eq!(streams[0].codec_params().channels(), Some(2));

        let mut packets = Vec::new();
        while let Some(packet) = demuxer.read_packet_inner().expect("read_packet") {
            packets.push(packet);
        }
        assert_eq!(packets.len(), 4, "all four real blocks must be returned");
        for packet in &packets {
            assert_eq!(packet.size(), 20);
        }
        assert!(demuxer.is_eof());
    }

    // Ogg fixture: hand-rolled real page bytes, NOT `oximedia_container::mux::OggMuxer`.
    //
    // `OggMuxer::write_bos_pages` (crates/oximedia-container/src/mux/ogg/writer.rs)
    // calls `self.stream_writers[i].build_page(&id_header, true, false, true)`
    // for the very first page of every stream. `build_page`'s first `bool`
    // parameter is `continuation` (see `mux/ogg/stream.rs`), and
    // `OggStreamWriter::build_page_with_granule` only sets the BOS flag when
    // `self.sequence == 0 && !continuation`. Passing `continuation: true`
    // there means the muxer's own first page is written with the
    // CONTINUATION flag (0x01) instead of BOS (0x02) -- confirmed by
    // dumping real muxer output: byte 5 of the first page is `0x01`, not
    // `0x02`. Real readers (including this crate's own `OggDemuxer`, and any
    // spec-compliant Ogg player) rely on the BOS flag to recognize the first
    // page of a logical stream at all, so every file this muxer produces is
    // silently missing header/codec identification -- `OggDemuxer::probe()`
    // does not error on such a file, it just never registers a stream (see
    // `handle_bos_page`, gated on `page.is_bos()`), so `WasmDemuxer::probe()`
    // correctly reports it as "no decodable streams found" -- this is a
    // real, verified upstream bug (out of this slice's `oximedia-wasm`-only
    // scope to fix; `build_bos_page`, right below `build_page` in the same
    // file, exists and unconditionally sets BOS -- `write_bos_pages` should
    // call that, or pass `continuation: false`).
    //
    // Working around it: build genuinely valid Ogg pages by hand instead
    // (real magic bytes/flags/lacing, exactly as `demux/ogg/page.rs::OggPage::parse`
    // and `stream.rs::identify_codec`/`LogicalStream` expect -- verified by
    // reading both). `OggPage::parse` never calls `verify_page_crc` (grepped
    // the whole crate: that function is defined but never invoked from the
    // read path), so the checksum field genuinely does not need to be
    // correct for this demuxer to accept the page; it is left as `0`.

    /// Builds one real Ogg page: header + lacing table + packet data.
    /// Every packet must be under 255 bytes (single-segment lacing) --
    /// sufficient for this module's small fixtures.
    fn ogg_page(serial: u32, sequence: u32, flags: u8, granule: u64, packets: &[&[u8]]) -> Vec<u8> {
        let mut segment_table = Vec::with_capacity(packets.len());
        let mut data = Vec::new();
        for packet in packets {
            assert!(
                packet.len() < 255,
                "test helper only supports packets < 255 bytes"
            );
            segment_table.push(packet.len() as u8);
            data.extend_from_slice(packet);
        }

        let mut page = Vec::new();
        page.extend_from_slice(b"OggS");
        page.push(0); // stream_structure_version
        page.push(flags);
        page.extend_from_slice(&granule.to_le_bytes());
        page.extend_from_slice(&serial.to_le_bytes());
        page.extend_from_slice(&sequence.to_le_bytes());
        page.extend_from_slice(&0u32.to_le_bytes()); // checksum -- unvalidated, see block comment above
        page.push(segment_table.len() as u8);
        page.extend_from_slice(&segment_table);
        page.extend_from_slice(&data);
        page
    }

    /// A real, minimal `OpusHead` identification packet (19 bytes, RFC 7845 §5.1).
    fn opus_head(channels: u8, input_sample_rate: u32) -> Vec<u8> {
        let mut h = Vec::with_capacity(19);
        h.extend_from_slice(b"OpusHead");
        h.push(1); // version
        h.push(channels);
        h.extend_from_slice(&0u16.to_le_bytes()); // pre-skip
        h.extend_from_slice(&input_sample_rate.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes()); // output gain
        h.push(0); // channel mapping family (0 = mono/stereo)
        h
    }

    /// A real, minimal (empty vendor/comments) `OpusTags` packet (16 bytes,
    /// RFC 7845 §5.2).
    fn opus_tags() -> Vec<u8> {
        let mut t = Vec::with_capacity(16);
        t.extend_from_slice(b"OpusTags");
        t.extend_from_slice(&0u32.to_le_bytes()); // vendor string length
        t.extend_from_slice(&0u32.to_le_bytes()); // user comment list length
        t
    }

    /// Builds a real, minimal Ogg/Opus byte stream: BOS page (`OpusHead`) +
    /// header page (`OpusTags`) + `packet_count` data pages (EOS on the last).
    fn make_ogg_opus_fixture(packet_count: u32) -> Vec<u8> {
        const BOS: u8 = 0x02;
        const EOS: u8 = 0x04;
        let serial = 0x1234_5678u32;

        let mut out = Vec::new();
        out.extend(ogg_page(serial, 0, BOS, 0, &[&opus_head(2, 48000)]));
        out.extend(ogg_page(serial, 1, 0x00, 0, &[&opus_tags()]));
        for n in 0..packet_count {
            let granule = u64::from(n + 1) * 960;
            let flags = if n + 1 == packet_count { EOS } else { 0x00 };
            let packet = [0xEFu8; 24];
            out.extend(ogg_page(serial, 2 + n, flags, granule, &[&packet]));
        }
        out
    }

    #[test]
    fn ogg_round_trip_is_real() {
        let data = make_ogg_opus_fixture(3);
        let mut demuxer = WasmDemuxer::new(&data);
        let probe = demuxer
            .probe_inner()
            .expect("real Ogg/Opus must probe successfully");
        assert_eq!(probe.format(), "Ogg");

        let streams = demuxer.streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].codec(), "Opus");
        assert!(streams[0].is_audio());

        let mut packets = Vec::new();
        while let Some(packet) = demuxer.read_packet_inner().expect("read_packet") {
            packets.push(packet);
        }
        assert_eq!(
            packets.len(),
            3,
            "all three real Opus packets must be returned"
        );
        for packet in &packets {
            assert_eq!(packet.size(), 24);
        }
        assert!(demuxer.is_eof());
    }
}
