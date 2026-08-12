//! VP9 loop filter — exact port of libvpx `vp9/common/vp9_loopfilter.c`
//! (`vp9_filter_block_plane_non420`, the general per-MI reference path,
//! bit-identical to the ss00/ss11 mask fast paths) and the
//! `vpx_dsp/loopfilter.c` kernels (8-bit build).
//!
//! Frame application order matches `vp9_loop_filter_frame` /
//! `loop_filter_rows`: superblocks in raster order; per superblock and per
//! plane, all vertical edges first, then all horizontal edges.

#![allow(clippy::too_many_arguments)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::similar_names)]

use super::recon::{FrameMi, MiInfo, PlaneBuf};
use super::tables;
use super::tables_inter;

/// Maximum loop filter level.
const MAX_LOOP_FILTER: i32 = 63;

/// Per-level thresholds (`loop_filter_thresh`): `mblim`, `lim`, `hev_thr`.
#[derive(Clone, Copy, Default)]
struct LfThresh {
    mblim: u8,
    lim: u8,
    hev_thr: u8,
}

/// Frame-level loop filter state: thresholds per level and the filter-level
/// table indexed by segment, reference frame and loop-filter mode bucket
/// (`lfi_n->lvl[seg][ref_frame][mode_bucket]`).
pub struct LoopFilterInfo {
    thr: [LfThresh; MAX_LOOP_FILTER as usize + 1],
    /// Filter level per `[segment][ref_frame][mode_bucket]` — libvpx
    /// `loop_filter_info_n::lvl[MAX_SEGMENTS][MAX_REF_FRAMES][MAX_MODE_LF_DELTAS]`
    /// (`vp9_loopfilter.h:48`, `8 x 4 x 2`). `ref_frame` follows the
    /// [`super::refs`] convention that [`MiInfo::ref_frame`]`[0]` already
    /// uses: `0` INTRA, `1` LAST, `2` GOLDEN, `3` ALTREF. `mode_bucket` is
    /// [`tables_inter::MODE_LF_LUT`]`[mode]` (`0` or `1`).
    lvl: [[[u8; 2]; 4]; 8],
}

impl LoopFilterInfo {
    /// Builds thresholds and filter levels for an **intra-only** frame
    /// (key frame or intra-only frame): every block has `ref_frame[0] ==
    /// INTRA_FRAME` and, since every intra mode maps to loop-filter mode
    /// bucket `0` in [`tables_inter::MODE_LF_LUT`], only
    /// `lvl[seg][INTRA_FRAME][0]` is ever read back out of the table this
    /// builds — exactly the value `intra_ref_delta` alone determines.
    ///
    /// Delegates to [`Self::new_with_deltas`] with the LAST/GOLDEN/ALTREF
    /// reference deltas and both mode deltas set to `0`. Those entries are
    /// then unreachable for an all-intra frame (nothing above ever indexes
    /// them), **not** a substitute for the real per-reference/per-mode
    /// deltas a frame that may contain inter blocks needs — such callers
    /// must use [`Self::new_with_deltas`] with the frame header's actual
    /// `ref_deltas`/`mode_deltas` instead.
    #[must_use]
    pub fn new(
        default_filt_lvl: u8,
        sharpness: u8,
        delta_enabled: bool,
        intra_ref_delta: i8,
        seg_enabled: bool,
        seg_abs_delta: bool,
        seg_lf_feature: &[(bool, i16); 8],
    ) -> Self {
        Self::new_with_deltas(
            default_filt_lvl,
            sharpness,
            delta_enabled,
            [intra_ref_delta, 0, 0, 0],
            [0, 0],
            seg_enabled,
            seg_abs_delta,
            seg_lf_feature,
        )
    }

    /// Builds thresholds and the full filter-level table
    /// (`update_sharpness` + `vp9_loop_filter_frame_init`, libvpx
    /// `vp9/common/vp9_loopfilter.c:252-295`), for frames that may contain
    /// inter blocks.
    ///
    /// `ref_deltas` is `[INTRA, LAST, GOLDEN, ALTREF]` and `mode_deltas` is
    /// the two-entry mode-delta array — both match
    /// `LoopFilterHeader::ref_deltas` / `::mode_deltas`
    /// (`crate::vp9::uncompressed`) directly (libvpx `MAX_REF_LF_DELTAS ==
    /// 4`, `MAX_MODE_LF_DELTAS == 2`, `vp9_loopfilter.h:29-30,81,85`).
    #[must_use]
    pub fn new_with_deltas(
        default_filt_lvl: u8,
        sharpness: u8,
        delta_enabled: bool,
        ref_deltas: [i8; 4],
        mode_deltas: [i8; 2],
        seg_enabled: bool,
        seg_abs_delta: bool,
        seg_lf_feature: &[(bool, i16); 8],
    ) -> Self {
        let mut thr = [LfThresh::default(); MAX_LOOP_FILTER as usize + 1];
        for (lvl, t) in thr.iter_mut().enumerate() {
            let lvl = lvl as i32;
            // update_sharpness()
            let mut block_inside_limit =
                lvl >> (i32::from(sharpness > 0) + i32::from(sharpness > 4));
            if sharpness > 0 && block_inside_limit > 9 - i32::from(sharpness) {
                block_inside_limit = 9 - i32::from(sharpness);
            }
            if block_inside_limit < 1 {
                block_inside_limit = 1;
            }
            t.lim = block_inside_limit as u8;
            t.mblim = (2 * (lvl + 2) + block_inside_limit) as u8;
            // vp9_loop_filter_init(): hev_thr = lvl >> 4
            t.hev_thr = (lvl >> 4) as u8;
        }

        // vp9_loop_filter_frame_init() (vp9_loopfilter.c:252-295).
        let scale = 1i32 << (default_filt_lvl >> 5);
        let mut lvl = [[[0u8; 2]; 4]; 8];
        for (seg_id, seg_lvl) in lvl.iter_mut().enumerate() {
            let mut lvl_seg = i32::from(default_filt_lvl);
            if seg_enabled && seg_lf_feature[seg_id].0 {
                let data = i32::from(seg_lf_feature[seg_id].1);
                lvl_seg = if seg_abs_delta {
                    data
                } else {
                    i32::from(default_filt_lvl) + data
                }
                .clamp(0, MAX_LOOP_FILTER);
            }
            if !delta_enabled {
                // memset(lfi->lvl[seg_id], lvl_seg, ...) (:277-280): every
                // ref/mode-bucket entry gets the flat per-segment level
                // verbatim -- `lvl_seg` is already in `0..=MAX_LOOP_FILTER`,
                // so libvpx applies no further clamp here either.
                *seg_lvl = [[lvl_seg as u8; 2]; 4];
                continue;
            }
            // INTRA_FRAME: only mode bucket 0 is ever read back (:283-284).
            let intra_lvl = lvl_seg + i32::from(ref_deltas[0]) * scale;
            seg_lvl[0][0] = intra_lvl.clamp(0, MAX_LOOP_FILTER) as u8;
            // `seg_lvl[0][1]` (INTRA's second mode bucket) is left at its `0`
            // default: libvpx never writes it either (the loop below starts
            // at `LAST_FRAME`), and it is unreachable -- every intra mode
            // maps to bucket 0 in `tables_inter::MODE_LF_LUT`.
            //
            // LAST/GOLDEN/ALTREF: both mode buckets (:286-292).
            for (ref_idx, ref_row) in seg_lvl.iter_mut().enumerate().skip(1) {
                for (mode, bucket) in ref_row.iter_mut().enumerate() {
                    let inter_lvl = lvl_seg
                        + i32::from(ref_deltas[ref_idx]) * scale
                        + i32::from(mode_deltas[mode]) * scale;
                    *bucket = inter_lvl.clamp(0, MAX_LOOP_FILTER) as u8;
                }
            }
        }

        Self { thr, lvl }
    }
}

