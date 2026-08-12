//! VP9 backward probability adaptation — ports of libvpx v1.15.2
//! `vp9_adapt_coef_probs` (`vp9/common/vp9_entropy.c:1055-1100`),
//! `vp9_adapt_mode_probs` (`vp9/common/vp9_entropymode.c:340-410`),
//! `vp9_adapt_mv_probs` (`vp9/common/vp9_entropymv.c:154-188`) and the
//! `vpx_dsp` probability-merge primitives they are built from
//! (`vpx_dsp/prob.h:48-95`, `vpx_dsp/prob.c:26-47`).
//!
//! # What backward adaptation is
//!
//! A VP9 frame that is neither error-resilient nor frame-parallel ends by
//! folding the symbol counts it observed ([`FrameCounts`]) into its working
//! probability context, so the *next* frame that names the same
//! `frame_context_idx` starts from probabilities tuned to what actually
//! happened. libvpx runs it at the end of `vp9_decode_frame`
//! (`vp9_decodeframe.c:3049-3057`):
//!
//! ```c
//! if (!cm->error_resilient_mode && !cm->frame_parallel_decoding_mode) {
//!   vp9_adapt_coef_probs(cm);
//!   if (!frame_is_intra_only(cm)) {
//!     vp9_adapt_mode_probs(cm);
//!     vp9_adapt_mv_probs(cm, cm->allow_high_precision_mv);
//!   }
//! }
//! ```
//!
//! Two properties of that snippet drive this module:
//!
//! * **Coefficient adaptation runs for every adapting frame, including key
//!   and intra-only frames.** [`adapt_coef_probs`] is therefore live code:
//!   [`super::state::Vp9DecState::finish_frame`] calls it, and an intra-only
//!   frame that loads an adapted context decodes against its result.
//! * **Mode and MV adaptation run only for inter frames.**
//!   [`super::state::Vp9DecState::adapt_probabilities`] keeps that gate, and
//!   it is not cosmetic: a key frame populates the `partition`, `y_mode` and
//!   `skip` counters exactly as an inter frame does, so dropping it would
//!   move probabilities libvpx leaves alone and desynchronise every later
//!   frame that loads the context.
//!
//! # `pre_fc` is the *saved* context, not the working one
//!
//! Every adaptation function merges into `cm->fc` from
//! `cm->frame_contexts[cm->frame_context_idx]` — the context as it was
//! *loaded*, before the compressed header's `diff_update_prob` passes
//! modified the working copy, and before the post-adaptation save writes it
//! back. Passing the post-header working context as `pre` would be a
//! different (wrong) computation whenever the frame updated a probability,
//! so every entry point here takes `pre` explicitly rather than deriving it.
//!
//! # Coverage of `vp9_adapt_mode_probs`
//!
//! [`adapt_mode_probs`] is the **complete** eleven-group statement list of
//! `vp9_adapt_mode_probs` (`vp9_entropymode.c:340-410`), in libvpx's order:
//! `intra_inter`, `comp_inter`, `comp_ref`, `single_ref`, `inter_mode`,
//! `y_mode`, `uv_mode`, `partition`, `switchable_interp` (gated on
//! `cm->interp_filter == SWITCHABLE`), the transform sizes (gated on
//! `cm->tx_mode == TX_MODE_SELECT`, [`adapt_tx_probs`]) and `skip`
//! ([`adapt_skip_probs`]). The last two keep their own entry points because
//! they are separately testable and because [`adapt_tx_probs`]'s gate is a
//! different condition from the function's own.
//!
//! Every group has a [`FrameProbs`] field to land in — including
//! [`FrameProbs::uv_mode`], the one the compressed header never
//! forward-updates, which exists precisely so that this function has
//! somewhere to write. An earlier revision of this module deliberately
//! refused to define a function called `adapt_mode_probs` while only two of
//! the eleven groups had a home; the fields exist now, so the name is
//! honest.

#![forbid(unsafe_code)]
// Every probability here is a `u8` by construction: the merge arithmetic is
// carried in wider integers and clamped to `[1, 255]` (`get_prob`) or bounded
// by `255 * 256` before the `>> 8` (`weighted_prob`) before any narrowing.
#![allow(clippy::cast_possible_truncation)]

use super::counts::{
    FrameCounts, NmvCounts, COEFF_CONTEXTS, COEF_BANDS, EOB_MODEL_TOKEN, ONE_TOKEN, PLANE_TYPES,
    REF_TYPES, SKIP_CONTEXTS, TWO_TOKEN, TX_SIZES, TX_SIZE_CONTEXTS, UNCONSTRAINED_NODES,
    ZERO_TOKEN,
};
use super::hdr::{FrameProbs, TxMode};
use super::tables;
use super::tables_inter::{
    NmvContext, CLASS0_SIZE, INTER_MODE_TREE, MV_CLASS0_TREE, MV_CLASS_TREE, MV_FP_TREE,
    MV_JOINT_TREE, MV_OFFSET_BITS, SWITCHABLE_INTERP_TREE,
};
use crate::vp9::uncompressed::Vp9FrameType;

/// libvpx `COEF_COUNT_SAT` / `COEF_COUNT_SAT_KEY` / `COEF_COUNT_SAT_AFTER_KEY`
/// (`vp9_entropy.c:1048-1053`) — all three are 24, so the count saturation is
/// the same whichever branch `vp9_adapt_coef_probs` takes.
pub const COEF_COUNT_SAT: u32 = 24;
/// libvpx `COEF_MAX_UPDATE_FACTOR_KEY` (`vp9_entropy.c:1051`), used when
/// `frame_is_intra_only(cm)`.
pub const COEF_MAX_UPDATE_FACTOR_KEY: u32 = 112;
/// libvpx `COEF_MAX_UPDATE_FACTOR_AFTER_KEY` (`vp9_entropy.c:1053`), used by
/// an inter frame whose predecessor was a key frame — "adapt quickly".
pub const COEF_MAX_UPDATE_FACTOR_AFTER_KEY: u32 = 128;
/// libvpx `COEF_MAX_UPDATE_FACTOR` (`vp9_entropy.c:1049`), the steady-state
/// inter-frame factor.
pub const COEF_MAX_UPDATE_FACTOR: u32 = 112;

