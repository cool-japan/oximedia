//! VP8 inter prediction: sub-pixel motion compensation (RFC 6386 §18).
//!
//! Given a reference surface ([`super::refs::RefSurface`]) and the motion
//! vectors for a macroblock, this module synthesises the macroblock's
//! prediction into the reconstruction planes. Residual addition (§14) and
//! loop filtering (§15) then finish the macroblock, exactly as for an intra
//! macroblock.
//!
//! # Sub-pixel interpolation
//!
//! Interpolation is two one-dimensional convolutions (RFC 6386 §18.3,
//! rfc6386.txt lines 6421-6431): a horizontal pass into an intermediate
//! buffer, then a vertical pass from it. Both passes run the same six-tap
//! arithmetic — `temp = (reference[-2] * filter[0]) + ... + (reference[3] *
//! filter[5]) + 64; temp >>= 7; output[c] = CLAMP_255(temp);` — the
//! vertical one stepping by the stride instead of by one (`sixtap_horiz`,
//! rfc6386.txt lines 11936-11967; `sixtap_vert`, lines 11970-12009). The
//! RFC's prose form is `interp()`, lines 6506-6523, whose comment names the
//! rounding: `return clamp255((a + 64) >> 7); /* round to nearest 8-bit
//! value */`, line 6521.
//!
//! `sixtap_2d` (rfc6386.txt lines 12012-12031) wires the two together:
//! `DECLARE_ALIGNED(16, unsigned char, temp[16*(16+5)]);`, then
//! `sixtap_horiz(temp, 16, reference - 2 * reference_stride,
//! reference_stride, cols, rows + 5, filters[mx]);` and `sixtap_vert(output,
//! output_stride, temp + 2 * 16, 16, cols, rows, filters[my]);`. So the
//! intermediate is **`h + 5` rows tall, starting two rows above the
//! block**, and the vertical pass starts two rows into it. Two consequences
//! this module depends on:
//!
//! * the intermediate is `unsigned char`, so the horizontal pass result is
//!   **clamped to `0..=255` before the vertical pass** — with the negative
//!   taps of the bicubic kernel that clamp is observable, and skipping it
//!   changes the output;
//! * `temp` is `16 * (16 + 5)` bytes, which caps a single call at a 16x16
//!   block. Larger requests are rejected rather than silently split.
//!
//! # Why there is no whole-pixel special case
//!
//! The reference decoder short-circuits whole-pixel vectors (`filter_block`,
//! rfc6386.txt lines 12046-12073: `if (!mv->raw) return reference;` and `if
//! (mx | my)`). This module instead runs *every* block through the same two
//! passes, because row 0 of both filter tables is the identity
//! `{0, 0, 128, 0, 0, 0}` and `(p * 128 + 64) >> 7 == p` for every `p` in
//! `0..=255`, with the clamp a no-op — so the unconditional path is
//! bit-exact with the short-circuit, and has no branch to get wrong
//! (`test_full_pel_predict_matches_filter_at_frac_zero` and
//! `test_identity_is_memcpy_*` pin this). The only difference is memory
//! *reads*: the unconditional path touches the full six-tap window even at
//! fraction zero. That is deliberate — it makes the bounds contract
//! uniform — and harmless, since the window reaches at most three pixels
//! outside the block and the reference surface carries a 32-pixel
//! replicated border.
//!
//! # Bounds and errors
//!
//! Every entry point computes the exact source rectangle *including* the
//! two-up/two-left and three-down/three-right tap reach and validates it
//! before touching memory; a violation is
//! [`CodecError::InvalidBitstream`], never a clamp and never a panic.
//!
//! # Motion vector units
//!
//! See [`Mv`]. Luma vectors are eighth-pel *luma* units; the derived chroma
//! vectors are eighth-pel *chroma* units, so both feed the same
//! `int = v >> 3` / `frac = v & 7` split.

//! Wave-2 package stub: populated by its implementation package (P3/P4/P5),
//! wired into the decode path and un-deadcoded at P6.
#![forbid(unsafe_code)]

use crate::error::{CodecError, CodecResult};

