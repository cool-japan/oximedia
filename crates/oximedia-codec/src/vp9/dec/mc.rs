//! VP9 motion-compensation interpolation — an exact port of libvpx
//! `vpx_dsp/vpx_convolve.c` (8-bit build): the ten `vpx_convolve*_c`
//! functions unscaled VP9 inter prediction actually reaches, unified behind
//! a single entry point, [`convolve`].
//!
//! # Provenance
//!
//! Transcribed verbatim from the libvpx reference implementation, tag
//! **v1.15.2** (commit `d168454`) — the same tag [`super::tables_inter`]
//! cites:
//!
//! * `vpx_dsp/vpx_convolve.c` — the static `convolve_horiz`/`convolve_vert`/
//!   `convolve_avg_horiz`/`convolve_avg_vert` cores and the ten
//!   `vpx_convolve{,8,8_avg,8_horiz,8_avg_horiz,8_vert,8_avg_vert,_copy,_avg}_c`
//!   entry points ([`convolve_copy`], [`convolve_avg`], [`convolve_horiz`],
//!   [`convolve_avg_horiz`], [`convolve_vert`], [`convolve_avg_vert`],
//!   [`convolve_2d`], [`convolve_2d_avg`] below).
//! * `vpx_dsp/vpx_filter.h` — `FILTER_BITS 7`, `SUBPEL_TAPS 8` ([`FILTER_BITS`],
//!   [`SUBPEL_TAPS`]; `SUBPEL_BITS`/`SUBPEL_MASK`/`SUBPEL_SHIFTS` are not
//!   needed here, see "API shape" below).
//! * `vpx_dsp/vpx_dsp_common.h` — `clip_pixel` ([`clip_pixel`]).
//! * `vpx_ports/mem.h` — `ROUND_POWER_OF_TWO(value, n)` ([`round_pow2`]).
//! * `vp9/common/vp9_scale.c:74-84` and `vp9/common/vp9_reconinter.h`'s
//!   `inter_predictor` — the `predict[subpel_x != 0][subpel_y != 0][ref]`
//!   dispatch table this module's [`convolve`] reproduces (see "Why a
//!   4-way dispatch" below).
//! * `vp9/common/vp9_reconinter.c`'s `vp9_build_inter_predictor` —
//!   `subpel_x = mv.col & SUBPEL_MASK`, confirming a subpel fraction of
//!   zero is what selects the copy/vert-only/horiz-only legs.
//!
//! # API shape vs. libvpx
//!
//! libvpx's `vpx_convolve8_c(..., const InterpKernel *filter, int x0_q4,
//! int x_step_q4, int y0_q4, int y_step_q4, ...)` threads a whole 16-phase
//! kernel family plus a phase/step pair through every call, because a
//! *scaled* reference frame can select a different phase for every output
//! column/row (`x_q4 = x0_q4 + x * x_step_q4`, phase `x_q4 & SUBPEL_MASK`).
//! VP9 inter prediction from an **unscaled** reference — `x_step_q4 ==
//! y_step_q4 == 16`, the only case this module implements; scaled
//! prediction is libvpx's separate `vpx_scaled_2d` family (package P15's
//! job, not this one's) — always resolves to exactly *one* phase per axis:
//! since `16 * x` is a multiple of `SUBPEL_SHIFTS` (16) for every `x`,
//! `(x0_q4 + 16 * x) & 15 == x0_q4 & 15` regardless of `x`, so the whole
//! block is filtered with a single 8-tap row per axis. [`convolve`]
//! therefore takes that already-selected row directly
//! (`Option<&[i16; SUBPEL_TAPS]>`) instead of a 16-row family plus a phase
//! index — the phase-stepping arithmetic (`x_q4 >> SUBPEL_BITS`, `& SUBPEL_MASK`)
//! collapses away entirely, which is why `SUBPEL_BITS`/`SUBPEL_MASK`/
//! `SUBPEL_SHIFTS` do not appear in this file. Callers (from package P10
//! onward) select the row with `FILTER_KERNELS[filter][subpel]` from
//! [`super::tables_inter`] and pass it in; `None` stands in for a subpel
//! fraction of zero.
//!
//! # Why a 4-way dispatch, not "always run the 2-D path"
//!
//! `None` for an axis is not merely "kernel row 0 for that axis" — even
//! though row 0 of every real filter family *is* the identity
//! `[0, 0, 0, 128, 0, 0, 0, 0]`, and running it through the general 2-D
//! machinery would be numerically a no-op (`clip_pixel(ROUND_POWER_OF_TWO(128
//! * v, 7)) == v` exactly, for every `v` in `0..=255`; see
//! `separable_consistency_*` in the tests). libvpx dispatches on
//! `(subpel_x != 0, subpel_y != 0, ref)` to one of four *different*
//! functions (`vp9_scale.c:74-84`, unscaled block) —
//!
//! | `subpel_x != 0` | `subpel_y != 0` | function |
//! |---|---|---|
//! | false | false | [`convolve_copy`] / [`convolve_avg`] |
//! | false | true  | [`convolve_vert`] / [`convolve_avg_vert`] |
//! | true  | false | [`convolve_horiz`] / [`convolve_avg_horiz`] |
//! | true  | true  | [`convolve_2d`] / [`convolve_2d_avg`] |
//!
//! — because the *cost* differs: a horizontal-only prediction reads exactly
//! `h` rows (no vertical tap reach at all), a vertical-only one reads
//! exactly `w` columns, and only the full 2-D path needs both extensions
//! plus the intermediate buffer. Collapsing to "always 2-D" would demand a
//! taller/wider source rectangle than the block actually needs — border
//! rows/columns a horizontal- or vertical-only prediction has no reason to
//! touch, and in `#![deny(unsafe_code)]` Rust with plain slice indexing
//! that is the difference between an in-bounds read and a panic near a
//! frame edge, not just wasted work. [`convolve`] therefore mirrors
//! libvpx's four-way split exactly, keyed on `(x_kernel.is_some(),
//! y_kernel.is_some())`.
//!
//! # The 8-bit intermediate is load-bearing
//!
//! [`convolve_2d`]'s horizontal pass writes into a `u8` temp buffer
//! (`vpx_convolve8_c`'s `uint8_t temp[64 * 135]`) — **clipped to `0..=255`
//! before the vertical pass ever sees it.** This is VP9 (`vpx_dsp`), not
//! AV1 (`aom_dsp`), which carries a wider intermediate; skipping the clip
//! here changes the output whenever the horizontal sum overshoots 255 or
//! undershoots 0, which happens routinely with real (non-`EIGHTTAP`-only)
//! content — see `intermediate_clip_is_observable_on_overshoot`/
//! `..._on_undershoot`.
//!
//! # Layout
//!
//! Arithmetic primitives ([`round_pow2`], [`clip_pixel`], [`avg_pixel`],
//! [`tap_sum`]), then one private function per libvpx entry point listed
//! above, then [`convolve`] itself, which only dispatches.

/// Number of taps in an interpolation kernel.
///
/// libvpx `vpx_dsp/vpx_filter.h:27`: `SUBPEL_TAPS 8`. Every kernel
/// [`convolve`] accepts has exactly this many entries.
const SUBPEL_TAPS: usize = 8;

