//! Tests for VP9 inter prediction ([`super`]).
//!
//! Lives beside the implementation rather than inside it so the module stays
//! under the 2000-line ceiling (the same split `super::super::mvref` uses).
//!
//! The prediction checks are driven by an independent oracle
//! ([`oracle_pixel`]) that shares no code with the implementation: it fetches
//! every tap directly from the reference plane at clamped coordinates instead
//! of building an `mc_buf`, and re-derives the two-pass rounding from
//! `vpx_dsp/vpx_convolve.c` instead of calling [`super::super::mc`].

use super::*;
use crate::vp9::dec::refs::{LAST_FRAME, NONE_FRAME};

// -----------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------

/// A plane filled with a deterministic, non-separable pattern: no row
/// equals another and no column equals another, so a transposed,
/// shifted or edge-replicated read cannot pass by coincidence.
fn gradient_plane(width: usize, height: usize) -> PlaneBuf {
    let mut data = vec![0u8; width * height];
    for y in 0..height {
        for x in 0..width {
            let v = (7 * x + 13 * y + ((x * y) % 11) + (x % 5) * 17) % 251;
            data[y * width + x] = v as u8;
        }
    }
    PlaneBuf {
        data,
        stride: width,
        width,
        height,
    }
}

/// A blank (mid-grey) plane, for destinations.
fn blank_plane(width: usize, height: usize) -> PlaneBuf {
    PlaneBuf {
        data: vec![128u8; width * height],
        stride: width,
        width,
        height,
    }
}

/// MI-aligned plane dimensions for a 4:2:0 frame of `width` x `height`.
fn aligned_dims(width: usize, height: usize, ss_x: usize, ss_y: usize) -> (usize, usize) {
    let mi_cols = width.div_ceil(MI_SIZE);
    let mi_rows = height.div_ceil(MI_SIZE);
    ((mi_cols * MI_SIZE) >> ss_x, (mi_rows * MI_SIZE) >> ss_y)
}

fn geometry(width: usize, height: usize) -> FrameGeometry {
    FrameGeometry {
        mi_rows: height.div_ceil(MI_SIZE),
        mi_cols: width.div_ceil(MI_SIZE),
        width,
        height,
        ss_x: 1,
        ss_y: 1,
    }
}

/// Three gradient planes shaped like a decoded 4:2:0 frame of
/// `width` x `height`, wrapped in a reference slot. Every plane is
/// given a different pattern offset so a plane mix-up is visible.
fn ref_slot(width: usize, height: usize, salt: u8) -> Vp9RefSlot {
    let (y_w, y_h) = aligned_dims(width, height, 0, 0);
    let (c_w, c_h) = aligned_dims(width, height, 1, 1);
    let mut planes = [
        gradient_plane(y_w, y_h),
        gradient_plane(c_w, c_h),
        gradient_plane(c_w, c_h),
    ];
    for (i, plane) in planes.iter_mut().enumerate() {
        for byte in &mut plane.data {
            *byte = byte.wrapping_add(salt.wrapping_mul(i as u8 + 1));
        }
    }
    Vp9RefSlot {
        planes,
        mvs: Vec::new(),
        mi_rows: height.div_ceil(MI_SIZE),
        mi_cols: width.div_ceil(MI_SIZE),
        width,
        height,
        intra_only: false,
        show_frame: true,
    }
}

fn frame_planes(width: usize, height: usize) -> [PlaneBuf; MAX_MB_PLANE] {
    let (y_w, y_h) = aligned_dims(width, height, 0, 0);
    let (c_w, c_h) = aligned_dims(width, height, 1, 1);
    [
        blank_plane(y_w, y_h),
        blank_plane(c_w, c_h),
        blank_plane(c_w, c_h),
    ]
}

/// A single-reference inter block.
fn inter_mi(sb_type: u8, mv: MotionVector, filter: u8) -> MiInfo {
    MiInfo {
        sb_type,
        is_inter: true,
        ref_frame: [LAST_FRAME, NONE_FRAME],
        mv: [mv, MotionVector::zero()],
        interp_filter: filter,
        ..MiInfo::default()
    }
}

// -----------------------------------------------------------------
// Independent oracle
// -----------------------------------------------------------------

