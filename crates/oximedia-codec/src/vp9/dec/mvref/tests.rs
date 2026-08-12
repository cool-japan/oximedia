//! Tests for the motion-vector reference candidate scan.
//!
//! Lives beside [`super`] rather than inside it so the module stays under the
//! 2000-line ceiling; the neighbourhoods below are synthetic mode-info grids
//! built to drive one branch of `find_mv_refs` each.

use super::*;
use crate::vp9::dec::refs::{ALTREF_FRAME, GOLDEN_FRAME, LAST_FRAME, NONE_FRAME};

// Raw MB_MODE_COUNT mode numbers (`vp9_enums.h:110-124`).
const DC_PRED: u8 = 0;
const TM_PRED: u8 = 9;
const NEARESTMV: u8 = 10;
const NEARMV: u8 = 11;
const ZEROMV: u8 = 12;
const NEWMV: u8 = 13;

// BLOCK_SIZE indices used below (`vp9_enums.h:46-58`).
const BLOCK_4X4: u8 = 0;
const BLOCK_4X8: u8 = 1;
const BLOCK_8X4: u8 = 2;
const BLOCK_16X16: u8 = 6;
const BLOCK_64X64: u8 = 12;

/// A synthetic mode-info grid. [`FrameMi`]'s constructor is private to
/// [`super::super::recon`], so the tests drive [`MiGrid`] directly.
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
        self.mi[row * self.cols + col] = mi;
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

fn mv(row: i16, col: i16) -> MotionVector {
    MotionVector::new(row, col)
}

/// An inter neighbour with one reference.
fn inter(mode: u8, ref_frame: i8, mv0: MotionVector) -> MiInfo {
    MiInfo {
        sb_type: BLOCK_16X16,
        mode,
        is_inter: true,
        ref_frame: [ref_frame, NONE_FRAME],
        mv: [mv0, MotionVector::zero()],
        ..MiInfo::default()
    }
}

/// An inter neighbour with two references (compound prediction).
fn compound(mode: u8, refs: [i8; 2], mvs: [MotionVector; 2]) -> MiInfo {
    MiInfo {
        sb_type: BLOCK_16X16,
        mode,
        is_inter: true,
        ref_frame: refs,
        mv: mvs,
        ..MiInfo::default()
    }
}

/// An intra neighbour (`read_intra_frame_mode_info` defaults).
fn intra(mode: u8) -> MiInfo {
    MiInfo {
        sb_type: BLOCK_16X16,
        mode,
        ..MiInfo::default()
    }
}

/// A sub-8x8 inter neighbour with four distinct sub-block vectors.
fn sub8x8(mode: u8, ref_frame: i8, bmv: [MotionVector; 4]) -> MiInfo {
    MiInfo {
        sb_type: BLOCK_4X4,
        mode,
        is_inter: true,
        ref_frame: [ref_frame, NONE_FRAME],
        // libvpx copies bmi[3] into mi->mv (`vp9_decodemv.c:770`).
        mv: [bmv[3], MotionVector::zero()],
        bmv: [
            [bmv[0], MotionVector::zero()],
            [bmv[1], MotionVector::zero()],
            [bmv[2], MotionVector::zero()],
            [bmv[3], MotionVector::zero()],
        ],
        ..MiInfo::default()
    }
}

const NO_BIAS: [bool; 4] = [false; 4];

/// A 8x8-MI grid with a block at (4, 4): far enough from every edge that
/// the clamp is inert and a scan result is the raw candidate list.
fn grid8() -> TestGrid {
    TestGrid::new(8, 8)
}

fn scan(grid: &TestGrid, ref_frame: i8, bsize: u8) -> MvRefResult {
    find_mv_refs(
        grid,
        &TileBounds::full_width(grid.cols),
        None,
        &NO_BIAS,
        ref_frame,
        4,
        4,
        bsize,
        -1,
    )
}

// -- is_inside (vp9_mvref_common.h:278-284) ---------------------------

#[test]
fn is_inside_rejects_positions_above_the_frame() {
    let tile = TileBounds::full_width(8);
    assert!(!is_inside(&tile, 4, 0, 8, [-1, 0]));
    assert!(is_inside(&tile, 4, 1, 8, [-1, 0]));
}

#[test]
fn is_inside_rejects_positions_below_the_frame() {
    let tile = TileBounds::full_width(8);
    // Column 0 of the position (mi_col 1 + offset -1) stays inside the tile,
    // so only the row bound decides.
    assert!(!is_inside(&tile, 1, 7, 8, [1, -1]));
    assert!(is_inside(&tile, 1, 6, 8, [1, -1]));
    // The same position one column further left leaves the frame entirely.
    assert!(!is_inside(&tile, 0, 6, 8, [1, -1]));
}

#[test]
fn is_inside_uses_tile_columns_but_has_no_tile_row_bound() {
    // A tile covering columns 4..8: a left neighbour at column 3 is out,
    // one at column 4 is in.
    let tile = TileBounds {
        mi_col_start: 4,
        mi_col_end: 8,
    };
    assert!(!is_inside(&tile, 4, 4, 8, [0, -1]));
    assert!(is_inside(&tile, 5, 4, 8, [0, -1]));
    // Rows are bounded only by the frame — there is no `mi_row_start`
    // test in VP9's is_inside, so a row far above a would-be tile row
    // boundary is still inside.
    assert!(is_inside(&tile, 5, 7, 8, [-3, -1]));
}

#[test]
fn is_inside_rejects_at_and_beyond_the_tile_column_end() {
    let tile = TileBounds {
        mi_col_start: 0,
        mi_col_end: 4,
    };
    assert!(!is_inside(&tile, 3, 4, 8, [-1, 1]));
    assert!(is_inside(&tile, 2, 4, 8, [-1, 1]));
}

