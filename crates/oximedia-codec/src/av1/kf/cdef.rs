//! AV1 CDEF — constrained directional enhancement filter (spec 7.15).
//!
//! Exact port of the CDEF process: per-8x8-block application driven by the
//! per-64x64 `cdef_idx` grid, the direction/variance search on luma, and
//! the primary/secondary tap filter with the `constrain` damping function.
//! CDEF reads from the deblocked frame (`CurrFrame`) and writes into a
//! separate output (`CdefFrame`); this implementation materializes that as
//! a copy of the planes that is filtered and then swapped in.

#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use rayon::prelude::*;

use super::bits::floor_log2;
use super::hdr::{CdefParams, FrameHdr};
use super::recon::{auto_band_count, plan_bands, split_out_bands, OutBand, PlaneBuf};
use super::tables_conv::{CDEF_DIRECTIONS, CDEF_PRI_TAPS, CDEF_SEC_TAPS, CDEF_UV_DIR, DIV_TABLE};

/// Smallest luma area worth giving a CDEF band of its own. Below roughly
/// this much work per band the rayon dispatch costs more than the filtering
/// it hands off, so small frames stay on the serial path.
const CDEF_MIN_BAND_PIXELS: usize = 1 << 16;

/// Frame state the CDEF pass needs.
pub struct CdefInput<'a> {
    pub hdr: &'a FrameHdr,
    pub sub_x: bool,
    pub sub_y: bool,
    pub num_planes: usize,
    pub mi_rows: usize,
    pub mi_cols: usize,
    pub skips: &'a [u8],
    /// Per-MI `cdef_idx` written during tile decode (-1 where unset).
    pub cdef_idx: &'a [i16],
}

