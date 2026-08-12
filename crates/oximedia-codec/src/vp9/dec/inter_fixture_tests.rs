//! VP9 multi-frame conformance fixtures (VP9 track package P4): real
//! `ffmpeg`/`libvpx-vp9`-encoded inter sequences, split into their raw
//! per-packet payloads (`<name>.frameN.bin`, no IVF/container framing --
//! see `testdata/RECIPE.md`), each with libvpx's own reconstruction of
//! every *shown* frame (`<name>.ref.yuv`, planar YUV 4:2:0), decoder
//! identity confirmed the same way as the `vp8` fixtures (ffmpeg's own
//! `-loglevel verbose` stream-mapping line, not assumed) and cross-checked
//! byte-identical against ffmpeg's independent native `vp9` decoder before
//! being committed.
//!
//! # What this file is now: the decoder's conformance gate
//!
//! Every test below feeds a whole real coded sequence through the **public**
//! [`Vp9Decoder`](crate::vp9::Vp9Decoder) API
//! ([`assert_bit_exact_sequence`], VP9 track package P12) and requires every
//! frame the decoder emits -- key, intra-only, inter and
//! `show_existing_frame` redisplay alike -- to be byte-identical to libvpx's
//! own reconstruction of it. `verified_frames == shown_frames` is asserted
//! per stream, so "the gate passed" cannot mean "the gate checked nothing";
//! [`the_gate_is_not_vacuous_a_planted_mismatch_is_reported`] plants a
//! one-byte corruption to prove the comparison itself has teeth.
//!
//! Superframe unpacking, `frame_size_with_refs` resolution, the
//! decoded-picture buffer, `refresh_frame_flags`, hidden frames and the
//! redisplay path are all *production* code inside the decoder now -- the
//! earlier revision of this file simulated them in the harness because
//! inter reconstruction did not exist yet.
//!
//! | stream | dims | coded/shown | flags that make it interesting |
//! |---|---|---|---|
//! | [`p9still`](p9still_every_shown_frame_bit_exact_vs_libvpx) | 76x42 | 8/8 | frozen source (`loop` filter replays source frame 0): every inter frame is near-all-skip |
//! | [`p9basic`](p9basic_every_shown_frame_bit_exact_vs_libvpx) | 76x42 | 8/8 | the *adapting* member of the `p9basic`/`p9er` control pair: identical content and encode settings, backward adaptation on |
//! | [`p9er`](p9er_every_shown_frame_bit_exact_vs_libvpx) | 76x42 | 8/8 | the control: `error_resilient` on every frame, so no backward adaptation and no temporal MV candidates |
//! | [`p9tc`](p9tc_every_shown_frame_bit_exact_vs_libvpx) | 512x64 | 5/5 | `tile_cols_log2 == 1` (2 tile columns) -- and, measured, the richest single stream in the set: ~945 inter blocks over four inter frames, all four inter modes, real motion vectors, two switchable filters |
//! | [`p9hp`](p9hp_every_shown_frame_bit_exact_vs_libvpx) | 76x42 | 8/8 | `-crf 8`: the lowest quantizer, i.e. the deepest coefficient coding in the 76x42 group |
//! | [`p9alt`](p9alt_every_shown_frame_bit_exact_vs_libvpx) | 128x128 | 8/9 (1 hidden) | 2-pass `auto-alt-ref`: a genuine **hidden** ALTREF frame inside a real superframe, `sign_bias` flips on the frames that reference it, and real compound prediction |
//!
//! Beware of over-reading the 76x42 group: `p9still`, `p9basic` and `p9hp`
//! are near-static encodes whose inter frames decode 14, 31 and 42 inter
//! blocks respectively across eight frames -- every one NEARESTMV with a
//! zero motion vector, no compound reference, no motion-vector symbols read
//! at all. They pin the *frame-level* machinery (adaptation on/off,
//! reference refresh, skip coding) and very little of motion compensation.
//! What the corpus really exercises is measured, not assumed, by
//! [`the_corpus_genuinely_exercises_the_inter_machinery`].
//!
//! Full provenance -- exact encode/decode commands, every frame's real
//! header-parse layout, sha256 of every committed byte, and the
//! cross-decoder agreement statement -- is in `testdata/RECIPE.md`.
//!
//! # The two deferred features, tested as refusals rather than skipped
//!
//! Two fixtures are *not* expected to decode end to end, and are asserted to
//! fail at a named packet with a named [`CodecError::UnsupportedFeature`]
//! instead of being `#[ignore]`d or thresholded:
//!
//! | stream | decodes | refuses at | feature |
//! |---|---|---|---|
//! | [`p9seg`](p9seg_inter_segmentation_is_refused_at_the_first_inter_frame) | packet 0 (key frame, bit-exact -- *intra* segmentation works) | packet 1 | inter-frame segmentation (`read_inter_segment_id`) |
//! | [`scaled`](scaled_reference_scaling_is_refused_at_the_resolution_change) | packets 0-5 (96x64, bit-exact, incl. 5 inter frames) | packet 6 | reference scaling (64x48 frame predicting from 96x64 slots) |
//!
//! Both assert the refusal's *position* as well as its text: a test that only
//! said "this stream errors" would still pass if packet 0 broke for an
//! unrelated reason.
//!
//! # Sibling import (P4-VERIFY): `compound`, `scaled`, `switch`
//!
//! Three more real `ffmpeg`/`libvpx-vp9` streams imported from a
//! separately-generated fixture package, each covering ground the seven
//! streams above do not -- full attribution, verbatim encode commands, and
//! import validation are in `testdata/RECIPE.md`'s "Sibling import"
//! section, not repeated here.
//!
//! | stream | dims | coded/shown | what it adds |
//! |---|---|---|---|
//! | [`compound`](compound_every_shown_frame_bit_exact_vs_libvpx) | 96x64 | 13/12 (1 hidden) | a second real hidden-ARF-in-a-superframe stream, with the hidden sub-frame on a *different* `frame_context_idx` than the shown chain |
//! | [`scaled`](scaled_reference_scaling_is_refused_at_the_resolution_change) | 96x64 -> 64x48 at coded frame 6 | 12/12 | the only fixture here (native or imported) with a real mid-stream resolution change -- fills the `p9scale` gap `testdata/RECIPE.md` documents as honestly skipped, and pins where reference scaling is refused |
//! | [`switch`](switch_every_shown_frame_bit_exact_vs_libvpx) | 100x68 | 10/10 | frame-level `interpolation_filter` genuinely varies *within* one sequence (`SWITCHABLE` then fixed `EIGHTTAP`), on non-8-aligned dimensions -- the only fixture with a *partial last superblock row* (`mi_rows = 9`), which is why it found a real prediction-extent bug when it was imported |
//!
//! # `p9sef` (P4-VERIFY): the one hand-built exception
//!
//! Every fixture above -- native or imported -- is 100% real encoder
//! output; `p9sef` is the **one deliberate exception**, hand-built because
//! neither this package's own encodes nor the sibling package's five
//! streams happened to produce a real `show_existing_frame` occurrence
//! (both `RECIPE.md`s document the (bounded, not exhaustive) search that
//! didn't find one). `p9sef.frame0.bin` is a byte-identical copy of
//! `p9still.frame0.bin` (a real encoder-produced key frame -- reused, not
//! re-derived); `p9sef.frame1.bin` is one hand-constructed byte encoding a
//! `show_existing_frame` header. Every bit is spelled out in
//! `testdata/RECIPE.md`'s "p9sef" section. Its redisplayed pixels are now
//! verified like any other emitted frame -- the decoder serves them from the
//! real decoded-picture buffer -- see
//! [`p9sef_redisplays_the_stored_slot_bit_exactly`].
//!
//! `compound`'s `.frameN.bin` files use a **different convention** than
//! every other stream in this file: they are already superframe-split
//! (decode order across the whole stream, index bytes discarded) rather
//! than one file per raw IVF packet, so `COMPOUND_FRAMES.len() == 13`
//! counts VP9 frames, not IVF packets -- see the doc comment on
//! [`COMPOUND_FRAMES`] and `testdata/RECIPE.md`'s "Sibling import" section
//! for exactly why and what that costs (no second confirming case for the
//! `Superframe::parse` size-sum fix from this fixture).

