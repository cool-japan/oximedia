//! Unit tests for the parent `mode` module — the VP8 inter-frame macroblock
//! prediction record decoder.
//!
//! Split out of `mode.rs` (which is close to this workspace's 2000-line file
//! limit) as a verbatim move: the module is still `dec::mode::tests`, so every
//! `use super::...` path and every relative `include_bytes!` resolves exactly
//! as it did inline.

use super::super::header::{InterFrameHeader, LoopFilterHeader, QuantHeader, SegmentHeader};
use super::super::tables;
use super::super::tables_inter::{
    DEFAULT_MV_CONTEXT, DEFAULT_UV_MODE_PROB, DEFAULT_YMODE_PROB, MBSPLIT_16X8, MBSPLIT_4X4,
    MBSPLIT_8X16, MBSPLIT_8X8, MVPIS_SHORT, MVPSHORT, MVPSIGN, SMALL_MVTREE,
};
// P3 relocated the RFC-verified `TestBoolEncoder` (RFC 6386 §7.3 `write_bool` /
// `flush_bool_encoder`, lines 1141-1228) out of `header.rs`'s tests into `dec/testutil.rs`,
// together with the symbol-level writers these tests need: `write_tree` (dual of
// `BoolDecoder::read_tree`) and `write_mv_component` (dual of `mv::read_mv_component`, taking
// a raw quarter-pel value). It was still a stub when this package started; now that it has
// landed, these tests use it rather than carrying a second copy.
use super::super::testutil::TestBoolEncoder;
use super::*;

// -- Reference §17.2 MV reader (the injected seam's stand-in) ---------
//
// Transcribed from dixie `read_mv_component` / `read_mv` (rfc6386.txt lines 10054-10095,
// 10174-10181). P3 owns the production copy in `dec/mv.rs`; P6 replaces every
// `reference_read_mv` injection below with it, leaving this as an independent cross-check to
// keep or delete. Until then it is what keeps the fixture test's bitstream in sync across the
// NEWMV / NEW4X4 macroblocks.

/// dixie `read_mv_component` (lines 10054-10095). Returns an **eighth-pel** component: the
/// coded quarter-pel magnitude doubled (`return x << 1`, line 10093).
fn reference_read_mv_component(bd: &mut BoolDecoder<'_>, p: &[u8; 19]) -> i32 {
    const BITS: usize = MVPSHORT + 8 - 1;
    const LONG_WIDTH: usize = 10;
    let mut x = 0i32;
    if bd.get_bool(p[MVPIS_SHORT]) {
        // Long form: bits 0..2 low-to-high, then 9..4 high-to-low.
        for i in 0..3 {
            x += i32::from(bd.get_bool(p[BITS + i])) << i;
        }
        for i in (4..LONG_WIDTH).rev() {
            x += i32::from(bd.get_bool(p[BITS + i])) << i;
        }
        // Bit 3 is implicit when any higher bit is set (line 10088).
        if (x & 0xFFF0) == 0 || bd.get_bool(p[BITS + 3]) {
            x += 8;
        }
    } else {
        x = bd.read_tree(&SMALL_MVTREE, &p[MVPSHORT..]);
    }
    // The sign bool is read only for a non-zero magnitude (line 10091).
    if x != 0 && bd.get_bool(p[MVPSIGN]) {
        x = -x;
    }
    x << 1
}

/// dixie `read_mv` (lines 10174-10181): row component first (`mvc[0]`), then column
/// (`mvc[1]`).
fn reference_read_mv(bd: &mut BoolDecoder<'_>, mvc: &[[u8; 19]; 2]) -> CodecResult<(i16, i16)> {
    let row = reference_read_mv_component(bd, &mvc[0]);
    let col = reference_read_mv_component(bd, &mvc[1]);
    Ok((row as i16, col as i16))
}

/// A reader that must never fire: used by tests whose bitstreams contain no NEWMV/NEW4X4
/// macroblock, so an unexpected call fails loudly instead of desyncing silently.
fn forbidden_read_mv(_: &mut BoolDecoder<'_>, _: &[[u8; 19]; 2]) -> CodecResult<(i16, i16)> {
    Err(CodecError::Internal(
        "read_mv called by a test that expects no explicitly-coded MV".to_string(),
    ))
}

// -- builders ---------------------------------------------------------

/// A minimal inter-frame header: no segmentation, no skip flag, and the caller's
/// intra/last/golden probabilities.
fn test_header(prob_intra: u8, prob_last: u8, prob_gf: u8) -> Vp8Header {
    Vp8Header {
        width: 96,
        height: 64,
        version: 0,
        horizontal_scale: 0,
        vertical_scale: 0,
        color_space: 0,
        clamping_required: true,
        segment: SegmentHeader::default(),
        loop_filter: LoopFilterHeader::default(),
        quant: QuantHeader::default(),
        coeff_probs: tables::DEFAULT_COEFF_PROBS,
        mb_no_skip_coeff: false,
        prob_skip_false: 0,
        partitions_start: 10,
        first_partition_size: 1,
        num_token_partitions: 1,
        is_keyframe: false,
        inter: InterFrameHeader {
            prob_intra,
            prob_last,
            prob_gf,
            ..InterFrameHeader::default()
        },
    }
}

/// An inter record with sub-vectors replicated — the shape [`find_near_mvs`] sees for a
/// whole-macroblock mode.
fn inter_mb(ref_frame: RefFrame, mode: InterMode, mv: Mv) -> InterMbInfo {
    InterMbInfo {
        segment_id: 0,
        skip_coeff: false,
        is_inter: true,
        ref_frame,
        mode: MbMode::Inter {
            mode,
            mv,
            sub: None,
        },
        sub_mvs: [mv; 16],
        has_y2: mode != InterMode::Split,
    }
}

fn no_sign_bias() -> SignBias {
    SignBias {
        golden: false,
        altref: false,
    }
}

/// The only macroblock of a 1x1 frame: every neighbour is the border.
const MB0: MbPosition = MbPosition {
    mb_col: 0,
    mb_row: 0,
    mb_cols: 1,
    mb_rows: 1,
};

/// Column 1, row 1 of a 2x2 frame: left and above are real records, the above-left is the
/// border.
const MB11: MbPosition = MbPosition {
    mb_col: 1,
    mb_row: 1,
    mb_cols: 2,
    mb_rows: 2,
};

/// Decodes one record, returning it with the bool decoder's final byte position (a desync
/// detector in its own right).
fn decode_at(
    bytes: &[u8],
    header: &Vp8Header,
    state: &mut Vp8State,
    ctx: &ModeRowCtx,
    pos: MbPosition,
    read_mv: MvReadFn<'_>,
) -> (InterMbInfo, usize) {
    let mut bd = BoolDecoder::new(bytes);
    let info = read_mb_mode_record(&mut bd, header, state, ctx, pos, read_mv)
        .expect("synthetic record must decode");
    (info, bd.position())
}

