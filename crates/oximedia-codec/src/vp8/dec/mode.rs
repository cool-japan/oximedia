//! VP8 **inter-frame macroblock prediction records** — RFC 6386 §16.
//!
//! Decodes the per-macroblock *prediction record* stream that lives in an inter frame's first
//! (header) partition, immediately after the frame header: for every macroblock, in raster order,
//! its segment id (§9.3/§10), its `mb_skip_coeff` flag (§9.11), whether it is intra- or
//! inter-predicted (`Bool(prob_intra)`, §16), and then either the intra prediction modes (§16.1)
//! or the reference frame, motion-vector reference mode and motion vector(s) (§16.2-§16.4).
//!
//! The output is one [`InterMbInfo`] per macroblock: the reference frame, the mode, all sixteen
//! 4x4 sub-block motion vectors (filled even for whole-macroblock modes, so motion compensation
//! and the neighbour search never special-case `SPLITMV`), and the `has_y2` flag gating the
//! second-order (Y2) coefficient block.
//!
//! # Provenance
//!
//! Every structural decision below is transcribed from, and cited against, RFC 6386's normative
//! text and the two reference implementations it embeds: the §16.3 `vp8_find_near_mvs()` /
//! `vp8_clamp_mv()` listing and the complete `dixie.c` decoder of §20.11 (`modemv.c`). Cited line
//! numbers are lines of `rfc6386.txt`, as elsewhere in this `dec/` tree. Where the two listings
//! differ in *form* (§16.3 clamps inside `find_near_mvs`; dixie clamps at each use site in
//! `decode_mvs`) the comment says so and proves them equivalent.
//!
//! # THE INTEGRATION SEAM — how this module is wired in
//!
//! **1. Motion-vector reading is injected, not imported.** Decoding a `NEWMV` / `NEW4X4` vector
//! needs the §17.2 component decoder, which is `super::mv`, written **concurrently** with this
//! module; rather than depend on a module that might not exist yet, every entry point takes a
//! caller-supplied reader (the [`MvReadFn`] alias). [`super::inter`] injects `mv::read_mv` there;
//! `test_production_mv_reader_matches_the_reference_transcription` proves that injection agrees,
//! record for record, with this file's independent dixie transcription on real bitstream. The
//! contract that reader **must** satisfy:
//!
//! - **Order**: the pair is `(row, col)`, vertical first — RFC 6386 §17 line 6031 ("a vertical
//!   component (row) followed by a horizontal component (column)"); dixie `read_mv` (lines
//!   10176-10181) reads `mvc[0]` into `d.y` (row), `mvc[1]` into `d.x` (column). dixie's `union
//!   mv` is *column* first in memory (`struct { int16_t x, y; }`, line 8634) while this module
//!   uses `(row, col)` (libvpx's `MV { short row, col; }`), so every transcribed dixie line has
//!   its components swapped; that swap is spelled out **once**, in [`MvClampRect::clamp`].
//! - **Units**: **eighth-pel**, the coded quarter-pel magnitude *doubled* (dixie
//!   `read_mv_component` returns `x << 1`, line 10093). The `<< 7` (16-pixel) clamp bounds in
//!   [`MvClampRect::for_mb`] are only correct in those units.
//! - **Errors**: any `Err` is propagated verbatim.
//!
//! `mv::MotionVector` is `{ row: i16, col: i16 }` in eighth-pels and `mv::read_mv` returns
//! `CodecResult<MotionVector>` after scaling each component by `* 2`, so it **satisfies that
//! contract**. It is a *struct*, not a tuple, so the injection is a one-line adapter rather than
//! the function itself:
//!
//! ```ignore
//! let mut read_mv = |bd: &mut BoolDecoder<'_>, probs: &[[u8; 19]; 2]| {
//!     mv::read_mv(bd, probs).map(|m| (m.row, m.col))
//! };
//! ```
//!
//! The `cfg(test)` reference transcription at the bottom of this file is deliberately **kept**:
//! with the production reader wired, it is no longer a stand-in but an independent oracle, and
//! the agreement test above is worth more than either alone.
//!
//! **2. Clamping is decided here, computed in `mv`.** Near-MV clamping is *part of* §16.3's
//! `find_near_mvs`/`decode_mvs` contract — it decides the stored, neighbour-visible value of a
//! `NEARESTMV`/`NEARMV` macroblock — so *where* it applies is this module's business, expressed
//! as [`MvClampRect`]. The arithmetic itself is `mv::clamp_mv(mv, mb_to_left, mb_to_right,
//! mb_to_top, mb_to_bottom)`, which was derived independently from the same two citations and
//! agreed; [`MvClampRect::for_mb`] produces exactly its four arguments, in that order, and
//! [`MvClampRect::clamp`] calls it, so the two cannot drift apart.
//!
//! **3. Out of scope.** This module never touches pixels, coefficients, reference-frame buffers,
//! the loop filter or any token partition. It reads only the first-partition [`BoolDecoder`] the
//! caller ([`super::inter`]) hands it — the one [`Vp8Header::parse_interframe`] returns — and its
//! only mutation of [`Vp8State`] is the per-macroblock segment-map write mandated by §9.3.

#![forbid(unsafe_code)]

use super::bool_decoder::BoolDecoder;
use super::header::Vp8Header;
use super::state::{SignBias, Vp8State};
use super::tables::{BMODE_TREE, B_PRED, DC_PRED};
use super::tables_inter::{
    sub_mv_ref_context, ABOVE4X4, BMODE_PROB, LEFT4X4, MBSPLITS, MBSPLIT_PROBS, MBSPLIT_TREE,
    MODE_CONTEXTS, MV_REF_TREE, NEARESTMV, NEARMV, NEW4X4, NEWMV, NUM_MBSPLIT_PARTS, SPLITMV,
    SUB_MV_REF_PROBS2, SUB_MV_REF_TREE, UV_MODE_TREE, YMODE_TREE, ZERO4X4, ZEROMV,
};
use crate::error::{CodecError, CodecResult};

