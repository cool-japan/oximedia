//! VP9 inter prediction — the **decoder's** motion-compensated predictor
//! build: [`dec_build_inter_predictors_sb`] and the per-plane
//! [`dec_build_inter_predictors`] it drives.
//!
//! # Provenance
//!
//! Transcribed from the libvpx reference implementation, tag **v1.15.2**
//! (commit `d168454`) — the same tag [`super::mc`] and [`super::tables_inter`]
//! cite:
//!
//! * `vp9/decoder/vp9_decodeframe.c:458-491` — `build_mc_border`
//!   ([`build_mc_border`]).
//! * `vp9/decoder/vp9_decodeframe.c:558-580` — `extend_and_predict`
//!   (8-bit build; [`extend_and_predict`]).
//! * `vp9/decoder/vp9_decodeframe.c:578-714` — `dec_build_inter_predictors`
//!   ([`dec_build_inter_predictors`]).
//! * `vp9/decoder/vp9_decodeframe.c:716-786` — `dec_build_inter_predictors_sb`
//!   ([`dec_build_inter_predictors_sb`]).
//! * `vp9/common/vp9_reconinter.c:62-126` — `round_mv_comp_q4`,
//!   `mi_mv_pred_q4`, `round_mv_comp_q2`, `mi_mv_pred_q2`,
//!   `clamp_mv_to_umv_border_sb`, `average_split_mvs`
//!   ([`round_mv_comp_q4`], [`mi_mv_pred_q4`], [`round_mv_comp_q2`],
//!   [`mi_mv_pred_q2`], [`clamp_mv_to_umv_border_sb`],
//!   [`average_split_mvs`]).
//! * `vp9/common/vp9_reconinter.h:23-32` — `inter_predictor`'s
//!   `predict[subpel_x != 0][subpel_y != 0][ref]` dispatch, which
//!   [`super::mc::convolve`] already reproduces.
//! * `vpx_dsp/vpx_filter.h:23-25` — `SUBPEL_BITS 4`, `SUBPEL_MASK 15`,
//!   `SUBPEL_SHIFTS 16` ([`SUBPEL_BITS`], [`SUBPEL_MASK`],
//!   [`SUBPEL_SHIFTS`]).
//! * `vpx_scale/yv12config.h:25` — `VP9_INTERP_EXTEND 4`
//!   ([`VP9_INTERP_EXTEND`]).
//! * `vpx_scale/generic/yv12config.c:248-257` — `y_crop_width`/`y_crop_height`
//!   and `uv_crop_width = (width + ss_x) >> ss_x` ([`plane_display_dims`]).
//!
//! # Why the *decoder* has its own predictor build
//!
//! libvpx's common `vp9_build_inter_predictors_sb`
//! (`vp9_reconinter.c:253-257`) simply reads the reference frame through
//! the frame buffer's *border*, which the encoder keeps populated by
//! `vpx_extend_frame_borders`. **The VP9 decoder never extends a decoded
//! frame's borders** — grep `vp9/decoder/*.c` for `extend`: the only hits
//! are `extend_and_predict` / `build_mc_border`, i.e. this file's subject.
//! The decoder's reference pixels are therefore *only* the ones inside the
//! reference frame's **display** rectangle (`y_crop_width` x
//! `y_crop_height`, `uv_crop_*` for chroma); every fetch that reaches
//! outside it is served by [`build_mc_border`]'s clamped-coordinate copy,
//! which replicates the display edge.
//!
//! Two consequences the rest of the VP9 decoder depends on:
//!
//! 1. The decoded-picture buffer ([`Vp9RefSlot`]) needs **no border
//!    extension and no padding**: its MI-aligned planes are read only
//!    inside their display rectangle.
//! 2. The MI-alignment padding of a reference frame (columns
//!    `display_width .. mi_cols * 8`, rows likewise) is *never* read by
//!    motion compensation, so it does not matter that our slots keep the
//!    reconstruction there while libvpx's encoder-side buffers would hold
//!    a replicated edge. It still matters to the *loop filter* of the frame
//!    that produced it, which is why the planes stay MI-aligned.
//!
//! # Two-path edge handling
//!
//! [`dec_build_inter_predictors`] keeps libvpx's exact two-level structure:
//!
//! ```text
//! if (is_scaled || scaled_mv.col || scaled_mv.row || (frame_width & 0x7) ||
//!     (frame_height & 0x7)) {                       // outer: is a border even possible?
//!   ... compute x1/y1 and the 8-tap extension ...
//!   if (x0 < 0 || x0 > fw - 1 || x1 < 0 || x1 > fw - 1 ||
//!       y0 < 0 || y0 > fh - 1 || y1 < 0 || y1 > fh - 1) {   // inner: does it actually exit?
//!     extend_and_predict(...); return;                     //   -> mc_buf path
//!   }
//! }
//! inter_predictor(buf_ptr, ...);                           //   -> direct path
//! ```
//!
//! A single fat physical border around the reference plane is **not**
//! equivalent to this: the extension window depends on the block's own
//! `subpel_x`/`subpel_y` (`VP9_INTERP_EXTEND - 1` before, `VP9_INTERP_EXTEND`
//! after, and only on filtered axes), and the fetch is clamped to the
//! *display* rectangle rather than to any allocated size.
//!
//! Note that libvpx's `x1`/`y1` are deliberately conservative by one
//! (`x1 = ((x0_16 + (w - 1) * xs) >> SUBPEL_BITS) + 1`, i.e. `x0 + w` for
//! the unscaled `xs == 16` case, one column past the block). That extra
//! column widens `b_w` by one and makes a block flush against the right
//! frame edge take the mc_buf path. It is transcribed as-is: "fixing" it
//! would change *which* blocks take which path, and with it the claim that
//! this is a verbatim transcription.
//!
//! ## The one addition to libvpx's control flow
//!
//! When the outer condition is **false** (zero motion vector *and* both
//! plane display dimensions multiples of 8), libvpx performs an unchecked
//! `w` x `h` copy from `(x0, y0)`. That read can still leave the display
//! rectangle: a block may overhang the MI grid (a `PARTITION_VERT` of a
//! 64x64 superblock at the last MI column codes a 32-pixel-wide block over
//! a single remaining MI column, overhanging by 24 pixels), and libvpx then
//! reads its frame buffer's *uninitialised* border. Those output pixels are
//! dead — see "Dead overhang" below — so the garbage never reaches the
//! bitstream's output, but a Rust slice index would panic instead of
//! quietly reading past the plane.
//!
//! This module therefore adds one branch libvpx does not have: when the
//! outer condition is false and the direct window would leave the display
//! rectangle, it takes the mc_buf path with exactly the `x1`/`y1`/`x_pad`/
//! `y_pad` the inner check would have computed there (`x1 = x0 + w`,
//! `y1 = y0 + h`, both pads zero, because the outer condition being false
//! forces `scaled_mv == 0` and hence `subpel_x == subpel_y == 0`). The
//! result is bit-identical for every *live* pixel: [`build_mc_border`]
//! reproduces the real pixel for any in-range coordinate and replicates the
//! edge for any out-of-range one, which is precisely what a bordered
//! libvpx buffer would have held.
//!
//! # Dead overhang, and why the destination write is clipped
//!
//! A block that overhangs the MI grid predicts pixels past the plane's
//! MI-aligned width/height. libvpx writes them into the frame border; this
//! module clips them away, because they are provably never read again:
//!
//! * the loop filter iterates the MI grid, so the furthest pixel it can
//!   touch is `mi_cols * 8 - 1` (the plane's last column);
//! * intra prediction clamps its edge fetch to the display rectangle
//!   (`vp9_reconintra.c`'s `build_intra_predictors`, `frame_width` /
//!   `frame_height` arguments);
//! * inter prediction — this file — clamps its fetch to the display
//!   rectangle too.
//!
//! Clipping cannot disturb the pixels that *are* kept: [`super::mc`]'s
//! convolutions are pointwise-separable, so output column `j` (row `i`)
//! depends only on the source and the kernels, never on `w` (`h`). Reducing
//! `w`/`h` also only ever shrinks the source window, so it cannot introduce
//! an out-of-range read; and since the clipped `w`/`h` never exceed the
//! original (`<= 64`), [`super::mc::convolve`]'s `w <= 64 && h <= 64`
//! precondition and its `TEMP_STRIDE == 64` intermediate stay satisfied.
//! The clip is applied **only** to the final convolution: every coordinate
//! libvpx's border decision consumes is computed from the unclipped `w`/`h`.
//!
//! # Two different MV clamps — and where the decoder applies neither
//!
//! * [`clamp_mv_to_umv_border_sb`] (here, `vp9_reconinter.c:91-111`) — the
//!   **MC-stage** clamp. It scales the vector to the plane's subsampling
//!   (`row * (1 << (1 - ss_y))`, so the result is always q4 = 1/16 pel *of
//!   that plane*) and bounds it by `mb_to_*_edge * (1 << (1 - ss))` ±
//!   sub-pixel margins built from `VP9_INTERP_EXTEND` and the *plane* block
//!   size.
//! * [`BlockEdges::clamp_mv_ref`] (`super::mvref`, `vp9_mvref_common.h:216-220`)
//!   — the **candidate** clamp, margin `MV_BORDER` = 16 pixels, applied to
//!   motion-vector *predictors* before a mode is decoded. Different
//!   function, different margin, different stage.
//!
//! **The decoder's unscaled path does not call the MC-stage clamp**, and
//! neither does this module. `dec_build_inter_predictors`
//! (`vp9_decodeframe.c:603-646`) calls `clamp_mv_to_umv_border_sb` only
//! inside `if (is_scaled)`; its `else` branch scales the vector and nothing
//! more, and `dec_build_inter_predictors_sb` hands it `mi->mv[ref]` /
//! `average_split_mvs(...)` raw. (The *common* `build_inter_predictors`,
//! `vp9_reconinter.c:127-208`, which the encoder's reconstruction uses,
//! does clamp — and encoder and decoder must reconstruct identically, so
//! libvpx itself certifies that the clamp changes no output.)
//!
//! The reason it cannot: the clamp only engages once the vector points so
//! far out that *no visible pixel contributes* (libvpx's own comment says
//! as much), and it lands on a whole-pixel value — from there every tap of
//! every output pixel reads the same replicated edge sample `p`, and a
//! kernel row summing to `1 << FILTER_BITS` returns
//! `ROUND_POWER_OF_TWO(128 * p, 7) == p` whatever the phase.
//! `far_out_of_frame_vector_is_unaffected_by_the_mc_clamp` pins the
//! equality on a real prediction rather than on this paragraph.
//!
//! Dropping the clamp costs no safety either: the fetch window is `w + 8`
//! wide and `h + 8` tall however far the vector points (its size depends
//! only on `w`/`h` and the sub-pixel phases), and [`build_mc_border`]
//! clamps every coordinate it reads. Decoded vectors are themselves bounded
//! — `is_mv_valid` (`vp9_decodemv.c:389-392`) rejects `|component| >=
//! 16384`, i.e. beyond ±2048 pixels — so no coordinate arithmetic here can
//! overflow.
//!
//! [`clamp_mv_to_umv_border_sb`] is implemented and tested regardless: it
//! is the scaled path's (P15's) clamp, and it is the piece of
//! `vp9_reconinter.c` a reader comparing the two builds will look for.
//!
//! # Reference scaling
//!
//! Not implemented (VP9 package P15). Any active reference whose display
//! dimensions differ from the current frame's is refused with an honest
//! [`CodecError::UnsupportedFeature`] rather than silently mispredicted:
//! libvpx's scaled path replaces the whole coordinate derivation
//! (`sf->scale_value_x`, `vp9_scale_mv`, per-column phase stepping with
//! `xs`/`ys != 16`), which neither this module nor [`super::mc`] performs.
//! With that refusal in place `is_scaled` is always false here, so libvpx's
//! `is_scaled ||` term in the outer condition and its `sf->x_step_q4 !=
//! SUBPEL_SHIFTS` terms in the pad conditions are constant-false and appear
//! only as comments.