/// `mi->skip && is_inter_block(mi)` (libvpx `vp9/common/vp9_loopfilter.c:1111`).
/// `is_inter_block(mi)` is `ref_frame[0] > INTRA_FRAME`, which is exactly
/// what [`MiInfo::is_inter`] already carries (see that field's own doc
/// comment), so this is a direct field read, not a re-derivation.
#[inline]
fn is_inter_skip(mi: &MiInfo) -> bool {
    mi.skip && mi.is_inter
}

/// Anything that can supply this frame's per-MI decode info by grid
/// position — implemented for [`FrameMi`] (the real decode driver) and, in
/// this module's own tests, for a hand-built grid.
///
/// [`FrameMi`]'s constructor and mutator are private to
/// [`super::recon`] (only the tile/partition decode walk ever fills one
/// in), so this indirection is what lets [`filter_block_plane`]'s masking
/// and filter-level logic be unit-tested against synthetic [`MiInfo`] grids
/// without reaching into that decode walk at all. [`loop_filter_frame`], the
/// public entry point, keeps taking a concrete `&FrameMi` — only the
/// private per-superblock function needs the indirection.
trait MiGrid {
    /// Grid height in MI units.
    fn rows(&self) -> usize;
    /// Grid width in MI units.
    fn cols(&self) -> usize;
    /// Mode info at `(row, col)`.
    fn mi_at(&self, row: usize, col: usize) -> MiInfo;
}

impl MiGrid for FrameMi {
    fn rows(&self) -> usize {
        self.rows
    }

    fn cols(&self) -> usize {
        self.cols
    }

    fn mi_at(&self, row: usize, col: usize) -> MiInfo {
        // Path syntax, not `self.get(...)`: `FrameMi` has its own inherent
        // `get` of the same name, and a method-call would recurse into this
        // trait impl instead of reaching it.
        FrameMi::get(self, row, col)
    }
}

#[inline]
fn schar_clamp(t: i32) -> i32 {
    t.clamp(-128, 127)
}

#[inline]
fn round2(v: u32, n: u32) -> u8 {
    ((v + (1 << (n - 1))) >> n) as u8
}

#[inline]
fn ad(a: u8, b: u8) -> i32 {
    (i32::from(a) - i32::from(b)).abs()
}

/// `filter_mask` (vpx_dsp/loopfilter.c): true = apply filter.
#[inline]
#[allow(clippy::too_many_arguments)]
fn filter_mask(
    limit: u8,
    blimit: u8,
    p3: u8,
    p2: u8,
    p1: u8,
    p0: u8,
    q0: u8,
    q1: u8,
    q2: u8,
    q3: u8,
) -> bool {
    let l = i32::from(limit);
    let mut fail = ad(p3, p2) > l;
    fail |= ad(p2, p1) > l;
    fail |= ad(p1, p0) > l;
    fail |= ad(q1, q0) > l;
    fail |= ad(q2, q1) > l;
    fail |= ad(q3, q2) > l;
    fail |= ad(p0, q0) * 2 + ad(p1, q1) / 2 > i32::from(blimit);
    !fail
}

/// `flat_mask4` with thresh 1.
#[inline]
#[allow(clippy::too_many_arguments)]
fn flat_mask4(p3: u8, p2: u8, p1: u8, p0: u8, q0: u8, q1: u8, q2: u8, q3: u8) -> bool {
    let mut fail = ad(p1, p0) > 1;
    fail |= ad(q1, q0) > 1;
    fail |= ad(p2, p0) > 1;
    fail |= ad(q2, q0) > 1;
    fail |= ad(p3, p0) > 1;
    fail |= ad(q3, q0) > 1;
    !fail
}

/// `hev_mask`.
#[inline]
fn hev_mask(thresh: u8, p1: u8, p0: u8, q0: u8, q1: u8) -> bool {
    let t = i32::from(thresh);
    i32::from(p1).abs_diff(i32::from(p0)) as i32 > t
        || i32::from(q1).abs_diff(i32::from(q0)) as i32 > t
}

