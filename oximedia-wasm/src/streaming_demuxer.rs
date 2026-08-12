//! Incremental streaming demuxer for WASM.
//!
//! This module provides a chunk-oriented demuxer that can accept data
//! progressively — suitable for network streaming scenarios in the browser
//! where bytes arrive in pieces (e.g. `ReadableStream` chunks) rather than
//! as a complete file upfront.
//!
//! # Design
//!
//! Unlike [`WasmDemuxer`](crate::demuxer::WasmDemuxer) which requires the
//! full file in memory at construction time, `WasmStreamingDemuxer` maintains
//! an internal growable buffer.  Callers push chunks with
//! [`append_data`](WasmStreamingDemuxer::append_data) and pull packets with
//! [`read_packet`](WasmStreamingDemuxer::read_packet).  The demuxer signals
//! "not enough data yet" by returning `null` from `read_packet`.
//!
//! # How real demux works over a growing buffer
//!
//! The real per-format demuxers in `oximedia-container` (driven the same
//! way as [`WasmDemuxer`](crate::demuxer::WasmDemuxer), see `src/block_on.rs`
//! and `src/container_bridge.rs`) were designed against a whole, static
//! `MediaSource` -- several of them (Matroska's `ensure_buffer`, in
//! particular) latch an internal "source is exhausted" flag the first time
//! a read comes back short, and never retry even if the caller later
//! supplies more bytes to that same instance. Rather than fight that with
//! per-format patches, this demuxer takes the simple, honest-by-construction
//! approach: on every [`read_packet`](WasmStreamingDemuxer::read_packet)
//! call, it builds a **fresh** demuxer over the **entire** accumulated
//! buffer, probes it for real, replays (discarding) the packets already
//! emitted so far, then reads one more. This is `O(n)` work per call (so
//! `O(n^2)` over a full stream) rather than `O(1)`, but it is always
//! correct: every field the real demuxer tracks (cluster timecodes, sample
//! counters, DTS accumulators, ...) is rebuilt exactly as it would be from
//! a linear parse, because it *is* one. Given this crate's role (an
//! in-browser convenience API, not a hot codec loop), correctness-over-
//! throughput is the right tradeoff. A consequence: the full buffer must
//! stay resident for the life of the stream, which is why
//! [`bytes_consumed`](WasmStreamingDemuxer::bytes_consumed) never advances
//! past `0` and [`append_data`](WasmStreamingDemuxer::append_data) never
//! evicts already-"consumed" bytes -- there is no such thing as a
//! consumed-and-safe-to-discard prefix in this design.
//!
//! # Honesty contract
//!
//! Like [`WasmDemuxer`](crate::demuxer::WasmDemuxer), this never fabricates
//! stream or packet data. [`read_packet`](WasmStreamingDemuxer::read_packet)
//! returns `null` while there is genuinely not enough data buffered to
//! parse the next real packet, and honestly throws on a genuine parse
//! failure -- it never invents a stream list or wraps an arbitrary byte
//! window in a fake packet. It also never reports a packet whose length
//! could still grow once more bytes arrive: several real per-format
//! demuxers (e.g. `FlacDemuxer`, `WavDemuxer`) silently return a *short*
//! packet rather than erroring when their source runs out mid-frame, which
//! would be indistinguishable from a genuine short final frame without the
//! peek-ahead guard in [`demux_nth_packet`](WasmStreamingDemuxer::demux_nth_packet).
//!
//! # JavaScript Example
//!
//! ```javascript
//! import * as oximedia from 'oximedia-wasm';
//!
//! const sd = new oximedia.WasmStreamingDemuxer("webm");
//! try {
//!     for await (const chunk of response.body) {
//!         sd.append_data(chunk);
//!         let packet;
//!         while ((packet = sd.read_packet()) !== null) {
//!             console.log('Packet size:', packet.size());
//!         }
//!     }
//!     sd.flush();
//!     let packet;
//!     while ((packet = sd.read_packet()) !== null) {
//!         console.log('Final packet size:', packet.size());
//!     }
//! } catch (e) {
//!     console.error('Demux failed:', e);
//! }
//! ```

use wasm_bindgen::prelude::*;

