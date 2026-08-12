// Copyright 2025 OxiMedia Contributors
// Licensed under the Apache License, Version 2.0

//! Regression tests: malformed / truncated container input must return
//! `Err`, never panic — SLICE 3J (no-unwrap hardening for
//! `oximedia-container`, an untrusted-input demuxer crate).
//!
//! These synthesize minimal MP4 (ISOBMFF "box") and Matroska/WebM (EBML
//! "element") byte streams whose declared sizes lie about how much data
//! actually follows — truncated boxes and bad element sizes — and drive
//! them through the public [`Demuxer`] API only. A panic anywhere in this
//! file is a real bug: parsing attacker-controlled media must never crash
//! the process.

use bytes::Bytes;
use oximedia_container::demux::{MatroskaDemuxer, Mp4Demuxer, MpegTsDemuxer};
use oximedia_container::Demuxer;
use oximedia_io::MemorySource;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Runs `fut`, failing the test (rather than hanging the suite) if it
/// doesn't resolve within [`TIMEOUT`]. A regression that turns a bounds
/// check back into an unbounded loop/allocation should show up as a fast,
/// legible test failure — not a stuck CI job.
async fn bounded<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(TIMEOUT, fut)
        .await
        .expect("must not hang on malformed input")
}

// ============================================================================
// MP4 / ISOBMFF helpers
// ============================================================================

/// Builds a standard (32-bit size) ISOBMFF box: 4-byte big-endian size +
/// 4-byte type + body.
fn mp4_box(box_type: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let size = u32::try_from(8 + body.len()).expect("test box body fits in u32");
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(body);
    out
}

/// Builds an ISOBMFF box using the 64-bit extended-size form (`size32 == 1`
/// followed by an 8-byte real size), with a **declared** size that need not
/// match `body.len()` — this is how a truncated/lying box is expressed.
fn mp4_box_ext64_lying(box_type: &[u8; 4], declared_size: u64, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + body.len());
    out.extend_from_slice(&1u32.to_be_bytes()); // size32 == 1 sentinel
    out.extend_from_slice(box_type);
    out.extend_from_slice(&declared_size.to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn valid_ftyp() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(b"isom"); // major_brand
    body.extend_from_slice(&0u32.to_be_bytes()); // minor_version
    mp4_box(b"ftyp", &body)
}

// ============================================================================
// MP4: truncated / lying box-size regressions
// ============================================================================

/// An empty source must not panic; `probe()` should terminate (as `Ok` with
/// no streams, or `Err` — either is acceptable, a panic is not).
#[tokio::test]
async fn test_mp4_empty_buffer_no_panic() {
    let source = MemorySource::new(Bytes::new());
    let mut demuxer = Mp4Demuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    if result.is_ok() {
        assert!(demuxer.streams().is_empty());
    }
}

/// Fewer than 8 bytes total — not enough for even one box header.
#[tokio::test]
async fn test_mp4_truncated_box_header_no_panic() {
    let source = MemorySource::new(Bytes::from_static(&[0x00, 0x00, 0x00]));
    let mut demuxer = Mp4Demuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    if result.is_ok() {
        assert!(demuxer.streams().is_empty());
    }
}

/// A well-formed, complete `ftyp` box whose major/compatible brands are all
/// unrecognized must be a clean, deterministic `Err` (documented behavior
/// at `Mp4Demuxer::parse_headers`).
#[tokio::test]
async fn test_mp4_ftyp_invalid_brand_returns_err() {
    let mut body = Vec::new();
    body.extend_from_slice(b"bad!"); // major_brand: not a recognized MP4 brand
    body.extend_from_slice(&0u32.to_be_bytes()); // minor_version
    let data = mp4_box(b"ftyp", &body);

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = Mp4Demuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    assert!(
        result.is_err(),
        "an ftyp with no recognized brand must be rejected"
    );
}

/// A `moov` box declares a content size far larger than the bytes actually
/// present in the source. `read_n` must surface `UnexpectedEof`, not
/// index/slice past the end of what was read.
#[tokio::test]
async fn test_mp4_moov_declared_size_exceeds_data_returns_err() {
    let mut data = valid_ftyp();
    // Declare a 5000-byte moov but supply only 10 bytes of body.
    let mut header = Vec::new();
    header.extend_from_slice(&5000u32.to_be_bytes());
    header.extend_from_slice(b"moov");
    data.extend_from_slice(&header);
    data.extend_from_slice(&[0u8; 10]);

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = Mp4Demuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    assert!(
        result.is_err(),
        "a moov box whose declared size exceeds available data must error, not panic"
    );
}

