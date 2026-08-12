//! Tests for [`super`] — inter mode info and motion-vector decode.
//!
//! Every case is driven by [`BoolWriter`], the `#[cfg(test)]` port of
//! libvpx's bool *encoder*: a test encodes a chosen symbol sequence, decodes
//! it back through the module under test, and asserts the resulting
//! [`MiInfo`] field by field. The writer is pinned to libvpx's arithmetic
//! coder by the round-trip tests in [`super::super::boolenc`], which in turn
//! rest on the four bit-exact key-frame fixtures — so "encode then decode"
//! here is not a self-consistency check of two halves of the same mistake.
//!
//! Bit consumption is asserted the only way the decoder allows: every
//! sequence is followed by the [`SENTINEL`] literal, which must read back
//! intact. A single misplaced, extra or missing bool desynchronises the range
//! coder and destroys it.
//!
//! Note on test vectors: with `allow_high_precision_mv` off, the coded
//! magnitude of a motion-vector component is always **even** — the
//! high-precision bit defaults to 1 and the assembly is
//! `mag = base + ((d << 3) | (fr << 1) | hp) + 1`. Several fixtures below
//! pick even differences for exactly that reason.

use super::super::booldec::BoolReader;
use super::super::boolenc::BoolWriter;
use super::super::counts::FrameCounts;
use super::super::hdr::{FrameProbs, ReferenceMode, TxMode, SWITCHABLE};
use super::super::mvref::{find_mv_refs, get_mode_context, lower_mv_precision, MiGrid, TileBounds};
use super::super::predctx::{
    get_intra_inter_context, get_pred_context_comp_ref_p, get_pred_context_single_ref_p1,
    get_pred_context_single_ref_p2, get_pred_context_switchable_interp, get_reference_mode_context,
    get_skip_context, get_tx_size_context, CompRefState, SWITCHABLE_FILTERS,
};
use super::super::recon::MiInfo;
use super::super::refs::{ALTREF_FRAME, GOLDEN_FRAME, INTRA_FRAME, LAST_FRAME, NONE_FRAME};
use super::super::tables;
use super::super::tables_inter::{
    NmvComponent, NmvContext, DEFAULT_NMV_CONTEXT, DEFAULT_UV_MODE_PROBS, INTER_MODE_TREE,
    MV_CLASS0_TREE, MV_CLASS_TREE, MV_FP_TREE, MV_JOINT_TREE, SIZE_GROUP_LOOKUP,
    SWITCHABLE_INTERP_TREE,
};
use super::*;
use crate::vp9::mv::MotionVector;
use crate::vp9::uncompressed::SegmentationHeader;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Trailing literal every encoded sequence ends with; reading it back proves
/// the decoder consumed exactly the bits the encoder wrote.
const SENTINEL: u32 = 0xA5;

/// The default motion-vector probabilities, as a `static` so a test can name
/// one component by reference.
static NMV: NmvContext = DEFAULT_NMV_CONTEXT;

/// A mode-info grid backed by a flat `Vec`, for candidate scans.
struct TestGrid {
    rows: usize,
    cols: usize,
    mi: Vec<MiInfo>,
}

impl TestGrid {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            mi: vec![MiInfo::default(); rows * cols],
        }
    }

    fn set(&mut self, row: usize, col: usize, mi: MiInfo) -> &mut Self {
        let cols = self.cols;
        self.mi[row * cols + col] = mi;
        self
    }
}

impl MiGrid for TestGrid {
    fn mi_rows(&self) -> usize {
        self.rows
    }
    fn mi_cols(&self) -> usize {
        self.cols
    }
    fn mi_at(&self, row: usize, col: usize) -> MiInfo {
        self.mi[row * self.cols + col]
    }
}

/// An inter neighbour, as the candidate scan and the prediction contexts see
/// one.
fn inter_mi(mode: u8, ref_frame: i8, mv: MotionVector) -> MiInfo {
    MiInfo {
        sb_type: 6, // BLOCK_16X16
        mode,
        is_inter: true,
        ref_frame: [ref_frame, NONE_FRAME],
        mv: [mv, MotionVector::zero()],
        ..MiInfo::default()
    }
}

/// Segmentation off — the only configuration the entry point accepts.
fn seg_off() -> SegmentationHeader {
    SegmentationHeader::default()
}

/// The usual sign bias: only `ALTREF` points the other way, which makes
/// `comp_fixed_ref` = `ALTREF` and `comp_var_ref` = `[LAST, GOLDEN]`.
const ALTREF_BIAS: [bool; 4] = [false, false, false, true];

/// Frame configuration: `TX_MODE_SELECT`, single-reference, a fixed
/// (non-switchable) filter and no high-precision motion vectors.
fn frame_cfg(seg: &SegmentationHeader) -> InterFrameCfg<'_> {
    InterFrameCfg {
        tx_mode: TxMode::Select,
        reference_mode: ReferenceMode::Single,
        interp_filter: 0,
        allow_high_precision_mv: false,
        comp: CompRefState::from_sign_bias(&ALTREF_BIAS),
        seg,
        tile: TileBounds::full_width(8),
        prev_mvs: None,
    }
}

/// Decodes one block and checks the sentinel, i.e. that the decode consumed
/// exactly the encoded bits.
fn decode_block(
    data: &[u8],
    grid: &TestGrid,
    cfg: &InterFrameCfg<'_>,
    probs: &FrameProbs,
    counts: Option<&mut FrameCounts>,
    nb: Neighbours<'_>,
    pos: BlockPos,
) -> MiInfo {
    let mut r = BoolReader::new(data).expect("marker bit");
    let mi = read_inter_frame_mode_info(&mut r, grid, cfg, probs, counts, nb, pos)
        .expect("the fixture is a well-formed block");
    assert_eq!(
        r.read_literal(8),
        SENTINEL,
        "decode consumed the wrong number of bits"
    );
    assert!(!r.has_error(), "decode overran the buffer");
    mi
}

/// A block at the top-left corner of the frame, i.e. with no neighbours.
fn corner(bsize: u8) -> BlockPos {
    BlockPos {
        mi_row: 0,
        mi_col: 0,
        bsize,
    }
}

// ---------------------------------------------------------------------------
// Symbol writers — the inverse of each ported reader, in the same order
// ---------------------------------------------------------------------------

fn write_skip(w: &mut BoolWriter, probs: &FrameProbs, ctx: usize, skip: bool) {
    w.write_bool(probs.skip[ctx], skip);
}

fn write_is_inter(w: &mut BoolWriter, probs: &FrameProbs, ctx: usize, is_inter: bool) {
    w.write_bool(probs.intra_inter[ctx], is_inter);
}

fn write_tx_size(w: &mut BoolWriter, probs: &FrameProbs, ctx: usize, max_tx: u8, tx: u8) {
    let p: [u8; 3] = match max_tx {
        1 => [probs.tx8[ctx][0], 0, 0],
        2 => [probs.tx16[ctx][0], probs.tx16[ctx][1], 0],
        _ => probs.tx32[ctx],
    };
    w.write_bool(p[0], tx != 0);
    if tx != 0 && max_tx >= 2 {
        w.write_bool(p[1], tx != 1);
        if tx != 1 && max_tx >= 3 {
            w.write_bool(p[2], tx != 2);
        }
    }
}

