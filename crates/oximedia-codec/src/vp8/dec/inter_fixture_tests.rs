//! Multi-frame VP8 conformance suite: bit-exact decode of five
//! libvpx-encoded streams against libvpx's own reconstruction.
//!
//! # What these fixtures are
//!
//! Five IVF streams produced by `ffmpeg 7.1.1` + `libvpx 1.15.2`, split into
//! their raw per-frame VP8 payloads (`<name>.frameN.bin`, no container
//! framing), each with the reference reconstruction of every *shown* frame
//! (`<name>.ref.yuv`, planar YUV 4:2:0) decoded by **libvpx itself**
//! (`ffmpeg -c:v libvpx -i ... -fps_mode passthrough -f rawvideo`, with the
//! decoder identity verified from ffmpeg's stream-mapping line rather than
//! assumed). Full provenance — exact encode/decode commands, the static
//! header parse of every frame, and the determinism checks — is in
//! `testdata/README.md`.
//!
//! What each stream is *for*:
//!
//! | stream | frames (shown) | exercises |
//! |---|---|---|
//! | `p5basic` | 5 (5) | LAST-only P frames; NEAREST/NEAR/ZERO/NEWMV; sub-pixel MC; inherited loop-filter deltas (`delta_update == 0`) |
//! | `refswap` | 19 (18) | a genuine **hidden** altref frame (`show_frame == 0`), `sign_bias_alternate`, and `refresh_golden` + `copy_buffer_to_alternate == 2` in one frame |
//! | `refswap_er` | 5 (5) | `error_resilient`: `refresh_entropy_probs == 0` on **every** frame (the snapshot/restore path), plus bitstream segmentation |
//! | `splitmv` | 5 (5) | strong diagonal pan at a non-macroblock-aligned width (100x64), for SPLITMV partitions and edge-referencing vectors |
//! | `segdelta` | 5 (5) | segmentation with per-segment quantiser deltas, `refresh_entropy_probs == 0`, and `mb_lf_adjustments` deltas |
//!
//! # The two assertions, and why the second one is a diagnostic
//!
//! Every shown frame is checked (1) **byte-equal** to libvpx's
//! reconstruction on all three planes, and (2) at >= 30 dB PSNR against the
//! same reference. VP8 is exactly specified (RFC 6386), so (1) is the real
//! gate and (2) is subsumed by it — a bit-exact frame has infinite PSNR.
//! (2) is kept because it turns a regression's failure message into a
//! magnitude ("2.1 dB" = broken prediction, "48 dB" = one rounding
//! difference) instead of just an index. It is deliberately **not** a
//! source-fidelity measurement: unlike the key-frame fixtures in
//! `tests/vp8_real_bitstream.rs`, these streams have no pre-encode source
//! YUV checked in (they are `lavfi`-generated, and regenerating one needs an
//! ffmpeg this test suite cannot assume). To keep the pair from passing
//! vacuously on degenerate data, [`assert_reference_has_variation`] proves
//! the reference frames themselves are not a constant fill.
//!
//! # Hidden frames make index equality wrong
//!
//! `refswap` codes 19 frames but shows 18: packet 1 is an invisible altref.
//! Its reference YUV therefore holds 18 frames, and
//! `reference[i]` corresponds to *coded* frame `i` only up to the hidden
//! one. These tests walk the `show_frame` bit the decoder reports instead of
//! assuming any mapping (`testdata/README.md` tabulates the resulting
//! packet -> reference index mapping for `refswap`).

use super::*;

/// The 5 coded frames of the `p5basic` stream, in bitstream order.
const P5BASIC_FRAMES: [&[u8]; 5] = [
    include_bytes!("testdata/p5basic.frame0.bin"),
    include_bytes!("testdata/p5basic.frame1.bin"),
    include_bytes!("testdata/p5basic.frame2.bin"),
    include_bytes!("testdata/p5basic.frame3.bin"),
    include_bytes!("testdata/p5basic.frame4.bin"),
];

/// libvpx's reference reconstruction of every *shown* `p5basic` frame.
const P5BASIC_REF: &[u8] = include_bytes!("testdata/p5basic.ref.yuv");

