//! AV1 intra prediction (spec 7.11.2) and chroma-from-luma (spec 7.11.5).
//!
//! Exact port of the spec processes: edge (AboveRow/LeftCol) construction,
//! corner filter, intra edge filter/upsample selection and application,
//! basic (PAETH), recursive (filter intra), directional, DC, and smooth
//! predictors, plus the CFL luma-subsampling and alpha application.

use super::consts::{
    ANGLE_STEP, D67_PRED, DC_PRED, PAETH_PRED, SMOOTH_H_PRED, SMOOTH_PRED, SMOOTH_V_PRED, V_PRED,
};
use super::tables_conv::{
    DR_INTRA_DERIVATIVE, INTRA_EDGE_KERNEL, INTRA_FILTER_TAPS, MODE_TO_ANGLE, SM_WEIGHTS_TX_16X16,
    SM_WEIGHTS_TX_32X32, SM_WEIGHTS_TX_4X4, SM_WEIGHTS_TX_64X64, SM_WEIGHTS_TX_8X8,
};

/// `INTRA_FILTER_SCALE_BITS` (spec 03.symbols.md: "Scaling shift for intra
/// filtering process" = 4).
const INTRA_FILTER_SCALE_BITS: u32 = 4;

/// Edge buffer: logical indices -2..=MAX_EDGE-3 with a +2 offset.
const EDGE_BUF: usize = 2 + 2 * 129 + 2;

#[inline]
fn round2(x: i32, n: u32) -> i32 {
    (x + (1 << (n - 1))) >> n
}

#[inline]
fn round2_signed(x: i32, n: u32) -> i32 {
    if x >= 0 {
        round2(x, n)
    } else {
        -round2(-x, n)
    }
}

#[inline]
fn clip1(x: i32) -> i32 {
    x.clamp(0, 255)
}

/// Parameters for one intra prediction call.
pub struct PredParams {
    pub have_left: bool,
    pub have_above: bool,
    pub have_above_right: bool,
    pub have_below_left: bool,
    /// Y prediction mode (or DC for the CFL case; CFL applied separately).
    pub mode: usize,
    pub log2w: u32,
    pub log2h: u32,
    /// AngleDeltaY / AngleDeltaUV as applicable (already sign-adjusted).
    pub angle_delta: i32,
    /// `Some(filter_intra_mode)` when use_filter_intra (plane 0 only).
    pub filter_intra_mode: Option<usize>,
    /// Sequence-level `enable_intra_edge_filter`.
    pub enable_intra_edge_filter: bool,
    /// Output of the intra filter type process (neighbor uses smooth mode).
    pub filter_type: bool,
    /// Largest valid x/y coordinate in this plane (spec maxX/maxY).
    pub max_x: usize,
    pub max_y: usize,
}

