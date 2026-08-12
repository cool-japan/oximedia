//! VP8 decoder fuzzer.
//!
//! This fuzzer tests the VP8 decoder for:
//! - Frame header parsing
//! - Partition parsing
//! - Boolean decoder
//! - Coefficient decoding
//! - DCT transforms
//! - Loop filter
//! - Golden/altref frame management
//! - Inter-frame decode: motion-vector entropy decode, sub-pixel motion
//!   compensation, and last/golden/altref/hidden-frame reference management
//!   (`vp8/dec/inter.rs`'s `Vp8SequenceDecoder`)
//!
//! # Two fuzzing modes, selected by the first input byte
//!
//! A purely random byte stream can reach VP8's key-frame path (any bytes at
//! all get parsed as *a* frame tag), but essentially never reaches the
//! inter-frame path: [`Vp8Decoder`] rejects an inter frame honestly with
//! [`CodecError::InvalidBitstream`] unless a key frame already decoded and
//! published reference buffers first (see `vp8/decoder.rs`'s
//! `test_inter_frame_without_keyframe`), and a fuzzer starting from nothing
//! essentially never stumbles onto a valid key frame by chance. So:
//!
//! - **Mode 0 (`data[0] & 1 == 0`): raw single-input path.** The *whole*
//!   input, selector byte included, is sent to a fresh decoder exactly as
//!   the original harness did (RFC 6386's frame-tag byte has frame_type in
//!   bit 0, the same bit this mode switches on, so an even first byte
//!   reads as both "mode 0" and "this could be a key-frame tag" -- the
//!   selector must not consume it, or every key-frame byte pattern the
//!   dictionary or corpus provides would land shifted by one and never
//!   parse). Key-frame header/partition/boolean-decoder parsing stays
//!   fuzzed on its own, unprimed.
//! - **Mode 1 (`data[0] & 1 == 1`): inter-frame path.** A fresh decoder
//!   first decodes [`KEYFRAME`] -- a real, bit-exact-verified key frame
//!   (`vp8/dec/testdata/p5basic.frame0.bin`, frame 0 of one of the
//!   `inter_fixture_tests.rs` conformance streams) -- to establish real
//!   reference (last/golden/altref) buffers and entropy/segmentation/
//!   loop-filter state. The rest of the fuzz input is then split into a
//!   bounded sequence of length-prefixed chunks, each sent as a subsequent
//!   frame, so the fuzzer can mutate its way through inter-frame macroblock
//!   modes, motion vectors, sub-pixel MC and reference-frame management
//!   instead of being rejected before any of that code runs.
//!
//! The fuzzer should never panic, enter infinite loops, or cause memory
//! safety issues, in either mode.

#![no_main]

use libfuzzer_sys::fuzz_target;
use oximedia_codec::{DecoderConfig, VideoDecoder, Vp8Decoder};
use oximedia_core::CodecId;

/// A real, bit-exact-verified VP8 key frame: `p5basic.frame0.bin`, frame 0
/// of the `p5basic` stream in `vp8/dec/inter_fixture_tests.rs`'s multi-frame
/// conformance suite (96x64, LAST-only P frames follow it there). Embedded
/// so mode 1 below always starts from real decoder state rather than from
/// nothing.
const KEYFRAME: &[u8] =
    include_bytes!("../../crates/oximedia-codec/src/vp8/dec/testdata/p5basic.frame0.bin");

/// Maximum number of chunked `send_packet` calls in mode 1, bounding total
/// work per input the same way the pre-existing `receive_frame` loops below
/// bound theirs.
const MAX_CHUNKED_FRAMES: usize = 32;

/// Maximum `receive_frame` pulls after any single `send_packet`/`flush`.
const MAX_FRAMES_DRAINED: usize = 100;

fn new_decoder() -> Option<Vp8Decoder> {
    let config = DecoderConfig {
        codec: CodecId::Vp8,
        extradata: None,
        threads: 1,
        low_latency: false,
    };
    Vp8Decoder::new(config).ok()
}

/// Pulls queued frames until the decoder reports none left (or errors),
/// bounded so a decoder bug that never empties its queue can't hang the
/// fuzzer.
fn drain(decoder: &mut Vp8Decoder) {
    for _ in 0..MAX_FRAMES_DRAINED {
        match decoder.receive_frame() {
            Ok(Some(_frame)) => {}
            Ok(None) | Err(_) => break,
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };

    let Some(mut decoder) = new_decoder() else {
        return;
    };

    if mode & 1 == 0 {
        // Mode 0: raw single-input path -- the *whole* original `data`
        // (selector byte included) goes straight to a fresh decoder with no
        // priming, exactly like the pre-existing harness. The selector byte
        // does double duty as the first payload byte: RFC 6386's frame tag
        // has frame_type in bit 0 (0 = key frame), which is also this
        // branch's own selector bit, so an *even* first byte is both "take
        // mode 0" and "this would parse as a key-frame tag" -- passing
        // `rest` instead would silently shift every key-frame byte pattern
        // (including every token in the dictionary) off by one and defeat
        // them. Feeding `data` keeps key-frame parsing fuzzed byte-for-byte
        // as before, with no offset.
        let _ = decoder.send_packet(data, 0);
        drain(&mut decoder);
    } else {
        // Mode 1: prime with a real key frame first.
        if decoder.send_packet(KEYFRAME, 0).is_err() {
            // KEYFRAME is a checked-in conformance fixture and must always
            // decode; if it doesn't, that is a real regression for
            // `inter_fixture_tests.rs` to catch, not something to panic
            // over here (a fuzzer must never panic on its own account).
            return;
        }
        drain(&mut decoder);

        // Split the remaining bytes into a bounded sequence of
        // length-prefixed "next frame" chunks: one byte gives the chunk
        // length (clamped to what's left), the following `len` bytes are
        // sent as that frame's payload. This lets the fuzzer explore
        // multi-frame sequences -- golden/altref accumulation, hidden
        // (`show_frame == 0`) frames, entropy snapshot/restore -- instead
        // of only ever reaching a single post-keyframe packet.
        let mut remaining = rest;
        let mut pts: i64 = 1;
        for _ in 0..MAX_CHUNKED_FRAMES {
            let Some((&len, tail)) = remaining.split_first() else {
                break;
            };
            let len = usize::from(len).min(tail.len());
            let (frame, next) = tail.split_at(len);

            let _ = decoder.send_packet(frame, pts);
            drain(&mut decoder);

            pts += 1;
            remaining = next;
            if remaining.is_empty() {
                break;
            }
        }
    }

    let _ = decoder.flush();
    drain(&mut decoder);
});
