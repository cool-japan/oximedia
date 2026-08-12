//! VP9 motion-vector reference candidate scan — the spatial + temporal
//! neighbour search that produces a block's `nearestmv` / `nearmv` predictors
//! and the context that decodes its inter mode.
//!
//! Port of libvpx `vp9/common/vp9_mvref_common.c` and the inline helpers of
//! its header `vp9/common/vp9_mvref_common.h`, tag **v1.15.2** — the same tag
//! [`super::tables_inter`] and [`super::predctx`] cite. Every function carries
//! a `vp9_mvref_common.{c,h}:<lines>` citation for the source range it
//! reproduces; supporting definitions pulled from other files
//! (`vp9_mv.h`, `vp9_entropymv.h`, `vp9_onyxc_int.h`, `vpx_dsp/vpx_dsp_common.h`,
//! `vpx_scale/yv12config.h`) are cited where they are used.
//!
//! The three constant tables this scan needs — `mv_ref_blocks`,
//! `mode_2_counter` and `counter_to_context` — already live in
//! [`super::tables_inter`] and are used from there, not re-transcribed.
//!
//! # Common code vs. the decoder's specialised copy
//!
//! libvpx has *two* implementations of this search:
//!
//! * `find_mv_refs_idx` / `vp9_find_mv_refs` (`vp9_mvref_common.c:16-139`) —
//!   the common one, shared with the encoder. It scans every neighbour, fills
//!   both list entries, and accumulates the mode context inline.
//! * `dec_find_mv_refs` + `get_mode_context` (`vp9/decoder/vp9_decodemv.c:484-607`
//!   and `:670-689`) — the decoder's copy, which adds an `early_break` for
//!   every mode except `NEARMV` (it only needs `mv_list[0]` then) and lifts
//!   the mode-context accumulation into a separate two-neighbour pass.
//!
//! **This module ports the common form**, which is the simpler one to verify,
//! and is *output-identical* to the decoder's copy for everything the decoder
//! reads:
//!
//! 1. `mv_list[0]` and `mv_list[1]` — the scan order is the same, and
//!    `early_break` only stops a scan *after* the entry the caller will read
//!    has been written (`refmv_count - 1` is `0` for the early-breaking modes,
//!    `1` for `NEARMV`, which never early-breaks).
//! 2. The clamp — `dec_find_mv_refs` clamps `refmv_count` entries, the common
//!    form clamps all [`MAX_MV_REF_CANDIDATES`]. The extra entry the common
//!    form clamps is one the decoder never reads for that mode, and for the
//!    entries it *does* read the two agree. Clamping the unfilled entry is
//!    **not** cosmetic: at a frame edge the clamp range can exclude zero, so a
//!    zero-filled entry legitimately comes out non-zero (see
//!    `frame_edge_clamps_the_unfilled_entry` in the tests).
//! 3. `mode_context` — the common form accumulates over the first two
//!    neighbours and can only leave that loop early at `i == 1`, *after* that
//!    neighbour's `mode_2_counter` contribution has been added
//!    (`vp9_mvref_common.c:43` precedes `:46`), so the counter always holds
//!    both neighbours' votes — exactly what the decoder's `get_mode_context`
//!    computes. [`get_mode_context`] ports that decoder helper for callers
//!    that need the context *before* a reference frame is known (a
//!    `SEG_LVL_SKIP` block never runs the candidate scan at all), and
//!    `get_mode_context_agrees_with_find_mv_refs` pins the equality.
//!
//! # Two different clamps
//!
//! Do not confuse the clamp applied here with the one motion compensation
//! applies later:
//!
//! * [`BlockEdges::clamp_mv_ref`] — libvpx `clamp_mv_ref`
//!   (`vp9_mvref_common.h:216-220`), margin `MV_BORDER` = 16 pixels in 1/8-pel
//!   units. This one bounds *candidate* vectors and is part of the bitstream
//!   semantics: it changes decoded output.
//! * `clamp_mv_to_umv_border_sb` (`vp9/common/vp9_reconinter.c`) — the
//!   MC-stage clamp that keeps a *chosen* vector's fetch inside the reference
//!   frame's border. It is **not** in this module; it belongs to the
//!   motion-compensation package.
//!
//! [`BlockEdges::clamp_mv2`] is a third one (`vp9_mvref_common.h:287-292`,
//! margin `LEFT_TOP_MARGIN` = 1248): the encoder-side clamp
//! `vp9_find_best_ref_mvs` applies. See [`find_best_ref_mvs`] for why it
//! cannot change a list this module produced.
//!
//! # Grid access
//!
//! The scan reads decoded neighbours through the [`MiGrid`] trait rather than
//! [`FrameMi`] directly. [`FrameMi`] implements it, so the decode path passes
//! the real grid; the trait exists because `FrameMi`'s constructor is private
//! to [`super::recon`], and a candidate scan that cannot be driven by
//! synthetic neighbourhoods cannot be tested branch by branch.
//!
//! # Reference-frame and mode numbering
//!
//! `ref_frame` values are libvpx's `MV_REFERENCE_FRAME`
//! ([`INTRA_FRAME`](super::refs::INTRA_FRAME) `= 0`, LAST/GOLDEN/ALTREF
//! `= 1..=3`, [`NONE_FRAME`](super::refs::NONE_FRAME) `= -1`), and
//! [`MiInfo::mode`] is the raw `MB_MODE_COUNT` mode number (`NEARESTMV = 10`
//! … `NEWMV = 13`) that [`MODE_2_COUNTER`] is indexed by — *not* the
//! `INTER_OFFSET`-relative 0..=3 numbering of
//! [`INTER_MODE_NEARESTMV`](super::tables_inter::INTER_MODE_NEARESTMV).

