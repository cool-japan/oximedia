//! SIMD glue between the AV1 intra decode path and `oximedia-simd`.
//!
//! `oximedia-codec` denies `unsafe_code` at the crate level, so every
//! architecture intrinsic lives in `oximedia-simd`.  This module owns the
//! *addressing* — gathering plane samples into the layout the kernels expect
//! and scattering the results back — and leaves the arithmetic to
//! [`oximedia_simd::av1_loopfilter`].
//!
//! Intra prediction is deliberately **not** routed through hand-written
//! vector kernels.  The spec's predictor inner loops are elementwise over
//! contiguous `i32` and LLVM already auto-vectorises them to NEON; a
//! hand-written four-lane version measured slower at every block width AV1
//! actually uses (see the report accompanying this change).  What `pred.rs`
//! does instead is structural: replace per-sample clamps and branches with
//! contiguous copies and fills so the compiler's vectoriser has a clean loop
//! to work with.

use oximedia_simd::av1_loopfilter::{filter_edge4, Av1Edge4, Av1FilterSize, Av1LfParams};

use super::recon::PlaneBuf;

/// Applies the AV1 deblocking filter to the four sample lines crossing one
/// edge (spec 7.14.6, invoked once per `i in 0..4` by the edge loop filter
/// process).
///
/// `(x, y)` is the position of `q0` on the **first** of the four lines, and
/// `(dx, dy)` is the sample direction: `(1, 0)` for a vertical edge (the four
/// lines are then rows `y..y+4`) and `(0, 1)` for a horizontal edge (the four
/// lines are columns `x..x+4`).
///
/// Only the samples the spec's sample-filtering process touches are read and
/// written, so this performs exactly the accesses the per-line scalar code
/// would have performed.
#[allow(clippy::too_many_arguments)]
pub fn filter_edge_4lines(
    p: &mut PlaneBuf,
    x: usize,
    y: usize,
    dx: usize,
    dy: usize,
    filter_size: usize,
    plane: usize,
    limit: i32,
    blimit: i32,
    thresh: i32,
) {
    let params = Av1LfParams {
        limit,
        blimit,
        thresh,
        size: match filter_size {
            4 => Av1FilterSize::Size4,
            8 => Av1FilterSize::Size8,
            _ => Av1FilterSize::Size16,
        },
        chroma: plane != 0,
    };
    let span = params.span();
    let (lo, hi) = (span.start, span.end);
    let n = hi - lo;
    let stride = p.stride;
    let mut edge: Av1Edge4 = [[0u8; 4]; 14];

    if dy == 1 {
        // Horizontal edge: sample `k` of all four lines are the four adjacent
        // bytes at column `x` of row `y + k - 7`, so no transpose is needed.
        for k in lo..hi {
            let off = (y + k - 7) * stride + x;
            edge[k].copy_from_slice(&p.data[off..off + 4]);
        }
        filter_edge4(&mut edge, &params);
        for k in lo..hi {
            let off = (y + k - 7) * stride + x;
            p.data[off..off + 4].copy_from_slice(&edge[k]);
        }
    } else {
        // Vertical edge: each line is one row of `n` contiguous samples, so
        // the gather is a 4 x n transpose into the sample-major layout.
        let base_col = x + lo - 7;
        for r in 0..4 {
            let off = (y + r) * stride + base_col;
            for (i, &b) in p.data[off..off + n].iter().enumerate() {
                edge[lo + i][r] = b;
            }
        }
        filter_edge4(&mut edge, &params);
        for r in 0..4 {
            let off = (y + r) * stride + base_col;
            for (i, b) in p.data[off..off + n].iter_mut().enumerate() {
                *b = edge[lo + i][r];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! The equivalence tests in `oximedia_simd::av1_loopfilter` prove the
    //! kernel correct *given* a correctly populated edge.  These tests cover
    //! the other half — the addressing in [`filter_edge_4lines`] — by
    //! comparing it against an independent transcription of the spec's
    //! per-line access pattern (`at(x + pos*dx, y + pos*dy)`), for every
    //! `(pass, filterSize, plane)` combination the decoder can produce.
    //!
    //! This matters because the AV1 fixtures do not reach all of them: a
    //! probe over every deblocking fixture in `testdata/` showed
    //! `filterSize == 16` occurring only 5 times, all on horizontal edges,
    //! and never on the vertical-edge path — so the widest span (14 samples,
    //! `base_col = x - 7`, 4x14 transpose) would otherwise be exercised by no
    //! test at all.

    use super::filter_edge_4lines;
    use crate::av1::kf::recon::PlaneBuf;
    use oximedia_simd::av1_loopfilter::{filter_edge4_reference, Av1FilterSize, Av1LfParams};

    const DIM: usize = 64;

    fn params_of(filter_size: usize, plane: usize, l: i32, b: i32, t: i32) -> Av1LfParams {
        Av1LfParams {
            limit: l,
            blimit: b,
            thresh: t,
            size: match filter_size {
                4 => Av1FilterSize::Size4,
                8 => Av1FilterSize::Size8,
                _ => Av1FilterSize::Size16,
            },
            chroma: plane != 0,
        }
    }

    /// Plane content patterns chosen to reach every filter branch.
    fn make_plane(kind: usize, dx: usize, dy: usize, ex: usize, ey: usize) -> PlaneBuf {
        let mut data = vec![0u8; DIM * DIM];
        for r in 0..DIM {
            for c in 0..DIM {
                // Distance from the edge along the filter direction.
                let along = if dx == 1 {
                    c as i64 - ex as i64
                } else {
                    r as i64 - ey as i64
                };
                data[r * DIM + c] = match kind {
                    // High-frequency: the filter mask rejects most lines.
                    0 => ((r * 7 + c * 13) % 256) as u8,
                    // Flat either side of a step: reaches the wide filters.
                    1 => {
                        let base: i64 = if along < 0 { 100 } else { 118 };
                        (base + i64::from((r as u32 ^ c as u32) & 1)) as u8
                    }
                    // Flat near the edge, noisy further out: flat && !flat2.
                    2 => {
                        if along.abs() <= 3 {
                            if along < 0 {
                                100
                            } else {
                                116
                            }
                        } else {
                            ((r * 31 + c * 17) % 256) as u8
                        }
                    }
                    // Gentle ramp: passes the mask, no hev.
                    _ => ((r + c) % 64 + 96) as u8,
                };
            }
        }
        PlaneBuf {
            data,
            stride: DIM,
            width: DIM,
            height: DIM,
        }
    }

    /// Independent transcription of the spec's per-line addressing: line `i`
    /// starts at `(x + dy*i, y + dx*i)` and its sample at offset `k-7` sits
    /// at `(sx + dx*(k-7), sy + dy*(k-7))`.  Derived from spec 7.14.6.1, not
    /// from `filter_edge_4lines`.
    #[allow(clippy::too_many_arguments)]
    fn reference_edge_4lines(
        p: &mut PlaneBuf,
        x: usize,
        y: usize,
        dx: usize,
        dy: usize,
        filter_size: usize,
        plane: usize,
        limit: i32,
        blimit: i32,
        thresh: i32,
    ) {
        let params = params_of(filter_size, plane, limit, blimit, thresh);
        let stride = p.stride;
        for i in 0..4usize {
            let sx = (x + dy * i) as isize;
            let sy = (y + dx * i) as isize;
            let mut line = [[0u8; 4]; 14];
            let at = |k: usize| -> usize {
                let off = k as isize - 7;
                let px = sx + dx as isize * off;
                let py = sy + dy as isize * off;
                py as usize * stride + px as usize
            };
            for k in params.span() {
                line[k] = [p.data[at(k)]; 4];
            }
            filter_edge4_reference(&mut line, &params);
            for k in params.span() {
                p.data[at(k)] = line[k][0];
            }
        }
    }

    #[test]
    fn edge_addressing_matches_per_line_spec_reference() {
        // filterSize 16 is luma-only: the filter size process caps chroma at
        // min(8, baseSize).
        let configs = [(4usize, 0usize), (8, 0), (16, 0), (4, 1), (8, 1)];
        let mut changed = std::collections::BTreeMap::new();
        let mut cases = 0u32;

        for &(fs, plane) in &configs {
            for pass in 0..2usize {
                let (dx, dy) = if pass == 0 { (1usize, 0usize) } else { (0, 1) };
                for &(limit, blimit, thresh) in
                    &[(1i32, 5i32, 0i32), (9, 90, 2), (2, 40, 0), (63, 260, 15)]
                {
                    for &(x, y) in &[(16usize, 16usize), (32, 32), (16, 32), (32, 16)] {
                        for kind in 0..4usize {
                            let mut got = make_plane(kind, dx, dy, x, y);
                            let mut want = make_plane(kind, dx, dy, x, y);
                            let orig = got.data.clone();
                            filter_edge_4lines(
                                &mut got, x, y, dx, dy, fs, plane, limit, blimit, thresh,
                            );
                            reference_edge_4lines(
                                &mut want, x, y, dx, dy, fs, plane, limit, blimit, thresh,
                            );
                            assert_eq!(
                                got.data, want.data,
                                "addressing mismatch: fs={fs} plane={plane} pass={pass} \
                                 at ({x},{y}) limit={limit} blimit={blimit} thresh={thresh} \
                                 content={kind}"
                            );
                            if got.data != orig {
                                *changed.entry((fs, plane, pass)).or_insert(0u32) += 1;
                            }
                            cases += 1;
                        }
                    }
                }
            }
        }

        // A pass where the filter never fired would compare two untouched
        // planes and prove nothing about the addressing.
        for &(fs, plane) in &configs {
            for pass in 0..2usize {
                assert!(
                    changed.get(&(fs, plane, pass)).copied().unwrap_or(0) > 0,
                    "filter never modified the plane for fs={fs} plane={plane} pass={pass}; \
                     the comparison would be vacuous ({changed:?})"
                );
            }
        }
        assert_eq!(cases, 5 * 2 * 4 * 4 * 4);
    }
}

#[cfg(test)]
mod perf {
    //! Rough decode-timing harness (run with `--ignored --nocapture`).

    use crate::av1::kf::{decode_temporal_unit, TuOutcome};
    use std::time::Instant;

    fn time_fixture(label: &str, tu: &[u8], iters: u32) {
        // Warm-up.
        for _ in 0..3 {
            let mut seq = None;
            let _ = decode_temporal_unit(tu, &mut seq);
        }
        let mut best = f64::MAX;
        let mut total = 0.0f64;
        for _ in 0..iters {
            let mut seq = None;
            let t0 = Instant::now();
            let outcome = decode_temporal_unit(tu, &mut seq);
            let dt = t0.elapsed().as_secs_f64() * 1e3;
            assert!(matches!(outcome, Ok(TuOutcome::Frame(..))), "{label}");
            total += dt;
            if dt < best {
                best = dt;
            }
        }
        let mean = total / f64::from(iters);
        println!("{label}: best {best:.3} ms, mean {mean:.3} ms over {iters} iters");
    }

    #[test]
    #[ignore = "timing harness; run explicitly with --ignored --nocapture"]
    fn av1_decode_timing() {
        time_fixture(
            "s5_lr320_q30 320x192",
            include_bytes!("testdata/s5_lr320_q30.obu"),
            60,
        );
        time_fixture(
            "s5_blur_q20  320x192",
            include_bytes!("testdata/s5_blur_q20.obu"),
            60,
        );
        time_fixture(
            "s1_svt256tc  256x128",
            include_bytes!("testdata/s1_svt256tc.obu"),
            60,
        );
        time_fixture(
            "s2_aom128    128x128",
            include_bytes!("testdata/s2_aom128.obu"),
            200,
        );
        time_fixture(
            "s3_aom128    128x128",
            include_bytes!("testdata/s3_aom128.obu"),
            200,
        );
        time_fixture(
            "s4_aom128    128x128",
            include_bytes!("testdata/s4_aom128.obu"),
            200,
        );
        time_fixture(
            "s1_svt128    128x128",
            include_bytes!("testdata/s1_svt128.obu"),
            200,
        );
        time_fixture(
            "s2_aom76x42   76x42 ",
            include_bytes!("testdata/s2_aom76x42.obu"),
            400,
        );
        time_fixture(
            "s1_ll64        64x64",
            include_bytes!("testdata/s1_ll64.obu"),
            400,
        );
    }

    /// Isolated timing of the two kernels this module accelerates, so the
    /// numbers are not confounded by the rest of the decoder.
    #[test]
    #[ignore = "timing harness; run explicitly with --ignored --nocapture"]
    fn av1_kernel_timing() {
        use crate::av1::kf::consts::{DC_PRED, PAETH_PRED, SMOOTH_PRED, V_PRED};
        use crate::av1::kf::pred::{predict_intra, PredParams};
        use crate::av1::kf::recon::PlaneBuf;

        const S: usize = 512;
        let mut buf = vec![0u8; S * S];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((i * 7 + i / S * 13) % 256) as u8;
        }

        // ── Intra prediction ────────────────────────────────────────────
        let modes = [DC_PRED, V_PRED, PAETH_PRED, SMOOTH_PRED, V_PRED + 3];
        let mut sink = 0u64;
        let t0 = Instant::now();
        let iters = 200;
        for _ in 0..iters {
            for &mode in &modes {
                for log2 in 2u32..=5 {
                    let p = PredParams {
                        have_left: true,
                        have_above: true,
                        have_above_right: false,
                        have_below_left: false,
                        mode,
                        log2w: log2,
                        log2h: log2,
                        angle_delta: 0,
                        filter_intra_mode: None,
                        enable_intra_edge_filter: true,
                        filter_type: false,
                        max_x: S - 1,
                        max_y: S - 1,
                    };
                    let mut yy = 64;
                    while yy + 64 < S {
                        let mut xx = 64;
                        while xx + 64 < S {
                            predict_intra(&mut buf, S, xx, yy, &p);
                            xx += 1 << log2;
                        }
                        yy += 1 << log2;
                    }
                }
            }
            sink += u64::from(buf[S * 100 + 100]);
        }
        let dt = t0.elapsed().as_secs_f64() * 1e3 / f64::from(iters);
        println!("predict_intra sweep: {dt:.3} ms/iter (sink {sink})");

        // ── Loop filter ─────────────────────────────────────────────────
        let mut plane = PlaneBuf {
            data: buf.clone(),
            stride: S,
            width: S,
            height: S,
        };
        for &(fs, plane_idx, label) in &[
            (4usize, 0usize, "fs=4  luma"),
            (8, 0, "fs=8  luma"),
            (8, 1, "fs=8  chroma"),
            (16, 0, "fs=16 luma"),
        ] {
            let t0 = Instant::now();
            let iters = 300;
            for _ in 0..iters {
                for pass in 0..2usize {
                    let (dx, dy) = if pass == 0 { (1usize, 0usize) } else { (0, 1) };
                    let mut yy = 32;
                    while yy + 32 < S {
                        let mut xx = 32;
                        while xx + 32 < S {
                            super::super::simd::filter_edge_4lines(
                                &mut plane, xx, yy, dx, dy, fs, plane_idx, 9, 90, 2,
                            );
                            xx += 4;
                        }
                        yy += 4;
                    }
                }
            }
            let dt = t0.elapsed().as_secs_f64() * 1e3 / f64::from(iters);
            println!("loop filter {label}: {dt:.3} ms/iter");
        }
    }
}