// --- Motion-vector representation ----------------------------------------

/// A motion vector as `(row, col)` in **eighth-pel** units. See the module doc's seam section for
/// both conventions and their citations.
pub(super) type Mv = (i16, i16);

/// The zero motion vector. dixie tests this as `mv.raw == 0` on the 32-bit union (lines 10258,
/// 10268); tuple equality is the same test.
const ZERO_MV: Mv = (0, 0);

/// The injected §17.2 motion-vector reader (package P3's `mv::read_mv`): returns `(row, col)` in
/// eighth-pel units, reading two components from the supplied `[[u8; 19]; 2]` table (`[0]` = row,
/// `[1]` = column). Full contract in the module doc's seam section.
pub(super) type MvReadFn<'f> =
    &'f mut dyn FnMut(&mut BoolDecoder<'_>, &[[u8; 19]; 2]) -> CodecResult<(i16, i16)>;

/// The eighth-pel rectangle a motion vector is clamped into (dixie `struct mv_clamp_rect`, line
/// 9838).
///
/// Fields are `i32`, not `i16`, exactly as in both reference decoders (dixie's `int to_left,
/// to_right, ...`; libvpx's `int mb_to_left_edge`): for a wide frame `(mb_cols - mb_col) << 7`
/// exceeds `i16::MAX`, in which case the bound is simply never reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MvClampRect {
    /// Minimum column (horizontal) component.
    pub to_left: i32,
    /// Maximum column (horizontal) component.
    pub to_right: i32,
    /// Minimum row (vertical) component.
    pub to_top: i32,
    /// Maximum row (vertical) component.
    pub to_bottom: i32,
}

impl MvClampRect {
    /// The clamp rectangle for macroblock `(mb_col, mb_row)` of an `mb_cols` x `mb_rows` frame: a
    /// one-macroblock border around it, in eighth-pel units (16 pixels == `16 << 3` == 128).
    ///
    /// Two independent citations, algebraically the *same* formula. RFC 6386 §16.3 `vp8_clamp_mv`
    /// (lines 5609-5620) clamps to `mb_to_left_edge - LEFT_TOP_MARGIN` .. `mb_to_right_edge +
    /// RIGHT_BOTTOM_MARGIN`, libvpx's margins being `16 << 3` with `mb_to_left_edge = -((mb_col *
    /// 16) << 3)`. dixie `vp8_dixie_modemv_process_row` (lines 10598-10601) sets `to_left = -((col
    /// + 1) << 7)`, `to_right = (mb_cols - col) << 7`, `to_top = -((row + 1) << 7)`, `to_bottom =
    /// (mb_rows - row) << 7`, decrementing the horizontal pair by `16 << 3` per column (lines
    /// 10639-10640). Identity: `-(mb_col << 7) - 128 == -((mb_col + 1) << 7)` and `((mb_cols - 1 -
    /// mb_col) << 7) + 128 == (mb_cols - mb_col) << 7`.
    pub(super) fn for_mb(mb_col: usize, mb_row: usize, mb_cols: usize, mb_rows: usize) -> Self {
        let col = mb_col as i32;
        let row = mb_row as i32;
        Self {
            to_left: -((col + 1) << 7),
            to_right: (mb_cols as i32 - col) << 7,
            to_top: -((row + 1) << 7),
            to_bottom: (mb_rows as i32 - row) << 7,
        }
    }

    /// Clamps `mv` into this rectangle (RFC 6386 §16.3 `vp8_clamp_mv`, lines 5609-5620; dixie
    /// `clamp_mv`, lines 9863-9877).
    ///
    /// **This is the one and only place** the `(row, col)` tuple order of [`Mv`] is mapped onto
    /// dixie's `(x = col, y = row)` field names: the row component clamps against
    /// `to_top`/`to_bottom`, the column component against `to_left`/`to_right`.
    ///
    /// The clamp arithmetic itself is [`super::mv::clamp_mv`] — P3's independently-derived
    /// transcription of the same `vp8_clamp_mv`, which takes the four bounds in exactly the order
    /// [`MvClampRect::for_mb`] produces them. The two were written concurrently from the same two
    /// citations and agreed; keeping one implementation means they cannot drift apart.
    pub(super) fn clamp(&self, mv: Mv) -> Mv {
        let clamped = super::mv::clamp_mv(
            super::mv::MotionVector {
                row: mv.0,
                col: mv.1,
            },
            self.to_left,
            self.to_right,
            self.to_top,
            self.to_bottom,
        );
        (clamped.row, clamped.col)
    }
}

// --- Reference frame / mode enumerations ---------------------------------

/// The prediction reference of one macroblock (dixie `enum reference_frame`, lines 8589-8595).
///
/// The discriminants are dixie's and are *load-bearing* twice: loop-filter reference deltas (RFC
/// 6386 §9.4/§15.2), which dixie indexes as `ref_delta[mbi->base.ref_frame]` (line 9193) — slot 0
/// intra, 1 last, 2 golden, 3 altref, the layout of
/// [`super::header::LoopFilterHeader::ref_deltas`]; and sign bias (§9.7), indexed by the same enum
/// (line 10205) with only golden and altref ever non-zero (lines 7804-7805), which is why
/// [`SignBias`] stores only those two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefFrame {
    /// Intra-predicted: no reference frame (dixie `CURRENT_FRAME`, 0).
    Intra = 0,
    /// The previous reconstructed frame (dixie `LAST_FRAME`, 1).
    Last = 1,
    /// The golden frame (dixie `GOLDEN_FRAME`, 2).
    Golden = 2,
    /// The altref frame (dixie `ALTREF_FRAME`, 3).
    AltRef = 3,
}

impl RefFrame {
    /// Index into the loop-filter *reference* delta array (RFC 6386 §9.4; dixie line 9193:
    /// `ref_delta[mbi->base.ref_frame]`).
    pub(super) const fn lf_ref_delta_slot(self) -> usize {
        self as usize
    }