use crate::block_on::poll_oxi;
use crate::container::ContainerFormat;
use crate::container_bridge::{convert_packet, convert_streams, RealDemuxer};
use crate::types::{WasmPacket, WasmStreamInfo};
use oximedia_core::OxiError;
use oximedia_io::source::MemorySource;

// ---------------------------------------------------------------------------
// Internal ring-buffer of chunks

/// Maximum number of bytes we buffer before forcing a read.
///
/// After this threshold, `append_data` refuses further chunks (see the
/// module docs for why bytes are never evicted from the front: this
/// implementation rebuilds demuxer state from the *entire* buffer on every
/// read, so there is no "already consumed, safe to discard" prefix).
const MAX_BUFFER_BYTES: usize = 32 * 1024 * 1024; // 32 MiB

/// Minimum number of buffered bytes before even attempting a real header
/// probe. Below this, a "not enough data" result is not informative --
/// every supported format's magic/header needs at least this many bytes,
/// so trying earlier would just mean repeatedly re-parsing a buffer that
/// cannot possibly succeed yet.
const MIN_PROBE_BYTES: usize = 32;

// ---------------------------------------------------------------------------

/// Incremental (streaming) demuxer WASM binding.
///
/// Accepts data chunks pushed from JavaScript and emits real packets as
/// soon as enough data is available to parse them.
///
/// The demuxer operates fully synchronously — no async/await or threads.
///
/// # Supported format strings
///
/// | String | Container |
/// |--------|-----------|
/// | `"webm"` / `"matroska"` / `"mkv"` | Matroska / WebM |
/// | `"ogg"` | Ogg |
/// | `"flac"` | FLAC |
/// | `"wav"` | WAV |
/// | `"mp4"` / `"mov"` | MP4 / ISOBMFF |
#[wasm_bindgen]
pub struct WasmStreamingDemuxer {
    /// Accumulated bytes not yet consumed by packet reader.
    buffer: Vec<u8>,
    /// Detected/declared container format.
    format: ContainerFormat,
    /// Whether a real header probe has succeeded.
    probed: bool,
    /// Streams discovered during probing (real, from the last successful
    /// probe/replay -- refreshed as more data lets the real demuxer refine
    /// duration/track info).
    streams: Vec<WasmStreamInfo>,
    /// Number of real packets emitted so far.
    packet_count: u64,
    /// Whether `flush()` has been called — no more data will arrive.
    flushed: bool,
    /// Whether the stream has been fully, genuinely drained: `flush()` was
    /// called and the most recent `read_packet()` attempt found nothing
    /// further to return.
    exhausted: bool,
}

#[wasm_bindgen]
impl WasmStreamingDemuxer {
    /// Create a streaming demuxer for the specified container format.
    ///
    /// # Arguments
    ///
    /// * `format_hint` - A case-insensitive string identifying the container
    ///   format (e.g. `"webm"`, `"ogg"`, `"wav"`).  The format is used to
    ///   guide packet parsing without requiring the entire file to be present.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript error if `format_hint` is not recognised.
    #[wasm_bindgen(constructor)]
    pub fn new(format_hint: &str) -> Result<WasmStreamingDemuxer, JsValue> {
        let format = parse_format_hint(format_hint).map_err(|e| crate::utils::js_err(&e))?;
        Ok(Self {
            buffer: Vec::new(),
            format,
            probed: false,
            streams: Vec::new(),
            packet_count: 0,
            flushed: false,
            exhausted: false,
        })
    }

    /// Append a chunk of raw bytes to the internal buffer.
    ///
    /// This method is cheap — it simply extends the buffer.  No parsing is
    /// performed until `read_packet()` is called.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer would exceed the 32 MiB safety limit
    /// after appending.
    pub fn append_data(&mut self, chunk: &[u8]) -> Result<(), JsValue> {
        self.append_data_inner(chunk)
            .map_err(|e| crate::utils::js_err(&e))
    }

    /// Attempt to read the next available packet from the buffer.
    ///
    /// Returns `null` if there is not yet enough data to form a complete
    /// packet — the caller should push more chunks and try again.
    ///
    /// Returns `null` after `flush()` has been called and the buffer is
    /// exhausted.
    ///
    /// # Errors
    ///
    /// Returns a JavaScript error on unrecoverable parse failures.
    pub fn read_packet(&mut self) -> Result<Option<WasmPacket>, JsValue> {
        self.read_packet_inner()
            .map_err(|e| crate::utils::js_err(&e))
    }

