//! AV1 deblocking loop-filter kernels (spec 7.14.6), 8-bit.
//!
//! # Why AV1 needs its own kernel
//!
//! [`crate::deblock_filter`] implements the H.264/AVC deblocking filter: a
//! different mask process and completely different filter taps.  Reusing it
//! for AV1 would not be an approximation, it would be wrong, so this module
//! implements the AV1 filters exactly.
//!
//! # Batching four lines
//!
//! The spec applies the filter to one *sample line* crossing an edge at a
//! time, and the decoder's edge walk always does four consecutive lines with
//! one set of filter parameters.  Those four lines are **independent**: for a
//! vertical edge they are four distinct rows, for a horizontal edge four
//! distinct columns, so no line ever reads a sample another line writes.
//! That is the load-bearing assumption which makes it legal to batch them
//! into four SIMD lanes, and it is what the equivalence tests check (batched
//! kernel vs. four independent single-line reference calls).
//!
//! # Layout
//!
//! Callers hand over an [`Av1Edge4`]: **sample-major, lane-minor**.
//! `edge[k][lane]` is the sample at offset `k - 7` from the edge on line
//! `lane`, i.e.
//!
//! ```text
//!   k    0   1   2   3   4   5   6 | 7   8   9  10  11  12  13
//!  smp  p6  p5  p4  p3  p2  p1  p0 | q0  q1  q2  q3  q4  q5  q6
//! ```
//!
//! This layout is chosen because it is exactly the memory order for a
//! horizontal edge (four adjacent columns are four adjacent bytes), so the
//! common case gathers with plain 4-byte copies.
//!
//! # Bit-exactness
//!
//! Three implementations are provided and are required to agree byte for
//! byte on every input:
//!
//! * [`filter_edge4_reference`] — a literal transcription of spec 7.14.6,
//!   including the unspecialised O(n²) wide-filter tap loop.  Test oracle.
//! * [`filter_edge4_scalar`] — the optimised algorithm on the portable
//!   backend.  This is the scalar fallback and is always compiled.
//! * [`filter_edge4`] — the same algorithm on the best backend the running
//!   CPU provides, chosen by runtime dispatch.
//!
//! The wide filter is specialised from the spec's tap loop into an
//! incremental sliding window.  That is an identity, not an approximation:
//! the spec output is `Round2(A(i) + B(i), log2Size)` where
//! `A(i) = Σ_{m=i-n}^{i+n} v(m)` and `B(i) = Σ_{m=i-n2}^{i+n2} v(m)` (each
//! tap has weight 1, plus a second weight-1 contribution for `|j| <= n2`,
//! which is what the spec's `tap` of 2 means).  Both sums are advanced in
//! O(1) per output instead of re-summing 2n+1 taps.

#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_possible_wrap)]

use crate::simd4::{Portable, Simd4};

/// Four sample lines crossing one edge, sample-major / lane-minor.
///
/// See the module docs for the index → sample mapping.
pub type Av1Edge4 = [[u8; 4]; 14];

/// Index of `p0` in an [`Av1Edge4`].
pub const P0: usize = 6;
/// Index of `q0` in an [`Av1Edge4`].
pub const Q0: usize = 7;

/// Filter size selected by the spec's filter-size process (7.14.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Av1FilterSize {
    /// `filterSize == 4` — narrow filter only.
    Size4,
    /// `filterSize == 8` — narrow or 6/8-tap wide filter.
    Size8,
    /// `filterSize == 16` — narrow, 8-tap or 14-tap wide filter (luma only).
    Size16,
}

/// Parameters for one four-line AV1 edge filter.
///
/// `limit`, `blimit` and `thresh` come from the adaptive filter strength
/// process (spec 7.14.4); `size` from the filter size process (7.14.3).
#[derive(Debug, Clone, Copy)]
pub struct Av1LfParams {
    /// Spec `limit`.
    pub limit: i32,
    /// Spec `blimit`.
    pub blimit: i32,
    /// Spec `thresh`.
    pub thresh: i32,
    /// Spec `filterSize`.
    pub size: Av1FilterSize,
    /// True for U/V planes (spec `plane != 0`).
    pub chroma: bool,
}