    /// This reference's sign-bias flag (RFC 6386 §9.7; dixie lines 7804-7805: only golden and
    /// altref are ever coded).
    const fn sign_bias(self, sign_bias: &SignBias) -> bool {
        match self {
            Self::Intra | Self::Last => false,
            Self::Golden => sign_bias.golden,
            Self::AltRef => sign_bias.altref,
        }
    }
}

/// The motion-vector reference mode of an inter macroblock (RFC 6386 §16.2 `mv_ref`, lines
/// 5500-5514).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InterMode {
    /// Use the "nearest" surviving neighbour vector.
    Nearest,
    /// Use the "near" (second-nearest) surviving neighbour vector.
    Near,
    /// Use the zero vector.
    Zero,
    /// Explicitly coded offset from `best_mv`.
    New,
    /// Split into partitions, each with its own sub-mode and vector.
    Split,
}

impl InterMode {
    /// Maps a decoded [`MV_REF_TREE`] leaf onto this enum. The leaves are the
    /// `NEARESTMV..SPLITMV` constants, which continue the intra `ymode` enumeration (RFC 6386
    /// §16.2 lines 5498-5506: `mv_nearest = num_ymodes`); anything else means tree and decoder
    /// have drifted apart — reported honestly rather than panicking.
    fn from_tree_leaf(leaf: i32) -> CodecResult<Self> {
        let leaf = leaf as usize;
        if leaf == NEARESTMV {
            Ok(Self::Nearest)
        } else if leaf == NEARMV {
            Ok(Self::Near)
        } else if leaf == ZEROMV {
            Ok(Self::Zero)
        } else if leaf == NEWMV {
            Ok(Self::New)
        } else if leaf == SPLITMV {
            Ok(Self::Split)
        } else {
            Err(CodecError::InvalidBitstream(format!(
                "VP8: mv_ref tree produced out-of-range mode {leaf}"
            )))
        }
    }

    /// Index into the loop-filter *mode* delta array for an inter macroblock (RFC 6386 §9.4;
    /// dixie lines 9199-9206: slot 0 is intra `B_PRED`, 1 `ZEROMV`, 3 `SPLITMV`, 2 everything
    /// else).
    pub(super) const fn lf_mode_delta_slot(self) -> usize {
        match self {
            Self::Zero => 1,
            Self::Split => 3,
            _ => 2,
        }
    }
}

/// The `SPLITMV` partition specification of one macroblock (RFC 6386 §16.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SplitInfo {
    /// `mbsplit` id: [`super::tables_inter::MBSPLIT_16X8`] ..
    /// [`super::tables_inter::MBSPLIT_4X4`] (dixie `base.partitioning`, line 10381).
    pub partitioning: u8,
    /// Number of partitions this shape has ([`NUM_MBSPLIT_PARTS`]`[partitioning]`).
    pub num_parts: u8,
    /// The decoded sub-mode ([`LEFT4X4`]..[`NEW4X4`]) of each of the sixteen 4x4 sub-blocks: a
    /// partition's sub-mode is replicated into every sub-block it owns, so this array is always
    /// fully populated and is directly comparable against [`MBSPLITS`]`[partitioning]`.
    pub sub_modes: [u8; 16],
}

/// The prediction mode of one macroblock: intra (§16.1) or inter (§16.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MbMode {
    /// Intra-predicted, exactly as in a key frame but with the *frame-level* trees and
    /// probabilities (RFC 6386 §16.1).
    Intra {
        /// Whole-macroblock luma mode, `DC_PRED..B_PRED`.
        ymode: u8,
        /// The sixteen 4x4 luma sub-block modes; `Some` iff `ymode` is `B_PRED`.
        bmodes: Option<[u8; 16]>,
        /// Chroma mode, `DC_PRED..TM_PRED`.
        uv_mode: u8,
    },
    /// Inter-predicted (RFC 6386 §16.2-§16.4).
    Inter {
        /// The motion-vector reference mode.
        mode: InterMode,
        /// The macroblock-level motion vector. For `SPLITMV` this is the **last** sub-block's
        /// vector (dixie line 10525: `this->base.mv = this->split.mvs[15]`) — the value a later
        /// macroblock's neighbour search sees, so the rule is bitstream-visible, not cosmetic.
        mv: Mv,
        /// `Some` iff `mode` is [`InterMode::Split`].
        sub: Option<SplitInfo>,
    },
}

/// One macroblock's complete inter-frame prediction record (dixie `struct mb_info`, lines
/// 8653-8676).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InterMbInfo {
    /// Segment id `0..=3` (RFC 6386 §9.3/§10), meaningful only when the frame enables
    /// segmentation; inherited from [`Vp8State::segment_map`] when the frame does not update it.
    pub segment_id: u8,
    /// `mb_skip_coeff` (RFC 6386 §9.11): this macroblock codes no coefficient tokens at all.
    pub skip_coeff: bool,
    /// `true` iff `ref_frame != RefFrame::Intra`; its own field because it is the raw
    /// `Bool(prob_intra)` datum of §16.
    pub is_inter: bool,
    /// The prediction reference.
    pub ref_frame: RefFrame,
    /// The prediction mode.
    pub mode: MbMode,
    /// The vector of each of the sixteen 4x4 luma sub-blocks, raster order, filled for **every**
    /// macroblock: zero for intra, the macroblock vector replicated for the whole-block inter
    /// modes, the per-partition vectors spread through [`MBSPLITS`] for `SPLITMV`.
    pub sub_mvs: [Mv; 16],
    /// Whether this macroblock codes a second-order (Y2/WHT) block — every mode except intra
    /// `B_PRED` and inter `SPLITMV` (RFC 6386 §13.1; dixie line 12833 `if (mbi->base.y_mode !=
    /// SPLITMV) // && != BPRED`).
    pub has_y2: bool,
}