/// An independent model of one predicted pixel: fetch with **clamped**
/// coordinates (which is what a decoded reference frame offers — the
/// display rectangle, edge-replicated), run the horizontal 8-tap pass
/// with libvpx's 8-bit intermediate clip, then the vertical one.
///
/// This deliberately shares no code with the implementation: it indexes
/// the reference plane directly per tap instead of building an `mc_buf`,
/// and it re-derives the two-pass rounding from `vpx_convolve.c` rather
/// than calling [`super::mc`]. It is exact for *both* implementation
/// paths, because the direct path is only ever taken when clamping is a
/// no-op.
fn oracle_pixel(
    plane: &PlaneBuf,
    fw: i32,
    fh: i32,
    src_x: i32,
    src_y: i32,
    subpel_x: usize,
    subpel_y: usize,
    kernels: &InterpKernels,
) -> u8 {
    let fetch = |x: i32, y: i32| -> i32 {
        let cx = clamp_i32(x, 0, fw - 1) as usize;
        let cy = clamp_i32(y, 0, fh - 1) as usize;
        i32::from(plane.data[cy * plane.stride + cx])
    };
    let horiz = |x: i32, y: i32| -> i32 {
        if subpel_x == 0 {
            return fetch(x, y);
        }
        let k = &kernels[subpel_x];
        let mut sum = 0i32;
        for (t, &tap) in k.iter().enumerate() {
            sum += fetch(x - 3 + t as i32, y) * i32::from(tap);
        }
        // ROUND_POWER_OF_TWO(sum, FILTER_BITS) then clip_pixel: the
        // intermediate really is 8-bit in VP9.
        ((sum + 64) >> 7).clamp(0, 255)
    };
    if subpel_y == 0 {
        return horiz(src_x, src_y) as u8;
    }
    let k = &kernels[subpel_y];
    let mut sum = 0i32;
    for (t, &tap) in k.iter().enumerate() {
        sum += horiz(src_x, src_y - 3 + t as i32) * i32::from(tap);
    }
    (((sum + 64) >> 7).clamp(0, 255)) as u8
}

/// The vector as motion compensation sees it: 1/16-pel units of the
/// plane, `(row, col)`. libvpx's unscaled `scaled_mv` assignment, in
/// 32-bit arithmetic (`MV32`), which is also what the implementation
/// does — the MC-stage clamp is deliberately absent from both.
fn scaled_q4(mv: MotionVector, ss_x: usize, ss_y: usize) -> (i32, i32) {
    (
        i32::from(mv.row) * (1 << (1 - ss_y)),
        i32::from(mv.col) * (1 << (1 - ss_x)),
    )
}

/// The whole `w` x `h` prediction, per [`oracle_pixel`]. `mv_q4` is
/// `(row, col)` in this plane's 1/16-pel units, i.e. [`scaled_q4`].
fn oracle_block(
    plane: &PlaneBuf,
    fw: usize,
    fh: usize,
    block_x: usize,
    block_y: usize,
    mv_q4: (i32, i32),
    w: usize,
    h: usize,
    kernels: &InterpKernels,
) -> Vec<u8> {
    let (mv_row, mv_col) = mv_q4;
    let subpel_x = (mv_col & SUBPEL_MASK) as usize;
    let subpel_y = (mv_row & SUBPEL_MASK) as usize;
    let x0 = block_x as i32 + (mv_col >> SUBPEL_BITS);
    let y0 = block_y as i32 + (mv_row >> SUBPEL_BITS);
    let mut out = Vec::with_capacity(w * h);
    for row in 0..h as i32 {
        for col in 0..w as i32 {
            out.push(oracle_pixel(
                plane,
                fw as i32,
                fh as i32,
                x0 + col,
                y0 + row,
                subpel_x,
                subpel_y,
                kernels,
            ));
        }
    }
    out
}

/// Extracts the `w` x `h` region at `(x, y)` from a plane.
fn extract(plane: &PlaneBuf, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h);
    for row in 0..h {
        let base = (y + row) * plane.stride + x;
        out.extend_from_slice(&plane.data[base..base + w]);
    }
    out
}

// -----------------------------------------------------------------
// average_split_mvs / rounding
// -----------------------------------------------------------------

/// The whole point of transcribing `round_mv_comp_q4` literally: C's
/// `/` truncates toward zero, and so does Rust's. A floor-dividing
/// implementation would answer -2 where this answers -1.
#[test]
fn round_mv_comp_q4_truncates_toward_zero_like_c() {
    // (-5 - 2) / 4 == -7 / 4 == -1 (truncating) vs -2 (flooring).
    assert_eq!(round_mv_comp_q4(-5), -1);
    assert_eq!(round_mv_comp_q4(-7), -2);
    assert_eq!(round_mv_comp_q4(-1), -0);
    assert_eq!(round_mv_comp_q4(-2), -1);
    assert_eq!(round_mv_comp_q4(5), 1);
    assert_eq!(round_mv_comp_q4(7), 2);
    assert_eq!(round_mv_comp_q4(2), 1);
    assert_eq!(round_mv_comp_q4(0), 0);
    // Symmetry about zero is the signature of round-half-away-from-zero.
    for v in -64..=64 {
        assert_eq!(round_mv_comp_q4(-v), -round_mv_comp_q4(v), "v={v}");
    }
}