/// `filter4` on four pixels addressed via (buf, idx, step).
#[inline]
fn filter4(buf: &mut [u8], pos: usize, step: usize, mask: bool, thresh: u8) {
    if !mask {
        return;
    }
    let ip1 = pos - 2 * step;
    let ip0 = pos - step;
    let iq0 = pos;
    let iq1 = pos + step;

    let hev = hev_mask(thresh, buf[ip1], buf[ip0], buf[iq0], buf[iq1]);
    // The reference XORs with 0x80 to reinterpret pixels as signed chars;
    // subtracting 128 is the same conversion.
    let ps1 = i32::from(buf[ip1]) - 128;
    let ps0 = i32::from(buf[ip0]) - 128;
    let qs0 = i32::from(buf[iq0]) - 128;
    let qs1 = i32::from(buf[iq1]) - 128;

    // add outer taps if we have high edge variance
    let mut filter = if hev { schar_clamp(ps1 - qs1) } else { 0 };
    // inner taps
    filter = schar_clamp(filter + 3 * (qs0 - ps0));

    let filter1 = schar_clamp(filter + 4) >> 3;
    let filter2 = schar_clamp(filter + 3) >> 3;

    buf[iq0] = (schar_clamp(qs0 - filter1) + 128) as u8;
    buf[ip0] = (schar_clamp(ps0 + filter2) + 128) as u8;

    // outer tap adjustments
    let filter = if hev { 0 } else { (filter1 + 1) >> 1 };
    buf[iq1] = (schar_clamp(qs1 - filter) + 128) as u8;
    buf[ip1] = (schar_clamp(ps1 + filter) + 128) as u8;
}

/// `filter8`.
#[inline]
fn filter8(buf: &mut [u8], pos: usize, step: usize, mask: bool, thresh: u8, flat: bool) {
    if flat && mask {
        let g = |i: i32| u32::from(buf[(pos as i64 + i64::from(i) * step as i64) as usize]);
        let (p3, p2, p1, p0) = (g(-4), g(-3), g(-2), g(-1));
        let (q0, q1, q2, q3) = (g(0), g(1), g(2), g(3));
        let mut s = |i: i32, v: u8| {
            buf[(pos as i64 + i64::from(i) * step as i64) as usize] = v;
        };
        s(-3, round2(p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0, 3));
        s(-2, round2(p3 + p3 + p2 + 2 * p1 + p0 + q0 + q1, 3));
        s(-1, round2(p3 + p2 + p1 + 2 * p0 + q0 + q1 + q2, 3));
        s(0, round2(p2 + p1 + p0 + 2 * q0 + q1 + q2 + q3, 3));
        s(1, round2(p1 + p0 + q0 + 2 * q1 + q2 + q3 + q3, 3));
        s(2, round2(p0 + q0 + q1 + 2 * q2 + q3 + q3 + q3, 3));
    } else {
        filter4(buf, pos, step, mask, thresh);
    }
}

/// `filter16` (15-tap wide filter).
#[inline]
fn filter16(
    buf: &mut [u8],
    pos: usize,
    step: usize,
    mask: bool,
    thresh: u8,
    flat: bool,
    flat2: bool,
) {
    if flat2 && flat && mask {
        let g = |i: i32| u32::from(buf[(pos as i64 + i64::from(i) * step as i64) as usize]);
        let (p7, p6, p5, p4) = (g(-8), g(-7), g(-6), g(-5));
        let (p3, p2, p1, p0) = (g(-4), g(-3), g(-2), g(-1));
        let (q0, q1, q2, q3) = (g(0), g(1), g(2), g(3));
        let (q4, q5, q6, q7) = (g(4), g(5), g(6), g(7));
        let mut out = [0u8; 14];
        out[0] = round2(p7 * 7 + p6 * 2 + p5 + p4 + p3 + p2 + p1 + p0 + q0, 4);
        out[1] = round2(p7 * 6 + p6 + p5 * 2 + p4 + p3 + p2 + p1 + p0 + q0 + q1, 4);
        out[2] = round2(
            p7 * 5 + p6 + p5 + p4 * 2 + p3 + p2 + p1 + p0 + q0 + q1 + q2,
            4,
        );
        out[3] = round2(
            p7 * 4 + p6 + p5 + p4 + p3 * 2 + p2 + p1 + p0 + q0 + q1 + q2 + q3,
            4,
        );
        out[4] = round2(
            p7 * 3 + p6 + p5 + p4 + p3 + p2 * 2 + p1 + p0 + q0 + q1 + q2 + q3 + q4,
            4,
        );
        out[5] = round2(
            p7 * 2 + p6 + p5 + p4 + p3 + p2 + p1 * 2 + p0 + q0 + q1 + q2 + q3 + q4 + q5,
            4,
        );
        out[6] = round2(
            p7 + p6 + p5 + p4 + p3 + p2 + p1 + p0 * 2 + q0 + q1 + q2 + q3 + q4 + q5 + q6,
            4,
        );
        out[7] = round2(
            p6 + p5 + p4 + p3 + p2 + p1 + p0 + q0 * 2 + q1 + q2 + q3 + q4 + q5 + q6 + q7,
            4,
        );
        out[8] = round2(
            p5 + p4 + p3 + p2 + p1 + p0 + q0 + q1 * 2 + q2 + q3 + q4 + q5 + q6 + q7 * 2,
            4,
        );
        out[9] = round2(
            p4 + p3 + p2 + p1 + p0 + q0 + q1 + q2 * 2 + q3 + q4 + q5 + q6 + q7 * 3,
            4,
        );
        out[10] = round2(
            p3 + p2 + p1 + p0 + q0 + q1 + q2 + q3 * 2 + q4 + q5 + q6 + q7 * 4,
            4,
        );
        out[11] = round2(
            p2 + p1 + p0 + q0 + q1 + q2 + q3 + q4 * 2 + q5 + q6 + q7 * 5,
            4,
        );
        out[12] = round2(p1 + p0 + q0 + q1 + q2 + q3 + q4 + q5 * 2 + q6 + q7 * 6, 4);
        out[13] = round2(p0 + q0 + q1 + q2 + q3 + q4 + q5 + q6 * 2 + q7 * 7, 4);
        let s = |b: &mut [u8], i: i32, v: u8| {
            b[(pos as i64 + i64::from(i) * step as i64) as usize] = v;
        };
        for (k, i) in (-7..=6).enumerate() {
            s(buf, i, out[k]);
        }
    } else {
        filter8(buf, pos, step, mask, thresh, flat);
    }
}