/// `read_ref_frames`'s single-reference chain.
fn write_single_ref(w: &mut BoolWriter, probs: &FrameProbs, ctx0: usize, ctx1: usize, rf: i8) {
    match rf {
        LAST_FRAME => w.write_bool(probs.single_ref[ctx0][0], false),
        GOLDEN_FRAME => {
            w.write_bool(probs.single_ref[ctx0][0], true);
            w.write_bool(probs.single_ref[ctx1][1], false);
        }
        _ => {
            w.write_bool(probs.single_ref[ctx0][0], true);
            w.write_bool(probs.single_ref[ctx1][1], true);
        }
    }
}

fn write_inter_mode(w: &mut BoolWriter, probs: &FrameProbs, ctx: usize, mode: u8) {
    w.write_tree(&INTER_MODE_TREE, &probs.inter_mode[ctx], mode - NEARESTMV);
}

fn write_switchable(w: &mut BoolWriter, probs: &FrameProbs, ctx: usize, filter: u8) {
    w.write_tree(
        &SWITCHABLE_INTERP_TREE,
        &probs.switchable_interp[ctx],
        filter,
    );
}

fn write_intra_mode_y(w: &mut BoolWriter, probs: &FrameProbs, group: usize, mode: u8) {
    w.write_tree(&tables::INTRA_MODE_TREE, &probs.y_mode[group], mode);
}

fn write_intra_mode_uv(w: &mut BoolWriter, y_mode: u8, uv_mode: u8) {
    w.write_tree(
        &tables::INTRA_MODE_TREE,
        &DEFAULT_UV_MODE_PROBS[y_mode as usize],
        uv_mode,
    );
}

/// The inverse of [`read_mv_component`]: decomposes `value` into the fields
/// libvpx assembles and writes them in the read order.
///
/// # Panics
///
/// When `usehp` is false and the value's high-precision bit is 0 — such a
/// value cannot be coded without high-precision bits, which is exactly what
/// `read_mv_component`'s `hp = 1` default means.
fn write_mv_component(w: &mut BoolWriter, c: &NmvComponent, value: i32, usehp: bool) {
    assert_ne!(value, 0, "a zero component is not codable");
    let sign = value < 0;
    let z = value.abs() - 1;
    let (class, offset) = mv_class(z);
    let d = (offset >> 3) as usize;
    let fr = ((offset >> 1) & 3) as u8;
    let hp = (offset & 1) != 0;
    assert!(
        usehp || hp,
        "value {value} has an even magnitude and needs high-precision bits"
    );

    w.write_bool(c.sign, sign);
    w.write_tree(&MV_CLASS_TREE, &c.classes, class as u8);
    if class == 0 {
        w.write_tree(&MV_CLASS0_TREE, &c.class0, d as u8);
    } else {
        let n = class + CLASS0_BITS - 1;
        for i in 0..n {
            w.write_bool(c.bits[i], (d >> i) & 1 != 0);
        }
    }
    let fp_probs: &[u8] = if class == 0 { &c.class0_fp[d] } else { &c.fp };
    w.write_tree(&MV_FP_TREE, fp_probs, fr);
    if usehp {
        w.write_bool(if class == 0 { c.class0_hp } else { c.hp }, hp);
    }
}

/// The inverse of [`read_mv`]: writes the joint and the present components.
fn write_mv(
    w: &mut BoolWriter,
    ctx: &NmvContext,
    ref_mv: MotionVector,
    target: MotionVector,
    allow_hp: bool,
) {
    let diff = MotionVector::new(
        target.row.wrapping_sub(ref_mv.row),
        target.col.wrapping_sub(ref_mv.col),
    );
    let joint = diff.joint() as u8;
    w.write_tree(&MV_JOINT_TREE, &ctx.joints, joint);
    let usehp = allow_hp && use_mv_hp(ref_mv);
    if mv_joint_vertical(joint) {
        write_mv_component(w, &ctx.comps[0], i32::from(diff.row), usehp);
    }
    if mv_joint_horizontal(joint) {
        write_mv_component(w, &ctx.comps[1], i32::from(diff.col), usehp);
    }
}

// ---------------------------------------------------------------------------
// Frame-level filter mapping
// ---------------------------------------------------------------------------

#[test]
fn frame_interp_filter_applies_the_literal_permutation() {
    // literal_to_filter[] = { EIGHTTAP_SMOOTH, EIGHTTAP, EIGHTTAP_SHARP,
    // BILINEAR } — the first two are swapped relative to the filter
    // numbering, which is the whole reason the mapping exists.
    assert_eq!(frame_interp_filter(0), 1, "literal 0 is EIGHTTAP_SMOOTH");
    assert_eq!(frame_interp_filter(1), 0, "literal 1 is EIGHTTAP");
    assert_eq!(frame_interp_filter(2), 2);
    assert_eq!(frame_interp_filter(3), 3);
}

#[test]
fn frame_interp_filter_passes_switchable_through() {
    assert_eq!(frame_interp_filter(SWITCHABLE), SWITCHABLE);
    assert_eq!(
        frame_interp_filter(frame_interp_filter(SWITCHABLE)),
        SWITCHABLE,
        "idempotent on SWITCHABLE, so a double application cannot turn \
         per-block filtering into a fixed filter"
    );
}

// ---------------------------------------------------------------------------
// Motion-vector component round trips
// ---------------------------------------------------------------------------

/// The smallest and largest magnitude codable in `class`, from
/// `mag = (class ? CLASS0_SIZE << (class + 2) : 0) + ((d << 3) | (fr << 1) | hp) + 1`.
fn class_bounds(class: usize) -> (i32, i32) {
    let base = if class == 0 { 0 } else { 2i32 << (class + 2) };
    let bits = if class == 0 {
        1
    } else {
        class + CLASS0_BITS - 1
    };
    let max_d = (1i32 << bits) - 1;
    (base + 1, base + ((max_d << 3) | 7) + 1)
}

#[test]
fn read_mv_component_round_trips_every_class_and_sign() {
    let comp = &NMV.comps[0];
    for class in 0..=10usize {
        let (lo, hi) = class_bounds(class);
        for mag in [lo, lo + 1, (lo + hi) / 2, hi - 1, hi] {
            for signed in [mag, -mag] {
                let mut w = BoolWriter::new();
                write_mv_component(&mut w, comp, signed, true);
                w.write_literal(SENTINEL, 8);
                let data = w.finish();
                let mut r = BoolReader::new(&data).expect("marker");
                assert_eq!(
                    read_mv_component(&mut r, comp, true),
                    signed,
                    "class {class} value {signed}"
                );
                assert_eq!(
                    r.read_literal(8),
                    SENTINEL,
                    "class {class} value {signed}: wrong bit count"
                );
            }
        }
    }
}