use super::refs::RefSurface;
use super::tables_inter::{BILINEAR_FILTERS, SIXTAP_FILTERS};
use super::BORDER;

/// A motion vector in **eighth-pel** units, ordered `(x, y)`.
///
/// `x` is the horizontal (column) component and `y` the vertical (row)
/// component, matching the reference decoder's `union mv { struct { int16
/// x, y; } d; }` and its use throughout `predict.c` (`reference += ((mv->d.y
/// >> 3) * stride) + (mv->d.x >> 3)`, rfc6386.txt line 12058).
///
/// Note that this is the opposite of libvpx's `MV { short row, col; }` and
/// of the *decode* order in RFC 6386 §17.2, where the vertical component is
/// read from the bitstream first. Whoever wires this module up must convert;
/// the ordering is checked by `test_mv_tuple_order_is_x_then_y`. The integer
/// and fractional parts are always split as `v >> 3` (arithmetic shift, so
/// it rounds towards minus infinity) and `v & 7` (always `0..=7`), for
/// negative vectors too.
pub(super) type Mv = (i16, i16);

/// Largest block dimension a single prediction call accepts.
///
/// Set by the reference decoder's intermediate buffer, `temp[16 * (16 + 5)]`
/// (rfc6386.txt line 12023).
const MAX_BLOCK: usize = 16;

/// Row length of the intermediate buffer (`sixtap_2d` passes a stride of 16,
/// rfc6386.txt lines 12025-12029).
const TEMP_STRIDE: usize = MAX_BLOCK;

/// Rows in the intermediate buffer: `h + 5` for the largest supported block.
const TEMP_ROWS: usize = MAX_BLOCK + 5;

/// Six-tap form of [`BILINEAR_FILTERS`].
///
/// The reference decoder stores the bilinear kernels zero-padded to six taps
/// (`bilinear_filters`, rfc6386.txt lines 11093-11104) and runs them through
/// exactly the same `sixtap_2d` code as the bicubic kernels — only the table
/// pointer changes (`ctx->subpixel_filters`, rfc6386.txt lines 12683-12686).
/// This module does the same, so there is one filter core, parameterised by
/// table. [`BILINEAR_FILTERS`] stores just the two non-zero taps; they land
/// at indices 2 and 3, the `p[0]` and `p[1]` positions.
const BILINEAR_TAPS: [[i32; 6]; 8] = expand_bilinear_taps();

/// Zero-pads [`BILINEAR_FILTERS`] into the six-tap layout.
const fn expand_bilinear_taps() -> [[i32; 6]; 8] {
    let mut out = [[0i32; 6]; 8];
    let mut phase = 0;
    while phase < 8 {
        out[phase][2] = BILINEAR_FILTERS[phase][0];
        out[phase][3] = BILINEAR_FILTERS[phase][1];
        phase += 1;
    }
    out
}

/// The reconstruction (sub-pixel interpolation) filter a frame uses,
/// selected by the 3-bit version number in the frame tag.
///
/// RFC 6386 §9.1 tabulates the profiles (rfc6386.txt lines 1690-1703):
/// version 0 = Bicubic / Normal, 1 = Bilinear / Simple, 2 = Bilinear /
/// None, 3 = None / None, "Other | Reserved for future use".
///
/// The loop-filter column is informational: the same section states that the
/// "simple" and "none" settings "[n]either affect the decoding process. In
/// decoding, the only loop filter settings that matter are those in the
/// frame header" (rfc6386.txt lines 1706-1713). So this enum carries the
/// reconstruction-filter column only.
///
/// # What version 3 actually does
///
/// The §9.1 label "None" describes the *encoder* profile — version 3 streams
/// only ever code whole-pixel luma vectors. The reference decoder does not
/// implement a third filter: it selects the bilinear table for every
/// non-zero version — `if (ctx->frame_hdr.version) ctx->subpixel_filters =
/// bilinear_filters; else ctx->subpixel_filters = sixtap_filters;`
/// (rfc6386.txt lines 12683-12686) — and treats version 3 specially only
/// when deriving *chroma* vectors, where
/// it truncates the fraction (`int full_pixel = ctx->frame_hdr.version == 3;`
/// then `uvmv.d.x &= ~7`, rfc6386.txt lines 12310, 12331-12340). Luma
/// vectors are never masked.
///
/// [`ReconFilter::FullPel`] therefore predicts through the bilinear core and
/// masks chroma vectors, which is bit-exact with the reference decoder for
/// every stream, well-formed or not. For a well-formed version-3 stream all
/// fractions are zero and that degenerates to the plain whole-pixel copy
/// [`full_pel_predict`] performs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum ReconFilter {
    /// Version 0: the six-tap "bicubic" kernels ([`SIXTAP_FILTERS`]).
    Sixtap,
    /// Versions 1 and 2: the bilinear kernels ([`BILINEAR_FILTERS`]).
    Bilinear,
    /// Version 3: bilinear kernels plus whole-pixel chroma vectors.
    FullPel,
}