/// One 8-pixel-wide horizontal edge with the given kernel size.
fn lpf_horizontal(buf: &mut [u8], pos: usize, pitch: usize, t: &LfThresh, size: u8, count: usize) {
    for i in 0..8 * count {
        let s = pos + i;
        let (p3, p2, p1, p0) = (
            buf[s - 4 * pitch],
            buf[s - 3 * pitch],
            buf[s - 2 * pitch],
            buf[s - pitch],
        );
        let (q0, q1, q2, q3) = (
            buf[s],
            buf[s + pitch],
            buf[s + 2 * pitch],
            buf[s + 3 * pitch],
        );
        let mask = filter_mask(t.lim, t.mblim, p3, p2, p1, p0, q0, q1, q2, q3);
        match size {
            4 => filter4(buf, s, pitch, mask, t.hev_thr),
            8 => {
                let flat = flat_mask4(p3, p2, p1, p0, q0, q1, q2, q3);
                filter8(buf, s, pitch, mask, t.hev_thr, flat);
            }
            _ => {
                let flat = flat_mask4(p3, p2, p1, p0, q0, q1, q2, q3);
                let flat2 = flat_mask5(
                    buf[s - 8 * pitch],
                    buf[s - 7 * pitch],
                    buf[s - 6 * pitch],
                    buf[s - 5 * pitch],
                    p0,
                    q0,
                    buf[s + 4 * pitch],
                    buf[s + 5 * pitch],
                    buf[s + 6 * pitch],
                    buf[s + 7 * pitch],
                );
                filter16(buf, s, pitch, mask, t.hev_thr, flat, flat2);
            }
        }
    }
}

/// One 8-pixel-tall vertical edge (`count` rows for the 16-wide kernel).
fn lpf_vertical(buf: &mut [u8], pos: usize, pitch: usize, t: &LfThresh, size: u8, count: usize) {
    let rows = if size == 16 { count } else { 8 };
    for i in 0..rows {
        let s = pos + i * pitch;
        let (p3, p2, p1, p0) = (buf[s - 4], buf[s - 3], buf[s - 2], buf[s - 1]);
        let (q0, q1, q2, q3) = (buf[s], buf[s + 1], buf[s + 2], buf[s + 3]);
        let mask = filter_mask(t.lim, t.mblim, p3, p2, p1, p0, q0, q1, q2, q3);
        match size {
            4 => filter4(buf, s, 1, mask, t.hev_thr),
            8 => {
                let flat = flat_mask4(p3, p2, p1, p0, q0, q1, q2, q3);
                filter8(buf, s, 1, mask, t.hev_thr, flat);
            }
            _ => {
                let flat = flat_mask4(p3, p2, p1, p0, q0, q1, q2, q3);
                let flat2 = flat_mask5(
                    buf[s - 8],
                    buf[s - 7],
                    buf[s - 6],
                    buf[s - 5],
                    p0,
                    q0,
                    buf[s + 4],
                    buf[s + 5],
                    buf[s + 6],
                    buf[s + 7],
                );
                filter16(buf, s, 1, mask, t.hev_thr, flat, flat2);
            }
        }
    }
}

/// `flat_mask5` with thresh 1 (as called by `mb_lpf_*_edge_w`: the outer
/// ring p7..p4 / q4..q7 plays the role of p4..p1 / q1..q4).
#[inline]
#[allow(clippy::too_many_arguments)]
fn flat_mask5(
    p4: u8,
    p3: u8,
    p2: u8,
    p1: u8,
    p0: u8,
    q0: u8,
    q1: u8,
    q2: u8,
    q3: u8,
    q4: u8,
) -> bool {
    let flat4 = flat_mask4(p3, p2, p1, p0, q0, q1, q2, q3);
    flat4 && ad(p4, p0) <= 1 && ad(q4, q0) <= 1
}

/// `filter_selectively_vert` (non-dual path used by the non420 filter).
fn filter_selectively_vert(
    buf: &mut [u8],
    row_pos: usize,
    pitch: usize,
    mut mask_16x16: u32,
    mut mask_8x8: u32,
    mut mask_4x4: u32,
    mut mask_4x4_int: u32,
    lfi: &LoopFilterInfo,
    lfl: &[u8],
) {
    let mut mask = mask_16x16 | mask_8x8 | mask_4x4 | mask_4x4_int;
    let mut s = row_pos;
    let mut li = 0usize;
    while mask != 0 {
        let t = &lfi.thr[lfl[li] as usize];
        if mask & 1 != 0 {
            if mask_16x16 & 1 != 0 {
                lpf_vertical(buf, s, pitch, t, 16, 8);
            } else if mask_8x8 & 1 != 0 {
                lpf_vertical(buf, s, pitch, t, 8, 8);
            } else if mask_4x4 & 1 != 0 {
                lpf_vertical(buf, s, pitch, t, 4, 8);
            }
        }
        if mask_4x4_int & 1 != 0 {
            lpf_vertical(buf, s + 4, pitch, t, 4, 8);
        }
        s += 8;
        li += 1;
        mask >>= 1;
        mask_16x16 >>= 1;
        mask_8x8 >>= 1;
        mask_4x4 >>= 1;
        mask_4x4_int >>= 1;
    }
}

