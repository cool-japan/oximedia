//! AV1 deblocking loop filter (spec 7.14), intra-frame path.
//!
//! Exact port of the loop filter process: per-plane vertical-then-horizontal
//! passes, the edge loop filter process with block/transform edge detection,
//! and the filter size and adaptive filter strength processes (with per-block
//! `DeltaLFs` and segment features).
//!
//! The sample filtering process itself — the filter mask, the narrow
//! (filter4) filter and the wide 6/8/14-tap filters — lives in
//! `oximedia_simd::av1_loopfilter`, which filters the four sample lines of an
//! edge as one SIMD batch.  That crate carries a literal transcription of the
//! spec alongside the optimised paths and proves them byte-identical.
//!
//! On intra frames every block satisfies `isIntra == 1`, `ref ==
//! INTRA_FRAME` and `modeType == 0` (all Y modes are intra), which this
//! implementation relies on (it decodes keyframes/intra-only frames only).

#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use super::consts::{FRAME_LF_COUNT, MAX_LOOP_FILTER, SEG_LVL_MAX};
use super::hdr::FrameHdr;
use super::tables_conv::{TX_HEIGHT, TX_WIDTH};

/// Everything the loop filter needs from the decoded frame state.
pub struct LfInput<'a> {
    pub hdr: &'a FrameHdr,
    pub sub_x: bool,
    pub sub_y: bool,
    pub num_planes: usize,
    pub mi_rows: usize,
    pub mi_cols: usize,
    /// Per-MI grids from reconstruction.
    pub mi_sizes: &'a [u8],
    pub skips: &'a [u8],
    pub seg_ids: &'a [u8],
    pub delta_lfs: &'a [[i8; FRAME_LF_COUNT]],
    /// `LoopfilterTxSizes[plane]` at plane-subsampled MI positions (stored
    /// on the shared MI grid).
    pub lf_tx_sizes: &'a [Vec<u8>; 3],
}

/// `SEG_LVL_ALT_LF_Y_V` (spec symbols) — base index of the LF features.
const SEG_LVL_ALT_LF_Y_V: usize = 1;

/// Applies the loop filter to all three planes in place.
pub fn loop_filter_frame(planes: &mut [super::recon::PlaneBuf; 3], input: &LfInput<'_>) {
    for plane in 0..input.num_planes {
        if plane == 0 || input.hdr.lf.level[1 + plane] != 0 {
            for pass in 0..2 {
                let row_step = if plane == 0 {
                    1
                } else {
                    1 << usize::from(input.sub_y)
                };
                let col_step = if plane == 0 {
                    1
                } else {
                    1 << usize::from(input.sub_x)
                };
                let mut row = 0;
                while row < input.mi_rows {
                    let mut col = 0;
                    while col < input.mi_cols {
                        loop_filter_edge(planes, input, plane, pass, row, col);
                        col += col_step;
                    }
                    row += row_step;
                }
            }
        }
    }
}

#[inline]
fn grid(input: &LfInput<'_>, r: usize, c: usize) -> usize {
    r * input.mi_cols + c
}