#![forbid(unsafe_code)]

use super::predctx::{has_second_ref, is_inter_block};
use super::recon::{FrameMi, MiInfo};
use super::refs::{MvRefRow, INTRA_FRAME};
use super::tables::{NUM_8X8_BLOCKS_HIGH, NUM_8X8_BLOCKS_WIDE};
use super::tables_inter::{
    COUNTER_INVALID_CASE, COUNTER_TO_CONTEXT, MODE_2_COUNTER, MV_REF_BLOCKS,
};
use crate::vp9::mv::MotionVector;

/// Number of candidates a reference list holds — libvpx
/// `MAX_MV_REF_CANDIDATES` (`vp9/common/vp9_enums.h:135`).
pub const MAX_MV_REF_CANDIDATES: usize = 2;

/// Number of neighbour positions probed per block size — libvpx
/// `MVREF_NEIGHBOURS` (`vp9_mvref_common.h:24`), the column count of
/// [`MV_REF_BLOCKS`].
pub const MVREF_NEIGHBOURS: usize = 8;

/// Candidate-clamp margin — libvpx `MV_BORDER` (`vp9_mvref_common.h:214`):
/// `(16 << 3)`, "Allow 16 pels in 1/8th pel units".
pub const MV_BORDER: i32 = 16 << 3;

/// libvpx `BLOCK_8X8` (`vp9/common/vp9_enums.h:49`): the first block size that
/// is *not* sub-8x8, i.e. the `sb_type` threshold below which a candidate
/// carries per-sub-block motion vectors.
pub const BLOCK_8X8: u8 = 3;

/// Number of VP9 block sizes (`BLOCK_SIZES`) — the row count of
/// [`MV_REF_BLOCKS`] and of the `num_8x8_blocks_*_lookup` tables.
const BLOCK_SIZES: usize = 13;

/// Pixels per mode-info unit — libvpx `MI_SIZE` (`vp9_enums.h:24`).
const MI_SIZE: i32 = 8;

/// libvpx `LEFT_TOP_MARGIN` / `RIGHT_BOTTOM_MARGIN`
/// (`vp9_mvref_common.h:20-22`):
/// `((VP9_ENC_BORDER_IN_PIXELS - VP9_INTERP_EXTEND) << 3)` with
/// `VP9_ENC_BORDER_IN_PIXELS = 160` and `VP9_INTERP_EXTEND = 4`
/// (`vpx_scale/yv12config.h:25-26`), i.e. `156 << 3` = 1248. Both margins are
/// the same expression in libvpx; one constant covers both.
const UMV_MARGIN: i32 = (160 - 4) << 3;

/// Which sub-block of a sub-8x8 candidate supplies the motion vector —
/// libvpx `idx_n_column_to_subblock` (`vp9_mvref_common.h:209-211`):
///
/// ```c
/// static const int idx_n_column_to_subblock[4][2] = {
///   { 1, 2 }, { 1, 3 }, { 3, 2 }, { 3, 3 }
/// };
/// ```
///
/// Indexed `[block_idx][(search_col == 0) as usize]`, so the **second**
/// column is the above neighbour (whose column offset is 0) and the **first**
/// is a left neighbour. For sub-block 0 that means: the above neighbour
/// contributes its sub-block 2 (bottom-left, the one adjacent across the
/// horizontal edge) and the left neighbour its sub-block 1 (top-right,
/// adjacent across the vertical edge).
const IDX_N_COLUMN_TO_SUBBLOCK: [[usize; 2]; 4] = [[1, 2], [1, 3], [3, 2], [3, 3]];

// ---------------------------------------------------------------------------
// Grid access
// ---------------------------------------------------------------------------

/// Read access to the frame's decoded mode-info grid, as the candidate scan
/// needs it (libvpx `xd->mi[col + row * xd->mi_stride]` over
/// `cm->mi_rows` x `cm->mi_cols`).
pub trait MiGrid {
    /// Grid height in MI units (libvpx `cm->mi_rows`).
    fn mi_rows(&self) -> usize;
    /// Grid width in MI units (libvpx `cm->mi_cols`).
    fn mi_cols(&self) -> usize;
    /// Mode info at `(row, col)`; callers guarantee `row < mi_rows()` and
    /// `col < mi_cols()`.
    fn mi_at(&self, row: usize, col: usize) -> MiInfo;
}

impl MiGrid for FrameMi {
    fn mi_rows(&self) -> usize {
        self.rows
    }
    fn mi_cols(&self) -> usize {
        self.cols
    }
    fn mi_at(&self, row: usize, col: usize) -> MiInfo {
        self.get(row, col)
    }
}

/// The tile's mode-info **column** range — libvpx `TileInfo`
/// (`vp9/common/vp9_tile_common.h:20-23`), of which
/// [`is_inside`] reads only `mi_col_start` / `mi_col_end`.
///
/// VP9's `is_inside` has **no tile-row bound**: a candidate above the current
/// block is rejected only by the frame top (`mi_row + row < 0`) and the frame
/// bottom (`>= mi_rows`), never by a tile row start. Tile *rows* exist in VP9
/// but do not restrict this scan (unlike AV1, whose equivalent tests
/// `mi_row_start`). Adding a row test here would silently change decoded
/// output on multi-tile-row streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileBounds {
    /// First MI column of the tile (inclusive), libvpx `tile->mi_col_start`.
    pub mi_col_start: usize,
    /// One past the last MI column of the tile, libvpx `tile->mi_col_end`.
    /// Callers must keep this within the grid (`<= mi_cols`).
    pub mi_col_end: usize,
}