#![forbid(unsafe_code)]
#![allow(clippy::too_many_arguments)]

use super::mc;
use super::mvref::BlockEdges;
use super::recon::{MiInfo, PlaneBuf};
use super::refs::Vp9RefSlot;
use super::tables::{NUM_8X8_BLOCKS_HIGH, NUM_8X8_BLOCKS_WIDE};
use super::tables_inter::{InterpKernels, FILTER_KERNELS, SUBPEL_TAPS};
use crate::error::{CodecError, CodecResult};
use crate::vp9::mv::MotionVector;

/// Number of planes in a frame — libvpx `MAX_MB_PLANE`
/// (`vp9/common/vp9_blockd.h:32`).
pub const MAX_MB_PLANE: usize = 3;

/// libvpx `SUBPEL_BITS` (`vpx_dsp/vpx_filter.h:23`): motion vectors reach
/// motion compensation in 1/16-pel ("q4") units, so the integer pixel
/// offset is `mv >> SUBPEL_BITS`.
const SUBPEL_BITS: u32 = 4;

/// libvpx `SUBPEL_MASK` (`vpx_dsp/vpx_filter.h:24`): `(1 << SUBPEL_BITS) - 1`
/// = 15. The sub-pixel phase is `mv & SUBPEL_MASK`, selecting one of the 16
/// rows of an [`InterpKernels`] family.
const SUBPEL_MASK: i32 = (1 << SUBPEL_BITS) - 1;

