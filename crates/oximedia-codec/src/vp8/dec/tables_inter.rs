//! VP8 inter-frame (P-frame) constant tables — RFC 6386 §16-§18.
//!
//! Companion to [`super::tables`] (the key-frame / shared normative tables):
//! this module pins the additional trees, default probabilities and filter
//! coefficients that VP8 *inter* frames need on top of the key-frame set —
//! the frame-level luma/chroma mode trees used by non-key frames, the
//! macroblock/sub-block motion-vector reference-mode trees, the mbsplit
//! (macroblock-partition) tree and fill maps, the short-motion-vector
//! magnitude tree, default mode/MV probabilities, and the sub-pixel
//! interpolation filters.
//!
//! # Scope
//!
//! This is a **data-only** module: constant tables plus their structural
//! invariant tests, no decode logic. Its consumers are the macroblock-mode
//! ([`super::mode`]), motion-vector ([`super::mv`]) and motion-compensation
//! ([`super::mc`]) decoders, all of which are wired into the inter-frame
//! decode path in [`super::inter`].
//!
//! # Provenance
//!
//! Every table is transcribed from RFC 6386 (sections cited inline) and was
//! cross-checked against the current libvpx reference implementation
//! (`webmproject/libvpx`, `main` branch: `vp8/common/entropymode.c`,
//! `vp8/common/entropymv.c`, `vp8/common/entropymv.h`,
//! `vp8/common/modecont.c`, `vp8/common/blockd.h`,
//! `vp8/decoder/decodemv.c`) while writing this file. Several trees/enums
//! matched byte-for-byte against values already pinned in [`super::tables`]
//! (`KF_YMODE_TREE`, `UV_MODE_TREE`, `BMODE_TREE`), which independently
//! corroborates the source cross-check. [`UV_MODE_TREE`] is RFC 6386's same
//! `uv_mode_tree` object reused for inter frames; it is re-pinned here as an
//! independent literal copy plus an equality test against
//! [`super::tables::UV_MODE_TREE`] so the two copies cannot silently drift
//! apart.
//!
//! One naming note versus the task brief that requested these tables: the
//! libvpx symbol for the context-dependent sub-mv-ref probabilities that
//! actually drives bitstream decoding (indexed by an `(above==0, left==0,
//! left==above)` triple, 8 contexts) is `vp8_sub_mv_ref_prob3`, not
//! `..._prob2` — libvpx also has a separate, unrelated 5-row
//! `vp8_sub_mv_ref_prob2` that is not indexed by that triple. `[[u8; 3]; 8]`
//! (this module's [`SUB_MV_REF_PROBS2`]) is the `prob3` table; see its doc
//! comment.
//!
//! [`SIXTAP_FILTERS`] is the one table that predates this file: it is
//! salvaged verbatim from the frozen `vp8::motion::SUBPEL_FILTERS` seed
//! (`crates/oximedia-codec/src/vp8/motion.rs`, `SUBPEL_FILTERS` around
//! line 71), which independently matches libvpx `vp8_sub_pel_filters`
//! (`vp8/common/filter.c`).

#![forbid(unsafe_code)]

use super::tables::{
    B_HU_PRED, B_PRED, DC_PRED, H_PRED, NUM_UV_MODES, NUM_YMODES, TM_PRED, V_PRED,
};

// ---------------------------------------------------------------------------
// Inter-frame mode enumerations, continuing the intra enumeration
// (RFC 6386 §16.1-§16.2; libvpx `MB_PREDICTION_MODE` / `B_PREDICTION_MODE`).
// ---------------------------------------------------------------------------

/// First inter (motion-vector) macroblock mode. RFC 6386 continues the intra
/// `ymode` enumeration (`DC_PRED..B_PRED` = 0..4, [`NUM_YMODES`] = 5) rather
/// than starting a fresh one: `NEARESTMV = num_ymodes`.
pub const NEARESTMV: usize = NUM_YMODES;
/// "Use the second-nearest surviving candidate motion vector" mode.
pub const NEARMV: usize = NEARESTMV + 1;
/// Zero motion vector (reference the co-located block, no offset).
pub const ZEROMV: usize = NEARMV + 1;
/// Explicitly-coded ("new") motion vector.
pub const NEWMV: usize = ZEROMV + 1;
/// Macroblock split into sub-block partitions, each with its own mode/MV.
pub const SPLITMV: usize = NEWMV + 1;

