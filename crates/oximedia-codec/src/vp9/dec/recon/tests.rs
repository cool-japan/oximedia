//! Tests for the VP9 reconstruction driver ([`super`]).
//!
//! Split out of `recon.rs` to keep that file inside the 2000-line limit —
//! the same shape `modeinfo` and `interpred` already use.

use super::*;
use crate::vp9::dec::counts::{COEFF_CONTEXTS, COEF_BANDS, EOB_MODEL_TOKEN};

/// Real libvpx-vp9 key frame, 76x42, crf 24 — the same fixture the
/// bit-exact tests in `../mod.rs` use.
const KEYFRAME_76X42: &[u8] = include_bytes!("../testdata/kf76x42.frame0.bin");

fn keyframe_header() -> UncompressedHeader {
    UncompressedHeader::parse(KEYFRAME_76X42).expect("key frame header parses")
}

/// Counting must be observably free of side effects: the *same* frame
/// decoded with counters attached and with them detached has to produce
/// identical pixels.
///
/// This is not implied by the bit-exact fixture tests in `../mod.rs`:
/// every `kf*` fixture codes `frame_parallel_decoding_mode == 1`, so the
/// decoder detaches the counters for all of them and those tests never
/// run the counting path at all. Here both arms are run explicitly.
#[test]
fn counting_does_not_change_the_decode() {
    let hdr = keyframe_header();

    let mut probs_off = FrameProbs::defaults();
    let uncounted =
        decode_frame(&hdr, KEYFRAME_76X42, &mut probs_off, None, None).expect("key frame decodes");

    let mut probs_on = FrameProbs::defaults();
    let mut counts = FrameCounts::new();
    let counted = decode_frame(&hdr, KEYFRAME_76X42, &mut probs_on, Some(&mut counts), None)
        .expect("key frame decodes with counting on");

    for (i, (a, b)) in counted
        .planes
        .iter()
        .zip(uncounted.planes.iter())
        .enumerate()
    {
        assert_eq!(a.data, b.data, "plane {i} changed when counting was on");
    }
    assert!(
        probs_on == probs_off,
        "counting must not touch the probability context either"
    );
    assert!(
        counts.total_coef_tokens() > 0 && counts.total_eob_branches() > 0,
        "...and the counters must not be vacuously empty: this frame \
         codes coefficients"
    );
}

/// Invariants that hold for any real frame, and that a mis-placed
/// increment would break.
#[test]
fn counters_are_internally_consistent_on_a_real_frame() {
    let hdr = keyframe_header();
    let mut probs = FrameProbs::defaults();
    let mut counts = FrameCounts::new();
    decode_frame(&hdr, KEYFRAME_76X42, &mut probs, Some(&mut counts), None)
        .expect("key frame decodes");

    let mut branches = 0u64;
    let mut eob_tokens = 0u64;
    for tx in 0..4 {
        for plane in 0..2 {
            for band in 0..COEF_BANDS {
                for ctx in 0..COEFF_CONTEXTS {
                    // `ref` is 0 on the intra path, so the inter half of
                    // every counter must be untouched.
                    assert_eq!(
                        counts.eob_branch[tx][plane][1][band][ctx], 0,
                        "intra decode must not write the inter counters"
                    );
                    assert_eq!(counts.coef[tx][plane][1][band][ctx], [0; 4]);

                    let read = counts.eob_branch[tx][plane][0][band][ctx];
                    let terminated = counts.coef[tx][plane][0][band][ctx][EOB_MODEL_TOKEN];
                    assert!(
                        terminated <= read,
                        "tx {tx} plane {plane} band {band} ctx {ctx}: the EOB \
                         token was counted {terminated} times but the branch \
                         was only read {read} times — `eob_branch` must be \
                         incremented before the read"
                    );
                    branches += u64::from(read);
                    eob_tokens += u64::from(terminated);
                }
            }
        }
    }
    assert!(branches > 0 && eob_tokens > 0);
    assert_eq!(branches, counts.total_eob_branches());

    // The intra path never reads an inter-only symbol, so those counters
    // stay zero while the ones it does read do not.
    assert_eq!(counts.inter_mode, [[0; 4]; 7]);
    assert_eq!(counts.intra_inter, [[0; 2]; 4]);
    assert_eq!(counts.mv, super::super::counts::NmvCounts::default());
    assert!(
        counts.partition.iter().flatten().sum::<u32>() > 0,
        "read_partition counts on every partition decision"
    );
    assert!(
        counts.skip.iter().flatten().sum::<u32>() > 0,
        "read_skip counts whenever it actually reads the flag"
    );
}

