//! Frame-level deblocking driver.
//!
//! Walks every macroblock in raster order and applies the in-loop
//! deblocking filter to all 4×4-aligned edges of every macroblock:
//! one external left edge, one external top edge, three internal
//! vertical edges, three internal horizontal edges per MB.  Reads
//! and writes directly into the [`Frame`] so that filtered pixels
//! from one macroblock feed into the next macroblock's boundary
//! strength derivation.
//!
//! ## Scope
//!
//! - Luma plane only.  Chroma 4:2:0 deblocking is mechanically
//!   similar (smaller block sizes + Cb / Cr planes) and lands in
//!   a follow-up.
//! - `disable_deblocking_filter_idc = 0` is assumed (deblock all
//!   edges).  The other two modes (skip slice boundaries / skip
//!   external edges) are honoured by setting [`DeblockMbState::skip_external_filter`]
//!   on the affected boundary macroblocks.

use crate::h264::deblock::{
    alpha_threshold, beta_threshold, boundary_strength, normal_filter_line, should_filter_line,
    strong_filter_line, DeblockBlockInfo,
};
use crate::h264::frame::Frame;

/// Per-macroblock state the deblocker consumes.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeblockMbState {
    /// Luma QP at this macroblock.
    pub qp_y: u8,
    /// Macroblock is intra-coded (forces external-edge BS to 4).
    pub is_intra: bool,
    /// Per-4×4-block info (raster order inside the macroblock).
    /// Drives the spec's `bS` derivation for internal edges and
    /// for the "neighbour side" of external edges.
    pub block_info: [DeblockBlockInfo; 16],
    /// `true` when external-edge filtering must be skipped for
    /// this macroblock — supports `disable_deblocking_filter_idc`
    /// modes 1 and 2.
    pub skip_external_filter: bool,
}

/// Deblocks the luma plane of `frame`.
///
/// `states` is indexed `mb_y * pic_width_mbs + mb_x` and must
/// describe every macroblock in the picture.
///
/// `tc0_table` is the per-bS clipping table for the normal filter
/// (indexed `bs - 1` with bs ∈ 1..=3).
pub fn deblock_frame_luma(
    frame: &mut Frame,
    pic_width_mbs: usize,
    pic_height_mbs: usize,
    states: &[DeblockMbState],
    tc0_table: [u8; 3],
) {
    debug_assert_eq!(states.len(), pic_width_mbs * pic_height_mbs);

    for mb_y in 0..pic_height_mbs {
        for mb_x in 0..pic_width_mbs {
            let idx = mb_y * pic_width_mbs + mb_x;
            let mb = states[idx];
            if mb.skip_external_filter && mb_x == 0 && mb_y == 0 {
                // Whole picture has filtering disabled.
                continue;
            }
            let alpha = alpha_threshold(mb.qp_y);
            let beta = beta_threshold(mb.qp_y);
            let px = mb_x * 16;
            let py = mb_y * 16;

            // Vertical edges: edge_x ∈ {0, 4, 8, 12} → 4 edges per MB.
            for vert in 0..4 {
                let edge_x = px + vert * 4;
                let is_external = vert == 0;
                if is_external && (mb_x == 0 || mb.skip_external_filter) {
                    continue;
                }
                for row in 0..16 {
                    let bs = if is_external {
                        let left_mb = &states[idx - 1];
                        derive_edge_bs(
                            left_mb,
                            &mb,
                            (row / 4) * 4 + 3,        // left MB's rightmost-column 4×4
                            (row / 4) * 4,            // this MB's leftmost-column 4×4
                            mb.is_intra || left_mb.is_intra,
                        )
                    } else {
                        let l = (row / 4) * 4 + (vert - 1);
                        let r = (row / 4) * 4 + vert;
                        boundary_strength(mb.block_info[l], mb.block_info[r], false)
                    };
                    if bs == 0 {
                        continue;
                    }
                    apply_vertical(frame, edge_x, py + row, bs, alpha, beta, tc0_table);
                }
            }

            // Horizontal edges: edge_y ∈ {0, 4, 8, 12}.
            for horiz in 0..4 {
                let edge_y = py + horiz * 4;
                let is_external = horiz == 0;
                if is_external && (mb_y == 0 || mb.skip_external_filter) {
                    continue;
                }
                for col in 0..16 {
                    let bs = if is_external {
                        let top_mb = &states[idx - pic_width_mbs];
                        derive_edge_bs(
                            top_mb,
                            &mb,
                            12 + (col / 4), // top MB's bottommost-row 4×4
                            col / 4,        // this MB's topmost-row 4×4
                            mb.is_intra || top_mb.is_intra,
                        )
                    } else {
                        let above = (horiz - 1) * 4 + (col / 4);
                        let below = horiz * 4 + (col / 4);
                        boundary_strength(mb.block_info[above], mb.block_info[below], false)
                    };
                    if bs == 0 {
                        continue;
                    }
                    apply_horizontal(frame, px + col, edge_y, bs, alpha, beta, tc0_table);
                }
            }
        }
    }
}

