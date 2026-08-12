//! VP8 in-loop deblocking filter application (RFC 6386 §15).
//!
//! Computes the per-macroblock filter level and sweeps the simple or normal
//! edge filter (primitives in [`super::loopfilter`]) over every macroblock
//! and sub-block edge of the reconstructed frame.

use super::loopfilter::{
    compute_filter_params, normal_mbedge_filter_edge, normal_subblock_filter_edge,
    simple_filter_edge, FilterParams,
};
use super::Decoder;

impl Decoder<'_> {
    /// Computes the effective loop-filter level for a macroblock.
    ///
    /// Combines the frame base level with the per-segment level and any per-MB
    /// reference/mode deltas (RFC 6386 §15.2; dixie.c
    /// `calculate_filter_parameters`, rfc6386.txt lines 9163-9207).
    ///
    /// `ref_delta_slot` is the macroblock's reference frame indexed exactly as
    /// dixie indexes `ref_delta[]` (`0` intra/current, `1` last, `2` golden,
    /// `3` altref — [`super::mode::RefFrame::lf_ref_delta_slot`]).
    /// `mode_delta_slot` is `Some(slot)` when a *mode* delta applies at all:
    /// dixie adds one only for an intra `B_PRED` macroblock (slot 0) or for
    /// any inter macroblock (slot 1 `ZEROMV`, 3 `SPLITMV`, 2 otherwise —
    /// [`super::mode::InterMode::lf_mode_delta_slot`]); a whole-block intra
    /// macroblock takes none, which `None` expresses (lines 9192-9206).
    pub(super) fn compute_mb_filter_level(
        &self,
        segment_id: u8,
        ref_delta_slot: usize,
        mode_delta_slot: Option<usize>,
    ) -> i32 {
        let lf = &self.header.loop_filter;
        let mut level = lf.level;

        // Segment adjustment.
        if self.header.segment.enabled {
            let seg = self.header.segment.filter_strength[segment_id as usize];
            level = if self.header.segment.abs_delta {
                seg
            } else {
                level + seg
            };
        }
        level = level.clamp(0, 63);

        // Per-MB loop-filter deltas (RFC 6386 §15.2).
        if lf.delta_enabled {
            level += lf.ref_deltas[ref_delta_slot.min(lf.ref_deltas.len() - 1)];
            if let Some(slot) = mode_delta_slot {
                level += lf.mode_deltas[slot.min(lf.mode_deltas.len() - 1)];
            }
        }
        level.clamp(0, 63)
    }

    /// Runs the in-loop deblocking filter over the whole frame.
    ///
    /// Filters macroblock edges and (for the normal filter) the three interior
    /// 4-pixel sub-block edges of each macroblock. Edge order is the standard
    /// "all vertical edges of the MB left-to-right, then all horizontal edges
    /// top-to-bottom" sweep (RFC 6386 §15.1).
    pub(super) fn apply_loop_filter(&mut self) {
        if self.header.loop_filter.level == 0 {
            return;
        }
        let sharpness = self.header.loop_filter.sharpness;
        let simple = self.header.loop_filter.simple;

        for mb_y in 0..self.mb_rows {
            for mb_x in 0..self.mb_cols {
                let info = self.mb_info[mb_y * self.mb_cols + mb_x];
                if info.filter_level == 0 {
                    continue;
                }
                let params =
                    compute_filter_params(info.filter_level, sharpness, self.header.is_keyframe);
                // Inner (sub-block) edges are skipped when the MB carried no
                // coefficients at all and its mode does not force them
                // (B_PRED / SPLITMV) — see `MbInfo::filter_inner`.
                if simple {
                    self.filter_mb_simple(mb_x, mb_y, &params, info.filter_inner);
                } else {
                    self.filter_mb_normal(mb_x, mb_y, &params, info.filter_inner);
                }
            }
        }
    }

    /// Applies the simple loop filter to one macroblock (luma only).
    fn filter_mb_simple(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        params: &FilterParams,
        filter_inner: bool,
    ) {
        let stride = self.planes.y_stride;
        let off = self.planes.y_origin + mb_y * 16 * stride + mb_x * 16;

        // Left macroblock edge (vertical), 16 rows.
        if mb_x > 0 {
            for row in 0..16 {
                let p = off + row * stride;
                simple_filter_edge(&mut self.planes.y, p, 1, params.mbedge_limit);
            }
        }
        // Interior vertical sub-block edges at columns 4, 8, 12.
        if filter_inner {
            for col in [4usize, 8, 12] {
                for row in 0..16 {
                    let p = off + row * stride + col;
                    simple_filter_edge(&mut self.planes.y, p, 1, params.sub_bedge_limit);
                }
            }
        }
        // Top macroblock edge (horizontal), 16 columns.
        if mb_y > 0 {
            for col in 0..16 {
                let p = off + col;
                simple_filter_edge(&mut self.planes.y, p, stride, params.mbedge_limit);
            }
        }
        // Interior horizontal sub-block edges at rows 4, 8, 12.
        if filter_inner {
            for row in [4usize, 8, 12] {
                for col in 0..16 {
                    let p = off + row * stride + col;
                    simple_filter_edge(&mut self.planes.y, p, stride, params.sub_bedge_limit);
                }
            }
        }
    }

    /// Applies the normal loop filter to one macroblock (luma + chroma).
    fn filter_mb_normal(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        params: &FilterParams,
        filter_inner: bool,
    ) {
        let y_stride = self.planes.y_stride;
        let uv_stride = self.planes.uv_stride;
        let y_off = self.planes.y_origin + mb_y * 16 * y_stride + mb_x * 16;
        let uv_off = self.planes.uv_origin + mb_y * 8 * uv_stride + mb_x * 8;

        // --- left macroblock edge ---
        if mb_x > 0 {
            for row in 0..16 {
                let p = y_off + row * y_stride;
                normal_mbedge_filter_edge(&mut self.planes.y, p, 1, params);
            }
            for row in 0..8 {
                let p = uv_off + row * uv_stride;
                normal_mbedge_filter_edge(&mut self.planes.u, p, 1, params);
                normal_mbedge_filter_edge(&mut self.planes.v, p, 1, params);
            }
        }
        // --- interior vertical sub-block edges ---
        if filter_inner {
            for col in [4usize, 8, 12] {
                for row in 0..16 {
                    let p = y_off + row * y_stride + col;
                    normal_subblock_filter_edge(&mut self.planes.y, p, 1, params);
                }
            }
            // Chroma has a single interior vertical edge at column 4.
            for row in 0..8 {
                let p = uv_off + row * uv_stride + 4;
                normal_subblock_filter_edge(&mut self.planes.u, p, 1, params);
                normal_subblock_filter_edge(&mut self.planes.v, p, 1, params);
            }
        }
        // --- top macroblock edge ---
        if mb_y > 0 {
            for col in 0..16 {
                let p = y_off + col;
                normal_mbedge_filter_edge(&mut self.planes.y, p, y_stride, params);
            }
            for col in 0..8 {
                let p = uv_off + col;
                normal_mbedge_filter_edge(&mut self.planes.u, p, uv_stride, params);
                normal_mbedge_filter_edge(&mut self.planes.v, p, uv_stride, params);
            }
        }
        // --- interior horizontal sub-block edges ---
        if filter_inner {
            for row in [4usize, 8, 12] {
                for col in 0..16 {
                    let p = y_off + row * y_stride + col;
                    normal_subblock_filter_edge(&mut self.planes.y, p, y_stride, params);
                }
            }
            for col in 0..8 {
                let p = uv_off + 4 * uv_stride + col;
                normal_subblock_filter_edge(&mut self.planes.u, p, uv_stride, params);
                normal_subblock_filter_edge(&mut self.planes.v, p, uv_stride, params);
            }
        }
    }
}