impl ReconFilter {
    /// Selects the filter for a frame-tag version number.
    ///
    /// # Errors
    /// Versions above 3 are "[r]eserved for future use" (RFC 6386 §9.1) and
    /// are rejected.
    pub(super) fn from_version(version: u8) -> CodecResult<Self> {
        match version {
            0 => Ok(Self::Sixtap),
            1 | 2 => Ok(Self::Bilinear),
            3 => Ok(Self::FullPel),
            other => Err(CodecError::InvalidBitstream(format!(
                "VP8: unsupported bitstream version {other} (RFC 6386 §9.1 defines 0-3)"
            ))),
        }
    }

    /// The tap table this filter convolves with.
    fn taps(self) -> &'static [[i32; 6]; 8] {
        match self {
            Self::Sixtap => &SIXTAP_FILTERS,
            Self::Bilinear | Self::FullPel => &BILINEAR_TAPS,
        }
    }

    /// Whether derived chroma vectors are truncated to whole pixels
    /// (version 3 only).
    pub(super) fn full_pel_chroma(self) -> bool {
        matches!(self, Self::FullPel)
    }

    /// Predicts one `w` x `h` block with this filter.
    ///
    /// `dst` is written from index 0, `dst_stride` apart per row; pass a
    /// suffix slice (`&mut plane[block_offset..]`) to write into the middle
    /// of a plane. `src_off` is the index of the block's top-left source
    /// pixel inside `src`, with the integer part of the motion vector
    /// already applied; `xoff` and `yoff` are its eighth-pel fractional
    /// parts.
    ///
    /// Runs the unconditional two-pass filter described in the module docs,
    /// so `xoff == 0 && yoff == 0` is an exact copy through the identity
    /// kernel rather than a special case.
    ///
    /// # Errors
    /// * [`CodecError::InvalidBitstream`] when the source rectangle — the
    ///   block grown by 2 pixels up/left and 3 pixels down/right for the tap
    ///   reach — does not fit inside `src`. Callers that know the plane's
    ///   2-D geometry should validate with [`source_offset`] first, which
    ///   also catches a rectangle that wraps between rows.
    /// * [`CodecError::Internal`] for a caller-side mistake: a block larger
    ///   than 16x16, a zero dimension, a fraction outside `0..=7`, or a
    ///   `dst` too small for `w` x `h`.
    pub(super) fn predict(
        self,
        dst: &mut [u8],
        dst_stride: usize,
        src: &[u8],
        src_stride: usize,
        src_off: usize,
        xoff: usize,
        yoff: usize,
        w: usize,
        h: usize,
    ) -> CodecResult<()> {
        let taps = self.taps();
        filter_predict(
            dst, dst_stride, src, src_stride, src_off, xoff, yoff, w, h, taps,
        )
    }
}