#[test]
fn read_mv_component_spans_the_whole_coded_range() {
    // Class 10's upper bound is 16384, one past MV_UPP: the reader must be
    // able to produce it (the validity check is a separate, later step).
    assert_eq!(class_bounds(0), (1, 16));
    assert_eq!(class_bounds(10), (8193, 16384));
    let comp = &NMV.comps[1];
    let mut w = BoolWriter::new();
    write_mv_component(&mut w, comp, -16384, true);
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");
    assert_eq!(read_mv_component(&mut r, comp, true), -16384);
}

#[test]
fn read_mv_component_defaults_the_high_precision_bit_when_disabled() {
    // With usehp off no hp bool is coded and the decoder must supply 1, so
    // only odd offsets (even magnitudes) are reachable — and the sequence is
    // exactly one bool shorter.
    let comp = &NMV.comps[1];
    let value = 6; // z = 5 -> class 0, offset 5 -> d = 0, fr = 2, hp = 1

    let mut with_hp = BoolWriter::new();
    write_mv_component(&mut with_hp, comp, value, true);
    let n_with = with_hp.total_writes();

    let mut without = BoolWriter::new();
    write_mv_component(&mut without, comp, value, false);
    let n_without = without.total_writes();
    without.write_literal(SENTINEL, 8);
    let data = without.finish();

    assert_eq!(
        n_with,
        n_without + 1,
        "the high-precision bool is the only difference"
    );
    let mut r = BoolReader::new(&data).expect("marker");
    assert_eq!(read_mv_component(&mut r, comp, false), value);
    assert_eq!(r.read_literal(8), SENTINEL);
}

#[test]
fn read_mv_component_reads_integer_bits_least_significant_first() {
    // `d |= vpx_read(r, mvcomp->bits[i]) << i`: bit i of d is coded with
    // probability bits[i]. Class 3 carries three integer bits, and d = 1 is
    // asymmetric under bit reversal — a most-significant-first reader would
    // decode it as 4.
    let comp = &NMV.comps[0];
    let class = 3usize;
    let base = 2i32 << (class + 2);
    let value = base + 9 + 1; // d = 1, fr = 0, hp = 1
    let (c, offset) = mv_class(value - 1);
    assert_eq!(
        (c, offset >> 3),
        (class, 1),
        "the fixture must build class 3 with d = 1"
    );

    let mut w = BoolWriter::new();
    write_mv_component(&mut w, comp, value, true);
    w.write_literal(SENTINEL, 8);
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");
    assert_eq!(read_mv_component(&mut r, comp, true), value);
    assert_eq!(r.read_literal(8), SENTINEL);
}

#[test]
fn mv_class_inverts_the_read_assembly() {
    for class in 0..=10usize {
        let (lo, hi) = class_bounds(class);
        for mag in [lo, lo + 1, hi - 1, hi] {
            let (c, offset) = mv_class(mag - 1);
            assert_eq!(c, class, "magnitude {mag} belongs to class {class}");
            let base = if class == 0 { 0 } else { 2i32 << (class + 2) };
            assert_eq!(base + offset + 1, mag);
        }
    }
}

#[test]
fn mv_class_saturates_at_class_10() {
    // `z >= CLASS0_SIZE * 4096` short-circuits rather than indexing
    // log_in_base_2 out of range.
    assert_eq!(mv_class(2 * 4096), (10, 0));
    assert_eq!(mv_class(i32::MAX).0, 10);
    // The entry just below stays in class 9.
    assert_eq!(mv_class(2 * 4096 - 1).0, 9);
}

// ---------------------------------------------------------------------------
// read_mv
// ---------------------------------------------------------------------------

#[test]
fn read_mv_round_trips_every_joint() {
    let ref_mv = MotionVector::new(4, -6);
    for target in [
        ref_mv,                                              // MV_JOINT_ZERO
        MotionVector::new(ref_mv.row, ref_mv.col + 9),       // HNZVZ
        MotionVector::new(ref_mv.row - 11, ref_mv.col),      // HZVNZ
        MotionVector::new(ref_mv.row + 33, ref_mv.col - 71), // HNZVNZ
    ] {
        let mut w = BoolWriter::new();
        write_mv(&mut w, &NMV, ref_mv, target, true);
        w.write_literal(SENTINEL, 8);
        let data = w.finish();
        let mut r = BoolReader::new(&data).expect("marker");
        assert_eq!(read_mv(&mut r, ref_mv, &NMV, None, true), target);
        assert_eq!(r.read_literal(8), SENTINEL);
    }
}

#[test]
fn read_mv_reads_the_row_component_first() {
    // comps[0] is vertical and comps[1] horizontal. Encoding the two
    // components through each other's probabilities must not decode back.
    let mut ctx = DEFAULT_NMV_CONTEXT;
    ctx.comps[1].sign = 8;
    let ref_mv = MotionVector::zero();
    let target = MotionVector::new(5, -7);

    let mut good = BoolWriter::new();
    write_mv(&mut good, &ctx, ref_mv, target, true);
    good.write_literal(SENTINEL, 8);
    let data = good.finish();
    let mut r = BoolReader::new(&data).expect("marker");
    assert_eq!(read_mv(&mut r, ref_mv, &ctx, None, true), target);
    assert_eq!(r.read_literal(8), SENTINEL);

    let mut swapped = BoolWriter::new();
    swapped.write_tree(&MV_JOINT_TREE, &ctx.joints, 3);
    write_mv_component(&mut swapped, &ctx.comps[1], 5, true);
    write_mv_component(&mut swapped, &ctx.comps[0], -7, true);
    let bad = swapped.finish();
    let mut r2 = BoolReader::new(&bad).expect("marker");
    assert_ne!(
        read_mv(&mut r2, ref_mv, &ctx, None, true),
        target,
        "transposing the components must be observable"
    );
}

#[test]
fn read_mv_high_precision_follows_the_reference_not_only_the_frame() {
    // use_mv_hp(ref) is false once a component reaches 64, so an allow_hp
    // frame still codes a large-reference vector without hp bits.
    let small = MotionVector::new(8, 8);
    let large = MotionVector::new(64, 0);
    assert!(use_mv_hp(small) && !use_mv_hp(large));

    let mut w_small = BoolWriter::new();
    write_mv(
        &mut w_small,
        &NMV,
        small,
        MotionVector::new(small.row + 6, small.col),
        true,
    );
    let n_small = w_small.total_writes();

    let mut w_large = BoolWriter::new();
    let large_target = MotionVector::new(large.row + 6, large.col);
    write_mv(&mut w_large, &NMV, large, large_target, true);
    let n_large = w_large.total_writes();
    w_large.write_literal(SENTINEL, 8);
    let data = w_large.finish();

    assert_eq!(
        n_small,
        n_large + 1,
        "the large reference suppresses exactly the hp bool"
    );
    let mut r = BoolReader::new(&data).expect("marker");
    assert_eq!(read_mv(&mut r, large, &NMV, None, true), large_target);
    assert_eq!(r.read_literal(8), SENTINEL);
}