/// libvpx `MODE_MV_COUNT_SAT` (`vpx_dsp/prob.h:37`).
pub const MODE_MV_COUNT_SAT: u32 = 20;

/// libvpx `count_to_update_factor` (`vpx_dsp/prob.h:79-82`), i.e.
/// `MODE_MV_MAX_UPDATE_FACTOR (128) * count / MODE_MV_COUNT_SAT (20)` in
/// integer arithmetic, tabulated for `count` in `0..=20`.
const COUNT_TO_UPDATE_FACTOR: [u32; MODE_MV_COUNT_SAT as usize + 1] = [
    0, 6, 12, 19, 25, 32, 38, 44, 51, 57, 64, 70, 76, 83, 89, 96, 102, 108, 115, 121, 128,
];

/// `BAND_COEFF_CONTEXTS(band)` (`vp9_entropy.h:108`): band 0 carries 3
/// contexts, every other band 6. The remaining band-0 slots are neither
/// coded nor adapted.
const fn band_coeff_contexts(band: usize) -> usize {
    if band == 0 {
        3
    } else {
        COEFF_CONTEXTS
    }
}

/// libvpx `get_prob` (`vpx_dsp/prob.h:48-56`):
///
/// ```c
/// const int p = (int)(((uint64_t)num * 256 + (den >> 1)) / den);
/// const int clipped_prob = p | ((255 - p) >> 23) | (p == 0);
/// ```
///
/// The bit-twiddling clamp is exactly "saturate to `[1, 255]`": `p == 0`
/// forces the low bit, and `p > 255` makes `255 - p` negative so its
/// arithmetic shift fills with ones.
///
/// libvpx `assert(den != 0)`s instead of handling zero; every caller in this
/// module guards (`get_binary_prob` returns 128, `mode_mv_merge_prob`
/// returns the unmodified prior), so the zero branch below is unreachable
/// and exists only to keep this function total rather than panicking.
#[must_use]
pub fn get_prob(num: u32, den: u32) -> u8 {
    if den == 0 {
        return 128;
    }
    let p = (u64::from(num) * 256 + u64::from(den >> 1)) / u64::from(den);
    p.clamp(1, 255) as u8
}

/// libvpx `get_binary_prob` (`vpx_dsp/prob.h:58-62`): the probability of the
/// zero branch, or 128 when nothing was observed.
#[must_use]
pub fn get_binary_prob(n0: u32, n1: u32) -> u8 {
    let den = u64::from(n0) + u64::from(n1);
    if den == 0 {
        return 128;
    }
    let p = (u64::from(n0) * 256 + den / 2) / den;
    p.clamp(1, 255) as u8
}

/// libvpx `weighted_prob` (`vpx_dsp/prob.h:65-67`):
/// `ROUND_POWER_OF_TWO(prob1 * (256 - factor) + prob2 * factor, 8)`.
#[must_use]
pub fn weighted_prob(prob1: u8, prob2: u8, factor: u32) -> u8 {
    let f = factor.min(256);
    let acc = u32::from(prob1) * (256 - f) + u32::from(prob2) * f;
    ((acc + 128) >> 8) as u8
}

/// libvpx `merge_probs` (`vpx_dsp/prob.h:69-76`) — the coefficient-side
/// merge, parameterised by saturation and maximum update factor.
#[must_use]
pub fn merge_prob(pre_prob: u8, ct: [u32; 2], count_sat: u32, max_update_factor: u32) -> u8 {
    let prob = get_binary_prob(ct[0], ct[1]);
    if count_sat == 0 {
        // libvpx never calls with a zero saturation (COEF_COUNT_SAT is 24);
        // keeping the prior is the only defined answer without dividing by
        // zero.
        return pre_prob;
    }
    let total = u64::from(ct[0]) + u64::from(ct[1]);
    let count = total.min(u64::from(count_sat)) as u32;
    let factor = max_update_factor * count / count_sat;
    weighted_prob(pre_prob, prob, factor)
}

/// libvpx `mode_mv_merge_probs` (`vpx_dsp/prob.h:84-95`) — the mode/MV-side
/// merge, with `MODE_MV_COUNT_SAT` / `MODE_MV_MAX_UPDATE_FACTOR` baked in
/// via [`COUNT_TO_UPDATE_FACTOR`].
///
/// Note this is **not** [`merge_prob`] with those two constants substituted:
/// an all-zero count vector returns the prior untouched here, where
/// `merge_probs` would compute `get_binary_prob(0, 0) == 128` and then apply
/// it with factor 0. The two agree numerically in that case, but only
/// because factor 0 discards the 128; the code paths are distinct and both
/// are ported.
#[must_use]
pub fn mode_mv_merge_prob(pre_prob: u8, ct: [u32; 2]) -> u8 {
    let den = u64::from(ct[0]) + u64::from(ct[1]);
    if den == 0 {
        return pre_prob;
    }
    let count = den.min(u64::from(MODE_MV_COUNT_SAT)) as usize;
    let factor = COUNT_TO_UPDATE_FACTOR[count];
    let prob = get_prob_u64(u64::from(ct[0]), den);
    weighted_prob(pre_prob, prob, factor)
}

/// [`get_prob`] over `u64` operands, for the mode/MV path whose denominator
/// is a sum of two `u32` counters.
fn get_prob_u64(num: u64, den: u64) -> u8 {
    if den == 0 {
        return 128;
    }
    let p = (num.saturating_mul(256).saturating_add(den >> 1)) / den;
    p.clamp(1, 255) as u8
}