/// libvpx `SUBPEL_SHIFTS` (`vpx_dsp/vpx_filter.h:25`): `1 << SUBPEL_BITS`
/// = 16, one whole pixel in q4 units and the number of kernel phases.
const SUBPEL_SHIFTS: i32 = 1 << SUBPEL_BITS;

/// libvpx `VP9_INTERP_EXTEND` (`vpx_scale/yv12config.h:25`): how far an
/// 8-tap interpolation reaches past the block — `VP9_INTERP_EXTEND - 1` = 3
/// pixels before it and `VP9_INTERP_EXTEND` = 4 after.
const VP9_INTERP_EXTEND: i32 = 4;

/// libvpx `BLOCK_8X8` (`vp9/common/vp9_enums.h:49`) — `sb_type` values below
/// this carry per-sub-block motion vectors in [`MiInfo::bmv`].
const BLOCK_8X8: u8 = 3;

/// Number of VP9 block sizes (`BLOCK_SIZES`), the length of the
/// `num_8x8_blocks_*_lookup` tables.
const BLOCK_SIZES: usize = 13;

/// Pixels per mode-info unit — libvpx `MI_SIZE` (`vp9_enums.h:24`).
const MI_SIZE: usize = 8;

// ---------------------------------------------------------------------------
// Frame geometry
// ---------------------------------------------------------------------------

/// The current frame's geometry, as inter prediction needs it.
///
/// Gathers what libvpx spreads over `cm->mi_rows` / `cm->mi_cols`,
/// `cm->width` / `cm->height` and `xd->plane[i].subsampling_{x,y}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameGeometry {
    /// MI grid height, `(height + 7) >> 3` (libvpx `cm->mi_rows`).
    pub mi_rows: usize,
    /// MI grid width, `(width + 7) >> 3` (libvpx `cm->mi_cols`).
    pub mi_cols: usize,
    /// Display width in pixels (libvpx `cm->width` / `y_crop_width`).
    pub width: usize,
    /// Display height in pixels (libvpx `cm->height` / `y_crop_height`).
    pub height: usize,
    /// Chroma horizontal subsampling (`1` for 4:2:0).
    pub ss_x: usize,
    /// Chroma vertical subsampling (`1` for 4:2:0).
    pub ss_y: usize,
}

impl FrameGeometry {
    /// Subsampling of `plane`: none for luma, [`Self::ss_x`] /
    /// [`Self::ss_y`] for both chroma planes (libvpx
    /// `xd->plane[i].subsampling_{x,y}`, set by `vp9_setup_block_planes`).
    #[must_use]
    pub fn plane_ss(&self, plane: usize) -> (usize, usize) {
        if plane == 0 {
            (0, 0)
        } else {
            (self.ss_x, self.ss_y)
        }
    }
}

/// Display ("crop") dimensions of one plane of a frame whose luma display
/// size is `width` x `height` — libvpx `vpx_realloc_frame_buffer`
/// (`vpx_scale/generic/yv12config.c:248-255`):
///
/// ```c
/// ybf->y_crop_width = width;
/// ybf->y_crop_height = height;
/// ybf->uv_crop_width = (width + ss_x) >> ss_x;
/// ybf->uv_crop_height = (height + ss_y) >> ss_y;
/// ```
///
/// i.e. chroma rounds **up** (`div_ceil`), so a 76x42 4:2:0 frame has
/// 38x21 chroma display pixels while its MI-aligned chroma planes are
/// 40x24.
#[must_use]
pub fn plane_display_dims(width: usize, height: usize, ss_x: usize, ss_y: usize) -> (usize, usize) {
    ((width + ss_x) >> ss_x, (height + ss_y) >> ss_y)
}

// ---------------------------------------------------------------------------
// Small arithmetic helpers
// ---------------------------------------------------------------------------

/// libvpx `clamp` (`vpx_dsp/vpx_dsp_common.h:66-68`):
/// `value < low ? low : (value > high ? high : value)`.
///
/// Written as the ternary chain rather than [`Ord::clamp`], which panics
/// when `low > high` where libvpx degrades to `low`. (`super::mvref` keeps
/// a private copy of the same three lines; the two modules are independent
/// ports of independent libvpx call sites.)
const fn clamp_i32(value: i32, low: i32, high: i32) -> i32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

/// Narrows a clamped motion-vector component back to the `int16_t` a libvpx
/// `MV` field is.
///
/// libvpx narrows twice in [`clamp_mv_to_umv_border_sb`] — once on the
/// subsampling-scaled vector, once when `clamp_mv` writes a bound back into
/// the `MV` — and neither can actually truncate: a decoded component
/// satisfies `|mv| < 16384` (`MV_UPP`, `vp9_entropymv.h`), so doubling it
/// stays inside `i16`; and a clamp bound is only *selected* when the (in
/// range) value lies beyond it, which puts the bound inside `i16` too. The
/// saturating narrow therefore never fires; it exists so that a future
/// caller with unvalidated vectors degrades instead of wrapping.
fn to_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Bounds a `BLOCK_SIZE` index for the `num_8x8_blocks_*_lookup` tables,
/// matching `super::mvref`'s `bsize_index`: a debug assertion for the decode
/// path plus a release-mode clamp so a corrupt value degrades instead of
/// panicking.
fn bsize_index(bsize: u8) -> usize {
    debug_assert!(
        (bsize as usize) < BLOCK_SIZES,
        "VP9 BLOCK_SIZE out of range: {bsize}"
    );
    (bsize as usize).min(BLOCK_SIZES - 1)
}

