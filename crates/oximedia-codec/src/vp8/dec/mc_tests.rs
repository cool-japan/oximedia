//! Unit tests for the parent `mc` module — VP8 inter motion compensation.
//!
//! Split out of `mc.rs` (which is close to this workspace's 2000-line file
//! limit) as a verbatim move: the module is still `dec::mc::tests`, so every
//! `use super::*` path and every relative `include_bytes!` resolves exactly as
//! it did inline.

use super::*;

/// Whole-pixel prediction: a straight `w` x `h` copy — the **independent
/// oracle** for the phase-0 identity, and deliberately test-only.
///
/// RFC 6386's version-3 ("None" reconstruction filter) profile and every
/// zero-fraction vector *could* be short-circuited to a copy; this decoder
/// does not do that (see [`ReconFilter::FullPel`]), it runs them through the
/// filter's identity kernel like every other phase. This function is the
/// other implementation that claim is checked against, in
/// `test_full_pel_predict_matches_filter_at_frac_zero`; it lives here rather
/// than in the production module precisely because nothing in the decode
/// path may call it.
///
/// This is what a version-3 (RFC 6386 §9.1 "None" reconstruction filter)
/// block degenerates to, and what the reference decoder's `filter_block`
/// short-circuit returns for any whole-pixel vector (rfc6386.txt lines
/// 12052-12070). It is *not* on [`ReconFilter::predict`]'s path — see
/// [`ReconFilter::FullPel`] for why — but it is bit-identical to the
/// filtered path at fraction zero, which
/// `test_full_pel_predict_matches_filter_at_frac_zero` checks. Only the
/// `w` x `h` rectangle is read: no tap reach.
///
/// # Errors
/// [`CodecError::InvalidBitstream`] when the rectangle does not fit inside
/// `src`; [`CodecError::Internal`] for a zero dimension or too small a
/// `dst`.
fn full_pel_predict(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    w: usize,
    h: usize,
) -> CodecResult<()> {
    check_block_dims(w, h)?;
    check_dst(dst, dst_stride, w, h)?;
    let last = (src_off as i64) + (h as i64 - 1) * (src_stride as i64) + (w as i64 - 1);
    if last >= src.len() as i64 {
        return Err(oob("whole-pixel copy"));
    }
    for row in 0..h {
        let src_row = src
            .get(src_off + row * src_stride..src_off + row * src_stride + w)
            .ok_or_else(|| oob("whole-pixel copy row"))?;
        let dst_row = dst
            .get_mut(row * dst_stride..row * dst_stride + w)
            .ok_or_else(|| internal("destination row out of range"))?;
        dst_row.copy_from_slice(src_row);
    }
    Ok(())
}

/// A bordered test plane laid out like [`super::super::Planes`].
struct TestPlane {
    data: Vec<u8>,
    stride: usize,
    origin: usize,
    w: usize,
    h: usize,
}

impl TestPlane {
    /// Builds a `w` x `h` plane with a [`BORDER`]-pixel margin, the
    /// visible area filled by `f(x, y)` and the margin left at zero
    /// until [`TestPlane::extend`] replicates into it.
    fn new(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Self {
        let stride = w + 2 * BORDER;
        let origin = BORDER * stride + BORDER;
        let mut data = vec![0u8; stride * (h + 2 * BORDER)];
        for y in 0..h {
            for x in 0..w {
                data[origin + y * stride + x] = f(x, y);
            }
        }
        Self {
            data,
            stride,
            origin,
            w,
            h,
        }
    }

    /// A plane whose every byte, borders included, is pseudo-random.
    fn noise(w: usize, h: usize, seed: u32) -> Self {
        let mut plane = Self::new(w, h, |_, _| 0);
        let mut rng = Lcg(seed);
        for byte in &mut plane.data {
            *byte = rng.next();
        }
        plane
    }

    fn src(&self) -> SrcPlane<'_> {
        SrcPlane {
            data: &self.data,
            stride: self.stride,
            origin: self.origin,
            width: self.w,
            height: self.h,
        }
    }

    fn at(&self, x: usize, y: usize) -> usize {
        self.origin + y * self.stride + x
    }

    /// Replicates the edges into the whole border, like
    /// [`super::super::refs::RefSurface::extend_borders`].
    fn extend(&mut self) {
        for y in 0..self.h {
            let line = self.at(0, y);
            let (left, right) = (self.data[line], self.data[line + self.w - 1]);
            for b in 1..=BORDER {
                self.data[line - b] = left;
                self.data[line + self.w - 1 + b] = right;
            }
        }
        let ext = self.w + 2 * BORDER;
        let first = self.origin - BORDER;
        let last = first + (self.h - 1) * self.stride;
        for b in 1..=BORDER {
            self.data
                .copy_within(first..first + ext, first - b * self.stride);
            self.data
                .copy_within(last..last + ext, last + b * self.stride);
        }
    }
}

/// Deterministic pseudo-random bytes (a plain LCG, so the test data is
/// reproducible without a dependency).
struct Lcg(u32);

impl Lcg {
    fn next(&mut self) -> u8 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 24) as u8
    }
}

/// Independent reference implementation: a direct 2-D convolution.
///
/// Written from the RFC's `Hinterp` / `Vinterp` pseudo-code
/// (rfc6386.txt lines 6526-6600) with a deliberately different shape —
/// no intermediate buffer, no strides, the horizontal value recomputed
/// for every output pixel — so it cannot share a bug with
/// [`filter_predict`]. `clamp_intermediate` exists so a test can show
/// the pass-1 clamp is actually being exercised.
fn reference_conv(
    plane: &TestPlane,
    x: i64,
    y: i64,
    xoff: usize,
    yoff: usize,
    w: usize,
    h: usize,
    taps: &[[i32; 6]; 8],
    clamp_intermediate: bool,
) -> Vec<u8> {
    let (hf, vf) = (taps[xoff], taps[yoff]);
    let mut out = Vec::with_capacity(w * h);
    for r in 0..h as i64 {
        for c in 0..w as i64 {
            let mut vsum = 0i32;
            for (j, vcoeff) in vf.iter().enumerate() {
                let row = y + r + j as i64 - 2;
                let mut hsum = 0i32;
                for (i, hcoeff) in hf.iter().enumerate() {
                    let idx =
                        plane.origin as i64 + row * plane.stride as i64 + x + c + i as i64 - 2;
                    hsum += i32::from(plane.data[idx as usize]) * hcoeff;
                }
                let hval = (hsum + 64) >> 7;
                vsum += if clamp_intermediate {
                    hval.clamp(0, 255)
                } else {
                    hval
                } * vcoeff;
            }
            out.push(((vsum + 64) >> 7).clamp(0, 255) as u8);
        }
    }
    out
}