/// `filter_selectively_horiz`, including the shared-threshold 16-wide dual.
fn filter_selectively_horiz(
    buf: &mut [u8],
    row_pos: usize,
    pitch: usize,
    mut mask_16x16: u32,
    mut mask_8x8: u32,
    mut mask_4x4: u32,
    mut mask_4x4_int: u32,
    lfi_info: &LoopFilterInfo,
    lfl: &[u8],
) {
    let mut mask = mask_16x16 | mask_8x8 | mask_4x4 | mask_4x4_int;
    let mut s = row_pos;
    let mut li = 0usize;
    while mask != 0 {
        let mut count = 1usize;
        if mask & 1 != 0 {
            let lfi = &lfi_info.thr[lfl[li] as usize];
            if mask_16x16 & 1 != 0 {
                if mask_16x16 & 3 == 3 {
                    // 16-wide with the FIRST block's thresholds (dual).
                    lpf_horizontal(buf, s, pitch, lfi, 16, 2);
                    count = 2;
                } else {
                    lpf_horizontal(buf, s, pitch, lfi, 16, 1);
                }
            } else if mask_8x8 & 1 != 0 {
                if mask_8x8 & 3 == 3 {
                    let lfin = &lfi_info.thr[lfl[li + 1] as usize];
                    lpf_horizontal(buf, s, pitch, lfi, 8, 1);
                    lpf_horizontal(buf, s + 8, pitch, lfin, 8, 1);
                    if mask_4x4_int & 3 == 3 {
                        lpf_horizontal(buf, s + 4 * pitch, pitch, lfi, 4, 1);
                        lpf_horizontal(buf, s + 8 + 4 * pitch, pitch, lfin, 4, 1);
                    } else {
                        if mask_4x4_int & 1 != 0 {
                            lpf_horizontal(buf, s + 4 * pitch, pitch, lfi, 4, 1);
                        } else if mask_4x4_int & 2 != 0 {
                            lpf_horizontal(buf, s + 8 + 4 * pitch, pitch, lfin, 4, 1);
                        }
                    }
                    count = 2;
                } else {
                    lpf_horizontal(buf, s, pitch, lfi, 8, 1);
                    if mask_4x4_int & 1 != 0 {
                        lpf_horizontal(buf, s + 4 * pitch, pitch, lfi, 4, 1);
                    }
                }
            } else if mask_4x4 & 1 != 0 {
                if mask_4x4 & 3 == 3 {
                    let lfin = &lfi_info.thr[lfl[li + 1] as usize];
                    lpf_horizontal(buf, s, pitch, lfi, 4, 1);
                    lpf_horizontal(buf, s + 8, pitch, lfin, 4, 1);
                    if mask_4x4_int & 3 == 3 {
                        lpf_horizontal(buf, s + 4 * pitch, pitch, lfi, 4, 1);
                        lpf_horizontal(buf, s + 8 + 4 * pitch, pitch, lfin, 4, 1);
                    } else {
                        if mask_4x4_int & 1 != 0 {
                            lpf_horizontal(buf, s + 4 * pitch, pitch, lfi, 4, 1);
                        } else if mask_4x4_int & 2 != 0 {
                            lpf_horizontal(buf, s + 8 + 4 * pitch, pitch, lfin, 4, 1);
                        }
                    }
                    count = 2;
                } else {
                    lpf_horizontal(buf, s, pitch, lfi, 4, 1);
                    if mask_4x4_int & 1 != 0 {
                        lpf_horizontal(buf, s + 4 * pitch, pitch, lfi, 4, 1);
                    }
                }
            } else {
                lpf_horizontal(buf, s + 4 * pitch, pitch, lfi, 4, 1);
            }
        }
        s += 8 * count;
        li += count;
        mask >>= count;
        mask_16x16 >>= count;
        mask_8x8 >>= count;
        mask_4x4 >>= count;
        mask_4x4_int >>= count;
    }
}

/// `vp9_filter_block_plane_non420` for one 64x64 superblock of one plane.
fn filter_block_plane<M: MiGrid>(
    plane: &mut PlaneBuf,
    ss_x: usize,
    ss_y: usize,
    mi: &M,
    mi_row: usize,
    mi_col: usize,
    lfi: &LoopFilterInfo,
) {
    let row_step = 1 << ss_y;
    let col_step = 1 << ss_x;
    let stride = plane.stride;
    // dst offset of this SB in the plane
    let sb_off = ((mi_row * 8) >> ss_y) * stride + ((mi_col * 8) >> ss_x);

    let mut mask_16x16 = [0u32; 8];
    let mut mask_8x8 = [0u32; 8];
    let mut mask_4x4 = [0u32; 8];
    let mut mask_4x4_int = [0u32; 8];
    let mut lfl = [0u8; 64];

    // Vertical pass with mask construction.
    let mut r = 0usize;
    while r < 8 && mi_row + r < mi.rows() {
        let mut mask_16x16_c: u32 = 0;
        let mut mask_8x8_c: u32 = 0;
        let mut mask_4x4_c: u32 = 0;

        let mut c = 0usize;
        while c < 8 && mi_col + c < mi.cols() {
            let info = mi.mi_at(mi_row + r, mi_col + c);
            let sb_type = info.sb_type as usize;
            // `mi->skip && is_inter_block(mi)` (vp9_loopfilter.c:1111). For a
            // keyframe/intra-only frame every block has `is_inter == false`,
            // so this reduces to the old `false` constant exactly -- the
            // regression proof for the four keyframe goldens below.
            let skip_this = is_inter_skip(&info);
            let block_edge_left = if tables::NUM_4X4_BLOCKS_WIDE[sb_type] > 1 {
                (c & (tables::NUM_8X8_BLOCKS_WIDE[sb_type] as usize - 1)) == 0
            } else {
                true
            };
            let skip_this_c = skip_this && !block_edge_left;
            let block_edge_above = if tables::NUM_4X4_BLOCKS_HIGH[sb_type] > 1 {
                (r & (tables::NUM_8X8_BLOCKS_HIGH[sb_type] as usize - 1)) == 0
            } else {
                true
            };
            let skip_this_r = skip_this && !block_edge_above;
            let tx_size = usize::from(
                tables::UV_TXSIZE_LOOKUP[sb_type][info.tx_size as usize][usize::from(ss_x != 0)]
                    [usize::from(ss_y != 0)],
            );
            let tx_size = if ss_x == 0 && ss_y == 0 {
                info.tx_size as usize
            } else {
                tx_size
            };
            let skip_border_4x4_c = ss_x != 0 && mi_col + c == mi.cols() - 1;
            let skip_border_4x4_r = ss_y != 0 && mi_row + r == mi.rows() - 1;

            // `get_filter_level` (vp9_loopfilter.c:233-236):
            // `lvl[seg][ref_frame[0]][mode_lf_lut[mode]]`.
            let mode_bucket = tables_inter::MODE_LF_LUT[info.mode as usize] as usize;
            let ref_idx = info.ref_frame[0] as usize;
            let level = lfi.lvl[info.segment_id as usize][ref_idx][mode_bucket];
            lfl[(r << 3) + (c >> ss_x)] = level;
            if level == 0 {
                c += col_step;
                continue;
            }

            let cb = c >> ss_x;
            if tx_size == 3 {
                // TX_32X32
                if !skip_this_c && (cb & 3) == 0 {
                    if !skip_border_4x4_c {
                        mask_16x16_c |= 1 << cb;
                    } else {
                        mask_8x8_c |= 1 << cb;
                    }
                }
                if !skip_this_r && ((r >> ss_y) & 3) == 0 {
                    if !skip_border_4x4_r {
                        mask_16x16[r] |= 1 << cb;
                    } else {
                        mask_8x8[r] |= 1 << cb;
                    }
                }
            } else if tx_size == 2 {
                // TX_16X16
                if !skip_this_c && (cb & 1) == 0 {
                    if !skip_border_4x4_c {
                        mask_16x16_c |= 1 << cb;
                    } else {
                        mask_8x8_c |= 1 << cb;
                    }
                }
                if !skip_this_r && ((r >> ss_y) & 1) == 0 {
                    if !skip_border_4x4_r {
                        mask_16x16[r] |= 1 << cb;
                    } else {
                        mask_8x8[r] |= 1 << cb;
                    }
                }
            } else {
                // force 8x8 filtering on 32x32 boundaries
                if !skip_this_c {
                    if tx_size == 1 || (cb & 3) == 0 {
                        mask_8x8_c |= 1 << cb;
                    } else {
                        mask_4x4_c |= 1 << cb;
                    }
                }
                if !skip_this_r {
                    if tx_size == 1 || ((r >> ss_y) & 3) == 0 {
                        mask_8x8[r] |= 1 << cb;
                    } else {
                        mask_4x4[r] |= 1 << cb;
                    }
                }
                if !skip_this && tx_size < 1 && !skip_border_4x4_c {
                    mask_4x4_int[r] |= 1 << cb;
                }
            }
            c += col_step;
        }

        // Disable filtering on the leftmost column.
        let border_mask: u32 = if mi_col == 0 { !1u32 } else { !0u32 };
        let row_pos = sb_off + (r >> ss_y) * 8 * stride;
        filter_selectively_vert(
            &mut plane.data,
            row_pos,
            stride,
            mask_16x16_c & border_mask,
            mask_8x8_c & border_mask,
            mask_4x4_c & border_mask,
            mask_4x4_int[r],
            lfi,
            &lfl[r << 3..],
        );
        r += row_step;
    }

    // Horizontal pass.
    let mut r = 0usize;
    while r < 8 && mi_row + r < mi.rows() {
        let skip_border_4x4_r = ss_y != 0 && mi_row + r == mi.rows() - 1;
        let mask_4x4_int_r = if skip_border_4x4_r {
            0
        } else {
            mask_4x4_int[r]
        };
        let (m16, m8, m4);
        if mi_row + r == 0 {
            m16 = 0;
            m8 = 0;
            m4 = 0;
        } else {
            m16 = mask_16x16[r];
            m8 = mask_8x8[r];
            m4 = mask_4x4[r];
        }
        let row_pos = sb_off + (r >> ss_y) * 8 * stride;
        filter_selectively_horiz(
            &mut plane.data,
            row_pos,
            stride,
            m16,
            m8,
            m4,
            mask_4x4_int_r,
            lfi,
            &lfl[r << 3..],
        );
        r += row_step;
    }
}

