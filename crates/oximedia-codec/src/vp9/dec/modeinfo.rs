//! Inter-frame mode info: reference frames, inter modes, interpolation
//! filter and motion vectors.
//!
//! Exact port of the inter half of libvpx `vp9/decoder/vp9_decodemv.c`
//! (v1.15.2 — the local reference copy this was transcribed from is
//! byte-identical to `webmproject/libvpx` tag `v1.15.2`, verified by
//! SHA-256). Every function below names the libvpx function and line range it
//! comes from.
//!
//! [`read_inter_frame_mode_info`] is `read_inter_frame_mode_info`
//! (`vp9_decodemv.c:788-806`) and everything under it: `read_inter_segment_id`,
//! `read_skip`, `read_is_inter_block`, `read_tx_size`,
//! `read_inter_block_mode_info`, `read_intra_block_mode_info`,
//! `read_ref_frames`, `read_block_reference_mode`,
//! `read_switchable_interp_filter`, `read_inter_mode`, `assign_mv`, `read_mv`
//! and `read_mv_component`, plus `vp9_inc_mv` (`vp9_entropymv.c:139-152`) for
//! the backward-adaptation counters.
//!
//! The *intra*-frame mode info (`read_intra_frame_mode_info`,
//! `vp9_decodemv.c:192-233`) stays in [`super::recon`], which already decodes
//! key and intra-only frames bit-exactly. Only the non-key-frame intra *block*
//! path (`read_intra_block_mode_info`, `:354-387`) lives here, because it is
//! reached exclusively from an inter frame.
//!
//! # Scope this pass: segmentation-enabled inter frames are refused
//!
//! [`read_inter_frame_mode_info`] returns an honest
//! [`CodecError::UnsupportedFeature`] when `seg.enabled` is set, because
//! `read_inter_segment_id` (`:146-177`) needs the previous frame's segment map
//! and the current frame's map write-back, neither of which the decode driver
//! maintains yet. The segment-feature branches those maps feed —
//! `SEG_LVL_SKIP` in [`Ctx::read_skip`] and `SEG_LVL_REF_FRAME` in
//! [`Ctx::read_is_inter_block`] / [`Ctx::read_ref_frames`] — *are* transcribed
//! and unit-tested directly, so the later package that adds the maps adds call
//! sites, not algorithms. Key-frame / intra-only segmentation is untouched.
//!
//! # `MiInfo` conventions this module establishes
//!
//! * **`mode` of an inter block is the raw `MB_MODE_COUNT` number**, i.e.
//!   `NEARESTMV` = 10 … `NEWMV` = 13 ([`NEARESTMV`] below), *not* the
//!   `INTER_OFFSET` 0..=3 that [`INTER_MODE_TREE`] carries. libvpx's
//!   `read_inter_mode` (`:50-58`) returns `NEARESTMV + mode` for exactly this
//!   reason: [`MODE_2_COUNTER`](super::tables_inter::MODE_2_COUNTER) (which
//!   the next block's mode context reads through
//!   [`get_mode_context`]) and `MODE_LF_LUT` (which the loop filter reads) are
//!   both indexed by the raw number.
//! * **`interp_filter` of an *intra* block on an inter frame is `3`**
//!   ([`SWITCHABLE_FILTERS`]), not `0`. libvpx `read_intra_block_mode_info`
//!   (`:381-383`) sets it deliberately — "Initialize interp_filter here so we
//!   do not have to check for inter block modes in
//!   `get_pred_context_switchable_interp()`" — and
//!   [`get_pred_context_switchable_interp`] treats that value as the
//!   "no filter here" sentinel. `MiInfo::default()` leaves it `0`
//!   (`EIGHTTAP`), which would silently shift the *next* block's filter
//!   context, so every path here assigns it explicitly.
//! * `is_inter` is kept consistent with `ref_frame[0] > INTRA_FRAME` on every
//!   path; [`is_inter_block`] deliberately derives from `ref_frame[0]` and
//!   never reads the cached flag.
//!
//! # Two deliberate, inert deviations from libvpx
//!
//! 1. **Uninitialised second motion vectors are zero here.** libvpx's
//!    `copy_mv_pair` (`:394-396`) copies *both* entries of an `int_mv[2]` even
//!    for a single-reference block, so a non-compound block's `mv[1]` /
//!    `bmi[].as_mv[1]` ends up holding whatever was there — for sub-8x8, the
//!    explicit `invalid_mv = 0x80008000` sentinel (`:730-733`). This module
//!    leaves those slots zero. Nothing ever reads them: every consumer of a
//!    second motion vector is gated on `has_second_ref` /
//!    `ref_frame[1] > INTRA_FRAME`, which a single-reference block fails
//!    (`get_sub_block_mv`'s `which_mv == 1` is reachable only from
//!    `candidate->ref_frame[1] == ref_frame`, and `ref_frame` is
//!    `LAST`/`GOLDEN`/`ALTREF`).
//! 2. **An out-of-range motion vector is an immediate `Err`.** libvpx's
//!    `assign_mv` (`:402-434`) returns 0 on a failed `is_mv_valid`, which the
//!    caller folds into `xd->corrupted` and reports at the end of the tile.
//!    Both reject the same bitstreams; only the point of report differs, and
//!    no valid stream reaches it.
//!
//! # Why the *common* `find_mv_refs` is reused instead of `dec_find_mv_refs`
//!
//! The decoder has its own candidate scan, `dec_find_mv_refs`
//! (`vp9_decodemv.c:484-607`), which differs from the common
//! `vp9_find_mv_refs` [`super::mvref::find_mv_refs`] ports in two ways: it
//! takes an `early_break` (`mode != NEARMV`) that stops the search at the
//! first candidate, and its `ADD_MV_REF_LIST_EB` increments `refmv_count`
//! when it writes the second entry (the common `ADD_MV_REF_LIST` does not).
//! Neither can change a decoded motion vector:
//!
//! * The **only** values ever consumed are `tmp_mvs[refmv_count - 1]` at
//!   `:779-780` and `:627`. `refmv_count` resolves to `2` for `NEARMV`
//!   (either the `EB` increment on the second write, or the
//!   `refmv_count = MAX_MV_REF_CANDIDATES` fallthrough at `:596-597`) and to
//!   `1` for every other mode (`:600`) — i.e. `list[1]` for `NEARMV` and
//!   `list[0]` otherwise.
//! * `early_break` is off for `NEARMV`, so that path runs the identical full
//!   scan. For the other modes it truncates the search *after* `list[0]` is
//!   final, discarding only entries nobody reads.
//! * The clamp covers both entries whenever `refmv_count == 2`, which is
//!   exactly the `NEARMV` case; the common scan clamps both unconditionally.
//!
//! `sub8x8_nearmv_matches_find_mv_refs_near` pins the claim empirically rather
//! than resting on this argument.