#[test]
fn every_mv_ref_blocks_position_that_passes_is_inside_is_addressable() {
    // Exhaustive over block sizes, neighbour slots, block positions and
    // two tile layouts: is_inside must never admit a position outside the
    // grid or outside the tile's columns.
    let grid = TestGrid::new(9, 9);
    for tile in [
        TileBounds::full_width(9),
        TileBounds {
            mi_col_start: 3,
            mi_col_end: 7,
        },
    ] {
        for bsize in 0..BLOCK_SIZES as u8 {
            for &pos in MV_REF_BLOCKS[bsize as usize].iter() {
                for mi_row in 0..grid.rows {
                    for mi_col in tile.mi_col_start..tile.mi_col_end {
                        if !is_inside(&tile, mi_col, mi_row, grid.rows, pos) {
                            continue;
                        }
                        let row = mi_row as i32 + i32::from(pos[0]);
                        let col = mi_col as i32 + i32::from(pos[1]);
                        assert!(
                            row >= 0 && (row as usize) < grid.rows,
                            "bsize {bsize} pos {pos:?} at ({mi_row},{mi_col}) row {row}"
                        );
                        assert!(
                            col >= tile.mi_col_start as i32 && col < tile.mi_col_end as i32,
                            "bsize {bsize} pos {pos:?} at ({mi_row},{mi_col}) col {col}"
                        );
                        assert!(
                            candidate_at(&grid, &tile, mi_row, mi_col, pos).is_some(),
                            "bsize {bsize} pos {pos:?} at ({mi_row},{mi_col}) must resolve"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn mv_ref_blocks_never_probes_the_current_block() {
    // Every position has at least one negative component, so the scan can
    // never read the (not yet decoded) block it is predicting.
    for row in &MV_REF_BLOCKS {
        for pos in row {
            assert!(pos[0] < 0 || pos[1] < 0, "position {pos:?} is not causal");
        }
    }
}

// -- block edges and clamps -------------------------------------------

#[test]
fn block_edges_match_set_mi_row_col() {
    // 16x16 block (bw = bh = 2) at (1, 2) of a 5x5 MI grid.
    let e = BlockEdges::new(1, 2, BLOCK_16X16, 5, 5);
    assert_eq!(e.to_left, -(2 * 64));
    assert_eq!(e.to_right, (5 - 2 - 2) * 64);
    assert_eq!(e.to_top, -64);
    assert_eq!(e.to_bottom, (5 - 2 - 1) * 64);
}

#[test]
fn block_edges_are_negative_for_a_block_overhanging_the_frame() {
    // 64x64 block (bw = bh = 8) at (0, 0) of a 5x5 MI grid.
    let e = BlockEdges::new(0, 0, BLOCK_64X64, 5, 5);
    assert_eq!((e.to_left, e.to_top), (0, 0));
    assert_eq!((e.to_right, e.to_bottom), (-192, -192));
}

#[test]
fn clamp_mv_ref_bounds_are_the_edges_plus_mv_border() {
    let e = BlockEdges::new(1, 1, BLOCK_16X16, 8, 8);
    // row in [-64 - 128, 320 + 128], col in [-64 - 128, 320 + 128].
    assert_eq!(e.clamp_mv_ref(mv(-1000, 1000)), mv(-192, 448));
    assert_eq!(e.clamp_mv_ref(mv(10, -20)), mv(10, -20));
}

#[test]
fn clamp_mv_ref_applies_left_right_to_the_column_and_top_bottom_to_the_row() {
    // Deliberately asymmetric edges: a transposed clamp would swap these.
    let e = BlockEdges {
        to_left: 0,
        to_right: 64,
        to_top: -640,
        to_bottom: 640,
    };
    assert_eq!(e.clamp_mv_ref(mv(-700, -700)), mv(-700, -128));
    assert_eq!(e.clamp_mv_ref(mv(700, 700)), mv(700, 192));
}

#[test]
fn clamp_i32_degrades_like_libvpx_when_bounds_invert() {
    // libvpx's ternary returns `low`; `Ord::clamp` would panic.
    assert_eq!(clamp_i32(0, 10, -10), 10);
    assert_eq!(clamp_i32(-50, -10, -20), -10);
}

// -- find_mv_refs: empty neighbourhood ---------------------------------

#[test]
fn no_neighbours_yields_an_empty_list_and_both_predicted_context() {
    // Block at the very top-left of a single-tile frame: every candidate
    // position is outside.
    let grid = TestGrid::new(8, 8);
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        None,
        &NO_BIAS,
        LAST_FRAME,
        0,
        0,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 0);
    assert_eq!(result.list, [MotionVector::zero(); 2]);
    // counter 0 -> BOTH_PREDICTED, *not* 0.
    assert_eq!(result.mode_context, 2);
}

#[test]
fn frame_edge_clamps_the_unfilled_entry() {
    // 64x64 block at (0, 0) of a 5x5 MI grid: mb_to_right/bottom_edge are
    // -192, so the clamp range is [-128, -64] and the zero fill libvpx
    // memsets in comes out non-zero. Clamping only `count` entries — the
    // decoder's shortcut — would leave these at zero.
    let grid = TestGrid::new(5, 5);
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(5),
        None,
        &NO_BIAS,
        LAST_FRAME,
        0,
        0,
        BLOCK_64X64,
        -1,
    );
    assert_eq!(result.count, 0);
    assert_eq!(result.list, [mv(-64, -64), mv(-64, -64)]);
}

// -- find_mv_refs: same-reference matches ------------------------------

#[test]
fn above_neighbour_with_the_same_reference_is_the_first_candidate() {
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, LAST_FRAME, mv(8, -12)));
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.count, 1);
    assert_eq!(result.nearest(), mv(8, -12));
    assert_eq!(result.near(), MotionVector::zero());
}

#[test]
fn second_reference_slot_is_matched_only_when_the_first_does_not_match() {
    let mut grid = grid8();
    // ref_frame[0] is GOLDEN, ref_frame[1] is LAST: the else-if arm.
    grid.set(
        3,
        4,
        compound(NEWMV, [GOLDEN_FRAME, LAST_FRAME], [mv(1, 1), mv(2, 2)]),
    );
    assert_eq!(scan(&grid, LAST_FRAME, BLOCK_16X16).nearest(), mv(2, 2));
    // With ref_frame[0] matching, the first slot wins even though the
    // second also matches.
    grid.set(
        3,
        4,
        compound(NEWMV, [LAST_FRAME, LAST_FRAME], [mv(1, 1), mv(2, 2)]),
    );
    assert_eq!(scan(&grid, LAST_FRAME, BLOCK_16X16).nearest(), mv(1, 1));
}

#[test]
fn an_identical_second_candidate_is_deduplicated() {
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, LAST_FRAME, mv(8, -12)));
    grid.set(4, 3, inter(NEWMV, LAST_FRAME, mv(8, -12)));
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.count, 1, "duplicate must not fill the second slot");
    assert_eq!(result.list, [mv(8, -12), MotionVector::zero()]);
}