/// The working probability context is a real input to the decode, not a
/// threaded-through parameter the reconstruction ignores.
///
/// No fixture proves this on its own: a key frame always starts from the
/// defaults, and the intra-only fixture codes `reset_frame_context == 2`
/// with `frame_context_idx == 0`, so it resolves to the defaults too.
/// Perturbing the incoming probabilities is what makes the dependency
/// visible — an arithmetic decoder fed different probabilities cannot
/// reproduce the same picture from the same bits.
///
/// Each perturbation is applied to a probability group the frame is
/// certain to *read*: a single arbitrary coefficient node is not enough,
/// because a node that no block in this particular frame happens to
/// reach leaves the bitstream untouched (`coef[0][0][0][1][0][0]`, the
/// band-1 EOB node for 4x4 luma, is one such — a block whose first
/// coefficient is a zero token enters the zero run and never reads it).
#[test]
fn the_working_probability_context_is_load_bearing() {
    let hdr = keyframe_header();

    let mut baseline_probs = FrameProbs::defaults();
    let baseline = decode_frame(&hdr, KEYFRAME_76X42, &mut baseline_probs, None, None)
        .expect("key frame decodes from the defaults");

    /// Applies one perturbation and asserts it is observable.
    fn assert_perturbation_is_observable(
        hdr: &UncompressedHeader,
        baseline: &DecodedFrame,
        label: &str,
        perturb: impl FnOnce(&mut FrameProbs),
    ) {
        let mut probs = FrameProbs::defaults();
        perturb(&mut probs);
        assert!(
            probs != FrameProbs::defaults(),
            "{label}: no-op perturbation"
        );
        // An `Err` is equally good proof: the perturbed context
        // desynchronised the bitstream badly enough to fail the
        // tile-overrun check, which it could only do by being read.
        if let Ok(perturbed) = decode_frame(hdr, KEYFRAME_76X42, &mut probs, None, None) {
            assert_ne!(
                perturbed.planes[0].data, baseline.planes[0].data,
                "{label}: decoding the same bits against different \
                 probabilities must not produce the same picture — the \
                 context parameter would then be dead"
            );
        }
    }

    // The skip flag is read for every block that is not forced skip by
    // segmentation, so this is read whatever the frame's content is.
    assert_perturbation_is_observable(&hdr, &baseline, "skip", |p| p.skip = [8; 3]);

    // Every coefficient probability for luma intra blocks, across all
    // four transform sizes: a frame that codes any residual at all reads
    // some of these.
    assert_perturbation_is_observable(&hdr, &baseline, "coef", |p| {
        for tx in &mut p.coef {
            for band in &mut tx[0][0] {
                for ctx in band {
                    *ctx = [8, 8, 8];
                }
            }
        }
    });
}

/// The compressed header's `diff_update_prob` passes are applied to the
/// caller's context, so it comes back holding what the frame decoded
/// against — which is what backward adaptation and the conditional save
/// consume.
#[test]
fn the_compressed_header_updates_are_visible_to_the_caller() {
    let hdr = keyframe_header();
    let mut probs = FrameProbs::defaults();
    decode_frame(&hdr, KEYFRAME_76X42, &mut probs, None, None).expect("key frame decodes");
    assert!(
        probs != FrameProbs::defaults(),
        "this fixture's compressed header updates probabilities, and the \
         updates must land in the caller's context"
    );
}

// ---------------------------------------------------------------------------
// Inter-frame smoke drive (VP9 P11)
// ---------------------------------------------------------------------------