/// The 19 coded frames of the `refswap` stream, in bitstream order.
const REFSWAP_FRAMES: [&[u8]; 19] = [
    include_bytes!("testdata/refswap.frame0.bin"),
    include_bytes!("testdata/refswap.frame1.bin"),
    include_bytes!("testdata/refswap.frame2.bin"),
    include_bytes!("testdata/refswap.frame3.bin"),
    include_bytes!("testdata/refswap.frame4.bin"),
    include_bytes!("testdata/refswap.frame5.bin"),
    include_bytes!("testdata/refswap.frame6.bin"),
    include_bytes!("testdata/refswap.frame7.bin"),
    include_bytes!("testdata/refswap.frame8.bin"),
    include_bytes!("testdata/refswap.frame9.bin"),
    include_bytes!("testdata/refswap.frame10.bin"),
    include_bytes!("testdata/refswap.frame11.bin"),
    include_bytes!("testdata/refswap.frame12.bin"),
    include_bytes!("testdata/refswap.frame13.bin"),
    include_bytes!("testdata/refswap.frame14.bin"),
    include_bytes!("testdata/refswap.frame15.bin"),
    include_bytes!("testdata/refswap.frame16.bin"),
    include_bytes!("testdata/refswap.frame17.bin"),
    include_bytes!("testdata/refswap.frame18.bin"),
];

/// libvpx's reference reconstruction of every *shown* `refswap` frame.
const REFSWAP_REF: &[u8] = include_bytes!("testdata/refswap.ref.yuv");

/// The 5 coded frames of the `refswap_er` stream, in bitstream order.
const REFSWAP_ER_FRAMES: [&[u8]; 5] = [
    include_bytes!("testdata/refswap_er.frame0.bin"),
    include_bytes!("testdata/refswap_er.frame1.bin"),
    include_bytes!("testdata/refswap_er.frame2.bin"),
    include_bytes!("testdata/refswap_er.frame3.bin"),
    include_bytes!("testdata/refswap_er.frame4.bin"),
];

/// libvpx's reference reconstruction of every *shown* `refswap_er` frame.
const REFSWAP_ER_REF: &[u8] = include_bytes!("testdata/refswap_er.ref.yuv");

/// The 5 coded frames of the `splitmv` stream, in bitstream order.
const SPLITMV_FRAMES: [&[u8]; 5] = [
    include_bytes!("testdata/splitmv.frame0.bin"),
    include_bytes!("testdata/splitmv.frame1.bin"),
    include_bytes!("testdata/splitmv.frame2.bin"),
    include_bytes!("testdata/splitmv.frame3.bin"),
    include_bytes!("testdata/splitmv.frame4.bin"),
];

/// libvpx's reference reconstruction of every *shown* `splitmv` frame.
const SPLITMV_REF: &[u8] = include_bytes!("testdata/splitmv.ref.yuv");

/// The 5 coded frames of the `segdelta` stream, in bitstream order.
const SEGDELTA_FRAMES: [&[u8]; 5] = [
    include_bytes!("testdata/segdelta.frame0.bin"),
    include_bytes!("testdata/segdelta.frame1.bin"),
    include_bytes!("testdata/segdelta.frame2.bin"),
    include_bytes!("testdata/segdelta.frame3.bin"),
    include_bytes!("testdata/segdelta.frame4.bin"),
];

/// libvpx's reference reconstruction of every *shown* `segdelta` frame.
const SEGDELTA_REF: &[u8] = include_bytes!("testdata/segdelta.ref.yuv");
/// Peak signal-to-noise ratio between two equally-sized 8-bit planes.
fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "psnr operands must match in size");
    let mse: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(&x, &y)| {
            let d = f64::from(i32::from(x) - i32::from(y));
            d * d
        })
        .sum::<f64>()
        / a.len() as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0 * 255.0 / mse).log10()
}

/// Asserts a decoded plane equals libvpx's reconstruction bit-exactly,
/// reporting the first differing pixel's coordinates and the whole plane's
/// PSNR (the magnitude of the damage) when it does not.
fn assert_plane_bit_exact(what: &str, got: &[u8], want: &[u8], width: usize) {
    assert_eq!(
        got.len(),
        want.len(),
        "{what}: plane size mismatch (got {}, want {})",
        got.len(),
        want.len()
    );
    if let Some(i) = (0..got.len()).find(|&i| got[i] != want[i]) {
        let differing = (0..got.len()).filter(|&i| got[i] != want[i]).count();
        panic!(
            "{what}: first mismatch at index {i} (x={}, y={}): got {}, want {} \
             [{differing} of {} samples differ, plane PSNR {:.2} dB]",
            i % width,
            i / width,
            got[i],
            want[i],
            got.len(),
            psnr(got, want),
        );
    }
}

/// Proves the reference frames carry real picture content, so that the
/// bit-exact + PSNR pair cannot pass vacuously on a degenerate (constant)
/// reference.
fn assert_reference_has_variation(name: &str, luma: &[u8]) {
    let min = luma.iter().copied().min().unwrap_or(0);
    let max = luma.iter().copied().max().unwrap_or(0);
    assert!(
        max - min > 8,
        "{name}: reference luma looks like a constant fill (min {min}, max {max})"
    );
}