#[test]
fn a_distinct_second_candidate_fills_the_list_and_stops_the_scan() {
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, LAST_FRAME, mv(8, -12)));
    grid.set(4, 3, inter(NEWMV, LAST_FRAME, mv(-4, 6)));
    // A third same-reference neighbour that must never be reached
    // (position [-1, 1] of BLOCK_16X16 is scanned after the first two).
    grid.set(3, 5, inter(NEWMV, LAST_FRAME, mv(100, 100)));
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.count, 2);
    assert_eq!(result.list, [mv(8, -12), mv(-4, 6)]);
}

#[test]
fn pass_two_neighbours_contribute_whole_block_vectors() {
    let mut grid = grid8();
    // BLOCK_16X16 slot 3 is [1, -1] — a below-left neighbour, scanned in
    // pass 2, so even a sub-8x8 candidate contributes mi->mv.
    grid.set(
        5,
        3,
        sub8x8(NEWMV, LAST_FRAME, [mv(1, 1), mv(2, 2), mv(3, 3), mv(4, 4)]),
    );
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.nearest(), mv(4, 4), "mi->mv, not a bmi entry");
}

// -- find_mv_refs: different-reference pass ---------------------------

#[test]
fn different_reference_candidate_is_added_unscaled_with_equal_sign_bias() {
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, GOLDEN_FRAME, mv(8, -12)));
    let sign_bias = [false, false, false, false];
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        None,
        &sign_bias,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 1);
    assert_eq!(result.nearest(), mv(8, -12));
}

#[test]
fn different_reference_candidate_is_negated_with_opposite_sign_bias() {
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, ALTREF_FRAME, mv(8, -12)));
    // ALTREF points backwards, LAST forwards.
    let sign_bias = [false, false, false, true];
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        None,
        &sign_bias,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.nearest(), mv(-8, 12));
}

#[test]
fn intra_neighbours_vote_for_the_context_but_contribute_no_vector() {
    let mut grid = grid8();
    grid.set(3, 4, intra(TM_PRED));
    grid.set(4, 3, intra(DC_PRED));
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.count, 0);
    // 9 + 9 = 18 -> BOTH_INTRA.
    assert_eq!(result.mode_context, 6);
}

#[test]
fn compound_candidates_second_vector_needs_a_distinct_value() {
    let mut grid = grid8();
    // Both references differ from LAST, but mv[1] == mv[0], so only the
    // first is added (`IF_DIFF_REF_FRAME_ADD_MV`'s `mv[1] != mv[0]`).
    grid.set(
        3,
        4,
        compound(NEWMV, [GOLDEN_FRAME, ALTREF_FRAME], [mv(5, 5), mv(5, 5)]),
    );
    assert_eq!(scan(&grid, LAST_FRAME, BLOCK_16X16).count, 1);

    // Distinct vectors fill both slots from one neighbour.
    grid.set(
        3,
        4,
        compound(NEWMV, [GOLDEN_FRAME, ALTREF_FRAME], [mv(5, 5), mv(6, 6)]),
    );
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.count, 2);
    assert_eq!(result.list, [mv(5, 5), mv(6, 6)]);
}

#[test]
fn the_different_reference_pass_needs_a_neighbour_inside_the_tile() {
    // The block sits at the tile's left column, so its left neighbour is
    // out; the above row is out of the frame. `different_ref_found` stays
    // false and the differing-reference neighbour two columns left (in
    // the previous tile) is never considered.
    let mut grid = TestGrid::new(8, 8);
    grid.set(0, 2, inter(NEWMV, GOLDEN_FRAME, mv(9, 9)));
    let tile = TileBounds {
        mi_col_start: 4,
        mi_col_end: 8,
    };
    let result = find_mv_refs(
        &grid,
        &tile,
        None,
        &NO_BIAS,
        LAST_FRAME,
        0,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 0);
    assert_eq!(result.mode_context, 2);
}

#[test]
fn a_left_neighbour_in_the_previous_tile_is_excluded() {
    let mut grid = grid8();
    grid.set(4, 3, inter(NEWMV, LAST_FRAME, mv(7, 7)));
    // Same grid, two tile layouts: with the boundary at column 4 the left
    // neighbour disappears.
    let full = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        None,
        &NO_BIAS,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(full.nearest(), mv(7, 7));
    let split = find_mv_refs(
        &grid,
        &TileBounds {
            mi_col_start: 4,
            mi_col_end: 8,
        },
        None,
        &NO_BIAS,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(split.count, 0);
}

// -- find_mv_refs: temporal candidates ---------------------------------

fn prev_array(cols: usize, rows: usize, at: (usize, usize), record: MvRefRow) -> Vec<MvRefRow> {
    let mut mvs = vec![MvRefRow::default(); rows * cols];
    mvs[at.0 * cols + at.1] = record;
    mvs
}

#[test]
fn temporal_candidate_with_the_same_reference_is_added() {
    let grid = grid8();
    let prev = prev_array(
        8,
        8,
        (4, 4),
        MvRefRow {
            ref_frame: [LAST_FRAME, NONE_FRAME],
            mv: [mv(3, -5), MotionVector::zero()],
        },
    );
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        Some(&prev),
        &NO_BIAS,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 1);
    assert_eq!(result.nearest(), mv(3, -5));
}

#[test]
fn no_temporal_candidate_without_a_previous_frame() {
    let grid = grid8();
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.count, 0);
}

#[test]
fn temporal_second_slot_matches_only_when_the_first_does_not() {
    let grid = grid8();
    let prev = prev_array(
        8,
        8,
        (4, 4),
        MvRefRow {
            ref_frame: [GOLDEN_FRAME, LAST_FRAME],
            mv: [mv(1, 2), mv(3, 4)],
        },
    );
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        Some(&prev),
        &NO_BIAS,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.nearest(), mv(3, 4));
}