    /// Returns stream information discovered during header probing.
    ///
    /// The slice may be empty until enough header data has been received.
    pub fn streams(&self) -> Vec<WasmStreamInfo> {
        self.streams.clone()
    }

    /// Signal that no more data will be appended.
    ///
    /// After calling `flush()`, callers should drain remaining packets by
    /// calling `read_packet()` until it returns `null`.
    pub fn flush(&mut self) {
        self.flushed = true;
    }

    /// Returns the total number of bytes that have been consumed (read) so far.
    ///
    /// Always `0` in this implementation: see the module docs for why no
    /// prefix of `buffer` is ever safe to evict/report-consumed when every
    /// read rebuilds demuxer state from the full buffer.
    pub fn bytes_consumed(&self) -> u64 {
        0
    }

    /// Returns the number of bytes currently held in the buffer (including
    /// already-consumed but not yet evicted bytes).
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    /// Returns the number of packets emitted so far.
    pub fn packets_emitted(&self) -> u64 {
        self.packet_count
    }

    /// Returns `true` once `flush()` has been called and the most recent
    /// `read_packet()` attempt genuinely found nothing further to return.
    pub fn is_done(&self) -> bool {
        self.exhausted
    }
}

// ---------------------------------------------------------------------------
// Private helpers

impl WasmStreamingDemuxer {
    /// Inner impl of `append_data` — returns `String` error, no `JsValue`.
    fn append_data_inner(&mut self, chunk: &[u8]) -> Result<(), String> {
        if self.buffer.len() + chunk.len() > MAX_BUFFER_BYTES {
            return Err(
                "WasmStreamingDemuxer: buffer overflow — call read_packet() more frequently"
                    .to_string(),
            );
        }
        self.buffer.extend_from_slice(chunk);
        Ok(())
    }

    /// Inner impl of `read_packet` — returns `String` error, no `JsValue`.
    fn read_packet_inner(&mut self) -> Result<Option<WasmPacket>, String> {
        if !self.probed {
            self.try_probe()?;
            if !self.probed {
                return Ok(None);
            }
        }

        match self.demux_nth_packet(self.packet_count)? {
            Some(packet) => {
                self.packet_count += 1;
                self.exhausted = false;
                Ok(Some(packet))
            }
            None => {
                self.exhausted = self.flushed;
                Ok(None)
            }
        }
    }

    /// Attempts to probe the container format from accumulated header
    /// bytes, building and probing a real per-format demuxer.
    ///
    /// # Errors
    ///
    /// Returns `Err` only for a genuine, unrecoverable parse failure (a
    /// real error class other than "ran out of buffered bytes") -- e.g. a
    /// corrupt header, or (MP4) a patent-encumbered codec. Returns `Ok(())`
    /// both when there is not yet enough data to know either way, and after
    /// a successful probe (check `self.probed` to distinguish).
    fn try_probe(&mut self) -> Result<(), String> {
        // Wait for enough bytes before even attempting -- with too little
        // data buffered we genuinely don't know anything yet, which is
        // different from "we tried and it's not a valid header."
        if self.buffer.len() < MIN_PROBE_BYTES {
            return Ok(());
        }

        let source = MemorySource::new(bytes::Bytes::copy_from_slice(&self.buffer));
        let mut demuxer = RealDemuxer::new(self.format, source);

        match poll_oxi(demuxer.probe()) {
            Ok(_) => {
                let streams: Vec<WasmStreamInfo> = convert_streams(demuxer.streams())
                    .into_iter()
                    .map(WasmStreamInfo::from)
                    .collect();
                if streams.is_empty() {
                    return Err(format!(
                        "WasmStreamingDemuxer: no decodable streams found in {:?}",
                        self.format
                    ));
                }
                self.streams = streams;
                self.probed = true;
                Ok(())
            }
            Err(e) if self.is_insufficient_data(&e) => Ok(()), // keep waiting for more chunks
            Err(e) => Err(format!(
                "WasmStreamingDemuxer: header probe for {:?} failed: {e}",
                self.format
            )),
        }
    }