impl InterMbInfo {
    /// The record VP8 uses for the one-macroblock border left of column 0 and above row 0 (RFC
    /// 6386 §16.3 lines 5567-5571, "VP8 provides a border of 1 macroblock filled with 0,0 motion
    /// vectors"). dixie `calloc`s its `mb_info` storage with an extra border row and column and
    /// never writes to them (lines 10668-10684), so a border macroblock reads as `ref_frame ==
    /// CURRENT_FRAME` (intra), `y_mode == DC_PRED` (so: not `SPLITMV`) and `mv.raw == 0`.
    pub(super) const fn border() -> Self {
        Self {
            segment_id: 0,
            skip_coeff: false,
            is_inter: false,
            ref_frame: RefFrame::Intra,
            mode: MbMode::Intra {
                ymode: DC_PRED as u8,
                bmodes: None,
                uv_mode: DC_PRED as u8,
            },
            sub_mvs: [ZERO_MV; 16],
            has_y2: true,
        }
    }

    /// The macroblock-level motion vector (dixie `base.mv`): the zero vector for an intra
    /// macroblock (dixie line 10049: `this->base.mv.raw = 0`).
    pub(super) const fn mv(&self) -> Mv {
        match self.mode {
            MbMode::Intra { .. } => ZERO_MV,
            MbMode::Inter { mv, .. } => mv,
        }
    }

    /// Whether this macroblock used `SPLITMV` (dixie's `y_mode == SPLITMV` test, used by the
    /// neighbour search's split census at line 10332 and by `above_block_mv`/`left_block_mv` at
    /// lines 10101 and 10114).
    pub(super) const fn is_split(&self) -> bool {
        matches!(
            self.mode,
            MbMode::Inter {
                mode: InterMode::Split,
                ..
            }
        )
    }
}

// --- Neighbour context threaded by the caller's macroblock loop ----------

/// The above / left / above-left neighbour records the §16.3 search needs, threaded through the
/// caller's macroblock loop.
///
/// dixie keeps one `mb_info` array for the whole frame plus a border row/column and indexes
/// backwards (`this - 1`, `above`, `above - 1`); this type keeps only what the search reads, so
/// the caller (P6's macroblock loop) may store its own per-frame records however it likes:
///
/// ```ignore
/// let mut ctx = ModeRowCtx::new(mb_cols);
/// for mb_row in 0..mb_rows {
///     ctx.start_row();
///     for mb_col in 0..mb_cols {
///         let info = read_mb_mode_record(bd, header, state, &ctx, pos, read_mv)?;
///         ctx.advance(mb_col, &info);
///     }
/// }
/// ```
pub(super) struct ModeRowCtx {
    /// Records of the row above, one per column; border records before the first row.
    above: Vec<InterMbInfo>,
    /// Record of the macroblock to the left in the current row.
    left: InterMbInfo,
    /// Record of the above-left macroblock, i.e. the *previous* row's entry at the previous
    /// column, saved before [`ModeRowCtx::advance`] overwrote it.
    above_left: InterMbInfo,
    /// The border record, handed out for out-of-range columns.
    border: InterMbInfo,
}

impl ModeRowCtx {
    /// Creates the context for a frame `mb_cols` macroblocks wide, with the whole "row above"
    /// initialised to the §16.3 border record.
    pub(super) fn new(mb_cols: usize) -> Self {
        Self {
            above: vec![InterMbInfo::border(); mb_cols],
            left: InterMbInfo::border(),
            above_left: InterMbInfo::border(),
            border: InterMbInfo::border(),
        }
    }

    /// Resets the rolling left / above-left entries to the border record. Call once before each
    /// macroblock row (dixie's left border column).
    pub(super) fn start_row(&mut self) {
        self.left = self.border;
        self.above_left = self.border;
    }

    /// Records the just-decoded macroblock at `mb_col` and rolls the neighbour window on. The
    /// above-left entry for column `c + 1` is the *previous* row's entry at column `c`, which
    /// this call is about to overwrite — so it is saved here, in the same step.
    pub(super) fn advance(&mut self, mb_col: usize, info: &InterMbInfo) {
        if let Some(slot) = self.above.get_mut(mb_col) {
            self.above_left = core::mem::replace(slot, *info);
        }
        self.left = *info;
    }

    /// The record of the macroblock above column `mb_col`.
    pub(super) fn above(&self, mb_col: usize) -> &InterMbInfo {
        self.above.get(mb_col).unwrap_or(&self.border)
    }

    /// The record of the macroblock to the left.
    pub(super) const fn left(&self) -> &InterMbInfo {
        &self.left
    }

    /// The record of the above-left macroblock.
    pub(super) const fn above_left(&self) -> &InterMbInfo {
        &self.above_left
    }
}

/// Where in the frame a macroblock sits — bundled so the per-macroblock entry point keeps a
/// readable signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MbPosition {
    /// Macroblock column.
    pub mb_col: usize,
    /// Macroblock row.
    pub mb_row: usize,
    /// Frame width in macroblocks.
    pub mb_cols: usize,
    /// Frame height in macroblocks.
    pub mb_rows: usize,
}

// --- §16.3 neighbour survey ----------------------------------------------

/// `cnt` slot for the zero vector's weight — which, after the final step of [`find_near_mvs`],
/// doubles as the slot holding `best_mv` (dixie `CNT_BEST = 0, CNT_ZEROZERO = 0`, lines
/// 10215-10216).
const CNT_ZERO: usize = 0;
/// `cnt` slot for the "nearest" vector's weight.
const CNT_NEAREST: usize = 1;
/// `cnt` slot for the "near" vector's weight.
const CNT_NEAR: usize = 2;
/// `cnt` slot for the `SPLITMV` census.
const CNT_SPLITMV: usize = 3;