#[test]
fn read_mv_counts_the_difference_not_the_result() {
    let ref_mv = MotionVector::new(20, -4);
    let target = MotionVector::new(26, -4); // diff (+6, 0) -> MV_JOINT_HZVNZ

    let mut w = BoolWriter::new();
    write_mv(&mut w, &NMV, ref_mv, target, true);
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");
    let mut counts = FrameCounts::new();
    assert_eq!(
        read_mv(&mut r, ref_mv, &NMV, Some(&mut counts.mv), true),
        target
    );

    assert_eq!(
        counts.mv.joints,
        [0, 0, 1, 0],
        "the joint is derived from the difference, not the result"
    );
    assert_eq!(
        counts.mv.comps[0].sign,
        [1, 0],
        "difference row is positive"
    );
    assert_eq!(
        counts.mv.comps[1].sign,
        [0, 0],
        "the column component is neither coded nor counted"
    );
    // diff.row = 6 -> z = 5 -> class 0, offset 5 -> d = 0, fr = 2, hp = 1.
    assert_eq!(counts.mv.comps[0].classes[0], 1);
    assert_eq!(counts.mv.comps[0].class0, [1, 0]);
    assert_eq!(counts.mv.comps[0].class0_fp[0], [0, 0, 1, 0]);
    assert_eq!(counts.mv.comps[0].class0_hp, [0, 1]);
}

#[test]
fn inc_mv_counts_the_high_precision_bit_even_when_none_was_coded() {
    // libvpx's vp9_inc_mv passes usehp = 1 unconditionally
    // (vp9_entropymv.c:145,149), so a vector coded without hp bits still
    // votes in hp[1]. Counting the real use_hp instead would change every
    // adapted hp probability on a mixed-precision frame.
    let large = MotionVector::new(1000, 0); // use_mv_hp -> false
    let target = MotionVector::new(1006, 0);

    let mut w = BoolWriter::new();
    write_mv(&mut w, &NMV, large, target, true);
    let n_written = w.total_writes();
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");
    let mut counts = FrameCounts::new();
    assert_eq!(
        read_mv(&mut r, large, &NMV, Some(&mut counts.mv), true),
        target
    );

    assert_eq!(
        counts.mv.comps[0].class0_hp,
        [0, 1],
        "hp counted as 1 despite no hp bool being read"
    );
    assert_eq!(
        counts.mv.comps[0].class0_hp.iter().sum::<u32>(),
        1,
        "exactly one hp vote"
    );
    assert!(n_written > 0);
}

#[test]
fn inc_mv_agrees_with_the_fields_the_reader_assembled() {
    for class in 0..=10usize {
        let (lo, hi) = class_bounds(class);
        for mag in [lo, hi] {
            let mut counts = FrameCounts::new();
            inc_mv(MotionVector::new(mag as i16, 0), &mut counts.mv);
            assert_eq!(
                counts.mv.comps[0].classes[class], 1,
                "magnitude {mag} counted into class {class}"
            );
            assert_eq!(counts.mv.joints, [0, 0, 1, 0]);
            assert_eq!(counts.mv.comps[1].classes, [0; 11], "column not counted");
        }
    }
}

// ---------------------------------------------------------------------------
// Segmentation scope
// ---------------------------------------------------------------------------

#[test]
fn segmentation_enabled_inter_frame_is_refused_honestly() {
    let mut seg = seg_off();
    seg.enabled = true;
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let mut w = BoolWriter::new();
    w.write_literal(SENTINEL, 8);
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");

    // `MiInfo` has no `Debug`, so the `Ok` arm is matched rather than
    // unwrapped through `expect_err`.
    match read_inter_frame_mode_info(
        &mut r,
        &grid,
        &cfg,
        &probs,
        None,
        Neighbours::default(),
        corner(6),
    ) {
        Err(CodecError::UnsupportedFeature(msg)) => assert!(
            msg.contains("segmentation") && msg.contains("segment map"),
            "the message must name what is missing, got: {msg}"
        ),
        Err(other) => panic!("expected UnsupportedFeature, got {other:?}"),
        Ok(_) => panic!("segmentation-enabled inter frames are out of scope"),
    }
}

/// Builds a `Ctx` over the given fixtures, for the branches the entry point
/// refuses until the segment maps land.
macro_rules! seg_ctx {
    ($grid:expr, $cfg:expr, $probs:expr, $counts:expr) => {
        Ctx {
            grid: $grid,
            cfg: $cfg,
            probs: $probs,
            counts: $counts,
            above: None,
            left: None,
            mi_row: 0,
            mi_col: 0,
        }
    };
}

#[test]
fn segment_level_reference_frame_overrides_the_coded_reference() {
    // read_ref_frames' SEG_LVL_REF_FRAME branch (vp9_decodemv.c:309-312)
    // codes no bits at all.
    let mut seg = seg_off();
    seg.enabled = true;
    seg.feature_enabled[3][SEG_LVL_REF_FRAME] = true;
    seg.feature_data[3][SEG_LVL_REF_FRAME] = i16::from(GOLDEN_FRAME);
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);

    let mut w = BoolWriter::new();
    w.write_literal(SENTINEL, 8);
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");

    let mut ctx = seg_ctx!(&grid, &cfg, &probs, None);
    assert_eq!(ctx.read_ref_frames(&mut r, 3), [GOLDEN_FRAME, NONE_FRAME]);
    assert!(
        ctx.read_is_inter_block(&mut r, 3),
        "a non-INTRA pinned reference makes the block inter without a bit"
    );
    assert_eq!(
        r.read_literal(8),
        SENTINEL,
        "the segment override codes no bits"
    );
}

#[test]
fn segment_level_reference_frame_of_intra_makes_the_block_intra() {
    let mut seg = seg_off();
    seg.enabled = true;
    seg.feature_enabled[1][SEG_LVL_REF_FRAME] = true;
    seg.feature_data[1][SEG_LVL_REF_FRAME] = i16::from(INTRA_FRAME);
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let data = BoolWriter::new().finish();
    let mut r = BoolReader::new(&data).expect("marker");
    let mut ctx = seg_ctx!(&grid, &cfg, &probs, None);
    assert!(!ctx.read_is_inter_block(&mut r, 1));
}

#[test]
fn segment_level_skip_forces_skip_without_coding_a_bit() {
    let mut seg = seg_off();
    seg.enabled = true;
    seg.feature_enabled[2][SEG_LVL_SKIP] = true;
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let mut w = BoolWriter::new();
    w.write_literal(SENTINEL, 8);
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");
    let mut counts = FrameCounts::new();
    {
        let mut ctx = seg_ctx!(&grid, &cfg, &probs, Some(&mut counts));
        assert!(ctx.read_skip(&mut r, 2));
    }
    assert_eq!(r.read_literal(8), SENTINEL);
    assert_eq!(
        counts.skip, [[0; 2]; 3],
        "the forced flag is not counted (vp9_decodemv.c:181-182)"
    );
}