impl TileBounds {
    /// A tile spanning the whole frame width — the single-tile case, and what
    /// the tests use unless they are exercising a tile boundary.
    #[must_use]
    pub fn full_width(mi_cols: usize) -> Self {
        Self {
            mi_col_start: 0,
            mi_col_end: mi_cols,
        }
    }
}

/// Whether a candidate position is available — verbatim libvpx `is_inside`
/// (`vp9_mvref_common.h:278-284`):
///
/// ```c
/// return !(mi_row + mi_pos->row < 0 ||
///          mi_col + mi_pos->col < tile->mi_col_start ||
///          mi_row + mi_pos->row >= mi_rows ||
///          mi_col + mi_pos->col >= tile->mi_col_end);
/// ```
///
/// The argument order (`mi_col` before `mi_row`) is libvpx's; `pos` is a
/// `[row, col]` pair from [`MV_REF_BLOCKS`], **row first** — the same
/// transposition hazard that table's doc comment warns about.
///
/// MI coordinates are bounded by VP9's 2^16-pixel frame limit (`mi < 2^13`),
/// so the `i32` arithmetic below cannot overflow for any grid the decoder
/// builds.
#[must_use]
pub fn is_inside(
    tile: &TileBounds,
    mi_col: usize,
    mi_row: usize,
    mi_rows: usize,
    pos: [i8; 2],
) -> bool {
    let row = mi_row as i32 + i32::from(pos[0]);
    let col = mi_col as i32 + i32::from(pos[1]);
    !(row < 0
        || col < tile.mi_col_start as i32
        || row >= mi_rows as i32
        || col >= tile.mi_col_end as i32)
}

// ---------------------------------------------------------------------------
// Block edges and the candidate clamp
// ---------------------------------------------------------------------------

/// The current block's distances to the frame edges in 1/8-pel units —
/// libvpx's `xd->mb_to_{left,right,top,bottom}_edge`, assigned by
/// `set_mi_row_col` (`vp9/common/vp9_onyxc_int.h:421-432`):
///
/// ```c
/// xd->mb_to_top_edge = -((mi_row * MI_SIZE) * 8);
/// xd->mb_to_bottom_edge = ((mi_rows - bh - mi_row) * MI_SIZE) * 8;
/// xd->mb_to_left_edge = -((mi_col * MI_SIZE) * 8);
/// xd->mb_to_right_edge = ((mi_cols - bw - mi_col) * MI_SIZE) * 8;
/// ```
///
/// with `MI_SIZE = 8`, so each MI column is worth 64 units. The right/bottom
/// edges are **legitimately negative** for a block that overhangs the frame
/// edge (`mi_cols - bw - mi_col < 0`), which is why every field is signed and
/// why a zero motion vector can clamp to a non-zero one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockEdges {
    /// libvpx `mb_to_left_edge` (`<= 0`).
    pub to_left: i32,
    /// libvpx `mb_to_right_edge`.
    pub to_right: i32,
    /// libvpx `mb_to_top_edge` (`<= 0`).
    pub to_top: i32,
    /// libvpx `mb_to_bottom_edge`.
    pub to_bottom: i32,
}

impl BlockEdges {
    /// Edges of the `bsize` block at `(mi_row, mi_col)` in a
    /// `mi_rows` x `mi_cols` grid — libvpx `set_mi_row_col`.
    #[must_use]
    pub fn new(mi_row: usize, mi_col: usize, bsize: u8, mi_rows: usize, mi_cols: usize) -> Self {
        let idx = bsize_index(bsize);
        let bw = i32::from(NUM_8X8_BLOCKS_WIDE[idx]);
        let bh = i32::from(NUM_8X8_BLOCKS_HIGH[idx]);
        let (mi_row, mi_col) = (mi_row as i32, mi_col as i32);
        let (mi_rows, mi_cols) = (mi_rows as i32, mi_cols as i32);
        Self {
            to_left: -((mi_col * MI_SIZE) * 8),
            to_right: ((mi_cols - bw - mi_col) * MI_SIZE) * 8,
            to_top: -((mi_row * MI_SIZE) * 8),
            to_bottom: ((mi_rows - bh - mi_row) * MI_SIZE) * 8,
        }
    }

    /// The candidate clamp — verbatim libvpx `clamp_mv_ref`
    /// (`vp9_mvref_common.h:216-220`):
    ///
    /// ```c
    /// clamp_mv(mv, xd->mb_to_left_edge - MV_BORDER,
    ///          xd->mb_to_right_edge + MV_BORDER, xd->mb_to_top_edge - MV_BORDER,
    ///          xd->mb_to_bottom_edge + MV_BORDER);
    /// ```
    ///
    /// **`clamp_mv` takes its column bounds first** (`vp9_mv.h:47-51`:
    /// `clamp_mv(MV *mv, int min_col, int max_col, int min_row, int max_row)`),
    /// so left/right bound the *column* and top/bottom the *row*.
    ///
    /// Not to be confused with the motion-compensation clamp
    /// (`clamp_mv_to_umv_border_sb`), which is a different function with a
    /// different margin and lives in the MC package.
    #[must_use]
    pub fn clamp_mv_ref(&self, mv: MotionVector) -> MotionVector {
        self.clamp_mv(mv, MV_BORDER)
    }

    /// libvpx `clamp_mv2` (`vp9_mvref_common.h:287-292`) — the same clamp with
    /// the much wider `LEFT_TOP_MARGIN` / `RIGHT_BOTTOM_MARGIN` (1248) margin,
    /// applied by `vp9_find_best_ref_mvs`. See [`find_best_ref_mvs`]: it
    /// cannot change a list produced by [`find_mv_refs`], and the decoder
    /// (`vp9_decodemv.c:751,779`) does not call it at all.
    #[must_use]
    pub fn clamp_mv2(&self, mv: MotionVector) -> MotionVector {
        self.clamp_mv(mv, UMV_MARGIN)
    }

