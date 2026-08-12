//! VP9 prediction contexts — the entropy-context derivations an inter frame
//! needs before it can read any of its mode/reference/filter symbols.
//!
//! This is a port of libvpx `vp9/common/vp9_pred_common.c` and the inline
//! helpers in its header `vp9/common/vp9_pred_common.h`, tag **v1.15.2**
//! (commit `d168454`) — the same tag [`super::tables_inter`] cites and the
//! encoder that produced the Wave-3 fixtures. Every function below carries a
//! `vp9_pred_common.{c,h}:<lines>` citation for the exact source range it
//! reproduces.
//!
//! # Verification
//!
//! The expected values in this module's tests are not a second transcription
//! of the branch trees below — that would only prove the same reading twice.
//! They were produced by compiling the **verbatim** libvpx function bodies
//! (extracted by line range from `vp9_pred_common.c` / `vp9_pred_common.h`,
//! against minimal stub types) and dumping every context over the same
//! neighbour enumeration the tests use. Every table here is a literal copy of
//! that reference decoder's output: the seven 9x9 reference grids, the 5x5
//! interpolation-filter grid, the 3x3 segment-id and skip grids, the four 6x6
//! transform-size grids, and all 156 `get_segment_id` results.
//!
//! # What lives here, and what does not
//!
//! `vp9_pred_common.c` has eleven public entry points. Two of them — the
//! frame-level `vp9_compound_reference_allowed` (`vp9_pred_common.c:16-22`)
//! and `vp9_setup_compound_reference_mode` (`vp9_pred_common.c:24-40`) — were
//! already ported in [`super::refs`] as
//! [`is_compound_reference_allowed`](super::refs::is_compound_reference_allowed)
//! and
//! [`setup_compound_reference_mode`](super::refs::setup_compound_reference_mode).
//! They are *not* duplicated here; [`CompRefState::from_sign_bias`] calls the
//! `refs` version so there is exactly one derivation of `comp_fixed_ref` /
//! `comp_var_ref` in the decoder. Together with the nine functions in this
//! module, `vp9_pred_common` is fully ported.
//!
//! # Shape of the port
//!
//! libvpx reads its neighbours through `xd->above_mi` / `xd->left_mi`, which
//! are `NULL` at a frame edge (above) or a *tile* edge (left) — the "one
//! element border … initialized to 0" the source comments keep referring to.
//! Here that is an honest `Option<&MiInfo>`, so the "no neighbour" branch is a
//! distinct, testable input rather than a zeroed sentinel block. The caller is
//! responsible for producing `None` under exactly libvpx's conditions;
//! [`super::recon`] already does this for the intra path (`mi_row > 0` for
//! above, `mi_col > tile.mi_col_start` for left).
//!
//! Nothing here is fallible: every function is a total, side-effect-free
//! mapping from neighbour state (plus, for four of them, a little frame-level
//! state) to a small context index. There is no bitstream parsing, so there is
//! no error path to be honest or dishonest about.
//!
//! # Reference-frame convention
//!
//! **Verified identical to libvpx — no mapping layer is applied or needed.**
//! [`super::refs`] defines [`NONE_FRAME`](super::refs::NONE_FRAME) `= -1`,
//! [`INTRA_FRAME`](super::refs::INTRA_FRAME) `= 0`,
//! [`LAST_FRAME`](super::refs::LAST_FRAME) `= 1`,
//! [`GOLDEN_FRAME`](super::refs::GOLDEN_FRAME) `= 2`,
//! [`ALTREF_FRAME`](super::refs::ALTREF_FRAME) `= 3`, which is exactly
//! libvpx's `MV_REFERENCE_FRAME` numbering (`vp9/common/vp9_blockd.h`), and
//! [`MiInfo::default`](super::recon::MiInfo::default) initialises
//! `ref_frame` to `[INTRA_FRAME, NONE_FRAME]` just as
//! `read_intra_frame_mode_info` does. The comparisons below therefore use the
//! `refs` constants directly.
//!
//! ## `is_inter` is a cache; `ref_frame` is normative
//!
//! [`MiInfo`](super::recon::MiInfo) carries an `is_inter` bool, but this
//! module never reads it. libvpx derives both predicates from `ref_frame`
//! alone (`vp9_blockd.h:102-108`):
//!
//! ```text
//! is_inter_block(mi)  ==  mi->ref_frame[0] > INTRA_FRAME
//! has_second_ref(mi)  ==  mi->ref_frame[1] > INTRA_FRAME
//! ```
//!
//! so that is what [`is_inter_block`] and [`has_second_ref`] do. Should a
//! future package ever leave `MiInfo::is_inter` out of step with
//! `ref_frame[0]`, these contexts stay bit-exact with libvpx rather than
//! following the stale cache.
//!
//! # Integration hazard: `interp_filter`'s two "switchable" numbers
//!
//! [`SWITCHABLE_FILTERS`] is **3**, and it is the value an
//! `interp_filter` field must hold for a block that has no filter of its own.
//! libvpx sets it explicitly for every intra block inside an inter frame —
//! `mi->interp_filter = SWITCHABLE_FILTERS;` with the comment "Initialize
//! interp_filter here so we do not have to check for inter block modes in
//! [`get_pred_context_switchable_interp`]"
//! (`vp9/decoder/vp9_decodemv.c:381-383`).
//!
//! This is easy to get wrong twice over:
//!
//! - `MiInfo`'s own field documentation lists ``4`` as SWITCHABLE. That `4` is
//!   the frame-level `INTERP_FILTER` enum value `SWITCHABLE`
//!   (`vp9/common/vp9_filter.h`), which says "this frame codes a filter per
//!   block". It is **not** the per-block sentinel. Feeding `4` into
//!   [`get_pred_context_switchable_interp`] returns `4`, one past the end of
//!   `switchable_interp_prob[SWITCHABLE_FILTER_CONTEXTS]`.
//! - [`MiInfo::default`](super::recon::MiInfo::default) leaves `interp_filter`
//!   at `0` (EIGHTTAP). A default-constructed intra block therefore looks like
//!   a real EIGHTTAP neighbour to this function.
//!
//! Whichever package fills `MiInfo` for inter frames must set
//! `interp_filter = SWITCHABLE_FILTERS` (3) on intra blocks, matching
//! `vp9_decodemv.c:383`.

#![forbid(unsafe_code)]

use super::recon::MiInfo;
use super::refs::{
    setup_compound_reference_mode, ALTREF_FRAME, GOLDEN_FRAME, INTRA_FRAME, LAST_FRAME,
};
use super::tables;

/// libvpx `vp9/common/vp9_filter.h:26`: `SWITCHABLE_FILTERS 3`.
///
/// Doubles as the per-block "no switchable filter here" sentinel — see the
/// module-level integration hazard note.
pub const SWITCHABLE_FILTERS: u8 = 3;

/// libvpx `vp9/common/vp9_filter.h:31`:
/// `SWITCHABLE_FILTER_CONTEXTS (SWITCHABLE_FILTERS + 1)`.
///
/// The number of contexts [`get_pred_context_switchable_interp`] can return.
pub const SWITCHABLE_FILTER_CONTEXTS: usize = 4;

/// libvpx `vp9/common/vp9_enums.h:137`: `INTRA_INTER_CONTEXTS 4`.
pub const INTRA_INTER_CONTEXTS: usize = 4;

/// libvpx `vp9/common/vp9_enums.h:138`: `COMP_INTER_CONTEXTS 5`.
pub const COMP_INTER_CONTEXTS: usize = 5;

/// libvpx `vp9/common/vp9_enums.h:139`: `REF_CONTEXTS 5`.
pub const REF_CONTEXTS: usize = 5;

/// libvpx `vp9/common/vp9_seg_common.h:26`: `PREDICTION_PROBS 3`.
///
/// The number of contexts [`get_pred_context_seg_id`] can return.
pub const PREDICTION_PROBS: usize = 3;

/// libvpx `vp9/common/vp9_seg_common.h:23`: `MAX_SEGMENTS 8`.
pub const MAX_SEGMENTS: u8 = 8;

/// Number of contexts [`get_skip_context`] can return (`above + left`, each
/// contributing 0 or 1 — libvpx `vp9/common/vp9_enums.h:131`:
/// `SKIP_CONTEXTS 3`).
pub const SKIP_CONTEXTS: usize = 3;

/// libvpx `is_inter_block` (`vp9/common/vp9_blockd.h:102-104`).
///
/// Derived from `ref_frame[0]`, never from the `MiInfo::is_inter` cache — see
/// the module docs.
#[inline]
pub fn is_inter_block(mi: &MiInfo) -> bool {
    mi.ref_frame[0] > INTRA_FRAME
}

/// libvpx `has_second_ref` (`vp9/common/vp9_blockd.h:106-108`): the block is
/// compound-predicted.
#[inline]
pub fn has_second_ref(mi: &MiInfo) -> bool {
    mi.ref_frame[1] > INTRA_FRAME
}

/// Frame-level compound-reference state (libvpx `VP9_COMMON::comp_fixed_ref`,
/// `comp_var_ref`, `ref_frame_sign_bias`).
///
/// Only the two compound-reference contexts need any of this;
/// [`get_pred_context_single_ref_p1`] / [`get_pred_context_single_ref_p2`]
/// take no frame state at all, matching libvpx's own parameter lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompRefState {
    /// The reference that is always one half of a compound pair
    /// (`cm->comp_fixed_ref`).
    pub comp_fixed_ref: i8,
    /// The two references that can be the other half (`cm->comp_var_ref`).
    pub comp_var_ref: [i8; 2],
    /// `cm->ref_frame_sign_bias`, indexed by reference frame — entry `0`
    /// ([`INTRA_FRAME`]) is unused padding, exactly as in libvpx.
    pub ref_frame_sign_bias: [bool; 4],
}

impl CompRefState {
    /// Derives the compound-reference state from the frame's sign biases.
    ///
    /// Delegates to [`setup_compound_reference_mode`] (the port of
    /// `vp9_pred_common.c:24-40` that already lives in [`super::refs`]) so the
    /// decoder has a single derivation of these values.
    #[must_use]
    pub fn from_sign_bias(ref_frame_sign_bias: &[bool; 4]) -> Self {
        let (comp_fixed_ref, comp_var_ref) = setup_compound_reference_mode(ref_frame_sign_bias);
        Self {
            comp_fixed_ref,
            comp_var_ref,
            ref_frame_sign_bias: *ref_frame_sign_bias,
        }
    }