#![forbid(unsafe_code)]

use super::booldec::BoolReader;
use super::counts::{FrameCounts, NmvCounts, INTRA_MODES};
use super::hdr::{FrameProbs, ReferenceMode, TxMode, SWITCHABLE};
use super::mvref::{
    append_sub8x8_mvs_for_idx, find_best_ref_mvs, find_mv_refs, get_mode_context,
    lower_mv_precision, use_mv_hp, MiGrid, TileBounds, BLOCK_8X8,
};
use super::predctx::{
    get_intra_inter_context, get_pred_context_comp_ref_p, get_pred_context_single_ref_p1,
    get_pred_context_single_ref_p2, get_pred_context_switchable_interp, get_reference_mode_context,
    get_skip_context, get_tx_size_context, has_second_ref, is_inter_block, CompRefState,
    SWITCHABLE_FILTERS,
};
use super::recon::MiInfo;
use super::refs::{MvRefRow, ALTREF_FRAME, GOLDEN_FRAME, INTRA_FRAME, LAST_FRAME, NONE_FRAME};
use super::tables;
use super::tables_inter::{
    NmvComponent, NmvContext, CLASS0_SIZE, INTER_MODE_TREE, LITERAL_TO_FILTER, MV_CLASS0_TREE,
    MV_CLASS_TREE, MV_FP_TREE, MV_JOINT_TREE, MV_LOW, MV_UPP, SIZE_GROUP_LOOKUP,
    SWITCHABLE_INTERP_TREE,
};
use crate::error::{CodecError, CodecResult};
use crate::vp9::mv::MotionVector;
use crate::vp9::uncompressed::SegmentationHeader;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// libvpx `NEARESTMV` (`vp9/common/vp9_enums.h:120`).
///
/// The raw `MB_MODE_COUNT` mode numbering, in which the ten intra modes
/// occupy `0..=9` and the four inter modes follow. See the module docs for
/// why [`MiInfo::mode`] stores this rather than the `INTER_OFFSET` the tree
/// carries.
pub const NEARESTMV: u8 = 10;
/// libvpx `NEARMV` (`vp9_enums.h:121`). See [`NEARESTMV`].
pub const NEARMV: u8 = 11;
/// libvpx `ZEROMV` (`vp9_enums.h:122`). See [`NEARESTMV`].
pub const ZEROMV: u8 = 12;
/// libvpx `NEWMV` (`vp9_enums.h:123`). See [`NEARESTMV`].
pub const NEWMV: u8 = 13;

/// libvpx `SEG_LVL_REF_FRAME` (`vp9/common/vp9_seg_common.h:37`): the segment
/// feature that pins a block's reference frame, overriding both the
/// `is_inter` flag and the whole `read_ref_frames` symbol sequence.
pub const SEG_LVL_REF_FRAME: usize = 2;

/// libvpx `SEG_LVL_SKIP` (`vp9_seg_common.h:38`): the segment feature that
/// forces `skip` and `ZEROMV` without coding either.
pub const SEG_LVL_SKIP: usize = 3;

/// libvpx `CLASS0_BITS` (`vp9/common/vp9_entropymv.h:71`).
const CLASS0_BITS: usize = 1;

/// libvpx `segfeature_active` (`vp9_seg_common.h:57-61`):
/// `seg->enabled && (seg->feature_mask[segment_id] & (1 << feature_id))`.
#[must_use]
fn seg_feature_active(seg: &SegmentationHeader, segment_id: u8, feature: usize) -> bool {
    seg.enabled
        && seg
            .feature_enabled
            .get(segment_id as usize)
            .and_then(|f| f.get(feature))
            .copied()
            .unwrap_or(false)
}

/// libvpx `get_segdata` (`vp9_seg_common.h:63-66`):
/// `seg->feature_data[segment_id][feature_id]`.
#[must_use]
fn seg_data(seg: &SegmentationHeader, segment_id: u8, feature: usize) -> i16 {
    seg.feature_data
        .get(segment_id as usize)
        .and_then(|f| f.get(feature))
        .copied()
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Caller-supplied frame state
// ---------------------------------------------------------------------------

/// The frame-level state (libvpx `VP9_COMMON` fields) an inter block's mode
/// info reads, other than the probabilities and counters.
#[derive(Clone, Copy, Debug)]
pub struct InterFrameCfg<'a> {
    /// `cm->tx_mode`, as the compressed header decided it.
    pub tx_mode: TxMode,
    /// `cm->reference_mode`, as the compressed header decided it.
    pub reference_mode: ReferenceMode,
    /// `cm->interp_filter` — the frame-level interpolation filter,
    /// **already mapped** through [`LITERAL_TO_FILTER`], or
    /// [`SWITCHABLE`] (4).
    ///
    /// [`UncompressedHeader::interp_filter`](crate::vp9::uncompressed::UncompressedHeader)
    /// stores the *raw* two-bit literal instead (see the note on
    /// [`SWITCHABLE`]), so a caller building this from a parsed header must
    /// run it through [`frame_interp_filter`] — which is the whole of
    /// libvpx's `read_interp_filter` (`vp9_decodeframe.c:1456-1461`).
    /// Skipping that mapping swaps `EIGHTTAP` and `EIGHTTAP_SMOOTH` on every
    /// block of a non-switchable frame.
    pub interp_filter: u8,
    /// `cm->allow_high_precision_mv`.
    pub allow_high_precision_mv: bool,
    /// `cm->comp_fixed_ref` / `cm->comp_var_ref` / `cm->ref_frame_sign_bias`,
    /// derived once per frame by
    /// [`CompRefState::from_sign_bias`].
    pub comp: CompRefState,
    /// `cm->seg`.
    pub seg: &'a SegmentationHeader,
    /// `xd->tile` — the column range candidate positions are bounded by.
    pub tile: TileBounds,
    /// `cm->prev_frame->mvs`, `Some` exactly when `cm->use_prev_frame_mvs`
    /// holds.
    pub prev_mvs: Option<&'a [MvRefRow]>,
}

/// The two neighbouring mode infos every prediction context is derived from
/// (libvpx `xd->above_mi` / `xd->left_mi`).
///
/// `None` means "not available": above the first row of the frame, or left of
/// the first column *of the tile*.
///
/// No `Debug`: [`MiInfo`] deliberately has none (a derived one dumps the
/// whole mode-info record per neighbour).
#[derive(Clone, Copy, Default)]
pub struct Neighbours<'a> {
    /// `xd->above_mi`.
    pub above: Option<&'a MiInfo>,
    /// `xd->left_mi`.
    pub left: Option<&'a MiInfo>,
}