    /// Shared body of the two clamps above (libvpx `clamp_mv`, `vp9_mv.h:47-51`).
    fn clamp_mv(&self, mv: MotionVector, margin: i32) -> MotionVector {
        MotionVector::new(
            to_i16(clamp_i32(
                i32::from(mv.row),
                self.to_top - margin,
                self.to_bottom + margin,
            )),
            to_i16(clamp_i32(
                i32::from(mv.col),
                self.to_left - margin,
                self.to_right + margin,
            )),
        )
    }
}

/// libvpx `clamp` (`vpx_dsp/vpx_dsp_common.h:66-68`):
/// `value < low ? low : (value > high ? high : value)`.
///
/// Transcribed as the ternary chain rather than `Ord::clamp`, which panics
/// when `low > high`; libvpx's form degrades to `low` instead. A block wide
/// enough to invert the bounds is unreachable from a conformant bitstream but
/// trivially constructible in a test.
const fn clamp_i32(value: i32, low: i32, high: i32) -> i32 {
    if value < low {
        low
    } else if value > high {
        high
    } else {
        value
    }
}

/// Narrows a clamped edge value back to the `int16_t` a `MV` component is.
///
/// The clamp bounds can exceed the `i16` range on large frames, but a bound is
/// only ever *selected* when the value being clamped is outside it, and an
/// `i16` input can never be outside a bound that is itself out of `i16` range
/// — so this never actually saturates. It is written as a saturating narrow
/// rather than a cast so the unreachable case stays defined.
fn to_i16(value: i32) -> i16 {
    clamp_i32(value, i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Bounds-checks a `BLOCK_SIZE` before it indexes a 13-entry lookup.
///
/// `bsize` comes from [`MiInfo::sb_type`], which the partition decode
/// constrains to `0..BLOCK_SIZES`; the clamp keeps the lookup total for a
/// caller that gets it wrong instead of panicking mid-frame.
fn bsize_index(bsize: u8) -> usize {
    debug_assert!(
        (bsize as usize) < BLOCK_SIZES,
        "VP9 BLOCK_SIZE out of range: {bsize}"
    );
    (bsize as usize).min(BLOCK_SIZES - 1)
}

// ---------------------------------------------------------------------------
// Candidate list
// ---------------------------------------------------------------------------

/// Result of a candidate scan — libvpx's `mv_ref_list` plus the mode context
/// `find_mv_refs_idx` writes through `mode_context[ref_frame]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MvRefResult {
    /// The two candidates, clamped ([`BlockEdges::clamp_mv_ref`]). Entries
    /// past [`count`](Self::count) are the zero fill libvpx `memset`s in
    /// (`vp9_mvref_common.c:32`) — *also clamped*, so they are not necessarily
    /// zero.
    pub list: [MotionVector; MAX_MV_REF_CANDIDATES],
    /// How many candidates the scan actually wrote (0, 1 or 2).
    ///
    /// **This is not libvpx's `refmv_count` at `Done`.** The common
    /// `ADD_MV_REF_LIST` (`vp9_mvref_common.h:248-258`) writes the second
    /// entry *without* incrementing (`mv_ref_list[refmv_count] = mv; goto
    /// Done;`), so libvpx's counter reads `1` even with both entries filled;
    /// only the decoder's `ADD_MV_REF_LIST_EB` increments. This field is the
    /// honest count of written entries and is informational — the predictors
    /// are always [`nearest`](Self::nearest) and [`near`](Self::near),
    /// **never** `list[count - 1]`.
    pub count: usize,
    /// Inter-mode context: `counter_to_context[context_counter]`
    /// (`vp9_mvref_common.c:126`), the row selector for
    /// [`DEFAULT_INTER_MODE_PROBS`](super::tables_inter::DEFAULT_INTER_MODE_PROBS).
    pub mode_context: u8,
}

impl MvRefResult {
    /// `mv_list[0]` — the `nearestmv` predictor, valid regardless of
    /// [`count`](Self::count).
    #[must_use]
    pub fn nearest(&self) -> MotionVector {
        self.list[0]
    }

    /// `mv_list[1]` — the `nearmv` predictor, valid regardless of
    /// [`count`](Self::count) (an unfilled entry is the clamped zero fill,
    /// which is exactly what libvpx's `NEARMV` path reads when the scan found
    /// fewer than two candidates).
    #[must_use]
    pub fn near(&self) -> MotionVector {
        self.list[1]
    }
}

/// libvpx `ADD_MV_REF_LIST` (`vp9_mvref_common.h:248-258`).
///
/// Returns `true` when the macro's `goto Done` fires, i.e. the list is full
/// and the caller must stop scanning.
fn add_mv_ref_list(
    mv: MotionVector,
    list: &mut [MotionVector; MAX_MV_REF_CANDIDATES],
    count: &mut usize,
) -> bool {
    if *count > 0 {
        if mv != list[0] {
            list[1] = mv;
            *count = 2;
            return true;
        }
        false
    } else {
        list[0] = mv;
        *count = 1;
        false
    }
}

/// libvpx `get_sub_block_mv` (`vp9_mvref_common.h:224-231`):
///
/// ```c
/// return block_idx >= 0 && candidate->sb_type < BLOCK_8X8
///            ? candidate->bmi[idx_n_column_to_subblock[block_idx][search_col == 0]].as_mv[which_mv]
///            : candidate->mv[which_mv];
/// ```
///
/// Both conditions matter: the *candidate* must be sub-8x8 **and** the
/// *current* block must be a sub-8x8 sub-block (`block_idx >= 0`). A >=8x8
/// block scanning a sub-8x8 neighbour takes that neighbour's whole-block
/// vector.
fn get_sub_block_mv(
    candidate: &MiInfo,
    which_mv: usize,
    search_col: i8,
    block_idx: i32,
) -> MotionVector {
    if block_idx >= 0 && candidate.sb_type < BLOCK_8X8 {
        let column = usize::from(search_col == 0);
        if let Some(row) = IDX_N_COLUMN_TO_SUBBLOCK.get(block_idx as usize) {
            return candidate.bmv[row[column]][which_mv];
        }
        debug_assert!(false, "VP9 sub-8x8 block index out of range: {block_idx}");
    }
    candidate.mv[which_mv]
}

/// libvpx `scale_mv` (`vp9_mvref_common.h:234-243`): negates the candidate's
/// vector when its reference points the other way in time than the reference
/// being predicted.
fn scale_mv(
    mi: &MiInfo,
    ref_idx: usize,
    this_ref_frame: i8,
    sign_bias: &[bool; 4],
) -> MotionVector {
    flip_if_opposite_bias(
        mi.mv[ref_idx],
        mi.ref_frame[ref_idx],
        this_ref_frame,
        sign_bias,
    )
}

/// The `ref_sign_bias[a] != ref_sign_bias[b]` negation shared by `scale_mv`
/// and the two temporal blocks (`vp9_mvref_common.c:103-107`, `:115-119`).
///
/// libvpx indexes `ref_sign_bias` with a raw `MV_REFERENCE_FRAME`; every call
/// site guards the index to `LAST/GOLDEN/ALTREF` first (`is_inter_block`,
/// `has_second_ref`, or an explicit `> INTRA_FRAME`), so an out-of-range index
/// is unreachable and reads as `false` here rather than panicking.
fn flip_if_opposite_bias(
    mv: MotionVector,
    cand_ref: i8,
    this_ref_frame: i8,
    sign_bias: &[bool; 4],
) -> MotionVector {
    if bias_of(sign_bias, cand_ref) == bias_of(sign_bias, this_ref_frame) {
        mv
    } else {
        // `*= -1` in libvpx; MV components are bounded well inside `i16`, so
        // the wrapping form only differs from negation for an input this
        // decoder cannot produce.
        MotionVector::new(mv.row.wrapping_neg(), mv.col.wrapping_neg())
    }
}

/// `ref_sign_bias[frame]`, total for any `MV_REFERENCE_FRAME` value.
fn bias_of(sign_bias: &[bool; 4], frame: i8) -> bool {
    usize::try_from(frame)
        .ok()
        .and_then(|i| sign_bias.get(i))
        .copied()
        .unwrap_or(false)
}

/// `mode_2_counter[mode]` (`vp9_mvref_common.h:47-62`), indexed by the raw
/// `MB_MODE_COUNT` mode number [`MiInfo::mode`] carries.
fn mode_counter(mode: u8) -> usize {
    debug_assert!(
        (mode as usize) < MODE_2_COUNTER.len(),
        "VP9 prediction mode out of range: {mode}"
    );
    MODE_2_COUNTER
        .get(mode as usize)
        .map_or(0, |&counter| counter as usize)
}

/// `counter_to_context[context_counter]` (`vp9_mvref_common.h:67-87`).
///
/// The counter is at most `2 * 9` = 18 (two neighbours, intra being the
/// heaviest vote), so the lookup is always in range; the fallback returns the
/// table's own [`COUNTER_INVALID_CASE`] sentinel rather than inventing a
/// plausible context.
fn counter_to_context(counter: usize) -> u8 {
    COUNTER_TO_CONTEXT
        .get(counter)
        .copied()
        .unwrap_or(COUNTER_INVALID_CASE)
}

/// The decoded neighbour at `pos`, or `None` when the position is outside the
/// frame/tile (libvpx `is_inside` + `xd->mi[...]`).
///
/// The second bounds test is defensive: `is_inside` already guarantees it for
/// any [`TileBounds`] consistent with the grid (`mi_col_end <= mi_cols`),
/// which the decode path always builds. A caller that violates that gets a
/// missing neighbour rather than a panic.
fn candidate_at<G: MiGrid + ?Sized>(
    grid: &G,
    tile: &TileBounds,
    mi_row: usize,
    mi_col: usize,
    pos: [i8; 2],
) -> Option<MiInfo> {
    if !is_inside(tile, mi_col, mi_row, grid.mi_rows(), pos) {
        return None;
    }
    let row = usize::try_from(mi_row as i32 + i32::from(pos[0])).ok()?;
    let col = usize::try_from(mi_col as i32 + i32::from(pos[1])).ok()?;
    if row < grid.mi_rows() && col < grid.mi_cols() {
        Some(grid.mi_at(row, col))
    } else {
        None
    }
}

/// The previous frame's MV record for this MI position — libvpx
/// `cm->prev_frame->mvs + mi_row * cm->mi_cols + mi_col`
/// (`vp9_mvref_common.c:25-28`), fetched only when the caller passes the
/// array, i.e. when `cm->use_prev_frame_mvs` holds
/// ([`Vp9DecState::use_prev_frame_mvs`](super::state::Vp9DecState::use_prev_frame_mvs)).
///
/// `use_prev_frame_mvs` requires the previous frame to have the same
/// dimensions, so a short array is unreachable from the decode path and yields
/// no temporal candidate instead of a panic.
fn prev_mv_record(
    prev_mvs: Option<&[MvRefRow]>,
    mi_cols: usize,
    mi_row: usize,
    mi_col: usize,
) -> Option<MvRefRow> {
    prev_mvs.and_then(|mvs| mvs.get(mi_row * mi_cols + mi_col).copied())
}

/// Builds a block's motion-vector reference list — libvpx `find_mv_refs_idx`
/// (`vp9_mvref_common.c:16-131`), reached through `vp9_find_mv_refs`
/// (`:133-139`) with `block = -1`.
///
/// * `prev_mvs` — the previous frame's `MV_REF` array, `Some` exactly when
///   libvpx's `cm->use_prev_frame_mvs` holds. Indexed with the *current*
///   frame's `mi_cols` (they are equal whenever the flag is set).
/// * `sign_bias` — `cm->ref_frame_sign_bias`, indexed by reference frame
///   (entry 0, `INTRA_FRAME`, is unused).
/// * `block` — the sub-8x8 sub-block index (0..=3), or `-1` to take
///   whole-block vectors from the two nearest neighbours. Only those two
///   consult it. `-1` is right for every block of 8x8 or larger **and** for
///   the sub-8x8 `NEWMV` reference lookup, which libvpx also calls with
///   `block = -1` (`vp9_decodemv.c:748-749`); the sub-block index is used
///   only by [`append_sub8x8_mvs_for_idx`], i.e. for `NEARESTMV` / `NEARMV`.
///
/// The scan is five passes, in this order (any of them may finish the list and
/// stop the whole search):
///
/// 1. Neighbours 0..2, exact reference match, sub-block vectors, accumulating
///    the mode-context counter (`:37-53`).
/// 2. Neighbours 2..8, exact reference match, whole-block vectors (`:58-70`).
/// 3. The temporal record, exact reference match (`:73-79`).
/// 4. All 8 neighbours again, *different* reference, sign-bias flipped —
///    gated on having seen at least one neighbour (`:84-96`).
/// 5. The temporal record, different reference, sign-bias flipped — **not**
///    gated on `different_ref_found` (`:99-122`).
///
/// Both list entries are then clamped (`:128-130`).
pub fn find_mv_refs<G: MiGrid + ?Sized>(
    mi_grid: &G,
    tile: &TileBounds,
    prev_mvs: Option<&[MvRefRow]>,
    sign_bias: &[bool; 4],
    ref_frame: i8,
    mi_row: usize,
    mi_col: usize,
    bsize: u8,
    block: i32,
) -> MvRefResult {
    // "Blank the reference vector list" (`:31-32`).
    let mut list = [MotionVector::zero(); MAX_MV_REF_CANDIDATES];
    let mut count = 0usize;
    let mut context_counter = 0usize;
    let mut different_ref_found = false;

    let search = &MV_REF_BLOCKS[bsize_index(bsize)];
    let prev = prev_mv_record(prev_mvs, mi_grid.mi_cols(), mi_row, mi_col);

    'done: {
        // Pass 1 — "The nearest 2 blocks are treated differently … we get the
        // mv from the bmi substructure, and we also need to keep a mode
        // count." (`:34-53`)
        for &pos in search.iter().take(2) {
            let Some(candidate) = candidate_at(mi_grid, tile, mi_row, mi_col, pos) else {
                continue;
            };
            // Counted before any early-out, so both neighbours always vote.
            context_counter += mode_counter(candidate.mode);
            different_ref_found = true;

            if candidate.ref_frame[0] == ref_frame {
                if add_mv_ref_list(
                    get_sub_block_mv(&candidate, 0, pos[1], block),
                    &mut list,
                    &mut count,
                ) {
                    break 'done;
                }
            } else if candidate.ref_frame[1] == ref_frame
                && add_mv_ref_list(
                    get_sub_block_mv(&candidate, 1, pos[1], block),
                    &mut list,
                    &mut count,
                )
            {
                break 'done;
            }
        }

        // Pass 2 — the remaining neighbours, whole-block vectors, no mode
        // counting (`:55-70`).
        for &pos in search.iter().skip(2) {
            let Some(candidate) = candidate_at(mi_grid, tile, mi_row, mi_col, pos) else {
                continue;
            };
            different_ref_found = true;

            if candidate.ref_frame[0] == ref_frame {
                if add_mv_ref_list(candidate.mv[0], &mut list, &mut count) {
                    break 'done;
                }
            } else if candidate.ref_frame[1] == ref_frame
                && add_mv_ref_list(candidate.mv[1], &mut list, &mut count)
            {
                break 'done;
            }
        }

        // Pass 3 — "Check the last frame's mode and mv info." (`:72-79`)
        if let Some(prev) = prev {
            if prev.ref_frame[0] == ref_frame {
                if add_mv_ref_list(prev.mv[0], &mut list, &mut count) {
                    break 'done;
                }
            } else if prev.ref_frame[1] == ref_frame
                && add_mv_ref_list(prev.mv[1], &mut list, &mut count)
            {
                break 'done;
            }
        }

        // Pass 4 — "Since we couldn't find 2 mvs from the same reference frame
        // go back through the neighbors and find motion vectors from different
        // reference frames." (`:81-96`), i.e. `IF_DIFF_REF_FRAME_ADD_MV`
        // (`vp9_mvref_common.h:262-274`). Note the two adds are independent
        // `if`s here, not the `if`/`else if` of passes 1 and 2.
        if different_ref_found {
            for &pos in search.iter() {
                let Some(candidate) = candidate_at(mi_grid, tile, mi_row, mi_col, pos) else {
                    continue;
                };
                // "If the candidate is INTRA we don't want to consider its mv."
                if !is_inter_block(&candidate) {
                    continue;
                }
                if candidate.ref_frame[0] != ref_frame
                    && add_mv_ref_list(
                        scale_mv(&candidate, 0, ref_frame, sign_bias),
                        &mut list,
                        &mut count,
                    )
                {
                    break 'done;
                }
                if has_second_ref(&candidate)
                    && candidate.ref_frame[1] != ref_frame
                    && candidate.mv[1] != candidate.mv[0]
                    && add_mv_ref_list(
                        scale_mv(&candidate, 1, ref_frame, sign_bias),
                        &mut list,
                        &mut count,
                    )
                {
                    break 'done;
                }
            }
        }

        // Pass 5 — "Since we still don't have a candidate we'll try the last
        // frame." (`:98-122`). Unlike pass 4 this is *not* gated on
        // `different_ref_found`.
        if let Some(prev) = prev {
            if prev.ref_frame[0] != ref_frame && prev.ref_frame[0] > INTRA_FRAME {
                let mv = flip_if_opposite_bias(prev.mv[0], prev.ref_frame[0], ref_frame, sign_bias);
                if add_mv_ref_list(mv, &mut list, &mut count) {
                    break 'done;
                }
            }
            if prev.ref_frame[1] > INTRA_FRAME
                && prev.ref_frame[1] != ref_frame
                && prev.mv[1] != prev.mv[0]
            {
                let mv = flip_if_opposite_bias(prev.mv[1], prev.ref_frame[1], ref_frame, sign_bias);
                if add_mv_ref_list(mv, &mut list, &mut count) {
                    break 'done;
                }
            }
        }
    }

    // `Done:` — context, then "Clamp vectors" over *all*
    // `MAX_MV_REF_CANDIDATES` entries, not just the filled ones (`:124-130`).
    let mode_context = counter_to_context(context_counter);
    let edges = BlockEdges::new(mi_row, mi_col, bsize, mi_grid.mi_rows(), mi_grid.mi_cols());
    for mv in &mut list {
        *mv = edges.clamp_mv_ref(*mv);
    }

    MvRefResult {
        list,
        count,
        mode_context,
    }
}