/// The shared two-pass filter core, parameterised by tap table.
fn filter_predict(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    src_stride: usize,
    src_off: usize,
    xoff: usize,
    yoff: usize,
    w: usize,
    h: usize,
    taps: &[[i32; 6]; 8],
) -> CodecResult<()> {
    check_block_dims(w, h)?;
    check_dst(dst, dst_stride, w, h)?;
    if xoff >= 8 || yoff >= 8 {
        return Err(internal("sub-pixel fraction outside 0..=7"));
    }
    if src_stride == 0 {
        return Err(internal("zero source stride"));
    }

    // --- exact source rectangle, tap reach included -------------------
    // Rows -2 ..= h+2 (that is h + 5 rows) and columns -2 ..= w+2 (w + 5
    // columns) relative to the block origin: `sixtap_2d` starts the
    // horizontal pass at `reference - 2 * reference_stride` and runs it for
    // `rows + 5` rows (rfc6386.txt lines 12025-12027), and each output
    // sample reads `reference[-2] ..= reference[3]` (lines 11945-11951).
    let stride = src_stride as i64;
    let off = src_off as i64;
    let first = off - 2 * stride - 2;
    let last = off + (h as i64 + 2) * stride + (w as i64 + 2);
    if first < 0 || last >= src.len() as i64 {
        return Err(oob("sub-pixel prediction"));
    }
    // `first >= 0` proves this does not underflow.
    let base = src_off - 2 * src_stride - 2;

    let h_taps = taps.get(xoff).ok_or_else(|| internal("bad x fraction"))?;
    let v_taps = taps.get(yoff).ok_or_else(|| internal("bad y fraction"))?;

    // --- pass 1: horizontal, h + 5 rows into the intermediate buffer ---
    // The intermediate is `unsigned char` in the reference decoder, so the
    // clamp here is part of the arithmetic, not a safety net.
    let mut temp = [0u8; TEMP_STRIDE * TEMP_ROWS];
    for row in 0..h + 5 {
        let start = base + row * src_stride;
        let src_row = src
            .get(start..start + w + 5)
            .ok_or_else(|| oob("horizontal filter row"))?;
        let temp_row = temp
            .get_mut(row * TEMP_STRIDE..row * TEMP_STRIDE + w)
            .ok_or_else(|| internal("intermediate row out of range"))?;
        for (col, out) in temp_row.iter_mut().enumerate() {
            let mut sum = 0i32;
            for (tap, coeff) in h_taps.iter().enumerate() {
                sum += i32::from(src_row[col + tap]) * coeff;
            }
            *out = round_shift_clamp(sum);
        }
    }

    // --- pass 2: vertical, from two rows into the intermediate buffer ---
    for row in 0..h {
        let dst_row = dst
            .get_mut(row * dst_stride..row * dst_stride + w)
            .ok_or_else(|| internal("destination row out of range"))?;
        for (col, out) in dst_row.iter_mut().enumerate() {
            let mut sum = 0i32;
            for (tap, coeff) in v_taps.iter().enumerate() {
                // `temp + 2 * 16` with taps at -2 ..= +3 is rows
                // `row ..= row + 5` of the intermediate buffer.
                sum += i32::from(temp[(row + tap) * TEMP_STRIDE + col]) * coeff;
            }
            *out = round_shift_clamp(sum);
        }
    }
    Ok(())
}

/// `CLAMP_255((sum + 64) >> 7)` — the rounding shared by both passes
/// (rfc6386.txt lines 11945-11953 and 6521).
///
/// The shift is arithmetic, so a negative sum (possible with the bicubic
/// kernel's negative taps) floors before the clamp, exactly as the C `int`
/// right shift in the reference decoder does.
fn round_shift_clamp(sum: i32) -> u8 {
    let v = (sum + 64) >> 7;
    v.clamp(0, 255) as u8
}

/// Validates a requested block size against the intermediate buffer.
fn check_block_dims(w: usize, h: usize) -> CodecResult<()> {
    if w == 0 || h == 0 {
        return Err(internal("zero block dimension"));
    }
    if w > MAX_BLOCK || h > MAX_BLOCK {
        return Err(internal("block larger than 16x16"));
    }
    Ok(())
}

/// Validates that `dst` can hold a `w` x `h` block at `dst_stride`.
fn check_dst(dst: &[u8], dst_stride: usize, w: usize, h: usize) -> CodecResult<()> {
    if dst_stride < w {
        return Err(internal("destination stride smaller than the block"));
    }
    let needed = (h - 1) * dst_stride + w;
    if dst.len() < needed {
        return Err(internal("destination buffer too small"));
    }
    Ok(())
}