    /// libvpx `fix_ref_idx` (`vp9_pred_common.c:97`):
    /// `cm->ref_frame_sign_bias[cm->comp_fixed_ref]`.
    ///
    /// Which slot of a compound block's `ref_frame` pair holds
    /// [`Self::comp_fixed_ref`].
    #[must_use]
    pub fn fix_ref_idx(&self) -> usize {
        usize::from(self.ref_frame_sign_bias[self.comp_fixed_ref as usize])
    }

    /// libvpx `var_ref_idx` (`vp9_pred_common.c:98`): `!fix_ref_idx`.
    ///
    /// Which slot of a compound block's `ref_frame` pair holds the *variable*
    /// reference — the one the `comp_ref` symbol selects.
    #[must_use]
    pub fn var_ref_idx(&self) -> usize {
        1 - self.fix_ref_idx()
    }
}

/// libvpx `get_intra_inter_context` (`vp9_pred_common.h:97-111`).
///
/// Context for the `is_inter` flag. libvpx's own summary of the mapping
/// (`vp9_pred_common.h:93-96`):
///
/// ```text
/// 0 - inter/inter, inter/--, --/inter, --/--
/// 1 - intra/inter, inter/intra
/// 2 - intra/--, --/intra
/// 3 - intra/intra
/// ```
///
/// Range: `0..`[`INTRA_INTER_CONTEXTS`].
#[must_use]
pub fn get_intra_inter_context(above: Option<&MiInfo>, left: Option<&MiInfo>) -> usize {
    match (above, left) {
        (Some(a), Some(l)) => {
            let above_intra = !is_inter_block(a);
            let left_intra = !is_inter_block(l);
            if left_intra && above_intra {
                3
            } else {
                usize::from(left_intra || above_intra)
            }
        }
        (Some(edge), None) | (None, Some(edge)) => 2 * usize::from(!is_inter_block(edge)),
        (None, None) => 0,
    }
}

/// libvpx `vp9_get_reference_mode_context` (`vp9_pred_common.c:42-82`).
///
/// Context for the per-block single/compound `reference_mode` flag. Needs only
/// `comp_fixed_ref` out of the frame state (libvpx passes all of `cm`).
///
/// Range: `0..`[`COMP_INTER_CONTEXTS`].
#[must_use]
pub fn get_reference_mode_context(
    above: Option<&MiInfo>,
    left: Option<&MiInfo>,
    comp_fixed_ref: i8,
) -> usize {
    match (above, left) {
        (Some(a), Some(l)) => {
            if !has_second_ref(a) && !has_second_ref(l) {
                // neither edge uses comp pred (0/1) — libvpx's `^` on two
                // 0/1 ints is `!=` on two bools.
                usize::from(
                    (a.ref_frame[0] == comp_fixed_ref) != (l.ref_frame[0] == comp_fixed_ref),
                )
            } else if !has_second_ref(a) {
                // one of two edges uses comp pred (2/3)
                2 + usize::from(a.ref_frame[0] == comp_fixed_ref || !is_inter_block(a))
            } else if !has_second_ref(l) {
                // one of two edges uses comp pred (2/3)
                2 + usize::from(l.ref_frame[0] == comp_fixed_ref || !is_inter_block(l))
            } else {
                // both edges use comp pred (4)
                4
            }
        }
        (Some(edge), None) | (None, Some(edge)) => {
            if has_second_ref(edge) {
                // edge uses comp pred (3)
                3
            } else {
                // edge does not use comp pred (0/1)
                usize::from(edge.ref_frame[0] == comp_fixed_ref)
            }
        }
        // no edges available (1)
        (None, None) => 1,
    }
}

/// libvpx `vp9_get_pred_context_comp_ref_p` (`vp9_pred_common.c:85-165`).
///
/// Context for the `comp_ref` symbol, which picks between
/// `comp_var_ref[0]` and `comp_var_ref[1]` for a compound-predicted block.
///
/// Range: `0..`[`REF_CONTEXTS`].
#[must_use]
pub fn get_pred_context_comp_ref_p(
    above: Option<&MiInfo>,
    left: Option<&MiInfo>,
    comp: &CompRefState,
) -> usize {
    let fixed = comp.comp_fixed_ref;
    let cvr0 = comp.comp_var_ref[0];
    let cvr1 = comp.comp_var_ref[1];
    let var_ref_idx = comp.var_ref_idx();

    match (above, left) {
        (Some(a), Some(l)) => {
            let above_intra = !is_inter_block(a);
            let left_intra = !is_inter_block(l);

            if above_intra && left_intra {
                // intra/intra (2)
                2
            } else if above_intra || left_intra {
                // intra/inter
                let edge = if above_intra { l } else { a };
                if has_second_ref(edge) {
                    // comp pred (1/3)
                    1 + 2 * usize::from(edge.ref_frame[var_ref_idx] != cvr1)
                } else {
                    // single pred (1/3)
                    1 + 2 * usize::from(edge.ref_frame[0] != cvr1)
                }
            } else {
                // inter/inter
                let l_sg = !has_second_ref(l);
                let a_sg = !has_second_ref(a);
                let vrfa = if a_sg {
                    a.ref_frame[0]
                } else {
                    a.ref_frame[var_ref_idx]
                };
                let vrfl = if l_sg {
                    l.ref_frame[0]
                } else {
                    l.ref_frame[var_ref_idx]
                };

                if vrfa == vrfl && cvr1 == vrfa {
                    0
                } else if l_sg && a_sg {
                    // single/single — the only use of `comp_var_ref[0]` in
                    // the whole of vp9_pred_common (`vp9_pred_common.c:125-126`).
                    if (vrfa == fixed && vrfl == cvr0) || (vrfl == fixed && vrfa == cvr0) {
                        4
                    } else if vrfa == vrfl {
                        3
                    } else {
                        1
                    }
                } else if l_sg || a_sg {
                    // single/comp: `vrfc` is the compound edge's variable
                    // reference, `rfs` the single edge's reference.
                    let vrfc = if l_sg { vrfa } else { vrfl };
                    let rfs = if a_sg { vrfa } else { vrfl };
                    if vrfc == cvr1 && rfs != cvr1 {
                        1
                    } else if rfs == cvr1 && vrfc != cvr1 {
                        2
                    } else {
                        4
                    }
                } else if vrfa == vrfl {
                    // comp/comp
                    4
                } else {
                    2
                }
            }
        }
        (Some(edge), None) | (None, Some(edge)) => {
            if is_inter_block(edge) {
                if has_second_ref(edge) {
                    4 * usize::from(edge.ref_frame[var_ref_idx] != cvr1)
                } else {
                    3 * usize::from(edge.ref_frame[0] != cvr1)
                }
            } else {
                2
            }
        }
        // no edges available (2)
        (None, None) => 2,
    }
}

/// The `single_ref_p1` context contribution of a lone edge block, shared by
/// libvpx's "one edge available" (`vp9_pred_common.c:219-223`) and
/// "intra/inter" (`vp9_pred_common.c:185-189`) branches, which are textually
/// identical.
#[inline]
fn single_ref_p1_edge(edge: &MiInfo) -> usize {
    if has_second_ref(edge) {
        1 + usize::from(edge.ref_frame[0] == LAST_FRAME || edge.ref_frame[1] == LAST_FRAME)
    } else {
        4 * usize::from(edge.ref_frame[0] == LAST_FRAME)
    }
}

/// libvpx `vp9_get_pred_context_single_ref_p1` (`vp9_pred_common.c:167-231`).
///
/// Context for the first single-reference bit — "is the reference
/// [`LAST_FRAME`]?". Takes no frame state.
///
/// Range: `0..`[`REF_CONTEXTS`].
#[must_use]
pub fn get_pred_context_single_ref_p1(above: Option<&MiInfo>, left: Option<&MiInfo>) -> usize {
    match (above, left) {
        (Some(a), Some(l)) => {
            let above_intra = !is_inter_block(a);
            let left_intra = !is_inter_block(l);

            if above_intra && left_intra {
                // intra/intra
                2
            } else if above_intra || left_intra {
                // intra/inter or inter/intra
                single_ref_p1_edge(if above_intra { l } else { a })
            } else {
                // inter/inter
                let above_has_second = has_second_ref(a);
                let left_has_second = has_second_ref(l);
                let above0 = a.ref_frame[0];
                let above1 = a.ref_frame[1];
                let left0 = l.ref_frame[0];
                let left1 = l.ref_frame[1];

                if above_has_second && left_has_second {
                    1 + usize::from(
                        above0 == LAST_FRAME
                            || above1 == LAST_FRAME
                            || left0 == LAST_FRAME
                            || left1 == LAST_FRAME,
                    )
                } else if above_has_second || left_has_second {
                    // `rfs` is the single edge's reference; `crf1`/`crf2` the
                    // compound edge's pair.
                    let rfs = if above_has_second { left0 } else { above0 };
                    let crf1 = if above_has_second { above0 } else { left0 };
                    let crf2 = if above_has_second { above1 } else { left1 };

                    if rfs == LAST_FRAME {
                        3 + usize::from(crf1 == LAST_FRAME || crf2 == LAST_FRAME)
                    } else {
                        usize::from(crf1 == LAST_FRAME || crf2 == LAST_FRAME)
                    }
                } else {
                    2 * usize::from(above0 == LAST_FRAME) + 2 * usize::from(left0 == LAST_FRAME)
                }
            }
        }
        (Some(edge), None) | (None, Some(edge)) => {
            if is_inter_block(edge) {
                single_ref_p1_edge(edge)
            } else {
                // intra
                2
            }
        }
        // no edges available
        (None, None) => 2,
    }
}