impl Av1LfParams {
    /// Spec `filterLen` (7.14.6.2) implied by `size` and `chroma`.
    #[must_use]
    #[inline]
    pub fn filter_len(&self) -> u32 {
        match self.size {
            Av1FilterSize::Size4 => 4,
            Av1FilterSize::Size8 if self.chroma => 6,
            Av1FilterSize::Size8 => 8,
            Av1FilterSize::Size16 if self.chroma => 6,
            Av1FilterSize::Size16 => 16,
        }
    }

    /// The `[lo, hi)` range of [`Av1Edge4`] indices the caller must populate
    /// (and which the kernel may write back).
    ///
    /// This matches exactly the samples the spec's sample-filtering process
    /// touches, so gathering this range introduces no reads the scalar
    /// implementation would not also perform.
    #[must_use]
    #[inline]
    pub fn span(&self) -> core::ops::Range<usize> {
        match self.size {
            Av1FilterSize::Size16 => 0..14,
            _ => 3..11,
        }
    }
}

// ── Public entry points ──────────────────────────────────────────────────────

/// Filters four sample lines crossing one edge, selecting the fastest
/// implementation available on the running CPU.
///
/// Only `edge[k]` for `k` in `params.span()` is read or written.
#[inline]
pub fn filter_edge4(edge: &mut Av1Edge4, params: &Av1LfParams) {
    #[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
    {
        if crate::simd4::neon_available() {
            filter_edge4_generic::<crate::simd4::Neon>(edge, params);
            return;
        }
    }
    filter_edge4_generic::<Portable>(edge, params);
}

/// Filters four sample lines using the portable scalar backend.
///
/// Always compiled on every target; this is the fallback [`filter_edge4`]
/// selects when no architecture backend is available, and it is exercised
/// directly by the test-suite so it can never rot.
#[inline]
pub fn filter_edge4_scalar(edge: &mut Av1Edge4, params: &Av1LfParams) {
    filter_edge4_generic::<Portable>(edge, params);
}

/// Literal transcription of the spec's sample filtering process (7.14.6),
/// applied to each of the four lines independently.
///
/// This keeps the spec's unspecialised wide-filter tap loop and exists to be
/// the oracle for the optimised paths.  It is not used at decode time.
pub fn filter_edge4_reference(edge: &mut Av1Edge4, params: &Av1LfParams) {
    for lane in 0..4 {
        let mut line = [0i32; 14];
        for k in params.span() {
            line[k] = i32::from(edge[k][lane]);
        }
        filter_line_reference(&mut line, params);
        for k in params.span() {
            edge[k][lane] = line[k] as u8;
        }
    }
}

// ── Reference (spec-literal) implementation ──────────────────────────────────