/// The inter-mode context on its own — libvpx's decoder-side
/// `get_mode_context` (`vp9/decoder/vp9_decodemv.c:670-689`):
///
/// ```c
/// for (i = 0; i < 2; ++i) {
///   const POSITION *const mv_ref = &mv_ref_search[i];
///   if (is_inside(tile, mi_col, mi_row, cm->mi_rows, mv_ref)) {
///     const MODE_INFO *const candidate = xd->mi[mv_ref->col + mv_ref->row * xd->mi_stride];
///     context_counter += mode_2_counter[candidate->mode];
///   }
/// }
/// return counter_to_context[context_counter];
/// ```
///
/// The decoder needs this before it knows a reference frame (and for
/// `SEG_LVL_SKIP` blocks, which never run the candidate scan), so it is a
/// separate entry point rather than a by-product of [`find_mv_refs`]. The two
/// always agree — see the module docs and
/// `get_mode_context_agrees_with_find_mv_refs`.
#[must_use]
pub fn get_mode_context<G: MiGrid + ?Sized>(
    mi_grid: &G,
    tile: &TileBounds,
    mi_row: usize,
    mi_col: usize,
    bsize: u8,
) -> u8 {
    let search = &MV_REF_BLOCKS[bsize_index(bsize)];
    let mut context_counter = 0usize;
    for &pos in search.iter().take(2) {
        if let Some(candidate) = candidate_at(mi_grid, tile, mi_row, mi_col, pos) {
            context_counter += mode_counter(candidate.mode);
        }
    }
    counter_to_context(context_counter)
}