/// `predict_intra` (spec 7.11.2.1) writing into `buf` (one plane, `stride`
/// wide) at (`x`, `y`).
#[allow(clippy::too_many_lines)]
pub fn predict_intra(buf: &mut [u8], stride: usize, x: usize, y: usize, p: &PredParams) {
    let w = 1usize << p.log2w;
    let h = 1usize << p.log2h;
    let px = |bx: usize, by: usize| -> i32 { i32::from(buf[by * stride + bx]) };

    // AboveRow / LeftCol construction (indices offset by +2).
    let mut above = [0i32; EDGE_BUF];
    let mut left = [0i32; EDGE_BUF];
    let bit_depth = 8u32;

    if !p.have_above && p.have_left {
        let v = px(x - 1, y);
        for i in 0..w + h {
            above[2 + i] = v;
        }
    } else if !p.have_above && !p.have_left {
        let v = (1 << (bit_depth - 1)) - 1;
        for i in 0..w + h {
            above[2 + i] = v;
        }
    } else {
        let above_limit =
            core::cmp::min(p.max_x, x + if p.have_above_right { 2 * w } else { w } - 1);
        // `min(above_limit, x + i)` is `x + i` while `i <= above_limit - x`
        // and `above_limit` after that, so the loop splits into a straight
        // widening copy of a contiguous run followed by an edge-replicating
        // fill.  This is the same sequence of values, without a clamp per
        // sample.
        let n_copy = core::cmp::min(w + h, above_limit + 1 - x);
        let src_off = (y - 1) * stride + x;
        for (i, &b) in buf[src_off..src_off + n_copy].iter().enumerate() {
            above[2 + i] = i32::from(b);
        }
        let edge_val = px(above_limit, y - 1);
        for v in above.iter_mut().take(2 + w + h).skip(2 + n_copy) {
            *v = edge_val;
        }
    }

    if !p.have_left && p.have_above {
        let v = px(x, y - 1);
        for i in 0..w + h {
            left[2 + i] = v;
        }
    } else if !p.have_left && !p.have_above {
        let v = (1 << (bit_depth - 1)) + 1;
        for i in 0..w + h {
            left[2 + i] = v;
        }
    } else {
        let left_limit = core::cmp::min(p.max_y, y + if p.have_below_left { 2 * h } else { h } - 1);
        // Same split as AboveRow; the reads stay column-strided so only the
        // clamp is removed, not the load pattern.
        let n_copy = core::cmp::min(w + h, left_limit + 1 - y);
        for i in 0..n_copy {
            left[2 + i] = px(x - 1, y + i);
        }
        let edge_val = px(x - 1, left_limit);
        for v in left.iter_mut().take(2 + w + h).skip(2 + n_copy) {
            *v = edge_val;
        }
    }

    // AboveRow[-1] / LeftCol[-1].
    let corner = if p.have_above && p.have_left {
        px(x - 1, y - 1)
    } else if p.have_above {
        px(x, y - 1)
    } else if p.have_left {
        px(x - 1, y)
    } else {
        1 << (bit_depth - 1)
    };
    above[1] = corner;
    left[1] = corner;

    // Prediction dispatch.
    let mut pred = vec![0i32; w * h];
    if let Some(fi_mode) = p.filter_intra_mode {
        recursive_intra(&mut pred, &above, &left, w, h, fi_mode);
    } else if is_directional_mode(p.mode) {
        directional_intra(&mut pred, &mut above, &mut left, w, h, x, y, p);
    } else if p.mode == SMOOTH_PRED || p.mode == SMOOTH_V_PRED || p.mode == SMOOTH_H_PRED {
        smooth_intra(&mut pred, &above, &left, w, h, p.mode, p.log2w, p.log2h);
    } else if p.mode == DC_PRED {
        dc_intra(
            &mut pred,
            &above,
            &left,
            w,
            h,
            p.log2w,
            p.log2h,
            p.have_left,
            p.have_above,
        );
    } else {
        // PAETH_PRED (basic intra prediction process).
        debug_assert_eq!(p.mode, PAETH_PRED);
        for i in 0..h {
            let (li, tl) = (left[2 + i], above[1]);
            let row = &mut pred[i * w..(i + 1) * w];
            for (j, r) in row.iter_mut().enumerate() {
                let base = above[2 + j] + li - tl;
                let p_left = (base - li).abs();
                let p_top = (base - above[2 + j]).abs();
                let p_top_left = (base - tl).abs();
                *r = if p_left <= p_top && p_left <= p_top_left {
                    li
                } else if p_top <= p_top_left {
                    above[2 + j]
                } else {
                    tl
                };
            }
        }
    }

    for i in 0..h {
        let off = (y + i) * stride + x;
        for (b, &v) in buf[off..off + w].iter_mut().zip(&pred[i * w..(i + 1) * w]) {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                *b = v.clamp(0, 255) as u8;
            }
        }
    }
}

/// `is_directional_mode` (spec 5.11.44).
#[inline]
pub fn is_directional_mode(mode: usize) -> bool {
    (V_PRED..=D67_PRED).contains(&mode)
}