#[test]
fn temporal_different_reference_is_negated_on_opposite_sign_bias() {
    // No spatial neighbours at all (top-left block): this also pins that
    // the second temporal block is *not* gated on `different_ref_found`.
    let grid = TestGrid::new(8, 8);
    let prev = prev_array(
        8,
        8,
        (0, 0),
        MvRefRow {
            ref_frame: [ALTREF_FRAME, NONE_FRAME],
            mv: [mv(8, -12), MotionVector::zero()],
        },
    );
    let sign_bias = [false, false, false, true];
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        Some(&prev),
        &sign_bias,
        LAST_FRAME,
        0,
        0,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 1);
    assert_eq!(result.nearest(), mv(-8, 12));
}

#[test]
fn temporal_second_different_reference_needs_a_distinct_vector() {
    let grid = TestGrid::new(8, 8);
    let same = prev_array(
        8,
        8,
        (0, 0),
        MvRefRow {
            ref_frame: [GOLDEN_FRAME, ALTREF_FRAME],
            mv: [mv(4, 4), mv(4, 4)],
        },
    );
    let tile = TileBounds::full_width(8);
    let result = find_mv_refs(
        &grid,
        &tile,
        Some(&same),
        &NO_BIAS,
        LAST_FRAME,
        0,
        0,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 1);

    let distinct = prev_array(
        8,
        8,
        (0, 0),
        MvRefRow {
            ref_frame: [GOLDEN_FRAME, ALTREF_FRAME],
            mv: [mv(4, 4), mv(5, 5)],
        },
    );
    let result = find_mv_refs(
        &grid,
        &tile,
        Some(&distinct),
        &NO_BIAS,
        LAST_FRAME,
        0,
        0,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 2);
    assert_eq!(result.list, [mv(4, 4), mv(5, 5)]);
}

#[test]
fn an_intra_temporal_record_contributes_nothing() {
    let grid = TestGrid::new(8, 8);
    // MvRefRow::default() is [INTRA_FRAME, NONE_FRAME]: it must fail both
    // the same-reference test and the `> INTRA_FRAME` different-reference
    // test.
    let prev = vec![MvRefRow::default(); 64];
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        Some(&prev),
        &NO_BIAS,
        LAST_FRAME,
        0,
        0,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.count, 0);
}

#[test]
fn spatial_candidates_precede_temporal_ones() {
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, LAST_FRAME, mv(1, 1)));
    let prev = prev_array(
        8,
        8,
        (4, 4),
        MvRefRow {
            ref_frame: [LAST_FRAME, NONE_FRAME],
            mv: [mv(2, 2), MotionVector::zero()],
        },
    );
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        Some(&prev),
        &NO_BIAS,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.list, [mv(1, 1), mv(2, 2)]);
}

// -- sub-block vectors -------------------------------------------------

#[test]
fn nearest_two_neighbours_use_sub_block_vectors_when_a_block_index_is_given() {
    // BLOCK_4X4 slots 0 and 1 are the above ([-1, 0]) and left ([0, -1])
    // neighbours; idx_n_column_to_subblock[block][search_col == 0] picks
    // sub-block 2 from the above neighbour and 1 from the left one for
    // block 0.
    let mut grid = grid8();
    grid.set(
        3,
        4,
        sub8x8(
            NEWMV,
            LAST_FRAME,
            [mv(10, 0), mv(11, 0), mv(12, 0), mv(13, 0)],
        ),
    );
    grid.set(
        4,
        3,
        sub8x8(
            NEWMV,
            LAST_FRAME,
            [mv(20, 0), mv(21, 0), mv(22, 0), mv(23, 0)],
        ),
    );
    let tile = TileBounds::full_width(8);
    let block0 = find_mv_refs(&grid, &tile, None, &NO_BIAS, LAST_FRAME, 4, 4, BLOCK_4X4, 0);
    assert_eq!(block0.list, [mv(12, 0), mv(21, 0)]);
    // Block 3: above -> sub-block 3, left -> sub-block 3.
    let block3 = find_mv_refs(&grid, &tile, None, &NO_BIAS, LAST_FRAME, 4, 4, BLOCK_4X4, 3);
    assert_eq!(block3.list, [mv(13, 0), mv(23, 0)]);
    // Blocks 1 and 2 pick the remaining table rows.
    let block1 = find_mv_refs(&grid, &tile, None, &NO_BIAS, LAST_FRAME, 4, 4, BLOCK_4X4, 1);
    assert_eq!(block1.list, [mv(13, 0), mv(21, 0)]);
    let block2 = find_mv_refs(&grid, &tile, None, &NO_BIAS, LAST_FRAME, 4, 4, BLOCK_4X4, 2);
    assert_eq!(block2.list, [mv(12, 0), mv(23, 0)]);
}

#[test]
fn a_block_of_8x8_or_larger_reads_whole_block_vectors_from_sub8x8_neighbours() {
    let mut grid = grid8();
    grid.set(
        3,
        4,
        sub8x8(
            NEWMV,
            LAST_FRAME,
            [mv(10, 0), mv(11, 0), mv(12, 0), mv(13, 0)],
        ),
    );
    // block = -1 -> mi->mv (which libvpx set from bmi[3]).
    assert_eq!(scan(&grid, LAST_FRAME, BLOCK_16X16).nearest(), mv(13, 0));
}

#[test]
fn a_non_sub8x8_neighbour_never_yields_a_sub_block_vector() {
    let mut grid = grid8();
    let mut candidate = inter(NEWMV, LAST_FRAME, mv(7, 7));
    candidate.bmv = [[mv(99, 99), MotionVector::zero()]; 4];
    grid.set(3, 4, candidate);
    let result = find_mv_refs(
        &grid,
        &TileBounds::full_width(8),
        None,
        &NO_BIAS,
        LAST_FRAME,
        4,
        4,
        BLOCK_4X4,
        0,
    );
    assert_eq!(result.nearest(), mv(7, 7));
}

// -- mode context ------------------------------------------------------