// ---------------------------------------------------------------------------
// Precision
// ---------------------------------------------------------------------------

/// libvpx `use_mv_hp` (`vp9/common/vp9_entropymv.h:30-33`):
///
/// ```c
/// const int kMvRefThresh = 64;  // threshold for use of high-precision 1/8 mv
/// return abs(ref->row) < kMvRefThresh && abs(ref->col) < kMvRefThresh;
/// ```
///
/// Older libvpx spelled the same predicate `(abs(ref->row) >> 3) <
/// COMPANDED_MVREF_THRESH` with `COMPANDED_MVREF_THRESH = 8`, which is
/// identical: `floor(|v| / 8) < 8` iff `|v| < 64`.
///
/// The magnitude test runs in `i32` because `i16::abs` panics in debug builds
/// on `i16::MIN` — unreachable for a real motion vector, trivial to hit from a
/// synthetic one.
#[must_use]
pub fn use_mv_hp(mv: MotionVector) -> bool {
    i32::from(mv.row).abs() < 64 && i32::from(mv.col).abs() < 64
}

/// libvpx `lower_mv_precision` (`vp9_mvref_common.h:294-300`):
///
/// ```c
/// const int use_hp = allow_hp && use_mv_hp(mv);
/// if (!use_hp) {
///   if (mv->row & 1) mv->row += (mv->row > 0 ? -1 : 1);
///   if (mv->col & 1) mv->col += (mv->col > 0 ? -1 : 1);
/// }
/// ```
///
/// Odd components are rounded **toward zero** (`-3` becomes `-2`, `3` becomes
/// `2`), and the whole adjustment is skipped when the frame allows
/// high-precision MVs *and* this vector is small enough to use them
/// ([`use_mv_hp`]).
#[must_use]
pub fn lower_mv_precision(mv: MotionVector, allow_hp: bool) -> MotionVector {
    if allow_hp && use_mv_hp(mv) {
        return mv;
    }
    MotionVector::new(round_toward_zero(mv.row), round_toward_zero(mv.col))
}