    /// Rebuilds a demuxer from scratch over the full buffer, replays
    /// (discards) the first `n` real packets, and returns the `n`-th
    /// (0-indexed) one if it is available *and* trustworthy.
    ///
    /// See the module docs for why rebuild-and-replay is used, and for the
    /// peek-ahead guard against reporting a packet that might still be an
    /// artifact of the buffer ending mid-frame.
    fn demux_nth_packet(&mut self, n: u64) -> Result<Option<WasmPacket>, String> {
        let source = MemorySource::new(bytes::Bytes::copy_from_slice(&self.buffer));
        let mut demuxer = RealDemuxer::new(self.format, source);

        if let Err(e) = poll_oxi(demuxer.probe()) {
            return if self.is_insufficient_data(&e) {
                Ok(None)
            } else {
                Err(format!(
                    "WasmStreamingDemuxer: re-probe for {:?} failed: {e}",
                    self.format
                ))
            };
        }

        // Refresh cached stream info -- harmless if unchanged, more
        // accurate if additional buffered data let the real demuxer refine
        // duration/track fields.
        let streams: Vec<WasmStreamInfo> = convert_streams(demuxer.streams())
            .into_iter()
            .map(WasmStreamInfo::from)
            .collect();
        if !streams.is_empty() {
            self.streams = streams;
        }

        for _ in 0..n {
            if let Err(e) = poll_oxi(demuxer.read_packet()) {
                return if self.is_insufficient_data(&e) {
                    // The buffer no longer parses as far as packets we
                    // previously emitted -- should not normally happen
                    // since the buffer only grows, but there is nothing
                    // honest to do except wait for more data.
                    Ok(None)
                } else {
                    Err(format!(
                        "WasmStreamingDemuxer: replay failed for {:?}: {e}",
                        self.format
                    ))
                };
            }
        }

        let packet = match poll_oxi(demuxer.read_packet()) {
            Ok(packet) => packet,
            Err(e) if self.is_insufficient_data(&e) => return Ok(None),
            Err(e) => {
                return Err(format!(
                    "WasmStreamingDemuxer: read_packet failed for {:?}: {e}",
                    self.format
                ))
            }
        };

        if !self.flushed {
            // Confidence guard: only report this packet if real container
            // structure can also be observed *after* it. Several real
            // per-format demuxers (FLAC, WAV) silently return a short
            // packet rather than erroring when their source runs out
            // mid-frame -- indistinguishable, from the packet alone, from
            // a genuine short final frame. Once `flush()` has been called
            // there is no more data ever coming, so whatever came back for
            // packet `n` is genuinely final and this check is skipped.
            match poll_oxi(demuxer.read_packet()) {
                Ok(_) => {} // confirmed: real data follows: packet `n` is trustworthy
                Err(e) if self.is_insufficient_data(&e) => return Ok(None),
                Err(_) => {} // an unrelated later parse error doesn't invalidate packet `n`
            }
        }

        Ok(Some(WasmPacket::from(convert_packet(packet))))
    }

    /// Classifies an [`OxiError`] as "genuinely not enough data buffered
    /// yet" (as opposed to a real, unrecoverable parse failure) -- the
    /// answer depends on `self.flushed` (see below), which is why this is
    /// a method rather than a free function.
    ///
    /// `Eof` is always ambiguous-safe to treat as "nothing to report right
    /// now": pre-`flush()` it means "no more buffered packets *yet*",
    /// post-`flush()` it means "genuinely done" -- both resolve to
    /// `Ok(None)` at the call sites either way, just with different
    /// `self.exhausted` bookkeeping (set from `self.flushed` in
    /// `read_packet_inner`).
    ///
    /// `UnexpectedEof` (and `WavDemuxer`'s specific "Missing fmt/data
    /// chunk" `Parse` errors, which -- verified by reading
    /// `crates/oximedia-container/src/demux/wav/mod.rs::parse_header` --
    /// are produced by deliberately catching `UnexpectedEof` mid-chunk-scan
    /// and are therefore genuinely ambiguous between "truncated so far" and
    /// "structurally missing") are only treated as "not enough data yet"
    /// *before* `flush()`. After `flush()` there is no more data ever
    /// coming, so a real per-format demuxer that still can't complete its
    /// parse is reporting a genuine failure, not a transient one -- treating
    /// it as "keep waiting" forever past that point would silently mask a
    /// truncated/malformed stream as "just still streaming".
    fn is_insufficient_data(&self, err: &OxiError) -> bool {
        match err {
            OxiError::Eof => true,
            _ if self.flushed => false,
            OxiError::UnexpectedEof => true,
            OxiError::Parse { message, .. } => {
                message == "Missing fmt chunk" || message == "Missing data chunk"
            }
            _ => false,
        }
    }
}