#[test]
fn mode_context_is_the_hand_traced_counter_lookup() {
    // (above mode, left mode, expected counter, expected context).
    let cases: [(u8, u8, usize, u8); 9] = [
        (NEARESTMV, NEARMV, 0, 2),  // BOTH_PREDICTED
        (NEWMV, NEARESTMV, 1, 3),   // NEW_PLUS_NON_INTRA
        (NEWMV, NEWMV, 2, 4),       // BOTH_NEW
        (ZEROMV, NEARESTMV, 3, 1),  // ZERO_PLUS_PREDICTED
        (ZEROMV, NEWMV, 4, 3),      // NEW_PLUS_NON_INTRA
        (ZEROMV, ZEROMV, 6, 0),     // BOTH_ZERO
        (DC_PRED, NEARESTMV, 9, 5), // INTRA_PLUS_NON_INTRA
        (DC_PRED, NEWMV, 10, 5),    // INTRA_PLUS_NON_INTRA
        (DC_PRED, ZEROMV, 12, 5),   // INTRA_PLUS_NON_INTRA
    ];
    for (above, left, counter, context) in cases {
        let mut grid = grid8();
        let build = |mode: u8| {
            if mode < NEARESTMV {
                intra(mode)
            } else {
                inter(mode, LAST_FRAME, mv(1, 1))
            }
        };
        grid.set(3, 4, build(above));
        grid.set(4, 3, build(left));
        assert_eq!(
            mode_counter(above) + mode_counter(left),
            counter,
            "counter for ({above}, {left})"
        );
        assert_eq!(
            scan(&grid, ALTREF_FRAME, BLOCK_16X16).mode_context,
            context,
            "context for ({above}, {left})"
        );
    }
}

#[test]
fn a_single_neighbour_votes_alone() {
    // Only the above neighbour is inside (block sits on the tile's left
    // column): counter 9 -> INTRA_PLUS_NON_INTRA.
    let mut grid = grid8();
    grid.set(3, 4, intra(DC_PRED));
    let tile = TileBounds {
        mi_col_start: 4,
        mi_col_end: 8,
    };
    let result = find_mv_refs(
        &grid,
        &tile,
        None,
        &NO_BIAS,
        LAST_FRAME,
        4,
        4,
        BLOCK_16X16,
        -1,
    );
    assert_eq!(result.mode_context, 5);
}

#[test]
fn the_second_neighbour_still_votes_when_it_completes_the_list() {
    // Two same-reference neighbours with distinct vectors: the add at
    // neighbour 1 fills the list and ends the scan, but its mode vote was
    // already counted (`vp9_mvref_common.c:43` precedes `:46`).
    let mut grid = grid8();
    grid.set(3, 4, inter(ZEROMV, LAST_FRAME, mv(1, 1)));
    grid.set(4, 3, inter(ZEROMV, LAST_FRAME, mv(2, 2)));
    let result = scan(&grid, LAST_FRAME, BLOCK_16X16);
    assert_eq!(result.count, 2, "the scan ended at the second neighbour");
    // 3 + 3 = 6 -> BOTH_ZERO. A counter of 3 (one vote) would give 1.
    assert_eq!(result.mode_context, 0);
}

#[test]
fn mode_context_is_never_the_invalid_sentinel() {
    for above in [DC_PRED, TM_PRED, NEARESTMV, NEARMV, ZEROMV, NEWMV] {
        for left in [DC_PRED, TM_PRED, NEARESTMV, NEARMV, ZEROMV, NEWMV] {
            let mut grid = grid8();
            let build = |mode: u8| {
                if mode < NEARESTMV {
                    intra(mode)
                } else {
                    inter(mode, GOLDEN_FRAME, mv(1, 1))
                }
            };
            grid.set(3, 4, build(above));
            grid.set(4, 3, build(left));
            let context = scan(&grid, LAST_FRAME, BLOCK_16X16).mode_context;
            assert_ne!(
                context, COUNTER_INVALID_CASE,
                "({above}, {left}) reached the INVALID_CASE sentinel"
            );
            assert!(
                context < 7,
                "({above}, {left}) context {context} out of range"
            );
        }
    }
}

#[test]
fn get_mode_context_agrees_with_find_mv_refs() {
    // The decoder computes the context in a separate pass; it must equal
    // the one the candidate scan derives, including when the scan stops
    // early or when a neighbour is outside the tile.
    let modes = [DC_PRED, NEARESTMV, ZEROMV, NEWMV];
    for (i, &above) in modes.iter().enumerate() {
        for (j, &left) in modes.iter().enumerate() {
            let mut grid = grid8();
            let build = |mode: u8, m: i16| {
                if mode < NEARESTMV {
                    intra(mode)
                } else {
                    inter(mode, LAST_FRAME, mv(m, m))
                }
            };
            grid.set(3, 4, build(above, i as i16 + 1));
            grid.set(4, 3, build(left, j as i16 + 5));
            for tile in [
                TileBounds::full_width(8),
                TileBounds {
                    mi_col_start: 4,
                    mi_col_end: 8,
                },
            ] {
                for bsize in [BLOCK_4X4, BLOCK_16X16, BLOCK_64X64] {
                    let scan =
                        find_mv_refs(&grid, &tile, None, &NO_BIAS, LAST_FRAME, 4, 4, bsize, -1);
                    assert_eq!(
                        scan.mode_context,
                        get_mode_context(&grid, &tile, 4, 4, bsize),
                        "({above}, {left}) bsize {bsize} tile {tile:?}"
                    );
                }
            }
        }
    }
}

// -- precision ---------------------------------------------------------

#[test]
fn lower_mv_precision_rounds_toward_zero() {
    assert_eq!(lower_mv_precision(mv(3, -3), false), mv(2, -2));
    assert_eq!(lower_mv_precision(mv(-1, 1), false), mv(0, 0));
    assert_eq!(lower_mv_precision(mv(-7, 9), false), mv(-6, 8));
    // Even components are untouched.
    assert_eq!(lower_mv_precision(mv(-8, 8), false), mv(-8, 8));
    // Large odd values (beyond the high-precision threshold) round even
    // when allow_hp is set.
    assert_eq!(lower_mv_precision(mv(-65, 65), true), mv(-64, 64));
}

#[test]
fn lower_mv_precision_keeps_odd_components_only_for_small_vectors_with_hp() {
    assert_eq!(lower_mv_precision(mv(-63, 63), true), mv(-63, 63));
    // One component at the threshold disables high precision for both — and
    // the odd negative component rounds *toward* zero (-63 -> -62), which is
    // the direction libvpx's `mv->row += (mv->row > 0 ? -1 : 1)` takes.
    assert_eq!(lower_mv_precision(mv(-63, 64), true), mv(-62, 64));
}