/// Number of inter macroblock motion-vector reference modes
/// (`ZEROMV`/`NEARESTMV`/`NEARMV`/`NEWMV`/`SPLITMV`).
pub const NUM_MV_REFS: usize = 5;

/// First sub-block motion-vector mode for a `SPLITMV` macroblock. libvpx's
/// `B_PREDICTION_MODE` enum continues the intra 4x4 submode enumeration
/// (`B_DC_PRED..B_HU_PRED` = 0..10, [`B_HU_PRED`] = 9): `LEFT4X4 =
/// B_HU_PRED + 1`. This reuses the same per-4x4-sub-block mode storage slot
/// that intra `B_PRED` submodes use, which is why the numbering continues
/// rather than restarting at 0.
pub const LEFT4X4: usize = B_HU_PRED + 1;
/// Copy the motion vector from the sub-block above.
pub const ABOVE4X4: usize = LEFT4X4 + 1;
/// Zero motion vector for this sub-block.
pub const ZERO4X4: usize = ABOVE4X4 + 1;
/// Explicitly-coded motion vector for this sub-block.
pub const NEW4X4: usize = ZERO4X4 + 1;

/// Number of sub-block motion-vector reference modes for `SPLITMV`
/// macroblocks (`LEFT4X4`/`ABOVE4X4`/`ZERO4X4`/`NEW4X4`).
pub const NUM_SUB_MV_REFS: usize = 4;

/// `mbsplit` id: two 16x8 partitions (top half / bottom half).
pub const MBSPLIT_16X8: usize = 0;
/// `mbsplit` id: two 8x16 partitions (left half / right half).
pub const MBSPLIT_8X16: usize = 1;
/// `mbsplit` id: four 8x8 partitions (quadrants).
pub const MBSPLIT_8X8: usize = 2;
/// `mbsplit` id: sixteen 4x4 partitions (one per luma sub-block).
pub const MBSPLIT_4X4: usize = 3;

/// Number of macroblock partition (`mbsplit`) shapes (16x8/8x16/8x8/4x4).
pub const NUM_MBSPLIT_TYPES: usize = 4;

// ---------------------------------------------------------------------------
// Prediction-mode trees (RFC 6386 tree-coding convention, §8; trees
// themselves RFC 6386 §11.2/§16.1, §16.2). Same flat-`i8` representation as
// `super::tables`: a value `<= 0` at array position `i` is a leaf, encoded
// as the negation of the leaf value; a value `> 0` is the (even) index of
// the next node pair. See `BoolDecoder::read_tree_from` in `bool_decoder.rs`.
// ---------------------------------------------------------------------------

/// Tree for the five frame-level luma modes in a **non**-key frame
/// (RFC 6386 §11.2/§16.1 `ymode_tree`). Unlike [`super::tables::KF_YMODE_TREE`]
/// (which puts `B_PRED` first), this tree puts `DC_PRED` first — key frames
/// are overwhelmingly `B_PRED`, inter-frame intra macroblocks are not.
pub const YMODE_TREE: [i8; 8] = [
    -(DC_PRED as i8),
    2,
    4,
    6,
    -(V_PRED as i8),
    -(H_PRED as i8),
    -(TM_PRED as i8),
    -(B_PRED as i8),
];

/// Tree for the four chroma prediction modes (RFC 6386 §11.4/§16.1
/// `uv_mode_tree`). Bit-for-bit the same tree object RFC 6386 uses for key
/// frames ([`super::tables::UV_MODE_TREE`]) — only the driving probabilities
/// differ between key and inter frames. Re-pinned here as an independent
/// copy; see `test_uv_mode_tree_matches_keyframe_copy`.
pub const UV_MODE_TREE: [i8; 6] = [
    -(DC_PRED as i8),
    2,
    -(V_PRED as i8),
    4,
    -(H_PRED as i8),
    -(TM_PRED as i8),
];

/// Tree for the five inter macroblock motion-vector reference modes
/// (RFC 6386 §16.2 `mv_ref_tree`).
pub const MV_REF_TREE: [i8; 2 * (NUM_MV_REFS - 1)] = [
    -(ZEROMV as i8),
    2,
    -(NEARESTMV as i8),
    4,
    -(NEARMV as i8),
    6,
    -(NEWMV as i8),
    -(SPLITMV as i8),
];