/// Picks the boundary strength for an external (MB-boundary)
/// edge.  When either side is intra (`force_intra_bs`) we use the
/// strong-filter `bS = 4`.  Otherwise we run the standard
/// derivation against the relevant 4×4 sub-block info on each
/// side.
fn derive_edge_bs(
    side_a: &DeblockMbState,
    side_b: &DeblockMbState,
    block_a: usize,
    block_b: usize,
    force_intra_bs: bool,
) -> u8 {
    if force_intra_bs {
        4
    } else {
        boundary_strength(side_a.block_info[block_a], side_b.block_info[block_b], true)
    }
}

/// Applies the deblocking filter to one vertical-edge line of the
/// luma plane.  `edge_x` is the column at the boundary (samples
/// `[edge_x - 2 .. edge_x + 2)` are read and possibly updated);
/// `row` is the absolute luma row.
fn apply_vertical(
    frame: &mut Frame,
    edge_x: usize,
    row: usize,
    bs: u8,
    alpha: u8,
    beta: u8,
    tc0_table: [u8; 3],
) {
    if edge_x < 2 || edge_x + 1 >= frame.width {
        return;
    }
    let read = |x: usize| frame.get_luma(x, row).unwrap_or(0);
    let p1 = read(edge_x - 2);
    let p0 = read(edge_x - 1);
    let q0 = read(edge_x);
    let q1 = read(edge_x + 1);
    if !should_filter_line(p1, p0, q0, q1, alpha, beta) {
        return;
    }
    if bs == 4 {
        if edge_x < 3 || edge_x + 2 >= frame.width {
            return;
        }
        let p2 = read(edge_x - 3);
        let q2 = read(edge_x + 2);
        let out = strong_filter_line([p2, p1, p0, q0, q1, q2]);
        frame.set_luma(edge_x - 2, row, out[1]);
        frame.set_luma(edge_x - 1, row, out[2]);
        frame.set_luma(edge_x, row, out[3]);
        frame.set_luma(edge_x + 1, row, out[4]);
    } else {
        let tc0 = tc0_table[(bs - 1) as usize];
        let out = normal_filter_line([p1, p0, q0, q1], tc0);
        frame.set_luma(edge_x - 1, row, out[1]);
        frame.set_luma(edge_x, row, out[2]);
    }
}