use super::testutil::{assert_bit_exact_sequence, run_sequence};
use super::{Vp9DecState, Vp9RefSlot};
use crate::error::CodecError;

/// The 8 coded frames of `p9still`, in bitstream order.
const P9STILL_FRAMES: [&[u8]; 8] = [
    include_bytes!("testdata/p9still.frame0.bin"),
    include_bytes!("testdata/p9still.frame1.bin"),
    include_bytes!("testdata/p9still.frame2.bin"),
    include_bytes!("testdata/p9still.frame3.bin"),
    include_bytes!("testdata/p9still.frame4.bin"),
    include_bytes!("testdata/p9still.frame5.bin"),
    include_bytes!("testdata/p9still.frame6.bin"),
    include_bytes!("testdata/p9still.frame7.bin"),
];
/// libvpx's reference reconstruction of every shown `p9still` frame.
const P9STILL_REF: &[u8] = include_bytes!("testdata/p9still.ref.yuv");

/// The 8 coded frames of `p9basic`, in bitstream order.
const P9BASIC_FRAMES: [&[u8]; 8] = [
    include_bytes!("testdata/p9basic.frame0.bin"),
    include_bytes!("testdata/p9basic.frame1.bin"),
    include_bytes!("testdata/p9basic.frame2.bin"),
    include_bytes!("testdata/p9basic.frame3.bin"),
    include_bytes!("testdata/p9basic.frame4.bin"),
    include_bytes!("testdata/p9basic.frame5.bin"),
    include_bytes!("testdata/p9basic.frame6.bin"),
    include_bytes!("testdata/p9basic.frame7.bin"),
];
/// libvpx's reference reconstruction of every shown `p9basic` frame.
const P9BASIC_REF: &[u8] = include_bytes!("testdata/p9basic.ref.yuv");

/// The 8 coded frames of `p9er`, in bitstream order.
const P9ER_FRAMES: [&[u8]; 8] = [
    include_bytes!("testdata/p9er.frame0.bin"),
    include_bytes!("testdata/p9er.frame1.bin"),
    include_bytes!("testdata/p9er.frame2.bin"),
    include_bytes!("testdata/p9er.frame3.bin"),
    include_bytes!("testdata/p9er.frame4.bin"),
    include_bytes!("testdata/p9er.frame5.bin"),
    include_bytes!("testdata/p9er.frame6.bin"),
    include_bytes!("testdata/p9er.frame7.bin"),
];
/// libvpx's reference reconstruction of every shown `p9er` frame.
const P9ER_REF: &[u8] = include_bytes!("testdata/p9er.ref.yuv");

/// The 5 coded frames of `p9tc`, in bitstream order.
const P9TC_FRAMES: [&[u8]; 5] = [
    include_bytes!("testdata/p9tc.frame0.bin"),
    include_bytes!("testdata/p9tc.frame1.bin"),
    include_bytes!("testdata/p9tc.frame2.bin"),
    include_bytes!("testdata/p9tc.frame3.bin"),
    include_bytes!("testdata/p9tc.frame4.bin"),
];
/// libvpx's reference reconstruction of every shown `p9tc` frame.
const P9TC_REF: &[u8] = include_bytes!("testdata/p9tc.ref.yuv");