/// Recursive intra prediction process (spec 7.11.2.3) — filter intra.
fn recursive_intra(
    pred: &mut [i32],
    above: &[i32; EDGE_BUF],
    left: &[i32; EDGE_BUF],
    w: usize,
    h: usize,
    filter_intra_mode: usize,
) {
    let w4 = w >> 2;
    let h2 = h >> 1;
    for i2 in 0..h2 {
        for j4 in 0..w4 {
            let mut p = [0i32; 7];
            for (i, pi) in p.iter_mut().enumerate() {
                if i < 5 {
                    *pi = if i2 == 0 {
                        above[2 + ((j4 << 2) + i) - 1]
                    } else if j4 == 0 && i == 0 {
                        left[2 + ((i2 << 1) - 1)]
                    } else {
                        pred[((i2 << 1) - 1) * w + (j4 << 2) + i - 1]
                    };
                } else {
                    *pi = if j4 == 0 {
                        left[2 + ((i2 << 1) + i - 5)]
                    } else {
                        pred[((i2 << 1) + i - 5) * w + (j4 << 2) - 1]
                    };
                }
            }
            for i1 in 0..2usize {
                for j1 in 0..4usize {
                    let mut pr = 0i32;
                    for (i, &pi) in p.iter().enumerate() {
                        pr +=
                            i32::from(INTRA_FILTER_TAPS[filter_intra_mode][(i1 << 2) + j1][i]) * pi;
                    }
                    pred[((i2 << 1) + i1) * w + (j4 << 2) + j1] =
                        clip1(round2_signed(pr, INTRA_FILTER_SCALE_BITS));
                }
            }
        }
    }
}

