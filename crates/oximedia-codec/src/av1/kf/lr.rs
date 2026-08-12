//! AV1 loop restoration (spec 7.17): Wiener and self-guided (SGR) filters.
//!
//! Exact port of the loop restoration process: per-4x4-block application
//! over 64-luma-row stripes (with the 8-row offset and the deblocked-frame
//! fallback outside the stripe), the self-guided box filter, the Wiener
//! 7-tap separable filter, plus the `read_lr`/`read_lr_unit` syntax
//! (spec 5.11.57/58) with `decode_signed_subexp_with_ref_bool`.

#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use rayon::prelude::*;

use super::cdfs::CdfCtx;
use super::consts::{
    FILTER_BITS, RESTORE_NONE, RESTORE_SGRPROJ, RESTORE_SWITCHABLE, RESTORE_WIENER,
    SGRPROJ_MTABLE_BITS, SGRPROJ_PARAMS_BITS, SGRPROJ_PRJ_BITS, SGRPROJ_PRJ_SUBEXP_K,
    SGRPROJ_RECIP_BITS, SGRPROJ_RST_BITS, SGRPROJ_SGR_BITS,
};
use super::hdr::FrameHdr;
use super::msac::Msac;
use super::recon::{auto_band_count, plan_bands, split_out_bands, OutBand, PlaneBuf};
use super::tables_conv::{
    SGRPROJ_XQD_MAX, SGRPROJ_XQD_MID, SGRPROJ_XQD_MIN, SGR_PARAMS, WIENER_TAPS_K, WIENER_TAPS_MAX,
    WIENER_TAPS_MID, WIENER_TAPS_MIN,
};

/// Smallest luma area worth giving a loop-restoration band of its own. The
/// self-guided box filter is far heavier per pixel than CDEF, so the split
/// pays off on smaller frames.
const LR_MIN_BAND_PIXELS: usize = 1 << 14;

/// `Round2` for unsigned/positive quantities.
#[inline]
fn round2(x: i64, n: u32) -> i64 {
    if n == 0 {
        return x;
    }
    (x + (1 << (n - 1))) >> n
}

/// Per-plane loop restoration unit grids.
pub struct LrUnitGrids {
    pub unit_rows: [usize; 3],
    pub unit_cols: [usize; 3],
    /// `LrType[plane][unitRow][unitCol]`.
    pub types: [Vec<u8>; 3],
    /// `LrWiener[plane][...][pass][coef]`.
    pub wiener: [Vec<[[i32; 3]; 2]>; 3],
    /// `LrSgrSet[plane][...]`.
    pub sgr_set: [Vec<u8>; 3],
    /// `LrSgrXqd[plane][...][i]`.
    pub sgr_xqd: [Vec<[i32; 2]>; 3],
}

/// `count_units_in_frame` (spec 5.11.57).
#[inline]
pub fn count_units_in_frame(unit_size: usize, frame_size: usize) -> usize {
    core::cmp::max((frame_size + (unit_size >> 1)) / unit_size, 1)
}

impl LrUnitGrids {
    pub fn new(hdr: &FrameHdr, sub_x: bool, sub_y: bool, num_planes: usize) -> Self {
        let mut g = Self {
            unit_rows: [0; 3],
            unit_cols: [0; 3],
            types: [Vec::new(), Vec::new(), Vec::new()],
            wiener: [Vec::new(), Vec::new(), Vec::new()],
            sgr_set: [Vec::new(), Vec::new(), Vec::new()],
            sgr_xqd: [Vec::new(), Vec::new(), Vec::new()],
        };
        for plane in 0..num_planes {
            if hdr.lr.frame_restoration_type[plane] == RESTORE_NONE {
                continue;
            }
            let (sx, sy) = if plane == 0 {
                (0u32, 0u32)
            } else {
                (u32::from(sub_x), u32::from(sub_y))
            };
            let unit_size = hdr.lr.loop_restoration_size[plane] as usize;
            let rows =
                count_units_in_frame(unit_size, round2(i64::from(hdr.frame_height), sy) as usize);
            let cols = count_units_in_frame(
                unit_size,
                round2(i64::from(hdr.upscaled_width), sx) as usize,
            );
            g.unit_rows[plane] = rows;
            g.unit_cols[plane] = cols;
            g.types[plane] = vec![RESTORE_NONE as u8; rows * cols];
            g.wiener[plane] = vec![[[0; 3]; 2]; rows * cols];
            g.sgr_set[plane] = vec![0; rows * cols];
            g.sgr_xqd[plane] = vec![[0; 2]; rows * cols];
        }
        g
    }
}