/// Where and how big the block being decoded is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockPos {
    /// `mi_row`.
    pub mi_row: usize,
    /// `mi_col`.
    pub mi_col: usize,
    /// `mi->sb_type`, a VP9 `BLOCK_SIZE` (`0..=12`).
    pub bsize: u8,
}

/// libvpx `read_interp_filter` (`vp9/decoder/vp9_decodeframe.c:1456-1461`):
///
/// ```c
/// static INTERP_FILTER read_interp_filter(struct vpx_read_bit_buffer *rb) {
///   const INTERP_FILTER literal_to_filter[] = { EIGHTTAP_SMOOTH, EIGHTTAP,
///                                               EIGHTTAP_SHARP, BILINEAR };
///   return vpx_rb_read_bit(rb) ? SWITCHABLE
///                              : literal_to_filter[vpx_rb_read_literal(rb, 2)];
/// }
/// ```
///
/// The bit reads themselves already happened in
/// `uncompressed.rs`'s `parse_interp_filter`, which stores the raw literal;
/// this applies the permutation that function does not. Passing
/// [`SWITCHABLE`] (4) through returns it unchanged, so it is safe to apply to
/// any stored `interp_filter` exactly once.
///
/// A value outside `0..=4` cannot come from a parsed header (`read_bits(2)`
/// plus the switchable flag), and maps to [`SWITCHABLE`] rather than
/// silently naming a filter kernel that was never coded.
#[must_use]
pub fn frame_interp_filter(raw: u8) -> u8 {
    if raw == SWITCHABLE {
        SWITCHABLE
    } else {
        LITERAL_TO_FILTER
            .get(raw as usize)
            .copied()
            .unwrap_or(SWITCHABLE)
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Reads one block's mode info on an **inter** frame — libvpx
/// `read_inter_frame_mode_info` (`vp9_decodemv.c:788-806`):
///
/// ```c
/// mi->segment_id = read_inter_segment_id(cm, xd, mi_row, mi_col, r, x_mis, y_mis);
/// mi->skip = read_skip(cm, xd, mi->segment_id, r);
/// inter_block = read_is_inter_block(cm, xd, mi->segment_id, r);
/// mi->tx_size = read_tx_size(cm, xd, !mi->skip || !inter_block, r);
/// if (inter_block) read_inter_block_mode_info(pbi, xd, mi, mi_row, mi_col, r);
/// else             read_intra_block_mode_info(cm, xd, mi, r);
/// ```
///
/// `grid` is the frame's mode-info grid as decoded *so far*: the candidate
/// scan reads only positions above and left of the current block (every entry
/// of [`MV_REF_BLOCKS`](super::tables_inter::MV_REF_BLOCKS) has a negative
/// row or a negative column), so the block's own cells are never read and
/// need not be written before the call.
///
/// Returns the block's decoded [`MiInfo`]; the caller stores it into the grid
/// and, per `vp9_read_mode_info` (`:835-842`), copies `ref_frame` / `mv` into
/// the frame's `MV_REF` array for every mode-info cell the block covers.
///
/// # Errors
///
/// * [`CodecError::UnsupportedFeature`] when `cfg.seg.enabled` — see the
///   module docs.
/// * [`CodecError::InvalidBitstream`] for a `NEWMV` outside
///   `(MV_LOW, MV_UPP)`, or a `SEG_LVL_SKIP` block below `BLOCK_8X8`
///   (`:709-713`).
pub fn read_inter_frame_mode_info<G: MiGrid + ?Sized>(
    r: &mut BoolReader<'_>,
    grid: &G,
    cfg: &InterFrameCfg<'_>,
    probs: &FrameProbs,
    counts: Option<&mut FrameCounts>,
    nb: Neighbours<'_>,
    pos: BlockPos,
) -> CodecResult<MiInfo> {
    if cfg.seg.enabled {
        return Err(CodecError::UnsupportedFeature(
            "VP9 inter frame with segmentation enabled: read_inter_segment_id \
             needs the previous frame's segment map and the current frame's \
             map write-back, which the decode driver does not maintain yet"
                .to_string(),
        ));
    }

    let mut ctx = Ctx {
        grid,
        cfg,
        probs,
        counts,
        above: nb.above,
        left: nb.left,
        mi_row: pos.mi_row,
        mi_col: pos.mi_col,
    };

    // `read_inter_segment_id` with `!seg->enabled` (`:154`).
    let segment_id = 0u8;

    let mut mi = MiInfo {
        sb_type: pos.bsize,
        segment_id,
        ..MiInfo::default()
    };
    mi.skip = ctx.read_skip(r, segment_id);
    let inter_block = ctx.read_is_inter_block(r, segment_id);
    mi.tx_size = ctx.read_tx_size(r, pos.bsize, !mi.skip || !inter_block);

    if inter_block {
        ctx.read_inter_block_mode_info(r, &mut mi)?;
    } else {
        ctx.read_intra_block_mode_info(r, &mut mi);
    }
    Ok(mi)
}

/// Everything a single block's mode-info read needs, bundled so the ported
/// functions keep libvpx's short parameter lists.
struct Ctx<'a, G: MiGrid + ?Sized> {
    grid: &'a G,
    cfg: &'a InterFrameCfg<'a>,
    probs: &'a FrameProbs,
    counts: Option<&'a mut FrameCounts>,
    above: Option<&'a MiInfo>,
    left: Option<&'a MiInfo>,
    mi_row: usize,
    mi_col: usize,
}

impl<G: MiGrid + ?Sized> Ctx<'_, G> {
    // -----------------------------------------------------------------
    // Flags read before the mode
    // -----------------------------------------------------------------

    /// libvpx `read_skip` (`vp9_decodemv.c:179-190`).
    ///
    /// `SEG_LVL_SKIP` forces the flag without coding a bit, so libvpx counts
    /// nothing on that branch.
    fn read_skip(&mut self, r: &mut BoolReader<'_>, segment_id: u8) -> bool {
        if seg_feature_active(self.cfg.seg, segment_id, SEG_LVL_SKIP) {
            return true;
        }
        let ctx = get_skip_context(self.above, self.left);
        let skip = r.read_bool(self.probs.skip[ctx]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.skip[ctx][usize::from(skip)] += 1;
        }
        skip
    }

    /// libvpx `read_is_inter_block` (`:436-447`).
    ///
    /// A `SEG_LVL_REF_FRAME` segment answers this without a bit: the block is
    /// inter exactly when the pinned reference is not `INTRA_FRAME`.
    fn read_is_inter_block(&mut self, r: &mut BoolReader<'_>, segment_id: u8) -> bool {
        if seg_feature_active(self.cfg.seg, segment_id, SEG_LVL_REF_FRAME) {
            return seg_data(self.cfg.seg, segment_id, SEG_LVL_REF_FRAME) != i16::from(INTRA_FRAME);
        }
        let ctx = get_intra_inter_context(self.above, self.left);
        let is_inter = r.read_bool(self.probs.intra_inter[ctx]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.intra_inter[ctx][usize::from(is_inter)] += 1;
        }
        is_inter
    }

    /// libvpx `read_tx_size` (`:80-89`):
    ///
    /// ```c
    /// if (allow_select && tx_mode == TX_MODE_SELECT && bsize >= BLOCK_8X8)
    ///   return read_selected_tx_size(cm, xd, max_tx_size, r);
    /// else
    ///   return VPXMIN(max_tx_size, tx_mode_to_biggest_tx_size[tx_mode]);
    /// ```
    ///
    /// On an inter frame `allow_select` is `!mi->skip || !inter_block`
    /// (`:800`): a skipped inter block codes no transform size, because it
    /// codes no residual to apply one to. The key / intra-only frame caller
    /// in [`super::recon`] passes a constant `1` and reaches the same code
    /// through the free function [`read_tx_size`].
    fn read_tx_size(&mut self, r: &mut BoolReader<'_>, bsize: u8, allow_select: bool) -> u8 {
        read_tx_size(
            r,
            self.probs,
            self.counts.as_deref_mut(),
            self.above,
            self.left,
            bsize,
            self.cfg.tx_mode,
            allow_select,
        )
    }

    // -----------------------------------------------------------------
    // Intra block on an inter frame
    // -----------------------------------------------------------------

    /// libvpx `read_intra_block_mode_info` (`:354-387`).
    ///
    /// Unlike the key-frame path in [`super::recon`], the Y mode here is
    /// **not** conditioned on the neighbours' modes: it uses
    /// `fc->y_mode_prob[size_group]` with
    /// [`SIZE_GROUP_LOOKUP`], and the sub-8x8 cases pass the literal group
    /// `0` (`:363`, `:367-374`) — numerically the same as
    /// `SIZE_GROUP_LOOKUP[bsize]` for `bsize < BLOCK_8X8`, transcribed as the
    /// literal libvpx writes.
    fn read_intra_block_mode_info(&mut self, r: &mut BoolReader<'_>, mi: &mut MiInfo) {
        match mi.sb_type {
            // BLOCK_4X4
            0 => {
                for i in 0..4 {
                    mi.bmi[i] = self.read_intra_mode_y(r, 0);
                }
                mi.mode = mi.bmi[3];
            }
            // BLOCK_4X8
            1 => {
                let m0 = self.read_intra_mode_y(r, 0);
                mi.bmi[0] = m0;
                mi.bmi[2] = m0;
                let m1 = self.read_intra_mode_y(r, 0);
                mi.bmi[1] = m1;
                mi.bmi[3] = m1;
                mi.mode = m1;
            }
            // BLOCK_8X4
            2 => {
                let m0 = self.read_intra_mode_y(r, 0);
                mi.bmi[0] = m0;
                mi.bmi[1] = m0;
                let m1 = self.read_intra_mode_y(r, 0);
                mi.bmi[2] = m1;
                mi.bmi[3] = m1;
                mi.mode = m1;
            }
            bsize => {
                let group = usize::from(SIZE_GROUP_LOOKUP[bsize as usize]);
                mi.mode = self.read_intra_mode_y(r, group);
            }
        }
        mi.uv_mode = self.read_intra_mode_uv(r, mi.mode);

        // ":381-386" — the filter sentinel and the intra reference pair.
        mi.interp_filter = SWITCHABLE_FILTERS;
        mi.ref_frame = [INTRA_FRAME, NONE_FRAME];
        mi.is_inter = false;
    }

    /// libvpx `read_intra_mode_y` (`:31-38`) — `read_intra_mode` on
    /// `fc->y_mode_prob[size_group]`, counted into `y_mode[size_group][mode]`.
    fn read_intra_mode_y(&mut self, r: &mut BoolReader<'_>, size_group: usize) -> u8 {
        let mode = r.read_tree(&tables::INTRA_MODE_TREE, &self.probs.y_mode[size_group]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.y_mode[size_group][mode as usize] += 1;
        }
        mode
    }

    /// libvpx `read_intra_mode_uv` (`:40-48`) — `read_intra_mode` on
    /// `fc->uv_mode_prob[y_mode]`, counted into `uv_mode[y_mode][uv_mode]`.
    fn read_intra_mode_uv(&mut self, r: &mut BoolReader<'_>, y_mode: u8) -> u8 {
        let mode = r.read_tree(
            &tables::INTRA_MODE_TREE,
            &self.probs.uv_mode[y_mode as usize],
        );
        if let Some(c) = self.counts.as_deref_mut() {
            c.uv_mode[y_mode as usize][mode as usize] += 1;
        }
        mode
    }

    // -----------------------------------------------------------------
    // Reference frames
    // -----------------------------------------------------------------

    /// libvpx `read_block_reference_mode` (`:287-300`), returning
    /// `mode == COMPOUND_REFERENCE` rather than the enum.
    ///
    /// libvpx's `read_ref_frames` then `assert(0)`s on anything that is
    /// neither `SINGLE_REFERENCE` nor `COMPOUND_REFERENCE`; the boolean makes
    /// that third case unrepresentable instead. `REFERENCE_MODE_SELECT` codes
    /// the choice, any other frame-level mode dictates it.
    fn read_block_reference_mode(&mut self, r: &mut BoolReader<'_>) -> bool {
        if self.cfg.reference_mode != ReferenceMode::Select {
            return self.cfg.reference_mode == ReferenceMode::Compound;
        }
        let ctx = get_reference_mode_context(self.above, self.left, self.cfg.comp.comp_fixed_ref);
        let is_compound = r.read_bool(self.probs.comp_inter[ctx]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.comp_inter[ctx][usize::from(is_compound)] += 1;
        }
        is_compound
    }

    /// libvpx `read_ref_frames` (`:302-341`).
    ///
    /// The compound branch's `idx` is `ref_frame_sign_bias[comp_fixed_ref]`
    /// (`:317`), i.e. [`CompRefState::fix_ref_idx`]: which *slot* of the pair
    /// holds the fixed reference. The coded bit picks `comp_var_ref[bit]` for
    /// the other slot.
    fn read_ref_frames(&mut self, r: &mut BoolReader<'_>, segment_id: u8) -> [i8; 2] {
        if seg_feature_active(self.cfg.seg, segment_id, SEG_LVL_REF_FRAME) {
            let pinned = seg_data(self.cfg.seg, segment_id, SEG_LVL_REF_FRAME) as i8;
            return [pinned, NONE_FRAME];
        }

        if self.read_block_reference_mode(r) {
            let idx = self.cfg.comp.fix_ref_idx();
            let ctx = get_pred_context_comp_ref_p(self.above, self.left, &self.cfg.comp);
            let bit = r.read_bool(self.probs.comp_ref[ctx]);
            if let Some(c) = self.counts.as_deref_mut() {
                c.comp_ref[ctx][usize::from(bit)] += 1;
            }
            let mut ref_frame = [NONE_FRAME; 2];
            ref_frame[idx] = self.cfg.comp.comp_fixed_ref;
            ref_frame[1 - idx] = self.cfg.comp.comp_var_ref[usize::from(bit)];
            return ref_frame;
        }

        let ctx0 = get_pred_context_single_ref_p1(self.above, self.left);
        let bit0 = r.read_bool(self.probs.single_ref[ctx0][0]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.single_ref[ctx0][0][usize::from(bit0)] += 1;
        }
        if !bit0 {
            return [LAST_FRAME, NONE_FRAME];
        }
        let ctx1 = get_pred_context_single_ref_p2(self.above, self.left);
        let bit1 = r.read_bool(self.probs.single_ref[ctx1][1]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.single_ref[ctx1][1][usize::from(bit1)] += 1;
        }
        let first = if bit1 { ALTREF_FRAME } else { GOLDEN_FRAME };
        [first, NONE_FRAME]
    }

    // -----------------------------------------------------------------
    // Mode and filter
    // -----------------------------------------------------------------

    /// libvpx `read_inter_mode` (`:50-58`): the tree carries `INTER_OFFSET`
    /// tokens and the counter is indexed by them, but the returned mode is
    /// `NEARESTMV + token` — the raw number [`MiInfo::mode`] stores.
    fn read_inter_mode(&mut self, r: &mut BoolReader<'_>, ctx: usize) -> u8 {
        let token = r.read_tree(&INTER_MODE_TREE, &self.probs.inter_mode[ctx]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.inter_mode[ctx][token as usize] += 1;
        }
        NEARESTMV + token
    }

    /// libvpx `read_switchable_interp_filter` (`:343-352`).
    fn read_switchable_interp_filter(&mut self, r: &mut BoolReader<'_>) -> u8 {
        let ctx = get_pred_context_switchable_interp(self.above, self.left);
        let filter = r.read_tree(&SWITCHABLE_INTERP_TREE, &self.probs.switchable_interp[ctx]);
        if let Some(c) = self.counts.as_deref_mut() {
            c.switchable_interp[ctx][filter as usize] += 1;
        }
        filter
    }

    // -----------------------------------------------------------------
    // Inter block
    // -----------------------------------------------------------------

    /// libvpx `read_inter_block_mode_info` (`:691-786`).
    ///
    /// Bit order, which is the part most easily got wrong: reference frames,
    /// **then** the block mode (only for `bsize >= BLOCK_8X8`, and only when
    /// `SEG_LVL_SKIP` did not already force `ZEROMV`), **then** the
    /// interpolation filter, **then** the sub-8x8 per-sub-block modes and all
    /// motion vectors. The filter sits between the block mode and the
    /// sub-block modes (`:716-721`), matching specification §6.4.14
    /// `inter_block_mode_info`.
    fn read_inter_block_mode_info(
        &mut self,
        r: &mut BoolReader<'_>,
        mi: &mut MiInfo,
    ) -> CodecResult<()> {
        let bsize = mi.sb_type;
        let allow_hp = self.cfg.allow_high_precision_mv;

        mi.ref_frame = self.read_ref_frames(r, mi.segment_id);
        mi.is_inter = is_inter_block(mi);
        let is_compound = has_second_ref(mi);
        let refs_used = 1 + usize::from(is_compound);

        // `:705` — derived once per block and reused for the block mode *and*
        // every sub-8x8 `b_mode`.
        let inter_mode_ctx = usize::from(get_mode_context(
            self.grid,
            &self.cfg.tile,
            self.mi_row,
            self.mi_col,
            bsize,
        ));

        if seg_feature_active(self.cfg.seg, mi.segment_id, SEG_LVL_SKIP) {
            mi.mode = ZEROMV;
            if bsize < BLOCK_8X8 {
                return Err(CodecError::InvalidBitstream(
                    "VP9: invalid usage of segment feature on small blocks \
                     (SEG_LVL_SKIP below BLOCK_8X8)"
                        .to_string(),
                ));
            }
        } else if bsize >= BLOCK_8X8 {
            mi.mode = self.read_inter_mode(r, inter_mode_ctx);
        }

        mi.interp_filter = if self.cfg.interp_filter == SWITCHABLE {
            self.read_switchable_interp_filter(r)
        } else {
            self.cfg.interp_filter
        };

        if bsize < BLOCK_8X8 {
            self.read_sub8x8_mvs(r, mi, inter_mode_ctx, refs_used)?;
        } else {
            let mut best_ref_mvs = [MotionVector::zero(); 2];
            if mi.mode != ZEROMV {
                for (ref_idx, best) in best_ref_mvs.iter_mut().enumerate().take(refs_used) {
                    let list = self.whole_block_mv_refs(mi.ref_frame[ref_idx], bsize);
                    let (nearest, near) = find_best_ref_mvs(&list, allow_hp);
                    // `tmp_mvs[refmv_count - 1]` (`:779-780`): `refmv_count`
                    // is 2 for NEARMV and 1 for everything else — see the
                    // module docs.
                    *best = if mi.mode == NEARMV { near } else { nearest };
                }
            }
            mi.mv = self.assign_mv(
                r,
                mi.mode,
                &best_ref_mvs,
                &best_ref_mvs,
                refs_used,
                allow_hp,
            )?;
        }
        Ok(())
    }

    /// The `bsize < BLOCK_8X8` half of `read_inter_block_mode_info`
    /// (`:723-770`).
    ///
    /// `num_4x4_w` / `num_4x4_h` are libvpx's `1 << xd->bmode_blocks_wl` /
    /// `_hl`, which `decode_partition` sets from the 8x8 parent's partition
    /// (`vp9_decodeframe.c:1192-1193`, `1256-1257`):
    /// `PARTITION_HORZ` gives `BLOCK_8X4` and `(2, 1)`, `PARTITION_VERT`
    /// gives `BLOCK_4X8` and `(1, 2)`, `PARTITION_SPLIT` gives `BLOCK_4X4`
    /// and `(1, 1)` — i.e. exactly `num_4x4_blocks_{wide,high}_lookup[bsize]`,
    /// which is derivable from `bsize` alone and needs no partition argument.
    fn read_sub8x8_mvs(
        &mut self,
        r: &mut BoolReader<'_>,
        mi: &mut MiInfo,
        inter_mode_ctx: usize,
        refs_used: usize,
    ) -> CodecResult<()> {
        let bsize = mi.sb_type;
        let allow_hp = self.cfg.allow_high_precision_mv;
        let num_4x4_w = usize::from(tables::NUM_4X4_BLOCKS_WIDE[bsize as usize]);
        let num_4x4_h = usize::from(tables::NUM_4X4_BLOCKS_HIGH[bsize as usize]);

        let mut best_ref_mvs = [MotionVector::zero(); 2];
        let mut got_mv_refs_for_new = false;
        let mut b_mode = NEARESTMV;

        let mut idy = 0;
        while idy < 2 {
            let mut idx = 0;
            while idx < 2 {
                let j = idy * 2 + idx;
                b_mode = self.read_inter_mode(r, inter_mode_ctx);

                let mut best_sub8x8 = [MotionVector::zero(); 2];
                if b_mode == NEARESTMV || b_mode == NEARMV {
                    for (ref_idx, best) in best_sub8x8.iter_mut().enumerate().take(refs_used) {
                        let sub = append_sub8x8_mvs_for_idx(
                            self.grid,
                            &self.cfg.tile,
                            self.cfg.prev_mvs,
                            &self.cfg.comp.ref_frame_sign_bias,
                            mi,
                            ref_idx,
                            j,
                            self.mi_row,
                            self.mi_col,
                        );
                        *best = if b_mode == NEARESTMV {
                            sub.nearest
                        } else {
                            sub.near
                        };
                    }
                } else if b_mode == NEWMV && !got_mv_refs_for_new {
                    // `:743-755` — the NEWMV reference is looked up with
                    // `block = -1` and only once per block, not once per
                    // sub-block.
                    for (ref_idx, best) in best_ref_mvs.iter_mut().enumerate().take(refs_used) {
                        let list = mv_ref_list(
                            self.grid,
                            self.cfg,
                            mi.ref_frame[ref_idx],
                            self.mi_row,
                            self.mi_col,
                            bsize,
                        );
                        *best = lower_mv_precision(list[0], allow_hp);
                        got_mv_refs_for_new = true;
                    }
                }

                mi.bmv[j] =
                    self.assign_mv(r, b_mode, &best_ref_mvs, &best_sub8x8, refs_used, allow_hp)?;

                // `:763-764`, run before the next sub-block so that
                // `append_sub8x8_mvs_for_idx` sees the finished entries.
                if num_4x4_h == 2 {
                    mi.bmv[j + 2] = mi.bmv[j];
                }
                if num_4x4_w == 2 {
                    mi.bmv[j + 1] = mi.bmv[j];
                }
                idx += num_4x4_w;
            }
            idy += num_4x4_h;
        }

        // `:768-770`: the block mode is the *last* sub-block's, and the block
        // motion vector is sub-block 3's.
        mi.mode = b_mode;
        mi.mv = mi.bmv[3];
        Ok(())
    }

    /// The candidate list for a whole (>= 8x8) block, i.e. `dec_find_mv_refs`
    /// with `block = -1` (`:777-778`).
    fn whole_block_mv_refs(&self, ref_frame: i8, bsize: u8) -> [MotionVector; 2] {
        mv_ref_list(
            self.grid,
            self.cfg,
            ref_frame,
            self.mi_row,
            self.mi_col,
            bsize,
        )
    }

    /// libvpx `assign_mv` (`:402-434`).
    ///
    /// `ref_mv` is the `NEWMV` predictor and `near_nearest_mv` the value
    /// `NEARESTMV` / `NEARMV` copy; at the block level libvpx passes the same
    /// array for both (`:783-784`).
    fn assign_mv(
        &mut self,
        r: &mut BoolReader<'_>,
        mode: u8,
        ref_mv: &[MotionVector; 2],
        near_nearest_mv: &[MotionVector; 2],
        refs_used: usize,
        allow_hp: bool,
    ) -> CodecResult<[MotionVector; 2]> {
        match mode {
            NEWMV => {
                let mut mv = [MotionVector::zero(); 2];
                for (i, out) in mv.iter_mut().enumerate().take(refs_used) {
                    *out = read_mv(
                        r,
                        ref_mv[i],
                        &self.probs.mv,
                        self.counts.as_deref_mut().map(|c| &mut c.mv),
                        allow_hp,
                    );
                    if !is_mv_valid(*out) {
                        return Err(CodecError::InvalidBitstream(format!(
                            "VP9: motion vector ({}, {}) outside the coded range \
                             ({MV_LOW}, {MV_UPP})",
                            out.row, out.col
                        )));
                    }
                }
                Ok(mv)
            }
            // `copy_mv_pair` (`:422`) copies both entries; see the module docs
            // on why the unused second entry is zero here rather than libvpx's
            // uninitialised value.
            NEARESTMV | NEARMV => Ok(*near_nearest_mv),
            ZEROMV => Ok([MotionVector::zero(); 2]),
            other => Err(CodecError::InvalidBitstream(format!(
                "VP9: invalid inter prediction mode {other}"
            ))),
        }
    }
}