/// Directional intra prediction process (spec 7.11.2.4) including corner
/// filter, edge filter and edge upsampling.
#[allow(clippy::too_many_lines)]
fn directional_intra(
    pred: &mut [i32],
    above: &mut [i32; EDGE_BUF],
    left: &mut [i32; EDGE_BUF],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    p: &PredParams,
) {
    let p_angle = i32::from(MODE_TO_ANGLE[p.mode]) + p.angle_delta * ANGLE_STEP as i32;
    let mut upsample_above = false;
    let mut upsample_left = false;

    if p.enable_intra_edge_filter {
        if p_angle != 90 && p_angle != 180 {
            if p_angle > 90 && p_angle < 180 && (w + h) >= 24 {
                // Filter corner process (spec 7.11.2.7).
                let s = left[2] * 5 + above[1] * 6 + above[2] * 5;
                let v = round2(s, 4);
                above[1] = v;
                left[1] = v;
            }
            let filter_type = p.filter_type;
            if p.have_above {
                let strength = edge_filter_strength(w, h, filter_type, p_angle - 90);
                let num_px =
                    core::cmp::min(w, p.max_x - x + 1) + if p_angle < 90 { h } else { 0 } + 1;
                edge_filter(&mut above[1..], num_px, strength);
            }
            if p.have_left {
                let strength = edge_filter_strength(w, h, filter_type, p_angle - 180);
                let num_px =
                    core::cmp::min(h, p.max_y - y + 1) + if p_angle > 180 { w } else { 0 } + 1;
                edge_filter(&mut left[1..], num_px, strength);
            }
        }
        upsample_above = use_edge_upsample(w, h, p.filter_type, p_angle - 90);
        let num_px = w + if p_angle < 90 { h } else { 0 };
        if upsample_above {
            edge_upsample(above, num_px);
        }
        upsample_left = use_edge_upsample(w, h, p.filter_type, p_angle - 180);
        let num_px = h + if p_angle > 180 { w } else { 0 };
        if upsample_left {
            edge_upsample(left, num_px);
        }
    }

    let ua = u32::from(upsample_above);
    let ul = u32::from(upsample_left);

    if p_angle < 90 {
        let dx = i32::from(DR_INTRA_DERIVATIVE[p_angle as usize]);
        let max_base_x = ((w + h - 1) << ua) as i32;
        let tail = above[2 + max_base_x as usize];
        for i in 0..h {
            let idx = (i as i32 + 1) * dx;
            let base0 = idx >> (6 - ua);
            let shift = ((idx << ua) >> 1) & 0x1F;
            let row = &mut pred[i * w..(i + 1) * w];
            if ua == 0 {
                // `base` advances by exactly one per `j`, so the two taps
                // read two overlapping contiguous windows of AboveRow.
                // `count` is how many leading `j` satisfy `base < maxBaseX`;
                // beyond it the spec substitutes AboveRow[maxBaseX], so the
                // kernel never reads past the edge buffer.
                let count = (max_base_x - base0).clamp(0, w as i32) as usize;
                if count == 0 {
                    row.fill(tail);
                } else {
                    let edge = &above[(2 + base0) as usize..];
                    for (j, r) in row[..count].iter_mut().enumerate() {
                        *r = round2(edge[j] * (32 - shift) + edge[j + 1] * shift, 5);
                    }
                    row[count..].fill(tail);
                }
            } else {
                // Upsampled edge: `base` advances by two per `j`, so the
                // loads are strided — kept on the spec's scalar path.
                for j in 0..w {
                    let base = base0 + ((j as i32) << ua);
                    row[j] = if base < max_base_x {
                        round2(
                            above[(2 + base) as usize] * (32 - shift)
                                + above[(2 + base + 1) as usize] * shift,
                            5,
                        )
                    } else {
                        tail
                    };
                }
            }
        }
    } else if p_angle > 90 && p_angle < 180 {
        let dx = i32::from(DR_INTRA_DERIVATIVE[(180 - p_angle) as usize]);
        let dy = i32::from(DR_INTRA_DERIVATIVE[(p_angle - 90) as usize]);
        for i in 0..h {
            for j in 0..w {
                let idx = ((j as i32) << 6) - (i as i32 + 1) * dx;
                let base = idx >> (6 - ua);
                if base >= -(1 << ua) {
                    let shift = ((idx << ua) >> 1) & 0x1F;
                    pred[i * w + j] = round2(
                        above[(2 + base) as usize] * (32 - shift)
                            + above[(2 + base + 1) as usize] * shift,
                        5,
                    );
                } else {
                    let idx = ((i as i32) << 6) - (j as i32 + 1) * dy;
                    let base = idx >> (6 - ul);
                    let shift = ((idx << ul) >> 1) & 0x1F;
                    pred[i * w + j] = round2(
                        left[(2 + base) as usize] * (32 - shift)
                            + left[(2 + base + 1) as usize] * shift,
                        5,
                    );
                }
            }
        }
    } else if p_angle > 180 {
        let dy = i32::from(DR_INTRA_DERIVATIVE[(270 - p_angle) as usize]);
        for i in 0..h {
            for j in 0..w {
                let idx = (j as i32 + 1) * dy;
                let base = (idx >> (6 - ul)) + ((i as i32) << ul);
                let shift = ((idx << ul) >> 1) & 0x1F;
                pred[i * w + j] = round2(
                    left[(2 + base) as usize] * (32 - shift)
                        + left[(2 + base + 1) as usize] * shift,
                    5,
                );
            }
        }
    } else if p_angle == 90 {
        // V_PRED: every row is a copy of AboveRow[0..w].
        for i in 0..h {
            pred[i * w..(i + 1) * w].copy_from_slice(&above[2..2 + w]);
        }
    } else {
        // p_angle == 180 — H_PRED: every row is a splat of LeftCol[i].
        for i in 0..h {
            pred[i * w..(i + 1) * w].fill(left[2 + i]);
        }
    }
}

/// Intra edge filter strength selection (spec 7.11.2.9).
fn edge_filter_strength(w: usize, h: usize, filter_type: bool, delta: i32) -> u32 {
    let d = delta.unsigned_abs();
    let blk_wh = w + h;
    let mut strength = 0;
    if filter_type {
        if blk_wh <= 8 {
            if d >= 40 {
                strength = 1;
            }
            if d >= 64 {
                strength = 2;
            }
        } else if blk_wh <= 16 {
            if d >= 20 {
                strength = 1;
            }
            if d >= 48 {
                strength = 2;
            }
        } else if blk_wh <= 24 {
            if d >= 4 {
                strength = 3;
            }
        } else {
            strength = 3;
        }
    } else if blk_wh <= 8 {
        if d >= 56 {
            strength = 1;
        }
    } else if blk_wh <= 16 {
        if d >= 40 {
            strength = 1;
        }
    } else if blk_wh <= 24 {
        if d >= 8 {
            strength = 1;
        }
        if d >= 16 {
            strength = 2;
        }
        if d >= 32 {
            strength = 3;
        }
    } else if blk_wh <= 32 {
        strength = 1;
        if d >= 4 {
            strength = 2;
        }
        if d >= 32 {
            strength = 3;
        }
    } else {
        strength = 3;
    }
    strength
}