/// Bounds a sub-block index for [`MiInfo::bmv`]'s four entries. Every index
/// this module forms is `< 4` by construction (see [`average_split_mvs`]);
/// the clamp mirrors [`bsize_index`]'s defensive shape.
fn bmv_index(block: usize) -> usize {
    debug_assert!(block < 4, "VP9 sub-8x8 block index out of range: {block}");
    block.min(3)
}

// ---------------------------------------------------------------------------
// Sub-8x8 motion-vector derivation (vp9_reconinter.c:62-126)
// ---------------------------------------------------------------------------

/// libvpx `round_mv_comp_q4` (`vp9_reconinter.c:62-64`):
///
/// ```c
/// static INLINE int round_mv_comp_q4(int value) {
///   return (value < 0 ? value - 2 : value + 2) / 4;
/// }
/// ```
///
/// The away-from-zero bias plus C's truncating division is a round-half-away
/// -from-zero over four values. Rust's `/` on `i32` truncates toward zero
/// exactly like C's, which is what makes the transcription literal — see
/// `round_mv_comp_q4_truncates_toward_zero_like_c`.
#[must_use]
pub const fn round_mv_comp_q4(value: i32) -> i32 {
    (if value < 0 { value - 2 } else { value + 2 }) / 4
}

/// libvpx `round_mv_comp_q2` (`vp9_reconinter.c:78-80`): the same shape over
/// two values, `(value < 0 ? value - 1 : value + 1) / 2`.
#[must_use]
pub const fn round_mv_comp_q2(value: i32) -> i32 {
    (if value < 0 { value - 1 } else { value + 1 }) / 2
}

/// libvpx `mi_mv_pred_q4` (`vp9_reconinter.c:66-76`): the rounded average of
/// all four sub-block vectors of reference `ref_idx`.
///
/// The sums are formed in `i32` because C promotes the four `int16_t`
/// operands to `int` before adding; four components below `MV_UPP` sum to at
/// most 65532, which no `i16` accumulator would hold.
#[must_use]
pub fn mi_mv_pred_q4(mi: &MiInfo, ref_idx: usize) -> MotionVector {
    let mut row = 0i32;
    let mut col = 0i32;
    for bmv in &mi.bmv {
        row += i32::from(bmv[ref_idx].row);
        col += i32::from(bmv[ref_idx].col);
    }
    MotionVector::new(to_i16(round_mv_comp_q4(row)), to_i16(round_mv_comp_q4(col)))
}

/// libvpx `mi_mv_pred_q2` (`vp9_reconinter.c:82-89`): the rounded average of
/// two named sub-block vectors of reference `ref_idx`.
#[must_use]
pub fn mi_mv_pred_q2(mi: &MiInfo, ref_idx: usize, block0: usize, block1: usize) -> MotionVector {
    let a = mi.bmv[bmv_index(block0)][ref_idx];
    let b = mi.bmv[bmv_index(block1)][ref_idx];
    MotionVector::new(
        to_i16(round_mv_comp_q2(i32::from(a.row) + i32::from(b.row))),
        to_i16(round_mv_comp_q2(i32::from(a.col) + i32::from(b.col))),
    )
}

/// libvpx `average_split_mvs` (`vp9_reconinter.c:113-126`) — which motion
/// vector a sub-8x8 block contributes to one plane:
///
/// ```c
/// const int ss_idx = ((pd->subsampling_x > 0) << 1) | (pd->subsampling_y > 0);
/// switch (ss_idx) {
///   case 0: res = mi->bmi[block].as_mv[ref].as_mv; break;
///   case 1: res = mi_mv_pred_q2(mi, ref, block, block + 2); break;
///   case 2: res = mi_mv_pred_q2(mi, ref, block, block + 1); break;
///   case 3: res = mi_mv_pred_q4(mi, ref); break;
/// }
/// ```
///
/// Transcribed as a match on the two subsampling flags, which is exhaustive
/// where libvpx needs a `default: assert(...)`. For the 4:2:0 profile this
/// decoder supports, luma takes `ss_idx == 0` (the sub-block's own vector,
/// four 4x4 predictions per 8x8) and chroma takes `ss_idx == 3` (one 4x4
/// prediction from the average of all four).
#[must_use]
pub fn average_split_mvs(
    ss_x: usize,
    ss_y: usize,
    mi: &MiInfo,
    ref_idx: usize,
    block: usize,
) -> MotionVector {
    match (ss_x > 0, ss_y > 0) {
        // ss_idx 0
        (false, false) => mi.bmv[bmv_index(block)][ref_idx],
        // ss_idx 1
        (false, true) => mi_mv_pred_q2(mi, ref_idx, block, block + 2),
        // ss_idx 2
        (true, false) => mi_mv_pred_q2(mi, ref_idx, block, block + 1),
        // ss_idx 3
        (true, true) => mi_mv_pred_q4(mi, ref_idx),
    }
}

// ---------------------------------------------------------------------------
// The motion-compensation clamp (vp9_reconinter.c:91-111)
// ---------------------------------------------------------------------------