/// libvpx `read_tx_size` (`vp9_decodemv.c:80-89`) plus the
/// `read_selected_tx_size` (`:64-78`) it delegates to:
///
/// ```c
/// static TX_SIZE read_tx_size(VP9_COMMON *cm, MACROBLOCKD *xd, int allow_select,
///                             vpx_reader *r) {
///   TX_MODE tx_mode = cm->tx_mode;
///   BLOCK_SIZE bsize = xd->mi[0]->sb_type;
///   const TX_SIZE max_tx_size = max_txsize_lookup[bsize];
///   if (allow_select && tx_mode == TX_MODE_SELECT && bsize >= BLOCK_8X8)
///     return read_selected_tx_size(cm, xd, max_tx_size, r);
///   else
///     return VPXMIN(max_tx_size, tx_mode_to_biggest_tx_size[tx_mode]);
/// }
/// ```
///
/// A free function rather than only a [`Ctx`] method because **both** frame
/// types read this symbol, from the same probabilities, with the same
/// context ([`get_tx_size_context`]) and the same counter indexing — and the
/// counter indexing is the subtle half: the array is picked by the block's
/// *maximum* transform size (`get_tx_counts`, `vp9_pred_common.h:183-191`)
/// and indexed by the size actually coded, so `p8x8` / `p16x16` / `p32x32`
/// name the maximum, not the result. Two copies of that rule would be two
/// chances to get it wrong in only one of them.
#[must_use]
pub fn read_tx_size(
    r: &mut BoolReader<'_>,
    probs: &FrameProbs,
    counts: Option<&mut FrameCounts>,
    above: Option<&MiInfo>,
    left: Option<&MiInfo>,
    bsize: u8,
    tx_mode: TxMode,
    allow_select: bool,
) -> u8 {
    let max_tx_size = tables::MAX_TXSIZE_LOOKUP[bsize as usize];
    if !(allow_select && tx_mode == TxMode::Select && bsize >= BLOCK_8X8) {
        return max_tx_size.min(tx_mode.biggest_tx_size() as u8);
    }

    let ctx = get_tx_size_context(above, left, bsize);
    // Copied out of the probability context rather than borrowed, so the
    // counter update below can take `counts` mutably.
    let p: [u8; 3] = match max_tx_size {
        1 => [probs.tx8[ctx][0], 0, 0],
        2 => [probs.tx16[ctx][0], probs.tx16[ctx][1], 0],
        _ => probs.tx32[ctx],
    };
    let mut tx = u8::from(r.read_bool(p[0]));
    if tx != 0 && max_tx_size >= 2 {
        tx += u8::from(r.read_bool(p[1]));
        if tx != 1 && max_tx_size >= 3 {
            tx += u8::from(r.read_bool(p[2]));
        }
    }
    if let Some(c) = counts {
        let coded = tx as usize;
        match max_tx_size {
            1 => c.tx.p8x8[ctx][coded] += 1,
            2 => c.tx.p16x16[ctx][coded] += 1,
            _ => c.tx.p32x32[ctx][coded] += 1,
        }
    }
    tx
}