/// Applies the loop filter to the whole frame (`vp9_loop_filter_frame`,
/// `loop_filter_rows` order: raster superblocks, per SB vertical then
/// horizontal, per plane).
pub fn loop_filter_frame(
    planes: &mut [PlaneBuf; 3],
    subsampling: (usize, usize),
    mi: &FrameMi,
    lfi: &LoopFilterInfo,
) {
    let (ss_x, ss_y) = subsampling;
    let mut mi_row = 0;
    while mi_row < mi.rows {
        let mut mi_col = 0;
        while mi_col < mi.cols {
            for (p, plane) in planes.iter_mut().enumerate() {
                let (px, py) = if p == 0 { (0, 0) } else { (ss_x, ss_y) };
                filter_block_plane(plane, px, py, mi, mi_row, mi_col, lfi);
            }
            mi_col += 8;
        }
        mi_row += 8;
    }
}

#[cfg(test)]
mod tests {
    use super::super::refs::{INTRA_FRAME, LAST_FRAME, NONE_FRAME};
    use super::*;

    // `BLOCK_SIZE` enum order (`tables`/`tables_inter`): 4X4=0, 4X8=1,
    // 8X4=2, 8X8=3, 8X16=4, 16X8=5, 16X16=6, ..., 64X64=12.
    const BLOCK_16X16: u8 = 6;
    // `TX_SIZE` enum order: TX_4X4=0, TX_8X8=1, TX_16X16=2, TX_32X32=3.
    const TX_8X8: u8 = 1;
    // Raw `MB_MODE_COUNT` mode number (matches `MiInfo::mode` and
    // `tables_inter::MODE_LF_LUT`'s indexing -- see that table's doc
    // comment): `NEARESTMV == 10`, loop-filter mode bucket 1.
    const NEARESTMV: u8 = 10;

    /// A hand-built MI grid for driving [`filter_block_plane`] directly.
    /// `FrameMi`'s own constructor and mutator are private to
    /// [`super::super::recon`] (only the real tile/partition decode walk
    /// fills one in), so this is the [`MiGrid`] implementor the loop
    /// filter's own unit tests use instead.
    struct SyntheticMi {
        rows: usize,
        cols: usize,
        data: Vec<MiInfo>,
    }

    impl SyntheticMi {
        fn filled(rows: usize, cols: usize, mi: MiInfo) -> Self {
            Self {
                rows,
                cols,
                data: vec![mi; rows * cols],
            }
        }

        fn set(&mut self, row: usize, col: usize, mi: MiInfo) {
            self.data[row * self.cols + col] = mi;
        }
    }

    impl MiGrid for SyntheticMi {
        fn rows(&self) -> usize {
            self.rows
        }

        fn cols(&self) -> usize {
            self.cols
        }

        fn mi_at(&self, row: usize, col: usize) -> MiInfo {
            self.data[row * self.cols + col]
        }
    }

    /// A square plane whose pixel value depends only on its column.
    fn flat_plane(size: usize, pattern: impl Fn(usize) -> u8) -> PlaneBuf {
        let mut data = vec![0u8; size * size];
        for y in 0..size {
            for x in 0..size {
                data[y * size + x] = pattern(x);
            }
        }
        PlaneBuf {
            data,
            stride: size,
            width: size,
            height: size,
        }
    }

    // -- LoopFilterInfo::new_with_deltas: level-table matrix ---------------