#[test]
fn round_mv_comp_q2_truncates_toward_zero_like_c() {
    assert_eq!(round_mv_comp_q2(-3), -2);
    assert_eq!(round_mv_comp_q2(-1), -1);
    assert_eq!(round_mv_comp_q2(-2), -1);
    assert_eq!(round_mv_comp_q2(3), 2);
    assert_eq!(round_mv_comp_q2(0), 0);
    for v in -64..=64 {
        assert_eq!(round_mv_comp_q2(-v), -round_mv_comp_q2(v), "v={v}");
    }
}

#[test]
fn average_split_mvs_luma_picks_the_sub_block_vector() {
    let mut mi = inter_mi(0, MotionVector::zero(), 0);
    for (i, bmv) in mi.bmv.iter_mut().enumerate() {
        bmv[0] = MotionVector::new(i as i16 * 3, i as i16 * -5);
    }
    for block in 0..4 {
        assert_eq!(
            average_split_mvs(0, 0, &mi, 0, block),
            MotionVector::new(block as i16 * 3, block as i16 * -5),
            "block {block}"
        );
    }
}

/// 4:2:0 chroma averages all four sub-block vectors, with the
/// away-from-zero rounding — including when the sum is negative and
/// not a multiple of four.
#[test]
fn average_split_mvs_chroma_420_averages_four_with_negative_rounding() {
    let mut mi = inter_mi(0, MotionVector::zero(), 0);
    mi.bmv[0][0] = MotionVector::new(-1, -3);
    mi.bmv[1][0] = MotionVector::new(-1, -1);
    mi.bmv[2][0] = MotionVector::new(-2, 0);
    mi.bmv[3][0] = MotionVector::new(-1, 1);
    // rows sum to -5 -> (-5 - 2) / 4 == -1 ; cols sum to -3 -> -1.
    assert_eq!(
        average_split_mvs(1, 1, &mi, 0, 0),
        MotionVector::new(-1, -1)
    );
    // The chroma derivation ignores `block` entirely (ss_idx == 3).
    for block in 0..4 {
        assert_eq!(
            average_split_mvs(1, 1, &mi, 0, block),
            MotionVector::new(-1, -1),
            "block {block}"
        );
    }
    // Positive counterpart, same magnitudes: rounding is symmetric.
    let mut pos = inter_mi(0, MotionVector::zero(), 0);
    pos.bmv[0][0] = MotionVector::new(1, 3);
    pos.bmv[1][0] = MotionVector::new(1, 1);
    pos.bmv[2][0] = MotionVector::new(2, 0);
    pos.bmv[3][0] = MotionVector::new(1, -1);
    assert_eq!(average_split_mvs(1, 1, &pos, 0, 0), MotionVector::new(1, 1));
}

/// The second reference has its own four sub-block vectors. Also pins
/// the units: averaging four equal vectors returns that vector
/// unchanged — `mi_mv_pred_q4` divides the *sum of four* by four, it
/// does not rescale into chroma units (the subsampling scale is applied
/// later, by the `scaled_mv` assignment).
#[test]
fn average_split_mvs_uses_the_named_reference() {
    let mut mi = inter_mi(0, MotionVector::zero(), 0);
    for bmv in &mut mi.bmv {
        bmv[0] = MotionVector::new(8, 8);
        bmv[1] = MotionVector::new(-8, -8);
    }
    assert_eq!(average_split_mvs(1, 1, &mi, 0, 0), MotionVector::new(8, 8));
    assert_eq!(
        average_split_mvs(1, 1, &mi, 1, 0),
        MotionVector::new(-8, -8)
    );
}

// -----------------------------------------------------------------
// clamp_mv_to_umv_border_sb
// -----------------------------------------------------------------

/// The subsampling scale: luma doubles (1/8-pel luma -> 1/16-pel luma),
/// 4:2:0 chroma passes through (1/8-pel luma == 1/16-pel chroma).
#[test]
fn mc_clamp_scales_by_subsampling() {
    // A block in the middle of a large frame: nothing is clamped.
    let edges = BlockEdges::new(8, 8, 12, 64, 64);
    let mv = MotionVector::new(9, -13);
    assert_eq!(
        clamp_mv_to_umv_border_sb(&edges, mv, 64, 64, 0, 0),
        MotionVector::new(18, -26)
    );
    assert_eq!(
        clamp_mv_to_umv_border_sb(&edges, mv, 32, 32, 1, 1),
        MotionVector::new(9, -13)
    );
}

/// The bound itself: a vector pointing far off the left edge is pinned
/// at `mb_to_left_edge * 2 - ((VP9_INTERP_EXTEND + bw) << SUBPEL_BITS)`.
#[test]
fn mc_clamp_pins_far_out_of_frame_vectors() {
    let (mi_row, mi_col) = (2, 3);
    let edges = BlockEdges::new(mi_row, mi_col, 12, 64, 64);
    let bw = 64;
    let clamped = clamp_mv_to_umv_border_sb(&edges, MotionVector::new(0, -16000), bw, 64, 0, 0);
    let expect = edges.to_left * 2 - ((VP9_INTERP_EXTEND + bw as i32) << SUBPEL_BITS);
    assert_eq!(i32::from(clamped.col), expect);
    // Which is exactly "one block plus the 8-tap reach" past the frame:
    // the leftmost integer sample the fetch can name.
    let x0 = ((mi_col * MI_SIZE) as i32) + (expect >> SUBPEL_BITS);
    assert_eq!(x0, -(bw as i32) - VP9_INTERP_EXTEND);
}