/// Builds the out-of-bounds error. Never a clamp: a reference read outside
/// the surface means the bitstream asked for pixels that do not exist.
fn oob(what: &str) -> CodecError {
    CodecError::InvalidBitstream(format!("VP8: {what} reads outside the reference surface"))
}

/// Builds the caller-error variant.
fn internal(what: &str) -> CodecError {
    CodecError::Internal(format!("VP8 motion compensation: {what}"))
}

/// A reference plane and the geometry needed to bounds-check reads into it.
///
/// `width` / `height` are the **macroblock-aligned** extent; the surface
/// additionally carries [`BORDER`] replicated pixels on every side, and
/// reads may land anywhere in that margin.
#[derive(Clone, Copy)]
pub(super) struct SrcPlane<'a> {
    /// Whole plane buffer, borders included.
    pub(super) data: &'a [u8],
    /// Row length, borders included.
    pub(super) stride: usize,
    /// Offset of pixel (0,0).
    pub(super) origin: usize,
    /// Macroblock-aligned plane width.
    pub(super) width: usize,
    /// Macroblock-aligned plane height.
    pub(super) height: usize,
}

/// A destination plane being reconstructed.
pub(super) struct DstPlane<'a> {
    /// Whole plane buffer, borders included.
    pub(super) data: &'a mut [u8],
    /// Row length, borders included.
    pub(super) stride: usize,
    /// Offset of pixel (0,0).
    pub(super) origin: usize,
}

/// Validates a 2-D source rectangle against a bordered plane and returns the
/// flat offset of its top-left pixel.
///
/// `x` / `y` are the block origin in plane coordinates (they may be
/// negative). The rectangle checked is the block grown by the tap reach: 2
/// pixels up and left, 3 pixels down and right — the same window the
/// reference decoder tests before falling back to its edge emulation,
/// `if (x < 2 || x + b_w - 1 + 3 >= w || y < 2 || y + b_h - 1 + 3 >= h)`
/// (rfc6386.txt lines 12256-12258).
///
/// Where the reference decoder then *synthesises* the missing pixels with
/// `build_mc_border` (rfc6386.txt lines 12259-12266), this decoder relies on
/// the surface's [`BORDER`]-pixel replicated margin, which holds exactly the
/// pixels `build_mc_border` would have produced, and rejects anything
/// further out. Non-SPLITMV vectors are clamped during decode to at most 16
/// pixels beyond the frame edge (RFC 6386 §18.1, rfc6386.txt lines
/// 6367-6376), so with the tap reach they stay within 19 of 32. SPLITMV
/// sub-vectors are *not* re-clamped (same section, lines 6372-6376), so a
/// pathological stream can point further out than the border covers; that
/// is reported honestly instead of being silently clamped, and widening the
/// margin or adding `build_mc_border` emulation would be the way to accept
/// it.
///
/// # Errors
/// [`CodecError::InvalidBitstream`] when the rectangle leaves the allocated
/// plane, horizontally or vertically.
pub(super) fn source_offset(
    plane: &SrcPlane<'_>,
    x: i64,
    y: i64,
    w: usize,
    h: usize,
) -> CodecResult<usize> {
    check_block_dims(w, h)?;
    let border = BORDER as i64;
    let (x_lo, x_hi) = (x - 2, x + w as i64 - 1 + 3);
    let (y_lo, y_hi) = (y - 2, y + h as i64 - 1 + 3);
    if x_lo < -border
        || y_lo < -border
        || x_hi >= plane.width as i64 + border
        || y_hi >= plane.height as i64 + border
    {
        return Err(CodecError::InvalidBitstream(format!(
            "VP8: motion vector points outside the {}x{} reference surface \
             (+/-{BORDER} border): needs x {x_lo}..={x_hi}, y {y_lo}..={y_hi}",
            plane.width, plane.height
        )));
    }
    // Every corner is inside the allocation, so the flat offset is too.
    let off = plane.origin as i64 + y * plane.stride as i64 + x;
    if off < 0 || off >= plane.data.len() as i64 {
        return Err(oob("motion vector"));
    }
    Ok(off as usize)
}