/// `loop_filter_edge` / edge loop filter process (spec 7.14.2).
fn loop_filter_edge(
    planes: &mut [super::recon::PlaneBuf; 3],
    input: &LfInput<'_>,
    plane: usize,
    pass: usize,
    row: usize,
    col: usize,
) {
    let (sub_x, sub_y) = if plane == 0 {
        (0usize, 0usize)
    } else {
        (usize::from(input.sub_x), usize::from(input.sub_y))
    };
    let (dx, dy) = if pass == 0 { (1usize, 0usize) } else { (0, 1) };
    let x = col * 4;
    let y = row * 4;
    let row = row | sub_y;
    let col = col | sub_x;

    // onScreen determination (frame dimensions, not MI-aligned).
    let on_screen = x < input.hdr.frame_width as usize
        && y < input.hdr.frame_height as usize
        && !(pass == 0 && x == 0)
        && !(pass == 1 && y == 0);
    if !on_screen {
        return;
    }

    let xp = x >> sub_x;
    let yp = y >> sub_y;
    let prev_row = row - (dy << sub_y);
    let prev_col = col - (dx << sub_x);

    let mi_size = usize::from(input.mi_sizes[grid(input, row, col)]);
    let tx_sz = usize::from(input.lf_tx_sizes[plane][grid(input, row >> sub_y, col >> sub_x)]);
    let plane_size = usize::from(
        super::tables_conv::SUBSAMPLED_SIZE[mi_size][if plane > 0 { sub_x } else { 0 }]
            [if plane > 0 { sub_y } else { 0 }],
    );
    let skip = input.skips[grid(input, row, col)] != 0;
    // isIntra == 1 always on intra frames.
    let prev_tx_sz =
        usize::from(input.lf_tx_sizes[plane][grid(input, prev_row >> sub_y, prev_col >> sub_x)]);

    // The spec's applyFilter condition is
    //   isTxEdge && (isBlockEdge || skip == 0 || isIntra == 1).
    // On intra frames the isIntra term is always 1, so applyFilter
    // degenerates to isTxEdge and the isBlockEdge/skip terms need not be
    // evaluated (plane_size and the skip grid feed only that condition).
    let _ = (plane_size, skip);
    let is_tx_edge = if pass == 0 {
        xp % usize::from(TX_WIDTH[tx_sz]) == 0
    } else {
        yp % usize::from(TX_HEIGHT[tx_sz]) == 0
    };
    if !is_tx_edge {
        return;
    }

    // Filter size process (spec 7.14.3).
    let base_size = if pass == 0 {
        core::cmp::min(
            usize::from(TX_WIDTH[prev_tx_sz]),
            usize::from(TX_WIDTH[tx_sz]),
        )
    } else {
        core::cmp::min(
            usize::from(TX_HEIGHT[prev_tx_sz]),
            usize::from(TX_HEIGHT[tx_sz]),
        )
    };
    let filter_size = if plane == 0 {
        core::cmp::min(16, base_size)
    } else {
        core::cmp::min(8, base_size)
    };

    // Adaptive filter strength (spec 7.14.4), falling back to the previous
    // block when lvl == 0.
    let (mut lvl, mut limit, mut blimit, mut thresh) =
        adaptive_filter_strength(input, row, col, plane, pass);
    if lvl == 0 {
        let (l2, li2, b2, t2) = adaptive_filter_strength(input, prev_row, prev_col, plane, pass);
        lvl = l2;
        limit = li2;
        blimit = b2;
        thresh = t2;
    }

    if lvl > 0 {
        // The spec filters the four sample lines of this edge one at a time
        // (`for i in 0..4`).  Those lines are independent — for a vertical
        // edge they are four distinct rows, for a horizontal edge four
        // distinct columns — so they are handed to the SIMD kernel as one
        // four-lane batch.  See `oximedia_simd::av1_loopfilter`.
        let p = &mut planes[plane];
        super::simd::filter_edge_4lines(
            p,
            xp,
            yp,
            dx,
            dy,
            filter_size,
            plane,
            limit,
            blimit,
            thresh,
        );
    }
}

/// Adaptive filter strength process (spec 7.14.4) + selection (7.14.5).
fn adaptive_filter_strength(
    input: &LfInput<'_>,
    row: usize,
    col: usize,
    plane: usize,
    pass: usize,
) -> (i32, i32, i32, i32) {
    let segment = usize::from(input.seg_ids[grid(input, row, col)]);
    // ref == INTRA_FRAME (0), modeType == 0 on intra frames.
    let i = if plane == 0 { pass } else { plane + 1 };
    let delta_lf = if input.hdr.delta_lf_multi {
        i32::from(input.delta_lfs[grid(input, row, col)][i])
    } else {
        i32::from(input.delta_lfs[grid(input, row, col)][0])
    };

    // Selection process.
    let base_filter_level =
        (delta_lf + input.hdr.lf.level[i] as i32).clamp(0, MAX_LOOP_FILTER as i32);
    let mut lvl_seg = base_filter_level;
    let feature = SEG_LVL_ALT_LF_Y_V + i;
    debug_assert!(feature < SEG_LVL_MAX);
    if input.hdr.seg.enabled && input.hdr.seg.feature_enabled[segment][feature] {
        lvl_seg = (input.hdr.seg.feature_data[segment][feature] + lvl_seg)
            .clamp(0, MAX_LOOP_FILTER as i32);
    }
    if input.hdr.lf.delta_enabled {
        // ref == INTRA_FRAME on intra frames.
        let n_shift = lvl_seg >> 5;
        lvl_seg += input.hdr.lf.ref_deltas[0] << n_shift;
        lvl_seg = lvl_seg.clamp(0, MAX_LOOP_FILTER as i32);
    }
    let lvl = lvl_seg;

    let sharpness = input.hdr.lf.sharpness as i32;
    let shift = if sharpness > 4 {
        2
    } else if sharpness > 0 {
        1
    } else {
        0
    };
    let limit = if sharpness > 0 {
        (lvl >> shift).clamp(1, 9 - sharpness)
    } else {
        core::cmp::max(1, lvl >> shift)
    };
    let blimit = 2 * (lvl + 2) + limit;
    let thresh = lvl >> 4;
    (lvl, limit, blimit, thresh)
}

// The sample filtering process itself (spec 7.14.6.1 mask process, 7.14.6.3
// narrow filter, 7.14.6.4 wide filter) lives in
// `oximedia_simd::av1_loopfilter`, which carries a literal transcription of
// the spec alongside the optimised paths and proves them byte-identical over
// an exhaustive sweep of filter levels, sharpness values, block sizes and
// planes.