/// The clamp reads the **plane** block size, not the 4x4 sub-block a
/// sub-8x8 prediction writes (libvpx passes `n4w_x4`/`n4h_x4` for every
/// sub-block of a sub-8x8 partition). The two disagree at a frame edge,
/// so P15's scaled path must not substitute one for the other.
#[test]
fn mc_clamp_uses_the_plane_block_size_not_the_sub_block_size() {
    let edges = BlockEdges::new(0, 0, super::BLOCK_8X8 - 1, 8, 8);
    let far_left = MotionVector::new(0, -4000);
    let with_block = clamp_mv_to_umv_border_sb(&edges, far_left, 8, 8, 0, 0);
    let with_sub_block = clamp_mv_to_umv_border_sb(&edges, far_left, 4, 4, 0, 0);
    assert_ne!(
        with_block, with_sub_block,
        "bw=8 (plane block) and bw=4 (sub-block) must not agree, \
         or this test cannot catch the confusion"
    );
    assert_eq!(
        i32::from(with_block.col),
        -((VP9_INTERP_EXTEND + 8) << SUBPEL_BITS)
    );
}

// -----------------------------------------------------------------
// build_mc_border
// -----------------------------------------------------------------

/// Every output pixel is the display-rectangle pixel at the clamped
/// coordinate — corners included.
#[test]
fn mc_border_replicates_the_display_edges() {
    // A 10x6 allocation whose display rectangle is only 7x4: the two
    // right columns and two bottom rows hold values the border build
    // must never read.
    let mut plane = gradient_plane(10, 6);
    for y in 0..6 {
        for x in 0..10 {
            if x >= 7 || y >= 4 {
                plane.data[y * 10 + x] = 200;
            }
        }
    }
    let (b_w, b_h) = (12usize, 9usize);
    let (x, y) = (-3i32, -2i32);
    let mut out = vec![0u8; b_w * b_h];
    build_mc_border(&plane, &mut out, b_w, x, y, b_w, b_h, 7, 4);
    for i in 0..b_h {
        for j in 0..b_w {
            let sx = clamp_i32(x + j as i32, 0, 6) as usize;
            let sy = clamp_i32(y + i as i32, 0, 3) as usize;
            assert_eq!(
                out[i * b_w + j],
                plane.data[sy * plane.stride + sx],
                "({j}, {i})"
            );
            assert_ne!(out[i * b_w + j], 200, "read outside the display rectangle");
        }
    }
}

/// A window entirely off the left edge collapses to the first column.
#[test]
fn mc_border_window_fully_outside_replicates_one_pixel_column() {
    let plane = gradient_plane(8, 8);
    let (b_w, b_h) = (5usize, 3usize);
    let mut out = vec![0u8; b_w * b_h];
    build_mc_border(&plane, &mut out, b_w, -20, 2, b_w, b_h, 8, 8);
    for i in 0..b_h {
        let expect = plane.data[(2 + i) * plane.stride];
        assert!(out[i * b_w..(i + 1) * b_w].iter().all(|&v| v == expect));
    }
}

// -----------------------------------------------------------------
// Whole-block prediction
// -----------------------------------------------------------------

/// ZEROMV must reproduce the co-located reference pixels exactly, in
/// every plane — the single strongest smoke test for the coordinate
/// derivation. Run on 8-multiple dimensions (libvpx's fast path, where
/// the border block is skipped outright).
#[test]
fn zeromv_reproduces_the_colocated_reference_exactly() {
    let (width, height) = (64usize, 32usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 37);
    let mut planes = frame_planes(width, height);
    let mi = inter_mi(12, MotionVector::zero(), 0); // BLOCK_64X64

    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, 0, 0)
        .expect("zero-MV prediction from an equally sized reference");

    for plane in 0..MAX_MB_PLANE {
        let (ss_x, ss_y) = geom.plane_ss(plane);
        let (fw, fh) = plane_display_dims(width, height, ss_x, ss_y);
        assert_eq!(
            extract(&planes[plane], 0, 0, fw, fh),
            extract(&slot.planes[plane], 0, 0, fw, fh),
            "plane {plane}"
        );
    }
}