/// libvpx `vpx_tree_merge_probs` / `tree_merge_probs_impl`
/// (`vpx_dsp/prob.c:26-47`): walks a VP9 token tree bottom-up, merging each
/// internal node against the summed leaf counts below its two branches.
///
/// `tree` is the flat `vpx_tree_index` array (`<= 0` is a leaf naming
/// `counts[-value]`, `> 0` is the index of the child node pair), `pre_probs`
/// / `probs` are indexed by `node >> 1`.
pub fn tree_merge_probs(tree: &[i8], pre_probs: &[u8], counts: &[u32], probs: &mut [u8]) {
    tree_merge_probs_impl(0, tree, pre_probs, counts, probs);
}

fn tree_merge_probs_impl(
    i: usize,
    tree: &[i8],
    pre_probs: &[u8],
    counts: &[u32],
    probs: &mut [u8],
) -> u32 {
    let l = tree[i];
    let left_count = if l <= 0 {
        counts[usize::from(l.unsigned_abs())]
    } else {
        tree_merge_probs_impl(l as usize, tree, pre_probs, counts, probs)
    };
    let r = tree[i + 1];
    let right_count = if r <= 0 {
        counts[usize::from(r.unsigned_abs())]
    } else {
        tree_merge_probs_impl(r as usize, tree, pre_probs, counts, probs)
    };
    probs[i >> 1] = mode_mv_merge_prob(pre_probs[i >> 1], [left_count, right_count]);
    left_count.saturating_add(right_count)
}

/// The `(count_sat, update_factor)` pair `vp9_adapt_coef_probs` selects
/// (`vp9_entropy.c:1084-1098`).
///
/// `frame_is_intra_only` is "key frame or intra-only frame";
/// `last_frame_type` is the *previous* decoded frame's type, which is why
/// [`super::state::Vp9DecState::finish_frame`] must adapt before it advances
/// `last_frame_type`.
#[must_use]
pub fn coef_adaptation_params(
    frame_is_intra_only: bool,
    last_frame_type: Vp9FrameType,
) -> (u32, u32) {
    if frame_is_intra_only {
        (COEF_COUNT_SAT, COEF_MAX_UPDATE_FACTOR_KEY)
    } else if last_frame_type == Vp9FrameType::Key {
        (COEF_COUNT_SAT, COEF_MAX_UPDATE_FACTOR_AFTER_KEY)
    } else {
        (COEF_COUNT_SAT, COEF_MAX_UPDATE_FACTOR)
    }
}

/// libvpx `vp9_adapt_coef_probs` + its `adapt_coef_probs` helper
/// (`vp9_entropy.c:1055-1100`).
///
/// `fc` is the working context to update in place, `pre` the saved context
/// the frame loaded (`cm->frame_contexts[cm->frame_context_idx]`).
///
/// The three branch counts per (band, ctx) reconstruct the binary decisions
/// the model tree actually coded:
///
/// ```c
/// { neob, eob_counts - neob }, { n0, n1 + n2 }, { n1, n2 }
/// ```
///
/// where `neob` is how often the EOB branch *terminated* the block and
/// `eob_counts` how often it was read at all — which is precisely why
/// [`FrameCounts::eob_branch`] has to be incremented before the EOB read
/// rather than derived from the token counts afterwards.
pub fn adapt_coef_probs(
    fc: &mut FrameProbs,
    pre: &FrameProbs,
    counts: &FrameCounts,
    frame_is_intra_only: bool,
    last_frame_type: Vp9FrameType,
) {
    let (count_sat, update_factor) = coef_adaptation_params(frame_is_intra_only, last_frame_type);
    for tx in 0..TX_SIZES {
        for i in 0..PLANE_TYPES {
            for j in 0..REF_TYPES {
                for k in 0..COEF_BANDS {
                    for l in 0..band_coeff_contexts(k) {
                        let slot = &counts.coef[tx][i][j][k][l];
                        let n0 = slot[ZERO_TOKEN];
                        let n1 = slot[ONE_TOKEN];
                        let n2 = slot[TWO_TOKEN];
                        let neob = slot[EOB_MODEL_TOKEN];
                        let eob = counts.eob_branch[tx][i][j][k][l];
                        let branch_ct = [
                            [neob, eob.saturating_sub(neob)],
                            [n0, n1.saturating_add(n2)],
                            [n1, n2],
                        ];
                        for (m, ct) in branch_ct.iter().enumerate().take(UNCONSTRAINED_NODES) {
                            fc.coef[tx][i][j][k][l][m] = merge_prob(
                                pre.coef[tx][i][j][k][l][m],
                                *ct,
                                count_sat,
                                update_factor,
                            );
                        }
                    }
                }
            }
        }
    }
}

/// libvpx `tx_counts_to_branch_counts_32x32` (`vp9_entropymode.c:288-297`).
#[must_use]
pub fn tx_counts_to_branch_counts_32x32(p: &[u32; TX_SIZES]) -> [[u32; 2]; TX_SIZES - 1] {
    [
        [p[0], p[1].saturating_add(p[2]).saturating_add(p[3])],
        [p[1], p[2].saturating_add(p[3])],
        [p[2], p[3]],
    ]
}

/// libvpx `tx_counts_to_branch_counts_16x16` (`vp9_entropymode.c:299-305`).
#[must_use]
pub fn tx_counts_to_branch_counts_16x16(p: &[u32; TX_SIZES - 1]) -> [[u32; 2]; TX_SIZES - 2] {
    [[p[0], p[1].saturating_add(p[2])], [p[1], p[2]]]
}

/// libvpx `tx_counts_to_branch_counts_8x8` (`vp9_entropymode.c:307-311`).
#[must_use]
pub fn tx_counts_to_branch_counts_8x8(p: &[u32; TX_SIZES - 2]) -> [[u32; 2]; TX_SIZES - 3] {
    [[p[0], p[1]]]
}