#[test]
fn segment_level_skip_below_8x8_is_rejected() {
    // vp9_decodemv.c:709-713: "Invalid usage of segment feature on small
    // blocks".
    let mut seg = seg_off();
    seg.enabled = true;
    seg.feature_enabled[0][SEG_LVL_SKIP] = true;
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let mut w = BoolWriter::new();
    write_single_ref(&mut w, &probs, 2, 2, LAST_FRAME);
    let data = w.finish();
    let mut r = BoolReader::new(&data).expect("marker");

    let mut mi = MiInfo {
        sb_type: 0, // BLOCK_4X4
        segment_id: 0,
        ..MiInfo::default()
    };
    let mut ctx = seg_ctx!(&grid, &cfg, &probs, None);
    let err = ctx
        .read_inter_block_mode_info(&mut r, &mut mi)
        .expect_err("SEG_LVL_SKIP below BLOCK_8X8 is invalid");
    match err {
        CodecError::InvalidBitstream(msg) => assert!(
            msg.contains("small blocks"),
            "the message must name the constraint, got: {msg}"
        ),
        other => panic!("expected InvalidBitstream, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Whole blocks: intra on an inter frame
// ---------------------------------------------------------------------------

#[test]
fn intra_block_on_an_inter_frame_uses_block_size_group_probabilities() {
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let bsize = 9u8; // BLOCK_32X32
    let group = usize::from(SIZE_GROUP_LOOKUP[bsize as usize]);
    assert_eq!(group, 3);

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, 0, false);
    write_is_inter(&mut w, &probs, 0, false);
    // allow_select = !skip || !inter_block = true; TX_MODE_SELECT; >= 8x8.
    write_tx_size(&mut w, &probs, 1, tables::MAX_TXSIZE_LOOKUP[9], 2);
    write_intra_mode_y(&mut w, &probs, group, 4); // D135_PRED
    write_intra_mode_uv(&mut w, 4, 9); // TM_PRED
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    let mut counts = FrameCounts::new();
    let mi = decode_block(
        &data,
        &grid,
        &cfg,
        &probs,
        Some(&mut counts),
        Neighbours::default(),
        corner(bsize),
    );

    assert_eq!(mi.sb_type, bsize);
    assert!(!mi.skip);
    assert!(!mi.is_inter);
    assert_eq!(mi.tx_size, 2);
    assert_eq!(mi.mode, 4);
    assert_eq!(mi.uv_mode, 9);
    assert_eq!(mi.ref_frame, [INTRA_FRAME, NONE_FRAME]);
    assert_eq!(
        mi.interp_filter, SWITCHABLE_FILTERS,
        "intra blocks carry the filter sentinel, not EIGHTTAP"
    );
    assert_eq!(mi.mv, [MotionVector::zero(); 2]);

    assert_eq!(counts.y_mode[group][4], 1);
    assert_eq!(counts.uv_mode[4][9], 1);
    assert_eq!(counts.intra_inter[0], [1, 0]);
    assert_eq!(counts.tx.p32x32[1][2], 1);
}

#[test]
fn intra_sub_8x8_block_on_an_inter_frame_replicates_its_modes() {
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);

    // BLOCK_4X8: two reads, each replicated down a column, both with the
    // literal size group 0.
    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, 0, false);
    write_is_inter(&mut w, &probs, 0, false);
    // bsize < BLOCK_8X8 -> read_tx_size takes the min() branch, no bits.
    write_intra_mode_y(&mut w, &probs, 0, 2);
    write_intra_mode_y(&mut w, &probs, 0, 7);
    write_intra_mode_uv(&mut w, 7, 1);
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    let mut counts = FrameCounts::new();
    let mi = decode_block(
        &data,
        &grid,
        &cfg,
        &probs,
        Some(&mut counts),
        Neighbours::default(),
        corner(1),
    );

    assert_eq!(mi.bmi, [2, 7, 2, 7], "BLOCK_4X8 replicates down columns");
    assert_eq!(mi.mode, 7, "the block mode is the last sub-block's");
    assert_eq!(mi.uv_mode, 1);
    assert_eq!(mi.tx_size, 0);
    assert_eq!(counts.y_mode[0][2], 1);
    assert_eq!(counts.y_mode[0][7], 1);
    assert_eq!(counts.uv_mode[7][1], 1);
}

// ---------------------------------------------------------------------------
// Whole blocks: single-reference inter
// ---------------------------------------------------------------------------

/// Encodes and decodes a `>= 8x8` single-reference inter block with no
/// neighbours. `mv` is the `(predictor, target)` pair a `NEWMV` codes.
fn single_ref_block(
    bsize: u8,
    ref_frame: i8,
    mode: u8,
    skip: bool,
    mv: Option<(MotionVector, MotionVector)>,
) -> (MiInfo, FrameCounts) {
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, 0, skip);
    write_is_inter(&mut w, &probs, 0, true);
    if !skip {
        write_tx_size(
            &mut w,
            &probs,
            1,
            tables::MAX_TXSIZE_LOOKUP[bsize as usize],
            1,
        );
    }
    write_single_ref(&mut w, &probs, 2, 2, ref_frame);
    write_inter_mode(&mut w, &probs, 2, mode);
    // A fixed frame filter codes no per-block symbol.
    if let Some((ref_mv, target)) = mv {
        write_mv(&mut w, &probs.mv, ref_mv, target, false);
    }
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    let mut counts = FrameCounts::new();
    let mi = decode_block(
        &data,
        &grid,
        &cfg,
        &probs,
        Some(&mut counts),
        Neighbours::default(),
        corner(bsize),
    );
    (mi, counts)
}

#[test]
fn zeromv_single_reference_block_decodes_every_field() {
    let (mi, counts) = single_ref_block(6, LAST_FRAME, ZEROMV, false, None);
    assert_eq!(mi.sb_type, 6);
    assert!(mi.is_inter);
    assert_eq!(mi.ref_frame, [LAST_FRAME, NONE_FRAME]);
    assert_eq!(mi.mode, ZEROMV);
    assert_eq!(mi.mv, [MotionVector::zero(); 2]);
    assert_eq!(mi.bmv, [[MotionVector::zero(); 2]; 4]);
    assert_eq!(mi.tx_size, 1);
    assert_eq!(
        mi.interp_filter, 0,
        "a non-switchable frame stamps its own filter"
    );
    // With no neighbours the mode context is counter_to_context[0] = 2.
    assert_eq!(counts.inter_mode[2][(ZEROMV - NEARESTMV) as usize], 1);
    assert_eq!(counts.single_ref[2][0], [1, 0], "bit0 = 0 selects LAST");
    assert_eq!(counts.switchable_interp, [[0; 3]; 4], "no filter coded");
    assert_eq!(counts.mv.joints, [0; 4], "ZEROMV codes no vector");
}

#[test]
fn single_reference_chain_selects_golden_and_altref() {
    let (golden, gc) = single_ref_block(6, GOLDEN_FRAME, ZEROMV, false, None);
    assert_eq!(golden.ref_frame, [GOLDEN_FRAME, NONE_FRAME]);
    assert_eq!(gc.single_ref[2][0], [0, 1], "bit0 = 1 leaves LAST");
    assert_eq!(gc.single_ref[2][1], [1, 0], "bit1 = 0 selects GOLDEN");

    let (altref, ac) = single_ref_block(6, ALTREF_FRAME, ZEROMV, false, None);
    assert_eq!(altref.ref_frame, [ALTREF_FRAME, NONE_FRAME]);
    assert_eq!(ac.single_ref[2][1], [0, 1], "bit1 = 1 selects ALTREF");
}