/// [`decode_at`] for a lone macroblock in a 1x1 frame.
fn decode_solo_with(
    bytes: &[u8],
    header: &Vp8Header,
    state: &mut Vp8State,
) -> (InterMbInfo, usize) {
    state.ensure_segment_map_size(1);
    let ctx = ModeRowCtx::new(1);
    decode_at(bytes, header, state, &ctx, MB0, &mut forbidden_read_mv)
}

/// [`decode_solo_with`] against fresh default state.
fn decode_solo(bytes: &[u8], header: &Vp8Header) -> (InterMbInfo, usize) {
    decode_solo_with(bytes, header, &mut Vp8State::new())
}

/// The tail every synthetic record that is *not* about the prediction mode ends with: intra,
/// DC_PRED luma, DC_PRED chroma, defaults.
fn write_intra_dc(e: &mut TestBoolEncoder) {
    e.write_bool(1, false); // prob_intra = 1 -> `false` selects INTRA
    e.write_tree(&YMODE_TREE, &DEFAULT_YMODE_PROB, DC_PRED as i32);
    e.write_tree(&UV_MODE_TREE, &DEFAULT_UV_MODE_PROB, DC_PRED as i32);
}

// 1. THE HARD GATE: p5basic.frame1.bin, every macroblock record
//
// Seeding: `Vp8State::new()` is not an approximation of "what p5basic's key frame left
// behind", it is *exactly* that state for every probability a mode record reads. RFC 6386
// §19.2 codes the ymode / uv / MV probability updates inside `if (!key_frame)` (lines
// 6856-6870) and §16.1 line 5455 requires the ymode/uv defaults to "be restored after
// detection of a key frame", so no key frame can leave those tables anywhere but at their
// defaults; the one table a key frame *does* update persistently, `coeff_probs`, drives the
// token partitions and is never read by a mode record. p5basic.frame0.bin is therefore not
// needed — fortunately, since only frames 1 and 2 are checked in (testdata/README).

/// Parses the fixture header and decodes all 24 (6x4) records.
fn decode_p5basic_frame1() -> (Vec<InterMbInfo>, usize, usize) {
    let data = include_bytes!("testdata/p5basic.frame1.bin");
    let mut state = Vp8State::new();
    let (header, mut bd) = Vp8Header::parse_interframe(data, 96, 64, &mut state)
        .expect("p5basic.frame1.bin must parse as a valid inter-frame header");
    let records = read_all_mb_mode_records(
        &mut bd,
        &header,
        &mut state,
        96usize.div_ceil(16),
        64usize.div_ceil(16),
        &mut reference_read_mv,
    )
    .expect("every macroblock record of p5basic.frame1 must decode");
    (records, bd.position(), header.first_partition_size)
}

/// [`decode_p5basic_frame1`] driven by the **production** motion-vector
/// reader — the exact injection `dec::inter` performs — instead of this
/// file's transcribed-from-dixie oracle.
fn decode_p5basic_frame1_with_production_mv() -> Vec<InterMbInfo> {
    let data = include_bytes!("testdata/p5basic.frame1.bin");
    let mut state = Vp8State::new();
    let (header, mut bd) = Vp8Header::parse_interframe(data, 96, 64, &mut state)
        .expect("p5basic.frame1.bin must parse as a valid inter-frame header");
    read_all_mb_mode_records(
        &mut bd,
        &header,
        &mut state,
        96usize.div_ceil(16),
        64usize.div_ceil(16),
        &mut |bd, probs| super::super::mv::read_mv(bd, probs).map(|m| (m.row, m.col)),
    )
    .expect("every macroblock record must decode with the production reader")
}

#[test]
fn test_production_mv_reader_matches_the_reference_transcription() {
    // The injected-reader seam, closed on real bitstream: P3's `mv::read_mv`
    // (the one `dec::inter` injects) and this file's independent dixie
    // transcription must produce byte-identical records, including the
    // `(row, col)` order and the eighth-pel doubling. A swap or a missing
    // `<< 1` in either would show up here rather than as wrong pixels.
    let (reference, _, _) = decode_p5basic_frame1();
    let production = decode_p5basic_frame1_with_production_mv();
    assert_eq!(production, reference);
}

#[test]
fn test_p5basic_frame1_records_are_wellformed() {
    let (records, _, _) = decode_p5basic_frame1();
    assert_eq!(records.len(), 24, "96x64 == 6x4 macroblocks");
    for (i, r) in records.iter().enumerate() {
        // p5basic is a LAST-only stream (testdata/README.md) and its frame-1 header carries
        // prob_last=255, so `Bool(prob_last)` is false ~255/256 of the time — LAST under the
        // §16.2 polarity. An inverted polarity would report golden/altref here.
        assert!(
            matches!(r.ref_frame, RefFrame::Intra | RefFrame::Last),
            "mb {i}: p5basic references only LAST (or intra), got {:?}",
            r.ref_frame
        );
        assert_eq!(r.is_inter, r.ref_frame != RefFrame::Intra, "mb {i}");
        match r.mode {
            MbMode::Intra { ymode, bmodes, .. } => {
                assert!(usize::from(ymode) <= B_PRED, "mb {i}: ymode {ymode}");
                assert_eq!(bmodes.is_some(), usize::from(ymode) == B_PRED);
                assert_eq!(r.sub_mvs, [ZERO_MV; 16], "mb {i}: intra MB has zero MVs");
                assert_eq!(r.has_y2, usize::from(ymode) != B_PRED);
            }
            MbMode::Inter { mode, mv, sub } => {
                assert_eq!(r.has_y2, mode != InterMode::Split);
                let Some(split) = sub else {
                    assert_ne!(mode, InterMode::Split);
                    assert_eq!(r.sub_mvs, [mv; 16], "mb {i}: whole-MB mode replicates");
                    continue;
                };
                // A split MB's vector is its last sub-block's, and every
                // sub-block of one partition shares one vector/sub-mode.
                assert_eq!(mode, InterMode::Split);
                assert_eq!(mv, r.sub_mvs[15], "mb {i}: split MB mv is sub_mvs[15]");
                let map = &MBSPLITS[usize::from(split.partitioning)];
                for (slot, &p) in map.iter().enumerate() {
                    let first = map.iter().position(|&q| q == p).unwrap_or(slot);
                    assert_eq!(r.sub_mvs[slot], r.sub_mvs[first], "one partition, one MV");
                    assert_eq!(split.sub_modes[slot], split.sub_modes[first]);
                }
            }
        }
    }
}

// 2. Golden mode dump — SELF-GENERATED, NOT EXTERNALLY VERIFIED.
//
// Produced by this very decoder and checked in to pin determinism and make any behavioural
// change visible in a diff. It is *not* evidence of bit-exactness against libvpx; external
// truth arrives at P6, which decodes the fixture set to pixels against the shipped
// `.ref.yuv`. What does carry independent weight today is the exact first-partition
// consumption asserted below.

