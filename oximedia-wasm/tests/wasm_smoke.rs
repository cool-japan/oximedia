//! WASM smoke tests for the demux/probe/hash boundary.
//!
//! These exercise the real, non-fabricated surface this crate promises to
//! JavaScript callers:
//!
//! - Real magic-byte format detection (`probe_format`).
//! - `WasmDemuxer`'s real demux path: `probe()`/`streams()`/`read_packet()`
//!   are backed by the actual `oximedia-container` per-format demuxers (see
//!   `src/demuxer.rs` module docs) -- `real_wav_round_trip_demuxes_actual_packets`
//!   below feeds it a genuine (if minimal) WAV file and checks the streams
//!   and packet bytes it returns are the real ones, not fabricated.
//! - `WasmDemuxer`'s honesty contract for inputs that are *not* a real,
//!   complete container: `probe()` must return an honest `Err` rather than
//!   fabricating a plausible stream/packet (see the deliberately truncated
//!   fixtures below).
//! - `WasmStreamingDemuxer`'s equivalent honesty contract for the
//!   chunk-oriented API (see `src/streaming_demuxer.rs` module docs).
//! - Real, deterministic content hashing (`probe_hash`) that reflects the
//!   actual input bytes rather than returning a constant.
//!
//! # Running
//!
//! This file is gated on `target_arch = "wasm32"` (see below), so it is a
//! no-op on the native host target -- plain `cargo test`/`cargo nextest`
//! neither compiles nor runs it. Native-target coverage for pure-logic
//! helpers lives in the `#[cfg(test)]` modules inside `src/*.rs` instead
//! (e.g. `src/demuxer.rs`, `src/streaming_demuxer.rs`, `src/probe.rs`).
//!
//! To actually compile and run *this* file, you need the
//! `wasm32-unknown-unknown` target and a JS runtime:
//!
//! ```bash
//! # Compile-only check (no JS runtime required):
//! cargo check -p oximedia-wasm --tests --target wasm32-unknown-unknown
//!
//! # Actually execute the tests, via Node.js (no browser needed):
//! wasm-pack test --node oximedia-wasm
//!
//! # ...or in a real browser engine:
//! wasm-pack test --headless --chrome oximedia-wasm
//! ```
//!
//! No `wasm_bindgen_test_configure!` call is made below, which keeps these
//! tests Node-compatible (calling `run_in_browser` would require `--chrome`
//! / `--firefox` / `--safari` instead of `--node`). None of the assertions
//! here need DOM/browser-only APIs.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;

/// Minimal-but-real Ogg page header (`OggS` capture pattern + version byte
/// + header-type byte + zeroed granule-position/serial/sequence/CRC
/// fields). Real magic bytes recognized by the byte-level sniff, not a
/// fabricated full container -- there is no valid Ogg packet payload
/// following it, which is precisely why the demuxer must honestly fail to
/// extract streams/packets from it rather than pretend to succeed.
fn tiny_ogg_bytes() -> Vec<u8> {
    let mut data = b"OggS".to_vec();
    data.push(0x00); // stream_structure_version
    data.push(0x02); // header_type_flag: beginning-of-stream
    data.extend_from_slice(&[0u8; 20]); // granule pos + serial + seq + CRC (zeroed)
    data
}

/// Minimal valid FLAC stream marker + `STREAMINFO` block size field.
fn tiny_flac_bytes() -> Vec<u8> {
    b"fLaC\x00\x00\x00\x22".to_vec()
}

#[wasm_bindgen_test]
fn probe_format_detects_real_magic_bytes() {
    let data = tiny_ogg_bytes();
    let result = oximedia_wasm::probe_format(&data).expect("Ogg magic bytes should be detected");
    assert_eq!(result.format(), "Ogg");
    assert!(result.confidence() > 0.9);
}

#[wasm_bindgen_test]
fn probe_format_rejects_garbage() {
    let data = [0xFFu8; 16];
    assert!(
        oximedia_wasm::probe_format(&data).is_err(),
        "unrecognized bytes must not be reported as a detected format"
    );
}

/// Builds a real, minimal WAV file (RIFF/WAVE header + `fmt ` chunk + `data`
/// chunk) directly, byte-for-byte -- mirrors the `make_sine_wav` pattern in
/// `oximedia-cli/src/decode_helper.rs`'s tests, simplified to constant PCM
/// since this test only checks byte counts/stream info, not audio content.
fn make_wav_bytes(sample_rate: u32, channels: u16, num_frames: u32) -> Vec<u8> {
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_size = num_frames * u32::from(channels) * u32::from(bits_per_sample / 8);
    let file_size = 36 + data_size;

    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_size.to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&channels.to_le_bytes());
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&block_align.to_le_bytes());
    buf.extend_from_slice(&bits_per_sample.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    buf.extend(std::iter::repeat_n(0u8, data_size as usize));
    buf
}