/// Runs one prediction into a fresh tight `w` x `h` buffer.
fn run_filter(
    plane: &TestPlane,
    x: i64,
    y: i64,
    xoff: usize,
    yoff: usize,
    w: usize,
    h: usize,
    filter: ReconFilter,
) -> CodecResult<Vec<u8>> {
    let off = source_offset(&plane.src(), x, y, w, h)?;
    let mut dst = vec![0u8; w * h];
    filter.predict(
        &mut dst,
        w,
        &plane.data,
        plane.stride,
        off,
        xoff,
        yoff,
        w,
        h,
    )?;
    Ok(dst)
}

/// Predicts a block straight from a [`SrcPlane`] at a motion vector,
/// bypassing [`predict_inter_mb`]'s block decomposition.
fn manual_block(
    src: &SrcPlane<'_>,
    x: usize,
    y: usize,
    mv: (i32, i32),
    w: usize,
    h: usize,
    filter: ReconFilter,
) -> CodecResult<Vec<u8>> {
    let off = source_offset(
        src,
        x as i64 + i64::from(mv.0 >> 3),
        y as i64 + i64::from(mv.1 >> 3),
        w,
        h,
    )?;
    let mut out = vec![0u8; w * h];
    filter.predict(
        &mut out,
        w,
        src.data,
        src.stride,
        off,
        (mv.0 & 7) as usize,
        (mv.1 & 7) as usize,
        w,
        h,
    )?;
    Ok(out)
}

/// A strided plane as `(data, stride, offset of pixel (0,0))`.
type Plane<'a> = (&'a [u8], usize, usize);

/// Copies a `w` x `h` region of a strided plane into a tight buffer.
fn region(plane: Plane<'_>, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
    let (data, stride, origin) = plane;
    let mut out = Vec::with_capacity(w * h);
    for r in 0..h {
        for c in 0..w {
            out.push(data[origin + (y + r) * stride + x + c]);
        }
    }
    out
}

/// Shorthand so the call sites below stay on one line each.
const SIX: ReconFilter = ReconFilter::Sixtap;
fn py(p: &super::super::Planes) -> Plane<'_> {
    (&p.y, p.y_stride, p.y_origin)
}
fn pu(p: &super::super::Planes) -> Plane<'_> {
    (&p.u, p.uv_stride, p.uv_origin)
}
fn pv(p: &super::super::Planes) -> Plane<'_> {
    (&p.v, p.uv_stride, p.uv_origin)
}
fn ry(r: &RefSurface) -> Plane<'_> {
    (&r.y, r.y_stride, r.y_origin)
}
fn ru(r: &RefSurface) -> Plane<'_> {
    (&r.u, r.uv_stride, r.uv_origin)
}
fn rv(r: &RefSurface) -> Plane<'_> {
    (&r.v, r.uv_stride, r.uv_origin)
}

/// [`predict_inter_mb`] with the macroblock coordinates tupled, purely
/// to keep the call sites in this module to one line each.
fn predict_mb(
    planes: &mut super::super::Planes,
    refs: &RefSurface,
    mb: (usize, usize),
    filter: ReconFilter,
    luma_mvs: &[Mv; 16],
    whole_mb: Option<Mv>,
) -> CodecResult<()> {
    predict_inter_mb(planes, refs, mb.0, mb.1, filter, luma_mvs, whole_mb)
}

/// The three filters, paired with the tap table they convolve with.
const FILTERS: [(ReconFilter, &[[i32; 6]; 8]); 3] = [
    (ReconFilter::Sixtap, &SIXTAP_FILTERS),
    (ReconFilter::Bilinear, &BILINEAR_TAPS),
    (ReconFilter::FullPel, &BILINEAR_TAPS),
];

// ---------------------------------------------------------------
// filter selection
// ---------------------------------------------------------------

#[test]
fn test_recon_filter_from_version() {
    // RFC 6386 §9.1, rfc6386.txt lines 1690-1703; the reference decoder
    // maps every non-zero version onto the bilinear table
    // (rfc6386.txt lines 12683-12686).
    use ReconFilter::{Bilinear, FullPel, Sixtap};
    for (v, want) in [(0u8, Sixtap), (1, Bilinear), (2, Bilinear), (3, FullPel)] {
        assert_eq!(ReconFilter::from_version(v).ok(), Some(want), "version {v}");
    }
    assert_eq!(FullPel.taps(), &BILINEAR_TAPS);
    for bad in 4..=7u8 {
        let got = ReconFilter::from_version(bad);
        assert!(
            matches!(got, Err(CodecError::InvalidBitstream(_))),
            "version {bad} is reserved (RFC 6386 §9.1)"
        );
    }
}

#[test]
fn test_only_version_3_masks_chroma_to_full_pel() {
    assert!(!ReconFilter::Sixtap.full_pel_chroma());
    assert!(!ReconFilter::Bilinear.full_pel_chroma());
    assert!(ReconFilter::FullPel.full_pel_chroma());
}