/// One token per macroblock, one line per macroblock row: `<ref><mode>[k](<row>,<col>)` where
/// ref is `I`/`L`/`G`/`A`, mode is `D`/`V`/`H`/`T`/`B` (intra ymode) or `S`/`N`/`Z`/`W`/`X`
/// (nearest/near/zero/new/split), `k` marks `skip_coeff`, and the pair is the macroblock
/// motion vector in eighth-pel `(row, col)`.
fn dump_records(records: &[InterMbInfo], mb_cols: usize) -> String {
    let mut out = String::new();
    for (i, r) in records.iter().enumerate() {
        if i % mb_cols == 0 {
            if i > 0 {
                out.push('\n');
            }
        } else {
            out.push(' ');
        }
        let refc = ['I', 'L', 'G', 'A'][r.ref_frame as usize];
        let modec = match r.mode {
            // `DC/V/H/TM/B_PRED` for intra, `NEAREST/NEAR/ZERO/NEW/SPLIT` for inter.
            MbMode::Intra { ymode, .. } => {
                *b"DVHTB".get(usize::from(ymode)).map_or(&b'?', |c| c) as char
            }
            MbMode::Inter { mode, .. } => b"SNZWX"[mode as usize] as char,
        };
        let mv = r.mv();
        out.push(refc);
        out.push(modec);
        if r.skip_coeff {
            out.push('k');
        }
        out.push_str(&format!("({},{})", mv.0, mv.1));
    }
    out
}

/// Golden dump of `p5basic.frame1.bin`; see the section comment above for what it does and
/// does not prove.
const P5BASIC_FRAME1_DUMP: &str = "\
LX(0,0) LZ(0,0) LZ(0,0) LZk(0,0) LZk(0,0) LX(0,0)
LX(0,0) LX(0,0) LW(0,16) LX(0,0) LZ(0,0) LX(-16,-32)
LZk(0,0) LXk(0,-16) LZk(0,0) LX(0,0) LX(0,0) LW(0,16)
LZk(0,0) LXk(0,-16) LZk(0,0) LZk(0,0) LZk(0,0) LZk(0,0)";

/// Bytes consumed from the first partition after all 24 records. This equals
/// `first_partition_size` **exactly**: the record stream ends precisely on the partition
/// boundary, with neither unread bytes nor the §7.3 two-byte overrun — the strongest evidence
/// available before P6's pixel-exact comparison that this decoder reads the right number of
/// bools in the right order.
const P5BASIC_FRAME1_END_POSITION: usize = 63;

#[test]
fn test_p5basic_frame1_matches_golden_dump() {
    // The exact-consumption assertion is the acceptance test with real teeth: the first
    // partition holds the frame header followed by exactly the mode records and nothing else,
    // so any field-order or tree/probability error consumes a different number of bools and
    // lands the decoder somewhere else entirely.
    let (records, position, size) = decode_p5basic_frame1();
    assert_eq!(dump_records(&records, 6), P5BASIC_FRAME1_DUMP);
    assert_eq!(position, P5BASIC_FRAME1_END_POSITION);
    assert_eq!(
        position, size,
        "record stream ends on the partition boundary"
    );
    // Deterministic across runs.
    let (again, position2, _) = decode_p5basic_frame1();
    assert_eq!(dump_records(&again, 6), P5BASIC_FRAME1_DUMP);
    assert_eq!(position2, position);
}

#[test]
fn test_p5basic_frame1_mode_coverage() {
    // What the fixture actually exercises, pinned so a future fixture swap cannot silently
    // narrow it. The SPLITMV macroblocks drive `decode_split_mv` (partition tree, sub-mode
    // contexts, MBSPLITS fill) and the NEWMV ones drive the injected §17.2 reader, both
    // against real encoder output; no intra, NEARESTMV or NEARMV macroblock occurs here, so
    // those paths rest on the synthetic tests below.
    let (records, _, _) = decode_p5basic_frame1();
    let count = |want: InterMode| {
        records
            .iter()
            .filter(|r| matches!(r.mode, MbMode::Inter { mode, .. } if mode == want))
            .count()
    };
    assert_eq!(count(InterMode::Zero), 12);
    assert_eq!(count(InterMode::Split), 10);
    assert_eq!(count(InterMode::New), 2);
    assert_eq!(records.iter().filter(|r| !r.is_inter).count(), 0);
}

// 3. Intra-in-inter: frame-level tree AND frame-level probabilities

#[test]
fn test_intra_in_inter_uses_frame_ymode_tree_not_keyframe_tree() {
    // Discriminates on both axes at once (RFC 6386 §16.1 lines 5403-5426). TREE: a leading
    // `0` bit decodes to DC_PRED under YMODE_TREE but to B_PRED under KF_YMODE_TREE (which
    // puts B_PRED first), and a kf-tree implementation would go on to read sixteen sub-mode
    // symbols, desynchronising everything after it. PROBABILITIES: the sentinel shares no
    // value with KF_YMODE_PROB, so a right-tree/wrong-probs implementation decodes different
    // bits and ends at a different byte.
    let ymode_sentinel = [3u8, 250, 7, 200];
    let uv_sentinel = [11u8, 240, 9];
    assert_ne!(ymode_sentinel, tables::KF_YMODE_PROB);
    let mut e = TestBoolEncoder::new();
    e.write_bool(1, false); // prob_intra = 1 -> `false` selects INTRA
    e.write_tree(&YMODE_TREE, &ymode_sentinel, DC_PRED as i32);
    e.write_tree(&UV_MODE_TREE, &uv_sentinel, tables::V_PRED as i32);
    let bytes = e.finish();
    let mut state = Vp8State::new();
    state.entropy.ymode_prob = ymode_sentinel;
    state.entropy.uv_mode_prob = uv_sentinel;
    let (info, position) = decode_solo_with(&bytes, &test_header(1, 128, 128), &mut state);
    assert!(!info.is_inter);
    assert_eq!(info.ref_frame, RefFrame::Intra);
    assert_eq!(
        info.mode,
        MbMode::Intra {
            ymode: DC_PRED as u8,
            bmodes: None,
            uv_mode: tables::V_PRED as u8,
        },
        "a KF_YMODE_TREE implementation decodes B_PRED here instead"
    );
    assert!(info.has_y2);
    assert_eq!(position, 3, "pinned consumption of the synthetic record");
}