/// The same, on dimensions that are *not* multiples of 8, where libvpx
/// routes even a zero vector through the mc_buf path. The copy must
/// still be exact over the display rectangle.
#[test]
fn zeromv_is_exact_on_non_multiple_of_8_dimensions() {
    let (width, height) = (76usize, 42usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 11);
    let mut planes = frame_planes(width, height);
    let mi = inter_mi(12, MotionVector::zero(), 2);

    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, 0, 0)
        .expect("zero-MV prediction");

    // Chroma display dims round up: 38x21, not 38x21-rounded-down.
    assert_eq!(plane_display_dims(width, height, 1, 1), (38, 21));
    for plane in 0..MAX_MB_PLANE {
        let (ss_x, ss_y) = geom.plane_ss(plane);
        let (fw, fh) = plane_display_dims(width, height, ss_x, ss_y);
        let block = if plane == 0 { 64 } else { 32 };
        let w = fw.min(block);
        let h = fh.min(block);
        assert_eq!(
            extract(&planes[plane], 0, 0, w, h),
            extract(&slot.planes[plane], 0, 0, w, h),
            "plane {plane}"
        );
    }
}

/// A large vector pointing well outside the frame: the prediction must
/// equal the edge-replicated, filtered fetch computed independently.
/// This is the mc_buf path plus [`clamp_mv_to_umv_border_sb`]'s bound.
#[test]
fn out_of_frame_newmv_matches_the_clamped_border_prediction() {
    let (width, height) = (48usize, 48usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 3);
    let mut planes = frame_planes(width, height);
    // BLOCK_16X16 == 6, at the bottom-right MI corner, pointing far
    // down-right and off the frame with a sub-pixel phase.
    let (mi_row, mi_col) = (4usize, 4usize);
    let mv = MotionVector::new(1234, 987);
    let mi = inter_mi(6, mv, 0);

    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, mi_row, mi_col)
        .expect("out-of-frame prediction");

    for plane in 0..MAX_MB_PLANE {
        let (ss_x, ss_y) = geom.plane_ss(plane);
        let (fw, fh) = plane_display_dims(width, height, ss_x, ss_y);
        let bw = 16 >> ss_x;
        let bh = 16 >> ss_y;
        let mv_q4 = scaled_q4(mv, ss_x, ss_y);
        let x0 = (mi_col * MI_SIZE) >> ss_x;
        let y0 = (mi_row * MI_SIZE) >> ss_y;
        let expect = oracle_block(
            &slot.planes[plane],
            fw,
            fh,
            x0,
            y0,
            mv_q4,
            bw,
            bh,
            &FILTER_KERNELS[0],
        );
        assert_eq!(
            extract(&planes[plane], x0, y0, bw, bh),
            expect,
            "plane {plane}"
        );
    }
}

/// Every filter family, at a half-pel phase on both axes, against the
/// independent oracle. A family mix-up (`EIGHTTAP_SMOOTH` is
/// `sub_pel_filters_8lp`, `EIGHTTAP_SHARP` is `8s`) would show here.
#[test]
fn every_filter_family_at_half_pel_matches_the_oracle() {
    let (width, height) = (64usize, 64usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 91);
    // 4 eighth-pels = half a luma pixel on both axes: phase 8 of 16 in
    // luma (`4 * 2`), phase 4 in a 4:2:0 chroma plane (`4 * 1`, a
    // quarter of a chroma pixel). Both are sub-pixel, so every plane
    // exercises the full 2-D convolution.
    let mv = MotionVector::new(4, 4);
    let (mi_row, mi_col) = (2usize, 2usize);
    let mut seen = Vec::new();
    for filter in 0..FILTER_KERNELS.len() as u8 {
        let mut planes = frame_planes(width, height);
        let mi = inter_mi(9, mv, filter); // BLOCK_32X32
        dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, mi_row, mi_col)
            .expect("half-pel prediction");

        for plane in 0..MAX_MB_PLANE {
            let (ss_x, ss_y) = geom.plane_ss(plane);
            let (fw, fh) = plane_display_dims(width, height, ss_x, ss_y);
            let bw = 32 >> ss_x;
            let bh = 32 >> ss_y;
            let mv_q4 = scaled_q4(mv, ss_x, ss_y);
            assert_eq!(mv_q4.0 & SUBPEL_MASK, if plane == 0 { 8 } else { 4 });
            assert_eq!(mv_q4.1 & SUBPEL_MASK, if plane == 0 { 8 } else { 4 });
            let x0 = (mi_col * MI_SIZE) >> ss_x;
            let y0 = (mi_row * MI_SIZE) >> ss_y;
            let got = extract(&planes[plane], x0, y0, bw, bh);
            assert_eq!(
                got,
                oracle_block(
                    &slot.planes[plane],
                    fw,
                    fh,
                    x0,
                    y0,
                    mv_q4,
                    bw,
                    bh,
                    &FILTER_KERNELS[filter as usize]
                ),
                "filter {filter}, plane {plane}"
            );
            if plane == 0 {
                seen.push(got);
            }
        }
    }
    // The four families must actually differ, or the check above is
    // vacuous.
    for i in 1..seen.len() {
        assert_ne!(seen[0], seen[i], "filter family {i} equals family 0");
    }
}