/// Tree for the four `SPLITMV` sub-block motion-vector reference modes
/// (RFC 6386 §16.2 `sub_mv_ref_tree`).
pub const SUB_MV_REF_TREE: [i8; 2 * (NUM_SUB_MV_REFS - 1)] = [
    -(LEFT4X4 as i8),
    2,
    -(ABOVE4X4 as i8),
    4,
    -(ZERO4X4 as i8),
    -(NEW4X4 as i8),
];

/// Tree selecting one of the four macroblock partition shapes
/// (RFC 6386 §16.2 `mbsplit_tree`; libvpx `vp8_mbsplit_tree`). `4x4` (the
/// finest, most expensive partitioning) is the shortest path; `16x8`
/// vs. `8x16` is the deepest split.
pub const MBSPLIT_TREE: [i8; 6] = [
    -(MBSPLIT_4X4 as i8),
    2,
    -(MBSPLIT_8X8 as i8),
    4,
    -(MBSPLIT_16X8 as i8),
    -(MBSPLIT_8X16 as i8),
];

// ---------------------------------------------------------------------------
// Default mode probabilities for non-key frames (RFC 6386 §16.1).
// ---------------------------------------------------------------------------

/// Default probabilities for [`YMODE_TREE`] (RFC 6386 §16.1). Frame headers
/// may override these via `ymode_prob` updates.
pub const DEFAULT_YMODE_PROB: [u8; 4] = [112, 86, 140, 37];

/// Default probabilities for [`UV_MODE_TREE`] (RFC 6386 §16.1). Frame
/// headers may override these via `uv_mode_prob` updates.
pub const DEFAULT_UV_MODE_PROB: [u8; 3] = [162, 101, 204];

/// Context-*free* default probabilities for `super::tables::BMODE_TREE`
/// (RFC 6386 §16.1 `bmode_prob`), used for a `B_PRED` sub-block in an inter
/// frame. Unlike key frames (which always use the above/left-conditioned
/// [`super::tables::KF_BMODE_PROB`]), inter frames decode `B_PRED` submodes
/// with this single, context-independent probability set.
pub const BMODE_PROB: [u8; 9] = [120, 90, 79, 133, 87, 85, 80, 111, 151];

// ---------------------------------------------------------------------------
// Macroblock partition (mbsplit) probabilities and fill maps
// (RFC 6386 §16.2).
// ---------------------------------------------------------------------------

/// Default probabilities for [`MBSPLIT_TREE`] (RFC 6386 §16.2
/// `mbsplit_probs`).
pub const MBSPLIT_PROBS: [u8; 3] = [110, 111, 150];

/// Number of partitions for each `mbsplit` shape, indexed by `mbsplit` id
/// (RFC 6386 §16.2 `mbsplit_count`): `16x8`=2, `8x16`=2, `8x8`=4, `4x4`=16.
pub const NUM_MBSPLIT_PARTS: [usize; NUM_MBSPLIT_TYPES] = [2, 2, 4, 16];

/// Per-4x4-sub-block partition-id map for each `mbsplit` shape
/// (RFC 6386 §16.2 `mbsplit_fill` / libvpx `vp8_mbsplits`), indexed
/// `[mbsplit id][sub-block raster index 0..16]` (sub-block raster order:
/// row-major, 4 columns per row).
///
/// - `16x8` (id 0): top two sub-block rows = partition 0, bottom two = 1.
/// - `8x16` (id 1): left two sub-block columns = partition 0, right two = 1.
/// - `8x8` (id 2): four quadrants, raster order (TL, TR, BL, BR) = (0,1,2,3).
/// - `4x4` (id 3): identity — partition id == sub-block index.
pub const MBSPLITS: [[usize; 16]; NUM_MBSPLIT_TYPES] = [
    // 16x8
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1],
    // 8x16
    [0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1],
    // 8x8
    [0, 0, 1, 1, 0, 0, 1, 1, 2, 2, 3, 3, 2, 2, 3, 3],
    // 4x4
    [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
];

// ---------------------------------------------------------------------------
// Mode-context and sub-mv-ref probabilities (RFC 6386 §16.2-§16.3).
// ---------------------------------------------------------------------------