/// Per-tile reference state for LR parameter prediction.
pub struct LrRefs {
    pub sgr_xqd: [[i32; 2]; 3],
    pub wiener: [[[i32; 3]; 2]; 3],
}

impl LrRefs {
    /// Per-tile init (spec 5.11.2 decode_tile).
    pub fn new() -> Self {
        let mut r = Self {
            sgr_xqd: [[0; 2]; 3],
            wiener: [[[0; 3]; 2]; 3],
        };
        for plane in 0..3 {
            for pass in 0..2 {
                r.sgr_xqd[plane][pass] = i32::from(SGRPROJ_XQD_MID[pass]);
                for i in 0..3 {
                    r.wiener[plane][pass][i] = i32::from(WIENER_TAPS_MID[i]);
                }
            }
        }
        r
    }
}

/// `inverse_recenter( r, v )` (spec 5.9.27).
fn inverse_recenter(r: i32, v: i32) -> i32 {
    if v > 2 * r {
        v
    } else if v & 1 != 0 {
        r - ((v + 1) >> 1)
    } else {
        r + (v >> 1)
    }
}

/// `decode_subexp_bool( numSyms, k )` (spec 5.11.58).
fn decode_subexp_bool(msac: &mut Msac, num_syms: i32, k: u32) -> i32 {
    let mut i = 0u32;
    let mut mk = 0i32;
    loop {
        let b2 = if i > 0 { k + i - 1 } else { k };
        let a = 1i32 << b2;
        if num_syms <= mk + 3 * a {
            let v = msac.read_ns((num_syms - mk) as u32) as i32;
            return v + mk;
        }
        if msac.read_literal(1) != 0 {
            i += 1;
            mk += a;
        } else {
            let v = msac.read_literal(b2) as i32;
            return v + mk;
        }
    }
}

/// `decode_signed_subexp_with_ref_bool( low, high, k, r )` (spec 5.11.58).
fn decode_signed_subexp_with_ref_bool(msac: &mut Msac, low: i32, high: i32, k: u32, r: i32) -> i32 {
    let mx = high - low;
    let r = r - low;
    let v = decode_subexp_bool(msac, mx, k);
    let x = if (r << 1) <= mx {
        inverse_recenter(r, v)
    } else {
        mx - 1 - inverse_recenter(mx - 1 - r, v)
    };
    x + low
}

