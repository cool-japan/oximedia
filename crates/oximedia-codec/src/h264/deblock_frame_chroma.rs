//! Frame-level chroma deblocking pass (4:2:0).
//!
//! The chroma planes are half-resolution per axis, so one
//! macroblock covers an 8×8 chroma block per plane (4 × 4×4
//! sub-blocks).  Each chroma 8×8 has only two block-aligned
//! edges per axis:
//!
//! ```text
//! Vertical:   external (mb_x · 8) and one internal (mb_x · 8 + 4)
//! Horizontal: external (mb_y · 8) and one internal (mb_y · 8 + 4)
//! ```
//!
//! Per ITU-T Rec. H.264 / ISO/IEC 14496-10 § 8.7.3.3 the chroma
//! strong filter (bS = 4) only touches `p0` and `q0` — unlike the
//! luma version which also rewrites `p1` / `q1` / `p2` / `q2`.
//! Otherwise the alpha / beta / normal-filter math is the same.

use crate::h264::deblock::{
    alpha_threshold, beta_threshold, boundary_strength, normal_filter_line, should_filter_line,
    DeblockBlockInfo,
};
use crate::h264::deblock_frame::DeblockMbState;
use crate::h264::frame::Frame;

/// Per-macroblock chroma state needed for the deblocker.  Two
/// 4-entry block-info strips — one for Cb (4 4×4 chroma sub-blocks
/// in raster) and one for Cr.  The luma `DeblockMbState` provides
/// `is_intra`, `qp_y` (used to derive `qp_chroma` separately), and
/// `skip_external_filter`; this struct fills in chroma-specific
/// info.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeblockChromaInfo {
    /// Chroma QP for this macroblock.
    pub qp_chroma: u8,
    /// 4-entry strip for Cb (raster within the chroma 8×8).
    pub cb_blocks: [DeblockBlockInfo; 4],
    /// 4-entry strip for Cr.
    pub cr_blocks: [DeblockBlockInfo; 4],
}