/// Applies §16.3's sign-bias inversion to a neighbour's motion vector (RFC 6386 §16.3 `mv_bias`,
/// lines 5580-5593; dixie lines 10199-10210): when the neighbour's reference frame and this
/// macroblock's disagree about sign bias, the neighbour's vector points "the other way in time"
/// and is negated before it can be compared or reused.
///
/// `saturating_neg` rather than `-`: real components are bounded by ±1023 quarter-pel (§17.1 line
/// 6041), but the value can reach here from an injected reader and a panic is never the right
/// answer to bad input.
fn mv_bias(neighbour_ref: RefFrame, this_ref: RefFrame, mv: Mv, sign_bias: &SignBias) -> Mv {
    if neighbour_ref.sign_bias(sign_bias) == this_ref.sign_bias(sign_bias) {
        mv
    } else {
        (mv.0.saturating_neg(), mv.1.saturating_neg())
    }
}

/// The §16.3 neighbour survey: RFC 6386 `vp8_find_near_mvs` (lines 5638-5765) / dixie
/// `find_near_mvs` (lines 10223-10355), transcribed statement for statement.
///
/// Returns `(near_mvs, cnt)` where `near_mvs[CNT_ZERO]` is `best_mv`, `[CNT_NEAREST]` is `nearest`
/// and `[CNT_NEAR]` is `near`, all **unclamped** — dixie clamps at each use site in `decode_mvs`
/// (lines 10498, 10501, 10507, 10516) rather than inside this function like the §16.3 listing does
/// (lines 5759-5761). Equivalent, because the census, the dedupe comparisons and the near/nearest
/// swap all run on unclamped values in both listings, so clamping after them cannot change `cnt`.
///
/// Two subtleties any restructuring would silently lose, so the index cursors stay explicit
/// integers exactly like the C pointers they stand for. `mv_idx` is C's `mv` pointer: the dedupe
/// test compares a candidate only against the **most recently entered** vector, not the whole list
/// (above = A, left = B, above-left = A therefore enters A twice, at slots 1 and 3). And
/// `cnt[CNT_SPLITMV]` does double duty (lines 5729-5738 / 10325-10335): before the merge check a
/// non-zero value means "three distinct vectors were entered" — slot 3 is only ever reached by the
/// above-left `+= 1` — and it is then **overwritten** by the `SPLITMV` census.
fn find_near_mvs(
    this_ref: RefFrame,
    left: &InterMbInfo,
    above: &InterMbInfo,
    above_left: &InterMbInfo,
    sign_bias: &SignBias,
) -> ([Mv; 4], [usize; 4]) {
    let mut near_mvs = [ZERO_MV; 4];
    let mut cnt = [0usize; 4];
    // C's `int_mv *mv = near_mvs;` and `int *cntx = cnt;`.
    let mut mv_idx = 0usize;
    let mut cnt_idx = 0usize;

    // --- Process above (lines 5654-5661 / 10255-10265) ---
    // The `+= 2` sits *outside* the inner `if` here (unlike left and above-left, which spell out
    // an explicit `else cnt[CNT_ZERO] += ...`): when the above vector is zero neither cursor has
    // moved yet, so `cnt[cnt_idx]` is still `cnt[CNT_ZERO]` and the two forms agree.
    if above.ref_frame != RefFrame::Intra {
        if above.mv() != ZERO_MV {
            mv_idx += 1;
            near_mvs[mv_idx] = mv_bias(above.ref_frame, this_ref, above.mv(), sign_bias);
            cnt_idx += 1;
        }
        cnt[cnt_idx] += 2;
    }

    // --- Process left (lines 5663-5678 / 10267-10285) ---
    if left.ref_frame != RefFrame::Intra {
        if left.mv() != ZERO_MV {
            let this_mv = mv_bias(left.ref_frame, this_ref, left.mv(), sign_bias);
            if this_mv != near_mvs[mv_idx] {
                mv_idx += 1;
                near_mvs[mv_idx] = this_mv;
                cnt_idx += 1;
            }
            cnt[cnt_idx] += 2;
        } else {
            cnt[CNT_ZERO] += 2;
        }
    }

    // --- Process above-left (lines 5680-5700 / 10287-10322) ---
    if above_left.ref_frame != RefFrame::Intra {
        if above_left.mv() != ZERO_MV {
            let this_mv = mv_bias(above_left.ref_frame, this_ref, above_left.mv(), sign_bias);
            if this_mv != near_mvs[mv_idx] {
                mv_idx += 1;
                near_mvs[mv_idx] = this_mv;
                cnt_idx += 1;
            }
            cnt[cnt_idx] += 1;
        } else {
            cnt[CNT_ZERO] += 1;
        }
    }

    // --- "If we have three distinct MVs ..." (lines 5729-5733 / 10325-10330) ---
    if cnt[CNT_SPLITMV] != 0 && near_mvs[mv_idx] == near_mvs[CNT_NEAREST] {
        cnt[CNT_NEAREST] += 1;
    }

    // --- SPLITMV census, overwriting the slot just tested (lines 5735-5738 / 10332-10335) ---
    cnt[CNT_SPLITMV] = (usize::from(above.is_split()) + usize::from(left.is_split())) * 2
        + usize::from(above_left.is_split());

    // --- Swap near and nearest if necessary (lines 5740-5749 / 10337-10346) ---
    if cnt[CNT_NEAR] > cnt[CNT_NEAREST] {
        cnt.swap(CNT_NEAREST, CNT_NEAR);
        near_mvs.swap(CNT_NEAREST, CNT_NEAR);
    }

    // --- Use near_mvs[0] to store "best" (lines 5751-5752 / 10348-10352) ---
    if cnt[CNT_NEAREST] >= cnt[CNT_ZERO] {
        near_mvs[CNT_ZERO] = near_mvs[CNT_NEAREST];
    }

    (near_mvs, cnt)
}