/// A top-level box declares (via the 64-bit extended-size form) a content
/// size of `u64::MAX`, with almost no data following. This must resolve
/// quickly to `Err` — not attempt a multi-exabyte allocation and not hang.
/// `moov` is the specific target because, per the incremental `read_n`
/// hardening comment in `demux/mp4/mod.rs`, it is the one box type whose
/// size is not otherwise capped before the read.
#[tokio::test]
async fn test_mp4_moov_extended_size_near_u64_max_returns_err_not_hang() {
    let mut data = valid_ftyp();
    data.extend_from_slice(&mp4_box_ext64_lying(b"moov", u64::MAX, &[0u8; 4]));

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = Mp4Demuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    assert!(
        result.is_err(),
        "a moov declaring a ~u64::MAX size must fail cleanly, not hang or abort on OOM"
    );
}

/// A `moov` box using `size32 == 0` ("box extends to end of file") is an
/// edge case the code treats as an empty/unknown-length box. Must not
/// panic regardless of how it resolves.
#[tokio::test]
async fn test_mp4_moov_zero_size_extends_to_eof_no_panic() {
    let mut data = valid_ftyp();
    let mut header = Vec::new();
    header.extend_from_slice(&0u32.to_be_bytes()); // size32 == 0: "extends to EOF"
    header.extend_from_slice(b"moov");
    data.extend_from_slice(&header);

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = Mp4Demuxer::new(source);
    let _ = bounded(demuxer.probe()).await;
}

// ============================================================================
// EBML / Matroska helpers (deliberately duplicated locally, matching the
// existing convention in `tests/it_mkv_conformance.rs`, so each integration
// test binary is self-contained)
// ============================================================================

fn ebml_vint_size(mut n: u64) -> Vec<u8> {
    let width = if n < 0x7F {
        1usize
    } else if n < 0x3FFF {
        2
    } else if n < 0x1F_FFFF {
        3
    } else if n < 0x0FFF_FFFF {
        4
    } else {
        8
    };
    let marker = 1u64 << (7 * width);
    n |= marker;
    let bytes = n.to_be_bytes();
    bytes[8 - width..].to_vec()
}

fn uint_bytes(v: u64) -> Vec<u8> {
    if v == 0 {
        return vec![0];
    }
    let bytes = v.to_be_bytes();
    let leading = bytes.iter().take_while(|&&b| b == 0).count();
    bytes[leading..].to_vec()
}

/// Builds a well-formed EBML element: `id` bytes + correct VINT size + `data`.
fn ebml_elem(id: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(id);
    out.extend_from_slice(&ebml_vint_size(data.len() as u64));
    out.extend_from_slice(data);
    out
}

/// Builds an EBML element whose declared size does **not** match
/// `actual_data.len()` — the "bad element size" primitive every test below
/// is built from. `declared_size` is encoded as an 8-byte (max-width) VINT
/// so arbitrarily large lies (short of the reserved all-ones "unknown
/// size") can be expressed.
fn ebml_elem_lying_size(id: &[u8], declared_size: u64, actual_data: &[u8]) -> Vec<u8> {
    let marker = 1u64 << (7 * 8);
    let coded = declared_size | marker;
    let mut out = Vec::new();
    out.extend_from_slice(id);
    out.extend_from_slice(&coded.to_be_bytes());
    out.extend_from_slice(actual_data);
    out
}

fn ebml_uint(id: &[u8], value: u64) -> Vec<u8> {
    ebml_elem(id, &uint_bytes(value))
}

fn ebml_string(id: &[u8], value: &str) -> Vec<u8> {
    ebml_elem(id, value.as_bytes())
}

fn build_ebml_header() -> Vec<u8> {
    let body = [
        ebml_uint(&[0x42, 0x86], 1),        // EBMLVersion
        ebml_uint(&[0x42, 0xF7], 1),        // EBMLReadVersion
        ebml_uint(&[0x42, 0xF2], 4),        // EBMLMaxIDLength
        ebml_uint(&[0x42, 0xF3], 8),        // EBMLMaxSizeLength
        ebml_string(&[0x42, 0x82], "webm"), // DocType
        ebml_uint(&[0x42, 0x87], 4),        // DocTypeVersion
        ebml_uint(&[0x42, 0x85], 2),        // DocTypeReadVersion
    ]
    .concat();
    ebml_elem(&[0x1A, 0x45, 0xDF, 0xA3], &body)
}