/// libvpx `vp9_get_pred_context_single_ref_p2` (`vp9_pred_common.c:233-316`).
///
/// Context for the second single-reference bit — read only when the first bit
/// said "not [`LAST_FRAME`]", and distinguishing [`GOLDEN_FRAME`] from
/// [`ALTREF_FRAME`]. Takes no frame state.
///
/// Note that unlike `p1`, the "intra/inter" branch
/// (`vp9_pred_common.c:250-260`) and the "one edge available" branch
/// (`vp9_pred_common.c:300-310`) are *not* the same expression, so neither is
/// factored out here.
///
/// Range: `0..`[`REF_CONTEXTS`].
#[must_use]
pub fn get_pred_context_single_ref_p2(above: Option<&MiInfo>, left: Option<&MiInfo>) -> usize {
    match (above, left) {
        (Some(a), Some(l)) => {
            let above_intra = !is_inter_block(a);
            let left_intra = !is_inter_block(l);

            if above_intra && left_intra {
                // intra/intra
                2
            } else if above_intra || left_intra {
                // intra/inter or inter/intra — vp9_pred_common.c:250-260
                let edge = if above_intra { l } else { a };
                if has_second_ref(edge) {
                    1 + 2 * usize::from(
                        edge.ref_frame[0] == GOLDEN_FRAME || edge.ref_frame[1] == GOLDEN_FRAME,
                    )
                } else if edge.ref_frame[0] == LAST_FRAME {
                    3
                } else {
                    4 * usize::from(edge.ref_frame[0] == GOLDEN_FRAME)
                }
            } else {
                // inter/inter
                let above_has_second = has_second_ref(a);
                let left_has_second = has_second_ref(l);
                let above0 = a.ref_frame[0];
                let above1 = a.ref_frame[1];
                let left0 = l.ref_frame[0];
                let left1 = l.ref_frame[1];

                if above_has_second && left_has_second {
                    if above0 == left0 && above1 == left1 {
                        3 * usize::from(
                            above0 == GOLDEN_FRAME
                                || above1 == GOLDEN_FRAME
                                || left0 == GOLDEN_FRAME
                                || left1 == GOLDEN_FRAME,
                        )
                    } else {
                        2
                    }
                } else if above_has_second || left_has_second {
                    let rfs = if above_has_second { left0 } else { above0 };
                    let crf1 = if above_has_second { above0 } else { left0 };
                    let crf2 = if above_has_second { above1 } else { left1 };

                    if rfs == GOLDEN_FRAME {
                        3 + usize::from(crf1 == GOLDEN_FRAME || crf2 == GOLDEN_FRAME)
                    } else if rfs == ALTREF_FRAME {
                        usize::from(crf1 == GOLDEN_FRAME || crf2 == GOLDEN_FRAME)
                    } else {
                        1 + 2 * usize::from(crf1 == GOLDEN_FRAME || crf2 == GOLDEN_FRAME)
                    }
                } else if above0 == LAST_FRAME && left0 == LAST_FRAME {
                    3
                } else if above0 == LAST_FRAME || left0 == LAST_FRAME {
                    let edge0 = if above0 == LAST_FRAME { left0 } else { above0 };
                    4 * usize::from(edge0 == GOLDEN_FRAME)
                } else {
                    2 * usize::from(above0 == GOLDEN_FRAME) + 2 * usize::from(left0 == GOLDEN_FRAME)
                }
            }
        }
        (Some(edge), None) | (None, Some(edge)) => {
            // vp9_pred_common.c:300-310
            if !is_inter_block(edge) || (edge.ref_frame[0] == LAST_FRAME && !has_second_ref(edge)) {
                2
            } else if has_second_ref(edge) {
                3 * usize::from(
                    edge.ref_frame[0] == GOLDEN_FRAME || edge.ref_frame[1] == GOLDEN_FRAME,
                )
            } else {
                4 * usize::from(edge.ref_frame[0] == GOLDEN_FRAME)
            }
        }
        // no edges available (2)
        (None, None) => 2,
    }
}

/// libvpx `get_pred_context_switchable_interp` (`vp9_pred_common.h:69-88`).
///
/// Context for the per-block interpolation-filter symbol. A missing neighbour
/// — and, per `vp9_decodemv.c:383`, an *intra* neighbour — contributes
/// [`SWITCHABLE_FILTERS`] (3); see the module-level hazard note.
///
/// Range: `0..`[`SWITCHABLE_FILTER_CONTEXTS`].
#[must_use]
pub fn get_pred_context_switchable_interp(above: Option<&MiInfo>, left: Option<&MiInfo>) -> usize {
    let left_type = left.map_or(SWITCHABLE_FILTERS, |m| m.interp_filter);
    let above_type = above.map_or(SWITCHABLE_FILTERS, |m| m.interp_filter);

    let ctx = if left_type == above_type {
        left_type
    } else if left_type == SWITCHABLE_FILTERS {
        above_type
    } else if above_type == SWITCHABLE_FILTERS {
        left_type
    } else {
        SWITCHABLE_FILTERS
    };
    usize::from(ctx)
}

/// libvpx `vp9_get_pred_context_seg_id` (`vp9_pred_common.h:41-48`).
///
/// Context for the `seg_id_predicted` flag: how many neighbours had their own
/// segment id temporally predicted.
///
/// Range: `0..`[`PREDICTION_PROBS`].
#[must_use]
pub fn get_pred_context_seg_id(above: Option<&MiInfo>, left: Option<&MiInfo>) -> usize {
    let above_sip = usize::from(above.is_some_and(|m| m.seg_id_predicted));
    let left_sip = usize::from(left.is_some_and(|m| m.seg_id_predicted));
    above_sip + left_sip
}

/// libvpx `vp9_get_skip_context` (`vp9_pred_common.h:55-61`).
///
/// Context for the `skip` flag: how many neighbours were skipped.
///
/// Range: `0..`[`SKIP_CONTEXTS`].
#[must_use]
pub fn get_skip_context(above: Option<&MiInfo>, left: Option<&MiInfo>) -> usize {
    let above_skip = usize::from(above.is_some_and(|m| m.skip));
    let left_skip = usize::from(left.is_some_and(|m| m.skip));
    above_skip + left_skip
}

/// libvpx `get_tx_size_context` (`vp9_pred_common.h:156-171`).
///
/// Context for the selected-transform-size symbol. Unlike the other
/// neighbour contexts this one also needs the *current* block's `sb_type`,
/// because the comparison is against `max_txsize_lookup[sb_type]`.
///
/// A skipped neighbour is treated as absent (it carries no meaningful
/// `tx_size`), and when one side is missing it inherits the other's context —
/// which is why "no neighbours at all" yields `1` for every `sb_type` with a
/// non-zero maximum transform size.
///
/// libvpx only reaches this function for `bsize >= BLOCK_8X8`
/// (`vp9_decodemv.c:85`), i.e. `sb_type >= 3` and `max_tx_size >= 1`.
///
/// Returns `0` or `1`.
#[must_use]
pub fn get_tx_size_context(above: Option<&MiInfo>, left: Option<&MiInfo>, sb_type: u8) -> usize {
    let max_tx_size = tables::MAX_TXSIZE_LOOKUP[sb_type as usize];
    let mut above_ctx = above.filter(|m| !m.skip).map_or(max_tx_size, |m| m.tx_size);
    let mut left_ctx = left.filter(|m| !m.skip).map_or(max_tx_size, |m| m.tx_size);
    if left.is_none() {
        left_ctx = above_ctx;
    }
    if above.is_none() {
        above_ctx = left_ctx;
    }
    usize::from(above_ctx + left_ctx > max_tx_size)
}

/// libvpx `get_segment_id` (`vp9_pred_common.h:22-39`).
///
/// The segment id a block inherits from a segment map: the *minimum* over
/// every mode-info cell the block covers, clipped to the frame
/// (`VPXMIN(cm->mi_cols - mi_col, bw)`).
///
/// libvpx's decoder-side twin `dec_get_segment_id`
/// (`vp9/decoder/vp9_decodemv.c:91-102`) is the same fold with `INT_MAX`
/// rather than `MAX_SEGMENTS` as the seed; for any block the two agree,
/// because a block always covers at least one cell and every stored id is
/// below [`MAX_SEGMENTS`].
///
/// # Preconditions
///
/// `mi_row < mi_rows`, `mi_col < mi_cols`, `segment_ids.len() >=
/// mi_rows * mi_cols`, and `bsize` a valid VP9 `BLOCK_SIZE` (`0..=12`). Under
/// those, the result is always a valid segment id (`< MAX_SEGMENTS`). A
/// violated precondition yields the out-of-range sentinel [`MAX_SEGMENTS`]
/// rather than a panic or a fabricated id, and trips a `debug_assert!`.
#[must_use]
pub fn get_segment_id(
    segment_ids: &[u8],
    mi_rows: usize,
    mi_cols: usize,
    bsize: u8,
    mi_row: usize,
    mi_col: usize,
) -> u8 {
    debug_assert!(
        mi_row < mi_rows && mi_col < mi_cols,
        "get_segment_id: ({mi_row},{mi_col}) outside {mi_rows}x{mi_cols} mi grid"
    );
    debug_assert!(
        segment_ids.len() >= mi_rows * mi_cols,
        "get_segment_id: segment map is {} cells, needs {}",
        segment_ids.len(),
        mi_rows * mi_cols
    );

    let bw = usize::from(tables::NUM_8X8_BLOCKS_WIDE[bsize as usize]);
    let bh = usize::from(tables::NUM_8X8_BLOCKS_HIGH[bsize as usize]);
    let xmis = mi_cols.saturating_sub(mi_col).min(bw);
    let ymis = mi_rows.saturating_sub(mi_row).min(bh);
    let mi_offset = mi_row * mi_cols + mi_col;

    let mut segment_id = MAX_SEGMENTS;
    for y in 0..ymis {
        for x in 0..xmis {
            if let Some(&id) = segment_ids.get(mi_offset + y * mi_cols + x) {
                segment_id = segment_id.min(id);
            }
        }
    }

    debug_assert!(
        segment_id < MAX_SEGMENTS,
        "get_segment_id: no in-range cell sampled at ({mi_row},{mi_col}) bsize {bsize}"
    );
    segment_id
}

#[cfg(test)]
mod tests {
    use super::super::refs::NONE_FRAME;
    use super::*;

    // ---------------------------------------------------------------------
    // Neighbour-state enumeration
    //
    // Nine states per side: no neighbour, intra, the three single references,
    // and four compound pairs. The compound set is deliberately over-provided
    // — see `ref_state` for why four distinct pairs are the minimum that
    // leaves no permanently-false comparison in the ported branch trees.
    // ---------------------------------------------------------------------

    const S_NONE: usize = 0;
    const S_INTRA: usize = 1;
    const S_LAST: usize = 2;
    const S_GOLDEN: usize = 3;
    const S_ALTREF: usize = 4;
    const S_COMP_LA: usize = 5;
    const S_COMP_GA: usize = 6;
    const S_COMP_LG: usize = 7;
    const S_COMP_GL: usize = 8;
    const N_REF_STATES: usize = 9;