#[test]
fn use_mv_hp_threshold_is_64_on_both_components() {
    assert!(use_mv_hp(mv(63, -63)));
    assert!(!use_mv_hp(mv(64, 0)));
    assert!(!use_mv_hp(mv(0, -64)));
    assert!(use_mv_hp(mv(0, 0)));
}

#[test]
fn find_best_ref_mvs_lowers_both_entries() {
    let list = [mv(3, 5), mv(-101, 7)];
    assert_eq!(find_best_ref_mvs(&list, false), (mv(2, 4), mv(-100, 6)));
    // With allow_hp the small vector keeps its odd components; the large
    // one still rounds.
    assert_eq!(find_best_ref_mvs(&list, true), (mv(3, 5), mv(-100, 6)));
}

#[test]
fn clamp_mv2_is_a_no_op_after_find_mv_refs() {
    // Frame-edge geometry (the case where clamping matters at all), every
    // candidate configuration the scan can produce, both precisions.
    for bsize in [BLOCK_4X4, BLOCK_16X16, BLOCK_64X64] {
        for (mi_row, mi_col) in [(0usize, 0usize), (0, 4), (4, 0), (4, 4)] {
            let mut grid = TestGrid::new(5, 5);
            grid.set(0, 0, inter(NEWMV, LAST_FRAME, mv(-9999, 9999)));
            grid.set(3, 3, inter(NEWMV, LAST_FRAME, mv(9999, -9999)));
            let result = find_mv_refs(
                &grid,
                &TileBounds::full_width(5),
                None,
                &NO_BIAS,
                LAST_FRAME,
                mi_row,
                mi_col,
                bsize,
                -1,
            );
            let edges = BlockEdges::new(mi_row, mi_col, bsize, 5, 5);
            for allow_hp in [false, true] {
                let (nearest, near) = find_best_ref_mvs(&result.list, allow_hp);
                assert_eq!(
                    edges.clamp_mv2(nearest),
                    nearest,
                    "bsize {bsize} at ({mi_row},{mi_col})"
                );
                assert_eq!(
                    edges.clamp_mv2(near),
                    near,
                    "bsize {bsize} at ({mi_row},{mi_col})"
                );
            }
        }
    }
}

// -- append_sub8x8_mvs_for_idx ----------------------------------------

/// The block being decoded: sub-8x8, single reference, with `bmv`
/// entries filled in raster order by the caller.
fn current_block(sb_type: u8, bmv: [MotionVector; 4]) -> MiInfo {
    MiInfo {
        sb_type,
        mode: NEWMV,
        is_inter: true,
        ref_frame: [LAST_FRAME, NONE_FRAME],
        bmv: [
            [bmv[0], MotionVector::zero()],
            [bmv[1], MotionVector::zero()],
            [bmv[2], MotionVector::zero()],
            [bmv[3], MotionVector::zero()],
        ],
        ..MiInfo::default()
    }
}

fn append(grid: &TestGrid, cur: &MiInfo, block: usize) -> Sub8x8Mvs {
    append_sub8x8_mvs_for_idx(
        grid,
        &TileBounds::full_width(grid.cols),
        None,
        &NO_BIAS,
        cur,
        0,
        block,
        4,
        4,
    )
}

/// Two same-reference neighbours giving the candidate list
/// `[list0, list1]` for block indices whose sub-block selection resolves
/// to those vectors.
fn grid_with_two_candidates() -> TestGrid {
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, LAST_FRAME, mv(30, 0)));
    grid.set(4, 3, inter(NEWMV, LAST_FRAME, mv(40, 0)));
    grid
}

#[test]
fn append_sub8x8_block_zero_takes_the_candidate_pair() {
    let grid = grid_with_two_candidates();
    let cur = current_block(BLOCK_4X4, [MotionVector::zero(); 4]);
    let out = append(&grid, &cur, 0);
    assert_eq!((out.nearest, out.near), (mv(30, 0), mv(40, 0)));
    // 1 + 1 = 2 -> BOTH_NEW.
    assert_eq!(out.mode_context, 4);
}

#[test]
fn append_sub8x8_blocks_one_and_two_use_the_first_sub_block_as_nearest() {
    let grid = grid_with_two_candidates();
    let cur = current_block(
        BLOCK_4X4,
        [
            mv(5, 0),
            MotionVector::zero(),
            MotionVector::zero(),
            MotionVector::zero(),
        ],
    );
    for block in [1usize, 2] {
        let out = append(&grid, &cur, block);
        assert_eq!(out.nearest, mv(5, 0), "block {block}");
        assert_eq!(out.near, mv(30, 0), "block {block}");
    }
    // When the first candidate equals `nearest`, the second is taken.
    let cur = current_block(
        BLOCK_4X4,
        [
            mv(30, 0),
            MotionVector::zero(),
            MotionVector::zero(),
            MotionVector::zero(),
        ],
    );
    assert_eq!(append(&grid, &cur, 1).near, mv(40, 0));
}

#[test]
fn append_sub8x8_block_three_probes_sub_blocks_before_the_list() {
    let grid = grid_with_two_candidates();
    // bmi[1] differs from bmi[2] -> near is bmi[1].
    let cur = current_block(
        BLOCK_4X4,
        [mv(1, 0), mv(2, 0), mv(3, 0), MotionVector::zero()],
    );
    let out = append(&grid, &cur, 3);
    assert_eq!((out.nearest, out.near), (mv(3, 0), mv(2, 0)));

    // bmi[1] == bmi[2] -> fall through to bmi[0].
    let cur = current_block(
        BLOCK_4X4,
        [mv(1, 0), mv(3, 0), mv(3, 0), MotionVector::zero()],
    );
    assert_eq!(append(&grid, &cur, 3).near, mv(1, 0));

    // All three sub-blocks equal -> fall through to the candidate list.
    let cur = current_block(
        BLOCK_4X4,
        [mv(3, 0), mv(3, 0), mv(3, 0), MotionVector::zero()],
    );
    assert_eq!(append(&grid, &cur, 3).near, mv(30, 0));
}

#[test]
fn append_sub8x8_near_defaults_to_zero_when_every_candidate_matches() {
    // A neighbourhood with a single candidate equal to `nearest`: nothing
    // differs, so `near` stays at libvpx's zero initialisation.
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, LAST_FRAME, mv(5, 0)));
    let cur = current_block(BLOCK_4X4, [mv(5, 0); 4]);
    let out = append(&grid, &cur, 1);
    assert_eq!(out.nearest, mv(5, 0));
    // list[1] is the (clamped, here inert) zero fill, which also differs
    // from nothing — the first differing candidate is that zero.
    assert_eq!(out.near, MotionVector::zero());
}