/// The 8 coded frames of `p9hp`, in bitstream order.
const P9HP_FRAMES: [&[u8]; 8] = [
    include_bytes!("testdata/p9hp.frame0.bin"),
    include_bytes!("testdata/p9hp.frame1.bin"),
    include_bytes!("testdata/p9hp.frame2.bin"),
    include_bytes!("testdata/p9hp.frame3.bin"),
    include_bytes!("testdata/p9hp.frame4.bin"),
    include_bytes!("testdata/p9hp.frame5.bin"),
    include_bytes!("testdata/p9hp.frame6.bin"),
    include_bytes!("testdata/p9hp.frame7.bin"),
];
/// libvpx's reference reconstruction of every shown `p9hp` frame.
const P9HP_REF: &[u8] = include_bytes!("testdata/p9hp.ref.yuv");

/// The 8 coded frames of `p9seg`, in bitstream order.
const P9SEG_FRAMES: [&[u8]; 8] = [
    include_bytes!("testdata/p9seg.frame0.bin"),
    include_bytes!("testdata/p9seg.frame1.bin"),
    include_bytes!("testdata/p9seg.frame2.bin"),
    include_bytes!("testdata/p9seg.frame3.bin"),
    include_bytes!("testdata/p9seg.frame4.bin"),
    include_bytes!("testdata/p9seg.frame5.bin"),
    include_bytes!("testdata/p9seg.frame6.bin"),
    include_bytes!("testdata/p9seg.frame7.bin"),
];
/// libvpx's reference reconstruction of every shown `p9seg` frame.
const P9SEG_REF: &[u8] = include_bytes!("testdata/p9seg.ref.yuv");

/// The 8 coded frames of `p9alt`, in bitstream order. Frame 1 is a real
/// superframe: a hidden ALTREF sub-frame followed by the visible frame that
/// references it (see `testdata/p9alt.layout.txt`).
const P9ALT_FRAMES: [&[u8]; 8] = [
    include_bytes!("testdata/p9alt.frame0.bin"),
    include_bytes!("testdata/p9alt.frame1.bin"),
    include_bytes!("testdata/p9alt.frame2.bin"),
    include_bytes!("testdata/p9alt.frame3.bin"),
    include_bytes!("testdata/p9alt.frame4.bin"),
    include_bytes!("testdata/p9alt.frame5.bin"),
    include_bytes!("testdata/p9alt.frame6.bin"),
    include_bytes!("testdata/p9alt.frame7.bin"),
];
/// libvpx's reference reconstruction of every shown `p9alt` frame (8 of the
/// 9 coded sub-frames -- the ALTREF sub-frame of the frame1 superframe is
/// hidden).
const P9ALT_REF: &[u8] = include_bytes!("testdata/p9alt.ref.yuv");

/// The 13 coded VP9 frames of `compound` (imported, see the module doc
/// comment's "Sibling import" section), in decode order. **Not** 13 IVF
/// packets: the source package's own splitter already unpacked the one
/// real superframe (coded frame 1, the hidden ALTREF, sharing IVF packet 1
/// with coded frame 2) before these files were generated, so
/// `COMPOUND_FRAMES[1]`/`[2]` are two plain single frames with their
/// superframe index bytes already stripped -- unlike every other `*_FRAMES`
/// array in this file, where one array entry is one real IVF packet
/// (occasionally unpacking to more than one VP9 frame via
/// `Superframe::parse` inside `assert_bit_exact_sequence` itself, as
/// `P9ALT_FRAMES[1]` does). `check.coded_frames` for this stream therefore
/// means "13 `.frameN.bin` files", not "13 transport packets".
const COMPOUND_FRAMES: [&[u8]; 13] = [
    include_bytes!("testdata/compound.frame0.bin"),
    include_bytes!("testdata/compound.frame1.bin"),
    include_bytes!("testdata/compound.frame2.bin"),
    include_bytes!("testdata/compound.frame3.bin"),
    include_bytes!("testdata/compound.frame4.bin"),
    include_bytes!("testdata/compound.frame5.bin"),
    include_bytes!("testdata/compound.frame6.bin"),
    include_bytes!("testdata/compound.frame7.bin"),
    include_bytes!("testdata/compound.frame8.bin"),
    include_bytes!("testdata/compound.frame9.bin"),
    include_bytes!("testdata/compound.frame10.bin"),
    include_bytes!("testdata/compound.frame11.bin"),
    include_bytes!("testdata/compound.frame12.bin"),
];
/// libvpx's reference reconstruction of every shown `compound` frame (12 of
/// the 13 coded frames -- coded frame 1, the hidden ALTREF, is not shown).
const COMPOUND_REF: &[u8] = include_bytes!("testdata/compound.ref.yuv");

/// The 10 coded frames of `switch` (imported), in bitstream order -- one
/// IVF packet per entry, this file's usual convention (no superframes in
/// this stream).
const SWITCH_FRAMES: [&[u8]; 10] = [
    include_bytes!("testdata/switch.frame0.bin"),
    include_bytes!("testdata/switch.frame1.bin"),
    include_bytes!("testdata/switch.frame2.bin"),
    include_bytes!("testdata/switch.frame3.bin"),
    include_bytes!("testdata/switch.frame4.bin"),
    include_bytes!("testdata/switch.frame5.bin"),
    include_bytes!("testdata/switch.frame6.bin"),
    include_bytes!("testdata/switch.frame7.bin"),
    include_bytes!("testdata/switch.frame8.bin"),
    include_bytes!("testdata/switch.frame9.bin"),
];
/// libvpx's reference reconstruction of every shown `switch` frame (all 10
/// -- no hidden frames in this stream).
const SWITCH_REF: &[u8] = include_bytes!("testdata/switch.ref.yuv");

