//! Shared bit-exact verification harness for `dec/*` test modules
//! (`cfg(test)` only).
//!
//! [`assert_bit_exact_sequence`] walks a real, possibly multi-frame VP9 coded
//! sequence the way an application does: it builds a [`Vp9Decoder`] and feeds
//! every committed `<name>.frameN.bin` through the **public**
//! [`VideoDecoder::send_packet`] / [`VideoDecoder::receive_frame`] API, then
//! compares every frame the decoder outputs, plane by plane, against the
//! matching slice of a concatenated reference YUV (every *shown* frame, in
//! output order, exactly how the `<name>.ref.yuv` fixtures in `testdata/` are
//! generated -- see `testdata/RECIPE.md`).
//!
//! Nothing is stubbed out or skipped any more: key frames, intra-only frames,
//! **inter** frames and `show_existing_frame` redisplays are all decoded by
//! the real decoder and all pixel-verified, because the decoder now keeps a
//! real decoded-picture buffer.
//!
//! What makes "it passed" mean something, concretely: the emitted-frame count
//! is asserted against each fixture's real shown-frame total, the consumed
//! reference bytes must account for the golden exactly (so a short walk fails
//! loudly rather than passing on a prefix), and
//! `the_gate_is_not_vacuous_a_planted_mismatch_is_reported` corrupts one byte
//! of one plane of the *last* frame of a sequence and requires this harness to
//! name that pixel.
//!
//! Everything the harness used to do by hand -- superframe unpacking,
//! `frame_size_with_refs` resolution against a simulated per-slot size table,
//! `refresh_frame_flags` bookkeeping, redisplay of a stored slot -- is now
//! production code inside [`Vp9Decoder`], which is the point: the fixtures
//! test the shipped decoder, not a test-only reimplementation of it.
//!
//! Two fixtures are expected to be *refused* rather than decoded, at a
//! specific packet, with a specific [`CodecError::UnsupportedFeature`]:
//! `p9seg` (inter-frame segmentation) and `scaled` (reference scaling). They
//! use [`run_sequence`] directly and assert the refusal's position and text --
//! see `super::inter_fixture_tests`.

use crate::error::CodecError;
use crate::traits::{DecoderConfig, VideoDecoder};
use crate::vp9::Vp9Decoder;

/// Splits a raw planar YUV420 reference dump into (Y, U, V).
pub(super) fn split_yuv(data: &[u8], w: usize, h: usize) -> (&[u8], &[u8], &[u8]) {
    let cw = w.div_ceil(2);
    let ch = h.div_ceil(2);
    let ysz = w * h;
    let csz = cw * ch;
    assert_eq!(data.len(), ysz + 2 * csz, "reference YUV size");
    (
        &data[..ysz],
        &data[ysz..ysz + csz],
        &data[ysz + csz..ysz + 2 * csz],
    )
}

/// One plane's comparison result: how far the decode is from the reference,
/// and precisely where it first goes wrong.
pub(super) struct PlaneDiff {
    /// Number of differing pixels.
    pub(super) bad: usize,
    /// Sum of squared error over the differing pixels.
    pub(super) sse: u64,
    /// `(col, row, got, want)` of the first mismatch in raster order.
    pub(super) first: Option<(usize, usize, u8, u8)>,
}

/// Compares one decoded plane (row stride `stride`) against a cropped
/// reference plane.
///
/// The first mismatch is reported as `(col, row, got, want)` rather than only
/// counted: a bit-exactness failure is located by its *first* divergence --
/// everything after it is downstream corruption -- and the mode-info cell
/// `(col >> 3, row >> 3)` is what a `layout.txt` dump and a libvpx trace are
/// indexed by.
pub(super) fn diff_plane(dec: &[u8], stride: usize, r#ref: &[u8], w: usize, h: usize) -> PlaneDiff {
    let mut diff = PlaneDiff {
        bad: 0,
        sse: 0,
        first: None,
    };
    for y in 0..h {
        for x in 0..w {
            let d = dec[y * stride + x];
            let e = r#ref[y * w + x];
            if d != e {
                if diff.first.is_none() {
                    diff.first = Some((x, y, d, e));
                }
                diff.bad += 1;
                let delta = i64::from(d) - i64::from(e);
                diff.sse += (delta * delta) as u64;
            }
        }
    }
    diff
}