    /// `ref_frame` pair per state (`None` = no neighbour at all).
    ///
    /// The four compound states are not redundant. Two of them exist so the
    /// `comp/comp` tails have both arms (see `N_REF_STATES` note above); the
    /// other two exist so the *second* reference is not always
    /// [`ALTREF_FRAME`]. Without `S_COMP_LG` / `S_COMP_GL`, every
    /// `ref_frame[1] == LAST_FRAME` test (`vp9_pred_common.c:189`, `:200`,
    /// `:209`, `:223`) and every `ref_frame[1] == GOLDEN_FRAME` test
    /// (`:259`, `:273`, `:282`, `:284`, `:286`, `:310`) would be
    /// permanently false, and a port that dropped one of those disjuncts
    /// would still pass every case.
    ///
    /// Both are bitstream-reachable: `[LAST, GOLDEN]` is what a compound
    /// block carries when `comp_fixed_ref == LAST` with
    /// `fix_ref_idx == 0` (sign bias `[_, false, true, true]`), and
    /// `[GOLDEN, LAST]` when `comp_fixed_ref == GOLDEN` with
    /// `fix_ref_idx == 0` (sign bias `[_, true, false, true]`).
    fn ref_state(state: usize) -> Option<[i8; 2]> {
        match state {
            S_NONE => None,
            S_INTRA => Some([INTRA_FRAME, NONE_FRAME]),
            S_LAST => Some([LAST_FRAME, NONE_FRAME]),
            S_GOLDEN => Some([GOLDEN_FRAME, NONE_FRAME]),
            S_ALTREF => Some([ALTREF_FRAME, NONE_FRAME]),
            S_COMP_LA => Some([LAST_FRAME, ALTREF_FRAME]),
            S_COMP_GA => Some([GOLDEN_FRAME, ALTREF_FRAME]),
            S_COMP_LG => Some([LAST_FRAME, GOLDEN_FRAME]),
            S_COMP_GL => Some([GOLDEN_FRAME, LAST_FRAME]),
            _ => unreachable!("ref_state: {state}"),
        }
    }