/// The direct path and the mc_buf path have to agree wherever both are
/// legal: a well-inside block with a sub-pixel vector (direct) against
/// the oracle, which models the clamped fetch.
#[test]
fn interior_subpel_block_takes_the_direct_path_and_still_matches() {
    let (width, height) = (64usize, 64usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 5);
    let mut planes = frame_planes(width, height);
    let (mi_row, mi_col) = (2usize, 2usize);
    let mv = MotionVector::new(-3, 5);
    let mi = inter_mi(6, mv, 1); // BLOCK_16X16, EIGHTTAP_SMOOTH
    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, mi_row, mi_col)
        .expect("interior prediction");

    let mv_q4 = scaled_q4(mv, 0, 0);
    let (x0, y0) = (mi_col * MI_SIZE, mi_row * MI_SIZE);
    assert_eq!(
        extract(&planes[0], x0, y0, 16, 16),
        oracle_block(
            &slot.planes[0],
            width,
            height,
            x0,
            y0,
            mv_q4,
            16,
            16,
            &FILTER_KERNELS[1]
        )
    );
}

/// Compound prediction: the second reference's pass must be the rounded
/// average with the first's output, computed in place.
#[test]
fn compound_prediction_averages_the_two_references() {
    let (width, height) = (64usize, 64usize);
    let geom = geometry(width, height);
    let slot0 = ref_slot(width, height, 17);
    let slot1 = ref_slot(width, height, 200);
    let mut planes = frame_planes(width, height);
    let (mi_row, mi_col) = (1usize, 1usize);
    let mv0 = MotionVector::new(4, -6);
    let mv1 = MotionVector::new(-10, 12);
    let mut mi = inter_mi(6, mv0, 0); // BLOCK_16X16
    mi.ref_frame = [LAST_FRAME, 3];
    mi.mv = [mv0, mv1];

    dec_build_inter_predictors_sb(
        &mut planes,
        &mi,
        [Some(&slot0), Some(&slot1)],
        &geom,
        mi_row,
        mi_col,
    )
    .expect("compound prediction");

    for plane in 0..MAX_MB_PLANE {
        let (ss_x, ss_y) = geom.plane_ss(plane);
        let (fw, fh) = plane_display_dims(width, height, ss_x, ss_y);
        let bw = 16 >> ss_x;
        let bh = 16 >> ss_y;
        let x0 = (mi_col * MI_SIZE) >> ss_x;
        let y0 = (mi_row * MI_SIZE) >> ss_y;
        let a = oracle_block(
            &slot0.planes[plane],
            fw,
            fh,
            x0,
            y0,
            scaled_q4(mv0, ss_x, ss_y),
            bw,
            bh,
            &FILTER_KERNELS[0],
        );
        let b = oracle_block(
            &slot1.planes[plane],
            fw,
            fh,
            x0,
            y0,
            scaled_q4(mv1, ss_x, ss_y),
            bw,
            bh,
            &FILTER_KERNELS[0],
        );
        let expect: Vec<u8> = a
            .iter()
            .zip(&b)
            .map(|(&p, &q)| ((i32::from(p) + i32::from(q) + 1) >> 1) as u8)
            .collect();
        assert_eq!(
            extract(&planes[plane], x0, y0, bw, bh),
            expect,
            "plane {plane}"
        );
        // ... and the two references really do differ, so the average
        // is not trivially either of them.
        assert_ne!(a, b, "plane {plane}: references coincide");
    }
}

/// Sub-8x8: luma predicts four 4x4 blocks from four distinct vectors,
/// chroma one 4x4 block from their average.
#[test]
fn sub_8x8_predicts_four_luma_sub_blocks_and_one_chroma_block() {
    let (width, height) = (32usize, 32usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 61);
    let mut planes = frame_planes(width, height);
    let (mi_row, mi_col) = (1usize, 1usize);
    let mut mi = inter_mi(0, MotionVector::zero(), 0); // BLOCK_4X4
    let bmvs = [
        MotionVector::new(2, 3),
        MotionVector::new(-4, 5),
        MotionVector::new(6, -7),
        MotionVector::new(-1, -2),
    ];
    for (i, bmv) in mi.bmv.iter_mut().enumerate() {
        bmv[0] = bmvs[i];
    }

    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, mi_row, mi_col)
        .expect("sub-8x8 prediction");

    // Luma: each 4x4 sub-block uses its own vector, in raster order.
    for (i, &bmv) in bmvs.iter().enumerate() {
        let (bx, by) = (i % 2, i / 2);
        let x0 = mi_col * MI_SIZE + 4 * bx;
        let y0 = mi_row * MI_SIZE + 4 * by;
        let mv_q4 = scaled_q4(bmv, 0, 0);
        assert_eq!(
            extract(&planes[0], x0, y0, 4, 4),
            oracle_block(
                &slot.planes[0],
                width,
                height,
                x0,
                y0,
                mv_q4,
                4,
                4,
                &FILTER_KERNELS[0]
            ),
            "luma sub-block {i}"
        );
    }
    // Chroma: one 4x4 block from the q4 average of all four.
    let avg_mv = average_split_mvs(1, 1, &mi, 0, 0);
    let mv_q4 = scaled_q4(avg_mv, 1, 1);
    let (fw, fh) = plane_display_dims(width, height, 1, 1);
    let x0 = (mi_col * MI_SIZE) >> 1;
    let y0 = (mi_row * MI_SIZE) >> 1;
    for plane in 1..MAX_MB_PLANE {
        assert_eq!(
            extract(&planes[plane], x0, y0, 4, 4),
            oracle_block(
                &slot.planes[plane],
                fw,
                fh,
                x0,
                y0,
                mv_q4,
                4,
                4,
                &FILTER_KERNELS[0]
            ),
            "chroma plane {plane}"
        );
    }
}