/// Deblocks both chroma planes of `frame` for a 4:2:0 picture.
///
/// `luma_states` mirrors the input to
/// [`crate::h264::deblock_frame::deblock_frame_luma`] (used here
/// for `is_intra` and `skip_external_filter`); `chroma_states`
/// carries the chroma-specific block info and QP.
pub fn deblock_frame_chroma_420(
    frame: &mut Frame,
    pic_width_mbs: usize,
    pic_height_mbs: usize,
    luma_states: &[DeblockMbState],
    chroma_states: &[DeblockChromaInfo],
    tc0_table: [u8; 3],
) {
    debug_assert_eq!(luma_states.len(), pic_width_mbs * pic_height_mbs);
    debug_assert_eq!(chroma_states.len(), pic_width_mbs * pic_height_mbs);

    for plane in [ChromaPlane::Cb, ChromaPlane::Cr] {
        for mb_y in 0..pic_height_mbs {
            for mb_x in 0..pic_width_mbs {
                let idx = mb_y * pic_width_mbs + mb_x;
                let mb = luma_states[idx];
                let cm = chroma_states[idx];
                if mb.skip_external_filter && mb_x == 0 && mb_y == 0 {
                    continue;
                }
                let alpha = alpha_threshold(cm.qp_chroma);
                let beta = beta_threshold(cm.qp_chroma);
                let cx = mb_x * 8;
                let cy = mb_y * 8;
                let plane_blocks = match plane {
                    ChromaPlane::Cb => cm.cb_blocks,
                    ChromaPlane::Cr => cm.cr_blocks,
                };

                // Vertical edges at chroma column 0 (external) and 4 (internal).
                for vert_idx in 0..2 {
                    let edge_x = cx + vert_idx * 4;
                    let is_external = vert_idx == 0;
                    if is_external && (mb_x == 0 || mb.skip_external_filter) {
                        continue;
                    }
                    for row in 0..8 {
                        let bs = if is_external {
                            let left = luma_states[idx - 1];
                            let left_chroma = chroma_states[idx - 1];
                            let left_block = match plane {
                                ChromaPlane::Cb => left_chroma.cb_blocks[(row / 4) * 2 + 1],
                                ChromaPlane::Cr => left_chroma.cr_blocks[(row / 4) * 2 + 1],
                            };
                            let cur_block = plane_blocks[(row / 4) * 2];
                            if mb.is_intra || left.is_intra {
                                4
                            } else {
                                boundary_strength(left_block, cur_block, true)
                            }
                        } else {
                            let l = (row / 4) * 2;
                            let r = (row / 4) * 2 + 1;
                            boundary_strength(plane_blocks[l], plane_blocks[r], false)
                        };
                        if bs == 0 {
                            continue;
                        }
                        apply_chroma_vertical(
                            frame, plane, edge_x, cy + row, bs, alpha, beta, tc0_table,
                        );
                    }
                }

                // Horizontal edges at chroma row 0 (external) and 4 (internal).
                for horiz_idx in 0..2 {
                    let edge_y = cy + horiz_idx * 4;
                    let is_external = horiz_idx == 0;
                    if is_external && (mb_y == 0 || mb.skip_external_filter) {
                        continue;
                    }
                    for col in 0..8 {
                        let bs = if is_external {
                            let above = luma_states[idx - pic_width_mbs];
                            let above_chroma = chroma_states[idx - pic_width_mbs];
                            let above_block = match plane {
                                ChromaPlane::Cb => above_chroma.cb_blocks[2 + (col / 4)],
                                ChromaPlane::Cr => above_chroma.cr_blocks[2 + (col / 4)],
                            };
                            let cur_block = plane_blocks[col / 4];
                            if mb.is_intra || above.is_intra {
                                4
                            } else {
                                boundary_strength(above_block, cur_block, true)
                            }
                        } else {
                            let above = col / 4;
                            let below = 2 + (col / 4);
                            boundary_strength(plane_blocks[above], plane_blocks[below], false)
                        };
                        if bs == 0 {
                            continue;
                        }
                        apply_chroma_horizontal(
                            frame, plane, cx + col, edge_y, bs, alpha, beta, tc0_table,
                        );
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ChromaPlane {
    Cb,
    Cr,
}

fn read_chroma(frame: &Frame, plane: ChromaPlane, x: usize, y: usize) -> u8 {
    match plane {
        ChromaPlane::Cb => frame.get_cb(x, y).unwrap_or(0),
        ChromaPlane::Cr => frame.get_cr(x, y).unwrap_or(0),
    }
}

fn write_chroma(frame: &mut Frame, plane: ChromaPlane, x: usize, y: usize, v: u8) {
    match plane {
        ChromaPlane::Cb => frame.set_cb(x, y, v),
        ChromaPlane::Cr => frame.set_cr(x, y, v),
    }
}

fn apply_chroma_vertical(
    frame: &mut Frame,
    plane: ChromaPlane,
    edge_x: usize,
    row: usize,
    bs: u8,
    alpha: u8,
    beta: u8,
    tc0_table: [u8; 3],
) {
    let cw = frame.chroma_width();
    if edge_x < 2 || edge_x + 1 >= cw {
        return;
    }
    let p1 = read_chroma(frame, plane, edge_x - 2, row);
    let p0 = read_chroma(frame, plane, edge_x - 1, row);
    let q0 = read_chroma(frame, plane, edge_x, row);
    let q1 = read_chroma(frame, plane, edge_x + 1, row);
    if !should_filter_line(p1, p0, q0, q1, alpha, beta) {
        return;
    }
    if bs == 4 {
        // Chroma strong filter: only p0 and q0 are rewritten
        // (spec § 8.7.3.3, equations 8-487 / 8-488).
        let new_p0 = ((2 * i32::from(p1) + i32::from(p0) + i32::from(q1) + 2) >> 2) as u8;
        let new_q0 = ((2 * i32::from(q1) + i32::from(q0) + i32::from(p1) + 2) >> 2) as u8;
        write_chroma(frame, plane, edge_x - 1, row, new_p0);
        write_chroma(frame, plane, edge_x, row, new_q0);
    } else {
        let tc0 = tc0_table[(bs - 1) as usize];
        let out = normal_filter_line([p1, p0, q0, q1], tc0);
        write_chroma(frame, plane, edge_x - 1, row, out[1]);
        write_chroma(frame, plane, edge_x, row, out[2]);
    }
}

fn apply_chroma_horizontal(
    frame: &mut Frame,
    plane: ChromaPlane,
    col: usize,
    edge_y: usize,
    bs: u8,
    alpha: u8,
    beta: u8,
    tc0_table: [u8; 3],
) {
    let ch = frame.chroma_height();
    if edge_y < 2 || edge_y + 1 >= ch {
        return;
    }
    let p1 = read_chroma(frame, plane, col, edge_y - 2);
    let p0 = read_chroma(frame, plane, col, edge_y - 1);
    let q0 = read_chroma(frame, plane, col, edge_y);
    let q1 = read_chroma(frame, plane, col, edge_y + 1);
    if !should_filter_line(p1, p0, q0, q1, alpha, beta) {
        return;
    }
    if bs == 4 {
        let new_p0 = ((2 * i32::from(p1) + i32::from(p0) + i32::from(q1) + 2) >> 2) as u8;
        let new_q0 = ((2 * i32::from(q1) + i32::from(q0) + i32::from(p1) + 2) >> 2) as u8;
        write_chroma(frame, plane, col, edge_y - 1, new_p0);
        write_chroma(frame, plane, col, edge_y, new_q0);
    } else {
        let tc0 = tc0_table[(bs - 1) as usize];
        let out = normal_filter_line([p1, p0, q0, q1], tc0);
        write_chroma(frame, plane, col, edge_y - 1, out[1]);
        write_chroma(frame, plane, col, edge_y, out[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(w: usize, h: usize, chroma: u8) -> Frame {
        let mut f = Frame::new(w, h);
        for y in 0..h / 2 {
            for x in 0..w / 2 {
                f.set_cb(x, y, chroma);
                f.set_cr(x, y, chroma);
            }
        }
        f
    }

    #[test]
    fn uniform_chroma_unchanged() {
        let mut frame = make_frame(32, 16, 128);
        let luma_states = vec![DeblockMbState::default(); 2];
        let chroma_states = vec![DeblockChromaInfo::default(); 2];
        deblock_frame_chroma_420(&mut frame, 2, 1, &luma_states, &chroma_states, [0, 0, 0]);
        for y in 0..8 {
            for x in 0..16 {
                assert_eq!(frame.get_cb(x, y), Some(128));
                assert_eq!(frame.get_cr(x, y), Some(128));
            }
        }
    }

    #[test]
    fn intra_external_edge_smooths_chroma_step() {
        // 32x16 luma → 16x8 chroma.  Place a small step at chroma
        // column 8 (the MB boundary between mb_x=0 and mb_x=1).
        let mut frame = make_frame(32, 16, 100);
        for y in 0..8 {
            for x in 8..16 {
                frame.set_cb(x, y, 110);
                frame.set_cr(x, y, 110);
            }
        }
        let luma_states = vec![DeblockMbState {
            qp_y: 28,
            is_intra: true,
            block_info: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 16],
            skip_external_filter: false,
        }; 2];
        let chroma_states = vec![DeblockChromaInfo {
            qp_chroma: 28,
            cb_blocks: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 4],
            cr_blocks: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 4],
        }; 2];
        deblock_frame_chroma_420(&mut frame, 2, 1, &luma_states, &chroma_states, [0, 0, 0]);
        let p0 = frame.get_cb(7, 4).unwrap();
        let q0 = frame.get_cb(8, 4).unwrap();
        assert!(p0 > 100, "p0 = {p0} should have moved up toward q0");
        assert!(q0 < 110, "q0 = {q0} should have moved down toward p0");
    }

    #[test]
    fn skip_external_filter_leaves_chroma_boundary_alone() {
        let mut frame = make_frame(32, 16, 100);
        for y in 0..8 {
            for x in 8..16 {
                frame.set_cb(x, y, 110);
            }
        }
        let luma_states = vec![DeblockMbState {
            qp_y: 28,
            is_intra: true,
            block_info: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 16],
            skip_external_filter: true,
        }; 2];
        let chroma_states = vec![DeblockChromaInfo {
            qp_chroma: 28,
            cb_blocks: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 4],
            cr_blocks: [DeblockBlockInfo { is_intra: true, ..Default::default() }; 4],
        }; 2];
        deblock_frame_chroma_420(&mut frame, 2, 1, &luma_states, &chroma_states, [0, 0, 0]);
        for y in 0..8 {
            assert_eq!(frame.get_cb(7, y), Some(100));
            assert_eq!(frame.get_cb(8, y), Some(110));
        }
    }
}