#[test]
fn test_intra_in_inter_bpred_submodes_are_context_free() {
    // RFC 6386 §16.1 lines 5429-5440: sixteen sub-modes, all driven by the one context-free
    // BMODE_PROB table. Encoding sixteen *different* sub-modes with that table and getting
    // all sixteen back proves no context indexing crept in.
    let submodes: [usize; 16] = core::array::from_fn(|i| i % 10);
    let mut e = TestBoolEncoder::new();
    e.write_bool(1, false); // intra
    e.write_tree(&YMODE_TREE, &DEFAULT_YMODE_PROB, B_PRED as i32);
    for &m in &submodes {
        e.write_tree(&BMODE_TREE, &BMODE_PROB, m as i32);
    }
    e.write_tree(&UV_MODE_TREE, &DEFAULT_UV_MODE_PROB, tables::TM_PRED as i32);
    let bytes = e.finish();
    let (info, _) = decode_solo(&bytes, &test_header(1, 128, 128));
    let expected: [u8; 16] = core::array::from_fn(|i| submodes[i] as u8);
    assert_eq!(
        info.mode,
        MbMode::Intra {
            ymode: B_PRED as u8,
            bmodes: Some(expected),
            uv_mode: tables::TM_PRED as u8,
        }
    );
    assert!(!info.has_y2, "B_PRED codes no Y2 block");
    // All ten sub-modes appear, i.e. the whole BMODE_TREE was walked.
    let mut seen = expected.to_vec();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 10, "every BMODE_TREE leaf must round trip");
}

// --- 4. §16.3 near-MV search -----------------------------------------

#[test]
fn test_find_near_mvs_all_intra_neighbours_gives_zero_census() {
    let b = InterMbInfo::border();
    let (near_mvs, cnt) = find_near_mvs(RefFrame::Last, &b, &b, &b, &no_sign_bias());
    assert_eq!(cnt, [0, 0, 0, 0], "intra neighbours contribute nothing");
    assert_eq!(near_mvs, [ZERO_MV; 4]);
    assert_eq!(mv_ref_probs(&cnt), MODE_CONTEXTS[0]);
}

#[test]
fn test_find_near_mvs_weights_are_two_two_one() {
    // Three distinct non-zero neighbours: above weighs 2, left 2, above-left 1 (RFC 6386
    // §16.3 lines 5574-5576).
    let above = inter_mb(RefFrame::Last, InterMode::New, (8, 0));
    let left = inter_mb(RefFrame::Last, InterMode::New, (0, 8));
    let al = inter_mb(RefFrame::Last, InterMode::New, (-8, -8));
    let (near_mvs, cnt) = find_near_mvs(RefFrame::Last, &left, &above, &al, &no_sign_bias());
    assert_eq!(near_mvs[CNT_NEAREST], (8, 0));
    assert_eq!(near_mvs[CNT_NEAR], (0, 8));
    assert_eq!(near_mvs[3], (-8, -8));
    // cnt[3] carried the above-left weight, then was overwritten by the split census (no
    // neighbour is split here).
    assert_eq!(cnt, [0, 2, 2, 0]);
}

#[test]
fn test_find_near_mvs_dedupes_against_most_recent_only() {
    // above = A, left = B, above-left = A. The dedupe compares only against the most recently
    // entered vector (B), so A enters twice — slots 1 and 3 — and the "three distinct MVs"
    // merge then folds the slot-3 copy back into NEAREST's count.
    let (a, b) = ((16i16, 16i16), (-16i16, 0i16));
    let above = inter_mb(RefFrame::Last, InterMode::New, a);
    let left = inter_mb(RefFrame::Last, InterMode::New, b);
    let al = inter_mb(RefFrame::Last, InterMode::New, a);
    let (near_mvs, cnt) = find_near_mvs(RefFrame::Last, &left, &above, &al, &no_sign_bias());
    assert_eq!(near_mvs[CNT_NEAREST], a);
    assert_eq!(near_mvs[CNT_NEAR], b);
    assert_eq!(near_mvs[3], a, "A was entered a second time, not merged");
    assert_eq!(cnt[CNT_NEAREST], 3, "2 from above + 1 from the merge");
    assert_eq!(cnt[CNT_NEAR], 2);
    assert_eq!(
        near_mvs[CNT_ZERO], a,
        "best = nearest when cnt[1] >= cnt[0]"
    );
}

#[test]
fn test_find_near_mvs_accumulates_identical_and_zero_neighbours() {
    // One shared non-zero vector accumulates 2 + 2 + 1 = 5, the maximum RFC 6386 line 5633
    // states is possible.
    let mv = (24i16, -8i16);
    let n = inter_mb(RefFrame::Last, InterMode::New, mv);
    let (near_mvs, cnt) = find_near_mvs(RefFrame::Last, &n, &n, &n, &no_sign_bias());
    assert_eq!((cnt[CNT_NEAREST], cnt[CNT_ZERO]), (5, 0));
    assert_eq!(near_mvs[CNT_NEAREST], mv);
    assert_eq!(near_mvs[CNT_NEAR], ZERO_MV);
    assert_eq!(near_mvs[CNT_ZERO], mv, "best_mv");
    assert!(cnt.iter().all(|&c| c <= 5), "no weight may exceed 5");

    // Inter neighbours with a *zero* vector never enter the list; their weight lands in
    // cnt[CNT_ZERO] instead (2 + 2 + 1).
    let z = inter_mb(RefFrame::Last, InterMode::Zero, ZERO_MV);
    let (zero_mvs, zero_cnt) = find_near_mvs(RefFrame::Last, &z, &z, &z, &no_sign_bias());
    assert_eq!((zero_cnt[CNT_ZERO], zero_cnt[CNT_NEAREST]), (5, 0));
    assert_eq!(
        zero_mvs[CNT_ZERO], ZERO_MV,
        "best stays zero: 0 >= 5 is false"
    );
}

#[test]
fn test_find_near_mvs_sign_bias_inverts_disagreeing_neighbour() {
    // RFC 6386 §16.3 `mv_bias` (lines 5580-5593): golden has sign bias set, LAST never does,
    // so a golden-referencing neighbour surveyed by a LAST-referencing macroblock is negated.
    let sb = SignBias {
        golden: true,
        altref: false,
    };
    let above = inter_mb(RefFrame::Golden, InterMode::New, (12, -20));
    let b = InterMbInfo::border();
    let (near_mvs, cnt) = find_near_mvs(RefFrame::Last, &b, &above, &b, &sb);
    assert_eq!(near_mvs[CNT_NEAREST], (-12, 20), "negated");
    assert_eq!(cnt[CNT_NEAREST], 2);

    // Same reference on both sides: no inversion.
    let (same, _) = find_near_mvs(RefFrame::Golden, &b, &above, &b, &sb);
    assert_eq!(same[CNT_NEAREST], (12, -20));

    // Two different references that *agree* on bias: no inversion either.
    let both = SignBias {
        golden: true,
        altref: true,
    };
    let alt = inter_mb(RefFrame::AltRef, InterMode::New, (12, -20));
    let (agree, _) = find_near_mvs(RefFrame::Golden, &b, &alt, &b, &both);
    assert_eq!(agree[CNT_NEAREST], (12, -20));
}