/// Derives the chroma motion vector of a whole-macroblock (non-SPLITMV)
/// prediction from its luma vector.
///
/// Chroma pixels have twice the diameter of luma pixels, so the same
/// displacement is half as many chroma pixels; in eighth-pel units that is a
/// halving, rounded **away from zero** (RFC 6386 §18.1, rfc6386.txt lines
/// 6304-6310: "the stored luma motion vectors are all doubled"). The
/// reference decoder writes it as `uvmv.d.x = (uvmv.d.x + 1 + (uvmv.d.x >>
/// 31) * 2) / 2;`, and the same for `d.y` (rfc6386.txt lines 12455-12457,
/// and identically at 12335-12337).
///
/// `v >> 31` is `-1` for a negative `v` and `0` otherwise, so the numerator
/// is `v + 1` when `v >= 0` and `v - 1` when `v < 0`, and C's division
/// truncates towards zero: half-way values round away from zero. libvpx
/// spells the same thing `v += 1 | (v >> 31); v /= 2;` — `1 | -1 == -1`,
/// `1 | 0 == 1` — which this function reproduces.
///
/// When `full_pel` (version 3) the fraction is then truncated,
/// `uvmv.d.x &= ~7` (rfc6386.txt lines 12337-12340; RFC 6386 §18.1 states
/// the same rule in prose at lines 6353-6362, "if the version number in the
/// frame tag specifies only full-pel chroma motion vectors, then the
/// fractional parts of both components of the vector are truncated to
/// zero").
///
/// The result is in eighth-pel **chroma** units.
pub(super) fn chroma_mv_whole_mb(mv: Mv, full_pel: bool) -> (i32, i32) {
    let half = |v: i16| -> i32 {
        let v = i32::from(v);
        // `1 | (v >> 31)`: +1 for v >= 0, -1 for v < 0.
        let rounded = v + (1 | (v >> 31));
        // Rust's `/` truncates towards zero, like C's.
        rounded / 2
    };
    apply_full_pel((half(mv.0), half(mv.1)), full_pel)
}

/// Derives the chroma motion vector of one 4x4 chroma sub-block of a
/// SPLITMV macroblock, `chroma_b` in `0..4` in raster order.
///
/// Each chroma sub-block covers the visible area of four luma sub-blocks:
/// chroma 0 covers luma {0,1,4,5}, chroma 1 {2,3,6,7}, chroma 2
/// {8,9,12,13}, chroma 3 {10,11,14,15} (RFC 6386 §18.1, rfc6386.txt lines
/// 6313-6320; the reference decoder passes the top-left index of each group,
/// `calculate_chroma_splitmv(mbi, 0/2/8/10, full_pixel)`, lines 12469-12472).
///
/// The four vectors are summed and divided by 8 — not 4, "because chroma
/// pixels have twice the diameter of luma pixels" (RFC 6386 §18.1's `avg()`,
/// rfc6386.txt lines 6333-6348) — rounding away from zero: `temp =
/// mvs[b].d.x + mvs[b+1].d.x + mvs[b+4].d.x + mvs[b+5].d.x; if (temp < 0)
/// temp -= 4; else temp += 4; mv.d.x = temp / 8;`
/// (`calculate_chroma_splitmv`, rfc6386.txt lines 12103-12143), which is the
/// same value as the RFC's prose form
/// `s >= 0 ? (s + 4) >> 3 : -((-s + 4) >> 3)`, and libvpx's
/// `temp += 4 + ((temp >> 31) * 8); mv = (temp / 8) & fullpixel_mask`.
///
/// The result is in eighth-pel **chroma** units.
///
/// # Errors
/// [`CodecError::Internal`] when `chroma_b` is not in `0..4`.
pub(super) fn chroma_mv_split(
    luma_mvs: &[Mv; 16],
    chroma_b: usize,
    full_pel: bool,
) -> CodecResult<(i32, i32)> {
    // Top-left luma sub-block of each 2x2 group.
    let top_left = match chroma_b {
        0 => 0usize,
        1 => 2,
        2 => 8,
        3 => 10,
        _ => return Err(internal("chroma sub-block index outside 0..4")),
    };
    let group = [top_left, top_left + 1, top_left + 4, top_left + 5];
    let mut sum_x = 0i32;
    let mut sum_y = 0i32;
    for b in group {
        let mv = luma_mvs
            .get(b)
            .ok_or_else(|| internal("luma sub-block index outside 0..16"))?;
        sum_x += i32::from(mv.0);
        sum_y += i32::from(mv.1);
    }
    let eighth = |s: i32| -> i32 {
        // `if (temp < 0) temp -= 4; else temp += 4; temp / 8`
        let biased = if s < 0 { s - 4 } else { s + 4 };
        biased / 8
    };
    Ok(apply_full_pel((eighth(sum_x), eighth(sum_y)), full_pel))
}