#[test]
fn append_sub8x8_covers_the_block_indices_of_every_sub8x8_size() {
    // BLOCK_4X4 decodes sub-blocks 0..4, BLOCK_4X8 decodes 0 and 1,
    // BLOCK_8X4 decodes 0 and 2 (`vp9_decodemv.c:734-736`).
    let grid = grid_with_two_candidates();
    for (sb_type, blocks) in [
        (BLOCK_4X4, vec![0usize, 1, 2, 3]),
        (BLOCK_4X8, vec![0usize, 1]),
        (BLOCK_8X4, vec![0usize, 2]),
    ] {
        let cur = current_block(sb_type, [mv(7, 0), mv(8, 0), mv(9, 0), mv(10, 0)]);
        for block in blocks {
            let out = append(&grid, &cur, block);
            let expected_nearest = match block {
                0 => mv(30, 0),
                1 | 2 => mv(7, 0),
                _ => mv(9, 0),
            };
            assert_eq!(
                out.nearest, expected_nearest,
                "sb_type {sb_type} block {block}"
            );
            assert_ne!(out.near, out.nearest, "sb_type {sb_type} block {block}");
        }
    }
}

#[test]
fn append_sub8x8_uses_the_selected_reference_slot() {
    // A compound current block: ref_idx 1 must scan for ALTREF and read
    // the second column of `bmv`.
    let mut grid = grid8();
    grid.set(3, 4, inter(NEWMV, ALTREF_FRAME, mv(77, 0)));
    let mut cur = current_block(BLOCK_4X4, [mv(1, 0); 4]);
    cur.ref_frame = [LAST_FRAME, ALTREF_FRAME];
    cur.bmv[0][1] = mv(2, 0);
    let out = append_sub8x8_mvs_for_idx(
        &grid,
        &TileBounds::full_width(8),
        None,
        &NO_BIAS,
        &cur,
        1,
        1,
        4,
        4,
    );
    assert_eq!(out.nearest, mv(2, 0), "bmv[0][ref_idx = 1]");
    assert_eq!(out.near, mv(77, 0), "the ALTREF candidate");
}

// -- differential test against the libvpx reference decoder ---------------

/// The linear congruential generator the C reference harness used, so both
/// sides build byte-identical cases: `state = state * 1664525 + 1013904223`
/// over `u32`, seeded with `0x0123_4567`.
struct Lcg(u32);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        self.0
    }

    /// C `next_range`: `next_u32() % n`.
    fn range(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }

    /// C `next_mv_component`: `next_range(2049) - 1024`.
    fn mv_component(&mut self) -> i16 {
        self.range(2049) as i16 - 1024
    }

    fn motion_vector(&mut self) -> MotionVector {
        let row = self.mv_component();
        let col = self.mv_component();
        MotionVector::new(row, col)
    }

    fn flag(&mut self) -> bool {
        self.range(2) != 0
    }
}

/// Backing stride of the reference harness's mode-info array (libvpx
/// `xd->mi_stride`), deliberately larger than the logical `mi_cols` of most
/// cases so a stride/width confusion cannot pass unnoticed.
const REF_STRIDE: usize = 8;

/// A grid whose logical size is `cm->mi_rows` x `cm->mi_cols` while its
/// backing array is [`REF_STRIDE`] wide — libvpx's own layout.
struct RefGrid {
    rows: usize,
    cols: usize,
    mi: Vec<MiInfo>,
}

impl MiGrid for RefGrid {
    fn mi_rows(&self) -> usize {
        self.rows
    }
    fn mi_cols(&self) -> usize {
        self.cols
    }
    fn mi_at(&self, row: usize, col: usize) -> MiInfo {
        self.mi[row * REF_STRIDE + col]
    }
}

/// One generated case: everything both implementations are fed.
struct RefCase {
    grid: RefGrid,
    prev: Vec<MvRefRow>,
    use_prev: bool,
    sign_bias: [bool; 4],
    tile: TileBounds,
    mi_row: usize,
    mi_col: usize,
    bsize: u8,
    ref_frame: i8,
    block: i32,
    allow_hp: bool,
    cur: MiInfo,
}