/// libvpx `clamp_mv_to_umv_border_sb` (`vp9_reconinter.c:91-111`) — the
/// **MC-stage** motion-vector clamp:
///
/// ```c
/// MV clamp_mv_to_umv_border_sb(const MACROBLOCKD *xd, const MV *src_mv, int bw,
///                              int bh, int ss_x, int ss_y) {
///   const int spel_left = (VP9_INTERP_EXTEND + bw) << SUBPEL_BITS;
///   const int spel_right = spel_left - SUBPEL_SHIFTS;
///   const int spel_top = (VP9_INTERP_EXTEND + bh) << SUBPEL_BITS;
///   const int spel_bottom = spel_top - SUBPEL_SHIFTS;
///   MV clamped_mv = { (short)(src_mv->row * (1 << (1 - ss_y))),
///                     (short)(src_mv->col * (1 << (1 - ss_x))) };
///   clamp_mv(&clamped_mv, xd->mb_to_left_edge * (1 << (1 - ss_x)) - spel_left,
///            xd->mb_to_right_edge * (1 << (1 - ss_x)) + spel_right,
///            xd->mb_to_top_edge * (1 << (1 - ss_y)) - spel_top,
///            xd->mb_to_bottom_edge * (1 << (1 - ss_y)) + spel_bottom);
///   return clamped_mv;
/// }
/// ```
///
/// Two things it does at once, and both matter:
///
/// * **Subsampling scale.** The input is 1/8-pel in *luma* units; the output
///   is 1/16-pel ("q4") in *this plane's* units. For luma (`ss == 0`) that
///   is a doubling; for a 4:2:0 chroma plane (`ss == 1`) the value passes
///   through unchanged, because halving the pixel size and doubling the
///   precision cancel.
/// * **Bounding.** Past the point where no visible pixel contributes, the
///   vector is pinned so the fetch stays within roughly one block plus
///   [`VP9_INTERP_EXTEND`] of the frame — which is what keeps
///   [`build_mc_border`]'s window bounded.
///
/// `bw`/`bh` are the **plane** block dimensions (libvpx's `4 * pd->n4_w` /
/// `4 * pd->n4_h`), *not* the 4x4 sub-block a sub-8x8 prediction writes:
/// `dec_build_inter_predictors_sb` passes `n4w_x4`/`n4h_x4` for every
/// sub-block of a sub-8x8 partition.
///
/// **`clamp_mv` takes its column bounds first** (`vp9_mv.h:47-51`:
/// `clamp_mv(MV *mv, int min_col, int max_col, int min_row, int max_row)`),
/// so `mb_to_left/right_edge` bound the column and `mb_to_top/bottom_edge`
/// the row.
///
/// Not to be confused with [`BlockEdges::clamp_mv_ref`], the candidate
/// clamp — see the module docs.
#[must_use]
pub fn clamp_mv_to_umv_border_sb(
    edges: &BlockEdges,
    src_mv: MotionVector,
    bw: usize,
    bh: usize,
    ss_x: usize,
    ss_y: usize,
) -> MotionVector {
    debug_assert!(ss_x <= 1, "libvpx assert(ss_x <= 1); got {ss_x}");
    debug_assert!(ss_y <= 1, "libvpx assert(ss_y <= 1); got {ss_y}");
    let spel_left = (VP9_INTERP_EXTEND + bw as i32) << SUBPEL_BITS;
    let spel_right = spel_left - SUBPEL_SHIFTS;
    let spel_top = (VP9_INTERP_EXTEND + bh as i32) << SUBPEL_BITS;
    let spel_bottom = spel_top - SUBPEL_SHIFTS;
    // `1 << (1 - ss)`: 2 for a full-resolution plane, 1 for a subsampled one.
    let scale_x = 1 << (1 - ss_x.min(1));
    let scale_y = 1 << (1 - ss_y.min(1));
    let col = i32::from(src_mv.col) * scale_x;
    let row = i32::from(src_mv.row) * scale_y;
    MotionVector::new(
        to_i16(clamp_i32(
            row,
            edges.to_top * scale_y - spel_top,
            edges.to_bottom * scale_y + spel_bottom,
        )),
        to_i16(clamp_i32(
            col,
            edges.to_left * scale_x - spel_left,
            edges.to_right * scale_x + spel_right,
        )),
    )
}

// ---------------------------------------------------------------------------
// Border construction (vp9_decodeframe.c:458-491)
// ---------------------------------------------------------------------------

/// libvpx `build_mc_border` (`vp9_decodeframe.c:458-491`): copies a
/// `b_w` x `b_h` window whose top-left corner is at reference-plane
/// coordinate `(x, y)` into `dst`, replicating the edges of the `w` x `h`
/// **display** rectangle for every coordinate outside it.
///
/// libvpx walks a `ref_row` pointer and splits each output row into
/// `memset(left) / memcpy(copy) / memset(right)`; this is that same split,
/// with the row pointer's clamp written out as the coordinate clamp it
/// implements. The pointer walk starts at `clamp(y, 0, h - 1)` and advances
/// only while the row index stays inside `1 ..= h - 1`, so output row `i`
/// always reads source row `clamp(y + i, 0, h - 1)` — identical to the form
/// used here, and much easier to check.
///
/// `w`/`h` are the plane's **display** dimensions
/// ([`plane_display_dims`]), never its MI-aligned allocation: a decoded
/// reference frame has no valid pixels outside them (see the module docs).
fn build_mc_border(
    src: &PlaneBuf,
    dst: &mut [u8],
    dst_stride: usize,
    x: i32,
    y: i32,
    b_w: usize,
    b_h: usize,
    w: usize,
    h: usize,
) {
    debug_assert!(w >= 1 && h >= 1, "empty display rectangle: {w}x{h}");
    debug_assert!(
        w <= src.stride && h <= src.height,
        "display rectangle outside plane"
    );
    let last_col = w as i32 - 1;
    let last_row = h as i32 - 1;
    for i in 0..b_h {
        let src_row = clamp_i32(y + i as i32, 0, last_row) as usize;
        let row_start = src_row * src.stride;
        let row = &src.data[row_start..row_start + w];

        // `int left = x < 0 ? -x : 0; if (left > b_w) left = b_w;`
        let left = usize::min(if x < 0 { (-x) as usize } else { 0 }, b_w);
        // `if (x + b_w > w) right = x + b_w - w; if (right > b_w) right = b_w;`
        let x_end = x + b_w as i32;
        let right = usize::min(
            if x_end > last_col + 1 {
                (x_end - w as i32) as usize
            } else {
                0
            },
            b_w,
        );
        // `copy = b_w - left - right;` — never negative for any window this
        // module forms (both clamps active means `copy == w`), but formed
        // saturating so a hypothetical inverted window degrades to an
        // edge-replicated row instead of panicking.
        let copy = b_w.saturating_sub(left).saturating_sub(right);

        let out = &mut dst[i * dst_stride..i * dst_stride + b_w];
        if left > 0 {
            out[..left].fill(row[0]);
        }
        if copy > 0 {
            let src_start = (x + left as i32).max(0) as usize;
            out[left..left + copy].copy_from_slice(&row[src_start..src_start + copy]);
        }
        if right > 0 {
            out[left + copy..].fill(row[w - 1]);
        }
    }
}