/// The claim the module docs make about the missing MC-stage clamp,
/// tested rather than argued: for a vector far enough outside the frame
/// that [`clamp_mv_to_umv_border_sb`] would pin it, the prediction this
/// module builds (unclamped, like libvpx's decoder) is identical to the
/// one the clamped vector produces (like libvpx's encoder-side
/// reconstruction) — in every plane and at a sub-pixel phase the clamp
/// discards.
#[test]
fn far_out_of_frame_vector_is_unaffected_by_the_mc_clamp() {
    let (width, height) = (48usize, 48usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 23);
    let (mi_row, mi_col) = (0usize, 0usize);
    // Legal (|component| < 16384) but far outside: up and to the left
    // by 500 pixels, with a non-zero sub-pixel phase the clamp drops.
    let far = MotionVector::new(-4003, -4005);
    let mi = inter_mi(9, far, 0); // BLOCK_32X32

    let mut planes = frame_planes(width, height);
    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, mi_row, mi_col)
        .expect("far out-of-frame prediction");

    let edges = BlockEdges::new(mi_row, mi_col, 9, geom.mi_rows, geom.mi_cols);
    for plane in 0..MAX_MB_PLANE {
        let (ss_x, ss_y) = geom.plane_ss(plane);
        let (fw, fh) = plane_display_dims(width, height, ss_x, ss_y);
        let bw = 32 >> ss_x;
        let bh = 32 >> ss_y;
        let unclamped = scaled_q4(far, ss_x, ss_y);
        let clamped = clamp_mv_to_umv_border_sb(&edges, far, bw, bh, ss_x, ss_y);
        let clamped_q4 = (i32::from(clamped.row), i32::from(clamped.col));
        assert_ne!(
            unclamped, clamped_q4,
            "plane {plane}: the clamp must actually engage here"
        );
        assert_ne!(unclamped.1 & SUBPEL_MASK, 0, "sub-pixel phase expected");
        assert_eq!(
            clamped_q4.1 & SUBPEL_MASK,
            0,
            "the clamp lands on a whole pixel"
        );
        let got = extract(&planes[plane], 0, 0, bw, bh);
        for mv_q4 in [unclamped, clamped_q4] {
            assert_eq!(
                got,
                oracle_block(
                    &slot.planes[plane],
                    fw,
                    fh,
                    0,
                    0,
                    mv_q4,
                    bw,
                    bh,
                    &FILTER_KERNELS[0]
                ),
                "plane {plane}, mv_q4 {mv_q4:?}"
            );
        }
        // And it really is the replicated top-left corner.
        assert!(got.iter().all(|&v| v == slot.planes[plane].data[0]));
    }
}

/// A block overhanging the MI grid (`PARTITION_VERT` at the last MI
/// column of a 8-multiple-wide frame) with a zero vector: the live
/// pixels must still be the co-located reference, and nothing may
/// panic on the dead ones.
#[test]
fn overhanging_block_with_zero_mv_predicts_its_live_pixels() {
    // 72 wide -> 9 MI columns, and 72 % 8 == 0, so libvpx would skip
    // its border check entirely here.
    let (width, height) = (72usize, 32usize);
    let geom = geometry(width, height);
    assert_eq!(width % 8, 0);
    let slot = ref_slot(width, height, 44);
    let mut planes = frame_planes(width, height);
    // BLOCK_32X64 == 10 at mi_col 8: 32 pixels wide over one remaining
    // MI column, overhanging the plane by 24 pixels.
    let mi = inter_mi(10, MotionVector::zero(), 0);
    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, 0, 8)
        .expect("overhanging block");

    let live_w = planes[0].width - 64;
    assert_eq!(live_w, 8);
    assert_eq!(
        extract(&planes[0], 64, 0, live_w, 32),
        extract(&slot.planes[0], 64, 0, live_w, 32)
    );
}