#[test]
fn test_bilinear_taps_are_the_table_zero_padded() {
    // Must reproduce `bilinear_filters`, rfc6386.txt lines 11093-11104.
    let expected: [[i32; 6]; 8] = [
        [0, 0, 128, 0, 0, 0],
        [0, 0, 112, 16, 0, 0],
        [0, 0, 96, 32, 0, 0],
        [0, 0, 80, 48, 0, 0],
        [0, 0, 64, 64, 0, 0],
        [0, 0, 48, 80, 0, 0],
        [0, 0, 32, 96, 0, 0],
        [0, 0, 16, 112, 0, 0],
    ];
    assert_eq!(BILINEAR_TAPS, expected);
    for (phase, row) in BILINEAR_TAPS.iter().enumerate() {
        assert_eq!(row[2], BILINEAR_FILTERS[phase][0]);
        assert_eq!(row[3], BILINEAR_FILTERS[phase][1]);
        assert_eq!(row.iter().sum::<i32>(), 128, "row {phase} must sum to 128");
    }
    // Row 0 of both tables is the identity that makes the unconditional
    // two-pass path a copy at fraction zero.
    assert_eq!(SIXTAP_FILTERS[0], [0, 0, 128, 0, 0, 0]);
    assert_eq!(BILINEAR_TAPS[0], [0, 0, 128, 0, 0, 0]);
}

// ---------------------------------------------------------------
// identity / whole-pixel
// ---------------------------------------------------------------

#[test]
fn test_identity_is_memcpy_all_block_sizes() -> CodecResult<()> {
    // Borders are noise too: the identity kernel must not read them, and
    // a regression that did would show up here.
    let plane = TestPlane::noise(48, 48, 0x1234_5678);
    for (w, h) in [
        (4usize, 4usize),
        (4, 8),
        (8, 4),
        (8, 8),
        (16, 8),
        (8, 16),
        (16, 16),
        (1, 1),
        (3, 5),
    ] {
        let want = region((&plane.data, plane.stride, plane.origin), 8, 9, w, h);
        for (filter, _) in FILTERS {
            assert_eq!(
                run_filter(&plane, 8, 9, 0, 0, w, h, filter)?,
                want,
                "{filter:?} {w}x{h} at fraction 0 must be an exact copy"
            );
        }
    }
    Ok(())
}

#[test]
fn test_full_pel_predict_matches_filter_at_frac_zero() -> CodecResult<()> {
    let plane = TestPlane::noise(32, 32, 0xDEAD_BEEF);
    let (w, h) = (16usize, 16usize);
    let mut copied = vec![0u8; w * h];
    full_pel_predict(
        &mut copied,
        w,
        &plane.data,
        plane.stride,
        plane.at(5, 7),
        w,
        h,
    )?;
    for (filter, _) in FILTERS {
        assert_eq!(
            copied,
            run_filter(&plane, 5, 7, 0, 0, w, h, filter)?,
            "{filter:?} at fraction 0 must equal the whole-pixel copy"
        );
    }
    Ok(())
}

#[test]
fn test_bilinear_and_sixtap_agree_at_frac_zero() -> CodecResult<()> {
    let plane = TestPlane::noise(32, 32, 0x0BAD_F00D);
    assert_eq!(
        run_filter(&plane, 3, 4, 0, 0, 8, 8, ReconFilter::Sixtap)?,
        run_filter(&plane, 3, 4, 0, 0, 8, 8, ReconFilter::Bilinear)?,
        "identity rows of both tables must agree"
    );
    Ok(())
}