    #[test]
    fn delta_disabled_is_flat_lvl_seg_everywhere() {
        // `!delta_enabled`: every `[ref][mode]` entry in every segment must
        // equal that segment's `lvl_seg`, regardless of ref_deltas/
        // mode_deltas (vp9_loopfilter.c:277-280 `memset` -- deliberately
        // nonzero here to prove they really are ignored).
        let seg_lf = [(false, 0i16); 8];
        let lfi = LoopFilterInfo::new_with_deltas(
            40,
            0,
            false,
            [9, -9, 9, -9],
            [9, -9],
            false,
            false,
            &seg_lf,
        );
        for seg in 0..8 {
            for r in 0..4 {
                for m in 0..2 {
                    assert_eq!(lfi.lvl[seg][r][m], 40, "seg {seg} ref {r} mode {m}");
                }
            }
        }
    }

    #[test]
    fn delta_enabled_intra_bucket_matches_old_scalar_formula() {
        // Parity check, both `delta_enabled` states: `lvl[seg][INTRA][0]`
        // must equal the pre-generalization scalar formula --
        // `delta_enabled ? clamp(lvl_seg + ref_deltas[INTRA] * scale) :
        // lvl_seg` -- the one property the four all-intra keyframe goldens
        // cannot isolate on their own, since they only ever exercise this
        // single table entry.
        for delta_enabled in [false, true] {
            for default_filt_lvl in [0u8, 10, 31, 32, 50, 63] {
                for intra_delta in [-60i8, -5, 0, 5, 60] {
                    let scale = 1i32 << (default_filt_lvl >> 5);
                    let expected = if delta_enabled {
                        (i32::from(default_filt_lvl) + i32::from(intra_delta) * scale).clamp(0, 63)
                            as u8
                    } else {
                        default_filt_lvl
                    };
                    let seg_lf = [(false, 0i16); 8];
                    let lfi = LoopFilterInfo::new_with_deltas(
                        default_filt_lvl,
                        0,
                        delta_enabled,
                        [intra_delta, 0, 0, 0],
                        [0, 0],
                        false,
                        false,
                        &seg_lf,
                    );
                    for seg in 0..8 {
                        assert_eq!(
                            lfi.lvl[seg][0][0], expected,
                            "delta_enabled={delta_enabled} lvl={default_filt_lvl} \
                             delta={intra_delta}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn delta_enabled_inter_buckets_match_ref_and_mode_deltas() {
        // Each of LAST/GOLDEN/ALTREF x both mode buckets, at both scale
        // regimes (`default_filt_lvl < 32` -> scale 1, `>= 32` -> scale 2).
        let ref_deltas = [3i8, -10, 20, -30]; // [INTRA, LAST, GOLDEN, ALTREF]
        let mode_deltas = [7i8, -7];
        let seg_lf = [(false, 0i16); 8];
        for default_filt_lvl in [10u8, 40] {
            let scale = 1i32 << (default_filt_lvl >> 5);
            let lfi = LoopFilterInfo::new_with_deltas(
                default_filt_lvl,
                0,
                true,
                ref_deltas,
                mode_deltas,
                false,
                false,
                &seg_lf,
            );
            for (ref_idx, &delta) in ref_deltas.iter().enumerate().skip(1) {
                for (mode, &mdelta) in mode_deltas.iter().enumerate() {
                    let expected = (i32::from(default_filt_lvl)
                        + i32::from(delta) * scale
                        + i32::from(mdelta) * scale)
                        .clamp(0, 63) as u8;
                    for seg in 0..8 {
                        assert_eq!(
                            lfi.lvl[seg][ref_idx][mode], expected,
                            "lvl={default_filt_lvl} ref={ref_idx} mode={mode}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn clamp_boundaries_saturate_both_directions() {
        let seg_lf = [(false, 0i16); 8];
        // Lower bound: very negative deltas push every entry to 0.
        let lo = LoopFilterInfo::new_with_deltas(
            0,
            0,
            true,
            [-64, -64, -64, -64],
            [-64, -64],
            false,
            false,
            &seg_lf,
        );
        assert_eq!(lo.lvl[0][INTRA_FRAME as usize][0], 0);
        for r in 1..4 {
            for m in 0..2 {
                assert_eq!(lo.lvl[0][r][m], 0, "ref {r} mode {m}");
            }
        }
        // Upper bound: `default_filt_lvl = 63` selects `scale = 2`
        // (`63 >> 5 == 1`), and large positive deltas push every entry to 63.
        let hi = LoopFilterInfo::new_with_deltas(
            63,
            0,
            true,
            [63, 63, 63, 63],
            [63, 63],
            false,
            false,
            &seg_lf,
        );
        assert_eq!(hi.lvl[0][INTRA_FRAME as usize][0], 63);
        for r in 1..4 {
            for m in 0..2 {
                assert_eq!(hi.lvl[0][r][m], 63, "ref {r} mode {m}");
            }
        }
    }

    #[test]
    fn segmentation_abs_and_delta_modes_are_per_segment() {
        // seg 0: SEG_LVL_ALT_LF active, abs mode -> lvl_seg = data directly.
        // seg 1: active, delta mode -> lvl_seg = default_filt_lvl + data.
        // seg 2: inactive -> lvl_seg = default_filt_lvl, unaffected by the
        // *other* segments' feature data.
        let mut seg_lf = [(false, 0i16); 8];
        seg_lf[0] = (true, 12);
        seg_lf[1] = (true, 12);
        let default_filt_lvl = 20u8;
        // delta_enabled=false so `lvl[seg][*][*]` is `lvl_seg` verbatim,
        // isolating the segmentation math from the ref/mode-delta math the
        // other tests above already cover.
        let abs = LoopFilterInfo::new_with_deltas(
            default_filt_lvl,
            0,
            false,
            [0; 4],
            [0, 0],
            true,
            true,
            &seg_lf,
        );
        assert_eq!(abs.lvl[0][0][0], 12, "seg 0 abs: lvl_seg = data");
        assert_eq!(
            abs.lvl[1][0][0], 12,
            "seg 1 abs: lvl_seg = data (same field, abs mode)"
        );
        assert_eq!(
            abs.lvl[2][0][0], default_filt_lvl,
            "seg 2 inactive: unaffected"
        );

        let delta = LoopFilterInfo::new_with_deltas(
            default_filt_lvl,
            0,
            false,
            [0; 4],
            [0, 0],
            true,
            false,
            &seg_lf,
        );
        assert_eq!(
            delta.lvl[0][0][0],
            default_filt_lvl + 12,
            "seg 0 delta: lvl_seg = base + data"
        );
        assert_eq!(delta.lvl[1][0][0], default_filt_lvl + 12, "seg 1 delta");
        assert_eq!(
            delta.lvl[2][0][0], default_filt_lvl,
            "seg 2 inactive: unaffected"
        );
    }

    #[test]
    fn seg_disabled_ignores_feature_data_even_if_marked_active() {
        // `seg_enabled = false` must short-circuit the per-segment feature
        // check entirely, matching `segfeature_active` gating on the frame's
        // own `seg->enabled` before ever consulting per-segment data.
        let mut seg_lf = [(false, 0i16); 8];
        seg_lf[3] = (true, 30); // would matter if seg_enabled were true
        let lfi =
            LoopFilterInfo::new_with_deltas(20, 0, false, [0; 4], [0, 0], false, false, &seg_lf);
        assert_eq!(lfi.lvl[3][0][0], 20);
    }

    #[test]
    fn new_delegates_to_new_with_deltas_with_zeroed_inter_deltas() {
        // The intra-only convenience constructor must produce exactly the
        // table `new_with_deltas` would with the matching zeroed
        // LAST/GOLDEN/ALTREF ref deltas and zeroed mode deltas -- pins the
        // delegation itself, over the *whole* table (not just the intra
        // slot), independent of what value is chosen.
        let seg_lf = [(false, 0i16); 8];
        let narrow = LoopFilterInfo::new(37, 3, true, -11, true, false, &seg_lf);
        let full = LoopFilterInfo::new_with_deltas(
            37,
            3,
            true,
            [-11, 0, 0, 0],
            [0, 0],
            true,
            false,
            &seg_lf,
        );
        for seg in 0..8 {
            for r in 0..4 {
                for m in 0..2 {
                    assert_eq!(
                        narrow.lvl[seg][r][m], full.lvl[seg][r][m],
                        "seg {seg} ref {r} mode {m}"
                    );
                }
            }
        }
    }

    // -- skip predicate (`mi->skip && is_inter_block(mi)`) ------------------

    #[test]
    fn skip_predicate_requires_both_skip_and_inter() {
        let base = MiInfo::default(); // skip=false, is_inter=false
        assert!(!is_inter_skip(&base));
        assert!(!is_inter_skip(&MiInfo {
            skip: true,
            is_inter: false,
            ..base
        }));
        assert!(!is_inter_skip(&MiInfo {
            skip: false,
            is_inter: true,
            ..base
        }));
        assert!(is_inter_skip(&MiInfo {
            skip: true,
            is_inter: true,
            ..base
        }));
    }

    // -- synthetic-grid proof: skip suppresses masking, not just the level -

    #[test]
    fn inter_skip_block_suppresses_its_own_interior_edge_but_not_a_non_skip_blocks() {
        // BLOCK_16X16 (sb_type 6, `NUM_8X8_BLOCKS_WIDE == 2`) with TX_8X8
        // (tx_size 1): the block spans 2 MI columns with an internal 8x8
        // transform boundary between them -- the "half-block" vertical edge
        // at local column 1, exactly the edge `skip_this_c` can suppress
        // (vp9_loopfilter.c:1111-1177, and this package's second change).
        let template = MiInfo {
            sb_type: BLOCK_16X16,
            skip: false,
            tx_size: TX_8X8,
            segment_id: 0,
            mode: NEARESTMV,
            is_inter: true,
            ref_frame: [LAST_FRAME, NONE_FRAME],
            ..MiInfo::default()
        };
        let mut grid = SyntheticMi::filled(8, 8, template);
        // The top-left 16x16 block (MI rows/cols 0..=1) is the skipped inter
        // block under test; every other 16x16 block tiling the rest of the
        // SB stays the non-skip control.
        for r in 0..2 {
            for c in 0..2 {
                grid.set(
                    r,
                    c,
                    MiInfo {
                        skip: true,
                        ..template
                    },
                );
            }
        }

        // Flat, nonzero filter level everywhere (`delta_enabled = false`),
        // decoupling this test from the level-table logic already covered
        // above -- every `[seg][ref][mode]` entry is `32`.
        let lfi = LoopFilterInfo::new(32, 0, false, 0, false, false, &[(false, 0i16); 8]);

        // A period-16 horizontal step (100 | 108 every 8 columns, constant
        // down every column), so: (a) every 8x8-granularity vertical edge is
        // a real, filter-eligible discontinuity -- at level 32 (lim=32,
        // mblim=100), `filter_mask` and `flat_mask4` both pass for an 8-unit
        // step, so `filter8`'s flat branch would fire if the edge is not
        // masked out; and (b) there is no *vertical* discontinuity at all
        // (every column is constant top-to-bottom), so the horizontal pass
        // -- which still runs over this same data -- is a mathematical
        // no-op and cannot contaminate the vertical-edge assertions below.
        let mut plane = flat_plane(64, |x| if x % 16 < 8 { 100 } else { 108 });

        filter_block_plane(&mut plane, 0, 0, &grid, 0, 0, &lfi);

        let row = 4usize;
        let px = |x: usize| plane.data[row * plane.stride + x];

        // Region A (skipped inter block): the interior edge at x=8 (between
        // MI col 0 and col 1 of the *same* coded block) must be untouched --
        // still exactly the initial step, pixel for pixel.
        assert_eq!(
            [px(5), px(6), px(7)],
            [100, 100, 100],
            "region A (skip): p2,p1,p0 must stay untouched"
        );
        assert_eq!(
            [px(8), px(9), px(10)],
            [108, 108, 108],
            "region A (skip): q0,q1,q2 must stay untouched"
        );

        // Region B (non-skip control block, MI cols 2..=3): the same-shaped
        // interior edge at x=24 must have been filtered -- p0/q0 move off
        // their initial values. Asserted as inequality (not the exact
        // flat8 output) so the test doesn't depend on reproducing that
        // rounding arithmetic correctly.
        assert_ne!(
            px(23),
            100,
            "region B (non-skip): p0 must have been filtered"
        );
        assert_ne!(
            px(24),
            108,
            "region B (non-skip): q0 must have been filtered"
        );
    }
}