/// The 12 coded frames of `scaled` (imported), in bitstream order -- one
/// IVF packet per entry (no superframes in this stream). Real mid-stream
/// resolution change at coded frame 6: 96x64 for frames 0-5, 64x48 for
/// frames 6-11 (see [`SCALED_DIMS`]).
const SCALED_FRAMES: [&[u8]; 12] = [
    include_bytes!("testdata/scaled.frame0.bin"),
    include_bytes!("testdata/scaled.frame1.bin"),
    include_bytes!("testdata/scaled.frame2.bin"),
    include_bytes!("testdata/scaled.frame3.bin"),
    include_bytes!("testdata/scaled.frame4.bin"),
    include_bytes!("testdata/scaled.frame5.bin"),
    include_bytes!("testdata/scaled.frame6.bin"),
    include_bytes!("testdata/scaled.frame7.bin"),
    include_bytes!("testdata/scaled.frame8.bin"),
    include_bytes!("testdata/scaled.frame9.bin"),
    include_bytes!("testdata/scaled.frame10.bin"),
    include_bytes!("testdata/scaled.frame11.bin"),
];
/// libvpx's reference reconstruction of every shown `scaled` frame (all 12
/// -- no hidden frames), **heterogeneous** dimensions (see [`SCALED_DIMS`]):
/// decoded with `vpxdec`, not ffmpeg, because ffmpeg's `-f rawvideo` muxer
/// silently re-normalizes every frame to the first frame's size for a
/// genuinely variable-resolution decode (see `testdata/RECIPE.md`'s
/// "Sibling import" section, point 4).
const SCALED_REF: &[u8] = include_bytes!("testdata/scaled.ref.yuv");
/// Expected `(width, height)` of each of [`SCALED_FRAMES`]'s 12 entries, in
/// order -- matches both the sibling package's own construction and this
/// crate's real header parse (cross-checked at import time, see
/// `testdata/RECIPE.md`).
/// `p9sef`'s two "frames": a byte-identical copy of `p9still.frame0.bin`
/// (real encoder-produced key frame) followed by one hand-constructed
/// `show_existing_frame` byte. See the module doc comment's `p9sef`
/// section and `testdata/RECIPE.md`'s "p9sef" section for the full,
/// bit-by-bit construction.
const P9SEF_FRAMES: [&[u8]; 2] = [
    include_bytes!("testdata/p9sef.frame0.bin"),
    include_bytes!("testdata/p9sef.frame1.bin"),
];
/// The key frame's libvpx-decoded pixels, concatenated twice: once for the
/// real decode of `p9sef.frame0.bin`, once for the redisplay
/// `p9sef.frame1.bin` requests (this harness cannot verify a redisplay's
/// pixels without an inter/DPB decoder -- see
/// [`assert_bit_exact_sequence`]'s doc comment -- so the second copy only
/// keeps the reference-byte accounting honest, it is not independently
/// re-verified against anything).
const P9SEF_REF: &[u8] = include_bytes!("testdata/p9sef.ref.yuv");

const SCALED_DIMS: [(u32, u32); 12] = [
    (96, 64),
    (96, 64),
    (96, 64),
    (96, 64),
    (96, 64),
    (96, 64),
    (64, 48),
    (64, 48),
    (64, 48),
    (64, 48),
    (64, 48),
    (64, 48),
];

// ---------------------------------------------------------------------------
// The gate: every shown frame of every stream, through the public decoder
// ---------------------------------------------------------------------------

/// Frozen source (`loop` replays source frame 0): all eight shown frames are
/// pixel-identical, and the seven inter frames decode 14 inter blocks
/// between them -- every one NEARESTMV with a zero motion vector.
///
/// That makes it the *structural* test, not a motion test: a failure here is
/// the reconstruction order, the mode-info branch, the coefficient `ref`
/// index or the skip/reference bookkeeping, because there is essentially no
/// motion compensation left to blame. Motion lives in `p9tc`, `p9alt`,
/// `compound` and `switch`.
#[test]
fn p9still_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&P9STILL_FRAMES, P9STILL_REF, 76, 42, "p9still");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (8, 8, 8)
    );
}

/// The adapting half of the `p9basic`/`p9er` control pair: nothing here is
/// error-resilient or frame-parallel, so every frame decodes against the
/// probability context its predecessor *adapted*, and
/// [`p9er`](p9er_every_shown_frame_bit_exact_vs_libvpx) is the identical
/// content and encode with adaptation switched off. Two streams that differ
/// only in that one flag and are both bit-exact is what isolates backward
/// adaptation.
///
/// Its content is near-static (31 inter blocks over seven inter frames, all
/// NEARESTMV, zero motion) -- deliberately not the stream to read a
/// motion-compensation claim off.
#[test]
fn p9basic_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&P9BASIC_FRAMES, P9BASIC_REF, 76, 42, "p9basic");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (8, 8, 8)
    );
}

/// `error_resilient` on every frame: no backward adaptation, no temporal
/// motion-vector candidates. The control for `p9basic`.
#[test]
fn p9er_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&P9ER_FRAMES, P9ER_REF, 76, 42, "p9er");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (8, 8, 8)
    );
}

/// 512x64 with `tile_cols_log2 == 1` (2 tile columns): per-tile bool
/// decoders, per-tile above/left context resets and the tile-boundary
/// availability rules, on inter frames rather than only the key frame.
///
/// Measured, it is also the densest stream in the corpus: ~945 inter and 53
/// intra blocks across four inter frames, all four inter modes
/// (767/100/10/155 NEAREST/NEAR/ZERO/NEW), 155 coded motion vectors and
/// both EIGHTTAP and EIGHTTAP_SMOOTH chosen per block on SWITCHABLE frames.
#[test]
fn p9tc_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&P9TC_FRAMES, P9TC_REF, 512, 64, "p9tc");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (5, 5, 5)
    );
}