/// `if (v & 1) v += (v > 0 ? -1 : 1)` — one component of
/// [`lower_mv_precision`].
fn round_toward_zero(value: i16) -> i16 {
    if value & 1 == 0 {
        value
    } else if value > 0 {
        value - 1
    } else {
        value + 1
    }
}

/// libvpx `vp9_find_best_ref_mvs` (`vp9_mvref_common.c:141-151`): lowers both
/// candidates to the frame's MV precision and returns them as
/// `(nearest_mv, near_mv)`.
///
/// # Why no `clamp_mv2`
///
/// libvpx also runs [`clamp_mv2`](BlockEdges::clamp_mv2) here, which this
/// signature has no edges for — because on a list from [`find_mv_refs`] it
/// provably cannot change anything:
///
/// * [`find_mv_refs`] has already clamped both entries into
///   `[edge - MV_BORDER, edge + MV_BORDER]` (margin 128),
/// * [`lower_mv_precision`] moves a component by at most 1, so every component
///   is within margin 129 of an edge,
/// * `clamp_mv2`'s margin is `LEFT_TOP_MARGIN` = 1248, a strictly wider band
///   around the same edges.
///
/// The decoder agrees: `vp9_decodemv.c:751,779` applies `lower_mv_precision`
/// to the selected candidate and no clamp at all. `clamp_mv2_is_a_no_op_after_find_mv_refs`
/// checks the claim empirically over a frame-edge grid.
#[must_use]
pub fn find_best_ref_mvs(
    list: &[MotionVector; MAX_MV_REF_CANDIDATES],
    allow_hp: bool,
) -> (MotionVector, MotionVector) {
    (
        lower_mv_precision(list[0], allow_hp),
        lower_mv_precision(list[1], allow_hp),
    )
}