/// Derives the four [`MV_REF_TREE`] node probabilities from the §16.3 census (RFC 6386
/// `vp8_mv_ref_probs`, lines 5794-5803; dixie lines 10481-10484). Each internal node is looked up
/// *independently*: `p[i] = mode_contexts[cnt[i]][i]`, never one shared context row.
///
/// The `min` makes a proof obligation explicit rather than repairing anything: RFC 6386 line 5633
/// states "The largest possible weight value in each case is 5" and every `cnt` slot is bounded by
/// 5 by construction (2 + 2 + 1 weights; `(1 + 1) * 2 + 1` split census; the "+1" merge only fires
/// on a slot holding 2), while [`MODE_CONTEXTS`] has six rows — so it is a no-op when reachable.
fn mv_ref_probs(cnt: &[usize; 4]) -> [u8; 4] {
    let last_row = MODE_CONTEXTS.len() - 1;
    [
        MODE_CONTEXTS[cnt[0].min(last_row)][0],
        MODE_CONTEXTS[cnt[1].min(last_row)][1],
        MODE_CONTEXTS[cnt[2].min(last_row)][2],
        MODE_CONTEXTS[cnt[3].min(last_row)][3],
    ]
}

// --- §16.4 split prediction ----------------------------------------------

/// The motion vector of the sub-block above sub-block `b` (dixie `above_block_mv`, lines
/// 10095-10107): for the top row (`b < 4`) the macroblock above — its bottom row of sub-block
/// vectors if split, else its macroblock vector (zero when intra or a border macroblock);
/// otherwise this macroblock's own sub-block four slots earlier, which §16.4 (lines 6006-6009)
/// guarantees is already filled.
fn above_block_mv(this_sub: &[Mv; 16], above: &InterMbInfo, b: usize) -> Mv {
    if b < 4 {
        if above.is_split() {
            above.sub_mvs[b + 12]
        } else {
            above.mv()
        }
    } else {
        this_sub[b - 4]
    }
}

/// The motion vector of the sub-block left of sub-block `b` (dixie `left_block_mv`, lines
/// 10109-10121). Mirror image of [`above_block_mv`]: the left column (`b & 3 == 0`) looks at the
/// macroblock to the left, taking its rightmost sub-block column (`b + 3`) when that macroblock
/// was split.
fn left_block_mv(this_sub: &[Mv; 16], left: &InterMbInfo, b: usize) -> Mv {
    if b & 3 == 0 {
        if left.is_split() {
            left.sub_mvs[b + 3]
        } else {
            left.mv()
        }
    } else {
        this_sub[b - 1]
    }
}

/// Decodes a `SPLITMV` macroblock's partition specification and per-partition vectors (RFC 6386
/// §16.4; dixie `decode_split_mv`, lines 10365-10425), filling `sub_mvs`/`sub_modes` and
/// returning the `mbsplit` id.
///
/// `clamped_best` is `clamp_mv(near_mvs[BEST], bounds)`: dixie clamps before this call (line
/// 10515) and `NEW4X4` adds the **clamped** base (lines 10404-10407).
///
/// dixie's outer loop is `for (j = 0, mask = 0; mask < 65535; j++)` with an unbounded inner scan
/// `for (k = 0; j != partition[k]; k++);`. That terminates precisely because [`MBSPLITS`]`[id]`
/// covers all sixteen sub-blocks with the contiguous ids `0..num_parts` — an invariant
/// `tables_inter`'s own `test_mbsplits_partition_ids_contiguous_and_balanced` pins — so iterating
/// `j` over `0..num_parts` and looking the first sub-block up with `position()` is the same loop
/// with the termination argument made explicit.
fn decode_split_mv(
    bd: &mut BoolDecoder<'_>,
    sub_mvs: &mut [Mv; 16],
    sub_modes: &mut [u8; 16],
    neighbours: (&InterMbInfo, &InterMbInfo),
    mv_probs: &[[u8; 19]; 2],
    clamped_best: Mv,
    read_mv: MvReadFn<'_>,
) -> CodecResult<u8> {
    let (left, above) = neighbours;

    let partition_id = bd.read_tree(&MBSPLIT_TREE, &MBSPLIT_PROBS) as usize;
    let map = MBSPLITS.get(partition_id).ok_or_else(|| {
        CodecError::InvalidBitstream(format!(
            "VP8: mbsplit tree produced out-of-range partition id {partition_id}"
        ))
    })?;
    let num_parts = NUM_MBSPLIT_PARTS[partition_id];

    for j in 0..num_parts {
        // "Find the first subblock in this partition" (line 10386).
        let k = map.iter().position(|&p| p == j).ok_or_else(|| {
            CodecError::Internal(format!(
                "VP8: mbsplit map {partition_id} has no sub-block for partition {j}"
            ))
        })?;

        // The sub-mode context operands are the left/above neighbours of the partition's *first*
        // sub-block `k` — not of every sub-block it owns (lines 10387-10389).
        let left_mv = left_block_mv(sub_mvs, left, k);
        let above_mv = above_block_mv(sub_mvs, above, k);
        // dixie `submv_ref`, lines 10145-10171: lez/aez/lea over the raw 32-bit vectors, mapped
        // onto the eight-row `prob3` table by `tables_inter::sub_mv_ref_context`.
        let ctx = sub_mv_ref_context(left_mv == ZERO_MV, above_mv == ZERO_MV, left_mv == above_mv);
        let sub_mode = bd.read_tree(&SUB_MV_REF_TREE, &SUB_MV_REF_PROBS2[ctx]) as usize;

        let mv = if sub_mode == LEFT4X4 {
            left_mv
        } else if sub_mode == ABOVE4X4 {
            above_mv
        } else if sub_mode == ZERO4X4 {
            ZERO_MV
        } else if sub_mode == NEW4X4 {
            // "added to the best vector returned by the earlier call to find_near_mvs" (RFC 6386
            // §16.4 lines 6015-6018); dixie lines 10404-10407 add the *clamped* best. Wrapping to
            // stay panic-free on an adversarial injected reader.
            let delta = read_mv(bd, mv_probs)?;
            (
                delta.0.wrapping_add(clamped_best.0),
                delta.1.wrapping_add(clamped_best.1),
            )
        } else {
            return Err(CodecError::InvalidBitstream(format!(
                "VP8: sub_mv_ref tree produced out-of-range sub-mode {sub_mode}"
            )));
        };

        // "Fill the MVs for this partition" (lines 10416-10422).
        for (slot, &p) in map.iter().enumerate() {
            if p == j {
                sub_mvs[slot] = mv;
                sub_modes[slot] = sub_mode as u8;
            }
        }
    }

    Ok(partition_id as u8)
}