#[test]
fn newmv_block_adds_the_difference_to_the_predictor() {
    // With an empty grid every candidate is the zero fill, so the NEWMV
    // predictor is (0, 0) and the decoded vector is the difference itself.
    let target = MotionVector::new(-14, 26);
    let (mi, counts) = single_ref_block(
        6,
        LAST_FRAME,
        NEWMV,
        false,
        Some((MotionVector::zero(), target)),
    );
    assert_eq!(mi.mode, NEWMV);
    assert_eq!(mi.mv[0], target);
    assert_eq!(
        mi.mv[1],
        MotionVector::zero(),
        "a single-reference block leaves the second vector alone"
    );
    assert_eq!(counts.mv.joints, [0, 0, 0, 1], "both components coded");
}

#[test]
fn a_skipped_inter_block_codes_no_transform_size() {
    // allow_select = !skip || !inter_block, so a skipped inter block takes
    // the min() branch: TX_MODE_SELECT's biggest size, capped by the block.
    let (mi, counts) = single_ref_block(6, LAST_FRAME, ZEROMV, true, None);
    assert!(mi.skip);
    assert_eq!(mi.tx_size, tables::MAX_TXSIZE_LOOKUP[6]);
    assert_eq!(counts.tx.p16x16, [[0; 3]; 2], "no tx_size symbol was coded");
    assert_eq!(counts.skip[0], [0, 1]);
}

#[test]
fn nearestmv_and_nearmv_take_different_candidates() {
    // Two distinct inter neighbours fill both candidate slots; NEARESTMV must
    // take list[0] and NEARMV list[1].
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let first = MotionVector::new(12, -4);
    let second = MotionVector::new(-30, 18);

    let mut grid = TestGrid::new(4, 8);
    grid.set(0, 1, inter_mi(ZEROMV, LAST_FRAME, first));
    grid.set(1, 0, inter_mi(ZEROMV, LAST_FRAME, second));
    let pos = BlockPos {
        mi_row: 1,
        mi_col: 1,
        bsize: 6,
    };
    let list = find_mv_refs(
        &grid,
        &cfg.tile,
        None,
        &ALTREF_BIAS,
        LAST_FRAME,
        pos.mi_row,
        pos.mi_col,
        pos.bsize,
        -1,
    )
    .list;
    assert_ne!(list[0], list[1], "the fixture must fill both entries");

    let above = grid.mi_at(0, 1);
    let left = grid.mi_at(1, 0);
    let nb = Neighbours {
        above: Some(&above),
        left: Some(&left),
    };
    let mode_ctx = usize::from(get_mode_context(
        &grid, &cfg.tile, pos.mi_row, pos.mi_col, pos.bsize,
    ));
    let skip_ctx = get_skip_context(nb.above, nb.left);
    let inter_ctx = get_intra_inter_context(nb.above, nb.left);
    let tx_ctx = get_tx_size_context(nb.above, nb.left, pos.bsize);
    let ref_ctx0 = get_pred_context_single_ref_p1(nb.above, nb.left);
    let ref_ctx1 = get_pred_context_single_ref_p2(nb.above, nb.left);

    for (mode, expected) in [(NEARESTMV, list[0]), (NEARMV, list[1])] {
        let mut w = BoolWriter::new();
        write_skip(&mut w, &probs, skip_ctx, false);
        write_is_inter(&mut w, &probs, inter_ctx, true);
        write_tx_size(
            &mut w,
            &probs,
            tx_ctx,
            tables::MAX_TXSIZE_LOOKUP[pos.bsize as usize],
            0,
        );
        write_single_ref(&mut w, &probs, ref_ctx0, ref_ctx1, LAST_FRAME);
        write_inter_mode(&mut w, &probs, mode_ctx, mode);
        w.write_literal(SENTINEL, 8);
        let data = w.finish();

        let mi = decode_block(&data, &grid, &cfg, &probs, None, nb, pos);
        assert_eq!(mi.mode, mode);
        assert_eq!(
            mi.mv[0],
            lower_mv_precision(expected, false),
            "mode {mode} must take its own candidate"
        );
    }
}

// ---------------------------------------------------------------------------
// Compound reference
// ---------------------------------------------------------------------------

#[test]
fn compound_reference_assigns_fixed_and_variable_slots() {
    let seg = seg_off();
    let mut cfg = frame_cfg(&seg);
    cfg.reference_mode = ReferenceMode::Select;
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let comp = cfg.comp;
    assert_eq!(comp.comp_fixed_ref, ALTREF_FRAME);
    assert_eq!(comp.comp_var_ref, [LAST_FRAME, GOLDEN_FRAME]);
    let fix_idx = comp.fix_ref_idx();
    let mode_ctx = get_reference_mode_context(None, None, comp.comp_fixed_ref);
    let comp_ctx = get_pred_context_comp_ref_p(None, None, &comp);

    for (bit, var_ref) in [(false, LAST_FRAME), (true, GOLDEN_FRAME)] {
        let mut w = BoolWriter::new();
        write_skip(&mut w, &probs, 0, false);
        write_is_inter(&mut w, &probs, 0, true);
        write_tx_size(&mut w, &probs, 1, tables::MAX_TXSIZE_LOOKUP[6], 0);
        w.write_bool(probs.comp_inter[mode_ctx], true);
        w.write_bool(probs.comp_ref[comp_ctx], bit);
        write_inter_mode(&mut w, &probs, 2, ZEROMV);
        w.write_literal(SENTINEL, 8);
        let data = w.finish();

        let mut counts = FrameCounts::new();
        let mi = decode_block(
            &data,
            &grid,
            &cfg,
            &probs,
            Some(&mut counts),
            Neighbours::default(),
            corner(6),
        );

        assert_eq!(
            mi.ref_frame[fix_idx], comp.comp_fixed_ref,
            "the fixed reference goes in the sign-bias-selected slot"
        );
        assert_eq!(mi.ref_frame[1 - fix_idx], var_ref);
        assert!(mi.is_inter);
        assert_eq!(counts.comp_inter[mode_ctx], [0, 1]);
        assert_eq!(counts.comp_ref[comp_ctx][usize::from(bit)], 1);
    }
}

#[test]
fn compound_newmv_reads_two_motion_vectors() {
    let seg = seg_off();
    let mut cfg = frame_cfg(&seg);
    cfg.reference_mode = ReferenceMode::Compound;
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let comp = cfg.comp;
    let fix_idx = comp.fix_ref_idx();
    let first = MotionVector::new(8, -10);
    let second = MotionVector::new(-4, 22);

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, 0, false);
    write_is_inter(&mut w, &probs, 0, true);
    write_tx_size(&mut w, &probs, 1, tables::MAX_TXSIZE_LOOKUP[6], 0);
    // COMPOUND_REFERENCE is frame-level here, so only the comp_ref bit codes.
    let comp_ctx = get_pred_context_comp_ref_p(None, None, &comp);
    w.write_bool(probs.comp_ref[comp_ctx], false);
    write_inter_mode(&mut w, &probs, 2, NEWMV);
    write_mv(&mut w, &probs.mv, MotionVector::zero(), first, false);
    write_mv(&mut w, &probs.mv, MotionVector::zero(), second, false);
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    let mut counts = FrameCounts::new();
    let mi = decode_block(
        &data,
        &grid,
        &cfg,
        &probs,
        Some(&mut counts),
        Neighbours::default(),
        corner(6),
    );

    assert_eq!(mi.ref_frame[fix_idx], ALTREF_FRAME);
    assert_eq!(mi.ref_frame[1 - fix_idx], LAST_FRAME);
    assert_eq!(
        mi.mv,
        [first, second],
        "both references get their own vector"
    );
    assert_eq!(
        counts.comp_inter, [[0; 2]; 5],
        "a frame-level COMPOUND mode codes no per-block reference-mode bit"
    );
    assert_eq!(
        counts.mv.joints[3], 2,
        "two vectors, both with a non-zero row and column"
    );
}