/// Fixed-point precision of interpolation kernels.
///
/// libvpx `vpx_dsp/vpx_filter.h:22`: `FILTER_BITS 7`. A valid kernel row
/// sums to `1 << FILTER_BITS == 128`; [`round_pow2`] is always called with
/// this shift for a filtered sample.
const FILTER_BITS: u32 = 7;

/// libvpx's constant tap-center offset, `SUBPEL_TAPS / 2 - 1 == 3`
/// (`vpx_dsp/vpx_convolve.c`'s `convolve_horiz`/`convolve_vert`: `src -=
/// SUBPEL_TAPS / 2 - 1;` / `src -= src_stride * (SUBPEL_TAPS / 2 - 1);`).
/// An output sample at position `p` reads taps `p - TAP_OFFSET ..= p +
/// (SUBPEL_TAPS - 1 - TAP_OFFSET)`, i.e. `p - 3 ..= p + 4`.
const TAP_OFFSET: usize = SUBPEL_TAPS / 2 - 1;

/// Row length of [`convolve_2d`]'s intermediate buffer.
///
/// libvpx fixes this at 64 (`vpx_convolve8_c`'s `uint8_t temp[64 * 135]`,
/// passed as the horizontal pass's `dst_stride` and the vertical pass's
/// `src_stride`) — tied to VP9's maximum block edge (64, one superblock),
/// not to the block's actual width `w`. This module allocates only
/// `TEMP_STRIDE * rows_needed` bytes rather than libvpx's oversized fixed
/// array (whose extra headroom budgets for the *scaled* prediction path
/// that also funnels through `vpx_convolve8_c`, per its own comment —
/// irrelevant here, this module is unscaled-only); every byte the vertical
/// pass reads was written by the horizontal pass first, so the smaller
/// buffer is behaviorally identical.
const TEMP_STRIDE: usize = 64;

/// libvpx `ROUND_POWER_OF_TWO(value, n)` (`vpx_ports/mem.h`):
/// `(value + (1 << (n - 1))) >> n`. Rust's `>>` on `i32` is an arithmetic
/// (sign-extending) shift, matching C's near-universal behavior for the
/// signed right shifts this port relies on (negative filtered sums are
/// routine with real kernels).
#[inline]
fn round_pow2(value: i32, n: u32) -> i32 {
    (value + (1 << (n - 1))) >> n
}