#[test]
fn test_constant_source_is_dc_preserving() -> CodecResult<()> {
    // Every kernel sums to 128, so a flat source must come out flat:
    // (v * 128 + 64) >> 7 == v.
    for level in [0u8, 1, 17, 128, 254, 255] {
        let plane = TestPlane::new(32, 32, |_, _| level);
        for xoff in 0..8 {
            for yoff in 0..8 {
                for (filter, _) in FILTERS {
                    let got = run_filter(&plane, 8, 8, xoff, yoff, 4, 4, filter)?;
                    assert!(
                        got.iter().all(|&p| p == level),
                        "{filter:?} ({xoff},{yoff}) on a flat {level} source"
                    );
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------
// against an independent convolution
// ---------------------------------------------------------------

#[test]
fn test_ramp_all_64_phases_match_direct_convolution() -> CodecResult<()> {
    // A 24x24 linear ramp. The bicubic kernels are not moment-exact
    // (row 2 has a first moment of 30, not 32), so there is no closed
    // form to compare against -- hence the independent convolution.
    let plane = TestPlane::new(24, 24, |x, y| ((x * 7 + y * 11) % 256) as u8);
    for xoff in 0..8 {
        for yoff in 0..8 {
            for (filter, taps) in FILTERS {
                assert_eq!(
                    run_filter(&plane, 6, 8, xoff, yoff, 4, 4, filter)?,
                    reference_conv(&plane, 6, 8, xoff, yoff, 4, 4, taps, true),
                    "{filter:?} phase ({xoff},{yoff})"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn test_random_source_matches_direct_convolution_all_sizes() -> CodecResult<()> {
    let plane = TestPlane::noise(40, 40, 0xC0FF_EE00);
    for (w, h) in [(4usize, 4usize), (8, 8), (16, 16), (16, 4), (4, 16)] {
        for (xoff, yoff) in [(0, 0), (1, 0), (0, 1), (3, 5), (7, 7), (4, 4), (2, 6)] {
            for (filter, taps) in FILTERS {
                assert_eq!(
                    run_filter(&plane, 5, 6, xoff, yoff, w, h, filter)?,
                    reference_conv(&plane, 5, 6, xoff, yoff, w, h, taps, true),
                    "{filter:?} {w}x{h} phase ({xoff},{yoff})"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn test_hand_computed_horizontal_tap() -> CodecResult<()> {
    // Source values 1,2,3,... left to right and constant down columns,
    // so the vertical identity pass is a no-op.
    //
    // Output pixel (0,0) at x=4 reads source columns 2..=7, values
    // 3,4,5,6,7,8. Kernel 1/8 is {0, -6, 123, 12, -1, 0}:
    //     0*3 + (-6)*4 + 123*5 + 12*6 + (-1)*7 + 0*8
    //   = -24 + 615 + 72 - 7 = 656;  (656 + 64) >> 7 = 720 >> 7 = 5.
    let plane = TestPlane::new(24, 24, |x, _| (x + 1) as u8);
    assert_eq!(
        run_filter(&plane, 4, 4, 1, 0, 4, 4, ReconFilter::Sixtap)?[0],
        5,
        "hand-computed 1/8 horizontal sample"
    );
    // Kernel 1/2 {3, -16, 77, 77, -16, 3}:
    //     9 - 64 + 385 + 462 - 112 + 24 = 704;  (704 + 64) >> 7 = 6.
    assert_eq!(
        run_filter(&plane, 4, 4, 4, 0, 4, 4, ReconFilter::Sixtap)?[0],
        6,
        "hand-computed 1/2 horizontal sample"
    );
    // Bilinear 1/2 {0, 0, 64, 64, 0, 0}:
    //     (64*5 + 64*6 + 64) >> 7 = 768 >> 7 = 6.
    assert_eq!(
        run_filter(&plane, 4, 4, 4, 0, 4, 4, ReconFilter::Bilinear)?[0],
        6,
        "hand-computed bilinear 1/2 horizontal sample"
    );
    Ok(())
}

#[test]
fn test_hand_computed_vertical_tap() -> CodecResult<()> {
    // Mirror image: values increase down the columns, so the horizontal
    // identity pass is a no-op and the same arithmetic applies to the
    // vertical pass.
    let plane = TestPlane::new(24, 24, |_, y| (y + 1) as u8);
    assert_eq!(
        run_filter(&plane, 4, 4, 0, 1, 4, 4, ReconFilter::Sixtap)?[0],
        5,
        "hand-computed 1/8 vertical sample"
    );
    assert_eq!(
        run_filter(&plane, 4, 4, 0, 4, 4, 4, ReconFilter::Sixtap)?[0],
        6,
        "hand-computed 1/2 vertical sample"
    );
    Ok(())
}

#[test]
fn test_intermediate_result_is_clamped_before_the_vertical_pass() -> CodecResult<()> {
    // Quarter-pel taps are {2, -11, 108, 36, -8, 1}. Feeding 255 into
    // every positive tap and 0 into both negative ones gives
    // 255 * (2 + 108 + 36 + 1) = 37485 -> (37485 + 64) >> 7 = 293,
    // which an `unsigned char` intermediate clamps to 255.
    //
    // The period-6 pattern is 255 at phases {0, 2, 3, 5} and 0 at
    // {1, 4}, shifted one column per row, so every row of the horizontal
    // pass overshoots somewhere.
    let plane = TestPlane::new(32, 32, |x, y| {
        u8::from(matches!((x + y) % 6, 0 | 2 | 3 | 5)) * 255
    });
    let clamped = reference_conv(&plane, 8, 8, 2, 3, 8, 8, &SIXTAP_FILTERS, true);
    assert_ne!(
        clamped,
        reference_conv(&plane, 8, 8, 2, 3, 8, 8, &SIXTAP_FILTERS, false),
        "test pattern must actually overflow the intermediate, \
             otherwise the clamp is not being exercised"
    );
    assert_eq!(
        run_filter(&plane, 8, 8, 2, 3, 8, 8, ReconFilter::Sixtap)?,
        clamped,
        "the intermediate buffer is unsigned char in the reference \
             decoder (rfc6386.txt line 12023), so pass 1 clamps"
    );
    Ok(())
}

#[test]
fn test_negative_intermediate_clamps_to_zero() -> CodecResult<()> {
    // The mirror image: 255 only where the negative taps land, so the
    // horizontal sum is 255 * (-11 - 8) = -4845 ->
    // (-4845 + 64) >> 7 = -38, which the intermediate clamps to 0.
    let plane = TestPlane::new(32, 32, |x, y| u8::from(matches!((x + y) % 6, 1 | 4)) * 255);
    let clamped = reference_conv(&plane, 8, 8, 2, 3, 8, 8, &SIXTAP_FILTERS, true);
    assert_ne!(
        clamped,
        reference_conv(&plane, 8, 8, 2, 3, 8, 8, &SIXTAP_FILTERS, false),
        "test pattern must actually underflow the intermediate"
    );
    assert_eq!(
        run_filter(&plane, 8, 8, 2, 3, 8, 8, ReconFilter::Sixtap)?,
        clamped
    );
    Ok(())
}

// ---------------------------------------------------------------
// block decomposition equivalence
// ---------------------------------------------------------------

/// Predicts a `size` x `size` block in one call and again as a grid of
/// 4x4 calls at the same vector, and returns both.
fn whole_vs_subblocks(
    plane: &TestPlane,
    size: usize,
    xoff: usize,
    yoff: usize,
    filter: ReconFilter,
) -> CodecResult<(Vec<u8>, Vec<u8>)> {
    let whole = run_filter(plane, 6, 6, xoff, yoff, size, size, filter)?;
    let mut split = vec![0u8; size * size];
    let per_row = size / 4;
    for b in 0..per_row * per_row {
        let (bx, by) = ((b % per_row) * 4, (b / per_row) * 4);
        let (sx, sy) = (6 + bx as i64, 6 + by as i64);
        let sub = run_filter(plane, sx, sy, xoff, yoff, 4, 4, filter)?;
        for r in 0..4 {
            for c in 0..4 {
                split[(by + r) * size + bx + c] = sub[r * 4 + c];
            }
        }
    }
    Ok((whole, split))
}

#[test]
fn test_16x16_equals_sixteen_4x4() -> CodecResult<()> {
    // The reference decoder predicts a non-SPLITMV macroblock as sixteen
    // 4x4 blocks (rfc6386.txt lines 12481-12497); this module issues one
    // 16x16. They must agree bit for bit.
    let plane = TestPlane::noise(48, 48, 0x5EED_1234);
    for (xoff, yoff) in [(0, 0), (1, 7), (3, 3), (5, 2), (7, 6), (4, 4)] {
        for (filter, _) in FILTERS {
            let (whole, split) = whole_vs_subblocks(&plane, 16, xoff, yoff, filter)?;
            assert_eq!(whole, split, "{filter:?} phase ({xoff},{yoff})");
        }
    }
    Ok(())
}

#[test]
fn test_8x8_equals_four_4x4() -> CodecResult<()> {
    // The same argument for the chroma planes (rfc6386.txt lines
    // 12499-12513 predict chroma as four 4x4 blocks).
    let plane = TestPlane::noise(32, 32, 0xABCD_0001);
    for (xoff, yoff) in [(0, 0), (2, 5), (7, 1)] {
        for (filter, _) in FILTERS {
            let (whole, split) = whole_vs_subblocks(&plane, 8, xoff, yoff, filter)?;
            assert_eq!(whole, split, "{filter:?} phase ({xoff},{yoff})");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------
// chroma motion vector derivation
// ---------------------------------------------------------------

#[test]
fn test_chroma_mv_whole_mb_rounds_away_from_zero() {
    // (v + 1 + (v >> 31) * 2) / 2 with C truncation, rfc6386.txt 12456.
    let luma = [0i16, 1, 2, 3, 4, 7, 8, 16, -1, -2, -3, -4, -7, -8, -16];
    let want = [0i32, 1, 1, 2, 2, 4, 4, 8, -1, -1, -2, -2, -4, -4, -8];
    for (&l, &w) in luma.iter().zip(want.iter()) {
        assert_eq!(chroma_mv_whole_mb((l, l), false), (w, w), "luma {l}");
    }
}

#[test]
fn test_chroma_mv_whole_mb_matches_the_c_expression() {
    // Cross-check the whole i16 range against a literal transcription of
    // rfc6386.txt line 12456.
    for v in i16::MIN..=i16::MAX {
        let c = i32::from(v);
        let want = (c + 1 + (c >> 31) * 2) / 2;
        assert_eq!(chroma_mv_whole_mb((v, v), false), (want, want), "for {v}");
    }
}

#[test]
fn test_chroma_mv_whole_mb_full_pel_mask() {
    // `uvmv.d.x &= ~7` (rfc6386.txt lines 12337-12340).
    for v in [9i16, 15, 16, 1, -9, -15, -16, -1] {
        let (x, y) = chroma_mv_whole_mb((v, v), true);
        assert_eq!(x & 7, 0, "x fraction must be cleared for {v}");
        assert_eq!(y & 7, 0, "y fraction must be cleared for {v}");
        assert_eq!(x, chroma_mv_whole_mb((v, v), false).0 & !7);
    }
}

#[test]
fn test_chroma_mv_split_sums_four_and_divides_by_eight() -> CodecResult<()> {
    // All four sub-vectors equal: the sum is 4v and 4v/8 == v/2, the
    // same value the whole-macroblock derivation produces (both are
    // "halve, rounding away from zero").
    for v in [0i16, 1, 2, 3, 7, 8, -1, -2, -3, -7, -8, 100, -100] {
        assert_eq!(
            chroma_mv_split(&[(v, v); 16], 0, false)?,
            chroma_mv_whole_mb((v, v), false),
            "uniform {v}"
        );
    }
    Ok(())
}

/// The luma sub-blocks each chroma sub-block averages (RFC 6386 §18.1,
/// rfc6386.txt lines 6313-6320).
const CHROMA_GROUPS: [[usize; 4]; 4] =
    [[0, 1, 4, 5], [2, 3, 6, 7], [8, 9, 12, 13], [10, 11, 14, 15]];

#[test]
fn test_chroma_mv_split_uses_the_co_located_group() -> CodecResult<()> {
    for (b, group) in CHROMA_GROUPS.iter().enumerate() {
        let mut mvs = [(0i16, 0i16); 16];
        for &g in group {
            mvs[g] = (8, -8);
        }
        // sum = 32 / -32 -> (32 + 4) / 8 = 4, (-32 - 4) / 8 = -4.
        assert_eq!(chroma_mv_split(&mvs, b, false)?, (4, -4), "sub-block {b}");
        for other in (0..4).filter(|&o| o != b) {
            assert_eq!(
                chroma_mv_split(&mvs, other, false)?,
                (0, 0),
                "sub-block {other} must not see group {b}"
            );
        }
    }
    Ok(())
}

#[test]
fn test_chroma_mv_split_rounding_matches_the_rfc_avg() -> CodecResult<()> {
    // RFC 6386 §18.1 `avg()`: s >= 0 ? (s + 4) >> 3 : -((-s + 4) >> 3)
    // (rfc6386.txt lines 6333-6348), an independent spelling of
    // `calculate_chroma_splitmv`'s bias-then-divide.
    let avg = |s: i32| -> i32 {
        if s >= 0 {
            (s + 4) >> 3
        } else {
            -((-s + 4) >> 3)
        }
    };
    let mut rng = Lcg(0x9999_0001);
    for _ in 0..2000 {
        let mut mvs = [(0i16, 0i16); 16];
        for mv in &mut mvs {
            let x = (i16::from(rng.next()) - 128) * 13;
            let y = (i16::from(rng.next()) - 128) * 13;
            *mv = (x, y);
        }
        for (b, group) in CHROMA_GROUPS.iter().enumerate() {
            let sx: i32 = group.iter().map(|&g| i32::from(mvs[g].0)).sum();
            let sy: i32 = group.iter().map(|&g| i32::from(mvs[g].1)).sum();
            assert_eq!(chroma_mv_split(&mvs, b, false)?, (avg(sx), avg(sy)), "{b}");
        }
    }
    Ok(())
}

#[test]
fn test_chroma_mv_split_full_pel_mask_and_bad_index() -> CodecResult<()> {
    let (x, y) = chroma_mv_split(&[(9i16, -9i16); 16], 0, true)?;
    assert_eq!((x & 7, y & 7), (0, 0));
    assert!(matches!(
        chroma_mv_split(&[(0i16, 0i16); 16], 4, false),
        Err(CodecError::Internal(_))
    ));
    Ok(())
}

#[test]
fn test_chroma_mv_derivation_is_overflow_safe() -> CodecResult<()> {
    // i16 extremes: the whole-macroblock halving and the split sum (up
    // to 4 * 32767) must both stay in i32.
    for v in [i16::MIN, i16::MIN + 1, i16::MAX - 1, i16::MAX] {
        let (x, y) = chroma_mv_whole_mb((v, v), false);
        assert!(x.abs() <= 16_384 && y.abs() <= 16_384);
        let (sx, sy) = chroma_mv_split(&[(v, v); 16], 0, false)?;
        assert!(sx.abs() <= 16_384 && sy.abs() <= 16_384);
    }
    Ok(())
}

// ---------------------------------------------------------------
// bounds
// ---------------------------------------------------------------

#[test]
fn test_border_read_is_the_replicated_corner() -> CodecResult<()> {
    // A vector 16 pixels up and left of macroblock (0,0) puts the whole
    // tap window (x, y in -18..=-10) inside the replicated top-left
    // corner, so every source sample is the corner pixel and the
    // DC-preserving kernels reproduce it exactly.
    let mut plane = TestPlane::new(32, 32, |x, y| ((x * 3 + y * 5) % 200 + 20) as u8);
    plane.extend();
    let corner = plane.data[plane.at(0, 0)];
    for xoff in 0..8 {
        for yoff in 0..8 {
            let got = run_filter(&plane, -16, -16, xoff, yoff, 4, 4, ReconFilter::Sixtap)?;
            assert!(
                got.iter().all(|&p| p == corner),
                "({xoff},{yoff}) must read the replicated corner {corner}"
            );
        }
    }
    Ok(())
}

#[test]
fn test_border_read_matches_direct_convolution() -> CodecResult<()> {
    // Straddling the edges: part border, part real pixels.
    let mut plane = TestPlane::new(32, 32, |x, y| ((x * 7 + y * 3) % 251) as u8);
    plane.extend();
    for (x, y) in [(-3i64, -3i64), (-5, 2), (2, -5), (30, 30), (-1, 29)] {
        for (xoff, yoff) in [(0, 0), (3, 4), (7, 7)] {
            assert_eq!(
                run_filter(&plane, x, y, xoff, yoff, 4, 4, ReconFilter::Sixtap)?,
                reference_conv(&plane, x, y, xoff, yoff, 4, 4, &SIXTAP_FILTERS, true),
                "at ({x},{y}) phase ({xoff},{yoff})"
            );
        }
    }
    Ok(())
}

#[test]
fn test_reads_up_to_the_border_edge_are_accepted() {
    let plane = TestPlane::new(32, 32, |_, _| 40);
    let border = BORDER as i64;
    // Tap window starting exactly at -BORDER is covered by replication.
    assert!(source_offset(&plane.src(), -border + 2, -border + 2, 4, 4).is_ok());
    assert!(source_offset(&plane.src(), -border + 1, -border + 2, 4, 4).is_err());
    assert!(source_offset(&plane.src(), -border + 2, -border + 1, 4, 4).is_err());
    // And the same at the bottom-right: last tap at width + BORDER - 1.
    let far = 32 + border - 4 - 3;
    assert!(source_offset(&plane.src(), far, far, 4, 4).is_ok());
    assert!(source_offset(&plane.src(), far + 1, far, 4, 4).is_err());
    assert!(source_offset(&plane.src(), far, far + 1, 4, 4).is_err());
}

#[test]
fn test_both_filters_accept_and_reject_the_same_rectangle() {
    // The bilinear kernels have zero taps in the outer positions, but
    // the reference decoder still runs them through `sixtap_2d` and so
    // still touches the full window; the bounds contract is therefore
    // identical for both tables.
    let mut plane = TestPlane::new(32, 32, |x, y| ((x + y) % 255) as u8);
    plane.extend();
    let border = BORDER as i64;
    for (x, y) in [
        (-border + 2, -border + 2),
        (-border + 1, 0),
        (0, -border + 1),
        (40, 40),
    ] {
        let six = run_filter(&plane, x, y, 3, 3, 4, 4, ReconFilter::Sixtap).is_ok();
        let bil = run_filter(&plane, x, y, 3, 3, 4, 4, ReconFilter::Bilinear).is_ok();
        assert_eq!(six, bil, "at ({x},{y})");
    }
}

#[test]
fn test_out_of_bounds_is_an_error_not_a_panic() {
    let plane = TestPlane::new(32, 32, |_, _| 7);
    for (x, y) in [
        (-64i64, 0i64),
        (0, -64),
        (200, 0),
        (0, 200),
        (-1_000_000, -1_000_000),
        (1_000_000, 1_000_000),
        (i32::MAX as i64, i32::MIN as i64),
    ] {
        assert!(
            matches!(
                source_offset(&plane.src(), x, y, 4, 4),
                Err(CodecError::InvalidBitstream(_))
            ),
            "({x},{y}) must be rejected"
        );
    }
}

#[test]
fn test_filter_predict_rejects_a_bad_source_offset_directly() {
    // `filter_predict` is reachable without `source_offset`, so it
    // repeats the flat bounds check.
    let plane = TestPlane::new(16, 16, |_, _| 3);
    let mut dst = vec![0u8; 16];
    for off in [0, plane.data.len() - 1] {
        assert!(matches!(
            SIX.predict(&mut dst, 4, &plane.data, plane.stride, off, 1, 1, 4, 4),
            Err(CodecError::InvalidBitstream(_))
        ));
    }
}

#[test]
fn test_caller_errors_are_reported() {
    let plane = TestPlane::new(32, 32, |_, _| 5);
    let off = plane.at(4, 4);
    let mut dst = vec![0u8; 32 * 32];
    let mut six = |w, h, xoff, yoff, ds, src_stride, off| {
        SIX.predict(&mut dst, ds, &plane.data, src_stride, off, xoff, yoff, w, h)
    };
    // (w, h, xoff, yoff, dst_stride): oversized block, zero dimension,
    // out-of-range fraction, too narrow a destination stride, and a zero
    // source stride -- all caller-side mistakes, not bitstream errors.
    for (w, h, xoff, yoff, ds, ss) in [
        (17usize, 4usize, 0usize, 0usize, 32usize, plane.stride),
        (4, 17, 0, 0, 32, plane.stride),
        (0, 4, 0, 0, 32, plane.stride),
        (4, 0, 0, 0, 32, plane.stride),
        (4, 4, 8, 0, 32, plane.stride),
        (4, 4, 0, 8, 32, plane.stride),
        (4, 4, 0, 0, 2, plane.stride),
        (4, 4, 0, 0, 32, 0),
    ] {
        assert!(
            matches!(
                six(w, h, xoff, yoff, ds, ss, off),
                Err(CodecError::Internal(_))
            ),
            "{w}x{h} ({xoff},{yoff}) dst stride {ds} src stride {ss}"
        );
    }
    // A destination that cannot hold the block.
    let mut tiny = vec![0u8; 3];
    assert!(matches!(
        SIX.predict(&mut tiny, 4, &plane.data, plane.stride, off, 0, 0, 4, 4),
        Err(CodecError::Internal(_))
    ));
    // The whole-pixel copy shares the dimension and bounds checks.
    let end = plane.data.len();
    assert!(matches!(
        full_pel_predict(&mut dst, 32, &plane.data, plane.stride, off, 0, 4),
        Err(CodecError::Internal(_))
    ));
    assert!(matches!(
        full_pel_predict(&mut dst, 32, &plane.data, plane.stride, end, 4, 4),
        Err(CodecError::InvalidBitstream(_))
    ));
}

// ---------------------------------------------------------------
// macroblock level
// ---------------------------------------------------------------

/// A reference surface (noise, borders replicated) and matching blank
/// reconstruction planes for an `mb_cols` x `mb_rows` frame.
fn mb_fixture(mb_cols: usize, mb_rows: usize) -> (RefSurface, super::super::Planes) {
    let Ok(mut refs) = RefSurface::new(mb_cols * 16, mb_rows * 16) else {
        unreachable!("valid dimensions")
    };
    let mut rng = Lcg(0x2468_ACE0);
    for row in 0..refs.aligned_height() {
        for col in 0..refs.aligned_width() {
            let i = refs.y_origin + row * refs.y_stride + col;
            refs.y[i] = rng.next();
        }
    }
    for row in 0..refs.aligned_uv_height() {
        for col in 0..refs.aligned_uv_width() {
            let i = refs.uv_origin + row * refs.uv_stride + col;
            refs.u[i] = rng.next();
            refs.v[i] = rng.next();
        }
    }
    refs.extend_borders();

    let y_stride = mb_cols * 16 + 2 * BORDER;
    let uv_stride = mb_cols * 8 + 2 * BORDER;
    let planes = super::super::Planes {
        y: vec![0u8; y_stride * (mb_rows * 16 + 2 * BORDER)],
        u: vec![0u8; uv_stride * (mb_rows * 8 + 2 * BORDER)],
        v: vec![0u8; uv_stride * (mb_rows * 8 + 2 * BORDER)],
        y_stride,
        uv_stride,
        y_origin: BORDER * y_stride + BORDER,
        uv_origin: BORDER * uv_stride + BORDER,
    };
    (refs, planes)
}

/// The luma / U / V planes of a reference surface as [`SrcPlane`]s.
fn ref_planes(refs: &RefSurface) -> [SrcPlane<'_>; 3] {
    fn uv<'a>(refs: &RefSurface, data: &'a [u8]) -> SrcPlane<'a> {
        SrcPlane {
            data,
            stride: refs.uv_stride,
            origin: refs.uv_origin,
            width: refs.aligned_uv_width(),
            height: refs.aligned_uv_height(),
        }
    }
    [
        SrcPlane {
            data: &refs.y,
            stride: refs.y_stride,
            origin: refs.y_origin,
            width: refs.aligned_width(),
            height: refs.aligned_height(),
        },
        uv(refs, &refs.u),
        uv(refs, &refs.v),
    ]
}

#[test]
fn test_predict_inter_mb_zero_mv_copies_the_reference() -> CodecResult<()> {
    let (refs, mut planes) = mb_fixture(3, 2);
    let mvs = [(0i16, 0i16); 16];
    predict_mb(
        &mut planes,
        &refs,
        (1, 1),
        ReconFilter::Sixtap,
        &mvs,
        Some((0, 0)),
    )?;
    assert_eq!(
        region(py(&planes), 16, 16, 16, 16),
        region(ry(&refs), 16, 16, 16, 16),
        "luma"
    );
    for (dst, src) in [(pu(&planes), ru(&refs)), (pv(&planes), rv(&refs))] {
        assert_eq!(region(dst, 8, 8, 8, 8), region(src, 8, 8, 8, 8), "chroma");
    }
    Ok(())
}

#[test]
fn test_predict_inter_mb_writes_only_its_own_macroblock() -> CodecResult<()> {
    let (refs, mut planes) = mb_fixture(3, 2);
    predict_mb(
        &mut planes,
        &refs,
        (1, 1),
        ReconFilter::Sixtap,
        &[(5i16, -3i16); 16],
        Some((5, -3)),
    )?;
    assert!(
        region(py(&planes), 0, 0, 16, 16).iter().all(|&p| p == 0),
        "macroblock (0,0) must be untouched"
    );
    Ok(())
}

#[test]
fn test_predict_inter_mb_whole_mb_matches_manual_blocks() -> CodecResult<()> {
    let (refs, mut planes) = mb_fixture(3, 3);
    let mv = (-11i16, 19i16);
    predict_mb(
        &mut planes,
        &refs,
        (1, 1),
        ReconFilter::Sixtap,
        &[mv; 16],
        Some(mv),
    )?;
    let src = ref_planes(&refs);
    // One 16x16 luma block at the macroblock vector.
    let mv32 = (i32::from(mv.0), i32::from(mv.1));
    assert_eq!(
        region(py(&planes), 16, 16, 16, 16),
        manual_block(&src[0], 16, 16, mv32, 16, 16, SIX)?,
        "luma"
    );
    // One 8x8 block per chroma plane at the derived vector.
    let cmv = chroma_mv_whole_mb(mv, false);
    for (i, dst) in [pu(&planes), pv(&planes)].into_iter().enumerate() {
        assert_eq!(
            region(dst, 8, 8, 8, 8),
            manual_block(&src[i + 1], 8, 8, cmv, 8, 8, SIX)?,
            "chroma plane {i}"
        );
    }
    Ok(())
}

#[test]
fn test_predict_inter_mb_splitmv_uses_per_subblock_vectors() -> CodecResult<()> {
    let (refs, mut planes) = mb_fixture(3, 3);
    let mut mvs = [(0i16, 0i16); 16];
    for (b, mv) in mvs.iter_mut().enumerate() {
        *mv = ((b as i16) - 8, 8 - (b as i16));
    }
    predict_mb(&mut planes, &refs, (1, 1), ReconFilter::Sixtap, &mvs, None)?;
    let src = ref_planes(&refs);
    for (b, mv) in mvs.iter().enumerate() {
        let (bx, by) = ((b % 4) * 4, (b / 4) * 4);
        let want = (i32::from(mv.0), i32::from(mv.1));
        assert_eq!(
            region(py(&planes), 16 + bx, 16 + by, 4, 4),
            manual_block(&src[0], 16 + bx, 16 + by, want, 4, 4, SIX)?,
            "luma sub-block {b}"
        );
    }
    Ok(())
}

#[test]
fn test_predict_inter_mb_splitmv_chroma_uses_the_group_average() -> CodecResult<()> {
    let (refs, mut planes) = mb_fixture(2, 2);
    let mut mvs = [(0i16, 0i16); 16];
    for (b, mv) in mvs.iter_mut().enumerate() {
        *mv = ((b as i16) * 3 - 16, 16 - (b as i16) * 2);
    }
    predict_mb(&mut planes, &refs, (1, 1), ReconFilter::Sixtap, &mvs, None)?;
    let src = ref_planes(&refs);
    for b in 0..4usize {
        let cmv = chroma_mv_split(&mvs, b, false)?;
        let (bx, by) = ((b % 2) * 4, (b / 2) * 4);
        for (i, dst) in [pu(&planes), pv(&planes)].into_iter().enumerate() {
            assert_eq!(
                region(dst, 8 + bx, 8 + by, 4, 4),
                manual_block(&src[i + 1], 8 + bx, 8 + by, cmv, 4, 4, SIX)?,
                "chroma plane {i} sub-block {b}"
            );
        }
    }
    Ok(())
}

#[test]
fn test_predict_inter_mb_full_pel_version_ignores_chroma_fractions() -> CodecResult<()> {
    let (refs, mut planes) = mb_fixture(2, 2);
    // A luma vector whose halved chroma vector has a non-zero fraction:
    // 9 -> 5 (fraction 5), masked to 0; -9 -> -5, masked to -8.
    let mv = (9i16, -9i16);
    predict_mb(
        &mut planes,
        &refs,
        (1, 1),
        ReconFilter::FullPel,
        &[mv; 16],
        Some(mv),
    )?;
    let cmv = chroma_mv_whole_mb(mv, true);
    assert_eq!((cmv.0 & 7, cmv.1 & 7), (0, 0));
    // With a whole-pixel chroma vector the prediction is a plain copy.
    let (sx, sy) = ((8 + (cmv.0 >> 3)) as usize, (8 + (cmv.1 >> 3)) as usize);
    assert_eq!(
        region(pu(&planes), 8, 8, 8, 8),
        region(ru(&refs), sx, sy, 8, 8)
    );
    Ok(())
}

#[test]
fn test_predict_inter_mb_rejects_vectors_beyond_the_border() {
    let (refs, mut planes) = mb_fixture(2, 2);
    // 40 pixels left of macroblock (0,0): past the 32-pixel margin.
    let mv = (-40i16 * 8, 0i16);
    assert!(matches!(
        predict_mb(
            &mut planes,
            &refs,
            (0, 0),
            ReconFilter::Sixtap,
            &[mv; 16],
            Some(mv)
        ),
        Err(CodecError::InvalidBitstream(_))
    ));
}

#[test]
fn test_predict_inter_mb_rejects_a_row_wrapping_vector() {
    // -40 pixels horizontally at a positive row lands on a *valid* flat
    // offset -- the previous row's right border -- so only the 2-D check
    // catches it. This is the regression a flat range check would miss.
    let (refs, mut planes) = mb_fixture(2, 2);
    let mv = (-40i16 * 8, 8i16 * 8);
    assert!(matches!(
        predict_mb(
            &mut planes,
            &refs,
            (0, 0),
            ReconFilter::Sixtap,
            &[mv; 16],
            Some(mv)
        ),
        Err(CodecError::InvalidBitstream(_))
    ));
}

#[test]
fn test_extreme_motion_vectors_never_panic() {
    let (refs, mut planes) = mb_fixture(2, 2);
    let extremes = [
        i16::MIN,
        i16::MIN + 1,
        -32_000,
        32_000,
        i16::MAX - 1,
        i16::MAX,
    ];
    for x in extremes {
        for y in extremes {
            let mvs = [(x, y); 16];
            for (filter, _) in FILTERS {
                // Whole-macroblock and SPLITMV forms: both must report
                // an error rather than panic or wrap.
                assert!(
                    predict_mb(&mut planes, &refs, (0, 0), filter, &mvs, Some((x, y))).is_err(),
                    "({x},{y}) {filter:?} whole-mb"
                );
                assert!(
                    predict_mb(&mut planes, &refs, (1, 1), filter, &mvs, None).is_err(),
                    "({x},{y}) {filter:?} splitmv"
                );
            }
        }
    }
}

#[test]
fn test_predict_inter_mb_rejects_a_macroblock_outside_the_frame() {
    let (refs, mut planes) = mb_fixture(2, 2);
    assert!(predict_mb(
        &mut planes,
        &refs,
        (9, 9),
        ReconFilter::Sixtap,
        &[(0i16, 0i16); 16],
        Some((0, 0))
    )
    .is_err());
}

#[test]
fn test_mv_tuple_order_is_x_then_y() -> CodecResult<()> {
    // A purely horizontal vector must move the prediction horizontally.
    let (refs, mut planes) = mb_fixture(2, 2);
    let mv = (8i16, 0i16); // one whole pixel to the right
    predict_mb(
        &mut planes,
        &refs,
        (0, 0),
        ReconFilter::Sixtap,
        &[mv; 16],
        Some(mv),
    )?;
    assert_eq!(
        region(py(&planes), 0, 0, 16, 16),
        region(ry(&refs), 1, 0, 16, 16),
        "a vector of (8, 0) must shift the prediction in x only"
    );
    Ok(())
}