    fn ref_state_name(state: usize) -> &'static str {
        match state {
            S_NONE => "none",
            S_INTRA => "intra",
            S_LAST => "single LAST",
            S_GOLDEN => "single GOLDEN",
            S_ALTREF => "single ALTREF",
            S_COMP_LA => "compound LAST+ALTREF",
            S_COMP_GA => "compound GOLDEN+ALTREF",
            S_COMP_LG => "compound LAST+GOLDEN",
            S_COMP_GL => "compound GOLDEN+LAST",
            _ => unreachable!("ref_state_name: {state}"),
        }
    }

    /// A mode-info block with the given reference pair. `is_inter` is set
    /// consistently here even though the module deliberately ignores it — see
    /// `contexts_ignore_the_is_inter_cache`.
    fn mi_with_refs(ref_frame: [i8; 2]) -> MiInfo {
        MiInfo {
            ref_frame,
            is_inter: ref_frame[0] > INTRA_FRAME,
            ..MiInfo::default()
        }
    }

    /// Runs `f` over the full 9x9 neighbour grid and compares against a flat
    /// literal table — the output of the compiled libvpx reference bodies over
    /// this same enumeration, not a second reading of the branch trees.
    fn check_ref_grid<F>(expected: &[[usize; N_REF_STATES]; N_REF_STATES], label: &str, f: F)
    where
        F: Fn(Option<&MiInfo>, Option<&MiInfo>) -> usize,
    {
        for above_state in 0..N_REF_STATES {
            for left_state in 0..N_REF_STATES {
                let above = ref_state(above_state).map(mi_with_refs);
                let left = ref_state(left_state).map(mi_with_refs);
                let got = f(above.as_ref(), left.as_ref());
                assert_eq!(
                    got,
                    expected[above_state][left_state],
                    "{label}: above={}, left={}",
                    ref_state_name(above_state),
                    ref_state_name(left_state)
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // get_intra_inter_context — vp9_pred_common.h:97-111
    // ---------------------------------------------------------------------

    /// Oracle transcribed from libvpx's own summary of the mapping,
    /// `vp9_pred_common.h:93-96`:
    ///   0 - inter/inter, inter/--, --/inter, --/--
    ///   1 - intra/inter, inter/intra
    ///   2 - intra/--, --/intra
    ///   3 - intra/intra
    /// Rows are `above`, columns `left`, in `S_*` order.
    #[rustfmt::skip]
    const INTRA_INTER_EXPECTED: [[usize; N_REF_STATES]; N_REF_STATES] = [
        //         none intra last gold altr cLA  cGA  cLG  cGL
        /* none */ [ 0,   2,   0,   0,   0,   0,   0,   0,   0 ],
        /* intra*/ [ 2,   3,   1,   1,   1,   1,   1,   1,   1 ],
        /* last */ [ 0,   1,   0,   0,   0,   0,   0,   0,   0 ],
        /* gold */ [ 0,   1,   0,   0,   0,   0,   0,   0,   0 ],
        /* altr */ [ 0,   1,   0,   0,   0,   0,   0,   0,   0 ],
        /* cLA  */ [ 0,   1,   0,   0,   0,   0,   0,   0,   0 ],
        /* cGA  */ [ 0,   1,   0,   0,   0,   0,   0,   0,   0 ],
        /* cLG  */ [ 0,   1,   0,   0,   0,   0,   0,   0,   0 ],
        /* cGL  */ [ 0,   1,   0,   0,   0,   0,   0,   0,   0 ],
    ];

    #[test]
    fn intra_inter_context_matches_libvpx() {
        check_ref_grid(&INTRA_INTER_EXPECTED, "intra_inter", |a, l| {
            get_intra_inter_context(a, l)
        });
    }

    #[test]
    fn intra_inter_context_stays_in_range() {
        for above_state in 0..N_REF_STATES {
            for left_state in 0..N_REF_STATES {
                let above = ref_state(above_state).map(mi_with_refs);
                let left = ref_state(left_state).map(mi_with_refs);
                assert!(
                    get_intra_inter_context(above.as_ref(), left.as_ref()) < INTRA_INTER_CONTEXTS
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // get_reference_mode_context — vp9_pred_common.c:42-82
    // ---------------------------------------------------------------------

    /// `comp_fixed_ref = ALTREF_FRAME` (the sign-bias case
    /// `sign_bias[LAST] == sign_bias[GOLDEN]`, vp9_pred_common.c:25-29).
    #[rustfmt::skip]
    const REF_MODE_EXPECTED_FIXED_ALTREF: [[usize; N_REF_STATES]; N_REF_STATES] = [
        //         none intra last gold altr cLA  cGA  cLG  cGL
        /* none */ [ 1,   0,   0,   0,   1,   3,   3,   3,   3 ],
        /* intra*/ [ 0,   0,   0,   0,   1,   3,   3,   3,   3 ],
        /* last */ [ 0,   0,   0,   0,   1,   2,   2,   2,   2 ],
        /* gold */ [ 0,   0,   0,   0,   1,   2,   2,   2,   2 ],
        /* altr */ [ 1,   1,   1,   1,   0,   3,   3,   3,   3 ],
        /* cLA  */ [ 3,   3,   2,   2,   3,   4,   4,   4,   4 ],
        /* cGA  */ [ 3,   3,   2,   2,   3,   4,   4,   4,   4 ],
        /* cLG  */ [ 3,   3,   2,   2,   3,   4,   4,   4,   4 ],
        /* cGL  */ [ 3,   3,   2,   2,   3,   4,   4,   4,   4 ],
    ];

    /// `comp_fixed_ref = LAST_FRAME` (the fall-through case
    /// vp9_pred_common.c:35-39). Differs from the ALTREF table wherever the
    /// `ref_frame[0] == comp_fixed_ref` test flips, so a hard-coded fixed
    /// reference cannot satisfy both.
    #[rustfmt::skip]
    const REF_MODE_EXPECTED_FIXED_LAST: [[usize; N_REF_STATES]; N_REF_STATES] = [
        //         none intra last gold altr cLA  cGA  cLG  cGL
        /* none */ [ 1,   0,   1,   0,   0,   3,   3,   3,   3 ],
        /* intra*/ [ 0,   0,   1,   0,   0,   3,   3,   3,   3 ],
        /* last */ [ 1,   1,   0,   1,   1,   3,   3,   3,   3 ],
        /* gold */ [ 0,   0,   1,   0,   0,   2,   2,   2,   2 ],
        /* altr */ [ 0,   0,   1,   0,   0,   2,   2,   2,   2 ],
        /* cLA  */ [ 3,   3,   3,   2,   2,   4,   4,   4,   4 ],
        /* cGA  */ [ 3,   3,   3,   2,   2,   4,   4,   4,   4 ],
        /* cLG  */ [ 3,   3,   3,   2,   2,   4,   4,   4,   4 ],
        /* cGL  */ [ 3,   3,   3,   2,   2,   4,   4,   4,   4 ],
    ];

    #[test]
    fn reference_mode_context_matches_libvpx_fixed_altref() {
        check_ref_grid(
            &REF_MODE_EXPECTED_FIXED_ALTREF,
            "ref_mode/ALTREF",
            |a, l| get_reference_mode_context(a, l, ALTREF_FRAME),
        );
    }

    #[test]
    fn reference_mode_context_matches_libvpx_fixed_last() {
        check_ref_grid(&REF_MODE_EXPECTED_FIXED_LAST, "ref_mode/LAST", |a, l| {
            get_reference_mode_context(a, l, LAST_FRAME)
        });
    }

    #[test]
    fn reference_mode_context_stays_in_range() {
        for fixed in [LAST_FRAME, GOLDEN_FRAME, ALTREF_FRAME] {
            for above_state in 0..N_REF_STATES {
                for left_state in 0..N_REF_STATES {
                    let above = ref_state(above_state).map(mi_with_refs);
                    let left = ref_state(left_state).map(mi_with_refs);
                    assert!(
                        get_reference_mode_context(above.as_ref(), left.as_ref(), fixed)
                            < COMP_INTER_CONTEXTS
                    );
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // get_pred_context_comp_ref_p — vp9_pred_common.c:85-165
    // ---------------------------------------------------------------------

    /// Sign biases `[intra, LAST, GOLDEN, ALTREF] = [_, false, false, true]`:
    /// `sign_bias[LAST] == sign_bias[GOLDEN]` so
    /// `comp_fixed_ref = ALTREF`, `comp_var_ref = [LAST, GOLDEN]`,
    /// `fix_ref_idx = sign_bias[ALTREF] = 1`, hence `var_ref_idx = 0`.
    /// `S_COMP_LA` / `S_COMP_GA` are the bitstream-realistic compound blocks
    /// under this setup: `ref_frame[1]` is the fixed ALTREF, `ref_frame[0]`
    /// the variable one.
    const SIGN_BIAS_FIXED_ALTREF: [bool; 4] = [false, false, false, true];

    /// Sign biases `[_, false, true, true]`: LAST agrees with neither GOLDEN
    /// nor ALTREF, so `comp_fixed_ref = LAST`, `comp_var_ref = [GOLDEN,
    /// ALTREF]`, `fix_ref_idx = sign_bias[LAST] = 0`, hence `var_ref_idx = 1`
    /// — the *opposite* slot from the table above, and `S_COMP_LG` is the
    /// realistic compound block here.
    ///
    /// The cell that discriminates the two slots is `S_COMP_LA` =
    /// `[LAST, ALTREF]` as a lone left edge, where the arm is
    /// `4 * (ref_frame[var_ref_idx] != comp_var_ref[1])`: slot 1 gives
    /// `4 * (ALTREF != ALTREF)` = **0**, slot 0 gives
    /// `4 * (LAST != ALTREF)` = **4**. See
    /// `comp_ref_p_lone_compound_edge_reads_the_variable_slot`.
    const SIGN_BIAS_FIXED_LAST: [bool; 4] = [false, false, true, true];

    #[rustfmt::skip]
    const COMP_REF_EXPECTED_VAR_IDX0: [[usize; N_REF_STATES]; N_REF_STATES] = [
        //         none intra last gold altr cLA  cGA  cLG  cGL
        /* none */ [ 2,   2,   3,   0,   3,   4,   0,   4,   0 ],
        /* intra*/ [ 2,   2,   3,   1,   3,   3,   1,   3,   1 ],
        /* last */ [ 3,   3,   3,   1,   4,   4,   1,   4,   1 ],
        /* gold */ [ 0,   1,   1,   0,   1,   2,   0,   2,   0 ],
        /* altr */ [ 3,   3,   4,   1,   3,   4,   1,   4,   1 ],
        /* cLA  */ [ 4,   3,   4,   2,   4,   4,   2,   4,   2 ],
        /* cGA  */ [ 0,   1,   1,   0,   1,   2,   0,   2,   0 ],
        /* cLG  */ [ 4,   3,   4,   2,   4,   4,   2,   4,   2 ],
        /* cGL  */ [ 0,   1,   1,   0,   1,   2,   0,   2,   0 ],
    ];

    #[rustfmt::skip]
    const COMP_REF_EXPECTED_VAR_IDX1: [[usize; N_REF_STATES]; N_REF_STATES] = [
        //         none intra last gold altr cLA  cGA  cLG  cGL
        /* none */ [ 2,   2,   3,   3,   0,   0,   0,   4,   4 ],
        /* intra*/ [ 2,   2,   3,   3,   1,   1,   1,   3,   3 ],
        /* last */ [ 3,   3,   3,   4,   1,   1,   1,   4,   4 ],
        /* gold */ [ 3,   3,   4,   3,   1,   1,   1,   4,   4 ],
        /* altr */ [ 0,   1,   1,   1,   0,   0,   0,   2,   2 ],
        /* cLA  */ [ 0,   1,   1,   1,   0,   0,   0,   2,   2 ],
        /* cGA  */ [ 0,   1,   1,   1,   0,   0,   0,   2,   2 ],
        /* cLG  */ [ 4,   3,   4,   4,   2,   2,   2,   4,   2 ],
        /* cGL  */ [ 4,   3,   4,   4,   2,   2,   2,   2,   4 ],
    ];

    #[test]
    fn comp_ref_p_context_matches_libvpx_var_ref_idx0() {
        let comp = CompRefState::from_sign_bias(&SIGN_BIAS_FIXED_ALTREF);
        assert_eq!(comp.comp_fixed_ref, ALTREF_FRAME);
        assert_eq!(comp.comp_var_ref, [LAST_FRAME, GOLDEN_FRAME]);
        assert_eq!(comp.fix_ref_idx(), 1);
        assert_eq!(comp.var_ref_idx(), 0);
        check_ref_grid(&COMP_REF_EXPECTED_VAR_IDX0, "comp_ref_p/var0", |a, l| {
            get_pred_context_comp_ref_p(a, l, &comp)
        });
    }

    #[test]
    fn comp_ref_p_context_matches_libvpx_var_ref_idx1() {
        let comp = CompRefState::from_sign_bias(&SIGN_BIAS_FIXED_LAST);
        assert_eq!(comp.comp_fixed_ref, LAST_FRAME);
        assert_eq!(comp.comp_var_ref, [GOLDEN_FRAME, ALTREF_FRAME]);
        assert_eq!(comp.fix_ref_idx(), 0);
        assert_eq!(comp.var_ref_idx(), 1);
        check_ref_grid(&COMP_REF_EXPECTED_VAR_IDX1, "comp_ref_p/var1", |a, l| {
            get_pred_context_comp_ref_p(a, l, &comp)
        });
    }

    /// `comp_var_ref[0]` appears exactly once in the whole of
    /// vp9_pred_common (`vp9_pred_common.c:125-126`); every other comparison
    /// uses `comp_var_ref[1]`. This pins the two single/single pairings that
    /// reach it, so the slot cannot be normalised away.
    #[test]
    fn comp_ref_p_uses_comp_var_ref_zero_in_single_single_branch() {
        let comp = CompRefState::from_sign_bias(&SIGN_BIAS_FIXED_ALTREF);
        // comp_fixed_ref = ALTREF, comp_var_ref[0] = LAST.
        let altref = mi_with_refs([ALTREF_FRAME, NONE_FRAME]);
        let last = mi_with_refs([LAST_FRAME, NONE_FRAME]);
        let golden = mi_with_refs([GOLDEN_FRAME, NONE_FRAME]);

        // (fixed, var_ref[0]) in either order -> 4.
        assert_eq!(
            get_pred_context_comp_ref_p(Some(&altref), Some(&last), &comp),
            4
        );
        assert_eq!(
            get_pred_context_comp_ref_p(Some(&last), Some(&altref), &comp),
            4
        );
        // (fixed, var_ref[1]) is a different, non-matching pairing -> 1.
        assert_eq!(
            get_pred_context_comp_ref_p(Some(&altref), Some(&golden), &comp),
            1
        );
    }

    /// The `comp/comp` tail (`vp9_pred_common.c:141-145`) has two arms:
    /// equal variable references -> 4, differing -> 2. Both need two distinct
    /// compound neighbours to be reachable.
    #[test]
    fn comp_ref_p_comp_comp_tail_has_both_arms() {
        let comp = CompRefState::from_sign_bias(&SIGN_BIAS_FIXED_ALTREF);
        let comp_la = mi_with_refs([LAST_FRAME, ALTREF_FRAME]);
        let comp_ga = mi_with_refs([GOLDEN_FRAME, ALTREF_FRAME]);
        // var refs both LAST (equal, and != comp_var_ref[1] = GOLDEN) -> 4.
        assert_eq!(
            get_pred_context_comp_ref_p(Some(&comp_la), Some(&comp_la), &comp),
            4
        );
        // var refs LAST vs GOLDEN (differing) -> 2, in both orders.
        assert_eq!(
            get_pred_context_comp_ref_p(Some(&comp_la), Some(&comp_ga), &comp),
            2
        );
        assert_eq!(
            get_pred_context_comp_ref_p(Some(&comp_ga), Some(&comp_la), &comp),
            2
        );
    }

    /// A lone compound edge is scored on `ref_frame[var_ref_idx]`
    /// (`vp9_pred_common.c:154-155`). `S_COMP_LA` = `[LAST, ALTREF]` reads
    /// LAST from slot 0 and ALTREF from slot 1, and `comp_var_ref[1]` is
    /// GOLDEN in one setup and ALTREF in the other — so an inverted
    /// `var_ref_idx` flips this cell.
    #[test]
    fn comp_ref_p_lone_compound_edge_reads_the_variable_slot() {
        let comp_la = mi_with_refs([LAST_FRAME, ALTREF_FRAME]);

        // var_ref_idx = 0, comp_var_ref[1] = GOLDEN: 4 * (LAST != GOLDEN) = 4.
        let fixed_altref = CompRefState::from_sign_bias(&SIGN_BIAS_FIXED_ALTREF);
        assert_eq!(fixed_altref.var_ref_idx(), 0);
        assert_eq!(
            get_pred_context_comp_ref_p(None, Some(&comp_la), &fixed_altref),
            4
        );

        // var_ref_idx = 1, comp_var_ref[1] = ALTREF: 4 * (ALTREF != ALTREF) = 0.
        let fixed_last = CompRefState::from_sign_bias(&SIGN_BIAS_FIXED_LAST);
        assert_eq!(fixed_last.var_ref_idx(), 1);
        assert_eq!(
            get_pred_context_comp_ref_p(None, Some(&comp_la), &fixed_last),
            0
        );
    }

    #[test]
    fn comp_ref_p_context_stays_in_range() {
        for sign_bias in [SIGN_BIAS_FIXED_ALTREF, SIGN_BIAS_FIXED_LAST] {
            let comp = CompRefState::from_sign_bias(&sign_bias);
            for above_state in 0..N_REF_STATES {
                for left_state in 0..N_REF_STATES {
                    let above = ref_state(above_state).map(mi_with_refs);
                    let left = ref_state(left_state).map(mi_with_refs);
                    assert!(
                        get_pred_context_comp_ref_p(above.as_ref(), left.as_ref(), &comp)
                            < REF_CONTEXTS
                    );
                }
            }
        }
    }

    /// Every sign-bias combination reachable from
    /// [`setup_compound_reference_mode`] must give a `fix_ref_idx` that is
    /// literally `sign_bias[comp_fixed_ref]`, and a `var_ref_idx` that is its
    /// complement (`vp9_pred_common.c:97-98`).
    #[test]
    fn fix_and_var_ref_idx_are_complementary_for_every_sign_bias() {
        for last in [false, true] {
            for golden in [false, true] {
                for altref in [false, true] {
                    let sign_bias = [false, last, golden, altref];
                    let comp = CompRefState::from_sign_bias(&sign_bias);
                    let expected_fix = usize::from(sign_bias[comp.comp_fixed_ref as usize]);
                    assert_eq!(comp.fix_ref_idx(), expected_fix, "sign_bias {sign_bias:?}");
                    assert_eq!(
                        comp.var_ref_idx(),
                        1 - expected_fix,
                        "sign_bias {sign_bias:?}"
                    );
                    assert_ne!(comp.fix_ref_idx(), comp.var_ref_idx());
                }
            }
        }
    }

    // ---------------------------------------------------------------------
    // get_pred_context_single_ref_p1 — vp9_pred_common.c:167-231
    // ---------------------------------------------------------------------

    #[rustfmt::skip]
    const SINGLE_REF_P1_EXPECTED: [[usize; N_REF_STATES]; N_REF_STATES] = [
        //         none intra last gold altr cLA  cGA  cLG  cGL
        /* none */ [ 2,   2,   4,   0,   0,   2,   1,   2,   2 ],
        /* intra*/ [ 2,   2,   4,   0,   0,   2,   1,   2,   2 ],
        /* last */ [ 4,   4,   4,   2,   2,   4,   3,   4,   4 ],
        /* gold */ [ 0,   0,   2,   0,   0,   1,   0,   1,   1 ],
        /* altr */ [ 0,   0,   2,   0,   0,   1,   0,   1,   1 ],
        /* cLA  */ [ 2,   2,   4,   1,   1,   2,   2,   2,   2 ],
        /* cGA  */ [ 1,   1,   3,   0,   0,   2,   1,   2,   2 ],
        /* cLG  */ [ 2,   2,   4,   1,   1,   2,   2,   2,   2 ],
        /* cGL  */ [ 2,   2,   4,   1,   1,   2,   2,   2,   2 ],
    ];

    #[test]
    fn single_ref_p1_context_matches_libvpx() {
        check_ref_grid(&SINGLE_REF_P1_EXPECTED, "single_ref_p1", |a, l| {
            get_pred_context_single_ref_p1(a, l)
        });
    }

    /// `single_ref_p1` tests *both* slots of a compound neighbour for
    /// [`LAST_FRAME`] (`vp9_pred_common.c:189`, `:200`, `:207`, `:209`,
    /// `:223`). Two compound pairs that differ only in whether LAST sits in
    /// slot 1 must therefore score differently — a port that dropped the
    /// `ref_frame[1]` disjunct would collapse them.
    #[test]
    fn single_ref_p1_reads_the_second_reference_slot() {
        let comp_ga = mi_with_refs([GOLDEN_FRAME, ALTREF_FRAME]); // no LAST
        let comp_gl = mi_with_refs([GOLDEN_FRAME, LAST_FRAME]); // LAST in slot 1
                                                                // Lone edge: 1 + (rf0 == LAST || rf1 == LAST).
        assert_eq!(get_pred_context_single_ref_p1(None, Some(&comp_ga)), 1);
        assert_eq!(get_pred_context_single_ref_p1(None, Some(&comp_gl)), 2);
        // Compound beside a single non-LAST edge: (crf1 == LAST || crf2 == LAST).
        let golden = mi_with_refs([GOLDEN_FRAME, NONE_FRAME]);
        assert_eq!(
            get_pred_context_single_ref_p1(Some(&comp_ga), Some(&golden)),
            0
        );
        assert_eq!(
            get_pred_context_single_ref_p1(Some(&comp_gl), Some(&golden)),
            1
        );
    }

    #[test]
    fn single_ref_p1_context_stays_in_range() {
        for above_state in 0..N_REF_STATES {
            for left_state in 0..N_REF_STATES {
                let above = ref_state(above_state).map(mi_with_refs);
                let left = ref_state(left_state).map(mi_with_refs);
                assert!(
                    get_pred_context_single_ref_p1(above.as_ref(), left.as_ref()) < REF_CONTEXTS
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // get_pred_context_single_ref_p2 — vp9_pred_common.c:233-316
    // ---------------------------------------------------------------------

    #[rustfmt::skip]
    const SINGLE_REF_P2_EXPECTED: [[usize; N_REF_STATES]; N_REF_STATES] = [
        //         none intra last gold altr cLA  cGA  cLG  cGL
        /* none */ [ 2,   2,   2,   4,   0,   0,   3,   3,   3 ],
        /* intra*/ [ 2,   2,   3,   4,   0,   1,   3,   3,   3 ],
        /* last */ [ 2,   3,   3,   4,   0,   1,   3,   3,   3 ],
        /* gold */ [ 4,   4,   4,   4,   2,   3,   4,   4,   4 ],
        /* altr */ [ 0,   0,   0,   2,   0,   0,   1,   1,   1 ],
        /* cLA  */ [ 0,   1,   1,   3,   0,   0,   2,   2,   2 ],
        /* cGA  */ [ 3,   3,   3,   4,   1,   2,   3,   2,   2 ],
        /* cLG  */ [ 3,   3,   3,   4,   1,   2,   2,   3,   2 ],
        /* cGL  */ [ 3,   3,   3,   4,   1,   2,   2,   2,   3 ],
    ];

    #[test]
    fn single_ref_p2_context_matches_libvpx() {
        check_ref_grid(&SINGLE_REF_P2_EXPECTED, "single_ref_p2", |a, l| {
            get_pred_context_single_ref_p2(a, l)
        });
    }

    /// The compound/compound branch (`vp9_pred_common.c:269-275`) splits on
    /// "identical reference pairs", and the identical case then splits again
    /// on whether GOLDEN is present. All three outcomes must be reachable.
    #[test]
    fn single_ref_p2_comp_comp_branch_covers_all_outcomes() {
        let comp_la = mi_with_refs([LAST_FRAME, ALTREF_FRAME]);
        let comp_ga = mi_with_refs([GOLDEN_FRAME, ALTREF_FRAME]);
        // identical pairs, no GOLDEN -> 3 * 0.
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_la), Some(&comp_la)),
            0
        );
        // identical pairs, GOLDEN present -> 3 * 1.
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_ga), Some(&comp_ga)),
            3
        );
        // differing pairs -> 2, in both orders.
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_la), Some(&comp_ga)),
            2
        );
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_ga), Some(&comp_la)),
            2
        );
    }

    /// The lone-edge branch (`vp9_pred_common.c:300-310`) is *not* the same
    /// expression as the intra/inter branch (`vp9_pred_common.c:250-260`):
    /// a single-LAST edge gives 2 alone but 3 beside an intra neighbour, and
    /// a compound edge is scaled by 3 alone but by 2 (offset 1) beside one.
    #[test]
    fn single_ref_p2_lone_edge_differs_from_intra_inter_edge() {
        let intra = mi_with_refs([INTRA_FRAME, NONE_FRAME]);
        let last = mi_with_refs([LAST_FRAME, NONE_FRAME]);
        let comp_ga = mi_with_refs([GOLDEN_FRAME, ALTREF_FRAME]);

        assert_eq!(get_pred_context_single_ref_p2(Some(&last), None), 2);
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&last), Some(&intra)),
            3,
            "single LAST beside an intra neighbour takes the 250-260 branch"
        );
        assert_eq!(get_pred_context_single_ref_p2(Some(&comp_ga), None), 3);
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_ga), Some(&intra)),
            3
        );
        let comp_la = mi_with_refs([LAST_FRAME, ALTREF_FRAME]);
        assert_eq!(get_pred_context_single_ref_p2(Some(&comp_la), None), 0);
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_la), Some(&intra)),
            1
        );
    }

    /// The `single_ref_p2` twin of the check above: both slots of a compound
    /// neighbour are tested for [`GOLDEN_FRAME`] (`vp9_pred_common.c:259`,
    /// `:272-273`, `:282`, `:284`, `:286`, `:310`).
    #[test]
    fn single_ref_p2_reads_the_second_reference_slot() {
        let comp_la = mi_with_refs([LAST_FRAME, ALTREF_FRAME]); // no GOLDEN
        let comp_lg = mi_with_refs([LAST_FRAME, GOLDEN_FRAME]); // GOLDEN in slot 1
                                                                // Lone edge: 3 * (rf0 == GOLDEN || rf1 == GOLDEN).
        assert_eq!(get_pred_context_single_ref_p2(None, Some(&comp_la)), 0);
        assert_eq!(get_pred_context_single_ref_p2(None, Some(&comp_lg)), 3);
        // Beside an intra edge: 1 + 2 * (rf0 == GOLDEN || rf1 == GOLDEN).
        let intra = mi_with_refs([INTRA_FRAME, NONE_FRAME]);
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_la), Some(&intra)),
            1
        );
        assert_eq!(
            get_pred_context_single_ref_p2(Some(&comp_lg), Some(&intra)),
            3
        );
    }

    #[test]
    fn single_ref_p2_context_stays_in_range() {
        for above_state in 0..N_REF_STATES {
            for left_state in 0..N_REF_STATES {
                let above = ref_state(above_state).map(mi_with_refs);
                let left = ref_state(left_state).map(mi_with_refs);
                assert!(
                    get_pred_context_single_ref_p2(above.as_ref(), left.as_ref()) < REF_CONTEXTS
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // The is_inter cache is ignored
    // ---------------------------------------------------------------------

    /// libvpx derives `is_inter_block` from `ref_frame[0]`
    /// (`vp9_blockd.h:102-104`). A `MiInfo` whose `is_inter` cache disagrees
    /// with its `ref_frame` must still produce the `ref_frame`-derived
    /// context.
    #[test]
    fn contexts_ignore_the_is_inter_cache() {
        let comp = CompRefState::from_sign_bias(&SIGN_BIAS_FIXED_ALTREF);
        for pair in [
            [INTRA_FRAME, NONE_FRAME],
            [LAST_FRAME, NONE_FRAME],
            [GOLDEN_FRAME, ALTREF_FRAME],
        ] {
            let honest = mi_with_refs(pair);
            let lying = MiInfo {
                is_inter: !honest.is_inter,
                ..honest
            };
            assert_eq!(is_inter_block(&lying), is_inter_block(&honest));
            for other in [None, Some(mi_with_refs([LAST_FRAME, NONE_FRAME]))] {
                let o = other.as_ref();
                assert_eq!(
                    get_intra_inter_context(Some(&lying), o),
                    get_intra_inter_context(Some(&honest), o)
                );
                assert_eq!(
                    get_reference_mode_context(Some(&lying), o, ALTREF_FRAME),
                    get_reference_mode_context(Some(&honest), o, ALTREF_FRAME)
                );
                assert_eq!(
                    get_pred_context_comp_ref_p(Some(&lying), o, &comp),
                    get_pred_context_comp_ref_p(Some(&honest), o, &comp)
                );
                assert_eq!(
                    get_pred_context_single_ref_p1(Some(&lying), o),
                    get_pred_context_single_ref_p1(Some(&honest), o)
                );
                assert_eq!(
                    get_pred_context_single_ref_p2(Some(&lying), o),
                    get_pred_context_single_ref_p2(Some(&honest), o)
                );
            }
        }
    }

    // ---------------------------------------------------------------------
    // get_pred_context_switchable_interp — vp9_pred_common.h:69-88
    // ---------------------------------------------------------------------

    const N_FILTER_STATES: usize = 5;

    /// `interp_filter` per state: no neighbour, then filters 0..=3. Filter 3
    /// is `SWITCHABLE_FILTERS`, which is what an intra block inside an inter
    /// frame carries (`vp9_decodemv.c:383`).
    fn filter_state(state: usize) -> Option<u8> {
        match state {
            0 => None,
            1 => Some(0),
            2 => Some(1),
            3 => Some(2),
            4 => Some(SWITCHABLE_FILTERS),
            _ => unreachable!("filter_state: {state}"),
        }
    }

    fn mi_with_filter(interp_filter: u8) -> MiInfo {
        MiInfo {
            interp_filter,
            ..MiInfo::default()
        }
    }

    /// Rows are `above`, columns `left`: none / f0 / f1 / f2 / f3(=sentinel).
    /// Note rows 0 and 4 are identical, and so are columns 0 and 4: a missing
    /// neighbour and a sentinel-carrying neighbour are indistinguishable, which
    /// is precisely why libvpx can skip the intra check.
    #[rustfmt::skip]
    const SWITCHABLE_INTERP_EXPECTED: [[usize; N_FILTER_STATES]; N_FILTER_STATES] = [
        //       none  f0   f1   f2   f3
        /* none*/ [ 3,   0,   1,   2,   3 ],
        /* f0  */ [ 0,   0,   3,   3,   0 ],
        /* f1  */ [ 1,   3,   1,   3,   1 ],
        /* f2  */ [ 2,   3,   3,   2,   2 ],
        /* f3  */ [ 3,   0,   1,   2,   3 ],
    ];

    #[test]
    fn switchable_interp_context_matches_libvpx() {
        for above_state in 0..N_FILTER_STATES {
            for left_state in 0..N_FILTER_STATES {
                let above = filter_state(above_state).map(mi_with_filter);
                let left = filter_state(left_state).map(mi_with_filter);
                let got = get_pred_context_switchable_interp(above.as_ref(), left.as_ref());
                assert_eq!(
                    got, SWITCHABLE_INTERP_EXPECTED[above_state][left_state],
                    "switchable_interp: above state {above_state}, left state {left_state}"
                );
                assert!(got < SWITCHABLE_FILTER_CONTEXTS);
            }
        }
    }

    /// The sentinel is `SWITCHABLE_FILTERS` (3), not the frame-level
    /// `SWITCHABLE` enum value (4) — see the module-level hazard note. This
    /// pins the constant so the two cannot be confused silently.
    #[test]
    fn switchable_filters_sentinel_is_three() {
        assert_eq!(SWITCHABLE_FILTERS, 3);
        assert_eq!(
            SWITCHABLE_FILTER_CONTEXTS,
            usize::from(SWITCHABLE_FILTERS) + 1
        );
        let sentinel = mi_with_filter(SWITCHABLE_FILTERS);
        // A sentinel-carrying neighbour behaves exactly like no neighbour.
        for other_filter in [0, 1, 2, SWITCHABLE_FILTERS] {
            let other = mi_with_filter(other_filter);
            assert_eq!(
                get_pred_context_switchable_interp(Some(&sentinel), Some(&other)),
                get_pred_context_switchable_interp(None, Some(&other)),
                "sentinel above vs no above, other filter {other_filter}"
            );
            assert_eq!(
                get_pred_context_switchable_interp(Some(&other), Some(&sentinel)),
                get_pred_context_switchable_interp(Some(&other), None),
                "sentinel left vs no left, other filter {other_filter}"
            );
        }
    }

    // ---------------------------------------------------------------------
    // get_pred_context_seg_id / get_skip_context
    //   — vp9_pred_common.h:41-48 and :55-61
    // ---------------------------------------------------------------------

    /// Rows are `above`, columns `left`: no neighbour / flag clear / flag set.
    #[rustfmt::skip]
    const FLAG_SUM_EXPECTED: [[usize; 3]; 3] = [
        //        none clear set
        /* none */ [ 0,   0,   1 ],
        /* clear*/ [ 0,   0,   1 ],
        /* set  */ [ 1,   1,   2 ],
    ];

    fn flag_state(state: usize, set_flag: impl Fn(&mut MiInfo, bool)) -> Option<MiInfo> {
        match state {
            0 => None,
            1 | 2 => {
                let mut mi = MiInfo::default();
                set_flag(&mut mi, state == 2);
                Some(mi)
            }
            _ => unreachable!("flag_state: {state}"),
        }
    }

    #[test]
    fn seg_id_context_matches_libvpx() {
        for above_state in 0..3 {
            for left_state in 0..3 {
                let above = flag_state(above_state, |mi, v| mi.seg_id_predicted = v);
                let left = flag_state(left_state, |mi, v| mi.seg_id_predicted = v);
                let got = get_pred_context_seg_id(above.as_ref(), left.as_ref());
                assert_eq!(
                    got, FLAG_SUM_EXPECTED[above_state][left_state],
                    "seg_id: above state {above_state}, left state {left_state}"
                );
                assert!(got < PREDICTION_PROBS);
            }
        }
    }

    #[test]
    fn skip_context_matches_libvpx() {
        for above_state in 0..3 {
            for left_state in 0..3 {
                let above = flag_state(above_state, |mi, v| mi.skip = v);
                let left = flag_state(left_state, |mi, v| mi.skip = v);
                let got = get_skip_context(above.as_ref(), left.as_ref());
                assert_eq!(
                    got, FLAG_SUM_EXPECTED[above_state][left_state],
                    "skip: above state {above_state}, left state {left_state}"
                );
                assert!(got < SKIP_CONTEXTS);
            }
        }
    }

    /// `seg_id_predicted` and `skip` are independent inputs: neither context
    /// may read the other's field.
    #[test]
    fn seg_id_and_skip_contexts_do_not_share_a_field() {
        let skipped = MiInfo {
            skip: true,
            ..MiInfo::default()
        };
        let predicted = MiInfo {
            seg_id_predicted: true,
            ..MiInfo::default()
        };
        assert_eq!(get_skip_context(Some(&skipped), None), 1);
        assert_eq!(get_pred_context_seg_id(Some(&skipped), None), 0);
        assert_eq!(get_skip_context(Some(&predicted), None), 0);
        assert_eq!(get_pred_context_seg_id(Some(&predicted), None), 1);
    }

    // ---------------------------------------------------------------------
    // get_tx_size_context — vp9_pred_common.h:156-171
    // ---------------------------------------------------------------------

    const N_TX_STATES: usize = 6;

    /// `(skip, tx_size)` per state: no neighbour, a skipped neighbour (whose
    /// `tx_size` libvpx never consults), then a coded neighbour per
    /// `tx_size` 0..=3.
    fn tx_state(state: usize) -> Option<(bool, u8)> {
        match state {
            0 => None,
            1 => Some((true, 0)),
            2 => Some((false, 0)),
            3 => Some((false, 1)),
            4 => Some((false, 2)),
            5 => Some((false, 3)),
            _ => unreachable!("tx_state: {state}"),
        }
    }

    fn mi_with_tx(skip: bool, tx_size: u8) -> MiInfo {
        MiInfo {
            skip,
            tx_size,
            ..MiInfo::default()
        }
    }

    /// BLOCK_8X8 (`sb_type` 3), `max_txsize_lookup` = TX_8X8 = 1.
    /// Rows `above`, columns `left`: none / skip / tx0 / tx1 / tx2 / tx3.
    #[rustfmt::skip]
    const TX_CTX_EXPECTED_MAX1: [[usize; N_TX_STATES]; N_TX_STATES] = [
        //         none skip tx0  tx1  tx2  tx3
        /* none */ [ 1,   1,   0,   1,   1,   1 ],
        /* skip */ [ 1,   1,   0,   1,   1,   1 ],
        /* tx0  */ [ 0,   0,   0,   0,   1,   1 ],
        /* tx1  */ [ 1,   1,   0,   1,   1,   1 ],
        /* tx2  */ [ 1,   1,   1,   1,   1,   1 ],
        /* tx3  */ [ 1,   1,   1,   1,   1,   1 ],
    ];

    /// BLOCK_16X16 (`sb_type` 6), `max_txsize_lookup` = TX_16X16 = 2.
    #[rustfmt::skip]
    const TX_CTX_EXPECTED_MAX2: [[usize; N_TX_STATES]; N_TX_STATES] = [
        //         none skip tx0  tx1  tx2  tx3
        /* none */ [ 1,   1,   0,   0,   1,   1 ],
        /* skip */ [ 1,   1,   0,   1,   1,   1 ],
        /* tx0  */ [ 0,   0,   0,   0,   0,   1 ],
        /* tx1  */ [ 0,   1,   0,   0,   1,   1 ],
        /* tx2  */ [ 1,   1,   0,   1,   1,   1 ],
        /* tx3  */ [ 1,   1,   1,   1,   1,   1 ],
    ];

    /// BLOCK_32X32 (`sb_type` 9), `max_txsize_lookup` = TX_32X32 = 3.
    #[rustfmt::skip]
    const TX_CTX_EXPECTED_MAX3: [[usize; N_TX_STATES]; N_TX_STATES] = [
        //         none skip tx0  tx1  tx2  tx3
        /* none */ [ 1,   1,   0,   0,   1,   1 ],
        /* skip */ [ 1,   1,   0,   1,   1,   1 ],
        /* tx0  */ [ 0,   0,   0,   0,   0,   0 ],
        /* tx1  */ [ 0,   1,   0,   0,   0,   1 ],
        /* tx2  */ [ 1,   1,   0,   0,   1,   1 ],
        /* tx3  */ [ 1,   1,   0,   1,   1,   1 ],
    ];

    fn check_tx_grid(sb_type: u8, expected: &[[usize; N_TX_STATES]; N_TX_STATES]) {
        for above_state in 0..N_TX_STATES {
            for left_state in 0..N_TX_STATES {
                let above = tx_state(above_state).map(|(s, t)| mi_with_tx(s, t));
                let left = tx_state(left_state).map(|(s, t)| mi_with_tx(s, t));
                let got = get_tx_size_context(above.as_ref(), left.as_ref(), sb_type);
                assert_eq!(
                    got, expected[above_state][left_state],
                    "tx_size ctx sb_type {sb_type}: above state {above_state}, \
                     left state {left_state}"
                );
                assert!(got < 2);
            }
        }
    }

    #[test]
    fn tx_size_context_matches_libvpx_max_tx_8x8() {
        assert_eq!(tables::MAX_TXSIZE_LOOKUP[3], 1);
        check_tx_grid(3, &TX_CTX_EXPECTED_MAX1);
    }

    #[test]
    fn tx_size_context_matches_libvpx_max_tx_16x16() {
        assert_eq!(tables::MAX_TXSIZE_LOOKUP[6], 2);
        check_tx_grid(6, &TX_CTX_EXPECTED_MAX2);
    }

    #[test]
    fn tx_size_context_matches_libvpx_max_tx_32x32() {
        assert_eq!(tables::MAX_TXSIZE_LOOKUP[9], 3);
        check_tx_grid(9, &TX_CTX_EXPECTED_MAX3);
    }

    /// BLOCK_64X64 shares TX_32X32 as its maximum, so it must reproduce the
    /// BLOCK_32X32 grid exactly.
    #[test]
    fn tx_size_context_block_64x64_matches_block_32x32() {
        assert_eq!(tables::MAX_TXSIZE_LOOKUP[12], tables::MAX_TXSIZE_LOOKUP[9]);
        check_tx_grid(12, &TX_CTX_EXPECTED_MAX3);
    }

    /// A skipped neighbour contributes `max_tx_size`, so its own `tx_size` is
    /// never read (`vp9_pred_common.h:162-165`).
    #[test]
    fn tx_size_context_ignores_a_skipped_neighbours_tx_size() {
        for sb_type in [3u8, 6, 9, 12] {
            for tx_size in 0..=3u8 {
                let skipped = mi_with_tx(true, tx_size);
                let reference = mi_with_tx(true, 0);
                for other_state in 0..N_TX_STATES {
                    let other = tx_state(other_state).map(|(s, t)| mi_with_tx(s, t));
                    assert_eq!(
                        get_tx_size_context(Some(&skipped), other.as_ref(), sb_type),
                        get_tx_size_context(Some(&reference), other.as_ref(), sb_type),
                        "skipped above tx_size {tx_size}, sb_type {sb_type}, \
                         other state {other_state}"
                    );
                    assert_eq!(
                        get_tx_size_context(other.as_ref(), Some(&skipped), sb_type),
                        get_tx_size_context(other.as_ref(), Some(&reference), sb_type),
                        "skipped left tx_size {tx_size}, sb_type {sb_type}, \
                         other state {other_state}"
                    );
                }
            }
        }
    }

    /// With no neighbours at all both sides fall back to `max_tx_size`, so
    /// the sum is `2 * max_tx_size` and the context is 1 for every block size
    /// libvpx actually calls this with (`bsize >= BLOCK_8X8`).
    #[test]
    fn tx_size_context_with_no_neighbours_is_one() {
        for sb_type in 3..=12u8 {
            assert_eq!(
                get_tx_size_context(None, None, sb_type),
                1,
                "sb_type {sb_type}"
            );
        }
    }

    // ---------------------------------------------------------------------
    // get_segment_id — vp9_pred_common.h:22-39
    // ---------------------------------------------------------------------

    const SEG_MI_ROWS: usize = 3;
    const SEG_MI_COLS: usize = 4;
    /// A 3x4 segment map with no repeated minima along the tested blocks, so a
    /// wrong clamp or a transposed `bw`/`bh` changes the answer.
    #[rustfmt::skip]
    const SEG_MAP: [u8; SEG_MI_ROWS * SEG_MI_COLS] = [
        3, 1, 4, 1,
        5, 2, 6, 5,
        7, 0, 7, 7,
    ];

    fn seg_id_at(bsize: u8, mi_row: usize, mi_col: usize) -> u8 {
        get_segment_id(&SEG_MAP, SEG_MI_ROWS, SEG_MI_COLS, bsize, mi_row, mi_col)
    }

    /// [`SEG_MAP`] folded for every `BLOCK_SIZE` at every position, indexed
    /// `[bsize][mi_row][mi_col]`. Note rows 0..=2 (BLOCK_4X4/4X8/8X4) are the
    /// map itself — those sizes cover a single mode-info cell — and that from
    /// BLOCK_32X32 (9) up the whole 3x4 map is clamped in at the origin.
    #[rustfmt::skip]
    const SEG_ID_EXPECTED_ALL: [[[u8; SEG_MI_COLS]; SEG_MI_ROWS]; 13] = [
        /*  0 */ [[3, 1, 4, 1], [5, 2, 6, 5], [7, 0, 7, 7]],
        /*  1 */ [[3, 1, 4, 1], [5, 2, 6, 5], [7, 0, 7, 7]],
        /*  2 */ [[3, 1, 4, 1], [5, 2, 6, 5], [7, 0, 7, 7]],
        /*  3 */ [[3, 1, 4, 1], [5, 2, 6, 5], [7, 0, 7, 7]],
        /*  4 */ [[3, 1, 4, 1], [5, 0, 6, 5], [7, 0, 7, 7]],
        /*  5 */ [[1, 1, 1, 1], [2, 2, 5, 5], [0, 0, 7, 7]],
        /*  6 */ [[1, 1, 1, 1], [0, 0, 5, 5], [0, 0, 7, 7]],
        /*  7 */ [[0, 0, 1, 1], [0, 0, 5, 5], [0, 0, 7, 7]],
        /*  8 */ [[1, 1, 1, 1], [0, 0, 5, 5], [0, 0, 7, 7]],
        /*  9 */ [[0, 0, 1, 1], [0, 0, 5, 5], [0, 0, 7, 7]],
        /* 10 */ [[0, 0, 1, 1], [0, 0, 5, 5], [0, 0, 7, 7]],
        /* 11 */ [[0, 0, 1, 1], [0, 0, 5, 5], [0, 0, 7, 7]],
        /* 12 */ [[0, 0, 1, 1], [0, 0, 5, 5], [0, 0, 7, 7]],
    ];

    /// Exhaustive check of the fold + clamp over every block size and
    /// position. The tests below single out the individually interesting
    /// cases and say why they are interesting; this one leaves no gap.
    #[test]
    fn segment_id_matches_libvpx_for_every_block_size_and_position() {
        for (bsize, per_size) in SEG_ID_EXPECTED_ALL.iter().enumerate() {
            for (row, per_row) in per_size.iter().enumerate() {
                for (col, &expected) in per_row.iter().enumerate() {
                    assert_eq!(
                        seg_id_at(bsize as u8, row, col),
                        expected,
                        "bsize {bsize} at ({row},{col})"
                    );
                }
            }
        }
    }

    /// BLOCK_8X8 covers exactly one mode-info cell, so the fold is the identity.
    #[test]
    fn segment_id_block_8x8_reads_a_single_cell() {
        assert_eq!(tables::NUM_8X8_BLOCKS_WIDE[3], 1);
        assert_eq!(tables::NUM_8X8_BLOCKS_HIGH[3], 1);
        for row in 0..SEG_MI_ROWS {
            for col in 0..SEG_MI_COLS {
                assert_eq!(
                    seg_id_at(3, row, col),
                    SEG_MAP[row * SEG_MI_COLS + col],
                    "({row},{col})"
                );
            }
        }
    }

    /// BLOCK_16X16 covers a 2x2 cell square; the id is the minimum over it.
    #[test]
    fn segment_id_block_16x16_takes_the_minimum() {
        // cells (0,0)(0,1)(1,0)(1,1) = 3,1,5,2
        assert_eq!(seg_id_at(6, 0, 0), 1);
        // cells (0,2)(0,3)(1,2)(1,3) = 4,1,6,5
        assert_eq!(seg_id_at(6, 0, 2), 1);
        // cells (1,0)(1,1)(2,0)(2,1) = 5,2,7,0
        assert_eq!(seg_id_at(6, 1, 0), 0);
        // cells (1,2)(1,3)(2,2)(2,3) = 6,5,7,7
        assert_eq!(seg_id_at(6, 1, 2), 5);
    }

    /// `bw` and `bh` come from different tables; a transposed pair would read
    /// a different cell set for every non-square block size.
    #[test]
    fn segment_id_non_square_blocks_use_width_and_height_separately() {
        // BLOCK_16X8: 2 cells wide, 1 high. At (1,0): cells (1,0)(1,1) = 5,2.
        assert_eq!(tables::NUM_8X8_BLOCKS_WIDE[5], 2);
        assert_eq!(tables::NUM_8X8_BLOCKS_HIGH[5], 1);
        assert_eq!(seg_id_at(5, 1, 0), 2);
        // BLOCK_8X16: 1 cell wide, 2 high. At (1,0): cells (1,0)(2,0) = 5,7.
        assert_eq!(tables::NUM_8X8_BLOCKS_WIDE[4], 1);
        assert_eq!(tables::NUM_8X8_BLOCKS_HIGH[4], 2);
        assert_eq!(seg_id_at(4, 1, 0), 5);
        // BLOCK_32X16 at (0,0): 4 wide, 2 high -> rows 0-1 entirely = min 1.
        assert_eq!(tables::NUM_8X8_BLOCKS_WIDE[8], 4);
        assert_eq!(tables::NUM_8X8_BLOCKS_HIGH[8], 2);
        assert_eq!(seg_id_at(8, 0, 0), 1);
        // BLOCK_16X32 at (0,0): 2 wide, 4 high -> clamped to 3 rows,
        // columns 0-1 = 3,1,5,2,7,0 -> min 0.
        assert_eq!(tables::NUM_8X8_BLOCKS_WIDE[7], 2);
        assert_eq!(tables::NUM_8X8_BLOCKS_HIGH[7], 4);
        assert_eq!(seg_id_at(7, 0, 0), 0);
    }

    /// `VPXMIN(cm->mi_cols - mi_col, bw)` and its row twin: a block hanging
    /// off the right or bottom edge only folds the cells that exist.
    #[test]
    fn segment_id_clamps_to_the_frame_edges() {
        // BLOCK_64X64 at the origin: 8x8 cells clamped to the whole 3x4 map.
        assert_eq!(seg_id_at(12, 0, 0), 0);
        // BLOCK_16X16 at (2,3): clamped to the single cell (2,3) = 7.
        assert_eq!(seg_id_at(6, 2, 3), 7);
        // BLOCK_16X16 at (0,3): clamped to one column, rows 0-1 = 1,5.
        assert_eq!(seg_id_at(6, 0, 3), 1);
        // BLOCK_16X16 at (2,0): clamped to one row, columns 0-1 = 7,0.
        assert_eq!(seg_id_at(6, 2, 0), 0);
        // BLOCK_32X32 at (1,1): clamped to rows 1-2, columns 1-3
        //   = 2,6,5,0,7,7 -> min 0.
        assert_eq!(seg_id_at(9, 1, 1), 0);
    }

    /// Every result is a valid segment id for every in-range position and
    /// block size (`vp9_pred_common.h:37`).
    #[test]
    fn segment_id_is_always_in_range() {
        for bsize in 0..13u8 {
            for row in 0..SEG_MI_ROWS {
                for col in 0..SEG_MI_COLS {
                    assert!(
                        seg_id_at(bsize, row, col) < MAX_SEGMENTS,
                        "bsize {bsize} at ({row},{col})"
                    );
                }
            }
        }
    }

    /// A uniform map yields that id for every block size and position — the
    /// case every non-segmented frame hits.
    #[test]
    fn segment_id_uniform_map_is_constant() {
        for id in 0..MAX_SEGMENTS {
            let map = [id; SEG_MI_ROWS * SEG_MI_COLS];
            for bsize in 0..13u8 {
                for row in 0..SEG_MI_ROWS {
                    for col in 0..SEG_MI_COLS {
                        assert_eq!(
                            get_segment_id(&map, SEG_MI_ROWS, SEG_MI_COLS, bsize, row, col),
                            id,
                            "id {id}, bsize {bsize} at ({row},{col})"
                        );
                    }
                }
            }
        }
    }
}