// --- Per-macroblock record decode ----------------------------------------

/// Reads the per-macroblock segment id (RFC 6386 §9.3/§10; dixie `read_segment_id`, lines
/// 9880-9887) — the same two-level tree the key-frame path in `super::decode_macroblock` uses.
fn read_segment_id(bd: &mut BoolDecoder<'_>, tree_probs: &[u8; 3]) -> u8 {
    if bd.get_bool(tree_probs[0]) {
        2 + u8::from(bd.get_bool(tree_probs[2]))
    } else {
        u8::from(bd.get_bool(tree_probs[1]))
    }
}

/// Decodes an intra-predicted macroblock inside an **inter** frame (RFC 6386 §16.1, lines
/// 5395-5450; dixie `decode_intra_mb_mode`, lines 9997-10051).
///
/// Three bitstream-visible differences from the key-frame path in `super::decode_macroblock`: the
/// luma mode uses [`YMODE_TREE`] (`DC_PRED` first) with the *frame-level, adaptive*
/// [`Vp8State`]`::entropy.ymode_prob`, **not** `KF_YMODE_TREE`/`KF_YMODE_PROB` (§16.1 lines
/// 5403-5426); `B_PRED` sub-modes use the single context-**free** [`BMODE_PROB`] table, not the
/// above/left-conditioned `KF_BMODE_PROB` (§16.1 lines 5429-5440, "in place of the contexts used
/// in key frames, these encodings use the single fixed probability table"; dixie line 10000, "no
/// context on the above/left block mode"), so an inter frame needs **no** above/left sub-mode
/// context arrays at all — the key-frame decoder's `above_bmode`/`left_bmode` tracking has no
/// counterpart here, though the record still carries the sixteen sub-modes because reconstruction
/// needs them; and the chroma mode uses the adaptive `uv_mode_prob`, not `KF_UV_MODE_PROB`. dixie
/// additionally sets `base.mv = 0` and `ref_frame = CURRENT_FRAME` (lines 10049-10050), which
/// [`InterMbInfo::mv`] and [`RefFrame::Intra`] encode.
fn read_intra_mb_mode(
    bd: &mut BoolDecoder<'_>,
    ymode_prob: &[u8; 4],
    uv_mode_prob: &[u8; 3],
) -> MbMode {
    let ymode = bd.read_tree(&YMODE_TREE, ymode_prob) as usize;
    let bmodes = if ymode == B_PRED {
        let mut modes = [0u8; 16];
        for m in &mut modes {
            *m = bd.read_tree(&BMODE_TREE, &BMODE_PROB) as u8;
        }
        Some(modes)
    } else {
        None
    };
    let uv_mode = bd.read_tree(&UV_MODE_TREE, uv_mode_prob) as u8;
    MbMode::Intra {
        ymode: ymode as u8,
        bmodes,
        uv_mode,
    }
}