/// Odd frame dimensions: chroma planes are `div_ceil` of the luma
/// display size, and the replicated column is the last *chroma display*
/// column, not the last allocated one.
#[test]
fn odd_frame_dimensions_use_div_ceil_chroma_display_dims() {
    let (width, height) = (33usize, 17usize);
    let geom = geometry(width, height);
    assert_eq!(plane_display_dims(width, height, 1, 1), (17, 9));
    let (c_w, c_h) = aligned_dims(width, height, 1, 1);
    assert_eq!((c_w, c_h), (20, 12));

    let mut slot = ref_slot(width, height, 7);
    // Poison the chroma allocation outside the display rectangle: a
    // correct fetch never reads it.
    for plane in 1..MAX_MB_PLANE {
        for y in 0..c_h {
            for x in 0..c_w {
                if x >= 17 || y >= 9 {
                    slot.planes[plane].data[y * c_w + x] = 0xEE;
                }
            }
        }
    }
    let mut planes = frame_planes(width, height);
    // BLOCK_16X16 at the bottom-right MI corner, pointing outside.
    let (mi_row, mi_col) = (2usize, 4usize);
    let mv = MotionVector::new(400, 400);
    let mi = inter_mi(6, mv, 0);
    dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, mi_row, mi_col)
        .expect("odd-dimension prediction");

    for plane in 1..MAX_MB_PLANE {
        let mv_q4 = scaled_q4(mv, 1, 1);
        let x0 = (mi_col * MI_SIZE) >> 1;
        let y0 = (mi_row * MI_SIZE) >> 1;
        let w = planes[plane].width - x0;
        let h = planes[plane].height - y0;
        let got = extract(&planes[plane], x0, y0, w.min(8), h.min(8));
        let expect = oracle_block(
            &slot.planes[plane],
            17,
            9,
            x0,
            y0,
            mv_q4,
            w.min(8),
            h.min(8),
            &FILTER_KERNELS[0],
        );
        assert_eq!(got, expect, "plane {plane}");
        assert!(
            !got.contains(&0xEE),
            "plane {plane}: fetched outside the chroma display rectangle"
        );
    }
}

// -----------------------------------------------------------------
// Honest refusals
// -----------------------------------------------------------------

#[test]
fn reference_scaling_is_refused_honestly() {
    let (width, height) = (64usize, 64usize);
    let geom = geometry(width, height);
    let slot = ref_slot(32, 32, 1);
    let mut planes = frame_planes(width, height);
    let mi = inter_mi(12, MotionVector::zero(), 0);
    let err = dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, 0, 0)
        .expect_err("a differently sized reference must be refused");
    match err {
        CodecError::UnsupportedFeature(msg) => {
            assert!(msg.contains("reference scaling"), "{msg}");
            assert!(msg.contains("32x32"), "{msg}");
            assert!(msg.contains("64x64"), "{msg}");
        }
        other => panic!("expected UnsupportedFeature, got {other:?}"),
    }
    // Nothing was written before the refusal.
    assert!(planes[0].data.iter().all(|&v| v == 128));
}

/// The *second* reference of a compound block is checked too.
#[test]
fn compound_second_reference_scaling_is_refused() {
    let (width, height) = (64usize, 64usize);
    let geom = geometry(width, height);
    let ok = ref_slot(width, height, 1);
    let scaled = ref_slot(32, 32, 1);
    let mut planes = frame_planes(width, height);
    let mut mi = inter_mi(12, MotionVector::zero(), 0);
    mi.ref_frame = [LAST_FRAME, 3];
    let err =
        dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&ok), Some(&scaled)], &geom, 0, 0)
            .expect_err("scaled second reference must be refused");
    assert!(matches!(err, CodecError::UnsupportedFeature(_)));
}

#[test]
fn empty_reference_slot_is_refused_honestly() {
    let (width, height) = (32usize, 32usize);
    let geom = geometry(width, height);
    let mut planes = frame_planes(width, height);
    let mi = inter_mi(9, MotionVector::zero(), 0);
    let err = dec_build_inter_predictors_sb(&mut planes, &mi, [None, None], &geom, 0, 0)
        .expect_err("an empty DPB slot must be refused");
    assert!(matches!(err, CodecError::InvalidBitstream(_)), "{err:?}");
}

#[test]
fn unresolved_switchable_filter_is_refused_honestly() {
    let (width, height) = (32usize, 32usize);
    let geom = geometry(width, height);
    let slot = ref_slot(width, height, 1);
    let mut planes = frame_planes(width, height);
    let mi = inter_mi(9, MotionVector::zero(), 4); // SWITCHABLE
    let err = dec_build_inter_predictors_sb(&mut planes, &mi, [Some(&slot), None], &geom, 0, 0)
        .expect_err("SWITCHABLE must never reach motion compensation");
    match err {
        CodecError::InvalidBitstream(msg) => assert!(msg.contains("interp_filter=4"), "{msg}"),
        other => panic!("expected InvalidBitstream, got {other:?}"),
    }
}