/// The 8 coded frames of `p9still`, in bitstream order — a 76x42 encode of a
/// frozen source, so every inter frame is near-all-skip / `ZEROMV`. See
/// `testdata/RECIPE.md` for provenance and
/// `super::super::inter_fixture_tests` for the rest of the fixture set.
const P9STILL_FRAMES: [&[u8]; 8] = [
    include_bytes!("../testdata/p9still.frame0.bin"),
    include_bytes!("../testdata/p9still.frame1.bin"),
    include_bytes!("../testdata/p9still.frame2.bin"),
    include_bytes!("../testdata/p9still.frame3.bin"),
    include_bytes!("../testdata/p9still.frame4.bin"),
    include_bytes!("../testdata/p9still.frame5.bin"),
    include_bytes!("../testdata/p9still.frame6.bin"),
    include_bytes!("../testdata/p9still.frame7.bin"),
];

/// libvpx's reference reconstruction of every shown `p9still` frame, planar
/// YUV 4:2:0, in output order.
const P9STILL_REF: &[u8] = include_bytes!("../testdata/p9still.ref.yuv");

/// Drives a whole coded sequence through the real cross-frame decoder state
/// and returns each decoded frame's three cropped planes, in decode order.
///
/// This is a *driver*, not a second verification harness:
/// `super::super::testutil::assert_bit_exact_sequence` stays the fixture
/// gate, and generalizing it to keep a decoded-picture buffer belongs to the
/// package that owns the inter fixtures. What is needed here is the ability
/// to look at one frame's pixels and say precisely where they first diverge.
fn decode_sequence(frames: &[&[u8]], w: usize, h: usize) -> Vec<(bool, [Vec<u8>; 3])> {
    let mut state = super::super::Vp9DecState::new();
    let mut out = Vec::new();
    for (packet, &payload) in frames.iter().enumerate() {
        // A packet may be a superframe (a hidden ALTREF plus the frame that
        // shows it); `Superframe::parse` rejects a coincidental marker-shape
        // match on a genuine single frame, which is then decoded as one.
        let subs = match crate::vp9::superframe::Superframe::parse(payload) {
            Ok(sf) => sf.frames,
            Err(_) => vec![payload.to_vec()],
        };
        for (sub, payload) in subs.iter().enumerate() {
            let payload: &[u8] = payload;
            let idx = format!("{packet}.{sub}");
            let mut hdr = UncompressedHeader::parse_with_ref_sizes(payload, &state.ref_sizes())
                .unwrap_or_else(|e| panic!("frame {idx}: header parse: {e}"));
            let cw = w.div_ceil(2);
            let chh = h.div_ceil(2);
            let dims = [(w, h), (cw, chh), (cw, chh)];
            let crop = |plane: &PlaneBuf, (pw, ph): (usize, usize)| {
                let mut v = Vec::with_capacity(pw * ph);
                for row in 0..ph {
                    let start = row * plane.stride;
                    v.extend_from_slice(&plane.data[start..start + pw]);
                }
                v
            };
            if hdr.show_existing_frame {
                // A redisplay decodes nothing and updates nothing: it re-emits
                // the named slot's pixels (spec 7.2, libvpx `vp9_decoder.c`'s
                // early return before `swap_frame_buffers`).
                let slot = state
                    .dpb
                    .get(hdr.frame_to_show as usize)
                    .and_then(Option::as_ref)
                    .unwrap_or_else(|| {
                        panic!("frame {idx}: show_existing_frame names an empty slot")
                    });
                out.push((
                    true,
                    [
                        crop(&slot.planes[0], dims[0]),
                        crop(&slot.planes[1], dims[1]),
                        crop(&slot.planes[2], dims[2]),
                    ],
                ));
                continue;
            }
            assert_eq!(
                (hdr.width as usize, hdr.height as usize),
                (w, h),
                "frame {idx}: unexpected frame size"
            );
            state
                .begin_frame(&mut hdr)
                .unwrap_or_else(|e| panic!("frame {idx}: begin_frame: {e}"));
            let decoded = if hdr.is_intra_only() {
                super::super::decode_intra_frame_with_state(&mut state, &hdr, payload)
            } else {
                super::super::decode_inter_frame_with_state(&mut state, &hdr, payload)
            }
            .unwrap_or_else(|e| panic!("frame {idx}: decode: {e}"));

            let planes = [
                crop(&decoded.planes[0], dims[0]),
                crop(&decoded.planes[1], dims[1]),
                crop(&decoded.planes[2], dims[2]),
            ];
            out.push((hdr.show_frame, planes));

            let slot = super::super::Vp9RefSlot::from_decoded_frame(
                decoded,
                hdr.intra_only,
                hdr.show_frame,
            )
            .unwrap_or_else(|e| panic!("frame {idx}: reference slot: {e}"));
            state
                .finish_frame(&hdr, slot)
                .unwrap_or_else(|e| panic!("frame {idx}: finish_frame: {e}"));
        }
    }
    out
}