/// The `if (cm->tx_mode == TX_MODE_SELECT)` block of `vp9_adapt_mode_probs`
/// (`vp9_entropymode.c:379-404`).
///
/// A no-op unless the frame coded `TX_MODE_SELECT`, exactly as in libvpx:
/// when the transform size is not per-block-selected, no tx symbol was read,
/// so no tx probability may move.
pub fn adapt_tx_probs(
    fc: &mut FrameProbs,
    pre: &FrameProbs,
    counts: &FrameCounts,
    tx_mode: TxMode,
) {
    if tx_mode != TxMode::Select {
        return;
    }
    for i in 0..TX_SIZE_CONTEXTS {
        let ct8 = tx_counts_to_branch_counts_8x8(&counts.tx.p8x8[i]);
        for (j, ct) in ct8.iter().enumerate() {
            fc.tx8[i][j] = mode_mv_merge_prob(pre.tx8[i][j], *ct);
        }
        let ct16 = tx_counts_to_branch_counts_16x16(&counts.tx.p16x16[i]);
        for (j, ct) in ct16.iter().enumerate() {
            fc.tx16[i][j] = mode_mv_merge_prob(pre.tx16[i][j], *ct);
        }
        let ct32 = tx_counts_to_branch_counts_32x32(&counts.tx.p32x32[i]);
        for (j, ct) in ct32.iter().enumerate() {
            fc.tx32[i][j] = mode_mv_merge_prob(pre.tx32[i][j], *ct);
        }
    }
}

/// The final loop of `vp9_adapt_mode_probs` (`vp9_entropymode.c:406-408`).
pub fn adapt_skip_probs(fc: &mut FrameProbs, pre: &FrameProbs, counts: &FrameCounts) {
    for i in 0..SKIP_CONTEXTS {
        fc.skip[i] = mode_mv_merge_prob(pre.skip[i], counts.skip[i]);
    }
}

/// libvpx `vp9_adapt_mode_probs` (`vp9_entropymode.c:340-410`), complete —
/// all eleven probability groups, in libvpx's order:
///
/// ```c
/// for (i = 0; i < INTRA_INTER_CONTEXTS; i++)
///   fc->intra_inter_prob[i] = mode_mv_merge_probs(pre_fc->intra_inter_prob[i],
///                                                 counts->intra_inter[i]);
/// for (i = 0; i < COMP_INTER_CONTEXTS; i++)
///   fc->comp_inter_prob[i] = mode_mv_merge_probs(pre_fc->comp_inter_prob[i],
///                                                counts->comp_inter[i]);
/// for (i = 0; i < REF_CONTEXTS; i++)
///   fc->comp_ref_prob[i] = mode_mv_merge_probs(pre_fc->comp_ref_prob[i],
///                                              counts->comp_ref[i]);
/// for (i = 0; i < REF_CONTEXTS; i++)
///   for (j = 0; j < 2; j++)
///     fc->single_ref_prob[i][j] = mode_mv_merge_probs(
///         pre_fc->single_ref_prob[i][j], counts->single_ref[i][j]);
/// for (i = 0; i < INTER_MODE_CONTEXTS; i++)
///   vpx_tree_merge_probs(vp9_inter_mode_tree, pre_fc->inter_mode_probs[i],
///                        counts->inter_mode[i], fc->inter_mode_probs[i]);
/// for (i = 0; i < BLOCK_SIZE_GROUPS; i++)
///   vpx_tree_merge_probs(vp9_intra_mode_tree, pre_fc->y_mode_prob[i],
///                        counts->y_mode[i], fc->y_mode_prob[i]);
/// for (i = 0; i < INTRA_MODES; ++i)
///   vpx_tree_merge_probs(vp9_intra_mode_tree, pre_fc->uv_mode_prob[i],
///                        counts->uv_mode[i], fc->uv_mode_prob[i]);
/// for (i = 0; i < PARTITION_CONTEXTS; i++)
///   vpx_tree_merge_probs(vp9_partition_tree, pre_fc->partition_prob[i],
///                        counts->partition[i], fc->partition_prob[i]);
/// if (cm->interp_filter == SWITCHABLE) { ... switchable_interp ... }
/// if (cm->tx_mode == TX_MODE_SELECT)  { ... tx probs ... }
/// for (i = 0; i < SKIP_CONTEXTS; ++i)
///   fc->skip_probs[i] = mode_mv_merge_probs(pre_fc->skip_probs[i],
///                                           counts->skip[i]);
/// ```
///
/// Two gates are load-bearing and both are the *frame-level* value, not a
/// per-block one:
///
/// * `switchable_interp` moves only when the frame coded
///   `interp_filter == SWITCHABLE`, i.e. when any block actually read a
///   filter symbol. `interp_filter_is_switchable` is
///   [`super::hdr::SWITCHABLE`] compared against the **raw** header value
///   (the `LITERAL_TO_FILTER` permutation maps `4` to itself, so either form
///   answers this question identically).
/// * the transform probabilities move only under `TX_MODE_SELECT`
///   ([`adapt_tx_probs`], which re-checks it itself).
///
/// libvpx calls this only for a non-intra frame
/// (`if (!frame_is_intra_only(cm))`, `vp9_decodeframe.c:3052`); the caller
/// ([`super::state::Vp9DecState::adapt_probabilities`]) keeps that gate,
/// because an intra frame's `y_mode` / `partition` / `skip` counters *are*
/// populated (a key frame codes partitions and skips) and adapting from them
/// would move probabilities libvpx leaves alone.
pub fn adapt_mode_probs(
    fc: &mut FrameProbs,
    pre: &FrameProbs,
    counts: &FrameCounts,
    interp_filter_is_switchable: bool,
    tx_mode: TxMode,
) {
    for ((p, pre_p), ct) in fc
        .intra_inter
        .iter_mut()
        .zip(pre.intra_inter.iter())
        .zip(counts.intra_inter.iter())
    {
        *p = mode_mv_merge_prob(*pre_p, *ct);
    }
    for ((p, pre_p), ct) in fc
        .comp_inter
        .iter_mut()
        .zip(pre.comp_inter.iter())
        .zip(counts.comp_inter.iter())
    {
        *p = mode_mv_merge_prob(*pre_p, *ct);
    }
    for ((p, pre_p), ct) in fc
        .comp_ref
        .iter_mut()
        .zip(pre.comp_ref.iter())
        .zip(counts.comp_ref.iter())
    {
        *p = mode_mv_merge_prob(*pre_p, *ct);
    }
    for ((row, pre_row), ct_row) in fc
        .single_ref
        .iter_mut()
        .zip(pre.single_ref.iter())
        .zip(counts.single_ref.iter())
    {
        for ((p, pre_p), ct) in row.iter_mut().zip(pre_row.iter()).zip(ct_row.iter()) {
            *p = mode_mv_merge_prob(*pre_p, *ct);
        }
    }

    for ((row, pre_row), ct_row) in fc
        .inter_mode
        .iter_mut()
        .zip(pre.inter_mode.iter())
        .zip(counts.inter_mode.iter())
    {
        tree_merge_probs(&INTER_MODE_TREE, pre_row, ct_row, row);
    }
    for ((row, pre_row), ct_row) in fc
        .y_mode
        .iter_mut()
        .zip(pre.y_mode.iter())
        .zip(counts.y_mode.iter())
    {
        tree_merge_probs(&tables::INTRA_MODE_TREE, pre_row, ct_row, row);
    }
    for ((row, pre_row), ct_row) in fc
        .uv_mode
        .iter_mut()
        .zip(pre.uv_mode.iter())
        .zip(counts.uv_mode.iter())
    {
        tree_merge_probs(&tables::INTRA_MODE_TREE, pre_row, ct_row, row);
    }
    for ((row, pre_row), ct_row) in fc
        .partition
        .iter_mut()
        .zip(pre.partition.iter())
        .zip(counts.partition.iter())
    {
        tree_merge_probs(&tables::PARTITION_TREE, pre_row, ct_row, row);
    }

    if interp_filter_is_switchable {
        for ((row, pre_row), ct_row) in fc
            .switchable_interp
            .iter_mut()
            .zip(pre.switchable_interp.iter())
            .zip(counts.switchable_interp.iter())
        {
            tree_merge_probs(&SWITCHABLE_INTERP_TREE, pre_row, ct_row, row);
        }
    }

    adapt_tx_probs(fc, pre, counts, tx_mode);
    adapt_skip_probs(fc, pre, counts);
}