/// Near-mv-count context probabilities feeding [`MV_REF_TREE`] (RFC 6386
/// §16.3; libvpx `vp8_mode_contexts`), indexed `[neighbour-agreement count
/// 0..6][MV_REF_TREE internal node 0..4]`.
///
/// Verified against libvpx `vp8_mv_ref_probs()` (`vp8/common/findnearmv.c`):
/// each of [`MV_REF_TREE`]'s four internal-node probabilities is looked up
/// *independently*, not as one shared context row —
/// `p[i] = vp8_mode_contexts[near_mv_ref_ct[i]][i]` for `i` in `0..4`, where
/// `near_mv_ref_ct[i]` is a per-node neighbour-agreement count (0..6)
/// computed from the left/above/above-right macroblocks. That counting is
/// P2+ decode logic, out of scope here. libvpx declares the table `const
/// int`, but `vp8_mv_ref_probs()` assigns straight into a `vp8_prob p[]`
/// with no arithmetic, and every verified entry fits `u8`
/// (VP8's normal `1..=255` probability range) — so it is typed consistently
/// with this module's other probability tables.
pub const MODE_CONTEXTS: [[u8; NUM_MV_REFS - 1]; 6] = [
    [7, 1, 1, 143],
    [14, 18, 14, 107],
    [135, 64, 57, 68],
    [60, 56, 128, 65],
    [159, 134, 128, 34],
    [234, 188, 128, 28],
];

/// Context-dependent probabilities for [`SUB_MV_REF_TREE`] (RFC 6386 §16.2;
/// libvpx `vp8_sub_mv_ref_prob3`), indexed `[context 0..8][SUB_MV_REF_TREE
/// internal node 0..3]`. See the module doc comment for the `prob2`/`prob3`
/// naming note: this is libvpx's `prob3`, chosen because it is the table
/// that is actually keyed by [`sub_mv_ref_context`] and matches this
/// package's requested `[8][3]` shape.
///
/// The context is `(above_is_zero << 2) | (left_is_zero << 1) |
/// (left_eq_above)` ([`sub_mv_ref_context`]). Three of the eight index
/// combinations are logically unreachable (`left_eq_above` implies
/// `left_is_zero == above_is_zero`, so e.g. `lez=1, aez=0, lea=1` can never
/// occur): rows 3, 5 and 6 below are libvpx's filler for those slots, each a
/// duplicate of a reachable row (row 3 duplicates row 7, row 5 duplicates
/// row 1, row 6 duplicates row 4) rather than distinct data.
pub const SUB_MV_REF_PROBS2: [[u8; NUM_SUB_MV_REFS - 1]; 8] = [
    [147, 136, 18],
    [223, 1, 34],
    [106, 145, 1],
    [208, 1, 1],
    [179, 121, 1],
    [223, 1, 34],
    [179, 121, 1],
    [208, 1, 1],
];

/// Computes the [`SUB_MV_REF_PROBS2`] row index from the left/above
/// sub-block motion-vector match pattern (RFC 6386 §16.2; libvpx
/// `(aez << 2) | (lez << 1) | (lea)` in `vp8/decoder/decodemv.c`).
#[must_use]
pub const fn sub_mv_ref_context(
    left_is_zero: bool,
    above_is_zero: bool,
    left_eq_above: bool,
) -> usize {
    ((above_is_zero as usize) << 2) | ((left_is_zero as usize) << 1) | (left_eq_above as usize)
}

// ---------------------------------------------------------------------------
// Motion vector component decoding (RFC 6386 §17.2).
// ---------------------------------------------------------------------------

/// Number of "short" motion-vector magnitude codes (0..=7), i.e. the leaf
/// count of [`SMALL_MVTREE`] (RFC 6386 §17.2 `mvnum_short`).
pub const MVNUM_SHORT: usize = 8;

/// Number of bits coding a "long" motion-vector magnitude (8 and above),
/// most-significant bit first except the third-from-last (RFC 6386 §17.2
/// `mvlong_width`).
pub const MVLONG_WIDTH: usize = 10;