/// `dec_find_mv_refs(..., block = -1)` (`:484-607`), expressed through the
/// common scan — see the module docs for why the two agree on every value
/// that is read.
fn mv_ref_list<G: MiGrid + ?Sized>(
    grid: &G,
    cfg: &InterFrameCfg<'_>,
    ref_frame: i8,
    mi_row: usize,
    mi_col: usize,
    bsize: u8,
) -> [MotionVector; 2] {
    find_mv_refs(
        grid,
        &cfg.tile,
        cfg.prev_mvs,
        &cfg.comp.ref_frame_sign_bias,
        ref_frame,
        mi_row,
        mi_col,
        bsize,
        -1,
    )
    .list
}

// ---------------------------------------------------------------------------
// Motion vectors
// ---------------------------------------------------------------------------

/// libvpx `is_mv_valid` (`:389-392`):
/// `mv->row > MV_LOW && mv->row < MV_UPP && mv->col > MV_LOW && mv->col < MV_UPP`.
#[must_use]
pub fn is_mv_valid(mv: MotionVector) -> bool {
    let (row, col) = (i32::from(mv.row), i32::from(mv.col));
    row > MV_LOW && row < MV_UPP && col > MV_LOW && col < MV_UPP
}

/// libvpx `mv_joint_vertical` (`vp9_entropymv.h:46-48`): the joint says the
/// **row** component is coded.
#[must_use]
fn mv_joint_vertical(joint: u8) -> bool {
    joint == 2 || joint == 3
}