/// libvpx `extend_and_predict` (`vp9_decodeframe.c:558-580`, 8-bit build):
/// build the edge-replicated window into a scratch buffer, then run the
/// ordinary interpolation out of it.
///
/// libvpx reuses one per-tile-worker `extend_and_predict_buf`; this
/// allocates the window it actually needs (at most `(64 + 8)^2` bytes for a
/// 64x64 block with both axes filtered), which is behaviourally identical.
fn extend_and_predict(
    ref_plane: &PlaneBuf,
    x0: i32,
    y0: i32,
    b_w: usize,
    b_h: usize,
    frame_width: usize,
    frame_height: usize,
    border_offset: usize,
    dst: &mut PlaneBuf,
    dst_origin: usize,
    subpel_x: usize,
    subpel_y: usize,
    kernels: &InterpKernels,
    w: usize,
    h: usize,
    avg: bool,
) {
    let mut mc_buf = vec![0u8; b_w * b_h];
    build_mc_border(
        ref_plane,
        &mut mc_buf,
        b_w,
        x0,
        y0,
        b_w,
        b_h,
        frame_width,
        frame_height,
    );
    let dst_stride = dst.stride;
    mc::convolve(
        &mc_buf,
        b_w,
        border_offset,
        &mut dst.data,
        dst_stride,
        dst_origin,
        kernel_row(kernels, subpel_x),
        kernel_row(kernels, subpel_y),
        w,
        h,
        avg,
    );
}

/// The already-phase-selected kernel row [`super::mc::convolve`] wants for
/// one axis: `FILTER_KERNELS[filter][subpel]`, or `None` when the sub-pixel
/// fraction is zero — libvpx's `predict[subpel_x != 0][subpel_y != 0]`
/// dispatch (`vp9_reconinter.h:23-32`) expressed as an `Option`.
fn kernel_row(kernels: &InterpKernels, subpel: usize) -> Option<&[i16; SUBPEL_TAPS]> {
    if subpel == 0 {
        None
    } else {
        kernels.get(subpel)
    }
}

// ---------------------------------------------------------------------------
// Per-plane predictor build (vp9_decodeframe.c:578-714)
// ---------------------------------------------------------------------------