/// Rebuilds case `n` of the reference sequence. **The order of the draws is
/// the contract** with the C harness — every `range` call below matches one
/// there, including the conditional ones (`ref_frame[1]`, and `block`, which
/// is only drawn for a sub-8x8 block).
fn build_ref_case(rng: &mut Lcg) -> RefCase {
    let mi_rows = 5 + rng.range(4) as usize;
    let mi_cols = 5 + rng.range(4) as usize;
    let tile_start = if rng.flag() { mi_cols / 2 } else { 0 };
    let tile_end = mi_cols;
    let span = tile_end - tile_start;
    let mi_row = rng.range(mi_rows as u32) as usize;
    let mi_col = tile_start + rng.range(span as u32) as usize;
    let bsize = rng.range(13) as u8;
    let ref_frame = 1 + rng.range(3) as i8;
    let use_prev = rng.flag();
    let allow_hp = rng.flag();

    let mut sign_bias = [false; 4];
    for bias in sign_bias.iter_mut().skip(1) {
        *bias = rng.flag();
    }

    let mut mi = vec![MiInfo::default(); REF_STRIDE * REF_STRIDE];
    for entry in &mut mi {
        let kind = rng.range(4);
        let mut m = MiInfo::default();
        match kind {
            0 => {
                m.sb_type = 3 + rng.range(10) as u8;
                m.mode = rng.range(10) as u8;
                m.ref_frame = [INTRA_FRAME, NONE_FRAME];
            }
            1 => {
                m.sb_type = 3 + rng.range(10) as u8;
                m.mode = 10 + rng.range(4) as u8;
                m.ref_frame = [1 + rng.range(3) as i8, NONE_FRAME];
                m.mv[0] = rng.motion_vector();
                m.is_inter = true;
            }
            2 => {
                m.sb_type = 3 + rng.range(10) as u8;
                m.mode = 10 + rng.range(4) as u8;
                m.ref_frame = [1 + rng.range(3) as i8, 1 + rng.range(3) as i8];
                m.mv[0] = rng.motion_vector();
                m.mv[1] = rng.motion_vector();
                m.is_inter = true;
            }
            _ => {
                m.sb_type = rng.range(3) as u8;
                m.mode = 10 + rng.range(4) as u8;
                m.ref_frame[0] = 1 + rng.range(3) as i8;
                m.ref_frame[1] = if rng.flag() {
                    1 + rng.range(3) as i8
                } else {
                    NONE_FRAME
                };
                for sub in 0..4 {
                    m.bmv[sub][0] = rng.motion_vector();
                    m.bmv[sub][1] = rng.motion_vector();
                }
                m.mv = m.bmv[3];
                m.is_inter = true;
            }
        }
        *entry = m;
    }

    let mut prev = vec![MvRefRow::default(); REF_STRIDE * REF_STRIDE];
    for record in &mut prev {
        let kind = rng.range(3);
        let mut r = MvRefRow {
            ref_frame: [INTRA_FRAME, NONE_FRAME],
            mv: [MotionVector::zero(); 2],
        };
        if kind != 0 {
            r.ref_frame[0] = 1 + rng.range(3) as i8;
            r.ref_frame[1] = if rng.flag() {
                1 + rng.range(3) as i8
            } else {
                NONE_FRAME
            };
            r.mv[0] = rng.motion_vector();
            r.mv[1] = rng.motion_vector();
        }
        *record = r;
    }

    let mut cur = MiInfo {
        sb_type: bsize,
        mode: 10 + rng.range(4) as u8,
        is_inter: true,
        ref_frame: [ref_frame, 1 + rng.range(3) as i8],
        ..MiInfo::default()
    };
    for sub in 0..4 {
        cur.bmv[sub][0] = rng.motion_vector();
        cur.bmv[sub][1] = rng.motion_vector();
    }
    let block = if bsize < BLOCK_8X8 {
        rng.range(4) as i32
    } else {
        -1
    };

    RefCase {
        grid: RefGrid {
            rows: mi_rows,
            cols: mi_cols,
            mi,
        },
        prev,
        use_prev,
        sign_bias,
        tile: TileBounds {
            mi_col_start: tile_start,
            mi_col_end: tile_end,
        },
        mi_row,
        mi_col,
        bsize,
        ref_frame,
        block,
        allow_hp,
        cur,
    }
}

/// Every candidate list, mode context, `find_best_ref_mvs` pair and sub-8x8
/// predictor pair must equal the reference decoder's, over 400 pseudo-random
/// neighbourhoods.
///
/// The expected values in `reference_cases.txt` are the output of the
/// **verbatim** libvpx v1.15.2 `find_mv_refs_idx`, `vp9_find_best_ref_mvs`
/// and `vp9_append_sub8x8_mvs_for_idx` bodies compiled against stub types —
/// a reference decoder, not a second reading of the same source. See the
/// header of that file.
#[test]
fn find_mv_refs_matches_the_libvpx_reference_decoder() {
    let expected = include_str!("reference_cases.txt");
    let mut rng = Lcg(0x0123_4567);
    let mut cases = 0usize;
    let mut sub8x8_cases = 0usize;

    for (line_no, line) in expected
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .enumerate()
    {
        let f: Vec<i32> = line
            .split_whitespace()
            .map(|t| t.parse().expect("reference field is an integer"))
            .collect();
        let case = build_ref_case(&mut rng);
        let label = format!(
            "case {line_no}: bsize {} at ({},{}) in {}x{} tile [{},{}) ref {} block {} \
             use_prev {} sign_bias {:?}",
            case.bsize,
            case.mi_row,
            case.mi_col,
            case.grid.rows,
            case.grid.cols,
            case.tile.mi_col_start,
            case.tile.mi_col_end,
            case.ref_frame,
            case.block,
            case.use_prev,
            case.sign_bias
        );
        let expected_len = if case.bsize < BLOCK_8X8 { 26 } else { 10 };
        assert_eq!(f.len(), expected_len, "{label}: field count");
        // Guards that the two generators are still in step: a drift in the
        // draw order would show up here rather than as a mysterious value
        // mismatch.
        assert_eq!(i32::from(case.allow_hp), f[5], "{label}: allow_hp");

        let prev = if case.use_prev {
            Some(case.prev.as_slice())
        } else {
            None
        };
        let result = find_mv_refs(
            &case.grid,
            &case.tile,
            prev,
            &case.sign_bias,
            case.ref_frame,
            case.mi_row,
            case.mi_col,
            case.bsize,
            case.block,
        );
        assert_eq!(
            [
                i32::from(result.list[0].row),
                i32::from(result.list[0].col),
                i32::from(result.list[1].row),
                i32::from(result.list[1].col),
            ],
            [f[0], f[1], f[2], f[3]],
            "{label}: candidate list"
        );
        assert_eq!(
            i32::from(result.mode_context),
            f[4],
            "{label}: mode context"
        );

        let (nearest, near) = find_best_ref_mvs(&result.list, case.allow_hp);
        assert_eq!(
            [
                i32::from(nearest.row),
                i32::from(nearest.col),
                i32::from(near.row),
                i32::from(near.col),
            ],
            [f[6], f[7], f[8], f[9]],
            "{label}: find_best_ref_mvs (also pins that clamp_mv2 is inert)"
        );

        if case.bsize < BLOCK_8X8 {
            sub8x8_cases += 1;
            for sub in 0..4usize {
                let out = append_sub8x8_mvs_for_idx(
                    &case.grid,
                    &case.tile,
                    prev,
                    &case.sign_bias,
                    &case.cur,
                    0,
                    sub,
                    case.mi_row,
                    case.mi_col,
                );
                let base = 10 + sub * 4;
                assert_eq!(
                    [
                        i32::from(out.nearest.row),
                        i32::from(out.nearest.col),
                        i32::from(out.near.row),
                        i32::from(out.near.col),
                    ],
                    [f[base], f[base + 1], f[base + 2], f[base + 3]],
                    "{label}: sub-8x8 block {sub}"
                );
            }
        }
        cases += 1;
    }

    assert_eq!(cases, 400, "every reference case must be checked");
    assert!(
        sub8x8_cases >= 50,
        "the sequence must exercise the sub-8x8 path ({sub8x8_cases} cases)"
    );
}