/// `-crf 8`, the lowest quantizer in the set: the deepest coefficient coding
/// of the three 76x42 streams, and so the one most sensitive to a
/// dequantizer or inverse-transform error.
///
/// `allow_high_precision_mv` is set on its inter frames, but the content is
/// near-static enough that no motion vector is ever coded (42 inter blocks,
/// all NEARESTMV, zero motion) -- the 1/8-pel bit is really exercised by
/// `p9tc`, `p9alt` and `compound`, which is measured in
/// [`the_corpus_genuinely_exercises_the_inter_machinery`], not assumed
/// here.
#[test]
fn p9hp_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&P9HP_FRAMES, P9HP_REF, 76, 42, "p9hp");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (8, 8, 8)
    );
}

/// 8 coded packets, 9 sub-frames (packet 1 is a real superframe), 8 shown:
/// the packet-1 ALTREF sub-frame is hidden — it decodes and refreshes slot 2
/// but emits nothing. The only native fixture with `sign_bias` flips, which
/// is what makes compound prediction and the fixed/variable reference split
/// reachable — and, measured, it does not merely make them reachable: 33 of
/// its blocks really decode as two-reference compound.
#[test]
fn p9alt_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&P9ALT_FRAMES, P9ALT_REF, 128, 128, "p9alt");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (8, 8, 8)
    );
}

/// 13 pre-split VP9 frames (not 13 IVF packets — see [`COMPOUND_FRAMES`]),
/// 12 shown. A second real hidden-ARF stream, independent of `p9alt`, with
/// the hidden sub-frame on a *different* `frame_context_idx` than the shown
/// chain — so it pins the per-index context save/load as well as the
/// compound-reference path (52 measured compound blocks; the fixture's own
/// `layout.txt` could only establish the frame-level *precondition* for
/// compound, since per-block use is arithmetic-coded).
#[test]
fn compound_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&COMPOUND_FRAMES, COMPOUND_REF, 96, 64, "compound");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (13, 12, 12)
    );
}

/// 100x68: the only fixture in the crate with a **partial last superblock
/// row** (`mi_rows = (68+7)>>3 = 9`, so superblock row 1 holds one real MI
/// row and seven that are outside the frame) *and* a partial last superblock
/// column (`mi_cols = 13`), plus a frame-level `interpolation_filter` that
/// genuinely varies within the sequence (`SWITCHABLE`, then fixed
/// `EIGHTTAP`).
///
/// This test was `#[ignore]`d when the stream was imported: the key frame
/// panicked writing row 72 of a 72-row plane, from a block in the
/// partially-out-of-frame superblock row. It is un-ignored here because the
/// whole sequence — key frame and all nine inter frames — now reconstructs
/// bit-exactly.
///
/// The import-time bug report guessed the fix would be clamping the
/// prediction extent to `mi_rows`; the fix that actually landed (in
/// `recon.rs`, by a sibling package, not here) is the opposite and is the
/// one libvpx justifies: the overhanging write is *legal* —
/// `decode_partition` only forces a split when a block's half falls outside,
/// and transform blocks step to the aligned picture edge — so the planes are
/// allocated with a `PLANE_OVERHANG` border, exactly like
/// `vpx_realloc_frame_buffer`'s, and the overhang is written and never read
/// back. Clamping would have silently corrupted the padding that motion
/// compensation legitimately reads near the frame edges. See
/// `recon::PLANE_OVERHANG` for the derivation and the poison-byte experiment
/// that measured "never read back".
///
/// It also answers the open question that bug report left — whether a second
/// defect was hiding in the *column* direction behind the row one (`mi_cols
/// = 13` is partial too): no.
#[test]
fn switch_every_shown_frame_bit_exact_vs_libvpx() {
    let check = assert_bit_exact_sequence(&SWITCH_FRAMES, SWITCH_REF, 100, 68, "switch");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (10, 10, 10)
    );
}

/// `p9sef`'s redisplay, served from the real decoded-picture buffer.
///
/// Packet 0 is a real encoder-produced key frame; packet 1 is the
/// hand-constructed `show_existing_frame` byte. Both emitted frames are
/// pixel-verified against `p9sef.ref.yuv` (which holds the key frame's
/// libvpx reconstruction twice), so the redisplay is checked rather than
/// merely counted — it must reproduce the stored slot byte for byte.
#[test]
fn p9sef_redisplays_the_stored_slot_bit_exactly() {
    let check = assert_bit_exact_sequence(&P9SEF_FRAMES, P9SEF_REF, 76, 42, "p9sef");
    assert_eq!(
        (
            check.coded_frames,
            check.shown_frames,
            check.verified_frames
        ),
        (2, 2, 2)
    );

    use crate::vp9::UncompressedHeader;
    let hdr = UncompressedHeader::parse_with_ref_sizes(P9SEF_FRAMES[1], &[None; 8])
        .expect("p9sef.frame1: header parse");
    assert!(
        hdr.show_existing_frame,
        "frame1 must be show_existing_frame"
    );
    assert_eq!(
        hdr.frame_to_show, 5,
        "frame1 must request slot 5 (the keyframe refreshed all 8 slots, \
         so any 0..=7 would be a valid encode -- slot 5 was chosen \
         specifically to discriminate a bit-order bug from a correct \
         parse, see testdata/RECIPE.md's p9sef section)"
    );
}

// ---------------------------------------------------------------------------
// The two deferred features: refused at a named packet, with a named error
// ---------------------------------------------------------------------------