#[test]
fn test_find_near_mvs_swaps_near_and_nearest_by_weight() {
    // above contributes a vector with weight 2; left a different one, weight 2; above-left
    // repeats left's, adding 1 — so "near" outweighs "nearest" and the two are swapped (lines
    // 5740-5749).
    let (a, b) = ((8i16, 8i16), (-8i16, -8i16));
    let above = inter_mb(RefFrame::Last, InterMode::New, a);
    let left = inter_mb(RefFrame::Last, InterMode::New, b);
    let al = inter_mb(RefFrame::Last, InterMode::New, b);
    let (near_mvs, cnt) = find_near_mvs(RefFrame::Last, &left, &above, &al, &no_sign_bias());
    assert_eq!(cnt[CNT_NEAREST], 3, "b's weight after the swap");
    assert_eq!(cnt[CNT_NEAR], 2);
    assert_eq!(near_mvs[CNT_NEAREST], b);
    assert_eq!(near_mvs[CNT_NEAR], a);
}

#[test]
fn test_find_near_mvs_split_census_counts_split_neighbours() {
    // cnt[CNT_SPLITMV] = (above_is_split + left_is_split) * 2 + above_left_is_split (lines
    // 5735-5738).
    let split = inter_mb(RefFrame::Last, InterMode::Split, ZERO_MV);
    let plain = inter_mb(RefFrame::Last, InterMode::Zero, ZERO_MV);
    let border = InterMbInfo::border();
    for (above, left, al, expected) in [
        (&split, &split, &split, 5),
        (&split, &plain, &border, 2),
        (&plain, &split, &split, 3),
        (&plain, &plain, &border, 0),
    ] {
        let (_, cnt) = find_near_mvs(RefFrame::Last, left, above, al, &no_sign_bias());
        assert_eq!(cnt[CNT_SPLITMV], expected);
    }
}

#[test]
fn test_mv_ref_probs_indexes_each_node_independently() {
    // RFC 6386 lines 5794-5803: p[i] = mode_contexts[cnt[i]][i].
    let cnt = [1usize, 2, 3, 4];
    let expected = [
        MODE_CONTEXTS[1][0],
        MODE_CONTEXTS[2][1],
        MODE_CONTEXTS[3][2],
        MODE_CONTEXTS[4][3],
    ];
    assert_eq!(mv_ref_probs(&cnt), expected);
    // A "one shared row" implementation would give MODE_CONTEXTS[1].
    assert_ne!(mv_ref_probs(&cnt), MODE_CONTEXTS[1]);
}

// --- 5. Clamping — including the row/column transposition discriminator ---

#[test]
fn test_clamp_rect_matches_both_reference_formulations() {
    // dixie lines 10598-10601 against the §16.3 `vp8_clamp_mv` form (lines 5609-5620): the
    // same numbers by two routes.
    let r = MvClampRect::for_mb(2, 1, 6, 4);
    assert_eq!(r.to_left, -((2 + 1) << 7));
    assert_eq!(r.to_right, (6 - 2) << 7);
    assert_eq!(r.to_top, -((1 + 1) << 7));
    assert_eq!(r.to_bottom, (4 - 1) << 7);
    // §16.3 form: mb_to_*_edge +/- a 16-pixel (16 << 3) margin.
    assert_eq!(r.to_left, -((2 * 16) << 3) - (16 << 3));
    assert_eq!(r.to_right, (((6 - 1 - 2) * 16) << 3) + (16 << 3));
    assert_eq!(r.to_top, -((1 * 16) << 3) - (16 << 3));
    assert_eq!(r.to_bottom, (((4 - 1 - 1) * 16) << 3) + (16 << 3));
}

#[test]
fn test_clamp_is_not_transposed_and_handles_edges() {
    // The discriminator for a row/column swap: at macroblock (0, 0) of a 6x4 frame the
    // vertical bound (to_bottom = 512) is reached and the horizontal one (to_right = 768) is
    // not, so a transposed implementation clamps the wrong component.
    let r = MvClampRect::for_mb(0, 0, 6, 4);
    assert_eq!(r.clamp((600, 600)), (512, 600));
    // Mirror on the negative side, where to_top and to_left differ.
    let r2 = MvClampRect::for_mb(3, 0, 6, 4);
    assert_eq!((r2.to_top, r2.to_left), (-128, -512));
    assert_eq!(r2.clamp((-300, -300)), (-128, -300));
    let inner = MvClampRect::for_mb(2, 2, 6, 4);
    for mv in [(0i16, 0i16), (100, -100), (-255, 255)] {
        assert_eq!(inner.clamp(mv), mv, "in-range vectors are untouched");
    }
    // A wide frame's bounds exceed i16 range; both reference decoders keep them in `int`, so
    // nothing is clamped.
    let wide = MvClampRect::for_mb(0, 0, 1024, 1024);
    assert!(wide.to_right > i32::from(i16::MAX));
    assert_eq!(wide.clamp((i16::MAX, i16::MAX)), (i16::MAX, i16::MAX));
}

#[test]
fn test_nearest_and_near_are_stored_clamped() {
    // dixie lines 10498/10501 store the *clamped* candidate, which is what a later
    // macroblock's neighbour survey then sees.
    let far = (10_000i16, 0i16);
    let above = inter_mb(RefFrame::Last, InterMode::New, far);
    let header = test_header(1, 1, 128);
    let mut e = TestBoolEncoder::new();
    e.write_bool(1, true); // inter
    e.write_bool(1, false); // prob_last: false -> LAST
                            // cnt = [0, 2, 0, 0] for a single non-zero above neighbour.
    e.write_tree(&MV_REF_TREE, &mv_ref_probs(&[0, 2, 0, 0]), NEARESTMV as i32);
    let bytes = e.finish();

    // Seed the previous row's column 1, then start a fresh row: (1, 1) sees the far-off
    // record as `above`, borders left and above-left.
    let mut state = Vp8State::new();
    state.ensure_segment_map_size(4);
    let mut ctx = ModeRowCtx::new(2);
    ctx.advance(1, &above);
    ctx.start_row();
    let (info, _) = decode_at(
        &bytes,
        &header,
        &mut state,
        &ctx,
        MB11,
        &mut forbidden_read_mv,
    );
    let bounds = MvClampRect::for_mb(1, 1, 2, 2);
    assert_eq!(info.mv(), bounds.clamp(far));
    assert_eq!(info.mv(), (bounds.to_bottom as i16, 0));
    assert_eq!(info.sub_mvs, [info.mv(); 16]);
}