/// libvpx `mv_joint_horizontal` (`vp9_entropymv.h:50-52`): the joint says the
/// **column** component is coded.
#[must_use]
fn mv_joint_horizontal(joint: u8) -> bool {
    joint == 1 || joint == 3
}

/// libvpx `read_mv` (`vp9_decodemv.c:267-285`):
///
/// ```c
/// const MV_JOINT_TYPE joint_type = vpx_read_tree(r, vp9_mv_joint_tree, ctx->joints);
/// const int use_hp = allow_hp && use_mv_hp(ref);
/// MV diff = { 0, 0 };
/// if (mv_joint_vertical(joint_type))   diff.row = read_mv_component(r, &ctx->comps[0], use_hp);
/// if (mv_joint_horizontal(joint_type)) diff.col = read_mv_component(r, &ctx->comps[1], use_hp);
/// vp9_inc_mv(&diff, counts);
/// mv->row = ref->row + diff.row;
/// mv->col = ref->col + diff.col;
/// ```
///
/// Three details worth naming:
///
/// * **`use_hp` depends on the *reference*, not the frame alone**
///   ([`use_mv_hp`]): a large predictor turns high-precision bits off even on
///   a frame that allows them.
/// * **`comps[0]` is the row and `comps[1]` the column** — the vertical
///   component is read first when both are present.
/// * **The counted value is the difference**, not the resulting vector.
///
/// The sum is `int` arithmetic assigned to an `int16_t` in C. `diff` is
/// bounded by `|diff| <= 16384` (class 10 contributes `2 << 12` and the
/// offset at most `8192`), so it always fits an `i16`; the outer sum is done
/// with `wrapping_add`, which is what the C assignment compiles to and which
/// a synthetic stream can reach without panicking a debug build.
#[must_use]
pub fn read_mv(
    r: &mut BoolReader<'_>,
    ref_mv: MotionVector,
    ctx: &NmvContext,
    counts: Option<&mut NmvCounts>,
    allow_hp: bool,
) -> MotionVector {
    let joint = r.read_tree(&MV_JOINT_TREE, &ctx.joints);
    let use_hp = allow_hp && use_mv_hp(ref_mv);

    let mut diff = MotionVector::zero();
    if mv_joint_vertical(joint) {
        diff.row = read_mv_component(r, &ctx.comps[0], use_hp) as i16;
    }
    if mv_joint_horizontal(joint) {
        diff.col = read_mv_component(r, &ctx.comps[1], use_hp) as i16;
    }

    if let Some(counts) = counts {
        inc_mv(diff, counts);
    }

    MotionVector::new(
        ref_mv.row.wrapping_add(diff.row),
        ref_mv.col.wrapping_add(diff.col),
    )
}