/// `read_lr_unit` (spec 5.11.58).
#[allow(clippy::too_many_arguments)]
pub fn read_lr_unit(
    msac: &mut Msac,
    cdfs: &mut CdfCtx,
    refs: &mut LrRefs,
    grids: &mut LrUnitGrids,
    frame_restoration_type: usize,
    plane: usize,
    unit_row: usize,
    unit_col: usize,
) {
    let restoration_type = if frame_restoration_type == RESTORE_WIENER {
        if msac.read_symbol(&mut cdfs.use_wiener) != 0 {
            RESTORE_WIENER
        } else {
            RESTORE_NONE
        }
    } else if frame_restoration_type == RESTORE_SGRPROJ {
        if msac.read_symbol(&mut cdfs.use_sgrproj) != 0 {
            RESTORE_SGRPROJ
        } else {
            RESTORE_NONE
        }
    } else {
        debug_assert_eq!(frame_restoration_type, RESTORE_SWITCHABLE);
        msac.read_symbol(&mut cdfs.restoration_type)
    };
    let idx = unit_row * grids.unit_cols[plane] + unit_col;
    grids.types[plane][idx] = restoration_type as u8;
    if restoration_type == RESTORE_WIENER {
        for pass in 0..2 {
            let first_coeff = if plane != 0 {
                grids.wiener[plane][idx][pass][0] = 0;
                1
            } else {
                0
            };
            for j in first_coeff..3 {
                let min = i32::from(WIENER_TAPS_MIN[j]);
                let max = i32::from(WIENER_TAPS_MAX[j]);
                let k = u32::from(WIENER_TAPS_K[j]);
                let v = decode_signed_subexp_with_ref_bool(
                    msac,
                    min,
                    max + 1,
                    k,
                    refs.wiener[plane][pass][j],
                );
                grids.wiener[plane][idx][pass][j] = v;
                refs.wiener[plane][pass][j] = v;
            }
        }
    } else if restoration_type == RESTORE_SGRPROJ {
        let lr_sgr_set = msac.read_literal(SGRPROJ_PARAMS_BITS as u32) as usize;
        grids.sgr_set[plane][idx] = lr_sgr_set as u8;
        for i in 0..2 {
            let radius = usize::from(SGR_PARAMS[lr_sgr_set][i * 2]);
            let min = i32::from(SGRPROJ_XQD_MIN[i]);
            let max = i32::from(SGRPROJ_XQD_MAX[i]);
            let v = if radius != 0 {
                decode_signed_subexp_with_ref_bool(
                    msac,
                    min,
                    max + 1,
                    SGRPROJ_PRJ_SUBEXP_K as u32,
                    refs.sgr_xqd[plane][i],
                )
            } else if i == 1 {
                ((1 << SGRPROJ_PRJ_BITS) - refs.sgr_xqd[plane][0]).clamp(min, max)
            } else {
                0
            };
            grids.sgr_xqd[plane][idx][i] = v;
            refs.sgr_xqd[plane][i] = v;
        }
    }
}

// ------------------------------------------------------------- application

/// Sources and geometry for the loop restoration pass.
pub struct LrApply<'a> {
    pub hdr: &'a FrameHdr,
    pub sub_x: bool,
    pub sub_y: bool,
    pub num_planes: usize,
    /// Deblocked frame before CDEF (`UpscaledCurrFrame`).
    pub pre_cdef: &'a [PlaneBuf; 3],
    pub grids: &'a LrUnitGrids,
}

struct StripeCtx {
    stripe_start_y: i32,
    stripe_end_y: i32,
    plane_end_x: i32,
    plane_end_y: i32,
}