/// libvpx `vp9_adapt_mv_probs` (`vp9_entropymv.c:154-188`), complete.
///
/// `allow_hp` is the frame's `allow_high_precision_mv`: the two
/// high-precision probabilities are adapted only when the frame could code
/// them, otherwise their counters are all zero *and* libvpx skips the merge
/// entirely.
pub fn adapt_mv_probs(fc: &mut NmvContext, pre: &NmvContext, counts: &NmvCounts, allow_hp: bool) {
    tree_merge_probs(&MV_JOINT_TREE, &pre.joints, &counts.joints, &mut fc.joints);

    for i in 0..2 {
        let pre_comp = &pre.comps[i];
        let c = &counts.comps[i];
        let comp = &mut fc.comps[i];

        comp.sign = mode_mv_merge_prob(pre_comp.sign, c.sign);
        tree_merge_probs(
            &MV_CLASS_TREE,
            &pre_comp.classes,
            &c.classes,
            &mut comp.classes,
        );
        tree_merge_probs(
            &MV_CLASS0_TREE,
            &pre_comp.class0,
            &c.class0,
            &mut comp.class0,
        );

        for j in 0..MV_OFFSET_BITS {
            comp.bits[j] = mode_mv_merge_prob(pre_comp.bits[j], c.bits[j]);
        }

        for j in 0..CLASS0_SIZE {
            tree_merge_probs(
                &MV_FP_TREE,
                &pre_comp.class0_fp[j],
                &c.class0_fp[j],
                &mut comp.class0_fp[j],
            );
        }

        tree_merge_probs(&MV_FP_TREE, &pre_comp.fp, &c.fp, &mut comp.fp);

        if allow_hp {
            comp.class0_hp = mode_mv_merge_prob(pre_comp.class0_hp, c.class0_hp);
            comp.hp = mode_mv_merge_prob(pre_comp.hp, c.hp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tables::PARTITION_TREE;
    use super::super::tables_inter::DEFAULT_NMV_CONTEXT;
    use super::*;

    // -- primitives, hand-traced against vpx_dsp/prob.h --------------------

    #[test]
    fn get_prob_rounds_and_saturates() {
        // (1 * 256 + 1) / 2 = 128
        assert_eq!(get_prob(1, 2), 128);
        // (0 * 256 + 2) / 4 = 0 -> the `(p == 0)` term forces 1
        assert_eq!(get_prob(0, 4), 1);
        // (4 * 256 + 2) / 4 = 256 -> the `((255 - p) >> 23)` term forces 255
        assert_eq!(get_prob(4, 4), 255);
        // (3 * 256 + 2) / 4 = 192 (768/4 = 192, +2 does not carry)
        assert_eq!(get_prob(3, 4), 192);
        // Rounding is "+ den/2", not truncation: (1*256 + 1)/3 = 85
        assert_eq!(get_prob(1, 3), 85);
    }

    #[test]
    fn get_binary_prob_handles_the_empty_case() {
        assert_eq!(get_binary_prob(0, 0), 128, "vpx_dsp/prob.h:58-62");
        assert_eq!(get_binary_prob(1, 1), 128);
        assert_eq!(get_binary_prob(0, 7), 1, "clamped away from zero");
        assert_eq!(get_binary_prob(7, 0), 255, "clamped away from 256");
    }

    #[test]
    fn weighted_prob_is_rounded_power_of_two() {
        // (200 * 128 + 100 * 128 + 128) >> 8 = 38528 >> 8 = 150
        assert_eq!(weighted_prob(200, 100, 128), 150);
        // factor 0 keeps the prior exactly
        assert_eq!(weighted_prob(200, 100, 0), 200);
        // factor 256 takes the new value exactly
        assert_eq!(weighted_prob(200, 100, 256), 100);
    }

    #[test]
    fn merge_prob_hand_traced() {
        // ct = [10, 10]: prob = 128, count = 20, factor = 112*20/24 = 93,
        // weighted_prob(128, 128, 93) = 128.
        assert_eq!(merge_prob(128, [10, 10], 24, 112), 128);
        // ct = [0, 24]: prob = get_binary_prob(0, 24) = 1 (clamped),
        // count = 24, factor = 112, (200*144 + 1*112 + 128) >> 8 = 113.
        assert_eq!(merge_prob(200, [0, 24], 24, 112), 113);
        // No counts at all: prob = 128 but factor = 0, so the prior survives.
        assert_eq!(merge_prob(200, [0, 0], 24, 112), 200);
        // Saturation: 1000 observations weigh exactly as much as 24 do.
        assert_eq!(
            merge_prob(200, [0, 1000], 24, 112),
            merge_prob(200, [0, 24], 24, 112)
        );
    }

    #[test]
    fn count_to_update_factor_table_is_the_documented_formula() {
        for (count, &factor) in COUNT_TO_UPDATE_FACTOR.iter().enumerate() {
            let expected = 128 * count as u32 / MODE_MV_COUNT_SAT;
            assert_eq!(
                factor, expected,
                "count_to_update_factor[{count}] must equal 128 * {count} / 20"
            );
        }
        assert_eq!(COUNT_TO_UPDATE_FACTOR[0], 0);
        assert_eq!(COUNT_TO_UPDATE_FACTOR[20], 128);
    }

    #[test]
    fn mode_mv_merge_prob_hand_traced() {
        // den == 0 keeps the prior untouched (vpx_dsp/prob.h:86-88).
        assert_eq!(mode_mv_merge_prob(100, [0, 0]), 100);
        // ct = [5, 5]: den 10, factor 64, prob 128 -> 128.
        assert_eq!(mode_mv_merge_prob(128, [5, 5]), 128);
        // ct = [20, 0]: den 20 -> factor 128 (full update), prob 255,
        // weighted_prob(10, 255, 128) = (1280 + 32640 + 128) >> 8 = 133.
        assert_eq!(mode_mv_merge_prob(10, [20, 0]), 133);
        // Saturation at 20 observations.
        assert_eq!(
            mode_mv_merge_prob(10, [200, 0]),
            mode_mv_merge_prob(10, [20, 0])
        );
    }

    /// The zero-count branch is where `merge_probs` and `mode_mv_merge_probs`
    /// diverge structurally (one returns the prior directly, the other
    /// computes 128 and applies it with factor 0). Both must land on the
    /// prior, and a refactor that collapses them must not change either.
    #[test]
    fn the_two_merges_agree_on_empty_counts_by_different_routes() {
        for pre in [1u8, 7, 128, 200, 255] {
            assert_eq!(mode_mv_merge_prob(pre, [0, 0]), pre);
            assert_eq!(merge_prob(pre, [0, 0], 24, 112), pre);
        }
    }

    #[test]
    fn tree_merge_probs_hand_traced_on_the_partition_tree() {
        // PARTITION_TREE = [0, 2, -1, 4, -2, -3]:
        //   node 0 -> leaf 0 (NONE) vs node 2
        //   node 2 -> leaf 1 (HORZ) vs node 4
        //   node 4 -> leaf 2 (VERT) vs leaf 3 (SPLIT)
        // counts [10, 0, 0, 10]:
        //   probs[2] = mode_mv_merge(128, [0, 10])  = 96
        //   probs[1] = mode_mv_merge(128, [0, 10])  = 96
        //   probs[0] = mode_mv_merge(128, [10, 10]) = 128
        let pre = [128u8, 128, 128];
        let counts = [10u32, 0, 0, 10];
        let mut probs = [0u8; 3];
        tree_merge_probs(&PARTITION_TREE, &pre, &counts, &mut probs);
        assert_eq!(probs, [128, 96, 96]);
    }

    #[test]
    fn tree_merge_probs_with_no_counts_keeps_every_prior() {
        let pre = [3u8, 111, 250];
        let counts = [0u32; 4];
        let mut probs = [0u8; 3];
        tree_merge_probs(&PARTITION_TREE, &pre, &counts, &mut probs);
        assert_eq!(probs, pre, "an unobserved tree may not move at all");
    }

    // -- tx branch counts --------------------------------------------------

    #[test]
    fn tx_branch_counts_match_libvpx() {
        // vp9_entropymode.c:288-311, with distinguishable per-size counts.
        let p32 = [1u32, 2, 4, 8];
        assert_eq!(
            tx_counts_to_branch_counts_32x32(&p32),
            [[1, 14], [2, 12], [4, 8]]
        );
        let p16 = [1u32, 2, 4];
        assert_eq!(tx_counts_to_branch_counts_16x16(&p16), [[1, 6], [2, 4]]);
        let p8 = [3u32, 5];
        assert_eq!(tx_counts_to_branch_counts_8x8(&p8), [[3, 5]]);
    }

    // -- coefficient adaptation --------------------------------------------

    #[test]
    fn coef_adaptation_params_match_vp9_adapt_coef_probs() {
        // frame_is_intra_only wins regardless of what preceded it.
        assert_eq!(
            coef_adaptation_params(true, Vp9FrameType::Key),
            (24, 112),
            "COEF_MAX_UPDATE_FACTOR_KEY"
        );
        assert_eq!(coef_adaptation_params(true, Vp9FrameType::Inter), (24, 112));
        // An inter frame right after a key frame adapts quickly.
        assert_eq!(
            coef_adaptation_params(false, Vp9FrameType::Key),
            (24, 128),
            "COEF_MAX_UPDATE_FACTOR_AFTER_KEY"
        );
        // Steady state.
        assert_eq!(
            coef_adaptation_params(false, Vp9FrameType::Inter),
            (24, 112)
        );
    }

    #[test]
    fn adapt_coef_probs_is_a_no_op_without_counts() {
        let pre = FrameProbs::defaults();
        let mut fc = FrameProbs::defaults();
        let counts = FrameCounts::new();
        adapt_coef_probs(&mut fc, &pre, &counts, true, Vp9FrameType::Key);
        assert!(
            fc == FrameProbs::defaults(),
            "a frame that coded no coefficients may not move any probability"
        );
    }

    #[test]
    fn adapt_coef_probs_hand_traced_single_node() {
        let pre = FrameProbs::defaults();
        let mut fc = FrameProbs::defaults();
        let mut counts = FrameCounts::new();

        // tx 0, luma, intra, band 1, ctx 0: 30 EOB-branch reads, 6 of which
        // terminated the block, and 24 coded tokens split 10 zero / 8 one /
        // 6 two-or-more.
        counts.eob_branch[0][0][0][1][0] = 30;
        counts.coef[0][0][0][1][0][EOB_MODEL_TOKEN] = 6;
        counts.coef[0][0][0][1][0][ZERO_TOKEN] = 10;
        counts.coef[0][0][0][1][0][ONE_TOKEN] = 8;
        counts.coef[0][0][0][1][0][TWO_TOKEN] = 6;

        adapt_coef_probs(&mut fc, &pre, &counts, true, Vp9FrameType::Key);

        // Branch counts: {6, 24}, {10, 14}, {8, 6}; count_sat 24, factor 112.
        let p = pre.coef[0][0][0][1][0];
        assert_eq!(
            fc.coef[0][0][0][1][0][0],
            merge_prob(p[0], [6, 24], 24, 112)
        );
        assert_eq!(
            fc.coef[0][0][0][1][0][1],
            merge_prob(p[1], [10, 14], 24, 112)
        );
        assert_eq!(fc.coef[0][0][0][1][0][2], merge_prob(p[2], [8, 6], 24, 112));
        // ...and the values really moved off the defaults.
        assert_ne!(fc.coef[0][0][0][1][0], p, "adaptation must be observable");

        // Every other node is untouched.
        assert_eq!(fc.coef[0][0][0][1][1], pre.coef[0][0][0][1][1]);
        assert_eq!(fc.coef[1][0][0][1][0], pre.coef[1][0][0][1][0]);
    }

    /// Band 0 codes only three contexts (`BAND_COEFF_CONTEXTS`), so contexts
    /// 3..6 of band 0 must never be adapted even if a caller put counts
    /// there.
    #[test]
    fn band_zero_adapts_only_its_three_coded_contexts() {
        let pre = FrameProbs::defaults();
        let mut fc = FrameProbs::defaults();
        let mut counts = FrameCounts::new();
        for ctx in 0..COEFF_CONTEXTS {
            counts.eob_branch[0][0][0][0][ctx] = 40;
            counts.coef[0][0][0][0][ctx][EOB_MODEL_TOKEN] = 20;
            counts.coef[0][0][0][0][ctx][ZERO_TOKEN] = 20;
        }
        adapt_coef_probs(&mut fc, &pre, &counts, true, Vp9FrameType::Key);
        for ctx in 0..3 {
            assert_ne!(
                fc.coef[0][0][0][0][ctx], pre.coef[0][0][0][0][ctx],
                "band 0 ctx {ctx} is coded and must adapt"
            );
        }
        for ctx in 3..COEFF_CONTEXTS {
            assert_eq!(
                fc.coef[0][0][0][0][ctx], pre.coef[0][0][0][0][ctx],
                "band 0 ctx {ctx} is never coded, so it may not adapt"
            );
        }
        assert_eq!(band_coeff_contexts(0), 3);
        assert_eq!(band_coeff_contexts(1), 6);
    }

    /// `pre` and `fc` are genuinely distinct inputs: adaptation merges the
    /// counts into the *saved* context, not into whatever the compressed
    /// header left in the working copy.
    #[test]
    fn adaptation_reads_pre_not_the_working_context() {
        let mut pre = FrameProbs::defaults();
        pre.coef[0][0][0][1][0][0] = 40;
        let mut fc = FrameProbs::defaults();
        // As if the compressed header had moved this node a long way.
        fc.coef[0][0][0][1][0][0] = 220;

        let mut counts = FrameCounts::new();
        counts.eob_branch[0][0][0][1][0] = 24;
        counts.coef[0][0][0][1][0][EOB_MODEL_TOKEN] = 24;

        adapt_coef_probs(&mut fc, &pre, &counts, true, Vp9FrameType::Key);

        // {24, 0} -> prob 255, factor 112: weighted_prob(40, 255, 112).
        assert_eq!(fc.coef[0][0][0][1][0][0], weighted_prob(40, 255, 112));
        assert_ne!(
            fc.coef[0][0][0][1][0][0],
            weighted_prob(220, 255, 112),
            "using the post-header working value as the prior would be wrong"
        );
    }

    #[test]
    fn update_factor_after_a_key_frame_differs_from_steady_state() {
        let pre = FrameProbs::defaults();
        let mut counts = FrameCounts::new();
        counts.eob_branch[0][0][0][1][0] = 24;
        counts.coef[0][0][0][1][0][EOB_MODEL_TOKEN] = 24;

        let mut after_key = FrameProbs::defaults();
        adapt_coef_probs(&mut after_key, &pre, &counts, false, Vp9FrameType::Key);
        let mut steady = FrameProbs::defaults();
        adapt_coef_probs(&mut steady, &pre, &counts, false, Vp9FrameType::Inter);

        assert_ne!(
            after_key.coef[0][0][0][1][0][0], steady.coef[0][0][0][1][0][0],
            "factor 128 after a key frame must not equal factor 112"
        );
    }

    // -- tx / skip ---------------------------------------------------------

    #[test]
    fn adapt_tx_probs_only_runs_for_tx_mode_select() {
        let pre = FrameProbs::defaults();
        let mut counts = FrameCounts::new();
        counts.tx.p32x32[0] = [10, 0, 0, 0];
        counts.tx.p16x16[0] = [10, 0, 0];
        counts.tx.p8x8[0] = [10, 0];

        for mode in [
            TxMode::Only4x4,
            TxMode::Allow8x8,
            TxMode::Allow16x16,
            TxMode::Allow32x32,
        ] {
            let mut fc = FrameProbs::defaults();
            adapt_tx_probs(&mut fc, &pre, &counts, mode);
            assert!(
                fc == FrameProbs::defaults(),
                "{mode:?} codes no tx symbol, so no tx probability may move"
            );
        }

        let mut fc = FrameProbs::defaults();
        adapt_tx_probs(&mut fc, &pre, &counts, TxMode::Select);
        assert_eq!(fc.tx8[0][0], mode_mv_merge_prob(pre.tx8[0][0], [10, 0]));
        assert_eq!(fc.tx16[0][0], mode_mv_merge_prob(pre.tx16[0][0], [10, 0]));
        assert_eq!(fc.tx32[0][0], mode_mv_merge_prob(pre.tx32[0][0], [10, 0]));
        assert_eq!(fc.tx8[1], pre.tx8[1], "context 1 saw nothing");
    }

    #[test]
    fn adapt_skip_probs_merges_every_context() {
        let pre = FrameProbs::defaults();
        let mut fc = FrameProbs::defaults();
        let mut counts = FrameCounts::new();
        counts.skip = [[30, 0], [0, 0], [5, 15]];
        adapt_skip_probs(&mut fc, &pre, &counts);
        assert_eq!(fc.skip[0], mode_mv_merge_prob(pre.skip[0], [30, 0]));
        assert_eq!(fc.skip[1], pre.skip[1], "no observations, no movement");
        assert_eq!(fc.skip[2], mode_mv_merge_prob(pre.skip[2], [5, 15]));
    }

    // -- motion vectors ----------------------------------------------------

    #[test]
    fn adapt_mv_probs_is_a_no_op_without_counts() {
        let pre = DEFAULT_NMV_CONTEXT;
        let mut fc = DEFAULT_NMV_CONTEXT;
        let counts = NmvCounts::default();
        adapt_mv_probs(&mut fc, &pre, &counts, true);
        assert!(
            fc == DEFAULT_NMV_CONTEXT,
            "no MVs coded, no probability moves"
        );
    }

    #[test]
    fn adapt_mv_probs_merges_joints_signs_and_bits() {
        let pre = DEFAULT_NMV_CONTEXT;
        let mut fc = DEFAULT_NMV_CONTEXT;
        let mut counts = NmvCounts::default();
        counts.joints = [10, 0, 0, 10];
        counts.comps[0].sign = [7, 3];
        counts.comps[0].bits[2] = [4, 12];
        counts.comps[1].classes[0] = 9;

        adapt_mv_probs(&mut fc, &pre, &counts, true);

        // MV_JOINT_TREE has the same shape as PARTITION_TREE, so the
        // hand-traced walk above applies to these counts too.
        let mut expected_joints = [0u8; 3];
        tree_merge_probs(
            &MV_JOINT_TREE,
            &pre.joints,
            &counts.joints,
            &mut expected_joints,
        );
        assert_eq!(fc.joints, expected_joints);
        assert_eq!(
            fc.comps[0].sign,
            mode_mv_merge_prob(pre.comps[0].sign, [7, 3])
        );
        assert_eq!(
            fc.comps[0].bits[2],
            mode_mv_merge_prob(pre.comps[0].bits[2], [4, 12])
        );
        assert_eq!(fc.comps[0].bits[3], pre.comps[0].bits[3], "unobserved bit");
        assert_ne!(
            fc.comps[1].classes, pre.comps[1].classes,
            "component 1 observed a class-0 magnitude"
        );
    }

    #[test]
    fn adapt_mv_probs_skips_high_precision_when_the_frame_forbids_it() {
        let pre = DEFAULT_NMV_CONTEXT;
        let mut counts = NmvCounts::default();
        counts.comps[0].class0_hp = [20, 0];
        counts.comps[0].hp = [0, 20];

        let mut without_hp = DEFAULT_NMV_CONTEXT;
        adapt_mv_probs(&mut without_hp, &pre, &counts, false);
        assert_eq!(without_hp.comps[0].class0_hp, pre.comps[0].class0_hp);
        assert_eq!(without_hp.comps[0].hp, pre.comps[0].hp);

        let mut with_hp = DEFAULT_NMV_CONTEXT;
        adapt_mv_probs(&mut with_hp, &pre, &counts, true);
        assert_eq!(
            with_hp.comps[0].class0_hp,
            mode_mv_merge_prob(pre.comps[0].class0_hp, [20, 0])
        );
        assert_eq!(
            with_hp.comps[0].hp,
            mode_mv_merge_prob(pre.comps[0].hp, [0, 20])
        );
    }
}