#[wasm_bindgen_test]
fn real_wav_round_trip_demuxes_actual_packets() {
    // Flipped from the old "asserts unavailability" shape: this is a real,
    // complete, decodable WAV file, and `WasmDemuxer` is now backed by the
    // real `oximedia-container` demuxers -- probe/streams/read_packet must
    // all succeed and return the genuine parsed data, not fabricate it.
    let data = make_wav_bytes(44100, 2, 512);
    let mut demuxer = oximedia_wasm::WasmDemuxer::new(&data);
    let probe = demuxer.probe().expect("a real, complete WAV must probe");
    assert_eq!(probe.format(), "Wav");

    let streams = demuxer.streams();
    assert_eq!(streams.len(), 1, "exactly one real PCM stream");
    assert_eq!(streams[0].codec(), "Pcm");
    assert_eq!(streams[0].codec_params().sample_rate(), Some(44100));
    assert_eq!(streams[0].codec_params().channels(), Some(2));

    let mut total_bytes = 0usize;
    loop {
        let packet = demuxer.read_packet().expect("read_packet must not error");
        let Some(packet) = packet else { break };
        total_bytes += packet.size();
    }
    assert_eq!(
        total_bytes,
        512 * 2 * 2,
        "every real PCM byte must be accounted for across packets"
    );
    assert!(demuxer.is_eof());
}

#[wasm_bindgen_test]
fn demuxer_probe_is_honest_not_fabricated() {
    // The demuxer must never invent a stream/packet it did not actually
    // parse. `tiny_flac_bytes()` is real FLAC magic followed by a
    // STREAMINFO block header declaring 34 bytes of data that are never
    // actually present -- the real FLAC demuxer (see `src/demuxer.rs`
    // module docs) genuinely cannot parse this, so `probe()` must fail for
    // that real reason, never by fabricating a plausible stream.
    let data = tiny_flac_bytes();
    let mut demuxer = oximedia_wasm::WasmDemuxer::new(&data);
    let result = demuxer.probe();
    assert!(
        result.is_err(),
        "demuxer.probe() fabricated success instead of honestly erroring"
    );
    assert!(
        demuxer.streams().is_empty(),
        "no stream list should ever be fabricated"
    );
}

#[wasm_bindgen_test]
fn demuxer_read_packet_before_probe_errors() {
    let data = tiny_flac_bytes();
    let mut demuxer = oximedia_wasm::WasmDemuxer::new(&data);
    assert!(demuxer.read_packet().is_err());
}

#[wasm_bindgen_test]
fn streaming_demuxer_never_fabricates_a_stream_or_packet() {
    let mut sd = oximedia_wasm::WasmStreamingDemuxer::new("webm")
        .expect("'webm' is a recognized format hint");

    // Below the internal probe threshold: genuinely "not enough data yet",
    // must be `null` (`Ok(None)`), not an error and not a fabricated
    // packet.
    sd.append_data(&[0u8; 8])
        .expect("append_data should accept a small chunk");
    let early = sd
        .read_packet()
        .expect("insufficient data should be Ok(None), not an error");
    assert!(early.is_none());

    // Past the probe threshold: must honestly error instead of inventing
    // a VP9+Opus stream pair the way the old implementation did.
    sd.append_data(&[0u8; 200])
        .expect("append_data should accept a larger chunk");
    let result = sd.read_packet();
    assert!(
        result.is_err(),
        "streaming demuxer fabricated a packet instead of honestly erroring"
    );
    assert!(
        sd.streams().is_empty(),
        "no stream list should ever be fabricated"
    );
}

#[wasm_bindgen_test]
fn probe_hash_is_stable_and_reflects_real_bytes() {
    let data = tiny_ogg_bytes();
    let h1 = oximedia_wasm::probe_hash(&data);
    let h2 = oximedia_wasm::probe_hash(&data);
    assert_eq!(h1.crc32(), h2.crc32(), "hash must be deterministic");
    assert_eq!(h1.fnv1a64(), h2.fnv1a64(), "hash must be deterministic");
    assert_eq!(h1.byte_length(), data.len());

    let other = oximedia_wasm::probe_hash(b"completely different payload");
    assert_ne!(
        h1.crc32(),
        other.crc32(),
        "hash must reflect actual input bytes, not a constant"
    );
}

#[wasm_bindgen_test]
fn probe_hash_matches_known_crc32_check_value() {
    // Standard CRC-32/ISO-HDLC check value for the ASCII string
    // "123456789" -- a real, externally-verifiable reference value, not
    // just a self-consistency check.
    let h = oximedia_wasm::probe_hash(b"123456789");
    assert_eq!(h.crc32(), "cbf43926");
}