/// libvpx `dec_build_inter_predictors` (`vp9_decodeframe.c:578-714`,
/// unscaled 8-bit path): predicts the `w` x `h` region at offset `(x, y)`
/// inside one plane of one block, from one reference.
///
/// * `dst` — the current frame's plane (MI-aligned, no border).
/// * `ref_plane` / `frame_width` / `frame_height` — the reference frame's
///   matching plane and its **display** rectangle
///   ([`plane_display_dims`]); every fetch outside that rectangle is
///   edge-replicated by [`build_mc_border`].
/// * `edges` — the block's `mb_to_*_edge` values ([`BlockEdges::new`] with
///   the block's `sb_type`), in 1/8-pel luma units. libvpx derives the
///   block's plane origin from them, and so does this.
/// * `x` / `y` / `w` / `h` — the sub-block's offset and size within the
///   block (`0, 0, bw, bh` for everything except a sub-8x8 partition,
///   where every sub-block is 4x4).
/// * `mv` — the block's (or sub-block's) vector in 1/8-pel luma units, as
///   decoded. libvpx's unscaled path passes `mi->mv[ref]` / the
///   `average_split_mvs` result through unclamped; so does this.
/// * `avg` — libvpx's `ref` index as [`super::mc::convolve`] consumes it:
///   `false` for the first reference (overwrite), `true` for the second of
///   a compound block (rounded average into what the first wrote).
///
/// The write is clipped to `dst`'s allocation; see "Dead overhang" in the
/// module docs for why that is not a deviation.
fn dec_build_inter_predictors(
    dst: &mut PlaneBuf,
    ref_plane: &PlaneBuf,
    frame_width: usize,
    frame_height: usize,
    edges: &BlockEdges,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    mv: MotionVector,
    kernels: &InterpKernels,
    ss_x: usize,
    ss_y: usize,
    avg: bool,
) {
    // libvpx's unscaled `else` branch, verbatim:
    //
    // ```c
    // x0 = (-xd->mb_to_left_edge >> (3 + pd->subsampling_x)) + x;
    // y0 = (-xd->mb_to_top_edge >> (3 + pd->subsampling_y)) + y;
    // x0_16 = x0 << SUBPEL_BITS;
    // y0_16 = y0 << SUBPEL_BITS;
    // scaled_mv.row = mv->row * (1 << (1 - pd->subsampling_y));
    // scaled_mv.col = mv->col * (1 << (1 - pd->subsampling_x));
    // xs = ys = 16;
    // ```
    //
    // Note what is *not* here: [`clamp_mv_to_umv_border_sb`]. The decoder
    // applies the MC-stage clamp only on its scaled branch — see the module
    // docs, "Two different MV clamps", for why an unclamped far-out vector
    // reaches the same pixels (and why it cannot run away: the fetch window
    // stays `w + 8` wide however far out the vector points).
    //
    // Co-ordinate of containing block to pixel precision, then to 1/16th
    // pixel precision. `mb_to_left_edge` is `-((mi_col * MI_SIZE) * 8)`, so
    // `-mb_to_left_edge >> (3 + ss_x)` is `(mi_col * MI_SIZE) >> ss_x` —
    // exactly the pixel `setup_pred_plane` / `vp9_setup_dst_planes` point
    // this plane's buffers at.
    let plane_x0 = (-edges.to_left >> (3 + ss_x)) as usize;
    let plane_y0 = (-edges.to_top >> (3 + ss_y)) as usize;
    let mut x0 = (plane_x0 + x) as i32;
    let mut y0 = (plane_y0 + y) as i32;
    let mut x0_16 = x0 << SUBPEL_BITS;
    let mut y0_16 = y0 << SUBPEL_BITS;

    // `scaled_mv` is libvpx's `MV32` — 32-bit fields, so the subsampling
    // doubling of a full-resolution plane never narrows.
    let scaled_mv_col = i32::from(mv.col) * (1 << (1 - ss_x.min(1)));
    let scaled_mv_row = i32::from(mv.row) * (1 << (1 - ss_y.min(1)));

    let subpel_x = (scaled_mv_col & SUBPEL_MASK) as usize;
    let subpel_y = (scaled_mv_row & SUBPEL_MASK) as usize;

    // Calculate the top left corner of the best matching block in the
    // reference frame. `>>` on a negative value is an arithmetic shift in
    // Rust, matching C's behaviour for these signed shifts (a negative
    // vector must floor, not truncate: -1 in q4 is integer offset -1 with
    // sub-pixel phase 15).
    x0 += scaled_mv_col >> SUBPEL_BITS;
    y0 += scaled_mv_row >> SUBPEL_BITS;
    x0_16 += scaled_mv_col;
    y0_16 += scaled_mv_row;

    // `buf_ptr = ref_frame + y0 * pre_buf->stride + x0` — captured *before*
    // the border block below moves `x0`/`y0`, exactly as libvpx does.
    let direct_x0 = x0;
    let direct_y0 = y0;

    // Destination, and the clipped extent actually written (module docs,
    // "Dead overhang"). Every border decision below uses the unclipped
    // `w`/`h`, as libvpx does.
    let dst_x = plane_x0 + x;
    let dst_y = plane_y0 + y;
    debug_assert!(
        dst_x < dst.width && dst_y < dst.height,
        "block origin outside its own plane: ({dst_x}, {dst_y}) in {}x{}",
        dst.width,
        dst.height
    );
    let w_eff = w.min(dst.width.saturating_sub(dst_x));
    let h_eff = h.min(dst.height.saturating_sub(dst_y));
    if w_eff == 0 || h_eff == 0 {
        return;
    }
    let dst_origin = dst_y * dst.stride + dst_x;

    let fw = frame_width as i32;
    let fh = frame_height as i32;

    // Do border extension if there is motion or the width/height is not a
    // multiple of 8 pixels. (`is_scaled ||` dropped: always false here.)
    if scaled_mv_col != 0
        || scaled_mv_row != 0
        || (frame_width & 0x7) != 0
        || (frame_height & 0x7) != 0
    {
        // Reference block bottom-right coordinates, `xs == ys == 16`.
        let mut y1 = ((y0_16 + (h as i32 - 1) * SUBPEL_SHIFTS) >> SUBPEL_BITS) + 1;
        let mut x1 = ((x0_16 + (w as i32 - 1) * SUBPEL_SHIFTS) >> SUBPEL_BITS) + 1;
        let mut x_pad = 0usize;
        let mut y_pad = 0usize;

        // `if (subpel_x || (sf->x_step_q4 != SUBPEL_SHIFTS))` — the step
        // term is constant-false for an unscaled reference.
        if subpel_x != 0 {
            x0 -= VP9_INTERP_EXTEND - 1;
            x1 += VP9_INTERP_EXTEND;
            x_pad = 1;
        }
        if subpel_y != 0 {
            y0 -= VP9_INTERP_EXTEND - 1;
            y1 += VP9_INTERP_EXTEND;
            y_pad = 1;
        }

        // Skip border extension if block is inside the frame.
        if x0 < 0
            || x0 > fw - 1
            || x1 < 0
            || x1 > fw - 1
            || y0 < 0
            || y0 > fh - 1
            || y1 < 0
            || y1 > fh - 1
        {
            let b_w = (x1 - x0 + 1) as usize;
            let b_h = (y1 - y0 + 1) as usize;
            let border_offset = y_pad * 3 * b_w + x_pad * 3;
            extend_and_predict(
                ref_plane,
                x0,
                y0,
                b_w,
                b_h,
                frame_width,
                frame_height,
                border_offset,
                dst,
                dst_origin,
                subpel_x,
                subpel_y,
                kernels,
                w_eff,
                h_eff,
                avg,
            );
            return;
        }
    } else if direct_x0 < 0
        || direct_x0 + w as i32 > fw - 1
        || direct_y0 < 0
        || direct_y0 + h as i32 > fh - 1
    {
        // Not libvpx control flow — the memory-safety branch described in
        // the module docs ("The one addition to libvpx's control flow").
        // Reached only by a block overhanging the MI grid with a zero
        // vector and 8-multiple display dimensions, where libvpx reads its
        // uninitialised frame border for pixels that are then discarded.
        // The window and pads are the ones the inner check would have
        // computed here (`x1 = x0 + w`, `y1 = y0 + h`, no pads, because a
        // zero `scaled_mv` forces `subpel_x == subpel_y == 0`).
        let b_w = w + 1;
        let b_h = h + 1;
        extend_and_predict(
            ref_plane,
            direct_x0,
            direct_y0,
            b_w,
            b_h,
            frame_width,
            frame_height,
            0,
            dst,
            dst_origin,
            subpel_x,
            subpel_y,
            kernels,
            w_eff,
            h_eff,
            avg,
        );
        return;
    }

    let src_origin = (direct_y0 * ref_plane.stride as i32 + direct_x0) as usize;
    let src_stride = ref_plane.stride;
    let dst_stride = dst.stride;
    mc::convolve(
        &ref_plane.data,
        src_stride,
        src_origin,
        &mut dst.data,
        dst_stride,
        dst_origin,
        kernel_row(kernels, subpel_x),
        kernel_row(kernels, subpel_y),
        w_eff,
        h_eff,
        avg,
    );
}

// ---------------------------------------------------------------------------
// Whole-block predictor build (vp9_decodeframe.c:716-786)
// ---------------------------------------------------------------------------