/// Truncates a chroma vector's fractional part for version-3 streams
/// (`mv.d.x &= ~7`).
fn apply_full_pel(mv: (i32, i32), full_pel: bool) -> (i32, i32) {
    if full_pel {
        (mv.0 & !7, mv.1 & !7)
    } else {
        mv
    }
}

/// Predicts one block of one plane at a sub-pixel motion vector.
///
/// `px` / `py` are the block's position in the destination plane; the source
/// is the same position in `src` displaced by `mv >> 3`, filtered at
/// `mv & 7` (`reference += ((mv->d.y >> 3) * stride) + (mv->d.x >> 3)`,
/// rfc6386.txt lines 12056-12058).
///
/// # Errors
/// [`CodecError::InvalidBitstream`] when the motion vector points outside
/// the reference surface's border; [`CodecError::Internal`] when the
/// destination block does not fit the plane.
fn predict_plane_block(
    dst: &mut DstPlane<'_>,
    src: &SrcPlane<'_>,
    px: usize,
    py: usize,
    mv: (i32, i32),
    w: usize,
    h: usize,
    filter: ReconFilter,
) -> CodecResult<()> {
    // Integer part floors (arithmetic shift) and the fraction is the
    // non-negative remainder, for negative vectors too.
    let int_x = (mv.0 >> 3) as i64;
    let int_y = (mv.1 >> 3) as i64;
    let xoff = (mv.0 & 7) as usize;
    let yoff = (mv.1 & 7) as usize;

    let src_x = px as i64 + int_x;
    let src_y = py as i64 + int_y;
    let src_off = source_offset(src, src_x, src_y, w, h)?;

    let dst_stride = dst.stride;
    let dst_off = dst
        .origin
        .checked_add(
            py.checked_mul(dst_stride)
                .ok_or_else(|| internal("destination block offset overflow"))?,
        )
        .and_then(|o| o.checked_add(px))
        .ok_or_else(|| internal("destination block offset overflow"))?;
    let dst_slice = dst
        .data
        .get_mut(dst_off..)
        .ok_or_else(|| internal("destination block outside the plane"))?;

    filter.predict(
        dst_slice, dst_stride, src.data, src.stride, src_off, xoff, yoff, w, h,
    )
}