/// Outcome of [`assert_bit_exact_sequence`]: how many coded packets the
/// sequence held, how many frames the decoder actually output (which is how
/// many `ref_yuv` slices were consumed), and how many of those were
/// pixel-verified.
pub(super) struct SequenceCheck {
    /// Number of entries in the `frames` slice passed in (coded packets; a
    /// superframe is one entry even though it unpacks to more than one coded
    /// frame).
    pub(super) coded_frames: usize,
    /// Number of frames the decoder emitted across the whole sequence,
    /// including `show_existing_frame` redisplays -- i.e. the number of
    /// frames `ref_yuv` must hold.
    ///
    /// This is the load-bearing count: asserting it against a fixture's real
    /// shown-frame total is what catches a decoder that drops a frame, emits
    /// a hidden one, or double-emits a redisplay.
    pub(super) shown_frames: usize,
    /// Number of emitted frames this call pixel-verified against `ref_yuv`.
    ///
    /// Now **necessarily equal to `shown_frames`** -- every emitted frame is
    /// compared, there is no longer any class the harness declines to check
    /// -- so asserting the two match is a restatement, not an independent
    /// check. It is kept because it used to differ (the pre-inter-decode
    /// harness pixel-checked only key/intra-only frames and reported the
    /// shortfall here), and a future deferral that reintroduced one would
    /// have to change this field rather than quietly weaken the gate. The
    /// real guard against a vacuous comparison is
    /// `the_gate_is_not_vacuous_a_planted_mismatch_is_reported`, plus
    /// [`SequenceRun::consumed_ref_bytes`] having to account for the golden
    /// exactly.
    pub(super) verified_frames: usize,
}

/// Raw result of driving a coded sequence through the public decoder, with no
/// assertions applied -- what [`assert_bit_exact_sequence`] checks and what
/// the two deliberately-refused fixtures inspect instead.
pub(super) struct SequenceRun {
    /// See [`SequenceCheck`].
    pub(super) check: SequenceCheck,
    /// Bytes of `ref_yuv` consumed by the emitted frames.
    pub(super) consumed_ref_bytes: usize,
    /// The first packet whose `send_packet` failed, with its error. `None`
    /// for a sequence that decoded end to end.
    pub(super) error: Option<(usize, CodecError)>,
    /// One line per mismatching plane, each naming the first divergence.
    pub(super) problems: Vec<String>,
}