/// Decodes a whole stream and checks every shown frame against libvpx.
///
/// Returns `(coded_frames, shown_frames, worst_psnr_db)` so each test can
/// state what it actually proved.
fn verify_stream(
    name: &str,
    width: u32,
    height: u32,
    frames: &[&[u8]],
    reference: &[u8],
) -> (usize, usize, f64) {
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let luma_len = w * h;
    let chroma_len = cw * ch;
    let frame_len = luma_len + 2 * chroma_len;
    assert_eq!(
        reference.len() % frame_len,
        0,
        "{name}: reference is not a whole number of {width}x{height} frames"
    );
    assert_reference_has_variation(name, &reference[..luma_len]);

    let mut decoder = Vp8SequenceDecoder::new();
    let mut shown = 0usize;
    let mut worst_psnr = f64::INFINITY;

    for (idx, payload) in frames.iter().enumerate() {
        let decoded = match decoder.decode_frame(payload) {
            Ok(d) => d,
            Err(e) => panic!("{name}: coded frame {idx} must decode, got {e}"),
        };
        assert_eq!(
            decoded.is_keyframe,
            idx == 0,
            "{name}: frame {idx} frame type"
        );
        assert_eq!(decoded.image.width, width, "{name}: frame {idx} width");
        assert_eq!(decoded.image.height, height, "{name}: frame {idx} height");

        if !decoded.show_frame {
            // A hidden frame updates the reference buffers and contributes
            // no picture to the reference YUV.
            continue;
        }

        let base = shown * frame_len;
        assert!(
            base + frame_len <= reference.len(),
            "{name}: coded frame {idx} is shown frame {shown}, past the end of the reference"
        );
        let ref_y = &reference[base..base + luma_len];
        let ref_u = &reference[base + luma_len..base + luma_len + chroma_len];
        let ref_v = &reference[base + luma_len + chroma_len..base + frame_len];

        let tag = format!("{name}.frame{idx} (shown {shown})");
        assert_plane_bit_exact(&format!("{tag}.Y"), &decoded.image.y, ref_y, w);
        assert_plane_bit_exact(&format!("{tag}.U"), &decoded.image.u, ref_u, cw);
        assert_plane_bit_exact(&format!("{tag}.V"), &decoded.image.v, ref_v, cw);

        let db = psnr(&decoded.image.y, ref_y);
        assert!(db >= 30.0, "{tag}: luma PSNR vs libvpx is {db:.2} dB");
        worst_psnr = worst_psnr.min(db);
        shown += 1;
    }

    assert_eq!(
        shown * frame_len,
        reference.len(),
        "{name}: decoded {shown} shown frames, reference holds {}",
        reference.len() / frame_len
    );
    (frames.len(), shown, worst_psnr)
}

/// States the bit-exactness result as a number: a byte-equal plane has zero
/// mean squared error, so the worst per-frame luma PSNR of a stream that
/// passed [`verify_stream`]'s equality gate is infinite. Any finite value
/// here would mean those equality assertions stopped running.
fn assert_exact(worst_psnr: f64) {
    assert!(
        worst_psnr.is_infinite(),
        "every shown frame was byte-equal, so the worst PSNR must be infinite, got {worst_psnr:.2} dB"
    );
}

#[test]
fn test_p5basic_all_frames_bit_exact() {
    let (coded, shown, worst_psnr) = verify_stream("p5basic", 96, 64, &P5BASIC_FRAMES, P5BASIC_REF);
    assert_eq!((coded, shown), (5, 5));
    assert_exact(worst_psnr);
}

#[test]
fn test_refswap_all_frames_bit_exact_with_hidden_altref() {
    let (coded, shown, worst_psnr) = verify_stream("refswap", 96, 64, &REFSWAP_FRAMES, REFSWAP_REF);
    // 19 coded, 18 shown: packet 1 is the invisible altref frame.
    assert_eq!((coded, shown), (19, 18));
    assert_exact(worst_psnr);
}

#[test]
fn test_refswap_er_all_frames_bit_exact() {
    let (coded, shown, worst_psnr) =
        verify_stream("refswap_er", 96, 64, &REFSWAP_ER_FRAMES, REFSWAP_ER_REF);
    assert_eq!((coded, shown), (5, 5));
    assert_exact(worst_psnr);
}

#[test]
fn test_splitmv_all_frames_bit_exact() {
    // 100x64: the width is not a macroblock multiple, so this also proves
    // the partial right-hand macroblock column is reconstructed and cropped
    // correctly.
    let (coded, shown, worst_psnr) =
        verify_stream("splitmv", 100, 64, &SPLITMV_FRAMES, SPLITMV_REF);
    assert_eq!((coded, shown), (5, 5));
    assert_exact(worst_psnr);
}