// ---------------------------------------------------------------------------
// Interpolation filter
// ---------------------------------------------------------------------------

#[test]
fn switchable_filter_is_read_after_the_block_mode() {
    // vp9_decodemv.c:714-721 reads the inter mode first and the filter
    // second. Encoding them the other way round must be observable.
    let seg = seg_off();
    let mut cfg = frame_cfg(&seg);
    cfg.interp_filter = SWITCHABLE;
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let filter_ctx = get_pred_context_switchable_interp(None, None);
    assert_eq!(filter_ctx, usize::from(SWITCHABLE_FILTERS));

    let head = |w: &mut BoolWriter| {
        write_skip(w, &probs, 0, false);
        write_is_inter(w, &probs, 0, true);
        write_tx_size(w, &probs, 1, tables::MAX_TXSIZE_LOOKUP[6], 0);
        write_single_ref(w, &probs, 2, 2, LAST_FRAME);
    };

    let mut good = BoolWriter::new();
    head(&mut good);
    write_inter_mode(&mut good, &probs, 2, NEARESTMV);
    write_switchable(&mut good, &probs, filter_ctx, 2);
    good.write_literal(SENTINEL, 8);
    let data = good.finish();

    let mut counts = FrameCounts::new();
    let mi = decode_block(
        &data,
        &grid,
        &cfg,
        &probs,
        Some(&mut counts),
        Neighbours::default(),
        corner(6),
    );
    assert_eq!(mi.mode, NEARESTMV);
    assert_eq!(mi.interp_filter, 2);
    assert_eq!(counts.switchable_interp[filter_ctx][2], 1);

    let mut swapped = BoolWriter::new();
    head(&mut swapped);
    write_switchable(&mut swapped, &probs, filter_ctx, 2);
    write_inter_mode(&mut swapped, &probs, 2, NEARESTMV);
    swapped.write_literal(SENTINEL, 8);
    let bad = swapped.finish();
    let mut r = BoolReader::new(&bad).expect("marker");
    let observable = match read_inter_frame_mode_info(
        &mut r,
        &grid,
        &cfg,
        &probs,
        None,
        Neighbours::default(),
        corner(6),
    ) {
        Err(_) => true,
        Ok(other) => {
            (other.mode, other.interp_filter) != (NEARESTMV, 2) || r.read_literal(8) != SENTINEL
        }
    };
    assert!(observable, "reversing mode and filter must be observable");
}

#[test]
fn a_fixed_frame_filter_is_stamped_on_every_block() {
    let seg = seg_off();
    let mut cfg = frame_cfg(&seg);
    cfg.interp_filter = frame_interp_filter(0); // raw literal 0 -> EIGHTTAP_SMOOTH
    assert_eq!(cfg.interp_filter, 1);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, 0, false);
    write_is_inter(&mut w, &probs, 0, true);
    write_tx_size(&mut w, &probs, 1, tables::MAX_TXSIZE_LOOKUP[6], 0);
    write_single_ref(&mut w, &probs, 2, 2, LAST_FRAME);
    write_inter_mode(&mut w, &probs, 2, ZEROMV);
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    let mi = decode_block(
        &data,
        &grid,
        &cfg,
        &probs,
        None,
        Neighbours::default(),
        corner(6),
    );
    assert_eq!(mi.interp_filter, 1);
}

// ---------------------------------------------------------------------------
// Sub-8x8
// ---------------------------------------------------------------------------

/// Encodes a sub-8x8 inter block whose sub-blocks all code `NEWMV` against a
/// zero predictor, with the given per-sub-block vectors, and decodes it.
fn sub8x8_block(bsize: u8, targets: &[MotionVector]) -> MiInfo {
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, 0, false);
    write_is_inter(&mut w, &probs, 0, true);
    // bsize < BLOCK_8X8: no tx_size symbol, and no block-level inter mode.
    write_single_ref(&mut w, &probs, 2, 2, LAST_FRAME);
    for target in targets {
        write_inter_mode(&mut w, &probs, 2, NEWMV);
        write_mv(&mut w, &probs.mv, MotionVector::zero(), *target, false);
    }
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    decode_block(
        &data,
        &grid,
        &cfg,
        &probs,
        None,
        Neighbours::default(),
        corner(bsize),
    )
}

#[test]
fn sub_8x8_block_4x4_reads_four_sub_blocks() {
    let mvs = [
        MotionVector::new(2, 4),
        MotionVector::new(-6, 8),
        MotionVector::new(10, -12),
        MotionVector::new(-14, 16),
    ];
    let mi = sub8x8_block(0, &mvs);
    for (j, want) in mvs.iter().enumerate() {
        assert_eq!(mi.bmv[j][0], *want, "sub-block {j}");
    }
    assert_eq!(mi.mode, NEWMV, "the block mode is the last sub-block's");
    assert_eq!(mi.mv[0], mvs[3], "the block vector is bmi[3]'s");
}

#[test]
fn sub_8x8_block_4x8_replicates_down_columns() {
    // num_4x4_w = 1, num_4x4_h = 2: sub-blocks 0 and 1 are read, each copied
    // to j + 2.
    let mvs = [MotionVector::new(4, -6), MotionVector::new(-8, 10)];
    let mi = sub8x8_block(1, &mvs);
    assert_eq!(mi.bmv[0][0], mvs[0]);
    assert_eq!(mi.bmv[1][0], mvs[1]);
    assert_eq!(mi.bmv[2][0], mvs[0], "bmi[2] = bmi[0]");
    assert_eq!(mi.bmv[3][0], mvs[1], "bmi[3] = bmi[1]");
    assert_eq!(mi.mv[0], mvs[1]);
}

#[test]
fn sub_8x8_block_8x4_replicates_across_rows() {
    // num_4x4_w = 2, num_4x4_h = 1: sub-blocks 0 and 2 are read, each copied
    // to j + 1.
    let mvs = [MotionVector::new(12, 14), MotionVector::new(-16, -18)];
    let mi = sub8x8_block(2, &mvs);
    assert_eq!(mi.bmv[0][0], mvs[0]);
    assert_eq!(mi.bmv[1][0], mvs[0], "bmi[1] = bmi[0]");
    assert_eq!(mi.bmv[2][0], mvs[1]);
    assert_eq!(mi.bmv[3][0], mvs[1], "bmi[3] = bmi[2]");
    assert_eq!(mi.mv[0], mvs[1]);
}