#[test]
fn test_newmv_is_stored_unclamped_on_top_of_a_clamped_base() {
    // dixie lines 10507-10512: the *base* is clamped, the sum is not. libvpx agrees,
    // deferring the check to motion compensation via `need_to_clamp_mvs` rather than altering
    // the stored vector.
    let far = (10_000i16, 0i16);
    let above = inter_mb(RefFrame::Last, InterMode::New, far);
    let header = test_header(1, 1, 128);
    let mv_probs = DEFAULT_MV_CONTEXT;
    let mut e = TestBoolEncoder::new();
    e.write_bool(1, true); // inter
    e.write_bool(1, false); // LAST
    e.write_tree(&MV_REF_TREE, &mv_ref_probs(&[0, 2, 0, 0]), NEWMV as i32);
    e.write_mv_component(&mv_probs[0], 3); // row delta: +3 quarter-pel
    e.write_mv_component(&mv_probs[1], -2); // col delta: -2 quarter-pel
    let bytes = e.finish();
    let mut state = Vp8State::new();
    state.ensure_segment_map_size(4);
    let mut ctx = ModeRowCtx::new(2);
    ctx.advance(1, &above);
    ctx.start_row();
    let (info, _) = decode_at(
        &bytes,
        &header,
        &mut state,
        &ctx,
        MB11,
        &mut reference_read_mv,
    );
    let bounds = MvClampRect::for_mb(1, 1, 2, 2);
    let clamped_best = bounds.clamp(far);
    // Deltas are doubled into eighth-pel by read_mv_component.
    let expected = (clamped_best.0 + 6, clamped_best.1 - 4);
    assert_eq!(info.mv(), expected);
    assert!(
        expected.0 > bounds.to_bottom as i16,
        "the sum must be allowed to exceed the clamp rectangle"
    );
}

// --- 6. SPLITMV ------------------------------------------------------

/// Builds a synthetic one-macroblock SPLITMV stream at [`MB11`]: `partition_id` plus one
/// `(sub_mode, optional quarter-pel delta)` per partition, encoded with the contexts a
/// correct decoder computes. Returns the bytes and the `clamp_mv(near_mvs[BEST])` that
/// `NEW4X4` offsets from — both derived from the *same* neighbour records the decoder will
/// survey, since the `mv_ref` probabilities depend on that survey (which has its own
/// dedicated tests above; here the two sides only have to agree).
fn encode_splitmv(
    partition_id: usize,
    parts: &[(usize, Option<(i32, i32)>)],
    left: &InterMbInfo,
    above: &InterMbInfo,
) -> (Vec<u8>, Mv) {
    let mv_probs = DEFAULT_MV_CONTEXT;
    let border = InterMbInfo::border();
    let (near_mvs, cnt) = find_near_mvs(RefFrame::Last, left, above, &border, &no_sign_bias());
    let bounds = MvClampRect::for_mb(MB11.mb_col, MB11.mb_row, MB11.mb_cols, MB11.mb_rows);
    let best = bounds.clamp(near_mvs[CNT_ZERO]);
    let mut e = TestBoolEncoder::new();
    e.write_bool(1, true); // inter
    e.write_bool(1, false); // LAST
    e.write_tree(&MV_REF_TREE, &mv_ref_probs(&cnt), SPLITMV as i32);
    e.write_tree(&MBSPLIT_TREE, &MBSPLIT_PROBS, partition_id as i32);

    // Mirror the decoder's own sub-block bookkeeping so the contexts (which depend on
    // already-decoded sub-vectors) match.
    let map = &MBSPLITS[partition_id];
    let mut sub_mvs = [ZERO_MV; 16];
    for (j, &(sub_mode, delta)) in parts.iter().enumerate() {
        let k = map.iter().position(|&p| p == j).expect("partition exists");
        let left_mv = left_block_mv(&sub_mvs, left, k);
        let above_mv = above_block_mv(&sub_mvs, above, k);
        let ctx = sub_mv_ref_context(left_mv == ZERO_MV, above_mv == ZERO_MV, left_mv == above_mv);
        e.write_tree(&SUB_MV_REF_TREE, &SUB_MV_REF_PROBS2[ctx], sub_mode as i32);
        let mv = if sub_mode == LEFT4X4 {
            left_mv
        } else if sub_mode == ABOVE4X4 {
            above_mv
        } else if sub_mode == ZERO4X4 {
            ZERO_MV
        } else {
            let (dr, dc) = delta.expect("NEW4X4 needs a delta");
            e.write_mv_component(&mv_probs[0], dr);
            e.write_mv_component(&mv_probs[1], dc);
            ((dr * 2) as i16 + best.0, (dc * 2) as i16 + best.1)
        };
        for (slot, &p) in map.iter().enumerate() {
            if p == j {
                sub_mvs[slot] = mv;
            }
        }
    }
    (e.finish(), best)
}

/// Decodes a synthetic SPLITMV macroblock at [`MB11`] with the supplied left/above
/// neighbours.
fn decode_splitmv(bytes: &[u8], left: &InterMbInfo, above: &InterMbInfo) -> InterMbInfo {
    let header = test_header(1, 1, 128);
    let mut state = Vp8State::new();
    state.ensure_segment_map_size(4);
    let mut ctx = ModeRowCtx::new(2);
    ctx.advance(1, above); // previous row, column 1 -> "above" of (1, 1)
    ctx.start_row();
    ctx.advance(0, left); // current row, column 0 -> "left" of (1, 1)
    decode_at(
        bytes,
        &header,
        &mut state,
        &ctx,
        MB11,
        &mut reference_read_mv,
    )
    .0
}

#[test]
fn test_splitmv_every_partition_shape_fills_all_sixteen_slots() {
    let border = InterMbInfo::border();
    for &partition_id in &[MBSPLIT_16X8, MBSPLIT_8X16, MBSPLIT_8X8, MBSPLIT_4X4] {
        let n = NUM_MBSPLIT_PARTS[partition_id];
        // A distinct NEW4X4 delta per partition makes the fill map observable slot by slot.
        let parts: Vec<(usize, Option<(i32, i32)>)> =
            (0..n).map(|j| (NEW4X4, Some((j as i32 % 8, 0)))).collect();
        let (bytes, best) = encode_splitmv(partition_id, &parts, &border, &border);
        assert_eq!(best, ZERO_MV, "border neighbours give a zero best_mv");
        let info = decode_splitmv(&bytes, &border, &border);
        let split = match info.mode {
            MbMode::Inter {
                mode: InterMode::Split,
                sub: Some(s),
                ..
            } => s,
            other => panic!("expected SPLITMV, got {other:?}"),
        };
        assert_eq!(usize::from(split.partitioning), partition_id);
        assert_eq!(usize::from(split.num_parts), n);
        assert!(!info.has_y2, "SPLITMV codes no Y2 block");
        // Each sub-block carries its partition's delta doubled into eighth-pel — i.e. the
        // MBSPLITS fill map, exactly.
        for (slot, &p) in MBSPLITS[partition_id].iter().enumerate() {
            let expect = ((p as i32 % 8 * 2) as i16, 0i16);
            assert_eq!(
                info.sub_mvs[slot], expect,
                "shape {partition_id}, slot {slot}"
            );
            assert_eq!(usize::from(split.sub_modes[slot]), NEW4X4);
        }
        assert_eq!(info.mv(), info.sub_mvs[15], "MB mv is the last sub-block's");
    }
}