#[test]
fn test_segdelta_all_frames_bit_exact() {
    let (coded, shown, worst_psnr) =
        verify_stream("segdelta", 96, 64, &SEGDELTA_FRAMES, SEGDELTA_REF);
    assert_eq!((coded, shown), (5, 5));
    assert_exact(worst_psnr);
}

#[test]
fn test_hidden_frame_is_decoded_but_not_shown() {
    // The `refswap` altref frame, decoded on its own after the key frame:
    // it must decode successfully and report `show_frame == false`, so a
    // caller knows not to emit it (and cannot mistake it for frame loss).
    let mut decoder = Vp8SequenceDecoder::new();
    let key = decoder
        .decode_frame(REFSWAP_FRAMES[0])
        .expect("key frame must decode");
    assert!(key.is_keyframe && key.show_frame);
    let hidden = decoder
        .decode_frame(REFSWAP_FRAMES[1])
        .expect("the hidden altref frame must decode");
    assert!(!hidden.is_keyframe, "packet 1 is an inter frame");
    assert!(!hidden.show_frame, "packet 1 is the invisible altref frame");
}

/// The byte offset at which `payload`'s first (header) partition ends —
/// frame tag bits 5.. hold its size, and an inter frame's tag is 3 bytes
/// (RFC 6386 §9.1).
fn first_partition_end(payload: &[u8]) -> usize {
    let tag = u32::from(payload[0]) | (u32::from(payload[1]) << 8) | (u32::from(payload[2]) << 16);
    3 + ((tag >> 5) & 0x7_FFFF) as usize
}

#[test]
fn test_truncated_inter_frame_is_rejected_honestly() {
    // A real inter frame cut off *inside its first partition*: the declared
    // first-partition size no longer fits the payload, which is exactly the
    // condition RFC 6386 §9.1's frame tag makes checkable. The decoder must
    // report it, not decode garbage and not panic.
    let full = P5BASIC_FRAMES[1];
    let part0_end = first_partition_end(full);
    assert!(
        part0_end > 3 && part0_end < full.len(),
        "fixture must have both a header and a token partition"
    );

    for cut in [3usize, part0_end / 2, part0_end - 1] {
        let mut decoder = Vp8SequenceDecoder::new();
        decoder
            .decode_frame(P5BASIC_FRAMES[0])
            .expect("key frame must decode");
        let err = match decoder.decode_frame(&full[..cut]) {
            Ok(_) => panic!("an inter frame cut to {cut} bytes must not decode"),
            Err(e) => e,
        };
        assert!(
            matches!(err, CodecError::InvalidBitstream(_)),
            "cut {cut}: expected an honest bitstream error, got {err:?}"
        );

        // The rejection happens before any cross-frame state is touched, so
        // the decoder stays usable: the *intact* frame must still decode
        // bit-exactly afterwards.
        let recovered = decoder
            .decode_frame(full)
            .expect("the intact frame must still decode after a rejected one");
        // p5basic frame 1 is shown frame 1: luma 96x64 at one frame in.
        let (luma_len, frame_len) = (96 * 64, 96 * 64 * 3 / 2);
        assert_eq!(
            recovered.image.y.as_slice(),
            &P5BASIC_REF[frame_len..frame_len + luma_len],
            "cut {cut}: recovery decode must still be bit-exact"
        );
    }
}

#[test]
fn test_truncated_token_partition_cannot_be_detected_but_never_panics() {
    // Honest documentation of a real limit rather than an aspiration: VP8's
    // *last* token partition carries no coded length — it "extends to the
    // end of the frame" (RFC 6386 §9.5), and the boolean decoder is defined
    // to keep supplying zero bits once its buffer is exhausted (§7.2). A
    // single-partition frame truncated after its header partition is
    // therefore indistinguishable from a legitimately smaller frame; no
    // decoder can reject it. What is guaranteed is that the decoder stays
    // memory-safe and terminates: no panic, no hang, no out-of-bounds read.
    let full = P5BASIC_FRAMES[1];
    let part0_end = first_partition_end(full);

    for extra in [0usize, 1, 4] {
        let mut decoder = Vp8SequenceDecoder::new();
        decoder
            .decode_frame(P5BASIC_FRAMES[0])
            .expect("key frame must decode");
        // Either outcome is acceptable; a panic is not.
        match decoder.decode_frame(&full[..part0_end + extra]) {
            Ok(decoded) => {
                assert_eq!(decoded.image.width, 96);
                assert_eq!(decoded.image.height, 64);
                assert_eq!(decoded.image.y.len(), 96 * 64);
            }
            Err(e) => assert!(
                matches!(e, CodecError::InvalidBitstream(_)),
                "unexpected error kind {e:?}"
            ),
        }
    }
}