/// Parse a user-supplied format hint string into a `ContainerFormat`.
///
/// Returns `Err(String)` (not `JsValue`) so it can be called from native tests.
fn parse_format_hint(hint: &str) -> Result<ContainerFormat, String> {
    match hint.to_lowercase().as_str() {
        "webm" | "matroska" | "mkv" => Ok(ContainerFormat::Matroska),
        "ogg" | "oga" | "ogv" => Ok(ContainerFormat::Ogg),
        "flac" => Ok(ContainerFormat::Flac),
        "wav" | "wave" => Ok(ContainerFormat::Wav),
        "mp4" | "mov" | "m4v" | "m4a" | "isobmff" => Ok(ContainerFormat::Mp4),
        other => Err(format!(
            "WasmStreamingDemuxer: unrecognised format hint '{other}'. \
             Supported values: webm, matroska, mkv, ogg, flac, wav, mp4, mov"
        )),
    }
}

// ---------------------------------------------------------------------------

impl WasmStreamingDemuxer {
    /// Construct a demuxer bypassing `JsValue` conversion — for native tests.
    #[cfg(test)]
    fn new_for_test(format_hint: &str) -> Self {
        let format = parse_format_hint(format_hint).expect("valid format hint in test");
        Self {
            buffer: Vec::new(),
            format,
            probed: false,
            streams: Vec::new(),
            packet_count: 0,
            flushed: false,
            exhausted: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oximedia_container::mux::{Muxer, MuxerConfig, WavMuxer};
    use oximedia_container::{CodecParams, Packet, PacketFlags, StreamInfo};
    use oximedia_core::{CodecId, Rational, Timestamp};

    #[test]
    fn test_parse_format_hint_webm() {
        assert!(matches!(
            parse_format_hint("webm").expect("webm should be valid"),
            ContainerFormat::Matroska
        ));
        assert!(matches!(
            parse_format_hint("WebM").expect("WebM should be valid"),
            ContainerFormat::Matroska
        ));
        assert!(matches!(
            parse_format_hint("MKV").expect("MKV should be valid"),
            ContainerFormat::Matroska
        ));
    }

    #[test]
    fn test_parse_format_hint_ogg() {
        assert!(matches!(
            parse_format_hint("ogg").expect("ogg should be valid"),
            ContainerFormat::Ogg
        ));
    }

    #[test]
    fn test_parse_format_hint_unknown_fails() {
        assert!(parse_format_hint("avi").is_err());
        assert!(parse_format_hint("").is_err());
    }

    /// Below the probe threshold there is genuinely not enough data yet --
    /// this must be `Ok(None)` ("keep streaming"), not an error and not a
    /// fabricated packet.
    #[test]
    fn test_read_packet_before_probe_threshold_returns_none() {
        let mut sd = WasmStreamingDemuxer::new_for_test("wav");
        sd.append_data_inner(&vec![0u8; 10])
            .expect("append should succeed");
        let packet = sd
            .read_packet_inner()
            .expect("insufficient data should be Ok(None), not an error");
        assert!(packet.is_none());
        assert!(!sd.probed);
    }

    /// Past the probe threshold but still not a real container: the real
    /// header parse genuinely fails (zeroed bytes are not valid EBML), so
    /// this must be an honest `Err` -- never a fabricated stream list. This
    /// is a regression test for the exact bug reported: the previous
    /// implementation always guessed a VP9 video + Opus audio pair for
    /// Matroska/WebM regardless of the actual track data, and wrapped raw
    /// byte windows in fake packets.
    #[test]
    fn test_read_packet_past_probe_threshold_on_garbage_is_honest_err() {
        let mut sd = WasmStreamingDemuxer::new_for_test("webm");
        sd.append_data_inner(&vec![0u8; 200])
            .expect("append should succeed");
        let result = sd.read_packet_inner();
        assert!(
            result.is_err(),
            "read_packet must not fabricate a packet from data that isn't real Matroska"
        );
        assert!(
            sd.streams().is_empty(),
            "no stream list should ever be fabricated"
        );
        assert!(
            !sd.probed,
            "probed must stay false -- no header was really parsed"
        );
        assert_eq!(sd.packets_emitted(), 0);
    }

    #[test]
    fn test_streams_never_fabricated_for_garbage_input() {
        for hint in ["webm", "ogg", "flac", "wav", "mp4"] {
            let mut sd = WasmStreamingDemuxer::new_for_test(hint);
            sd.append_data_inner(&vec![0u8; 64])
                .expect("append should succeed");
            let _ = sd.read_packet_inner();
            assert!(
                sd.streams().is_empty(),
                "{hint}: stream list must not be fabricated from garbage bytes"
            );
        }
    }

    /// This implementation deliberately never evicts bytes from the front
    /// of the buffer (see module docs): every read rebuilds demuxer state
    /// from the *entire* accumulated buffer, so the full history must stay
    /// available.
    #[test]
    fn test_buffer_is_never_evicted_by_design() {
        let mut sd = WasmStreamingDemuxer::new_for_test("ogg");
        sd.append_data_inner(&vec![0u8; 512])
            .expect("append should succeed");
        let _ = sd.read_packet_inner();
        assert_eq!(sd.bytes_consumed(), 0);
        assert_eq!(sd.buffer_len(), 512);

        sd.append_data_inner(&vec![0u8; 16])
            .expect("append should succeed");
        assert_eq!(sd.buffer_len(), 528);
    }

    // ------------------------------------------------------------------
    // Real streaming round-trip: a genuine WAV fixture fed in via
    // `append_data` in small pieces, proving packets are only emitted once
    // truly parseable and that the total byte count is preserved exactly.
    // ------------------------------------------------------------------

    fn make_wav_fixture(sample_rate: u32, channels: u8, num_frames: usize) -> Vec<u8> {
        let sink = MemorySource::new_writable(4096);
        let mut muxer = WavMuxer::new(sink, MuxerConfig::new().with_write_cues(false));

        let mut stream = StreamInfo::new(0, CodecId::Pcm, Rational::new(1, i64::from(sample_rate)));
        stream.codec_params = CodecParams::audio(sample_rate, channels);
        muxer.add_stream(stream).expect("add_stream");
        poll_oxi(muxer.write_header()).expect("write_header");

        let bytes_per_frame = usize::from(channels) * 2;
        let pcm = vec![0x22u8; num_frames * bytes_per_frame];
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
    fn streaming_wav_round_trip_is_real() {
        let data = make_wav_fixture(8000, 1, 1000);
        let mut sd = WasmStreamingDemuxer::new_for_test("wav");

        // Feed it in small pieces, draining packets between chunks -- the
        // point of the streaming API.
        let mut total_bytes = 0usize;
        let mut packet_count = 0u32;
        for chunk in data.chunks(37) {
            sd.append_data_inner(chunk).expect("append should succeed");
            loop {
                match sd.read_packet_inner().expect("must not error mid-stream") {
                    Some(packet) => {
                        total_bytes += packet.size();
                        packet_count += 1;
                        assert!(packet_count < 10_000, "runaway loop guard");
                    }
                    None => break,
                }
            }
        }
        sd.flush();
        loop {
            match sd.read_packet_inner().expect("must not error after flush") {
                Some(packet) => {
                    total_bytes += packet.size();
                    packet_count += 1;
                }
                None => break,
            }
        }

        assert!(sd.probed, "a real WAV header must have been parsed");
        assert_eq!(sd.streams().len(), 1);
        assert_eq!(sd.streams()[0].codec(), "Pcm");
        assert!(packet_count > 0, "must read at least one real packet");
        assert_eq!(
            total_bytes,
            1000 * 1 * 2,
            "all PCM bytes must be accounted for"
        );
        assert!(sd.is_done());
    }
}