/// Applies loop restoration: `planes` holds the CDEF output on input
/// (`UpscaledCdefFrame`) and the restored frame (`LrFrame`) on output.
///
/// `forced_bands` overrides the automatic row-band split (tests only); the
/// restored output does not depend on it.
pub fn loop_restore_frame(
    planes: &mut [PlaneBuf; 3],
    a: &LrApply<'_>,
    forced_bands: Option<usize>,
) {
    let mut out: [Vec<u8>; 3] = [
        planes[0].data.clone(),
        planes[1].data.clone(),
        planes[2].data.clone(),
    ];
    let want = forced_bands
        .unwrap_or_else(|| auto_band_count(planes[0].width * planes[0].height, LR_MIN_BAND_PIXELS));
    // Restoration runs over 4x4 luma blocks, so bands split on multiples of
    // four luma rows; the luma plane height is 4 * MiRows. Rows past
    // FrameHeight are never written and keep the CDEF output, as in the
    // serial loop.
    let bounds = plan_bands(planes[0].height, 4, want);
    let strides = [planes[0].stride, planes[1].stride, planes[2].stride];
    let windows = split_out_bands(&mut out, strides, usize::from(a.sub_y), &bounds);
    let jobs: Vec<((usize, usize), OutBand<'_>)> =
        bounds.iter().copied().zip(windows).collect::<Vec<_>>();
    if jobs.len() > 1 {
        jobs.into_par_iter()
            .for_each(|((y0, y1), mut band)| lr_rows(planes, &mut band, a, y0, y1));
    } else {
        for ((y0, y1), mut band) in jobs {
            lr_rows(planes, &mut band, a, y0, y1);
        }
    }
    for (p, o) in planes.iter_mut().zip(out.into_iter()) {
        p.data = o;
    }
}

/// Runs the restoration block loop over the luma rows `y0..y1` of one band.
fn lr_rows(planes: &[PlaneBuf; 3], out: &mut OutBand<'_>, a: &LrApply<'_>, y0: usize, y1: usize) {
    let end = core::cmp::min(y1, a.hdr.frame_height as usize);
    let upscaled_width = a.hdr.upscaled_width as usize;
    let mut y = y0;
    while y < end {
        let mut x = 0;
        while x < upscaled_width {
            for plane in 0..a.num_planes {
                if a.hdr.lr.frame_restoration_type[plane] != RESTORE_NONE {
                    loop_restore_block(planes, out, a, plane, y >> 2, x >> 2);
                }
            }
            x += 4;
        }
        y += 4;
    }
}

/// Loop restore block process (spec 7.17.1).
fn loop_restore_block(
    planes: &[PlaneBuf; 3],
    out: &mut OutBand<'_>,
    a: &LrApply<'_>,
    plane: usize,
    row: usize,
    col: usize,
) {
    let luma_y = row * 4;
    let stripe_num = (luma_y + 8) / 64;
    let (sub_x, sub_y) = if plane == 0 {
        (0u32, 0u32)
    } else {
        (u32::from(a.sub_x), u32::from(a.sub_y))
    };
    let stripe_start_y = (-8 + stripe_num as i32 * 64) >> sub_y;
    let stripe_end_y = stripe_start_y + (64 >> sub_y) - 1;
    let unit_size = a.hdr.lr.loop_restoration_size[plane] as usize;
    let unit_rows = a.grids.unit_rows[plane];
    let unit_cols = a.grids.unit_cols[plane];
    let unit_row = core::cmp::min(unit_rows - 1, ((row * 4 + 8) >> sub_y) / unit_size);
    let unit_col = core::cmp::min(unit_cols - 1, ((col * 4) >> sub_x) / unit_size);
    let plane_end_x = round2(i64::from(a.hdr.upscaled_width), sub_x) as i32 - 1;
    let plane_end_y = round2(i64::from(a.hdr.frame_height), sub_y) as i32 - 1;
    let x = (col * 4) >> sub_x;
    let y = (row * 4) >> sub_y;
    let w = core::cmp::min(4 >> sub_x, plane_end_x as usize - x + 1);
    let h = core::cmp::min(4 >> sub_y, plane_end_y as usize - y + 1);
    let idx = unit_row * unit_cols + unit_col;
    let r_type = usize::from(a.grids.types[plane][idx]);
    let sc = StripeCtx {
        stripe_start_y,
        stripe_end_y,
        plane_end_x,
        plane_end_y,
    };
    if r_type == RESTORE_WIENER {
        wiener_filter(planes, out, a, &sc, plane, idx, x, y, w, h);
    } else if r_type == RESTORE_SGRPROJ {
        self_guided_filter(planes, out, a, &sc, plane, idx, x, y, w, h);
    }
}

/// `get_source_sample` (spec 7.17.6).
#[inline]
fn get_source_sample(cdef: &PlaneBuf, pre_cdef: &PlaneBuf, sc: &StripeCtx, x: i32, y: i32) -> i32 {
    let x = x.clamp(0, sc.plane_end_x);
    let y = y.clamp(0, sc.plane_end_y);
    if y < sc.stripe_start_y {
        let y = core::cmp::max(sc.stripe_start_y - 2, y);
        i32::from(pre_cdef.data[(y as usize) * pre_cdef.stride + x as usize])
    } else if y > sc.stripe_end_y {
        let y = core::cmp::min(sc.stripe_end_y + 2, y);
        i32::from(pre_cdef.data[(y as usize) * pre_cdef.stride + x as usize])
    } else {
        i32::from(cdef.data[(y as usize) * cdef.stride + x as usize])
    }
}

/// Wiener filter process (spec 7.17.4), 8-bit: InterRound0 = 3,
/// InterRound1 = 11 (spec 7.11.3.2 with isCompound = 0).
#[allow(clippy::too_many_arguments)]
fn wiener_filter(
    planes: &[PlaneBuf; 3],
    out: &mut OutBand<'_>,
    a: &LrApply<'_>,
    sc: &StripeCtx,
    plane: usize,
    idx: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    const INTER_ROUND0: u32 = 3;
    const INTER_ROUND1: u32 = 11;
    let coeffs = &a.grids.wiener[plane][idx];
    let mut vfilter = [0i32; 7];
    let mut hfilter = [0i32; 7];
    wiener_coefficients(&coeffs[0], &mut vfilter);
    wiener_coefficients(&coeffs[1], &mut hfilter);

    let offset = 1i32 << (8 + FILTER_BITS as u32 - INTER_ROUND0 - 1);
    let limit = (1i32 << (8 + 1 + FILTER_BITS as u32 - INTER_ROUND0)) - 1;
    let cdef = &planes[plane];
    let pre = &a.pre_cdef[plane];
    let mut intermediate = [[0i32; 4]; 10]; // (h + 6) x w, h/w <= 4
    for r in 0..h + 6 {
        for c in 0..w {
            let mut s = 0i64;
            for (t, &hf) in hfilter.iter().enumerate() {
                let sx = x as i32 + c as i32 + t as i32 - 3;
                let sy = y as i32 + r as i32 - 3;
                s += i64::from(hf) * i64::from(get_source_sample(cdef, pre, sc, sx, sy));
            }
            let v = round2(s, INTER_ROUND0) as i32;
            intermediate[r][c] = v.clamp(-offset, limit - offset);
        }
    }
    for r in 0..h {
        for c in 0..w {
            let mut s = 0i64;
            for (t, &vf) in vfilter.iter().enumerate() {
                s += i64::from(vf) * i64::from(intermediate[r + t][c]);
            }
            let v = round2(s, INTER_ROUND1) as i32;
            out.put(plane, y + r, x + c, v.clamp(0, 255) as u8);
        }
    }
}

/// Wiener coefficient process (spec 7.17.5).
fn wiener_coefficients(coeff: &[i32; 3], filter: &mut [i32; 7]) {
    filter[3] = 128;
    for i in 0..3 {
        let c = coeff[i];
        filter[i] = c;
        filter[6 - i] = c;
        filter[3] -= 2 * c;
    }
}

/// Self-guided filter process (spec 7.17.2).
#[allow(clippy::too_many_arguments)]
fn self_guided_filter(
    planes: &[PlaneBuf; 3],
    out: &mut OutBand<'_>,
    a: &LrApply<'_>,
    sc: &StripeCtx,
    plane: usize,
    idx: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) {
    let set = usize::from(a.grids.sgr_set[plane][idx]);
    let mut flt0 = [[0i64; 4]; 4];
    let mut flt1 = [[0i64; 4]; 4];
    box_filter(planes, a, sc, plane, x, y, w, h, set, 0, &mut flt0);
    box_filter(planes, a, sc, plane, x, y, w, h, set, 1, &mut flt1);

    let w0 = i64::from(a.grids.sgr_xqd[plane][idx][0]);
    let w1 = i64::from(a.grids.sgr_xqd[plane][idx][1]);
    let w2 = (1i64 << SGRPROJ_PRJ_BITS) - w0 - w1;
    let r0 = SGR_PARAMS[set][0];
    let r1 = SGR_PARAMS[set][2];
    let cdef = &planes[plane];
    for i in 0..h {
        for j in 0..w {
            let u = i64::from(cdef.data[(y + i) * cdef.stride + x + j]) << SGRPROJ_RST_BITS;
            let mut v = w1 * u;
            v += w0 * if r0 != 0 { flt0[i][j] } else { u };
            v += w2 * if r1 != 0 { flt1[i][j] } else { u };
            let s = round2(v, (SGRPROJ_RST_BITS + SGRPROJ_PRJ_BITS) as u32);
            out.put(plane, y + i, x + j, s.clamp(0, 255) as u8);
        }
    }
}

/// Box filter process (spec 7.17.3). `f` is the output block (h x w).
#[allow(clippy::too_many_arguments)]
fn box_filter(
    planes: &[PlaneBuf; 3],
    a: &LrApply<'_>,
    sc: &StripeCtx,
    plane: usize,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    set: usize,
    pass: usize,
    f: &mut [[i64; 4]; 4],
) {
    let r = usize::from(SGR_PARAMS[set][pass * 2]);
    if r == 0 {
        return;
    }
    let eps = i64::from(SGR_PARAMS[set][pass * 2 + 1]);
    let n = ((2 * r + 1) * (2 * r + 1)) as i64;
    let n2e = n * n * eps;
    let s = ((1i64 << SGRPROJ_MTABLE_BITS) + n2e / 2) / n2e;
    let cdef = &planes[plane];
    let pre = &a.pre_cdef[plane];

    // A and B are valid for -1..=h and -1..=w (offset +1, max 6x6).
    let mut arr_a = [[0i64; 6]; 6];
    let mut arr_b = [[0i64; 6]; 6];
    for i in -1..=(h as i32) {
        for j in -1..=(w as i32) {
            let mut acc_a = 0i64;
            let mut acc_b = 0i64;
            for dy in -(r as i32)..=(r as i32) {
                for dx in -(r as i32)..=(r as i32) {
                    let c = i64::from(get_source_sample(
                        cdef,
                        pre,
                        sc,
                        x as i32 + j + dx,
                        y as i32 + i + dy,
                    ));
                    acc_a += c * c;
                    acc_b += c;
                }
            }
            // BitDepth == 8: no rounding shifts apply.
            let a_v = acc_a;
            let d_v = acc_b;
            let p = core::cmp::max(0, a_v * n - d_v * d_v);
            let z = round2(p * s, SGRPROJ_MTABLE_BITS as u32);
            let a2 = if z >= 255 {
                256
            } else if z == 0 {
                1
            } else {
                ((z << SGRPROJ_SGR_BITS) + z / 2) / (z + 1)
            };
            let one_over_n = ((1i64 << SGRPROJ_RECIP_BITS) + n / 2) / n;
            let b2 = ((1i64 << SGRPROJ_SGR_BITS) - a2) * acc_b * one_over_n;
            arr_a[(i + 1) as usize][(j + 1) as usize] = a2;
            arr_b[(i + 1) as usize][(j + 1) as usize] = round2(b2, SGRPROJ_RECIP_BITS as u32);
        }
    }
    for i in 0..h {
        let shift = if pass == 0 && (i & 1) != 0 { 4u32 } else { 5 };
        for j in 0..w {
            let mut acc_a = 0i64;
            let mut acc_b = 0i64;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let weight = if pass == 0 {
                        if (i as i32 + dy) & 1 != 0 {
                            if dx == 0 {
                                6
                            } else {
                                5
                            }
                        } else {
                            0
                        }
                    } else if dx == 0 || dy == 0 {
                        4
                    } else {
                        3
                    };
                    acc_a +=
                        weight * arr_a[(i as i32 + dy + 1) as usize][(j as i32 + dx + 1) as usize];
                    acc_b +=
                        weight * arr_b[(i as i32 + dy + 1) as usize][(j as i32 + dx + 1) as usize];
                }
            }
            let v = acc_a * i64::from(cdef.data[(y + i) * cdef.stride + x + j]) + acc_b;
            f[i][j] = round2(
                v,
                (SGRPROJ_SGR_BITS as u32 + shift) - SGRPROJ_RST_BITS as u32,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::consts::{RESTORE_NONE, RESTORE_SGRPROJ, RESTORE_WIENER};
    use super::super::hdr::FrameHdr;
    use super::super::tables_conv::{
        SGRPROJ_XQD_MAX, SGRPROJ_XQD_MIN, WIENER_TAPS_MAX, WIENER_TAPS_MIN,
    };
    use super::{loop_restore_frame, LrApply, LrUnitGrids, PlaneBuf};

    /// Deterministic filler (splitmix64).
    fn next(state: &mut u64) -> u64 {
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn make_planes(mi_rows: usize, mi_cols: usize, seed: u64) -> [PlaneBuf; 3] {
        let mut state = seed;
        let mut mk = |w: usize, h: usize| PlaneBuf {
            data: (0..w * h).map(|_| next(&mut state) as u8).collect(),
            stride: w,
            width: w,
            height: h,
        };
        let (lw, lh) = (mi_cols * 4, mi_rows * 4);
        [mk(lw, lh), mk(lw / 2, lh / 2), mk(lw / 2, lh / 2)]
    }

    /// Loop restoration reads the CDEF output plus the pre-CDEF deblocked
    /// frame and writes a separate `LrFrame`, so its result must not depend
    /// on how the frame is split into row bands.
    #[test]
    fn lr_row_bands_match_serial() {
        let shapes: [(usize, usize, u32, u32); 4] = [
            // (MiRows, MiCols, FrameWidth, FrameHeight) — the last two are
            // deliberately not MI-aligned, like the 76x42 fixtures.
            (12, 20, 76, 42),
            (32, 32, 128, 128),
            (48, 80, 320, 192),
            (18, 34, 133, 69),
        ];
        for (case, &(mi_rows, mi_cols, fw, fh)) in shapes.iter().enumerate() {
            for types in [
                [RESTORE_WIENER, RESTORE_SGRPROJ, RESTORE_SGRPROJ],
                [RESTORE_SGRPROJ, RESTORE_WIENER, RESTORE_WIENER],
            ] {
                let mut state = 0x1234_5678_u64.wrapping_add(case as u64) ^ types[0] as u64;
                let mut hdr = FrameHdr::default();
                hdr.frame_width = fw;
                hdr.frame_height = fh;
                hdr.upscaled_width = fw;
                hdr.lr.uses_lr = true;
                hdr.lr.frame_restoration_type = types;
                hdr.lr.loop_restoration_size = [64, 32, 32];
                let mut grids = LrUnitGrids::new(&hdr, true, true, 3);
                for plane in 0..3 {
                    let n = grids.unit_rows[plane] * grids.unit_cols[plane];
                    for idx in 0..n {
                        // Mix RESTORE_NONE units in, as SWITCHABLE luma does.
                        grids.types[plane][idx] = if next(&mut state) % 4 == 0 {
                            RESTORE_NONE as u8
                        } else {
                            types[plane] as u8
                        };
                        for pass in 0..2 {
                            for j in 0..3 {
                                let lo = i32::from(WIENER_TAPS_MIN[j]);
                                let hi = i32::from(WIENER_TAPS_MAX[j]);
                                let span = (hi - lo + 1) as u64;
                                grids.wiener[plane][idx][pass][j] =
                                    lo + (next(&mut state) % span) as i32;
                            }
                        }
                        grids.sgr_set[plane][idx] = (next(&mut state) % 16) as u8;
                        for i in 0..2 {
                            let lo = i32::from(SGRPROJ_XQD_MIN[i]);
                            let hi = i32::from(SGRPROJ_XQD_MAX[i]);
                            let span = (hi - lo + 1) as u64;
                            grids.sgr_xqd[plane][idx][i] = lo + (next(&mut state) % span) as i32;
                        }
                    }
                }
                let pre_cdef = make_planes(mi_rows, mi_cols, 0xDEAD_BEEF ^ case as u64);
                let apply = LrApply {
                    hdr: &hdr,
                    sub_x: true,
                    sub_y: true,
                    num_planes: 3,
                    pre_cdef: &pre_cdef,
                    grids: &grids,
                };
                let mut serial = make_planes(mi_rows, mi_cols, 0xFEED_FACE ^ case as u64);
                loop_restore_frame(&mut serial, &apply, Some(1));
                for bands in 2..=9usize {
                    let mut split = make_planes(mi_rows, mi_cols, 0xFEED_FACE ^ case as u64);
                    loop_restore_frame(&mut split, &apply, Some(bands));
                    for plane in 0..3 {
                        assert_eq!(
                            split[plane].data, serial[plane].data,
                            "LR plane {plane} differs at {fw}x{fh} with {bands} bands"
                        );
                    }
                }
            }
        }
    }
}