/// Inter-frame segmentation is refused, and the refusal lands exactly where
/// it should: **not** on the key frame.
///
/// `p9seg` is `aq-mode 1`, so `seg.enabled` is set on every frame. Packet 0
/// decodes and is bit-exact, which is the load-bearing half of this test —
/// *intra* segmentation (map read, `ALT_Q` feature data, per-segment
/// quantizer and loop-filter levels) is fully implemented. Only
/// `read_inter_segment_id`, which needs the previous frame's segment map and
/// this frame's write-back, is missing, so packet 1 must be the first
/// failure.
#[test]
fn p9seg_inter_segmentation_is_refused_at_the_first_inter_frame() {
    let dims = [(76u32, 42u32); 8];
    let run = run_sequence(&P9SEG_FRAMES, P9SEG_REF, &dims, "p9seg");

    assert!(
        run.problems.is_empty(),
        "p9seg's key frame must still be bit-exact:\n  {}",
        run.problems.join("\n  ")
    );
    assert_eq!(
        (run.check.shown_frames, run.check.verified_frames),
        (1, 1),
        "exactly the key frame decodes, and it is pixel-verified"
    );

    let (idx, err) = run
        .error
        .expect("inter-frame segmentation is not implemented, so this stream must be refused");
    assert_eq!(idx, 1, "the first inter frame is where it must be refused");
    match err {
        CodecError::UnsupportedFeature(msg) => {
            assert!(
                msg.contains("segmentation") && msg.contains("read_inter_segment_id"),
                "the refusal must name the feature and the missing piece: {msg}"
            );
        }
        other => panic!("expected an honest UnsupportedFeature, got {other:?}"),
    }
}