/// Offset of the "is long" flag probability within one [`DEFAULT_MV_CONTEXT`]
/// / [`MV_UPDATE_PROBS`] row (RFC 6386 §17.2 `mvpis_short`).
pub const MVPIS_SHORT: usize = 0;
/// Offset of the sign probability.
pub const MVPSIGN: usize = MVPIS_SHORT + 1;
/// Offset of the first of the `MVNUM_SHORT - 1` short-tree probabilities
/// (the internal-node probabilities for [`SMALL_MVTREE`]).
pub const MVPSHORT: usize = MVPSIGN + 1;
/// Offset of the first of the `MVLONG_WIDTH` long-form per-bit probabilities.
pub const MVPBITS: usize = MVPSHORT + MVNUM_SHORT - 1;
/// Total probabilities per motion-vector component: `is_short` (1) + `sign`
/// (1) + short tree (`MVNUM_SHORT - 1` = 7) + long bits (`MVLONG_WIDTH` =
/// 10) = 19 (RFC 6386 §17.2 `mvpcount`).
pub const MV_PROB_CNT: usize = MVPBITS + MVLONG_WIDTH;

/// Tree for the eight "short" motion-vector magnitudes 0..=7 (RFC 6386
/// §17.2 `small_mvtree`), driven by [`DEFAULT_MV_CONTEXT`]`[component]`
/// starting at offset [`MVPSHORT`].
pub const SMALL_MVTREE: [i8; 14] = [
    2, 8, //
    4, 6, //
    0, -1, //
    -2, -3, //
    10, 12, //
    -4, -5, //
    -6, -7,
];

/// Default motion-vector component probabilities, indexed `[component][MVP*
/// offset]` (RFC 6386 §17.2 `default_mv_context`); component 0 is the row
/// (vertical) delta, component 1 is the column (horizontal) delta.
pub const DEFAULT_MV_CONTEXT: [[u8; MV_PROB_CNT]; 2] = [
    // row
    [
        162, 128, 225, 146, 172, 147, 214, 39, 156, 128, 129, 132, 75, 145, 178, 206, 239, 254, 254,
    ],
    // column
    [
        164, 128, 204, 170, 119, 235, 140, 230, 228, 128, 130, 130, 74, 148, 180, 203, 236, 254,
        254,
    ],
];

/// Per-frame motion-vector probability *update* gate probabilities, indexed
/// `[component][MVP* offset]` (RFC 6386 §17.2 `mv_update_probs`): the
/// probability that the corresponding [`DEFAULT_MV_CONTEXT`] entry is *not*
/// replaced by a header-coded value for this frame (same
/// update-probability-not-value convention as
/// `super::tables::COEFF_UPDATE_PROBS`).
pub const MV_UPDATE_PROBS: [[u8; MV_PROB_CNT]; 2] = [
    // row
    [
        237, 246, 253, 253, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 250, 250, 252, 254,
        254,
    ],
    // column
    [
        231, 243, 245, 253, 254, 254, 254, 254, 254, 254, 254, 254, 254, 254, 251, 251, 254, 254,
        254,
    ],
];

// ---------------------------------------------------------------------------
// Sub-pixel interpolation filters (RFC 6386 §18).
// ---------------------------------------------------------------------------

/// 6-tap sub-pixel interpolation filter coefficients (RFC 6386 §18; libvpx
/// `vp8_sub_pel_filters`), indexed by eighth-pel phase `0..8`.
///
/// Salvaged verbatim from the frozen `vp8::motion::SUBPEL_FILTERS` seed
/// (`crates/oximedia-codec/src/vp8/motion.rs` around line 71): those values
/// were independently re-verified against libvpx `vp8_sub_pel_filters`
/// (`vp8/common/filter.c`) while researching this package, so they are
/// copied here rather than re-derived. Invariants (checked below): every row
/// sums to 128; row 0 is the identity `[0,0,128,0,0,0]`; row 4 is
/// palindromic; row `k` reversed equals row `8-k` for `k` in `1..=7`.
pub const SIXTAP_FILTERS: [[i32; 6]; 8] = [
    [0, 0, 128, 0, 0, 0],
    [0, -6, 123, 12, -1, 0],
    [2, -11, 108, 36, -8, 1],
    [0, -9, 93, 50, -6, 0],
    [3, -16, 77, 77, -16, 3],
    [0, -6, 50, 93, -9, 0],
    [1, -8, 36, 108, -11, 2],
    [0, -1, 12, 123, -6, 0],
];