// ---------------------------------------------------------------------------
// Sub-8x8
// ---------------------------------------------------------------------------

/// The two predictors for one sub-8x8 sub-block, plus the mode context the
/// same call derives — libvpx's `nearest_mv`, `near_mv` and `mode_context`
/// out-parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sub8x8Mvs {
    /// `nearest_mv`.
    pub nearest: MotionVector,
    /// `near_mv`.
    pub near: MotionVector,
    /// `mode_context[ref_frame]`, as [`find_mv_refs`] derives it.
    pub mode_context: u8,
}

/// Predictors for sub-block `block` of a sub-8x8 block — libvpx
/// `vp9_append_sub8x8_mvs_for_idx` (`vp9_mvref_common.c:153-199`).
///
/// `cur_mi` is the block **being decoded** (libvpx `xd->mi[0]`), which is not
/// in `mi_grid` yet: the function reads its `ref_frame[ref_idx]`, its
/// `sb_type`, and the sub-block vectors `bmv[..][ref_idx]` decoded so far
/// (libvpx `mi->bmi[n].as_mv[ref]`). Sub-blocks are decoded in raster order,
/// so entry `n < block` is already final when `block` is processed.
///
/// The selection per sub-block index (`:168-197`):
///
/// | `block` | `nearest`  | `near` (first candidate that differs from `nearest`) |
/// |---------|------------|------------------------------------------------------|
/// | 0       | `list[0]`  | `list[1]` (taken directly, not by the search)         |
/// | 1, 2    | `bmv[0]`   | `list[0]`, `list[1]`                                  |
/// | 3       | `bmv[2]`   | `bmv[1]`, `bmv[0]`, `list[0]`, `list[1]`              |
///
/// `near` falls back to zero when every candidate equals `nearest`
/// (`near_mv->as_int = 0` at `:167`).
///
/// # Equivalence with the decoder's copy
///
/// libvpx's decoder has its own `append_sub8x8_mvs_for_idx`
/// (`vp9_decodemv.c:609-668`) that returns a *single* vector: the one the
/// sub-block's already-decoded `b_mode` needs. It returns this function's
/// `nearest` for `NEARESTMV` and its `near` for `NEARMV`, candidate for
/// candidate and in the same order — including the `bmi[1]`-then-`bmi[0]`
/// probe order of sub-block 3 — so a caller picks the field matching `b_mode`.
pub fn append_sub8x8_mvs_for_idx<G: MiGrid + ?Sized>(
    mi_grid: &G,
    tile: &TileBounds,
    prev_mvs: Option<&[MvRefRow]>,
    sign_bias: &[bool; 4],
    cur_mi: &MiInfo,
    ref_idx: usize,
    block: usize,
    mi_row: usize,
    mi_col: usize,
) -> Sub8x8Mvs {
    debug_assert!(block < 4, "VP9 sub-8x8 block index out of range: {block}");
    debug_assert!(ref_idx < 2, "VP9 reference index out of range: {ref_idx}");

    let ref_frame = cur_mi.ref_frame[ref_idx.min(1)];
    let refs = find_mv_refs(
        mi_grid,
        tile,
        prev_mvs,
        sign_bias,
        ref_frame,
        mi_row,
        mi_col,
        cur_mi.sb_type,
        block as i32,
    );
    let bmv = |n: usize| cur_mi.bmv[n][ref_idx.min(1)];

    // `near_mv->as_int = 0;` (`:167`) — the fallback when every candidate
    // equals `nearest`.
    let mut near = MotionVector::zero();
    let nearest = match block {
        0 => {
            near = refs.list[1];
            refs.list[0]
        }
        1 | 2 => {
            let nearest = bmv(0);
            for candidate in refs.list {
                if nearest != candidate {
                    near = candidate;
                    break;
                }
            }
            nearest
        }
        _ => {
            let nearest = bmv(2);
            for candidate in [bmv(1), bmv(0), refs.list[0], refs.list[1]] {
                if nearest != candidate {
                    near = candidate;
                    break;
                }
            }
            nearest
        }
    };

    Sub8x8Mvs {
        nearest,
        near,
        mode_context: refs.mode_context,
    }
}

#[cfg(test)]
mod tests;