/// Applies the deblocking filter to one horizontal-edge line.
fn apply_horizontal(
    frame: &mut Frame,
    col: usize,
    edge_y: usize,
    bs: u8,
    alpha: u8,
    beta: u8,
    tc0_table: [u8; 3],
) {
    if edge_y < 2 || edge_y + 1 >= frame.height {
        return;
    }
    let read = |y: usize| frame.get_luma(col, y).unwrap_or(0);
    let p1 = read(edge_y - 2);
    let p0 = read(edge_y - 1);
    let q0 = read(edge_y);
    let q1 = read(edge_y + 1);
    if !should_filter_line(p1, p0, q0, q1, alpha, beta) {
        return;
    }
    if bs == 4 {
        if edge_y < 3 || edge_y + 2 >= frame.height {
            return;
        }
        let p2 = read(edge_y - 3);
        let q2 = read(edge_y + 2);
        let out = strong_filter_line([p2, p1, p0, q0, q1, q2]);
        frame.set_luma(col, edge_y - 2, out[1]);
        frame.set_luma(col, edge_y - 1, out[2]);
        frame.set_luma(col, edge_y, out[3]);
        frame.set_luma(col, edge_y + 1, out[4]);
    } else {
        let tc0 = tc0_table[(bs - 1) as usize];
        let out = normal_filter_line([p1, p0, q0, q1], tc0);
        frame.set_luma(col, edge_y - 1, out[1]);
        frame.set_luma(col, edge_y, out[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_uniform_frame(w: usize, h: usize, luma: u8) -> Frame {
        let mut f = Frame::new(w, h);
        for y in 0..h {
            for x in 0..w {
                f.set_luma(x, y, luma);
            }
        }
        f
    }

    #[test]
    fn uniform_frame_unchanged() {
        let mut frame = make_uniform_frame(32, 16, 128);
        let states = vec![DeblockMbState::default(); 2];
        deblock_frame_luma(&mut frame, 2, 1, &states, [0, 0, 0]);
        for y in 0..16 {
            for x in 0..32 {
                assert_eq!(frame.get_luma(x, y), Some(128));
            }
        }
    }

    #[test]
    fn empty_block_info_zero_bs_skips_all_filtering() {
        // A frame with a sharp step at column 16 — without nonzero
        // BS the filter should leave it alone.
        let mut frame = make_uniform_frame(32, 16, 100);
        for y in 0..16 {
            for x in 16..32 {
                frame.set_luma(x, y, 140);
            }
        }
        let states = vec![DeblockMbState::default(); 2];
        deblock_frame_luma(&mut frame, 2, 1, &states, [0, 0, 0]);
        for y in 0..16 {
            assert_eq!(frame.get_luma(15, y), Some(100));
            assert_eq!(frame.get_luma(16, y), Some(140));
        }
    }

    #[test]
    fn intra_neighbours_force_strong_filter_at_external_edge() {
        let mut frame = make_uniform_frame(32, 16, 100);
        // Small step that fits inside the alpha threshold at QP=28
        // (alpha=20, beta=8) so the filter actually engages.
        for y in 0..16 {
            for x in 16..32 {
                frame.set_luma(x, y, 110);
            }
        }
        let states = vec![DeblockMbState {
            qp_y: 28,
            is_intra: true,
            block_info: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 16],
            skip_external_filter: false,
        }; 2];
        deblock_frame_luma(&mut frame, 2, 1, &states, [0, 0, 0]);
        // The strong filter blends samples around the boundary so
        // p0 and q0 move toward each other.
        let p0 = frame.get_luma(15, 8).unwrap();
        let q0 = frame.get_luma(16, 8).unwrap();
        assert!(p0 > 100 && p0 <= 110, "p0 = {p0} should sit in [101, 110] after deblocking");
        assert!(q0 < 110 && q0 >= 100, "q0 = {q0} should sit in [100, 109] after deblocking");
    }

    #[test]
    fn skip_external_filter_leaves_boundary_alone() {
        let mut frame = make_uniform_frame(32, 16, 100);
        for y in 0..16 {
            for x in 16..32 {
                frame.set_luma(x, y, 140);
            }
        }
        let states = vec![DeblockMbState {
            qp_y: 28,
            is_intra: true,
            block_info: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 16],
            skip_external_filter: true,
        }; 2];
        deblock_frame_luma(&mut frame, 2, 1, &states, [0, 0, 0]);
        for y in 0..16 {
            assert_eq!(frame.get_luma(15, y), Some(100));
            assert_eq!(frame.get_luma(16, y), Some(140));
        }
    }
}