/// Compares one decoded plane against its reference and reports the first
/// divergence precisely enough to locate the responsible mode-info block.
fn first_divergence(dec: &[u8], r#ref: &[u8], w: usize, name: &str) -> Option<String> {
    let mut bad = 0usize;
    let mut first = None;
    for (i, (&d, &e)) in dec.iter().zip(r#ref.iter()).enumerate() {
        if d != e {
            bad += 1;
            if first.is_none() {
                let (x, y) = (i % w, i / w);
                first = Some(format!(
                    "{name}: first mismatch at ({x},{y}) mi=({},{}) dec={d} ref={e}",
                    x / 8,
                    y / 8
                ));
            }
        }
    }
    first.map(|f| format!("{f}; {bad}/{} pixels differ", dec.len()))
}

/// P11's gate: every frame of the `p9still` sequence — the key frame and all
/// seven inter frames — reconstructs byte-identically to libvpx.
///
/// `p9still` is the gentlest inter fixture in the set (a frozen source, so
/// the inter frames are near-all-skip / `ZEROMV` against a single LAST
/// reference), which is what makes it the right gate for the *driver*: a
/// failure here is structural — the reconstruction order, the mode-info
/// branch, the coefficient `ref` index, the mode-info replication — rather
/// than a corner of motion compensation. The rest of the fixture set (and
/// the harness that verifies a decoded-picture buffer across a whole
/// sequence) belongs to the package that owns the inter fixtures.
///
/// Frames 2..7 are not merely "more of the same": each decodes against the
/// probability context its predecessor *adapted*, so they also pin backward
/// adaptation of the mode, motion-vector and `uv_mode` probability groups —
/// none of which an intra-only sequence can reach.
#[test]
fn p9still_every_frame_is_bit_exact_vs_libvpx() {
    let (w, h) = (76usize, 42usize);
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let frame_len = w * h + 2 * cw * ch;
    assert_eq!(
        P9STILL_REF.len(),
        8 * frame_len,
        "p9still shows all 8 coded frames"
    );

    let decoded = decode_sequence(&P9STILL_FRAMES, w, h);
    assert_eq!(decoded.len(), 8, "p9still holds 8 coded frames");
    assert!(
        decoded.iter().all(|(shown, _)| *shown),
        "every p9still frame is shown"
    );

    let mut problems = Vec::new();
    for (idx, (_, planes)) in decoded.iter().enumerate() {
        let base = idx * frame_len;
        let r#ref = &P9STILL_REF[base..base + frame_len];
        for (name, dec, r#ref, pw) in [
            ("Y", &planes[0], &r#ref[..w * h], w),
            ("U", &planes[1], &r#ref[w * h..w * h + cw * ch], cw),
            ("V", &planes[2], &r#ref[w * h + cw * ch..], cw),
        ] {
            if let Some(msg) = first_divergence(dec, r#ref, pw, name) {
                problems.push(format!("frame {idx} {msg}"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "p9still differs from libvpx:\n  {}",
        problems.join("\n  ")
    );
}