/// libvpx `read_mv_component` (`vp9_decodemv.c:235-265`).
///
/// ```c
/// const int sign = vpx_read(r, mvcomp->sign);
/// const int mv_class = vpx_read_tree(r, vp9_mv_class_tree, mvcomp->classes);
/// const int class0 = mv_class == MV_CLASS_0;
/// if (class0) { d = vpx_read(r, mvcomp->class0[0]); mag = 0; }
/// else {
///   const int n = mv_class + CLASS0_BITS - 1;
///   d = 0;
///   for (i = 0; i < n; ++i) d |= vpx_read(r, mvcomp->bits[i]) << i;
///   mag = CLASS0_SIZE << (mv_class + 2);
/// }
/// fr = vpx_read_tree(r, vp9_mv_fp_tree, class0 ? mvcomp->class0_fp[d] : mvcomp->fp);
/// hp = usehp ? vpx_read(r, class0 ? mvcomp->class0_hp : mvcomp->hp) : 1;
/// mag += ((d << 3) | (fr << 1) | hp) + 1;
/// return sign ? -mag : mag;
/// ```
///
/// The integer bits are read **LSB first** (`d |= bit << i`), unlike every
/// other multi-bit field in VP9, and the class-0 magnitude bit is read
/// through [`MV_CLASS0_TREE`] rather than as a plain bool — the same single
/// `class0[0]` probability either way, but the tree spelling is what libvpx's
/// `class0` prob array is shaped for.
///
/// When high-precision bits are disabled the `hp` term defaults to `1`, not
/// `0`: an eighth-pel value of zero is not representable, so the magnitude
/// grid stays on odd eighths.
///
/// Returns the signed component in 1/8-pel units, `1..=16384` in magnitude.
#[must_use]
pub fn read_mv_component(r: &mut BoolReader<'_>, mvcomp: &NmvComponent, usehp: bool) -> i32 {
    let sign = r.read_bool(mvcomp.sign);
    let mv_class = usize::from(r.read_tree(&MV_CLASS_TREE, &mvcomp.classes));
    let class0 = mv_class == 0;

    let (d, mut mag) = if class0 {
        (
            usize::from(r.read_tree(&MV_CLASS0_TREE, &mvcomp.class0)),
            0i32,
        )
    } else {
        let n = mv_class + CLASS0_BITS - 1;
        let mut d = 0usize;
        for i in 0..n {
            d |= usize::from(r.read_bool(mvcomp.bits[i])) << i;
        }
        (d, (CLASS0_SIZE as i32) << (mv_class + 2))
    };

    let fr = i32::from(r.read_tree(
        &MV_FP_TREE,
        if class0 {
            &mvcomp.class0_fp[d]
        } else {
            &mvcomp.fp
        },
    ));

    let hp = if usehp {
        i32::from(r.read_bool(if class0 { mvcomp.class0_hp } else { mvcomp.hp }))
    } else {
        1
    };

    mag += (((d as i32) << 3) | (fr << 1) | hp) + 1;
    if sign {
        -mag
    } else {
        mag
    }
}