/// Bilinear sub-pixel interpolation filter coefficients (RFC 6386 §18),
/// indexed by eighth-pel phase `0..8`: `[128 - 16*x, 16*x]`. Used instead of
/// [`SIXTAP_FILTERS`] when the frame header selects the "bilinear" (rather
/// than "sixtap") reconstruction filter.
pub const BILINEAR_FILTERS: [[i32; 2]; 8] = [
    [128, 0],
    [112, 16],
    [96, 32],
    [80, 48],
    [64, 64],
    [48, 80],
    [32, 96],
    [16, 112],
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Cross-module access to the already-pinned key-frame tables, used for
    /// the `validate_tree` calibration tests and the `UV_MODE_TREE` drift
    /// check.
    use super::super::tables as kf_tables;

    // -- shared helpers -------------------------------------------------

    /// Validates a VP8 tree-coding array (RFC 6386 §8 `treed_read`
    /// convention, matching `BoolDecoder::read_tree_from` in
    /// `bool_decoder.rs`): a flat `i8` array of node pairs, where node `i`
    /// (always even) has its children at `tree[i]` / `tree[i + 1]`; a value
    /// `<= 0` is a leaf encoded as the negation of the leaf value (so leaf
    /// `0` is stored as plain `0`); a value `> 0` is the even array index of
    /// the next node pair.
    ///
    /// Walks the tree from the root (index 0) and checks: the array length
    /// matches a full binary tree with `expected_leaves` leaves (`2 *
    /// (expected_leaves - 1)`); every node pair is reachable exactly once
    /// (no cycles, no shared parents, no unreachable/dangling pairs); every
    /// positive entry is an in-bounds even node-pair index; and exactly
    /// `expected_leaves` distinct leaf values are produced across exactly
    /// `expected_leaves` leaf slots.
    fn validate_tree(tree: &[i8], expected_leaves: usize) {
        assert!(expected_leaves >= 2, "a tree needs at least 2 leaves");
        assert_eq!(
            tree.len(),
            2 * (expected_leaves - 1),
            "tree length must be 2*(expected_leaves-1) for a full binary tree over {expected_leaves} leaves, got length {}",
            tree.len()
        );

        let mut visited = vec![false; tree.len()];
        let mut leaves: Vec<i32> = Vec::new();
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            assert!(i % 2 == 0, "node-pair index {i} must be even");
            assert!(
                i < tree.len(),
                "node-pair index {i} out of bounds (len {})",
                tree.len()
            );
            assert!(
                !visited[i],
                "cycle or shared parent detected at node-pair index {i}"
            );
            visited[i] = true;

            for child in [tree[i], tree[i + 1]] {
                if child > 0 {
                    let next = child as usize;
                    assert!(
                        next % 2 == 0,
                        "internal-node target {next} must be an even index"
                    );
                    assert!(
                        next < tree.len(),
                        "internal-node target {next} out of bounds (len {})",
                        tree.len()
                    );
                    stack.push(next);
                } else {
                    leaves.push(-i32::from(child));
                }
            }
        }

        assert!(
            visited.iter().step_by(2).all(|&v| v),
            "tree has unreachable node-pair(s): {visited:?}"
        );

        assert_eq!(
            leaves.len(),
            expected_leaves,
            "expected {expected_leaves} leaf slots, found {} ({leaves:?})",
            leaves.len()
        );
        let distinct: BTreeSet<i32> = leaves.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            expected_leaves,
            "expected {expected_leaves} distinct leaf values, found {distinct:?} (slots: {leaves:?})"
        );
    }

    /// Every probability in a VP8 probability table must be in `1..=255`:
    /// `0` would collapse the boolean decoder's coding interval to zero
    /// width (RFC 6386 §7.3).
    fn assert_probs_in_range(probs: &[u8]) {
        for (i, &p) in probs.iter().enumerate() {
            assert!(
                (1..=255).contains(&p),
                "probability at index {i} is {p}, must be in 1..=255"
            );
        }
    }

    // -- 0. validate_tree calibration against already-pinned trees ------
    //
    // Confirms this package's validate_tree agrees with the convention the
    // existing keyframe tables already use, before trusting it on the new
    // inter-frame trees above.

    #[test]
    fn test_validate_tree_calibrates_against_kf_ymode_tree() {
        validate_tree(&kf_tables::KF_YMODE_TREE, NUM_YMODES);
    }

    #[test]
    fn test_validate_tree_calibrates_against_kf_uv_mode_tree() {
        validate_tree(&kf_tables::UV_MODE_TREE, NUM_UV_MODES);
    }

    #[test]
    fn test_validate_tree_calibrates_against_bmode_tree() {
        validate_tree(&kf_tables::BMODE_TREE, 10);
    }

    #[test]
    fn test_validate_tree_calibrates_against_coeff_tree() {
        // 12 distinct DCT token leaves: DCT_0..DCT_4 (5), CAT1..CAT6 (6), EOB (1).
        validate_tree(&kf_tables::COEFF_TREE, 12);
    }

    // -- 1. tree validity for every new tree -----------------------------

    #[test]
    fn test_ymode_tree_valid() {
        validate_tree(&YMODE_TREE, NUM_YMODES);
    }

    #[test]
    fn test_uv_mode_tree_valid() {
        validate_tree(&UV_MODE_TREE, NUM_UV_MODES);
    }

    #[test]
    fn test_uv_mode_tree_matches_keyframe_copy() {
        // RFC 6386's uv_mode_tree is literally the same tree for key frames
        // and inter frames (only the driving probabilities differ) -- pin
        // both copies equal so they cannot silently drift apart.
        assert_eq!(UV_MODE_TREE, kf_tables::UV_MODE_TREE);
    }

    #[test]
    fn test_mv_ref_tree_valid() {
        validate_tree(&MV_REF_TREE, NUM_MV_REFS);
    }

    #[test]
    fn test_sub_mv_ref_tree_valid() {
        validate_tree(&SUB_MV_REF_TREE, NUM_SUB_MV_REFS);
    }

    #[test]
    fn test_mbsplit_tree_valid() {
        validate_tree(&MBSPLIT_TREE, NUM_MBSPLIT_TYPES);
    }

    #[test]
    fn test_small_mvtree_valid() {
        validate_tree(&SMALL_MVTREE, MVNUM_SHORT);
    }

    // -- 2. every probability in every prob table in 1..=255 -------------

    #[test]
    fn test_default_ymode_prob_in_range() {
        assert_probs_in_range(&DEFAULT_YMODE_PROB);
    }

    #[test]
    fn test_default_uv_mode_prob_in_range() {
        assert_probs_in_range(&DEFAULT_UV_MODE_PROB);
    }

    #[test]
    fn test_bmode_prob_in_range() {
        assert_probs_in_range(&BMODE_PROB);
    }

    #[test]
    fn test_mbsplit_probs_in_range() {
        assert_probs_in_range(&MBSPLIT_PROBS);
    }

    #[test]
    fn test_mode_contexts_in_range() {
        for row in &MODE_CONTEXTS {
            assert_probs_in_range(row);
        }
    }

    #[test]
    fn test_sub_mv_ref_probs2_in_range() {
        for row in &SUB_MV_REF_PROBS2 {
            assert_probs_in_range(row);
        }
    }

    #[test]
    fn test_default_mv_context_in_range() {
        for row in &DEFAULT_MV_CONTEXT {
            assert_probs_in_range(row);
        }
    }

    #[test]
    fn test_mv_update_probs_in_range() {
        for row in &MV_UPDATE_PROBS {
            assert_probs_in_range(row);
        }
    }

    // -- 3. SIXTAP / BILINEAR filter structural invariants ---------------

    #[test]
    fn test_sixtap_rows_sum_to_128() {
        for (i, row) in SIXTAP_FILTERS.iter().enumerate() {
            let sum: i32 = row.iter().sum();
            assert_eq!(
                sum, 128,
                "SIXTAP_FILTERS row {i} sums to {sum}, expected 128"
            );
        }
    }

    #[test]
    fn test_sixtap_row0_is_identity() {
        assert_eq!(SIXTAP_FILTERS[0], [0, 0, 128, 0, 0, 0]);
    }

    #[test]
    fn test_sixtap_row4_is_palindromic() {
        let row = SIXTAP_FILTERS[4];
        let mut reversed = row;
        reversed.reverse();
        assert_eq!(row, reversed, "SIXTAP_FILTERS row 4 must be palindromic");
    }

    #[test]
    fn test_sixtap_mirror_property() {
        // Verified by hand against the actual SIXTAP_FILTERS values before
        // writing this assertion (see the module doc comment's provenance
        // section): it holds. If it ever fails, the table's values are
        // suspect -- do not weaken this test to make it pass; stop and
        // report instead.
        for k in 1..=7usize {
            let mut reversed = SIXTAP_FILTERS[k];
            reversed.reverse();
            assert_eq!(
                reversed,
                SIXTAP_FILTERS[8 - k],
                "row {k} reversed must equal row {}",
                8 - k
            );
        }
    }

    #[test]
    fn test_bilinear_rows_sum_to_128() {
        for (i, row) in BILINEAR_FILTERS.iter().enumerate() {
            let sum: i32 = row.iter().sum();
            assert_eq!(
                sum, 128,
                "BILINEAR_FILTERS row {i} sums to {sum}, expected 128"
            );
        }
    }

    #[test]
    fn test_bilinear_row0_is_identity() {
        assert_eq!(BILINEAR_FILTERS[0], [128, 0]);
    }

    #[test]
    fn test_bilinear_mirror_property() {
        for k in 1..=7usize {
            let mut reversed = BILINEAR_FILTERS[k];
            reversed.reverse();
            assert_eq!(
                reversed,
                BILINEAR_FILTERS[8 - k],
                "row {k} reversed must equal row {}",
                8 - k
            );
        }
    }

    // -- 4. MBSPLITS structural invariants -------------------------------

    #[test]
    fn test_mbsplits_partition_ids_contiguous_and_balanced() {
        for (i, row) in MBSPLITS.iter().enumerate() {
            let parts = NUM_MBSPLIT_PARTS[i];
            let distinct: BTreeSet<usize> = row.iter().copied().collect();
            assert_eq!(
                distinct.len(),
                parts,
                "MBSPLITS[{i}] uses {} distinct ids, expected {parts}",
                distinct.len()
            );
            let expected_ids: BTreeSet<usize> = (0..parts).collect();
            assert_eq!(
                distinct, expected_ids,
                "MBSPLITS[{i}] ids must cover 0..{parts} contiguously"
            );

            let expected_count = 16 / parts;
            for id in 0..parts {
                let count = row.iter().filter(|&&v| v == id).count();
                assert_eq!(
                    count, expected_count,
                    "MBSPLITS[{i}] id {id} appears {count} times, expected {expected_count}"
                );
            }
        }
    }

    #[test]
    fn test_mbsplits_4x4_is_identity() {
        let expected: [usize; 16] = core::array::from_fn(|i| i);
        assert_eq!(MBSPLITS[MBSPLIT_4X4], expected);
    }

    // -- 5. cross-check: MBSPLIT_TREE leaf count == MBSPLITS.len() -------

    #[test]
    fn test_mbsplit_tree_leaf_count_matches_mbsplits_len() {
        assert_eq!(MBSPLITS.len(), NUM_MBSPLIT_TYPES);
        assert_eq!(NUM_MBSPLIT_PARTS.len(), NUM_MBSPLIT_TYPES);
        validate_tree(&MBSPLIT_TREE, MBSPLITS.len());
    }

    // -- extra: internal consistency of the §17.2 layout constants -------

    #[test]
    fn test_mv_prob_layout_constants_consistent() {
        assert_eq!(MVPIS_SHORT, 0);
        assert_eq!(MVPSIGN, 1);
        assert_eq!(MVPSHORT, 2);
        assert_eq!(MVPBITS, MVPSHORT + MVNUM_SHORT - 1);
        assert_eq!(MV_PROB_CNT, MVPBITS + MVLONG_WIDTH);
        assert_eq!(MV_PROB_CNT, 19);
        assert_eq!(DEFAULT_MV_CONTEXT[0].len(), MV_PROB_CNT);
        assert_eq!(DEFAULT_MV_CONTEXT[1].len(), MV_PROB_CNT);
        assert_eq!(MV_UPDATE_PROBS[0].len(), MV_PROB_CNT);
        assert_eq!(MV_UPDATE_PROBS[1].len(), MV_PROB_CNT);
    }

    #[test]
    fn test_sub_mv_ref_context_covers_all_eight_indices() {
        let mut seen = [false; 8];
        for lez in [false, true] {
            for aez in [false, true] {
                for lea in [false, true] {
                    let idx = sub_mv_ref_context(lez, aez, lea);
                    assert!(
                        idx < 8,
                        "sub_mv_ref_context returned out-of-range index {idx}"
                    );
                    seen[idx] = true;
                }
            }
        }
        assert!(
            seen.iter().all(|&s| s),
            "sub_mv_ref_context must cover all 8 indices: {seen:?}"
        );
    }
}