#[test]
fn sub_8x8_new_mv_predictor_is_looked_up_once_per_block() {
    // got_mv_refs_for_new (vp9_decodemv.c:728,743,753) makes the NEWMV
    // reference lookup happen for the first NEWMV sub-block only, so every
    // sub-block's vector is (the same) predictor + its own difference.
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let cand = MotionVector::new(24, -16);
    let mut grid = TestGrid::new(4, 8);
    grid.set(0, 1, inter_mi(ZEROMV, LAST_FRAME, cand));
    grid.set(1, 0, inter_mi(ZEROMV, LAST_FRAME, cand));
    let pos = BlockPos {
        mi_row: 1,
        mi_col: 1,
        bsize: 0,
    };

    let predictor = lower_mv_precision(
        find_mv_refs(
            &grid,
            &cfg.tile,
            None,
            &ALTREF_BIAS,
            LAST_FRAME,
            pos.mi_row,
            pos.mi_col,
            pos.bsize,
            -1,
        )
        .list[0],
        false,
    );
    assert_ne!(
        predictor,
        MotionVector::zero(),
        "the fixture must actually predict"
    );

    let above = grid.mi_at(0, 1);
    let left = grid.mi_at(1, 0);
    let nb = Neighbours {
        above: Some(&above),
        left: Some(&left),
    };
    let mode_ctx = usize::from(get_mode_context(
        &grid, &cfg.tile, pos.mi_row, pos.mi_col, pos.bsize,
    ));
    let targets = [
        MotionVector::new(predictor.row + 2, predictor.col + 4),
        MotionVector::new(predictor.row - 6, predictor.col + 8),
        MotionVector::new(predictor.row + 10, predictor.col - 12),
        MotionVector::new(predictor.row - 14, predictor.col + 16),
    ];

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, get_skip_context(nb.above, nb.left), false);
    write_is_inter(
        &mut w,
        &probs,
        get_intra_inter_context(nb.above, nb.left),
        true,
    );
    write_single_ref(
        &mut w,
        &probs,
        get_pred_context_single_ref_p1(nb.above, nb.left),
        get_pred_context_single_ref_p2(nb.above, nb.left),
        LAST_FRAME,
    );
    for target in &targets {
        write_inter_mode(&mut w, &probs, mode_ctx, NEWMV);
        write_mv(&mut w, &probs.mv, predictor, *target, false);
    }
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    let mi = decode_block(&data, &grid, &cfg, &probs, None, nb, pos);
    for (j, want) in targets.iter().enumerate() {
        assert_eq!(mi.bmv[j][0], *want, "sub-block {j}");
    }
}

#[test]
fn sub8x8_nearmv_matches_find_mv_refs_near() {
    // Pins the module-doc claim that the common find_mv_refs stands in for
    // the decoder's dec_find_mv_refs: for sub-block 0 with b_mode = NEARMV,
    // libvpx takes mv_list[refmv_count - 1] with refmv_count = 2, i.e.
    // list[1] — exactly MvRefResult::near().
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let first = MotionVector::new(9, -3);
    let second = MotionVector::new(-21, 15);
    let mut grid = TestGrid::new(4, 8);
    grid.set(0, 1, inter_mi(NEWMV, LAST_FRAME, first));
    grid.set(1, 0, inter_mi(NEWMV, LAST_FRAME, second));
    let pos = BlockPos {
        mi_row: 1,
        mi_col: 1,
        bsize: 0,
    };

    let list = find_mv_refs(
        &grid,
        &cfg.tile,
        None,
        &ALTREF_BIAS,
        LAST_FRAME,
        pos.mi_row,
        pos.mi_col,
        pos.bsize,
        0,
    )
    .list;
    assert_ne!(list[0], list[1], "the fixture must fill both entries");

    let above = grid.mi_at(0, 1);
    let left = grid.mi_at(1, 0);
    let nb = Neighbours {
        above: Some(&above),
        left: Some(&left),
    };
    let mode_ctx = usize::from(get_mode_context(
        &grid, &cfg.tile, pos.mi_row, pos.mi_col, pos.bsize,
    ));

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, get_skip_context(nb.above, nb.left), false);
    write_is_inter(
        &mut w,
        &probs,
        get_intra_inter_context(nb.above, nb.left),
        true,
    );
    write_single_ref(
        &mut w,
        &probs,
        get_pred_context_single_ref_p1(nb.above, nb.left),
        get_pred_context_single_ref_p2(nb.above, nb.left),
        LAST_FRAME,
    );
    // Sub-block 0 codes NEARMV; the rest ZEROMV, which adds nothing.
    write_inter_mode(&mut w, &probs, mode_ctx, NEARMV);
    for _ in 1..4 {
        write_inter_mode(&mut w, &probs, mode_ctx, ZEROMV);
    }
    w.write_literal(SENTINEL, 8);
    let data = w.finish();

    let mi = decode_block(&data, &grid, &cfg, &probs, None, nb, pos);
    assert_eq!(
        mi.bmv[0][0], list[1],
        "sub-block 0 NEARMV takes the second candidate"
    );
    assert_eq!(mi.mode, ZEROMV, "the block mode is the last sub-block's");
    assert_eq!(mi.mv, [MotionVector::zero(); 2], "bmi[3] was ZEROMV");
}

// ---------------------------------------------------------------------------
// Error paths
// ---------------------------------------------------------------------------

#[test]
fn an_out_of_range_new_motion_vector_is_rejected() {
    // MV_UPP is 16383; a difference landing on 16384 must be an honest error
    // rather than a silently wrapped vector.
    let seg = seg_off();
    let cfg = frame_cfg(&seg);
    let probs = FrameProbs::defaults();
    let grid = TestGrid::new(4, 8);
    let huge = MotionVector::new(16384, 0);
    assert!(!is_mv_valid(huge));

    let mut w = BoolWriter::new();
    write_skip(&mut w, &probs, 0, false);
    write_is_inter(&mut w, &probs, 0, true);
    write_tx_size(&mut w, &probs, 1, tables::MAX_TXSIZE_LOOKUP[6], 0);
    write_single_ref(&mut w, &probs, 2, 2, LAST_FRAME);
    write_inter_mode(&mut w, &probs, 2, NEWMV);
    write_mv(&mut w, &probs.mv, MotionVector::zero(), huge, false);
    let data = w.finish();

    let mut r = BoolReader::new(&data).expect("marker");
    match read_inter_frame_mode_info(
        &mut r,
        &grid,
        &cfg,
        &probs,
        None,
        Neighbours::default(),
        corner(6),
    ) {
        Err(CodecError::InvalidBitstream(msg)) => assert!(
            msg.contains("16384"),
            "the message must name the value, got: {msg}"
        ),
        Err(other) => panic!("expected InvalidBitstream, got {other:?}"),
        Ok(_) => panic!("an out-of-range vector must not decode"),
    }
}

#[test]
fn is_mv_valid_matches_the_libvpx_bounds() {
    assert!(is_mv_valid(MotionVector::new(16382, -16383)));
    assert!(!is_mv_valid(MotionVector::new(16383, 0)), "MV_UPP excluded");
    assert!(
        !is_mv_valid(MotionVector::new(0, -16384)),
        "MV_LOW excluded"
    );
}