/// Decodes one macroblock's complete prediction record from the first partition (RFC 6386 §16;
/// dixie `vp8_dixie_modemv_process_row`'s loop body, lines 10613-10650, plus `decode_mvs`, lines
/// 10440-10577).
///
/// Field order, exactly as dixie reads it: segment id (only when the frame updates the map) →
/// `mb_skip_coeff` (only when the frame enables skipping) → `Bool(prob_intra)` → the intra or
/// inter branch. `state` is read for the adaptive mode/MV probabilities and the segment map, and
/// written for the segment map only (RFC 6386 §9.3).
///
/// # Errors
/// Propagates any error from the injected `read_mv`; an impossible tree result is reported as
/// [`CodecError::InvalidBitstream`].
pub(super) fn read_mb_mode_record(
    bd: &mut BoolDecoder<'_>,
    header: &Vp8Header,
    state: &mut Vp8State,
    ctx: &ModeRowCtx,
    pos: MbPosition,
    read_mv: MvReadFn<'_>,
) -> CodecResult<InterMbInfo> {
    let mb_idx = pos.mb_row * pos.mb_cols + pos.mb_col;

    // --- segment id (RFC 6386 §9.3/§10; dixie lines 10615-10617) ---
    // dixie reads it iff
    // `segment_hdr.update_map`, otherwise leaving `mb_info.base.segment_id` holding what the
    // previous frame left there — which is exactly `Vp8State::segment_map`. `update_map` is only
    // ever true when segmentation is enabled (see `parse_segmentation_inter`), so no separate
    // `enabled` test is needed here; consumers of `segment_id` must still gate on
    // `header.segment.enabled` themselves.
    let segment_id = if header.segment.update_map {
        let id = read_segment_id(bd, &header.segment.tree_probs);
        if let Some(slot) = state.segment_map.get_mut(mb_idx) {
            *slot = id;
        }
        id
    } else {
        state.segment_map.get(mb_idx).copied().unwrap_or(0)
    };

    // --- mb_skip_coeff (RFC 6386 §9.11; dixie lines 10619-10621) ---
    let skip_coeff = header.mb_no_skip_coeff && bd.get_bool(header.prob_skip_false);

    // --- intra vs inter (RFC 6386 §16 lines 5388-5393; dixie line 10630) ---
    let is_inter = bd.get_bool(header.inter.prob_intra);

    if !is_inter {
        let mode = read_intra_mb_mode(bd, &state.entropy.ymode_prob, &state.entropy.uv_mode_prob);
        let has_y2 = !matches!(mode, MbMode::Intra { ymode, .. } if ymode as usize == B_PRED);
        return Ok(InterMbInfo {
            segment_id,
            skip_coeff,
            is_inter: false,
            ref_frame: RefFrame::Intra,
            mode,
            sub_mvs: [ZERO_MV; 16],
            has_y2,
        });
    }

    // --- reference frame (RFC 6386 §16.2 lines 5468-5474; dixie lines 10467-10469) ---
    // Polarity, verbatim: "If 0, the reference frame is the previous frame (the last frame); if 1,
    // another bool (prob_gf) selects the reference frame between the golden frame (0) and the
    // altref frame (1)" — dixie: `ref_frame = bool_get(prob_last) ? 2 + bool_get(prob_gf) : 1`,
    // with `LAST_FRAME == 1`, `GOLDEN_FRAME == 2`, `ALTREF_FRAME == 3`.
    let ref_frame = if bd.get_bool(header.inter.prob_last) {
        if bd.get_bool(header.inter.prob_gf) {
            RefFrame::AltRef
        } else {
            RefFrame::Golden
        }
    } else {
        RefFrame::Last
    };

    // --- neighbour survey and mv_ref decode (RFC 6386 §16.3; dixie lines 10479-10486) ---
    let (near_mvs, cnt) = find_near_mvs(
        ref_frame,
        ctx.left(),
        ctx.above(pos.mb_col),
        ctx.above_left(),
        &state.sign_bias,
    );
    let probs = mv_ref_probs(&cnt);
    let mode = InterMode::from_tree_leaf(bd.read_tree(&MV_REF_TREE, &probs))?;

    let bounds = MvClampRect::for_mb(pos.mb_col, pos.mb_row, pos.mb_cols, pos.mb_rows);
    let mv_probs = state.entropy.mv_probs;

    // dixie lines 10493-10530. Note which values are stored clamped: NEARESTMV/NEARMV store the
    // clamped candidate (so the clamp propagates into later neighbour surveys), ZEROMV stores the
    // zero vector, and NEWMV stores `clamped_best + delta` **unclamped** — dixie applies no clamp
    // to the sum (lines 10507-10512), and libvpx defers it to motion compensation via its
    // `need_to_clamp_mvs` flag rather than altering the stored vector.
    let mut sub_mvs = [ZERO_MV; 16];
    let mut sub = None;
    let mv = match mode {
        InterMode::Nearest => bounds.clamp(near_mvs[CNT_NEAREST]),
        InterMode::Near => bounds.clamp(near_mvs[CNT_NEAR]),
        InterMode::Zero => ZERO_MV,
        InterMode::New => {
            let clamped_best = bounds.clamp(near_mvs[CNT_ZERO]);
            let delta = read_mv(bd, &mv_probs)?;
            (
                delta.0.wrapping_add(clamped_best.0),
                delta.1.wrapping_add(clamped_best.1),
            )
        }
        InterMode::Split => {
            let clamped_best = bounds.clamp(near_mvs[CNT_ZERO]);
            let mut sub_modes = [0u8; 16];
            let partitioning = decode_split_mv(
                bd,
                &mut sub_mvs,
                &mut sub_modes,
                (ctx.left(), ctx.above(pos.mb_col)),
                &mv_probs,
                clamped_best,
                read_mv,
            )?;
            sub = Some(SplitInfo {
                partitioning,
                num_parts: NUM_MBSPLIT_PARTS[partitioning as usize] as u8,
                sub_modes,
            });
            // "this->base.mv = this->split.mvs[15]" (line 10525): the macroblock-level vector of
            // a split macroblock is its **last** (bottom-right) sub-block's. Bitstream-visible:
            // it is what a later macroblock's `find_near_mvs` reads.
            sub_mvs[15]
        }
    };

    if mode != InterMode::Split {
        // Whole-macroblock modes apply one vector to all sixteen sub-blocks (RFC 6386 §16.2 lines
        // 5823-5826).
        sub_mvs = [mv; 16];
    }

    Ok(InterMbInfo {
        segment_id,
        skip_coeff,
        is_inter: true,
        ref_frame,
        mode: MbMode::Inter { mode, mv, sub },
        sub_mvs,
        // RFC 6386 §13.1 / dixie line 12833: every mode except intra B_PRED and inter SPLITMV
        // codes a Y2 block.
        has_y2: mode != InterMode::Split,
    })
}

/// Decodes **every** macroblock prediction record of an inter frame, in raster order, threading
/// the §16.3 neighbour context for the caller.
///
/// `bd` must be the first-partition decoder returned by [`Vp8Header::parse_interframe`]. On return
/// it sits at the end of the mode-record stream, which is the end of the first partition — a
/// strong desync check for the caller (see `test_p5basic_frame1_matches_golden_dump`).
///
/// # Errors
/// Propagates any error from [`read_mb_mode_record`].
pub(super) fn read_all_mb_mode_records(
    bd: &mut BoolDecoder<'_>,
    header: &Vp8Header,
    state: &mut Vp8State,
    mb_cols: usize,
    mb_rows: usize,
    read_mv: MvReadFn<'_>,
) -> CodecResult<Vec<InterMbInfo>> {
    let mut ctx = ModeRowCtx::new(mb_cols);
    let mut out = Vec::with_capacity(mb_cols * mb_rows);
    for mb_row in 0..mb_rows {
        ctx.start_row();
        for mb_col in 0..mb_cols {
            let pos = MbPosition {
                mb_col,
                mb_row,
                mb_cols,
                mb_rows,
            };
            let info = read_mb_mode_record(bd, header, state, &ctx, pos, &mut *read_mv)?;
            ctx.advance(mb_col, &info);
            out.push(info);
        }
    }
    Ok(out)
}

#[cfg(test)]
#[path = "mode_tests.rs"]
mod tests;