/// Applies CDEF: `planes` is CurrFrame on input, CdefFrame on output.
///
/// `forced_bands` overrides the automatic row-band split (tests only); the
/// filtered output does not depend on it.
pub fn cdef_frame(planes: &mut [PlaneBuf; 3], input: &CdefInput<'_>, forced_bands: Option<usize>) {
    // CdefFrame starts as a copy; filtered blocks overwrite it.
    let mut out: [Vec<u8>; 3] = [
        planes[0].data.clone(),
        planes[1].data.clone(),
        planes[2].data.clone(),
    ];
    let want = forced_bands.unwrap_or_else(|| {
        auto_band_count(planes[0].width * planes[0].height, CDEF_MIN_BAND_PIXELS)
    });
    // Each 8x8 CDEF block covers two MI rows, so bands split on even ones.
    // MI row r covers luma rows 4r..4r+8, so band k owns exactly luma rows
    // 4*r0..4*r1 — which requires MiRows to be even. It always is (spec
    // 5.9.2 `MiRows = 2 * ((frame_height_minus_1 + 8) >> 3)`), and the block
    // loop below already depends on it; pin it where it is consumed.
    debug_assert_eq!(input.mi_rows % 2, 0, "MiRows must be even");
    let mi_bounds = plan_bands(input.mi_rows, 2, want);
    let luma_bounds: Vec<(usize, usize)> = mi_bounds.iter().map(|&(a, b)| (a * 4, b * 4)).collect();
    let strides = [planes[0].stride, planes[1].stride, planes[2].stride];
    let windows = split_out_bands(&mut out, strides, usize::from(input.sub_y), &luma_bounds);
    let jobs: Vec<((usize, usize), OutBand<'_>)> = mi_bounds.into_iter().zip(windows).collect();
    if jobs.len() > 1 {
        jobs.into_par_iter()
            .for_each(|((r0, r1), mut band)| cdef_rows(planes, &mut band, input, r0, r1));
    } else {
        for ((r0, r1), mut band) in jobs {
            cdef_rows(planes, &mut band, input, r0, r1);
        }
    }
    for (p, o) in planes.iter_mut().zip(out.into_iter()) {
        p.data = o;
    }
}

/// Runs the CDEF block loop over the MI rows `r0..r1` of one output band.
fn cdef_rows(
    planes: &[PlaneBuf; 3],
    out: &mut OutBand<'_>,
    input: &CdefInput<'_>,
    r0: usize,
    r1: usize,
) {
    let step4 = 2usize; // Num_4x4_Blocks_Wide[BLOCK_8X8]
    let cdef_size4 = 16usize; // Num_4x4_Blocks_Wide[BLOCK_64X64]
    let cdef_mask4 = !(cdef_size4 - 1);
    let mut r = r0;
    while r < r1 {
        let mut c = 0;
        while c < input.mi_cols {
            let base_r = r & cdef_mask4;
            let base_c = c & cdef_mask4;
            let idx = input.cdef_idx[base_r * input.mi_cols + base_c];
            cdef_block(planes, out, input, r, c, idx);
            c += step4;
        }
        r += step4;
    }
}

/// CDEF block process (spec 7.15.1). The initial copy is implicit (the
/// output starts as a copy of the input).
fn cdef_block(
    planes: &[PlaneBuf; 3],
    out: &mut OutBand<'_>,
    input: &CdefInput<'_>,
    r: usize,
    c: usize,
    idx: i16,
) {
    if idx < 0 {
        return;
    }
    let g = |rr: usize, cc: usize| -> bool {
        rr < input.mi_rows && cc < input.mi_cols && input.skips[rr * input.mi_cols + cc] != 0
    };
    let skip = g(r, c) && g(r + 1, c) && g(r, c + 1) && g(r + 1, c + 1);
    if skip {
        return;
    }
    let (y_dir, var) = cdef_direction(&planes[0], r, c);
    let cdef: &CdefParams = &input.hdr.cdef;
    let idx = idx as usize;
    // Luma (coeffShift = 0 for 8-bit).
    let mut pri_str = cdef.y_pri_strength[idx] as i32;
    let sec_str = cdef.y_sec_strength[idx] as i32;
    let dir = if pri_str == 0 { 0 } else { y_dir };
    let var_str = if (var >> 6) != 0 {
        core::cmp::min(floor_log2((var >> 6) as u32), 12) as i32
    } else {
        0
    };
    pri_str = if var != 0 {
        (pri_str * (4 + var_str) + 8) >> 4
    } else {
        0
    };
    let damping = cdef.damping as i32;
    cdef_filter(planes, out, input, 0, r, c, pri_str, sec_str, damping, dir);
    if input.num_planes == 1 {
        return;
    }
    let pri_str = cdef.uv_pri_strength[idx] as i32;
    let sec_str = cdef.uv_sec_strength[idx] as i32;
    let dir = if pri_str == 0 {
        0
    } else {
        usize::from(CDEF_UV_DIR[usize::from(input.sub_x)][usize::from(input.sub_y)][y_dir])
    };
    let damping = cdef.damping as i32 - 1;
    cdef_filter(planes, out, input, 1, r, c, pri_str, sec_str, damping, dir);
    cdef_filter(planes, out, input, 2, r, c, pri_str, sec_str, damping, dir);
}

/// CDEF direction process (spec 7.15.2) on the luma plane.
fn cdef_direction(luma: &PlaneBuf, r: usize, c: usize) -> (usize, i64) {
    let mut partial = [[0i64; 15]; 8];
    let mut cost = [0i64; 8];
    let x0 = c << 2;
    let y0 = r << 2;
    for i in 0..8usize {
        for j in 0..8usize {
            let x = i64::from(luma.data[(y0 + i) * luma.stride + x0 + j]) - 128;
            partial[0][i + j] += x;
            partial[1][i + j / 2] += x;
            partial[2][i] += x;
            partial[3][3 + i - j / 2] += x;
            partial[4][7 + i - j] += x;
            partial[5][3 - i / 2 + j] += x;
            partial[6][j] += x;
            partial[7][i / 2 + j] += x;
        }
    }
    for i in 0..8 {
        cost[2] += partial[2][i] * partial[2][i];
        cost[6] += partial[6][i] * partial[6][i];
    }
    cost[2] *= i64::from(DIV_TABLE[8]);
    cost[6] *= i64::from(DIV_TABLE[8]);
    for i in 0..7 {
        cost[0] += (partial[0][i] * partial[0][i] + partial[0][14 - i] * partial[0][14 - i])
            * i64::from(DIV_TABLE[i + 1]);
        cost[4] += (partial[4][i] * partial[4][i] + partial[4][14 - i] * partial[4][14 - i])
            * i64::from(DIV_TABLE[i + 1]);
    }
    cost[0] += partial[0][7] * partial[0][7] * i64::from(DIV_TABLE[8]);
    cost[4] += partial[4][7] * partial[4][7] * i64::from(DIV_TABLE[8]);
    for i in (1..8).step_by(2) {
        for j in 0..5 {
            cost[i] += partial[i][3 + j] * partial[i][3 + j];
        }
        cost[i] *= i64::from(DIV_TABLE[8]);
        for j in 0..3 {
            cost[i] += (partial[i][j] * partial[i][j] + partial[i][10 - j] * partial[i][10 - j])
                * i64::from(DIV_TABLE[2 * j + 2]);
        }
    }
    let mut best_cost = 0i64;
    let mut y_dir = 0usize;
    for (i, &ci) in cost.iter().enumerate() {
        if ci > best_cost {
            best_cost = ci;
            y_dir = i;
        }
    }
    let var = (best_cost - cost[(y_dir + 4) & 7]) >> 10;
    (y_dir, var)
}

/// `constrain` (spec 7.15.3).
#[inline]
fn constrain(diff: i32, threshold: i32, damping: i32) -> i32 {
    if threshold == 0 {
        return 0;
    }
    let damping_adj = core::cmp::max(0, damping - floor_log2(threshold as u32) as i32);
    let sign = if diff < 0 { -1 } else { 1 };
    sign * (threshold - (diff.abs() >> damping_adj)).clamp(0, diff.abs())
}

/// CDEF filter process (spec 7.15.3) for one plane of one 8x8 luma block.
#[allow(clippy::too_many_arguments)]
fn cdef_filter(
    planes: &[PlaneBuf; 3],
    out: &mut OutBand<'_>,
    input: &CdefInput<'_>,
    plane: usize,
    r: usize,
    c: usize,
    pri_str: i32,
    sec_str: i32,
    damping: i32,
    dir: usize,
) {
    let (sub_x, sub_y) = if plane == 0 {
        (0usize, 0usize)
    } else {
        (usize::from(input.sub_x), usize::from(input.sub_y))
    };
    let x0 = (c * 4) >> sub_x;
    let y0 = (r * 4) >> sub_y;
    let w = 8 >> sub_x;
    let h = 8 >> sub_y;
    let p = &planes[plane];
    // Frame-wide availability (spec 5.11.52 is_inside_filter_region).
    let get_at = |i: usize, j: usize, d: usize, k: usize, sign: i32| -> Option<i32> {
        let y = y0 as i32 + i as i32 + sign * i32::from(CDEF_DIRECTIONS[d][k][0]);
        let x = x0 as i32 + j as i32 + sign * i32::from(CDEF_DIRECTIONS[d][k][1]);
        let candidate_r = (y << sub_y) >> 2;
        let candidate_c = (x << sub_x) >> 2;
        if candidate_r >= 0
            && (candidate_r as usize) < input.mi_rows
            && candidate_c >= 0
            && (candidate_c as usize) < input.mi_cols
        {
            Some(i32::from(p.data[(y as usize) * p.stride + x as usize]))
        } else {
            None
        }
    };
    let pri_tap_idx = (pri_str & 1) as usize;
    for i in 0..h {
        for j in 0..w {
            let mut sum = 0i32;
            let x = i32::from(p.data[(y0 + i) * p.stride + x0 + j]);
            let mut mx = x;
            let mut mn = x;
            for k in 0..2usize {
                for sign in [-1i32, 1] {
                    if let Some(pv) = get_at(i, j, dir, k, sign) {
                        sum += i32::from(CDEF_PRI_TAPS[pri_tap_idx][k])
                            * constrain(pv - x, pri_str, damping);
                        mx = core::cmp::max(pv, mx);
                        mn = core::cmp::min(pv, mn);
                    }
                    for dir_off in [-2i32, 2] {
                        let d2 = ((dir as i32 + dir_off) & 7) as usize;
                        if let Some(sv) = get_at(i, j, d2, k, sign) {
                            sum += i32::from(CDEF_SEC_TAPS[pri_tap_idx][k])
                                * constrain(sv - x, sec_str, damping);
                            mx = core::cmp::max(sv, mx);
                            mn = core::cmp::min(sv, mn);
                        }
                    }
                }
            }
            let v = (x + ((8 + sum - i32::from(sum < 0)) >> 4)).clamp(mn, mx);
            out.put(plane, y0 + i, x0 + j, v as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::hdr::FrameHdr;
    use super::{cdef_frame, CdefInput, PlaneBuf};

    /// Deterministic filler (splitmix64) so the fixtures below are the same
    /// on every run and every platform.
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

    /// CDEF reads `CurrFrame` and writes a separate `CdefFrame`, so the
    /// output must not depend on how the frame is split into row bands.
    #[test]
    fn cdef_row_bands_match_serial() {
        // MiRows/MiCols are always even (spec 5.9.2); 76x42 decodes to
        // 20x12 MI, which is the odd-size shape the fixtures cover.
        for &(mi_rows, mi_cols) in &[(2usize, 2usize), (12, 20), (16, 16), (18, 34), (34, 18)] {
            let mut state = 0x5EED_1234_u64 ^ (mi_rows as u64) << 32 ^ mi_cols as u64;
            let n = mi_rows * mi_cols;
            let skips: Vec<u8> = (0..n)
                .map(|_| u8::from(next(&mut state) & 3 == 0))
                .collect();
            let cdef_idx: Vec<i16> = (0..n)
                .map(|_| {
                    let v = next(&mut state) % 9;
                    if v == 8 {
                        -1
                    } else {
                        v as i16
                    }
                })
                .collect();
            let mut hdr = FrameHdr::default();
            hdr.frame_width = (mi_cols * 4) as u32;
            hdr.frame_height = (mi_rows * 4) as u32;
            hdr.cdef.damping = 4;
            hdr.cdef.bits = 3;
            for i in 0..8 {
                hdr.cdef.y_pri_strength[i] = (next(&mut state) % 16) as u32;
                hdr.cdef.y_sec_strength[i] = [0u32, 1, 2, 4][(next(&mut state) % 4) as usize];
                hdr.cdef.uv_pri_strength[i] = (next(&mut state) % 16) as u32;
                hdr.cdef.uv_sec_strength[i] = [0u32, 1, 2, 4][(next(&mut state) % 4) as usize];
            }
            let input = CdefInput {
                hdr: &hdr,
                sub_x: true,
                sub_y: true,
                num_planes: 3,
                mi_rows,
                mi_cols,
                skips: &skips,
                cdef_idx: &cdef_idx,
            };
            let mut serial = make_planes(mi_rows, mi_cols, 0xC0FF_EE00);
            cdef_frame(&mut serial, &input, Some(1));
            for bands in 2..=9usize {
                let mut split = make_planes(mi_rows, mi_cols, 0xC0FF_EE00);
                cdef_frame(&mut split, &input, Some(bands));
                for plane in 0..3 {
                    assert_eq!(
                        split[plane].data, serial[plane].data,
                        "CDEF plane {plane} differs at {mi_cols}x{mi_rows} MI with {bands} bands"
                    );
                }
            }
        }
    }
}