/// Predicts a whole inter macroblock — 16 luma and 2 x 4 chroma sub-blocks —
/// into the reconstruction planes (RFC 6386 §18).
///
/// * `whole_mb` is `Some(mv)` for every non-SPLITMV mode (NEARESTMV, NEARMV,
///   ZEROMV, NEWMV), which share one vector for the whole macroblock, and
///   `None` for SPLITMV.
/// * `luma_mvs` holds the 16 per-4x4 vectors in raster order; it is read
///   only for SPLITMV (for luma) and for the SPLITMV chroma derivation.
/// * Both are eighth-pel [`Mv`]s — `(x, y)`, horizontal first.
///
/// # Block decomposition
///
/// For a non-SPLITMV macroblock this issues one 16x16 luma and one 8x8
/// prediction per chroma plane, where the reference decoder issues sixteen
/// and four 4x4 predictions respectively (`predict_inter`, rfc6386.txt lines
/// 12481-12513). The results are identical: each output pixel depends only
/// on its own tap window, the intermediate clamp is per-pixel, and the
/// vertical pass consumes exactly the intermediate rows the horizontal pass
/// wrote for the same columns — so splitting or joining the block cannot
/// change a value. `test_16x16_equals_sixteen_4x4` and
/// `test_8x8_equals_four_4x4` check that on real data rather than trusting
/// the argument. SPLITMV predicts per 4x4 sub-block; neighbouring
/// sub-blocks that share a vector could be fused into one 8x8 / 8x4 / 16x8
/// call by the same argument — a pure speed optimisation, deliberately left
/// out of this first form.
///
/// # Errors
/// [`CodecError::InvalidBitstream`] when any vector points outside the
/// reference surface's border; [`CodecError::Internal`] when the macroblock
/// lies outside the destination planes.
pub(super) fn predict_inter_mb(
    planes: &mut super::Planes,
    refs: &RefSurface,
    mb_x: usize,
    mb_y: usize,
    filter: ReconFilter,
    luma_mvs: &[Mv; 16],
    whole_mb: Option<Mv>,
) -> CodecResult<()> {
    let full_pel = filter.full_pel_chroma();
    let (luma_x, luma_y) = (mb_x * 16, mb_y * 16);
    let (chroma_x, chroma_y) = (mb_x * 8, mb_y * 8);

    // --- luma ---------------------------------------------------------
    {
        let src = SrcPlane {
            data: &refs.y,
            stride: refs.y_stride,
            origin: refs.y_origin,
            width: refs.aligned_width(),
            height: refs.aligned_height(),
        };
        let mut dst = DstPlane {
            data: &mut planes.y,
            stride: planes.y_stride,
            origin: planes.y_origin,
        };
        match whole_mb {
            Some(mv) => {
                let mv = (i32::from(mv.0), i32::from(mv.1));
                predict_plane_block(&mut dst, &src, luma_x, luma_y, mv, 16, 16, filter)?;
            }
            None => {
                for (b, sub) in luma_mvs.iter().enumerate() {
                    let mv = (i32::from(sub.0), i32::from(sub.1));
                    let x = luma_x + (b % 4) * 4;
                    let y = luma_y + (b / 4) * 4;
                    predict_plane_block(&mut dst, &src, x, y, mv, 4, 4, filter)?;
                }
            }
        }
    }

    // --- chroma -------------------------------------------------------
    // U and V take the same vectors, so derive them once. A non-SPLITMV
    // macroblock has one vector for the whole 8x8; SPLITMV has one per 4x4.
    let cmvs = match whole_mb {
        Some(mv) => [chroma_mv_whole_mb(mv, full_pel); 4],
        None => {
            let mut cmvs = [(0i32, 0i32); 4];
            for (b, out) in cmvs.iter_mut().enumerate() {
                *out = chroma_mv_split(luma_mvs, b, full_pel)?;
            }
            cmvs
        }
    };
    for plane in 0..2 {
        let (src_data, dst_data) = if plane == 0 {
            (refs.u.as_slice(), planes.u.as_mut_slice())
        } else {
            (refs.v.as_slice(), planes.v.as_mut_slice())
        };
        let src = SrcPlane {
            data: src_data,
            stride: refs.uv_stride,
            origin: refs.uv_origin,
            width: refs.aligned_uv_width(),
            height: refs.aligned_uv_height(),
        };
        let mut dst = DstPlane {
            data: dst_data,
            stride: planes.uv_stride,
            origin: planes.uv_origin,
        };
        if whole_mb.is_some() {
            predict_plane_block(&mut dst, &src, chroma_x, chroma_y, cmvs[0], 8, 8, filter)?;
        } else {
            for (b, cmv) in cmvs.iter().enumerate() {
                let x = chroma_x + (b % 2) * 4;
                let y = chroma_y + (b / 2) * 4;
                predict_plane_block(&mut dst, &src, x, y, *cmv, 4, 4, filter)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mc_tests.rs"]
mod tests;