/// Intra edge upsample selection (spec 7.11.2.10).
fn use_edge_upsample(w: usize, h: usize, filter_type: bool, delta: i32) -> bool {
    let d = delta.abs();
    let blk_wh = w + h;
    if d <= 0 || d >= 40 {
        false
    } else if filter_type {
        blk_wh <= 8
    } else {
        blk_wh <= 16
    }
}

/// Intra edge filter process (spec 7.11.2.12) on `buf` where `buf[0]` is the
/// -1 entry (i.e. `buf = &edge[1..]` of the +2-offset array so that
/// `buf[i]` == `Edge[i - 1]`).
fn edge_filter(buf: &mut [i32], sz: usize, strength: u32) {
    if strength == 0 {
        return;
    }
    let mut edge = [0i32; 130];
    edge[..sz].copy_from_slice(&buf[..sz]);
    for i in 1..sz {
        let mut s = 0;
        for j in 0..5usize {
            let k = (i + j).saturating_sub(2).min(sz - 1);
            s += i32::from(INTRA_EDGE_KERNEL[(strength - 1) as usize][j]) * edge[k];
        }
        buf[i] = (s + 8) >> 4;
    }
}

/// Intra edge upsample process (spec 7.11.2.11) on a +2-offset edge array
/// (entries -1..numPx-1 valid on entry; -2..2*numPx-2 valid on exit).
fn edge_upsample(edge: &mut [i32; EDGE_BUF], num_px: usize) {
    let mut dup = [0i32; 132];
    dup[0] = edge[1]; // buf[-1]
    for i in 0..=num_px {
        dup[i + 1] = edge[1 + i]; // buf[i-1] for i = -1..numPx-1
    }
    dup[num_px + 2] = edge[2 + num_px - 1];

    edge[0] = dup[0]; // buf[-2]
    for i in 0..num_px {
        let s = -dup[i] + 9 * dup[i + 1] + 9 * dup[i + 2] - dup[i + 3];
        let s = clip1(round2(s, 4));
        // buf[2i - 1] and buf[2i] with +2 offset:
        edge[2 + 2 * i - 1] = s;
        edge[2 + 2 * i] = dup[i + 2];
    }
}

/// DC intra prediction process (spec 7.11.2.5).
#[allow(clippy::too_many_arguments)]
fn dc_intra(
    pred: &mut [i32],
    above: &[i32; EDGE_BUF],
    left: &[i32; EDGE_BUF],
    w: usize,
    h: usize,
    log2w: u32,
    log2h: u32,
    have_left: bool,
    have_above: bool,
) {
    let val = if have_left && have_above {
        let mut sum = 0i32;
        for k in 0..h {
            sum += left[2 + k];
        }
        for k in 0..w {
            sum += above[2 + k];
        }
        sum += ((w + h) >> 1) as i32;
        sum / (w + h) as i32
    } else if have_left {
        let mut sum = 0i32;
        for k in 0..h {
            sum += left[2 + k];
        }
        clip1((sum + ((h >> 1) as i32)) >> log2h)
    } else if have_above {
        let mut sum = 0i32;
        for k in 0..w {
            sum += above[2 + k];
        }
        clip1((sum + ((w >> 1) as i32)) >> log2w)
    } else {
        1 << 7
    };
    for v in pred.iter_mut().take(w * h) {
        *v = val;
    }
}