#[test]
fn test_splitmv_every_sub_mode() {
    // 8x8 quadrants, one of each sub-mode: LEFT4X4 copies the left neighbour's rightmost
    // column, ABOVE4X4 the above neighbour's bottom row, ZERO4X4 is zero, NEW4X4 is delta +
    // best.
    let left = inter_mb(RefFrame::Last, InterMode::New, (16, 16));
    let above = inter_mb(RefFrame::Last, InterMode::New, (-16, 32));
    let parts = [
        (LEFT4X4, None),
        (ABOVE4X4, None),
        (ZERO4X4, None),
        (NEW4X4, Some((5, -3))),
    ];
    let (bytes, best) = encode_splitmv(MBSPLIT_8X8, &parts, &left, &above);
    let info = decode_splitmv(&bytes, &left, &above);
    let map = &MBSPLITS[MBSPLIT_8X8];
    let expect = [
        (16i16, 16i16),             // partition 0: from the left MB
        (-16, 32),                  // partition 1: from the above MB
        (0, 0),                     // partition 2: zero
        (10 + best.0, -6 + best.1), // partition 3: delta + best_mv
    ];
    let split = match info.mode {
        MbMode::Inter { sub: Some(s), .. } => s,
        other => panic!("expected SPLITMV, got {other:?}"),
    };
    for (slot, &p) in map.iter().enumerate() {
        assert_eq!(info.sub_mvs[slot], expect[p], "sub-block {slot}");
        assert_eq!(usize::from(split.sub_modes[slot]), parts[p].0);
    }
    assert_eq!(info.mv(), info.sub_mvs[15]);
}

#[test]
fn test_splitmv_left_and_above_operands_come_from_within_the_macroblock() {
    // 4x4 split: sub-block 1's "left" operand is *this* macroblock's sub-block 0 (dixie
    // `left_block_mv`, line 10119: `this->split.mvs [b-1]`) and sub-block 4's "above" operand
    // is sub-block 0 (line 10106: `this->split.mvs[b-4]`), so partitions 1 and 4 coded as
    // LEFT4X4/ABOVE4X4 must reproduce partition 0's NEW4X4 vector.
    let border = InterMbInfo::border();
    let mut parts: Vec<(usize, Option<(i32, i32)>)> = vec![(ZERO4X4, None); 16];
    parts[0] = (NEW4X4, Some((7, 6)));
    parts[1] = (LEFT4X4, None);
    parts[4] = (ABOVE4X4, None);
    let (bytes, best) = encode_splitmv(MBSPLIT_4X4, &parts, &border, &border);
    assert_eq!(best, ZERO_MV);
    let info = decode_splitmv(&bytes, &border, &border);
    assert_eq!(info.sub_mvs[0], (14, 12));
    assert_eq!(info.sub_mvs[1], (14, 12), "LEFT4X4 from within this MB");
    assert_eq!(info.sub_mvs[4], (14, 12), "ABOVE4X4 from within this MB");
    assert_eq!(info.sub_mvs[2], ZERO_MV);

    // And a split *neighbour* hands over its edge sub-blocks, not its macroblock vector:
    // `above.sub_mvs[b + 12]` (bottom row) and `left.sub_mvs[b + 3]` (right column) — dixie
    // lines 10101, 10114.
    let mut left = inter_mb(RefFrame::Last, InterMode::Split, ZERO_MV);
    let mut above = inter_mb(RefFrame::Last, InterMode::Split, ZERO_MV);
    for (i, slot) in left.sub_mvs.iter_mut().enumerate() {
        *slot = (i as i16, 0);
    }
    for (i, slot) in above.sub_mvs.iter_mut().enumerate() {
        *slot = (0, i as i16);
    }
    let empty = [ZERO_MV; 16];
    assert_eq!(left_block_mv(&empty, &left, 0), (3, 0));
    assert_eq!(above_block_mv(&empty, &above, 0), (0, 12));
    // Sub-block 8 (third row, left column): left.sub_mvs[11].
    assert_eq!(left_block_mv(&empty, &left, 8), (11, 0));
    // A non-split neighbour hands over its single macroblock vector.
    let plain = inter_mb(RefFrame::Last, InterMode::New, (5, 5));
    assert_eq!(left_block_mv(&empty, &plain, 0), (5, 5));
    assert_eq!(above_block_mv(&empty, &plain, 0), (5, 5));
    // An intra neighbour hands over the zero vector.
    let intra = InterMbInfo::border();
    assert_eq!(left_block_mv(&empty, &intra, 0), ZERO_MV);
    assert_eq!(above_block_mv(&empty, &intra, 0), ZERO_MV);
}

// --- 7. Segment id / skip flag / reference-frame plumbing ------------

#[test]
fn test_segment_id_is_written_when_the_map_updates_and_inherited_otherwise() {
    let mut header = test_header(1, 1, 128);
    header.segment.enabled = true;
    header.segment.update_map = true;
    header.segment.tree_probs = [128, 128, 128];
    let mut e = TestBoolEncoder::new();
    // segment id 3 == tree_probs[0] -> 1, tree_probs[2] -> 1.
    e.write_bool(128, true);
    e.write_bool(128, true);
    write_intra_dc(&mut e);
    let bytes = e.finish();
    let mut state = Vp8State::new();
    state.ensure_segment_map_size(4);
    let ctx = ModeRowCtx::new(2);
    let (info, _) = decode_at(
        &bytes,
        &header,
        &mut state,
        &ctx,
        MB11,
        &mut forbidden_read_mv,
    );
    assert_eq!(info.segment_id, 3);
    assert_eq!(
        state.segment_map[3], 3,
        "the map is written (RFC 6386 §9.3)"
    );

    // With `update_map` clear, dixie reads no segment-id bits at all and the id is whatever
    // the previous frame left in the map.
    header.segment.update_map = false;
    let mut e2 = TestBoolEncoder::new();
    write_intra_dc(&mut e2); // no segment-id bits precede the intra flag
    let bytes2 = e2.finish();
    state.segment_map[3] = 2;
    let (info2, _) = decode_at(
        &bytes2,
        &header,
        &mut state,
        &ctx,
        MB11,
        &mut forbidden_read_mv,
    );
    assert_eq!(info2.segment_id, 2, "inherited from the persistent map");
    assert_eq!(state.segment_map[3], 2, "and left untouched");
}

#[test]
fn test_skip_flag_is_only_read_when_enabled() {
    for (enabled, expect_skip) in [(false, false), (true, true)] {
        let mut header = test_header(1, 1, 128);
        header.mb_no_skip_coeff = enabled;
        header.prob_skip_false = 128;
        let mut e = TestBoolEncoder::new();
        if enabled {
            e.write_bool(128, true); // skip
        }
        write_intra_dc(&mut e);
        let bytes = e.finish();
        let (info, _) = decode_solo(&bytes, &header);
        assert_eq!(info.skip_coeff, expect_skip);
    }
}