/// libvpx `vp9_inc_mv` (`vp9/common/vp9_entropymv.c:139-152`).
///
/// ```c
/// const MV_JOINT_TYPE j = vp9_get_mv_joint(mv);
/// ++counts->joints[j];
/// if (mv_joint_vertical(j))   inc_mv_component(mv->row, &counts->comps[0], 1, 1);
/// if (mv_joint_horizontal(j)) inc_mv_component(mv->col, &counts->comps[1], 1, 1);
/// ```
///
/// The joint is **recomputed from the difference** rather than taken from the
/// symbol that was read; the two always agree, because
/// [`read_mv_component`] can never return zero.
///
/// The `usehp` argument is hard-coded `1` here while the *read* used
/// `allow_hp && use_mv_hp(ref)`. That is deliberate in libvpx: on a block
/// that coded no high-precision bit, the counter still records the defaulted
/// `hp = 1`. Passing the real `use_hp` instead would change every adapted
/// `hp` probability on frames that mix precisions.
pub fn inc_mv(diff: MotionVector, counts: &mut NmvCounts) {
    let joint = diff.joint() as usize;
    counts.joints[joint] += 1;
    if mv_joint_vertical(joint as u8) {
        inc_mv_component(i32::from(diff.row), &mut counts.comps[0]);
    }
    if mv_joint_horizontal(joint as u8) {
        inc_mv_component(i32::from(diff.col), &mut counts.comps[1]);
    }
}

/// libvpx `inc_mv_component` (`vp9_entropymv.c:111-137`) with `incr = 1` and
/// `usehp = 1`, its only call site.
///
/// Decomposes the coded magnitude back into the fields
/// [`read_mv_component`] assembled: `z = |v| - 1`, class from
/// [`mv_class`], `o = z - class_base`, then `d = o >> 3`,
/// `f = (o >> 1) & 3`, `e = o & 1`.
fn inc_mv_component(v: i32, counts: &mut super::counts::NmvComponentCounts) {
    debug_assert!(v != 0, "VP9: zero motion-vector component cannot be coded");
    let sign = usize::from(v < 0);
    counts.sign[sign] += 1;
    let z = v.abs() - 1;

    let (class, offset) = mv_class(z);
    counts.classes[class] += 1;

    let d = (offset >> 3) as usize;
    let f = ((offset >> 1) & 3) as usize;
    let e = (offset & 1) as usize;

    if class == 0 {
        counts.class0[d] += 1;
        counts.class0_fp[d][f] += 1;
        counts.class0_hp[e] += 1;
    } else {
        let bits = class + CLASS0_BITS - 1;
        for i in 0..bits {
            counts.bits[i][(d >> i) & 1] += 1;
        }
        counts.fp[f] += 1;
        counts.hp[e] += 1;
    }
}

/// libvpx `vp9_get_mv_class` (`vp9_entropymv.c:99-105`) together with
/// `mv_class_base` (`:95-97`):
///
/// ```c
/// const MV_CLASS_TYPE c = (z >= CLASS0_SIZE * 4096) ? MV_CLASS_10
///                                                   : log_in_base_2[z >> 3];
/// *offset = z - (c ? CLASS0_SIZE << (c + 2) : 0);
/// ```
///
/// `log_in_base_2` is a 1025-entry table of `floor(log2(i))` with
/// `log_in_base_2[0] = 0` (verified entry by entry against the libvpx
/// source), which is what the branch below computes without the table.
///
/// Returns `(class, offset)`.
#[must_use]
fn mv_class(z: i32) -> (usize, i32) {
    let class = if z >= (CLASS0_SIZE as i32) * 4096 {
        10
    } else {
        let i = (z >> 3) as u32;
        if i == 0 {
            0
        } else {
            (u32::BITS - 1 - i.leading_zeros()) as usize
        }
    };
    let base = if class == 0 {
        0
    } else {
        (CLASS0_SIZE as i32) << (class + 2)
    };
    (class, z - base)
}

#[cfg(test)]
mod tests;