/// Smooth intra prediction process (spec 7.11.2.6).
fn smooth_intra(
    pred: &mut [i32],
    above: &[i32; EDGE_BUF],
    left: &[i32; EDGE_BUF],
    w: usize,
    h: usize,
    mode: usize,
    log2w: u32,
    log2h: u32,
) {
    let weights = |log2: u32| -> &'static [u8] {
        match log2 {
            2 => &SM_WEIGHTS_TX_4X4[..],
            3 => &SM_WEIGHTS_TX_8X8[..],
            4 => &SM_WEIGHTS_TX_16X16[..],
            5 => &SM_WEIGHTS_TX_32X32[..],
            _ => &SM_WEIGHTS_TX_64X64[..],
        }
    };
    if mode == SMOOTH_PRED {
        let wx = weights(log2w);
        let wy = weights(log2h);
        let (lb, ar) = (left[2 + h - 1], above[2 + w - 1]);
        for i in 0..h {
            let (wyi, li) = (i32::from(wy[i]), left[2 + i]);
            let row = &mut pred[i * w..(i + 1) * w];
            for (j, r) in row.iter_mut().enumerate() {
                let wxj = i32::from(wx[j]);
                let sm = wyi * above[2 + j] + (256 - wyi) * lb + wxj * li + (256 - wxj) * ar;
                *r = round2(sm, 9);
            }
        }
    } else if mode == SMOOTH_V_PRED {
        let wy = weights(log2h);
        let lb = left[2 + h - 1];
        for i in 0..h {
            let wyi = i32::from(wy[i]);
            let row = &mut pred[i * w..(i + 1) * w];
            for (j, r) in row.iter_mut().enumerate() {
                *r = round2(wyi * above[2 + j] + (256 - wyi) * lb, 8);
            }
        }
    } else {
        let wx = weights(log2w);
        let ar = above[2 + w - 1];
        for i in 0..h {
            let li = left[2 + i];
            let row = &mut pred[i * w..(i + 1) * w];
            for (j, r) in row.iter_mut().enumerate() {
                let wxj = i32::from(wx[j]);
                *r = round2(wxj * li + (256 - wxj) * ar, 8);
            }
        }
    }
}

/// Predict chroma from luma (spec 7.11.5): applies the scaled, mean-removed
/// subsampled luma on top of the DC prediction already in the chroma plane.
#[allow(clippy::too_many_arguments)]
pub fn predict_cfl(
    chroma: &mut [u8],
    chroma_stride: usize,
    luma: &[u8],
    luma_stride: usize,
    start_x: usize,
    start_y: usize,
    log2w: u32,
    log2h: u32,
    alpha: i32,
    sub_x: bool,
    sub_y: bool,
    max_luma_w: usize,
    max_luma_h: usize,
) {
    let w = 1usize << log2w;
    let h = 1usize << log2h;
    let sub_x_u = usize::from(sub_x);
    let sub_y_u = usize::from(sub_y);
    let mut l = vec![0i32; w * h];
    let mut luma_avg: i64 = 0;
    for i in 0..h {
        let mut luma_y = (start_y + i) << sub_y_u;
        luma_y = core::cmp::min(luma_y, max_luma_h - (1 << sub_y_u));
        for j in 0..w {
            let mut luma_x = (start_x + j) << sub_x_u;
            luma_x = core::cmp::min(luma_x, max_luma_w - (1 << sub_x_u));
            let mut t = 0i32;
            for dy in 0..=sub_y_u {
                for dx in 0..=sub_x_u {
                    t += i32::from(luma[(luma_y + dy) * luma_stride + luma_x + dx]);
                }
            }
            let v = t << (3 - sub_x_u - sub_y_u);
            l[i * w + j] = v;
            luma_avg += i64::from(v);
        }
    }
    let shift = log2w + log2h;
    let luma_avg = ((luma_avg + (1 << (shift - 1))) >> shift) as i32;

    for i in 0..h {
        for j in 0..w {
            let dc = i32::from(chroma[(start_y + i) * chroma_stride + start_x + j]);
            let scaled = round2_signed(alpha * (l[i * w + j] - luma_avg), 6);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            {
                chroma[(start_y + i) * chroma_stride + start_x + j] = clip1(dc + scaled) as u8;
            }
        }
    }
}