#[test]
fn test_reference_frame_polarity() {
    // RFC 6386 §16.2 lines 5468-5474 / dixie lines 10467-10469.
    for (last_bit, gf_bit, expected) in [
        (false, false, RefFrame::Last),
        (true, false, RefFrame::Golden),
        (true, true, RefFrame::AltRef),
    ] {
        let mut e = TestBoolEncoder::new();
        e.write_bool(1, true); // inter
        e.write_bool(128, last_bit);
        if last_bit {
            e.write_bool(128, gf_bit);
        }
        e.write_tree(&MV_REF_TREE, &mv_ref_probs(&[0, 0, 0, 0]), ZEROMV as i32);
        let bytes = e.finish();
        let (info, _) = decode_solo(&bytes, &test_header(1, 128, 128));
        assert_eq!(info.ref_frame, expected);
        assert_eq!(info.mv(), ZERO_MV);
        assert_eq!(info.sub_mvs, [ZERO_MV; 16]);
        assert!(info.has_y2);
    }
}

// --- 8. ModeRowCtx window and record invariants ----------------------

#[test]
fn test_mode_row_ctx_window_rotation() {
    let a = inter_mb(RefFrame::Last, InterMode::New, (1, 1));
    let b = inter_mb(RefFrame::Last, InterMode::New, (2, 2));
    let c = inter_mb(RefFrame::Last, InterMode::New, (3, 3));
    let border = InterMbInfo::border();
    let mut ctx = ModeRowCtx::new(3);
    ctx.start_row(); // row 0: everything above is border
    assert_eq!(*ctx.left(), border);
    assert_eq!(*ctx.above(0), border);
    assert_eq!(*ctx.above_left(), border);
    ctx.advance(0, &a);
    assert_eq!(*ctx.left(), a);
    ctx.advance(1, &b);
    ctx.advance(2, &c);

    // Row 1: at column 1, `above_left` must be row 0's column 0 even though that slot now
    // holds row 1's column 0.
    ctx.start_row();
    assert_eq!(*ctx.above(0), a);
    assert_eq!(*ctx.above_left(), border, "left border of the previous row");
    let a2 = inter_mb(RefFrame::Last, InterMode::New, (9, 9));
    ctx.advance(0, &a2);
    assert_eq!(*ctx.above(1), b, "not yet overwritten");
    assert_eq!(*ctx.above_left(), a, "the *previous* row's column 0");
    assert_eq!(*ctx.left(), a2);
}

#[test]
fn test_record_and_enum_invariants() {
    // The §16.3 border record: intra, zero, not split.
    let b = InterMbInfo::border();
    assert_eq!(b.ref_frame, RefFrame::Intra);
    assert!(!b.is_inter);
    assert_eq!(b.mv(), ZERO_MV);
    assert!(!b.is_split());
    assert_eq!(b.sub_mvs, [ZERO_MV; 16]);

    // Loop-filter delta slots: dixie lines 9190-9207.
    assert_eq!(RefFrame::Intra.lf_ref_delta_slot(), 0);
    assert_eq!(RefFrame::Last.lf_ref_delta_slot(), 1);
    assert_eq!(RefFrame::Golden.lf_ref_delta_slot(), 2);
    assert_eq!(RefFrame::AltRef.lf_ref_delta_slot(), 3);
    assert_eq!(InterMode::Zero.lf_mode_delta_slot(), 1);
    assert_eq!(InterMode::Split.lf_mode_delta_slot(), 3);
    for m in [InterMode::Nearest, InterMode::Near, InterMode::New] {
        assert_eq!(m.lf_mode_delta_slot(), 2);
    }

    // Inter mode ids continue the intra enumeration: RFC 6386 §16.2 lines 5498-5506, so the
    // `MV_REF_TREE` leaf values are exactly the `NEARESTMV..SPLITMV` constants and nothing else.
    for (leaf, want) in [
        (NEARESTMV, InterMode::Nearest),
        (NEARMV, InterMode::Near),
        (ZEROMV, InterMode::Zero),
        (NEWMV, InterMode::New),
        (SPLITMV, InterMode::Split),
    ] {
        assert_eq!(
            InterMode::from_tree_leaf(leaf as i32).expect("valid leaf"),
            want,
            "leaf {leaf}"
        );
    }
    // A leaf below the inter range is an intra ymode, never an inter mode.
    assert!(InterMode::from_tree_leaf(0).is_err());
    assert!(InterMode::from_tree_leaf(SPLITMV as i32 + 1).is_err());
}

#[test]
fn test_encoder_roundtrips_through_the_production_decoder() {
    // Validates `testutil`'s writers on the trees *this* module drives (header.rs and mv.rs
    // cover their own): every leaf survives a write/read round trip at the exact
    // probabilities used in decode, and every short MV component round-trips through the
    // dixie transcription above — an independent cross-check between P3's encoder and this
    // package's own reading of RFC 6386 §17.2.
    let cases: [(&[i8], &[u8]); 6] = [
        (&YMODE_TREE, &DEFAULT_YMODE_PROB),
        (&UV_MODE_TREE, &DEFAULT_UV_MODE_PROB),
        (&BMODE_TREE, &BMODE_PROB),
        (&MV_REF_TREE, &MODE_CONTEXTS[0]),
        (&SUB_MV_REF_TREE, &SUB_MV_REF_PROBS2[0]),
        (&MBSPLIT_TREE, &MBSPLIT_PROBS),
    ];
    for (tree, probs) in cases {
        // Every leaf of the tree, read straight out of the table itself.
        for leaf in tree.iter().filter(|&&t| t <= 0).map(|&t| i32::from(-t)) {
            let mut e = TestBoolEncoder::new();
            e.write_tree(tree, probs, leaf);
            let bytes = e.finish();
            let mut d = BoolDecoder::new(&bytes);
            assert_eq!(d.read_tree(tree, probs), leaf, "leaf {leaf}");
        }
    }

    // The §17.2 component writer, likewise — short form (0..=7), the long form's implicit
    // bit 3 (8..=15) and the long form with higher bits set, at both signs.
    let probs = DEFAULT_MV_CONTEXT;
    for v in [-1023, -300, -16, -8, -7, -1, 0, 1, 7, 8, 15, 16, 300, 1023] {
        let mut e = TestBoolEncoder::new();
        e.write_mv_component(&probs[0], v);
        let bytes = e.finish();
        let mut d = BoolDecoder::new(&bytes);
        assert_eq!(
            reference_read_mv_component(&mut d, &probs[0]),
            v * 2,
            "component {v} must round trip, doubled into eighth-pel"
        );
    }
}