/// libvpx `dec_build_inter_predictors_sb` (`vp9_decodeframe.c:716-786`):
/// builds every plane's inter prediction for one block, from one or two
/// references.
///
/// `block_refs[i]` is the decoded-picture-buffer slot named by
/// `mi.ref_frame[i]`; the caller resolves LAST/GOLDEN/ALTREF through the
/// frame's `ref_frame_idx`. Only `block_refs[0]` is consulted for a
/// single-reference block.
///
/// The reference loop is the **outer** one, exactly as libvpx has it, and
/// the second reference's pass averages into what the first wrote
/// ([`super::mc::convolve`]'s `avg`) — VP9 has no separate blend step.
///
/// # Sub-8x8 contract with the mode parser
///
/// For any `sb_type < BLOCK_8X8` — `BLOCK_4X4`, **and `BLOCK_4X8` /
/// `BLOCK_8X4`** — this reads all four [`MiInfo::bmv`] entries: luma
/// predicts `num_4x4_w * num_4x4_h == 2 * 2` sub-blocks whatever the
/// partition shape, and 4:2:0 chroma averages all four vectors. libvpx can
/// do that because `read_inter_block_mode_info` (`vp9_decodemv.c`)
/// *replicates* the two decoded vectors of a 4X8/8X4 block over the unused
/// entries:
///
/// ```c
/// if (num_4x4_h == 2) mi->bmi[j + 2] = mi->bmi[j];
/// if (num_4x4_w == 2) mi->bmi[j + 1] = mi->bmi[j];
/// ```
///
/// The mode parser must reproduce that replication. A parser that filled
/// only the decoded entries would leave zero vectors in `bmv[1]` / `bmv[3]`
/// and this function would predict half of a 4X8/8X4 block from them,
/// silently and only for those two block sizes.
///
/// # Errors
///
/// * [`CodecError::UnsupportedFeature`] when an active reference's display
///   dimensions differ from the current frame's — reference scaling is VP9
///   package P15 and is refused rather than mispredicted.
/// * [`CodecError::InvalidBitstream`] when an active reference slot is
///   empty, or when the block reached motion compensation with an
///   unresolved `interp_filter`.
pub fn dec_build_inter_predictors_sb(
    dst_planes: &mut [PlaneBuf; MAX_MB_PLANE],
    mi: &MiInfo,
    block_refs: [Option<&Vp9RefSlot>; 2],
    geom: &FrameGeometry,
    mi_row: usize,
    mi_col: usize,
) -> CodecResult<()> {
    // `const InterpKernel *kernel = vp9_filter_kernels[mi->interp_filter];`
    // — libvpx's table has a fifth (4-tap) entry a VP9 block never selects;
    // `SWITCHABLE` (4) must have been resolved to a concrete filter by the
    // mode parser.
    let filter = usize::from(mi.interp_filter);
    let kernels = FILTER_KERNELS.get(filter).ok_or_else(|| {
        CodecError::InvalidBitstream(format!(
            "VP9 inter block reached motion compensation with interp_filter={filter} \
             (0..=3 expected; SWITCHABLE=4 must be resolved when the mode is read)"
        ))
    })?;

    let is_compound = mi.ref_frame[1] > super::refs::INTRA_FRAME;
    let edges = BlockEdges::new(mi_row, mi_col, mi.sb_type, geom.mi_rows, geom.mi_cols);
    let idx = bsize_index(mi.sb_type);
    let num_8x8_w = usize::from(NUM_8X8_BLOCKS_WIDE[idx]);
    let num_8x8_h = usize::from(NUM_8X8_BLOCKS_HIGH[idx]);

    for ref_idx in 0..1 + usize::from(is_compound) {
        let slot = block_refs[ref_idx].ok_or_else(|| {
            CodecError::InvalidBitstream(format!(
                "VP9 inter block names reference frame {} for slot {ref_idx}, \
                 but that decoded-picture-buffer slot is empty",
                mi.ref_frame[ref_idx]
            ))
        })?;
        // `if (!vp9_is_valid_scale(sf)) vpx_internal_error(...)` plus
        // `vp9_is_scaled(sf)`: equal dimensions is the only case this
        // module implements, and it subsumes libvpx's validity check.
        if slot.width != geom.width || slot.height != geom.height {
            return Err(CodecError::UnsupportedFeature(format!(
                "VP9 reference scaling not yet implemented (reference {}x{} vs frame {}x{})",
                slot.width, slot.height, geom.width, geom.height
            )));
        }
        // The second reference of a compound block averages into the first
        // one's output (libvpx's `ref` argument to `inter_predictor`).
        let avg = ref_idx == 1;

        for plane in 0..MAX_MB_PLANE {
            let (ss_x, ss_y) = geom.plane_ss(plane);
            // `pd->n4_w = (bw << 1) >> ss_x;` (`set_plane_n4`), then
            // `n4w_x4 = 4 * num_4x4_w`.
            let n4_w = (num_8x8_w << 1) >> ss_x;
            let n4_h = (num_8x8_h << 1) >> ss_y;
            // libvpx reads the *reference* buffer's crop dimensions here
            // (`ref_frame_buf->buf.y_crop_width` / `uv_crop_width`), not the
            // current frame's — identical after the equal-dimensions check
            // above, and written that way so the distinction survives when
            // P15 lifts the check.
            let (frame_width, frame_height) =
                plane_display_dims(slot.width, slot.height, ss_x, ss_y);
            let ref_plane = &slot.planes[plane];
            let dst = &mut dst_planes[plane];

            if mi.sb_type < BLOCK_8X8 {
                // `int i = 0` is declared inside the plane loop: the
                // sub-block index restarts per plane.
                let mut block = 0usize;
                for by in 0..n4_h {
                    for bx in 0..n4_w {
                        let mv = average_split_mvs(ss_x, ss_y, mi, ref_idx, block);
                        block += 1;
                        dec_build_inter_predictors(
                            dst,
                            ref_plane,
                            frame_width,
                            frame_height,
                            &edges,
                            4 * bx,
                            4 * by,
                            4,
                            4,
                            mv,
                            kernels,
                            ss_x,
                            ss_y,
                            avg,
                        );
                    }
                }
            } else {
                dec_build_inter_predictors(
                    dst,
                    ref_plane,
                    frame_width,
                    frame_height,
                    &edges,
                    0,
                    0,
                    4 * n4_w,
                    4 * n4_h,
                    mi.mv[ref_idx],
                    kernels,
                    ss_x,
                    ss_y,
                    avg,
                );
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