/// Wraps `segment_children` in an unknown-size (streaming-style) Segment —
/// the standard convention used throughout `it_mkv_conformance.rs`.
fn build_segment(segment_children: &[u8]) -> Vec<u8> {
    let mut seg = Vec::new();
    seg.extend_from_slice(&[0x18, 0x53, 0x80, 0x67]); // Segment ID
    seg.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // unknown size
    seg.extend_from_slice(segment_children);
    seg
}

fn build_info(dur: f64) -> Vec<u8> {
    let body = [
        ebml_uint(&[0x2A, 0xD7, 0xB1], 1_000_000), // TimecodeScale
        {
            let mut f = vec![0x44, 0x89, 0x88]; // Duration ID + 8-byte-float size
            f.extend_from_slice(&dur.to_be_bytes());
            f
        },
    ]
    .concat();
    ebml_elem(&[0x15, 0x49, 0xA9, 0x66], &body)
}

/// Builds a single minimal video TrackEntry.
fn build_track_entry(number: u64, uid: u64) -> Vec<u8> {
    let body = [
        ebml_uint(&[0xD7], number),    // TrackNumber
        ebml_uint(&[0x73, 0xC5], uid), // TrackUID
        ebml_uint(&[0x83], 1),         // TrackType = video
        ebml_string(&[0x86], "V_VP9"), // CodecID
    ]
    .concat();
    ebml_elem(&[0xAE], &body)
}

// ============================================================================
// EBML / Matroska: bad element-size regressions
// ============================================================================

/// A top-level Segment child (`Info`) declares a size far larger than the
/// bytes actually present. `parse_segment_children` guards this with an
/// explicit `ensure_buffer` + length check before slicing — confirm the
/// guard holds: no panic, and `probe()` resolves (as `Ok` with whatever
/// was salvaged, or `Err`).
#[tokio::test]
async fn test_mkv_top_level_element_size_exceeds_available_data_no_panic() {
    let mut seg_body = build_ebml_header();
    seg_body.extend_from_slice(&build_segment(&ebml_elem_lying_size(
        &[0x15, 0x49, 0xA9, 0x66], // Info
        5000,
        &[0u8; 10],
    )));

    let source = MemorySource::new(Bytes::from(seg_body));
    let mut demuxer = MatroskaDemuxer::new(source);
    let _ = bounded(demuxer.probe()).await;
}

/// A `TrackEntry` inside `Tracks` declares an inner size larger than the
/// bytes remaining in the (correctly-sized) `Tracks` element. This must hit
/// `MatroskaParser::read_data`'s bounds check and surface `Err` — the
/// outer `Tracks` size is truthful, so this is not caught by the top-level
/// `ensure_buffer` gate; only the nested bounds check protects it.
#[tokio::test]
async fn test_mkv_track_entry_size_exceeds_tracks_element_returns_err() {
    let lying_track_entry = ebml_elem_lying_size(&[0xAE], 9000, &[0u8; 6]);
    let tracks = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &lying_track_entry);

    let mut data = build_ebml_header();
    let mut seg_body = build_info(0.0);
    seg_body.extend_from_slice(&tracks);
    data.extend_from_slice(&build_segment(&seg_body));

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = MatroskaDemuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    assert!(
        result.is_err(),
        "a TrackEntry whose declared size overruns its parent Tracks element must error"
    );
}

/// `CodecID` (a string field) inside an otherwise well-formed `TrackEntry`
/// declares a size larger than the bytes remaining in that `TrackEntry`.
#[tokio::test]
async fn test_mkv_codec_id_size_exceeds_track_entry_returns_err() {
    let mut entry_body = Vec::new();
    entry_body.extend_from_slice(&ebml_uint(&[0xD7], 1)); // TrackNumber
    entry_body.extend_from_slice(&ebml_uint(&[0x73, 0xC5], 1)); // TrackUID
    entry_body.extend_from_slice(&ebml_uint(&[0x83], 1)); // TrackType = video
    entry_body.extend_from_slice(&ebml_elem_lying_size(&[0x86], 500, b"V_VP9")); // CodecID, lying

    let track_entry = ebml_elem(&[0xAE], &entry_body);
    let tracks = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &track_entry);

    let mut data = build_ebml_header();
    let mut seg_body = build_info(0.0);
    seg_body.extend_from_slice(&tracks);
    data.extend_from_slice(&build_segment(&seg_body));

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = MatroskaDemuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    assert!(
        result.is_err(),
        "a CodecID string whose declared size overruns its TrackEntry must error"
    );
}