/// libvpx `clip_pixel` (`vpx_dsp/vpx_dsp_common.h`): clamp to the 8-bit
/// pixel range.
#[inline]
fn clip_pixel(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

/// libvpx's shared compound-averaging step,
/// `ROUND_POWER_OF_TWO(dst[x] + src[x], 1)` (`vpx_convolve_avg_c`) —
/// also the tail of every `convolve_avg_*` function, where `src[x]` is
/// already a `clip_pixel`'d filtered sample. Both operands are 8-bit, so
/// the rounded average is always in `0..=255`; libvpx relies on the same
/// fact when it assigns the `ROUND_POWER_OF_TWO(...)` result straight into
/// a `uint8_t` with no explicit clamp.
#[inline]
fn avg_pixel(a: u8, b: u8) -> u8 {
    round_pow2(i32::from(a) + i32::from(b), 1) as u8
}

/// The 8-tap dot product shared by every filtered pass: `Σ src[base + k *
/// step] * kernel[k]`. `step == 1` reproduces `convolve_horiz`/
/// `convolve_avg_horiz`'s contiguous `src_x[k]`; `step == src_stride`
/// reproduces `convolve_vert`/`convolve_avg_vert`'s strided `src_y[k *
/// src_stride]`. Not present as a named function in libvpx — the C source
/// repeats this loop four times verbatim — but the arithmetic is identical
/// in every occurrence, so factoring it out cannot introduce a divergence.
#[inline]
fn tap_sum(src: &[u8], base: usize, step: usize, kernel: &[i16; SUBPEL_TAPS]) -> i32 {
    let mut sum = 0i32;
    for (k, &tap) in kernel.iter().enumerate() {
        sum += i32::from(src[base + k * step]) * i32::from(tap);
    }
    sum
}

/// libvpx `vpx_convolve_copy_c`: row-wise copy, no filtering
/// (`subpel_x == 0 && subpel_y == 0`, `ref == 0`).
fn convolve_copy(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    w: usize,
    h: usize,
) {
    for row in 0..h {
        let s = src_origin + row * src_stride;
        let d = dst_origin + row * dst_stride;
        dst[d..d + w].copy_from_slice(&src[s..s + w]);
    }
}

/// libvpx `vpx_convolve_avg_c`: `dst[x] = ROUND_POWER_OF_TWO(dst[x] +
/// src[x], 1)`, no filtering (`subpel_x == 0 && subpel_y == 0`, `ref ==
/// 1`). Also the second half of [`convolve_2d_avg`] (`vpx_convolve8_avg_c`
/// averages its own fully-filtered temp buffer against `dst` with exactly
/// this function).
fn convolve_avg(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    w: usize,
    h: usize,
) {
    for row in 0..h {
        let s = src_origin + row * src_stride;
        let d = dst_origin + row * dst_stride;
        for col in 0..w {
            dst[d + col] = avg_pixel(dst[d + col], src[s + col]);
        }
    }
}

/// libvpx `convolve_horiz` (the shared static core of
/// `vpx_convolve8_horiz_c` and the horizontal pass of `vpx_convolve8_c`):
/// filters `rows` output rows of `w` pixels, taps landing on `col -
/// TAP_OFFSET ..= col + TAP_OFFSET + 1` (`col - 3 ..= col + 4`) of each row.
///
/// `rows` is independent of the block height a caller ultimately wants:
/// [`convolve_2d`] calls this with `h + SUBPEL_TAPS - 1` rows and a
/// `src_origin` already shifted up [`TAP_OFFSET`] rows, to build the
/// vertical pass's intermediate buffer; the direct horizontal-only dispatch
/// (`vpx_convolve8_horiz_c`, whose `(void)y0_q4; (void)y_step_q4;`
/// discards the vertical phase/step this crate's simplified API does not
/// even carry) calls it with exactly `h` rows and the unshifted block
/// origin.
fn convolve_horiz(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    kernel: &[i16; SUBPEL_TAPS],
    w: usize,
    rows: usize,
) {
    for row in 0..rows {
        let row_start = src_origin + row * src_stride;
        let d_row = dst_origin + row * dst_stride;
        for col in 0..w {
            let base = row_start + col - TAP_OFFSET;
            let sum = tap_sum(src, base, 1, kernel);
            dst[d_row + col] = clip_pixel(round_pow2(sum, FILTER_BITS));
        }
    }
}

/// libvpx `convolve_avg_horiz` (shared static core of
/// `vpx_convolve8_avg_horiz_c`): as [`convolve_horiz`], but the filtered
/// sample is averaged into `dst`'s existing contents
/// (via [`avg_pixel`]) instead of overwriting it.
///
/// Unlike [`convolve_horiz`], this is **never** composed into the 2-D
/// path: `vpx_convolve8_avg_c` runs the non-avg [`convolve_2d`] into a temp
/// buffer and averages *that* with [`convolve_avg`] (see [`convolve_2d_avg`]),
/// so this function is only ever reached directly — always with exactly
/// `h` rows, never the padded `intermediate_height` [`convolve_horiz`] can
/// take.
fn convolve_avg_horiz(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    kernel: &[i16; SUBPEL_TAPS],
    w: usize,
    h: usize,
) {
    for row in 0..h {
        let row_start = src_origin + row * src_stride;
        let d_row = dst_origin + row * dst_stride;
        for col in 0..w {
            let base = row_start + col - TAP_OFFSET;
            let sum = tap_sum(src, base, 1, kernel);
            let filtered = clip_pixel(round_pow2(sum, FILTER_BITS));
            dst[d_row + col] = avg_pixel(dst[d_row + col], filtered);
        }
    }
}

/// libvpx `convolve_vert` (shared static core of `vpx_convolve8_vert_c` and
/// the vertical pass of `vpx_convolve8_c`): filters `h` output rows of `w`
/// pixels, taps landing on `row - TAP_OFFSET ..= row + TAP_OFFSET + 1`
/// (`row - 3 ..= row + 4`) of each column.
///
/// Unlike [`convolve_horiz`], the row count is always exactly `h` — the
/// vertical pass is always the *last* stage, so nothing downstream ever
/// needs it to produce extra rows. [`convolve_2d`] and the direct
/// vertical-only dispatch both call this with `h`; what differs between
/// them is `src_origin`/`src_stride` (the intermediate buffer's
/// [`TEMP_STRIDE`] vs. the real plane geometry).
fn convolve_vert(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    kernel: &[i16; SUBPEL_TAPS],
    w: usize,
    h: usize,
) {
    for col in 0..w {
        let col_start = src_origin + col;
        let d_col = dst_origin + col;
        for row in 0..h {
            let base = col_start + row * src_stride - TAP_OFFSET * src_stride;
            let sum = tap_sum(src, base, src_stride, kernel);
            dst[d_col + row * dst_stride] = clip_pixel(round_pow2(sum, FILTER_BITS));
        }
    }
}

/// libvpx `convolve_avg_vert` (shared static core of
/// `vpx_convolve8_avg_vert_c`): as [`convolve_vert`], but averaged into
/// `dst` via [`avg_pixel`] instead of overwriting it. As with
/// [`convolve_avg_horiz`], never composed into the 2-D path — see
/// [`convolve_avg_horiz`]'s doc for why.
fn convolve_avg_vert(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    kernel: &[i16; SUBPEL_TAPS],
    w: usize,
    h: usize,
) {
    for col in 0..w {
        let col_start = src_origin + col;
        let d_col = dst_origin + col;
        for row in 0..h {
            let base = col_start + row * src_stride - TAP_OFFSET * src_stride;
            let sum = tap_sum(src, base, src_stride, kernel);
            let filtered = clip_pixel(round_pow2(sum, FILTER_BITS));
            dst[d_col + row * dst_stride] = avg_pixel(dst[d_col + row * dst_stride], filtered);
        }
    }
}

/// libvpx `vpx_convolve8_c`: full 2-D interpolation through an 8-bit
/// intermediate buffer (`subpel_x != 0 && subpel_y != 0`, `ref == 0`).
///
/// Two-pass, exactly mirroring the reference: the horizontal pass reads
/// from `src_origin` shifted up [`TAP_OFFSET`] rows and produces `h +
/// SUBPEL_TAPS - 1` rows into `temp` (stride [`TEMP_STRIDE`]); the vertical
/// pass reads `temp` starting [`TAP_OFFSET`] rows down. libvpx passes
/// `temp + 64 * (SUBPEL_TAPS / 2 - 1)` as `convolve_vert`'s `src` and lets
/// that function's own unconditional `-TAP_OFFSET` row shift net back to
/// `temp` row 0 — a shift-then-unshift that looks redundant written out,
/// but is exactly what the reference does, so it is kept rather than
/// "simplified" to a direct `convolve_vert(&temp, ..., 0, ...)` call: the
/// two are numerically identical, but the point of a verified port is that
/// there is nothing left to verify they agree.
///
/// # Panics
///
/// In debug builds, if `w > 64` or `h > 64` — mirroring libvpx's own
/// `assert(w <= 64); assert(h <= 64);` in `vpx_convolve8_c`, themselves
/// tied to [`TEMP_STRIDE`]. Release builds (`debug_assertions` off, like
/// libvpx's own `NDEBUG` builds) skip the check and silently corrupt
/// adjacent `temp` rows instead of panicking — callers must uphold this
/// precondition themselves, exactly as libvpx's callers do.
fn convolve_2d(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    x_kernel: &[i16; SUBPEL_TAPS],
    y_kernel: &[i16; SUBPEL_TAPS],
    w: usize,
    h: usize,
) {
    debug_assert!(
        w <= 64 && h <= 64,
        "libvpx vpx_convolve8_c: assert(w <= 64); assert(h <= 64); (got {w}x{h})"
    );
    let intermediate_height = h + SUBPEL_TAPS - 1;
    let mut temp = vec![0u8; TEMP_STRIDE * intermediate_height];
    let horiz_origin = src_origin - TAP_OFFSET * src_stride;
    convolve_horiz(
        src,
        src_stride,
        horiz_origin,
        &mut temp,
        TEMP_STRIDE,
        0,
        x_kernel,
        w,
        intermediate_height,
    );
    let vert_origin = TAP_OFFSET * TEMP_STRIDE;
    convolve_vert(
        &temp,
        TEMP_STRIDE,
        vert_origin,
        dst,
        dst_stride,
        dst_origin,
        y_kernel,
        w,
        h,
    );
}

/// libvpx `vpx_convolve8_avg_c`: full 2-D interpolation, then averaged into
/// `dst` (`subpel_x != 0 && subpel_y != 0`, `ref == 1`).
///
/// Composed exactly as the reference is: the *non-avg* [`convolve_2d`]
/// fills a `TEMP_STRIDE`-wide `h`-row temp buffer, then [`convolve_avg`]
/// averages it into `dst`. There is no `convolve_avg_2d` that fuses the two
/// kernels with the averaging step in one pass — libvpx does not have one
/// either.
///
/// # Panics
///
/// Same precondition as [`convolve_2d`] (`w <= 64 && h <= 64` in debug
/// builds), mirroring `vpx_convolve8_avg_c`'s own `assert`s.
fn convolve_2d_avg(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    x_kernel: &[i16; SUBPEL_TAPS],
    y_kernel: &[i16; SUBPEL_TAPS],
    w: usize,
    h: usize,
) {
    debug_assert!(
        w <= 64 && h <= 64,
        "libvpx vpx_convolve8_avg_c: assert(w <= 64); assert(h <= 64); (got {w}x{h})"
    );
    let mut temp = vec![0u8; TEMP_STRIDE * h];
    convolve_2d(
        src,
        src_stride,
        src_origin,
        &mut temp,
        TEMP_STRIDE,
        0,
        x_kernel,
        y_kernel,
        w,
        h,
    );
    convolve_avg(&temp, TEMP_STRIDE, 0, dst, dst_stride, dst_origin, w, h);
}

/// Unified VP9 motion-compensation convolution — dispatches to one of the
/// eight functions above exactly as libvpx's `predict[subpel_x !=
/// 0][subpel_y != 0][ref]` table does (`vp9/common/vp9_scale.c:74-84`,
/// unscaled block; see the module docs, "Why a 4-way dispatch").
///
/// * `src`/`src_stride`/`src_origin`: the reference plane, and the flat
///   index of the block's top-left pixel within it (`src[src_origin + row *
///   src_stride + col]` is source pixel `(col, row)`). Whichever axes are
///   filtered (`x_kernel`/`y_kernel` is `Some`) need border margin around
///   that origin: [`TAP_OFFSET`] (3) pixels before and `SUBPEL_TAPS -
///   1 - TAP_OFFSET` (4) after, along that axis. Unfiltered axes need none.
/// * `dst`/`dst_stride`/`dst_origin`: the destination plane and the flat
///   index of the block's top-left output pixel, laid out the same way.
/// * `x_kernel`/`y_kernel`: the single already-phase-selected 8-tap row for
///   that axis (see the module docs, "API shape vs. libvpx"), or `None` for
///   a zero subpel fraction on that axis.
/// * `w`/`h`: block dimensions in pixels. `w <= 64 && h <= 64` in debug
///   builds when both kernels are `Some` (the two-pass path) — see
///   [`convolve_2d`]'s panic docs.
/// * `avg`: `true` selects libvpx's compound (`ref == 1`) leg — the
///   filtered sample is averaged into `dst`'s existing contents instead of
///   overwriting it; `false` selects `ref == 0` (overwrite).
#[allow(clippy::too_many_arguments)]
pub(crate) fn convolve(
    src: &[u8],
    src_stride: usize,
    src_origin: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_origin: usize,
    x_kernel: Option<&[i16; SUBPEL_TAPS]>,
    y_kernel: Option<&[i16; SUBPEL_TAPS]>,
    w: usize,
    h: usize,
    avg: bool,
) {
    match (x_kernel, y_kernel) {
        (None, None) => {
            if avg {
                convolve_avg(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, w, h,
                );
            } else {
                convolve_copy(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, w, h,
                );
            }
        }
        (None, Some(yk)) => {
            if avg {
                convolve_avg_vert(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, yk, w, h,
                );
            } else {
                convolve_vert(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, yk, w, h,
                );
            }
        }
        (Some(xk), None) => {
            if avg {
                convolve_avg_horiz(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, xk, w, h,
                );
            } else {
                convolve_horiz(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, xk, w, h,
                );
            }
        }
        (Some(xk), Some(yk)) => {
            if avg {
                convolve_2d_avg(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, xk, yk, w, h,
                );
            } else {
                convolve_2d(
                    src, src_stride, src_origin, dst, dst_stride, dst_origin, xk, yk, w, h,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Test-only fixtures. Deliberately not imported from
    // `super::tables_inter`: every kernel here is invented for this test
    // module so the package is self-contained (see the module docs).
    // -----------------------------------------------------------------

    /// The phase-0 identity every real libvpx filter family shares
    /// (`sub_pel_filters_*[0]`, e.g. `EIGHTTAP[0]` in `super::tables_inter`)
    /// — a structural fact of the filter design (tap 3 carries the whole
    /// `FILTER_WEIGHT`), not data borrowed from a specific family.
    const IDENTITY: [i16; SUBPEL_TAPS] = [0, 0, 0, 128, 0, 0, 0, 0];

    /// A simple two-tap "rounded average of adjacent pixels" kernel
    /// (structurally libvpx's `BILINEAR[8]`, the half-pel bilinear row —
    /// reinvented here, not imported, so the golden tests below are
    /// self-contained): `round((a + b) / 2)`.
    const HALF: [i16; SUBPEL_TAPS] = [0, 0, 0, 64, 64, 0, 0, 0];

    /// A fully populated, deliberately asymmetric kernel: every tap
    /// distinct, none zero, still summing to exactly 128
    /// (`1 - 3 + 7 + 100 + 30 - 8 + 2 - 1 == 128`). [`HALF`] and
    /// [`IDENTITY`] are too structured to catch a reversed or off-by-one
    /// tap alignment (row 8 of every real family is even palindromic); this
    /// one cannot pass a transposed or shifted implementation by accident.
    const ASYM: [i16; SUBPEL_TAPS] = [1, -3, 7, 100, 30, -8, 2, -1];

    /// A second, distinct asymmetric kernel (`-2 + 5 + 60 + 70 - 4 + 6 - 8
    /// + 1 == 128`), used alongside [`ASYM`] wherever a test drives `x` and
    /// `y` with different kernels — an x/y swap bug would show up as a
    /// mismatch against the independent reference, not a self-consistent
    /// wrong answer.
    const ASYM2: [i16; SUBPEL_TAPS] = [-2, 5, 60, 70, -4, 6, -8, 1];

    /// A synthetic (non-libvpx) 8-tap kernel for `phase` in `0..16`. Every
    /// row sums to exactly 128 *by construction*: `ripple` is added to tap
    /// 1 and subtracted from tap 6, so it cancels out of the sum for any
    /// value, while `hi + lo` (the part that carries the row's weight) is
    /// `128 - 8 * phase + 8 * phase == 128` for every phase. Used to stand
    /// in for "the real kernel row libvpx would select at this subpel
    /// phase" without importing `super::tables_inter`.
    fn synth_kernel(phase: i32) -> [i16; SUBPEL_TAPS] {
        let hi = (128 - 8 * phase) as i16;
        let lo = (8 * phase) as i16;
        let ripple = (phase % 5 - 2) as i16;
        [0, ripple, 0, hi, lo, 0, -ripple, 0]
    }

    /// Deterministic pseudo-random bytes (a plain LCG, so test data is
    /// reproducible without a `rand` dependency) — Numerical-Recipes
    /// constants, the same ones `vp8::dec::mc_tests::Lcg` uses.
    struct Lcg(u32);

    impl Lcg {
        fn next(&mut self) -> u8 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 24) as u8
        }
    }

    /// `len` pseudo-random bytes from a fresh [`Lcg`].
    fn noise_buf(len: usize, seed: u32) -> Vec<u8> {
        let mut rng = Lcg(seed);
        (0..len).map(|_| rng.next()).collect()
    }

    /// An 11x11 buffer with `value(row, col) = row + col`. Sized so a 4x4
    /// output block anchored at padded `(3, 3)` has exactly the margin
    /// every dispatch path (including the full 2-D one) needs: rows/cols
    /// `-3..=+7` relative to the block, i.e. absolute `0..=10`.
    fn linear_field() -> (Vec<u8>, usize, usize) {
        let stride = 11;
        let mut buf = vec![0u8; stride * 11];
        for r in 0..11 {
            for c in 0..11 {
                buf[r * stride + c] = (r + c) as u8;
            }
        }
        (buf, stride, 3 * stride + 3)
    }

    /// Independent reference for the full 2-D path: recomputed from scratch
    /// for every output pixel, no temp buffer, no shared stride arithmetic
    /// with [`convolve_2d`] — structured so it cannot share a bug with it.
    /// `clamp_intermediate` mirrors [`convolve_2d`]'s `u8` temp buffer: when
    /// `true` the horizontal pass's rounded sum is clamped to `0..=255`
    /// before the vertical pass consumes it (matching production); when
    /// `false` the raw signed sum is kept, simulating a wider (`i16`/`i32`)
    /// intermediate. See `intermediate_clip_is_observable_on_overshoot`/
    /// `..._on_undershoot` for proof the two genuinely diverge.
    fn reference_2d(
        buf: &[u8],
        stride: usize,
        origin: usize,
        x_kernel: &[i16; SUBPEL_TAPS],
        y_kernel: &[i16; SUBPEL_TAPS],
        ox: usize,
        oy: usize,
        clamp_intermediate: bool,
    ) -> u8 {
        let mut vsum = 0i32;
        for (ty, &vc) in y_kernel.iter().enumerate() {
            let row = origin as isize
                + (oy as isize + ty as isize - TAP_OFFSET as isize) * stride as isize;
            let mut hsum = 0i32;
            for (tx, &hc) in x_kernel.iter().enumerate() {
                let idx = row + ox as isize + tx as isize - TAP_OFFSET as isize;
                hsum += i32::from(buf[idx as usize]) * i32::from(hc);
            }
            let hval = round_pow2(hsum, FILTER_BITS);
            let hval = if clamp_intermediate {
                i32::from(clip_pixel(hval))
            } else {
                hval
            };
            vsum += hval * i32::from(vc);
        }
        clip_pixel(round_pow2(vsum, FILTER_BITS))
    }

    /// Independent reference for the horizontal-only dispatch: `isize`
    /// pointer-style arithmetic computed inline, unlike [`convolve_horiz`]'s
    /// precomputed `usize` base.
    fn reference_horiz_pixel(
        buf: &[u8],
        stride: usize,
        origin: usize,
        kernel: &[i16; SUBPEL_TAPS],
        oy: usize,
        ox: usize,
    ) -> u8 {
        let row = origin as isize + oy as isize * stride as isize;
        let mut sum = 0i32;
        for (t, &c) in kernel.iter().enumerate() {
            let idx = row + ox as isize + t as isize - TAP_OFFSET as isize;
            sum += i32::from(buf[idx as usize]) * i32::from(c);
        }
        clip_pixel(round_pow2(sum, FILTER_BITS))
    }

    /// Independent reference for the vertical-only dispatch; see
    /// [`reference_horiz_pixel`].
    fn reference_vert_pixel(
        buf: &[u8],
        stride: usize,
        origin: usize,
        kernel: &[i16; SUBPEL_TAPS],
        oy: usize,
        ox: usize,
    ) -> u8 {
        let col = origin as isize + ox as isize;
        let mut sum = 0i32;
        for (t, &c) in kernel.iter().enumerate() {
            let idx = col + (oy as isize + t as isize - TAP_OFFSET as isize) * stride as isize;
            sum += i32::from(buf[idx as usize]) * i32::from(c);
        }
        clip_pixel(round_pow2(sum, FILTER_BITS))
    }

    // -----------------------------------------------------------------
    // 1. subpel(0,0) == plain copy, random blocks.
    // -----------------------------------------------------------------

    #[test]
    fn subpel_zero_zero_equals_plain_copy_for_random_blocks() {
        let stride = 12;
        let buf = noise_buf(stride * 12, 0x1234_5678);
        let src_origin = 2 * stride + 2;
        for &(w, h) in &[(1usize, 1usize), (4, 4), (8, 3), (3, 8), (7, 5)] {
            let mut dst = vec![0u8; w * h];
            convolve(
                &buf, stride, src_origin, &mut dst, w, 0, None, None, w, h, false,
            );
            let mut want = vec![0u8; w * h];
            for r in 0..h {
                let s = src_origin + r * stride;
                want[r * w..r * w + w].copy_from_slice(&buf[s..s + w]);
            }
            assert_eq!(dst, want, "{w}x{h}");
        }
    }

    // -----------------------------------------------------------------
    // 2. Constant-source (DC) invariance at every subpel position.
    // -----------------------------------------------------------------

    #[test]
    fn constant_source_is_dc_preserving_at_every_subpel_position() {
        let stride = 16;
        for &level in &[0u8, 1, 17, 128, 254, 255] {
            let buf = vec![level; stride * 16];
            let src_origin = 3 * stride + 3;
            for phase in 0..16i32 {
                let kx = synth_kernel(phase);
                let ky = synth_kernel(15 - phase);
                assert_eq!(kx.iter().map(|&t| i32::from(t)).sum::<i32>(), 128);
                assert_eq!(ky.iter().map(|&t| i32::from(t)).sum::<i32>(), 128);

                let mut dst_h = vec![0u8; 16];
                convolve(
                    &buf,
                    stride,
                    src_origin,
                    &mut dst_h,
                    4,
                    0,
                    Some(&kx),
                    None,
                    4,
                    4,
                    false,
                );
                assert!(
                    dst_h.iter().all(|&p| p == level),
                    "horiz phase {phase} level {level}: {dst_h:?}"
                );

                let mut dst_v = vec![0u8; 16];
                convolve(
                    &buf,
                    stride,
                    src_origin,
                    &mut dst_v,
                    4,
                    0,
                    None,
                    Some(&ky),
                    4,
                    4,
                    false,
                );
                assert!(
                    dst_v.iter().all(|&p| p == level),
                    "vert phase {phase} level {level}: {dst_v:?}"
                );

                let mut dst_2d = vec![0u8; 16];
                convolve(
                    &buf,
                    stride,
                    src_origin,
                    &mut dst_2d,
                    4,
                    0,
                    Some(&kx),
                    Some(&ky),
                    4,
                    4,
                    false,
                );
                assert!(
                    dst_2d.iter().all(|&p| p == level),
                    "2d phase ({phase},{}) level {level}: {dst_2d:?}",
                    15 - phase
                );
            }
        }
    }

    #[test]
    fn constant_source_is_dc_preserving_under_avg_too() {
        // The filtered sample must still equal `level` before it is
        // averaged; pre-filling `dst` with the same `level` isolates that
        // (round_avg(level, level) == level), so a filtering bug in the
        // avg dispatch paths cannot hide behind the averaging step.
        let stride = 16;
        let level = 200u8;
        let buf = vec![level; stride * 16];
        let src_origin = 3 * stride + 3;
        for phase in [0i32, 5, 10, 15] {
            let k = synth_kernel(phase);
            for (xk, yk) in [(Some(&k), None), (None, Some(&k)), (Some(&k), Some(&k))] {
                let mut dst = vec![level; 16];
                convolve(&buf, stride, src_origin, &mut dst, 4, 0, xk, yk, 4, 4, true);
                assert!(dst.iter().all(|&p| p == level), "phase {phase}: {dst:?}");
            }
        }
    }

    // -----------------------------------------------------------------
    // 3. Separable consistency: *-only == 2-D with the other axis at the
    //    identity phase.
    // -----------------------------------------------------------------

    #[test]
    fn separable_consistency_horiz_only_equals_2d_with_y_identity() {
        let stride = 14;
        let buf = noise_buf(stride * 14, 0x0BAD_F00D);
        let src_origin = 3 * stride + 3;
        let mut horiz_only = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut horiz_only,
            4,
            0,
            Some(&ASYM),
            None,
            4,
            4,
            false,
        );
        let mut as_2d = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut as_2d,
            4,
            0,
            Some(&ASYM),
            Some(&IDENTITY),
            4,
            4,
            false,
        );
        assert_eq!(horiz_only, as_2d);
    }

    #[test]
    fn separable_consistency_vert_only_equals_2d_with_x_identity() {
        let stride = 14;
        let buf = noise_buf(stride * 14, 0xFEED_1234);
        let src_origin = 3 * stride + 3;
        let mut vert_only = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut vert_only,
            4,
            0,
            None,
            Some(&ASYM),
            4,
            4,
            false,
        );
        let mut as_2d = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut as_2d,
            4,
            0,
            Some(&IDENTITY),
            Some(&ASYM),
            4,
            4,
            false,
        );
        assert_eq!(vert_only, as_2d);
    }

    // -----------------------------------------------------------------
    // 4. `_avg` against hand-computed `(a + b + 1) >> 1`.
    // -----------------------------------------------------------------

    #[test]
    fn avg_matches_hand_computed_rounding() {
        // (a+b+1)>>1 by hand:
        //   (0 + 0 + 1) >> 1   = 0
        //   (1 + 2 + 1) >> 1   = 2
        //   (254 + 1 + 1) >> 1 = 128
        //   (255 + 0 + 1) >> 1 = 128
        let src = [0u8, 2, 1, 0];
        let mut dst = vec![0u8, 1, 254, 255];
        convolve(&src, 4, 0, &mut dst, 4, 0, None, None, 4, 1, true);
        assert_eq!(dst, vec![0, 2, 128, 128]);
    }

    // -----------------------------------------------------------------
    // 5. One hand-computed 4x4 golden per dispatch path (HALF kernel over
    //    the linear field: for a flat `value(r,c) = r + c` source,
    //    round_avg(v, v+1) == v+1 always, which is what makes these
    //    tractable by hand).
    // -----------------------------------------------------------------

    #[test]
    fn golden_copy_4x4() {
        // output(oy,ox) = value(3+oy, 3+ox) = oy+ox+6.
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(
            &buf, stride, origin, &mut dst, 4, 0, None, None, 4, 4, false,
        );
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            6, 7, 8, 9,
            7, 8, 9, 10,
            8, 9, 10, 11,
            9, 10, 11, 12,
        ]);
    }

    #[test]
    fn golden_avg_copy_4x4() {
        // dst pre-filled 0: output = (v+1)>>1, v = oy+ox+6.
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(&buf, stride, origin, &mut dst, 4, 0, None, None, 4, 4, true);
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            3, 4, 4, 5,
            4, 4, 5, 5,
            4, 5, 5, 6,
            5, 5, 6, 6,
        ]);
    }

    #[test]
    fn golden_horiz_4x4() {
        // output(oy,ox) = round_avg(value(3+oy,ox+3), value(3+oy,ox+4))
        //               = round_avg(oy+ox+6, oy+ox+7) = oy+ox+7.
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            origin,
            &mut dst,
            4,
            0,
            Some(&HALF),
            None,
            4,
            4,
            false,
        );
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            7, 8, 9, 10,
            8, 9, 10, 11,
            9, 10, 11, 12,
            10, 11, 12, 13,
        ]);
    }

    #[test]
    fn golden_avg_horiz_4x4() {
        // dst pre-filled 0: output = (v+1)>>1, v = oy+ox+7.
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            origin,
            &mut dst,
            4,
            0,
            Some(&HALF),
            None,
            4,
            4,
            true,
        );
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            4, 4, 5, 5,
            4, 5, 5, 6,
            5, 5, 6, 6,
            5, 6, 6, 7,
        ]);
    }

    #[test]
    fn golden_vert_4x4() {
        // By the transposed version of the same arithmetic as
        // `golden_horiz_4x4`: oy+ox+7.
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            origin,
            &mut dst,
            4,
            0,
            None,
            Some(&HALF),
            4,
            4,
            false,
        );
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            7, 8, 9, 10,
            8, 9, 10, 11,
            9, 10, 11, 12,
            10, 11, 12, 13,
        ]);
    }

    #[test]
    fn golden_avg_vert_4x4() {
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            origin,
            &mut dst,
            4,
            0,
            None,
            Some(&HALF),
            4,
            4,
            true,
        );
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            4, 4, 5, 5,
            4, 5, 5, 6,
            5, 5, 6, 6,
            5, 6, 6, 7,
        ]);
    }

    #[test]
    fn golden_2d_4x4() {
        // Horizontal pass gives temp(r,ox) = r+ox+4 for r in 0..11; the
        // vertical pass then reads temp(oy+3,ox) and temp(oy+4,ox):
        // round_avg(oy+ox+7, oy+ox+8) = oy+ox+8.
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            origin,
            &mut dst,
            4,
            0,
            Some(&HALF),
            Some(&HALF),
            4,
            4,
            false,
        );
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            8, 9, 10, 11,
            9, 10, 11, 12,
            10, 11, 12, 13,
            11, 12, 13, 14,
        ]);
    }

    #[test]
    fn golden_avg_2d_4x4() {
        // dst pre-filled 0: output = (v+1)>>1, v = oy+ox+8.
        let (buf, stride, origin) = linear_field();
        let mut dst = vec![0u8; 16];
        convolve(
            &buf,
            stride,
            origin,
            &mut dst,
            4,
            0,
            Some(&HALF),
            Some(&HALF),
            4,
            4,
            true,
        );
        #[rustfmt::skip]
        assert_eq!(dst, vec![
            4, 5, 5, 6,
            5, 5, 6, 6,
            5, 6, 6, 7,
            6, 6, 7, 7,
        ]);
    }

    // -----------------------------------------------------------------
    // 6. Asymmetric-kernel hand-computed goldens: pin tap order and
    //    column/row alignment, which the HALF/IDENTITY-based tests above
    //    cannot (see [`ASYM`]'s doc comment).
    // -----------------------------------------------------------------

    #[test]
    fn asym_kernel_horizontal_hand_computed() {
        // Source row (taps 0..=7 directly, since src_origin - TAP_OFFSET ==
        // 0): 10,20,30,40,50,60,70,80.
        //   sum = 1*10 + (-3)*20 + 7*30 + 100*40 + 30*50 + (-8)*60 + 2*70 + (-1)*80
        //       = 10 - 60 + 210 + 4000 + 1500 - 480 + 140 - 80 = 5240
        //   (5240 + 64) >> 7 = 5304 >> 7 = 41 (41*128 = 5248, remainder 56).
        let buf: [u8; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
        let mut dst = [0u8; 1];
        convolve(&buf, 8, 3, &mut dst, 1, 0, Some(&ASYM), None, 1, 1, false);
        assert_eq!(dst[0], 41);
    }

    #[test]
    fn asym_kernel_vertical_hand_computed() {
        // Same arithmetic as the horizontal case, transposed: a single
        // column (stride 1) with values 10,20,...,80 at taps 0..=7.
        let buf: [u8; 8] = [10, 20, 30, 40, 50, 60, 70, 80];
        let mut dst = [0u8; 1];
        convolve(&buf, 1, 3, &mut dst, 1, 0, None, Some(&ASYM), 1, 1, false);
        assert_eq!(dst[0], 41);
    }

    #[test]
    fn asym_kernel_2d_hand_computed() {
        // src(r,c) = 10*(c+1) + r for r,c in 0..8. Horizontal pass (kernel
        // ASYM) gives temp(r) = 41 + r for r in 0..8 (the `+ r` term adds
        // `r * sum(ASYM) == 128r` to the row-0 sum before rounding, which
        // shifts the rounded quotient by exactly `r` without changing the
        // remainder). Vertical pass (kernel ASYM2) then combines
        // temp(0..8) = [41..48]:
        //   sum = sum_k (41+k) * ASYM2[k]
        //       = 41*128 + (0*-2 + 1*5 + 2*60 + 3*70 + 4*-4 + 5*6 + 6*-8 + 7*1)
        //       = 5248 + 308 = 5556
        //   (5556 + 64) >> 7 = 5620 >> 7 = 43 (43*128 = 5504, remainder 116).
        let stride = 8;
        let mut buf = vec![0u8; stride * 8];
        for r in 0..8 {
            for c in 0..8 {
                buf[r * stride + c] = (10 * (c + 1) + r) as u8;
            }
        }
        let src_origin = 3 * stride + 3;
        let mut dst = [0u8; 1];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut dst,
            1,
            0,
            Some(&ASYM),
            Some(&ASYM2),
            1,
            1,
            false,
        );
        assert_eq!(dst[0], 43);
    }

    // -----------------------------------------------------------------
    // 7. Independent-reference sweeps over pseudo-random source data.
    // -----------------------------------------------------------------

    #[test]
    fn convolve_horiz_matches_independent_reference_over_noise() {
        let stride = 15;
        let buf = noise_buf(stride * 8, 0xA5A5_0001);
        let src_origin = 3;
        let (w, h) = (8usize, 5usize);
        let mut dst = vec![0u8; w * h];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut dst,
            w,
            0,
            Some(&ASYM),
            None,
            w,
            h,
            false,
        );
        for oy in 0..h {
            for ox in 0..w {
                let want = reference_horiz_pixel(&buf, stride, src_origin, &ASYM, oy, ox);
                assert_eq!(dst[oy * w + ox], want, "at ({ox},{oy})");
            }
        }
    }

    #[test]
    fn convolve_vert_matches_independent_reference_over_noise() {
        let stride = 8;
        let buf = noise_buf(stride * 15, 0xA5A5_0002);
        let src_origin = 3 * stride;
        let (w, h) = (8usize, 5usize);
        let mut dst = vec![0u8; w * h];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut dst,
            w,
            0,
            None,
            Some(&ASYM),
            w,
            h,
            false,
        );
        for oy in 0..h {
            for ox in 0..w {
                let want = reference_vert_pixel(&buf, stride, src_origin, &ASYM, oy, ox);
                assert_eq!(dst[oy * w + ox], want, "at ({ox},{oy})");
            }
        }
    }

    #[test]
    fn convolve_2d_matches_independent_reference_over_noise() {
        let stride = 15;
        let buf = noise_buf(stride * 15, 0xC0FF_EE01);
        let src_origin = 3 * stride + 3;
        let (w, h) = (8usize, 8usize);
        let mut dst = vec![0u8; w * h];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut dst,
            w,
            0,
            Some(&ASYM),
            Some(&ASYM2),
            w,
            h,
            false,
        );
        for oy in 0..h {
            for ox in 0..w {
                let want = reference_2d(&buf, stride, src_origin, &ASYM, &ASYM2, ox, oy, true);
                assert_eq!(dst[oy * w + ox], want, "at ({ox},{oy})");
            }
        }
    }

    #[test]
    fn convolve_2d_avg_matches_independent_reference_over_noise() {
        let stride = 15;
        let buf = noise_buf(stride * 15, 0xC0FF_EE02);
        let src_origin = 3 * stride + 3;
        let (w, h) = (8usize, 8usize);
        let before = noise_buf(w * h, 0xACE1_5EED);
        let mut dst = before.clone();
        convolve(
            &buf,
            stride,
            src_origin,
            &mut dst,
            w,
            0,
            Some(&ASYM),
            Some(&ASYM2),
            w,
            h,
            true,
        );
        for oy in 0..h {
            for ox in 0..w {
                let filtered = reference_2d(&buf, stride, src_origin, &ASYM, &ASYM2, ox, oy, true);
                let want = avg_pixel(before[oy * w + ox], filtered);
                assert_eq!(dst[oy * w + ox], want, "at ({ox},{oy})");
            }
        }
    }

    // -----------------------------------------------------------------
    // 8. The 8-bit intermediate clip is observable (proof, not assumption):
    //    a hand-built pattern makes the horizontal pass overshoot 255 (and,
    //    mirrored, undershoot 0); the clamped and unclamped references are
    //    first shown to disagree, then `convolve` is shown to match the
    //    clamped one.
    // -----------------------------------------------------------------

    #[test]
    fn intermediate_clip_is_observable_on_overshoot() {
        // Row 3 of the 8-row vertical window is 255 at every column whose
        // tap weight in ASYM is positive (1,7,100,30,2) and 0 at every
        // column whose tap weight is negative (-3,-8,-1); every other row
        // is all zero. That row's horizontal sum is
        // 255 * (1+7+100+30+2) = 35700, which rounds to 279 -- clamped to
        // 255, but if a buggy implementation kept the unclamped 279 and
        // fed it to ASYM's dominant tap (weight 100, at vertical position
        // 3, exactly where this row lands), the final outputs diverge:
        //   clamped:   clip_pixel(round_pow2(255*100, 7)) = 199
        //   unclamped: clip_pixel(round_pow2(279*100, 7)) = 218
        let stride = 8;
        let mut buf = vec![0u8; stride * 8];
        buf[3 * stride..3 * stride + 8].copy_from_slice(&[255, 0, 255, 255, 255, 0, 255, 0]);
        let src_origin = 3 * stride + 3;

        let clamped = reference_2d(&buf, stride, src_origin, &ASYM, &ASYM, 0, 0, true);
        let unclamped = reference_2d(&buf, stride, src_origin, &ASYM, &ASYM, 0, 0, false);
        assert_eq!(clamped, 199, "clamped reference");
        assert_eq!(unclamped, 218, "unclamped reference");
        assert_ne!(
            clamped, unclamped,
            "pattern must actually overshoot the intermediate, \
             otherwise the clamp is not being exercised"
        );

        let mut dst = [0u8; 1];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut dst,
            1,
            0,
            Some(&ASYM),
            Some(&ASYM),
            1,
            1,
            false,
        );
        assert_eq!(
            dst[0], clamped,
            "convolve's u8 temp buffer must clamp the intermediate"
        );
    }

    #[test]
    fn intermediate_clip_is_observable_on_undershoot() {
        // The mirror image: row 5 (vertical tap position 5, ASYM's most
        // negative weight, -8) is 255 only at ASYM's negative-tap columns
        // (1,5,7). Horizontal sum = 255*(-3-8-1) = -3060, which rounds to
        // -24 -- clamped to 0. Fed unclamped into weight -8:
        //   clamped:   clip_pixel(round_pow2(0 * -8, 7))   = 0
        //   unclamped: clip_pixel(round_pow2(-24 * -8, 7)) = 2
        let stride = 8;
        let mut buf = vec![0u8; stride * 8];
        buf[5 * stride..5 * stride + 8].copy_from_slice(&[0, 255, 0, 0, 0, 255, 0, 255]);
        let src_origin = 3 * stride + 3;

        let clamped = reference_2d(&buf, stride, src_origin, &ASYM, &ASYM, 0, 0, true);
        let unclamped = reference_2d(&buf, stride, src_origin, &ASYM, &ASYM, 0, 0, false);
        assert_eq!(clamped, 0, "clamped reference");
        assert_eq!(unclamped, 2, "unclamped reference");
        assert_ne!(
            clamped, unclamped,
            "pattern must actually undershoot the intermediate, \
             otherwise the clamp is not being exercised"
        );

        let mut dst = [0u8; 1];
        convolve(
            &buf,
            stride,
            src_origin,
            &mut dst,
            1,
            0,
            Some(&ASYM),
            Some(&ASYM),
            1,
            1,
            false,
        );
        assert_eq!(
            dst[0], clamped,
            "convolve's u8 temp buffer must clamp the intermediate"
        );
    }

    // -----------------------------------------------------------------
    // 9. Write-addressing / no-overwrite proof at a non-zero `dst_origin`.
    //    Every test above uses a tightly packed `dst` (`dst_stride == w`,
    //    `dst_origin == 0`), which cannot catch a wrong `dst_stride`/
    //    `dst_origin` computation, or -- particularly for [`convolve_vert`]/
    //    [`convolve_avg_vert`], whose write loop walks column-then-row --
    //    a transposed row/col in the destination index. A real caller
    //    (from package P10 onward) always writes into the middle of a
    //    frame plane, never a block-sized buffer, so this is the shape
    //    that matters.
    // -----------------------------------------------------------------

    /// Marks bytes a dispatch must leave untouched.
    const SENTINEL: u8 = 0xAA;

    /// A 16x16 [`SENTINEL`]-filled buffer and the flat index of a 4x4
    /// rectangle placed away from every edge (rows 5..=8, cols 4..=7).
    fn sentinel_dst() -> (Vec<u8>, usize, usize) {
        let stride = 16;
        (vec![SENTINEL; stride * 16], stride, 5 * stride + 4)
    }

    /// Asserts that only the `w`x`h` rectangle at `origin` differs from
    /// [`SENTINEL`], and that it matches `want` exactly.
    fn assert_only_rectangle_written(
        dst: &[u8],
        stride: usize,
        origin: usize,
        w: usize,
        h: usize,
        want: &[u8],
    ) {
        let rows = dst.len() / stride;
        let (orow, ocol) = (origin / stride, origin % stride);
        for row in 0..rows {
            for col in 0..stride {
                let idx = row * stride + col;
                if row >= orow && row < orow + h && col >= ocol && col < ocol + w {
                    let (r, c) = (row - orow, col - ocol);
                    assert_eq!(
                        dst[idx],
                        want[r * w + c],
                        "inside rect at (row={row},col={col})"
                    );
                } else {
                    assert_eq!(dst[idx], SENTINEL, "outside rect at (row={row},col={col})");
                }
            }
        }
    }

    #[test]
    fn nonzero_dst_origin_2d_writes_only_its_own_rectangle() {
        let (buf, src_stride, src_origin) = linear_field();
        let (mut dst, dst_stride, dst_origin) = sentinel_dst();
        convolve(
            &buf,
            src_stride,
            src_origin,
            &mut dst,
            dst_stride,
            dst_origin,
            Some(&HALF),
            Some(&HALF),
            4,
            4,
            false,
        );
        #[rustfmt::skip]
        let want = [
            8, 9, 10, 11,
            9, 10, 11, 12,
            10, 11, 12, 13,
            11, 12, 13, 14,
        ];
        assert_only_rectangle_written(&dst, dst_stride, dst_origin, 4, 4, &want);
    }

    #[test]
    fn nonzero_dst_origin_2d_avg_writes_only_its_own_rectangle() {
        let (buf, src_stride, src_origin) = linear_field();
        let (mut dst, dst_stride, dst_origin) = sentinel_dst();
        // The avg goldens above assume a pre-existing prediction of 0
        // inside the block; zero out only the target rectangle so the rest
        // stays SENTINEL and "untouched" remains distinguishable from
        // "touched".
        for r in 0..4 {
            let row = dst_origin + r * dst_stride;
            dst[row..row + 4].fill(0);
        }
        convolve(
            &buf,
            src_stride,
            src_origin,
            &mut dst,
            dst_stride,
            dst_origin,
            Some(&HALF),
            Some(&HALF),
            4,
            4,
            true,
        );
        #[rustfmt::skip]
        let want = [
            4, 5, 5, 6,
            5, 5, 6, 6,
            5, 6, 6, 7,
            6, 6, 7, 7,
        ];
        assert_only_rectangle_written(&dst, dst_stride, dst_origin, 4, 4, &want);
    }

    #[test]
    fn nonzero_dst_origin_horiz_only_writes_only_its_own_rectangle() {
        let (buf, src_stride, src_origin) = linear_field();
        let (mut dst, dst_stride, dst_origin) = sentinel_dst();
        convolve(
            &buf,
            src_stride,
            src_origin,
            &mut dst,
            dst_stride,
            dst_origin,
            Some(&HALF),
            None,
            4,
            4,
            false,
        );
        #[rustfmt::skip]
        let want = [
            7, 8, 9, 10,
            8, 9, 10, 11,
            9, 10, 11, 12,
            10, 11, 12, 13,
        ];
        assert_only_rectangle_written(&dst, dst_stride, dst_origin, 4, 4, &want);
    }

    #[test]
    fn nonzero_dst_origin_vert_only_writes_only_its_own_rectangle() {
        // convolve_vert's write loop walks column-then-row (`for col { for
        // row { dst[d_col + row*dst_stride] } }`) -- the addressing shape
        // most likely to hide a transposed row/col bug behind a tightly
        // packed destination, which is exactly what this test does not
        // use.
        let (buf, src_stride, src_origin) = linear_field();
        let (mut dst, dst_stride, dst_origin) = sentinel_dst();
        convolve(
            &buf,
            src_stride,
            src_origin,
            &mut dst,
            dst_stride,
            dst_origin,
            None,
            Some(&HALF),
            4,
            4,
            false,
        );
        #[rustfmt::skip]
        let want = [
            7, 8, 9, 10,
            8, 9, 10, 11,
            9, 10, 11, 12,
            10, 11, 12, 13,
        ];
        assert_only_rectangle_written(&dst, dst_stride, dst_origin, 4, 4, &want);
    }

    // -----------------------------------------------------------------
    // Misc.
    // -----------------------------------------------------------------

    #[test]
    #[should_panic = "assert(w <= 64)"]
    fn convolve_2d_debug_asserts_block_size_limit() {
        let buf = vec![0u8; 8];
        let mut dst = vec![0u8; 8];
        convolve(
            &buf,
            1,
            0,
            &mut dst,
            1,
            0,
            Some(&HALF),
            Some(&HALF),
            65,
            4,
            false,
        );
    }
}