/// Spec 7.14.6.1 sample filtering for one line, with `line[k]` holding the
/// sample at offset `k - 7`.
fn filter_line_reference(line: &mut [i32; 14], params: &Av1LfParams) {
    let at = |line: &[i32; 14], off: i32| -> i32 { line[(off + 7) as usize] };
    let q = |line: &[i32; 14], k: i32| at(line, k);
    let pn = |line: &[i32; 14], k: i32| at(line, -(k + 1));

    let q0 = q(line, 0);
    let q1 = q(line, 1);
    let q2 = q(line, 2);
    let q3 = q(line, 3);
    let p0 = pn(line, 0);
    let p1 = pn(line, 1);
    let p2 = pn(line, 2);
    let p3 = pn(line, 3);

    let (limit, blimit, thresh) = (params.limit, params.blimit, params.thresh);
    let filter_size = match params.size {
        Av1FilterSize::Size4 => 4usize,
        Av1FilterSize::Size8 => 8,
        Av1FilterSize::Size16 => 16,
    };
    let plane = usize::from(params.chroma);

    let mut hev_mask = false;
    hev_mask |= (p1 - p0).abs() > thresh;
    hev_mask |= (q1 - q0).abs() > thresh;

    let filter_len = params.filter_len();

    let mut mask = false;
    mask |= (p1 - p0).abs() > limit;
    mask |= (q1 - q0).abs() > limit;
    mask |= (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit;
    if filter_len >= 6 {
        mask |= (p2 - p1).abs() > limit;
        mask |= (q2 - q1).abs() > limit;
    }
    if filter_len >= 8 {
        mask |= (p3 - p2).abs() > limit;
        mask |= (q3 - q2).abs() > limit;
    }
    let filter_mask = !mask;
    if !filter_mask {
        return;
    }

    let threshold_bd = 1i32;
    let flat_mask = if filter_size >= 8 {
        let mut m = false;
        m |= (p1 - p0).abs() > threshold_bd;
        m |= (q1 - q0).abs() > threshold_bd;
        m |= (p2 - p0).abs() > threshold_bd;
        m |= (q2 - q0).abs() > threshold_bd;
        if filter_len >= 8 {
            m |= (p3 - p0).abs() > threshold_bd;
            m |= (q3 - q0).abs() > threshold_bd;
        }
        !m
    } else {
        false
    };
    let flat_mask2 = if filter_size >= 16 {
        let q4 = q(line, 4);
        let q5 = q(line, 5);
        let q6 = q(line, 6);
        let p4 = pn(line, 4);
        let p5 = pn(line, 5);
        let p6 = pn(line, 6);
        let mut m = false;
        m |= (p6 - p0).abs() > threshold_bd;
        m |= (q6 - q0).abs() > threshold_bd;
        m |= (p5 - p0).abs() > threshold_bd;
        m |= (q5 - q0).abs() > threshold_bd;
        m |= (p4 - p0).abs() > threshold_bd;
        m |= (q4 - q0).abs() > threshold_bd;
        !m
    } else {
        false
    };

    if filter_size == 4 || !flat_mask {
        narrow_filter_reference(line, hev_mask);
    } else if filter_size == 8 || !flat_mask2 {
        wide_filter_reference(line, 3, plane);
    } else {
        wide_filter_reference(line, 4, plane);
    }
}

/// Spec 7.14.6.3 narrow filter, BitDepth = 8.
fn narrow_filter_reference(line: &mut [i32; 14], hev: bool) {
    #[inline]
    fn clamp4(v: i32) -> i32 {
        v.clamp(-128, 127)
    }
    let q0 = line[7];
    let q1 = line[8];
    let p0 = line[6];
    let p1 = line[5];
    let ps1 = p1 - 0x80;
    let ps0 = p0 - 0x80;
    let qs0 = q0 - 0x80;
    let qs1 = q1 - 0x80;
    let mut filter = if hev { clamp4(ps1 - qs1) } else { 0 };
    filter = clamp4(filter + 3 * (qs0 - ps0));
    let filter1 = clamp4(filter + 4) >> 3;
    let filter2 = clamp4(filter + 3) >> 3;
    line[7] = clamp4(qs0 - filter1) + 0x80;
    line[6] = clamp4(ps0 + filter2) + 0x80;
    if !hev {
        let f = (filter1 + 1) >> 1;
        line[8] = clamp4(qs1 - f) + 0x80;
        line[5] = clamp4(ps1 + f) + 0x80;
    }
}

/// Spec 7.14.6.4 wide filter, kept in its unspecialised O(n²) tap form.
fn wide_filter_reference(line: &mut [i32; 14], log2_size: u32, plane: usize) {
    let n: i32 = if log2_size == 4 {
        6
    } else if plane == 0 {
        3
    } else {
        2
    };
    let n2: i32 = if log2_size == 3 && plane == 0 { 0 } else { 1 };
    let at = |line: &[i32; 14], off: i32| -> i32 { line[(off + 7) as usize] };
    let mut f = [0i32; 12];
    for i in -n..n {
        let mut t = 0i32;
        for j in -n..=n {
            let pos = (i + j).clamp(-(n + 1), n);
            let tap = if j.abs() <= n2 { 2 } else { 1 };
            t += at(line, pos) * tap;
        }
        f[(i + n) as usize] = (t + (1 << (log2_size - 1))) >> log2_size;
    }
    for i in -n..n {
        line[(i + 7) as usize] = f[(i + n) as usize];
    }
}

// ── Optimised implementation (one algorithm, any backend) ────────────────────

/// Incremental sliding-window form of spec 7.14.6.4.
///
/// `LOG2` is `log2Size`, `N` is the spec's `n` and `N2` the spec's `n2`.
/// Writes outputs at offsets `-N .. N` (edge indices `7-N .. 7+N`); every
/// other index of `out` is left untouched.
#[inline(always)]
fn wide_filter_lanes<S: Simd4, const LOG2: i32, const N: i32, const N2: i32>(
    v: &[S; 14],
    out: &mut [S; 14],
) {
    // v(m) with the spec's edge clamp to [-(N+1), N].
    let vv = |m: i32| -> S {
        let c = if m < -(N + 1) {
            -(N + 1)
        } else if m > N {
            N
        } else {
            m
        };
        v[(c + 7) as usize]
    };

    // A(-N) = Σ_{m=-2N}^{0} v(m), B(-N) = Σ_{m=-N-N2}^{-N+N2} v(m).
    let mut a = S::splat(0);
    let mut m = -2 * N;
    while m <= 0 {
        a = a.add(vv(m));
        m += 1;
    }
    let mut b = S::splat(0);
    let mut m = -N - N2;
    while m <= -N + N2 {
        b = b.add(vv(m));
        m += 1;
    }

    let rnd = S::splat(1 << (LOG2 - 1));
    let mut i = -N;
    while i < N {
        out[(i + 7) as usize] = a.add(b).add(rnd).shr::<LOG2>();
        // Advance both windows by one sample.
        a = a.add(vv(i + N + 1)).sub(vv(i - N));
        b = b.add(vv(i + N2 + 1)).sub(vv(i - N2));
        i += 1;
    }
}

/// The optimised four-line filter, generic over the SIMD backend.
#[inline(always)]
fn filter_edge4_generic<S: Simd4>(edge: &mut Av1Edge4, params: &Av1LfParams) {
    let span = params.span();
    let (lo, hi) = (span.start, span.end);

    let mut v = [S::splat(0); 14];
    for k in lo..hi {
        let e = edge[k];
        v[k] = S::from_array([
            i32::from(e[0]),
            i32::from(e[1]),
            i32::from(e[2]),
            i32::from(e[3]),
        ]);
    }

    let p = |n: usize| v[P0 - n];
    let q = |n: usize| v[Q0 + n];

    let ones = S::splat(-1);
    let zero = S::splat(0);
    let one = S::splat(1);
    let thresh = S::splat(params.thresh);
    let limit = S::splat(params.limit);
    let blimit = S::splat(params.blimit);

    let d_p1p0 = p(1).sub(p(0)).abs();
    let d_q1q0 = q(1).sub(q(0)).abs();
    let hev = d_p1p0.gt(thresh).or(d_q1q0.gt(thresh));

    let filter_len = params.filter_len();

    // Filter mask process (spec 7.14.6.2).
    let d_p0q0 = p(0).sub(q(0)).abs();
    let d_p1q1 = p(1).sub(q(1)).abs();
    // |p0-q0|*2 + |p1-q1|/2 ; the operand of /2 is an abs() so it is
    // non-negative and the arithmetic shift is exactly integer division.
    let edge_sum = d_p0q0.add(d_p0q0).add(d_p1q1.shr::<1>());
    let mut mask = d_p1p0
        .gt(limit)
        .or(d_q1q0.gt(limit))
        .or(edge_sum.gt(blimit));
    if filter_len >= 6 {
        mask = mask
            .or(p(2).sub(p(1)).abs().gt(limit))
            .or(q(2).sub(q(1)).abs().gt(limit));
    }
    if filter_len >= 8 {
        mask = mask
            .or(p(3).sub(p(2)).abs().gt(limit))
            .or(q(3).sub(q(2)).abs().gt(limit));
    }
    let filter_mask = mask.andnot(ones);
    if !filter_mask.any() {
        return;
    }

    let big = params.size != Av1FilterSize::Size4;
    let widest = params.size == Av1FilterSize::Size16;

    // Flat masks (threshold_bd == 1 at BitDepth 8).
    let flat = if big {
        let mut m = d_p1p0
            .gt(one)
            .or(d_q1q0.gt(one))
            .or(p(2).sub(p(0)).abs().gt(one))
            .or(q(2).sub(q(0)).abs().gt(one));
        if filter_len >= 8 {
            m = m
                .or(p(3).sub(p(0)).abs().gt(one))
                .or(q(3).sub(q(0)).abs().gt(one));
        }
        m.andnot(ones)
    } else {
        zero
    };
    let flat2 = if widest {
        let m = p(6)
            .sub(p(0))
            .abs()
            .gt(one)
            .or(q(6).sub(q(0)).abs().gt(one))
            .or(p(5).sub(p(0)).abs().gt(one))
            .or(q(5).sub(q(0)).abs().gt(one))
            .or(p(4).sub(p(0)).abs().gt(one))
            .or(q(4).sub(q(0)).abs().gt(one));
        m.andnot(ones)
    } else {
        zero
    };

    // Branch selection — mutually exclusive by construction, mirroring
    // `if filterSize == 4 || !flat { narrow } else if filterSize == 8 ||
    //  !flat2 { wide(3) } else { wide(4) }`.
    let use_narrow = if big {
        filter_mask.and(flat.andnot(ones))
    } else {
        filter_mask
    };
    let use_wide8 = if widest {
        filter_mask.and(flat).and(flat2.andnot(ones))
    } else if big {
        filter_mask.and(flat)
    } else {
        zero
    };
    let use_wide14 = if widest {
        filter_mask.and(flat).and(flat2)
    } else {
        zero
    };

    let mut out = v;

    // Narrow filter (spec 7.14.6.3).
    if use_narrow.any() {
        let lo4 = S::splat(-128);
        let hi4 = S::splat(127);
        let c128 = S::splat(0x80);
        let ps1 = p(1).sub(c128);
        let ps0 = p(0).sub(c128);
        let qs0 = q(0).sub(c128);
        let qs1 = q(1).sub(c128);
        let f_hev = ps1.sub(qs1).clamp(lo4, hi4);
        let d = qs0.sub(ps0);
        let filter = S::select(hev, f_hev, zero)
            .add(d)
            .add(d)
            .add(d)
            .clamp(lo4, hi4);
        let filter1 = filter.add(S::splat(4)).clamp(lo4, hi4).shr::<3>();
        let filter2 = filter.add(S::splat(3)).clamp(lo4, hi4).shr::<3>();
        let oq0 = qs0.sub(filter1).clamp(lo4, hi4).add(c128);
        let op0 = ps0.add(filter2).clamp(lo4, hi4).add(c128);
        let f = filter1.add(one).shr::<1>();
        // p1/q1 are only updated when hev is false.
        let oq1 = S::select(hev, q(1), qs1.sub(f).clamp(lo4, hi4).add(c128));
        let op1 = S::select(hev, p(1), ps1.add(f).clamp(lo4, hi4).add(c128));

        out[P0 - 1] = S::select(use_narrow, op1, out[P0 - 1]);
        out[P0] = S::select(use_narrow, op0, out[P0]);
        out[Q0] = S::select(use_narrow, oq0, out[Q0]);
        out[Q0 + 1] = S::select(use_narrow, oq1, out[Q0 + 1]);
    }

    // Wide filter, log2Size == 3 (6 taps on chroma, 8 taps on luma).
    if use_wide8.any() {
        let mut w = v;
        if params.chroma {
            wide_filter_lanes::<S, 3, 2, 1>(&v, &mut w);
        } else {
            wide_filter_lanes::<S, 3, 3, 0>(&v, &mut w);
        }
        // Union of the written ranges: luma writes 4..=9, chroma 5..=8.
        for k in 4..10 {
            out[k] = S::select(use_wide8, w[k], out[k]);
        }
    }

    // Wide filter, log2Size == 4 (14 taps, luma only).
    if use_wide14.any() {
        let mut w = v;
        wide_filter_lanes::<S, 4, 6, 1>(&v, &mut w);
        for k in 1..13 {
            out[k] = S::select(use_wide14, w[k], out[k]);
        }
    }

    for k in lo..hi {
        let a = out[k].to_array();
        edge[k] = [a[0] as u8, a[1] as u8, a[2] as u8, a[3] as u8];
    }
}

#[cfg(test)]
mod tests;