/// Reference scaling is refused, and the refusal lands exactly at the
/// resolution change — not before it.
///
/// `scaled` codes packets 0-5 at 96x64 (one key frame plus five inter
/// frames) and packets 6-11 at 64x48. The first six decode and are
/// bit-exact; packet 6 is a genuinely non-key, non-intra-only 64x48 frame
/// whose three reference slots all still hold 96x64 pictures, which is
/// precisely the case VP9 answers with decoder-side reference scaling and
/// this decoder answers with an honest refusal.
#[test]
fn scaled_reference_scaling_is_refused_at_the_resolution_change() {
    let run = run_sequence(&SCALED_FRAMES, SCALED_REF, &SCALED_DIMS, "scaled");

    assert!(
        run.problems.is_empty(),
        "scaled's 96x64 prefix must be bit-exact:\n  {}",
        run.problems.join("\n  ")
    );
    assert_eq!(
        (run.check.shown_frames, run.check.verified_frames),
        (6, 6),
        "packets 0-5 all decode and are pixel-verified"
    );
    assert_eq!(
        run.consumed_ref_bytes,
        6 * (96 * 64 + 2 * 48 * 32),
        "and they consume exactly the six 96x64 frames at the head of \
         scaled.ref.yuv (see scaled.layout.txt's byte-offset table)"
    );

    let (idx, err) = run
        .error
        .expect("reference scaling is not implemented, so this stream must be refused");
    assert_eq!(idx, 6, "the first 64x48 frame is where it must be refused");
    match err {
        CodecError::UnsupportedFeature(msg) => {
            assert!(
                msg.contains("reference scaling") && msg.contains("96x64") && msg.contains("64x48"),
                "the refusal must name the feature and both geometries: {msg}"
            );
        }
        other => panic!("expected an honest UnsupportedFeature, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The gate's own credibility
// ---------------------------------------------------------------------------

/// A gate that reports "bit-exact" has to be able to report "not bit-exact".
///
/// One byte of `p9still`'s reference is flipped, in the *last* shown frame's
/// V plane, and the harness must name that exact pixel. Corrupting the last
/// frame rather than the first also proves the walk really reaches the end
/// of the sequence: a harness that silently stopped after frame 0 would
/// report nothing here and this test would fail.
#[test]
fn the_gate_is_not_vacuous_a_planted_mismatch_is_reported() {
    const W: usize = 76;
    const H: usize = 42;
    const CW: usize = 38;
    const CH: usize = 21;
    const FRAME_LEN: usize = W * H + 2 * CW * CH;

    let mut corrupted = P9STILL_REF.to_vec();
    assert_eq!(corrupted.len(), 8 * FRAME_LEN);
    // Frame 7, V plane, row 3, column 5.
    let offset = 7 * FRAME_LEN + W * H + CW * CH + 3 * CW + 5;
    corrupted[offset] ^= 0x40;

    let dims = [(W as u32, H as u32); 8];
    let run = run_sequence(&P9STILL_FRAMES, &corrupted, &dims, "p9still-corrupted");

    assert!(run.error.is_none(), "the stream itself still decodes");
    assert_eq!(run.check.verified_frames, 8);
    assert_eq!(
        run.problems.len(),
        1,
        "exactly one plane of one frame was corrupted, so exactly one \
         problem must be reported: {:?}",
        run.problems
    );
    let reported = &run.problems[0];
    assert!(
        reported.contains("p9still-corrupted.frame7.out7.V")
            && reported.contains("first mismatch at (5,3)")
            && reported.contains("1 of 798 pixels differ"),
        "the report must name the frame, the plane and the exact pixel: {reported}"
    );
}

/// `switch`'s stated reason for import — `interpolation_filter` genuinely
/// varying within one sequence — pinned at the header level, independent of
/// the pixel gate above, so a regression in either one is attributable.
/// Header parsing here is real production code
/// (`UncompressedHeader::parse_with_ref_sizes`).
#[test]
fn switch_interp_filter_genuinely_varies_header_level_only() {
    use crate::vp9::UncompressedHeader;

    let mut ref_sizes: [Option<(u32, u32)>; 8] = [None; 8];
    let mut interp_filters = Vec::with_capacity(SWITCH_FRAMES.len());
    for (idx, &payload) in SWITCH_FRAMES.iter().enumerate() {
        let hdr = UncompressedHeader::parse_with_ref_sizes(payload, &ref_sizes)
            .unwrap_or_else(|e| panic!("switch.frame{idx}: header parse: {e}"));
        interp_filters.push(hdr.interp_filter);
        for slot in 0..8 {
            if hdr.refresh_frame_flags & (1 << slot) != 0 {
                ref_sizes[slot] = Some((hdr.width, hdr.height));
            }
        }
    }

    // Frame 0 is the key frame: interp_filter is not coded for it (stays
    // at UncompressedHeader::default()'s 0). Frame 1 is SWITCHABLE (raw
    // value 4, uncompressed.rs's parse_interp_filter); frames 2-9 are fixed
    // EIGHTTAP (raw value 1) -- exactly `testdata/RECIPE.md`'s "Sibling
    // import" claim, pinned here independent of pixel decode.
    assert_eq!(interp_filters[1], 4, "frame 1 must be SWITCHABLE");
    for (idx, &f) in interp_filters.iter().enumerate().skip(2) {
        assert_eq!(f, 1, "frame {idx} must be fixed EIGHTTAP");
    }
}

// ---------------------------------------------------------------------------
// What the corpus actually exercises
// ---------------------------------------------------------------------------

/// Symbol counters aggregated over every fixture frame that codes them.
struct CorpusCoverage {
    /// `comp_inter[*][0]` / `[1]`: blocks that chose single / compound
    /// reference.
    single_vs_compound: [u32; 2],
    /// `comp_ref` reads — one per compound block's reference choice.
    comp_ref_reads: u32,
    /// `inter_mode[*][m]` totals, in libvpx's order: NEARESTMV, NEARMV,
    /// ZEROMV, NEWMV.
    inter_modes: [u32; 4],
    /// `intra_inter[*][0]` / `[1]`: intra / inter blocks inside inter frames.
    intra_vs_inter: [u32; 2],
    /// `switchable_interp[*][f]` totals: EIGHTTAP, EIGHTTAP_SMOOTH,
    /// EIGHTTAP_SHARP, chosen per block on a SWITCHABLE frame.
    switchable_filters: [u32; 3],
    /// `mv.joints[j]` totals: ZERO, HNZVZ, HZVNZ, HNZVNZ.
    mv_joints: [u32; 4],
    /// `mv.comps[*].hp` + `class0_hp` totals: the 1/8-pel bit, read only when
    /// `allow_high_precision_mv` holds and the vector is small enough.
    mv_hp_bits: u32,
    /// Frames whose symbols were counted at all.
    counted_frames: usize,
}

/// Decodes every fixture stream through the cross-frame decoder state and
/// aggregates the symbol counters libvpx's backward adaptation uses.
///
/// This is a **coverage probe, not the gate** — the gate is the per-stream
/// bit-exactness above, which runs through the public API. This one reaches
/// into `Vp9DecState` because the symbol counters are not on the public API
/// at all, and they are the only direct evidence of *which* decode paths a
/// fixture set really reaches: "every stream is bit-exact" is a much weaker
/// statement if the streams turn out to be all skip blocks (three of them
/// nearly are — see the test below).
///
/// **This is therefore a second frame loop**, and the only one left in this
/// crate's tests — `testutil` deliberately gave its own up to drive the
/// shipped decoder instead. It must stay in step with
/// `Vp9Decoder::decode_frame`: same superframe unpacking, same
/// `parse_with_ref_sizes` / `begin_frame` / decode / `finish_frame` order,
/// and the same `show_existing_frame` rule (emit nothing, disturb nothing —
/// here simply `continue`, which is why `p9sef` is not in the stream list
/// below). If that function's structure changes, change this one too, or its
/// numbers quietly stop describing the real decoder.
///
/// Only frames with `frame_parallel_decoding_mode == 0` and
/// `error_resilient_mode == 0` count their symbols at all (libvpx nulls
/// `xd->counts` otherwise), so `p9er` and `switch` contribute nothing here
/// despite decoding bit-exactly: 8 streams are walked, 6 contribute counts.
fn measure_corpus_coverage() -> CorpusCoverage {
    use super::{decode_inter_frame_with_state, decode_intra_frame_with_state};
    use crate::vp9::superframe::Superframe;
    use crate::vp9::UncompressedHeader;

    let mut cov = CorpusCoverage {
        single_vs_compound: [0; 2],
        comp_ref_reads: 0,
        inter_modes: [0; 4],
        intra_vs_inter: [0; 2],
        switchable_filters: [0; 3],
        mv_joints: [0; 4],
        mv_hp_bits: 0,
        counted_frames: 0,
    };

    for (name, frames) in [
        ("p9still", &P9STILL_FRAMES[..]),
        ("p9basic", &P9BASIC_FRAMES[..]),
        ("p9er", &P9ER_FRAMES[..]),
        ("p9hp", &P9HP_FRAMES[..]),
        ("p9tc", &P9TC_FRAMES[..]),
        ("p9alt", &P9ALT_FRAMES[..]),
        ("compound", &COMPOUND_FRAMES[..]),
        ("switch", &SWITCH_FRAMES[..]),
    ] {
        let mut state = Vp9DecState::new();
        for (idx, &payload) in frames.iter().enumerate() {
            let subs = match Superframe::parse(payload) {
                Ok(sf) => sf.frames,
                Err(_) => vec![payload.to_vec()],
            };
            for sub in &subs {
                let mut hdr = UncompressedHeader::parse_with_ref_sizes(sub, &state.ref_sizes())
                    .unwrap_or_else(|e| panic!("{name}.frame{idx}: header parse: {e}"));
                if hdr.show_existing_frame {
                    continue;
                }
                state
                    .begin_frame(&mut hdr)
                    .unwrap_or_else(|e| panic!("{name}.frame{idx}: begin_frame: {e}"));
                let counted = Vp9DecState::counts_enabled(&hdr);
                let decoded = if hdr.is_intra_only() {
                    decode_intra_frame_with_state(&mut state, &hdr, sub)
                } else {
                    decode_inter_frame_with_state(&mut state, &hdr, sub)
                }
                .unwrap_or_else(|e| panic!("{name}.frame{idx}: decode: {e}"));

                if counted {
                    cov.counted_frames += 1;
                    let c = &state.counts;
                    for ctx in c.comp_inter {
                        cov.single_vs_compound[0] += ctx[0];
                        cov.single_vs_compound[1] += ctx[1];
                    }
                    for ctx in c.comp_ref {
                        cov.comp_ref_reads += ctx[0] + ctx[1];
                    }
                    for ctx in c.inter_mode {
                        for (total, n) in cov.inter_modes.iter_mut().zip(ctx.iter()) {
                            *total += n;
                        }
                    }
                    for ctx in c.intra_inter {
                        cov.intra_vs_inter[0] += ctx[0];
                        cov.intra_vs_inter[1] += ctx[1];
                    }
                    for ctx in c.switchable_interp {
                        for (total, n) in cov.switchable_filters.iter_mut().zip(ctx.iter()) {
                            *total += n;
                        }
                    }
                    for (total, n) in cov.mv_joints.iter_mut().zip(c.mv.joints.iter()) {
                        *total += n;
                    }
                    for comp in &c.mv.comps {
                        cov.mv_hp_bits +=
                            comp.hp[0] + comp.hp[1] + comp.class0_hp[0] + comp.class0_hp[1];
                    }
                }

                let slot = Vp9RefSlot::from_decoded_frame(decoded, hdr.intra_only, hdr.show_frame)
                    .unwrap_or_else(|e| panic!("{name}.frame{idx}: reference slot: {e}"));
                state
                    .finish_frame(&hdr, slot)
                    .unwrap_or_else(|e| panic!("{name}.frame{idx}: finish_frame: {e}"));
            }
        }
    }
    cov
}

/// "Every stream is bit-exact" is only worth as much as the streams: a corpus
/// of all-skip frames would be reproduced perfectly by a decoder that did
/// nothing but copy `LAST`. This pins the decode paths the corpus really
/// reaches, so a future fixture change that quietly hollows it out fails
/// here rather than passing everything.
///
/// The numbers are lower bounds taken from the measured corpus, deliberately
/// well under the observed values so ordinary fixture churn does not trip
/// them. Observed at the time of writing, over 43 counted frames:
/// 85 compound blocks, 1969/344/47/528 NEAREST/NEAR/ZERO/NEWMV, 175 intra
/// blocks inside inter frames, 1364 switchable-filter choices across two
/// filters, 469 non-zero motion vectors, 619 high-precision MV bits.
///
/// It also records the corpus's real weakness rather than hiding it:
/// `p9still`, `p9basic` and `p9hp` are near-static 76x42 encodes whose inter
/// frames are almost entirely skip (14, 31 and 42 inter blocks in eight
/// frames, every one NEARESTMV with a zero motion vector, no compound, no MV
/// reads at all). Everything above the trivial case comes from `p9tc`,
/// `p9alt` and `compound`.
#[test]
fn the_corpus_genuinely_exercises_the_inter_machinery() {
    let cov = measure_corpus_coverage();

    // Deliberately slack: which streams count at all depends on their
    // `error_resilient` / `frame_parallel` flags, so a fixture swap that
    // flipped one stream would trip a tight floor for a reason that has
    // nothing to do with what this test is about. 43 observed.
    assert!(
        cov.counted_frames >= 30,
        "the adapting streams must contribute counted frames (~43 observed), got {}",
        cov.counted_frames
    );

    assert!(
        cov.single_vs_compound[1] >= 50 && cov.comp_ref_reads >= 50,
        "compound (two-reference) prediction must really be decoded, not \
         merely permitted by the frame headers: {} compound blocks, {} \
         comp_ref reads",
        cov.single_vs_compound[1],
        cov.comp_ref_reads
    );
    assert!(
        cov.single_vs_compound[0] >= 1000,
        "...alongside plenty of single-reference blocks: {}",
        cov.single_vs_compound[0]
    );

    for (mode, floor) in cov.inter_modes.iter().zip([1000u32, 200, 25, 300]) {
        assert!(
            *mode >= floor,
            "every inter mode must be exercised (NEAREST/NEAR/ZERO/NEW): {:?}",
            cov.inter_modes
        );
    }

    assert!(
        cov.intra_vs_inter[0] >= 100 && cov.intra_vs_inter[1] >= 2000,
        "inter frames must contain both intra and inter blocks: {:?}",
        cov.intra_vs_inter
    );

    assert!(
        cov.switchable_filters[0] >= 1000 && cov.switchable_filters[1] >= 25,
        "SWITCHABLE frames must really choose between filters, not always \
         land on EIGHTTAP: {:?}",
        cov.switchable_filters
    );

    let nonzero_mvs: u32 = cov.mv_joints[1..].iter().sum();
    assert!(
        nonzero_mvs >= 300
            && cov.mv_joints[1] >= 50
            && cov.mv_joints[2] >= 50
            && cov.mv_joints[3] >= 50,
        "motion vectors must be non-zero in both components, separately and \
         together: {:?}",
        cov.mv_joints
    );
    assert!(
        cov.mv_hp_bits >= 300,
        "the 1/8-pel high-precision motion-vector bit must really be read: {}",
        cov.mv_hp_bits
    );
}