/// `Tracks` itself declares a huge (8-byte-VINT-encoded) concrete size —
/// not the reserved all-ones "unknown size" sentinel, just an ordinary lie
/// — with almost nothing behind it. Must resolve to `Err` quickly, not
/// hang scanning for data that will never arrive.
#[tokio::test]
async fn test_mkv_huge_concrete_element_size_not_hang() {
    let tracks = ebml_elem_lying_size(&[0x16, 0x54, 0xAE, 0x6B], 0x00FF_FFFF_FFFF_FFFE, &[0u8; 4]);

    let mut data = build_ebml_header();
    data.extend_from_slice(&build_segment(&tracks));

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = MatroskaDemuxer::new(source);
    let _ = bounded(demuxer.probe()).await;
}

/// A `Cluster` contains a `SimpleBlock` whose declared element size is
/// larger than the bytes actually available in the source. `read_next_block`
/// reads the block via `self.buffer[header_size..total_size]` after calling
/// `ensure_buffer(total_size)`; if the source can't supply `total_size`
/// bytes this must surface as `Err`, not an out-of-bounds slice panic.
#[tokio::test]
async fn test_mkv_simpleblock_size_exceeds_available_data_returns_err() {
    let tracks = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &build_track_entry(1, 1));

    let mut seg_body = build_info(1.0);
    seg_body.extend_from_slice(&tracks);

    // Cluster: valid Timecode, followed by a SimpleBlock that claims far
    // more bytes than actually follow it (the source ends 4 bytes later).
    let cluster_ts = ebml_uint(&[0xE7], 0);
    let track_vint = ebml_vint_size(1);
    let mut lying_block_payload = Vec::new();
    lying_block_payload.extend_from_slice(&track_vint); // track number
    lying_block_payload.extend_from_slice(&0i16.to_be_bytes()); // timecode
    lying_block_payload.push(0x80); // flags: keyframe, no lacing
    let lying_simple_block = ebml_elem_lying_size(&[0xA3], 5000, &lying_block_payload);

    let mut cluster_body = cluster_ts;
    cluster_body.extend_from_slice(&lying_simple_block);
    let mut cluster = Vec::new();
    cluster.extend_from_slice(&[0x1F, 0x43, 0xB6, 0x75]); // Cluster ID
    cluster.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // unknown size
    cluster.extend_from_slice(&cluster_body);

    seg_body.extend_from_slice(&cluster);

    let mut data = build_ebml_header();
    data.extend_from_slice(&build_segment(&seg_body));

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = MatroskaDemuxer::new(source);
    bounded(demuxer.probe())
        .await
        .expect("probe stops at the first Cluster and must not itself fail here");

    let result = bounded(demuxer.read_packet()).await;
    assert!(
        result.is_err(),
        "a SimpleBlock whose declared size overruns the available source data \
         must make read_packet() return Err, not panic with an out-of-bounds slice"
    );
}

// ============================================================================
// MPEG-TS: truncated-stream regressions (bonus coverage — this crate's
// third major "untrusted-input demuxer" format alongside MP4 and Matroska)
// ============================================================================

/// Fewer than 188 bytes total (one TS packet) must not panic; the demuxer's
/// own `read_ts_packet` already treats a short read as EOF.
#[tokio::test]
async fn test_mpegts_truncated_packet_no_panic() {
    let data = vec![0x47u8; 50]; // sync bytes only, far short of one packet
    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = MpegTsDemuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    assert!(
        result.is_err(),
        "a stream shorter than one TS packet must not probe successfully"
    );
}

/// A handful of complete, correctly-synced TS packets that never carry a
/// PAT must fail cleanly once the source is exhausted, not hang scanning.
#[tokio::test]
async fn test_mpegts_no_pat_returns_err_not_hang() {
    const TS_PACKET_SIZE: usize = 188;
    let mut data = Vec::new();
    for _ in 0..5 {
        let mut pkt = vec![0xFFu8; TS_PACKET_SIZE];
        pkt[0] = 0x47; // sync byte
        pkt[1] = 0x00; // PUSI=0, PID hi = 0
        pkt[2] = 0x20; // PID lo = 0x0020 (not PAT's 0x0000, not a PMT)
        pkt[3] = 0x10; // AFC=01 payload only
        data.extend_from_slice(&pkt);
    }

    let source = MemorySource::new(Bytes::from(data));
    let mut demuxer = MpegTsDemuxer::new(source);
    let result = bounded(demuxer.probe()).await;
    assert!(
        result.is_err(),
        "a TS stream with no PAT must fail probing, not hang"
    );
}