/// Drives a whole coded sequence through the public [`Vp9Decoder`] API and
/// records what came out, without asserting anything.
///
/// `frames[i]` is one coded packet (a superframe counts as one entry --
/// unpacking it is `send_packet`'s job, not this harness's). `dims[i]` is the
/// display size every frame that packet emits must report; a fixture with a
/// mid-sequence resolution change simply varies it (`scaled`).
/// `ref_yuv` is the concatenation of every emitted frame's planar YUV 4:2:0
/// in output order, exactly as the committed `<name>.ref.yuv`.
///
/// Decoding stops at the first `send_packet` error, which is recorded in
/// [`SequenceRun::error`] rather than panicking: two fixtures are expected to
/// be refused at a known packet, and a refusal at any *other* packet has to be
/// reportable with its position.
pub(super) fn run_sequence(
    frames: &[&[u8]],
    ref_yuv: &[u8],
    dims: &[(u32, u32)],
    label: &str,
) -> SequenceRun {
    assert_eq!(
        frames.len(),
        dims.len(),
        "{label}: one dims entry is required per coded packet"
    );

    let mut decoder = Vp9Decoder::new(DecoderConfig::default()).expect("VP9 decoder constructs");
    let mut cursor = 0usize;
    let mut shown_frames = 0usize;
    let mut verified_frames = 0usize;
    let mut problems = Vec::new();
    let mut error = None;

    for (idx, (&payload, &(want_w, want_h))) in frames.iter().zip(dims.iter()).enumerate() {
        if let Err(e) = decoder.send_packet(payload, idx as i64) {
            error = Some((idx, e));
            break;
        }

        while let Some(frame) = decoder
            .receive_frame()
            .unwrap_or_else(|e| panic!("{label}.frame{idx}: receive_frame: {e}"))
        {
            let tag = format!("{label}.frame{idx}.out{shown_frames}");
            assert_eq!(
                (frame.width, frame.height),
                (want_w, want_h),
                "{tag}: output frame size"
            );
            let (w, h) = (frame.width as usize, frame.height as usize);
            let cw = w.div_ceil(2);
            let ch = h.div_ceil(2);
            let frame_len = w * h + 2 * cw * ch;
            assert!(
                cursor + frame_len <= ref_yuv.len(),
                "{tag}: emitted frame runs past the end of the reference \
                 ({cursor} + {frame_len} > {})",
                ref_yuv.len()
            );
            let (ry, ru, rv) = split_yuv(&ref_yuv[cursor..cursor + frame_len], w, h);
            cursor += frame_len;
            shown_frames += 1;

            assert_eq!(frame.planes.len(), 3, "{tag}: plane count");
            for (name, plane, r#ref, pw, ph) in [
                ("Y", &frame.planes[0], ry, w, h),
                ("U", &frame.planes[1], ru, cw, ch),
                ("V", &frame.planes[2], rv, cw, ch),
            ] {
                let diff = diff_plane(&plane.data, plane.stride, r#ref, pw, ph);
                if let Some((x, y, got, want)) = diff.first {
                    problems.push(format!(
                        "{tag}.{name}: first mismatch at ({x},{y}) mi=({},{}) \
                         got={got} want={want}; {} of {} pixels differ (sse {})",
                        x / 8,
                        y / 8,
                        diff.bad,
                        pw * ph,
                        diff.sse
                    ));
                }
            }
            verified_frames += 1;
        }
    }

    SequenceRun {
        check: SequenceCheck {
            coded_frames: frames.len(),
            shown_frames,
            verified_frames,
        },
        consumed_ref_bytes: cursor,
        error,
        problems,
    }
}

/// Asserts a [`run_sequence`] result is a clean, complete, bit-exact decode.
fn assert_run_is_clean(run: SequenceRun, ref_yuv: &[u8], label: &str) -> SequenceCheck {
    if let Some((idx, e)) = run.error {
        panic!("{label}: packet {idx} failed to decode: {e}");
    }
    assert!(
        run.problems.is_empty(),
        "{label}: decode differs from the libvpx/ffmpeg reference:\n  {}",
        run.problems.join("\n  ")
    );
    assert_eq!(
        run.consumed_ref_bytes,
        ref_yuv.len(),
        "{label}: consumed {} reference bytes across {} emitted frames, but \
         the golden holds {} bytes",
        run.consumed_ref_bytes,
        run.check.shown_frames,
        ref_yuv.len()
    );
    assert_eq!(
        run.check.verified_frames, run.check.shown_frames,
        "{label}: every emitted frame must be pixel-verified"
    );
    run.check
}

/// The fixture gate: every frame of a real coded sequence, decoded through the
/// public decoder API, must be byte-identical to libvpx's own reconstruction.
///
/// `frames` is the sequence of raw per-packet payloads in bitstream order,
/// exactly as the committed `<name>.frameN.bin` files. `ref_yuv` is the
/// concatenation of every shown frame's planar YUV 4:2:0, in output order,
/// exactly as the committed `<name>.ref.yuv`. Every frame in the sequence
/// must share one `w x h`; a fixture with a mid-sequence resolution change
/// (`scaled`) calls [`run_sequence`] with a real per-packet `dims` table
/// instead.
///
/// Panics on any decode error, pixel mismatch or frame-accounting mismatch.
pub(super) fn assert_bit_exact_sequence(
    frames: &[&[u8]],
    ref_yuv: &[u8],
    w: usize,
    h: usize,
    label: &str,
) -> SequenceCheck {
    let dims = vec![(w as u32, h as u32); frames.len()];
    let run = run_sequence(frames, ref_yuv, &dims, label);
    assert_run_is_clean(run, ref_yuv, label)
}
