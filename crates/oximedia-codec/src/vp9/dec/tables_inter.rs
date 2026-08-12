//! VP9 inter-frame constant tables — sub-pixel interpolation kernels, motion
//! vector entropy tables, inter-mode / interpolation-filter / reference-frame
//! trees and their default probabilities, motion-vector reference candidate
//! search tables, non-keyframe intra-mode and partition probabilities, and
//! the loop-filter mode lookup.
//!
//! Companion to [`super::tables`] (the intra / key-frame normative tables):
//! this module pins the additional constants a VP9 **inter** frame needs on
//! top of the intra set — including the *non-keyframe* counterparts of
//! several tables [`super::tables`] already carries a key-frame version of
//! ([`DEFAULT_Y_MODE_PROBS`] vs `KF_Y_MODE_PROBS`, [`DEFAULT_UV_MODE_PROBS`]
//! vs `KF_UV_MODE_PROBS`, [`DEFAULT_PARTITION_PROBS`] vs
//! `KF_PARTITION_PROBS`); the two sets are numerically distinct and neither
//! substitutes for the other. It is a **data-only** module — constant
//! tables plus their structural invariant tests, no decode logic — so it
//! contains no fallible operations at all.
//!
//! # Provenance
//!
//! Every table below is transcribed verbatim from the libvpx reference
//! implementation, tag **v1.15.2** (commit `d168454`), which is the encoder
//! and reference decoder used to produce the Wave-3 fixtures. Per-table
//! `libvpx <file>:<lines>` comments give the exact source location. Every
//! citation in this file (both the tables already present and the tables
//! added alongside this note) has been re-verified line-for-line against a
//! direct fetch of `refs/heads/main` of `webmproject/libvpx`; VP9's
//! normative default tables have not changed between that tag and `main`.
//!
//! Where the *VP9 Bitstream & Decoding Process Specification v0.7*
//! (22nd February 2017) states the same constants, the specification is
//! cited as an independent cross-check. The interpolation kernels in
//! particular were verified against **both** sources: the specification's
//! `subpel_filters[4][16][8]` (§8.5.2.3, "Motion vector scaling process")
//! lists the four families in exactly the order used here, which
//! independently corroborates the libvpx symbol-to-family mapping below.
//! The tables added without a specification citation have not been
//! independently cross-checked against the specification text — only
//! against libvpx source, with exact file:line citations.
//!
//! # Filter family naming
//!
//! libvpx's symbol suffixes do not spell out "smooth" and "sharp", and the
//! two are easy to transpose. The mapping is pinned by two independent
//! facts in the libvpx tree:
//!
//! * `vp9/common/vp9_filter.h:23-27` defines the bitstream filter indices
//!   `EIGHTTAP 0`, `EIGHTTAP_SMOOTH 1`, `EIGHTTAP_SHARP 2`, `BILINEAR 3`.
//! * `vp9/common/vp9_filter.c:79-82` defines the lookup table indexed by
//!   those constants:
//!   `vp9_filter_kernels[5] = { sub_pel_filters_8, sub_pel_filters_8lp,
//!   sub_pel_filters_8s, bilinear_filters, sub_pel_filters_4 }`.
//!
//! Composing the two: index 1 is `EIGHTTAP_SMOOTH` **and** `sub_pel_filters_8lp`,
//! index 2 is `EIGHTTAP_SHARP` **and** `sub_pel_filters_8s`. So the suffix
//! `8lp` ("low-pass", carrying the source comment `// freqmultiplier = 0.5`)
//! is the **smooth** family, and `8s` (source comment `// DCT based filter`)
//! is the **sharp** family. [`EIGHTTAP_SMOOTH`] and [`EIGHTTAP_SHARP`] below
//! follow that mapping, and [`FILTER_KERNELS`] reproduces the index order so
//! a bitstream `interp_filter` value indexes it directly.
//!
//! Note that the *frame-level header* literal is a different, permuted
//! encoding: `vp9/decoder/vp9_decodeframe.c:1456-1461` maps the two-bit
//! `raw_interpolation_filter` through
//! `literal_to_filter[] = { EIGHTTAP_SMOOTH, EIGHTTAP, EIGHTTAP_SHARP, BILINEAR }`.
//! That permutation belongs to header parsing, not to this table, and is
//! pinned here as [`LITERAL_TO_FILTER`] so the two encodings cannot be
//! confused.
//!
//! `sub_pel_filters_4` (`FOURTAP`, index 4) is deliberately **not** included:
//! it is not reachable from any VP9 bitstream syntax element — the
//! `interp_filter` range is 0..3 (`SWITCHABLE_FILTERS 3`, `BILINEAR 3`) — and
//! is used only by non-normative encoder-side search code.
//!
//! # Tree encoding
//!
//! Trees are libvpx `vpx_tree_index` arrays in the layout
//! [`super::booldec::BoolReader::read_tree`] consumes: a non-positive entry
//! `e` is the leaf token `-e`, a positive entry is the index of the next
//! node pair. A leaf for token 0 is therefore written `0` (libvpx writes it
//! `-TOKEN` where `TOKEN == 0`); this is unambiguous because index 0 is the
//! root and is never a branch target. This matches the encoding already used
//! by [`super::tables`] (for example `PARTITION_TREE`).

// ---------------------------------------------------------------------------
// Sub-pixel interpolation kernels
// ---------------------------------------------------------------------------

/// Number of sub-pixel phases per interpolation filter family.
///
/// libvpx `vpx_dsp/vpx_filter.h:24-26`: `SUBPEL_BITS 4`,
/// `SUBPEL_SHIFTS (1 << SUBPEL_BITS)`. Specification §2 ("Symbols and
/// abbreviated terms") likewise lists `SUBPEL_SHIFTS 16`, `SUBPEL_MASK 15`.
///
/// The bitstream selects the phase with the low four bits of the
/// eighth-pel motion vector scaled to 1/16 pel (`p & 15` in the
/// specification's prediction pseudo-code), so all 16 rows are reachable.
pub const SUBPEL_SHIFTS: usize = 16;

/// Number of taps in each interpolation kernel.
///
/// libvpx `vpx_dsp/vpx_filter.h:27`: `SUBPEL_TAPS 8`.
pub const SUBPEL_TAPS: usize = 8;

/// Fixed-point precision of the interpolation kernels.
///
/// libvpx `vpx_dsp/vpx_filter.h:22`: `FILTER_BITS 7`, hence every kernel row
/// sums to `1 << 7 == 128`.
pub const FILTER_BITS: u32 = 7;

/// Sum every interpolation kernel row must have (`1 << FILTER_BITS`).
pub const FILTER_WEIGHT: i32 = 128;

/// One interpolation filter family: 16 sub-pixel phases of 8 taps each.
///
/// libvpx `vpx_dsp/vpx_filter.h:29`: `typedef int16_t InterpKernel[SUBPEL_TAPS]`.
pub type InterpKernels = [[i16; SUBPEL_TAPS]; SUBPEL_SHIFTS];

/// `EIGHTTAP` (bitstream filter 0) — the regular 8-tap family.
///
/// libvpx `vp9/common/vp9_filter.c:28-38`, symbol `sub_pel_filters_8`,
/// source comment `// Lagrangian interpolation filter`.
/// Specification §8.5.2.3 `subpel_filters[0]`.
#[rustfmt::skip]
pub const EIGHTTAP: InterpKernels = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 1, -5, 126, 8, -3, 1, 0],
    [-1, 3, -10, 122, 18, -6, 2, 0],
    [-1, 4, -13, 118, 27, -9, 3, -1],
    [-1, 4, -16, 112, 37, -11, 4, -1],
    [-1, 5, -18, 105, 48, -14, 4, -1],
    [-1, 5, -19, 97, 58, -16, 5, -1],
    [-1, 6, -19, 88, 68, -18, 5, -1],
    [-1, 6, -19, 78, 78, -19, 6, -1],
    [-1, 5, -18, 68, 88, -19, 6, -1],
    [-1, 5, -16, 58, 97, -19, 5, -1],
    [-1, 4, -14, 48, 105, -18, 5, -1],
    [-1, 4, -11, 37, 112, -16, 4, -1],
    [-1, 3, -9, 27, 118, -13, 4, -1],
    [0, 2, -6, 18, 122, -10, 3, -1],
    [0, 1, -3, 8, 126, -5, 1, 0],
];

/// `EIGHTTAP_SMOOTH` (bitstream filter 1) — the low-pass 8-tap family.
///
/// libvpx `vp9/common/vp9_filter.c:54-64`, symbol `sub_pel_filters_8lp`,
/// source comment `// freqmultiplier = 0.5`.
/// Specification §8.5.2.3 `subpel_filters[1]`.
///
/// See the module-level "Filter family naming" note: the `8lp` suffix is the
/// **smooth** family, established by `vp9_filter_kernels[1]` sitting at the
/// `EIGHTTAP_SMOOTH == 1` index.
#[rustfmt::skip]
pub const EIGHTTAP_SMOOTH: InterpKernels = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [-3, -1, 32, 64, 38, 1, -3, 0],
    [-2, -2, 29, 63, 41, 2, -3, 0],
    [-2, -2, 26, 63, 43, 4, -4, 0],
    [-2, -3, 24, 62, 46, 5, -4, 0],
    [-2, -3, 21, 60, 49, 7, -4, 0],
    [-1, -4, 18, 59, 51, 9, -4, 0],
    [-1, -4, 16, 57, 53, 12, -4, -1],
    [-1, -4, 14, 55, 55, 14, -4, -1],
    [-1, -4, 12, 53, 57, 16, -4, -1],
    [0, -4, 9, 51, 59, 18, -4, -1],
    [0, -4, 7, 49, 60, 21, -3, -2],
    [0, -4, 5, 46, 62, 24, -3, -2],
    [0, -4, 4, 43, 63, 26, -2, -2],
    [0, -3, 2, 41, 63, 29, -2, -2],
    [0, -3, 1, 38, 64, 32, -1, -3],
];

/// `EIGHTTAP_SHARP` (bitstream filter 2) — the sharp 8-tap family.
///
/// libvpx `vp9/common/vp9_filter.c:41-51`, symbol `sub_pel_filters_8s`,
/// source comment `// DCT based filter`.
/// Specification §8.5.2.3 `subpel_filters[2]`.
///
/// See the module-level "Filter family naming" note: the `8s` suffix is the
/// **sharp** family, established by `vp9_filter_kernels[2]` sitting at the
/// `EIGHTTAP_SHARP == 2` index.
#[rustfmt::skip]
pub const EIGHTTAP_SHARP: InterpKernels = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [-1, 3, -7, 127, 8, -3, 1, 0],
    [-2, 5, -13, 125, 17, -6, 3, -1],
    [-3, 7, -17, 121, 27, -10, 5, -2],
    [-4, 9, -20, 115, 37, -13, 6, -2],
    [-4, 10, -23, 108, 48, -16, 8, -3],
    [-4, 10, -24, 100, 59, -19, 9, -3],
    [-4, 11, -24, 90, 70, -21, 10, -4],
    [-4, 11, -23, 80, 80, -23, 11, -4],
    [-4, 10, -21, 70, 90, -24, 11, -4],
    [-3, 9, -19, 59, 100, -24, 10, -4],
    [-3, 8, -16, 48, 108, -23, 10, -4],
    [-2, 6, -13, 37, 115, -20, 9, -4],
    [-2, 5, -10, 27, 121, -17, 7, -3],
    [-1, 3, -6, 17, 125, -13, 5, -2],
    [0, 1, -3, 8, 127, -7, 3, -1],
];

/// `BILINEAR` (bitstream filter 3) — the bilinear family, expressed in the
/// same 8-tap layout as the others.
///
/// libvpx `vp9/common/vp9_filter.c:15-25`, symbol `bilinear_filters`.
/// Specification §8.5.2.3 `subpel_filters[3]`.
///
/// The two nonzero taps sit at positions 3 and 4 so that a single 8-tap
/// convolution kernel serves all four families without a shape special-case;
/// row `k` is `[0, 0, 0, 128 - 8k, 8k, 0, 0, 0]`.
#[rustfmt::skip]
pub const BILINEAR: InterpKernels = [
    [0, 0, 0, 128, 0, 0, 0, 0],
    [0, 0, 0, 120, 8, 0, 0, 0],
    [0, 0, 0, 112, 16, 0, 0, 0],
    [0, 0, 0, 104, 24, 0, 0, 0],
    [0, 0, 0, 96, 32, 0, 0, 0],
    [0, 0, 0, 88, 40, 0, 0, 0],
    [0, 0, 0, 80, 48, 0, 0, 0],
    [0, 0, 0, 72, 56, 0, 0, 0],
    [0, 0, 0, 64, 64, 0, 0, 0],
    [0, 0, 0, 56, 72, 0, 0, 0],
    [0, 0, 0, 48, 80, 0, 0, 0],
    [0, 0, 0, 40, 88, 0, 0, 0],
    [0, 0, 0, 32, 96, 0, 0, 0],
    [0, 0, 0, 24, 104, 0, 0, 0],
    [0, 0, 0, 16, 112, 0, 0, 0],
    [0, 0, 0, 8, 120, 0, 0, 0],
];

/// The four interpolation filter families indexed by the bitstream
/// `interp_filter` value.
///
/// libvpx `vp9/common/vp9_filter.c:79-82` (`vp9_filter_kernels`, first four
/// entries) with the indices of `vp9/common/vp9_filter.h:23-27`.
/// Specification §8.5.2.3 `subpel_filters[4][16][8]`, same order.
#[rustfmt::skip]
pub const FILTER_KERNELS: [InterpKernels; 4] =
    [EIGHTTAP, EIGHTTAP_SMOOTH, EIGHTTAP_SHARP, BILINEAR];

/// Number of switchable filters, i.e. the number of values `interp_filter`
/// can take when the frame header selects per-block filter signalling.
///
/// libvpx `vp9/common/vp9_filter.h:26`: `SWITCHABLE_FILTERS 3`.
pub const SWITCHABLE_FILTERS: usize = 3;

/// Number of contexts for the switchable-filter syntax element.
///
/// libvpx `vp9/common/vp9_filter.h:31`:
/// `SWITCHABLE_FILTER_CONTEXTS (SWITCHABLE_FILTERS + 1)`.
pub const SWITCHABLE_FILTER_CONTEXTS: usize = SWITCHABLE_FILTERS + 1;

/// Frame-header two-bit literal to filter index mapping.
///
/// libvpx `vp9/decoder/vp9_decodeframe.c:1457-1458`:
/// `literal_to_filter[] = { EIGHTTAP_SMOOTH, EIGHTTAP, EIGHTTAP_SHARP, BILINEAR }`.
/// Specification §7.2.7 gives the same permutation for
/// `raw_interpolation_filter`.
///
/// This is **not** the same order as [`FILTER_KERNELS`]; it applies only to
/// the frame-level `interpolation_filter` syntax element.
#[rustfmt::skip]
pub const LITERAL_TO_FILTER: [u8; 4] = [1, 0, 2, 3];

// ---------------------------------------------------------------------------
// Inter mode syntax
// ---------------------------------------------------------------------------

/// Number of inter prediction modes (`NEARESTMV`, `NEARMV`, `ZEROMV`,
/// `NEWMV`).
///
/// libvpx `vp9/common/vp9_enums.h:129`: `INTER_MODES (1 + NEWMV - NEARESTMV)`.
pub const INTER_MODES: usize = 4;

/// Number of contexts for the inter-mode syntax element.
///
/// libvpx `vp9/common/vp9_enums.h:132`: `INTER_MODE_CONTEXTS 7`.
pub const INTER_MODE_CONTEXTS: usize = 7;

/// Inter mode token values, i.e. `INTER_OFFSET(mode)`.
///
/// libvpx `vp9/common/vp9_enums.h:120-123` numbers the modes
/// `NEARESTMV 10`, `NEARMV 11`, `ZEROMV 12`, `NEWMV 13`, and
/// `vp9/common/vp9_entropymode.h:27` defines
/// `INTER_OFFSET(mode) ((mode) - NEARESTMV)`. The tokens carried by
/// [`INTER_MODE_TREE`] are therefore these offsets, not the raw mode numbers.
///
/// **This offset numbering (0..=3) is not the numbering
/// [`MODE_2_COUNTER`] and [`MODE_LF_LUT`] use** — those two are indexed by
/// the raw `MB_MODE_COUNT` mode number (10..=13 for the same four modes).
/// See the doc comment on [`MODE_2_COUNTER`].
pub const INTER_MODE_NEARESTMV: u8 = 0;
/// See [`INTER_MODE_NEARESTMV`].
pub const INTER_MODE_NEARMV: u8 = 1;
/// See [`INTER_MODE_NEARESTMV`].
pub const INTER_MODE_ZEROMV: u8 = 2;
/// See [`INTER_MODE_NEARESTMV`].
pub const INTER_MODE_NEWMV: u8 = 3;

/// Inter mode tree.
///
/// libvpx `vp9/common/vp9_entropymode.c:257-260`:
/// ```text
/// const vpx_tree_index vp9_inter_mode_tree[TREE_SIZE(INTER_MODES)] = {
///   -INTER_OFFSET(ZEROMV), 2, -INTER_OFFSET(NEARESTMV), 4,
///   -INTER_OFFSET(NEARMV), -INTER_OFFSET(NEWMV)
/// };
/// ```
/// Resolving `INTER_OFFSET` (see [`INTER_MODE_NEARESTMV`]) gives
/// `{ -2, 2, -0, 4, -1, -3 }`, written below with `-0` as `0`.
/// Specification §9.3.1 `inter_mode_tree` is identical.
///
/// **The leaf order is not the mode enumeration order**: `ZEROMV` is the
/// first leaf and `NEARESTMV` the second. Reordering these to match the
/// enum silently corrupts every inter-mode decode.
#[rustfmt::skip]
pub const INTER_MODE_TREE: [i8; 6] = [-2, 2, 0, 4, -1, -3];

/// Default inter-mode probabilities, indexed by mode context then tree node.
///
/// libvpx `vp9/common/vp9_entropymode.c:233-242`
/// (`default_inter_mode_probs[INTER_MODE_CONTEXTS][INTER_MODES - 1]`).
/// Specification §10.5 `default_inter_mode_probs`.
///
/// The row comments are libvpx's own and describe the neighbourhood the
/// context counter encodes. The context index itself is derived by
/// [`MODE_2_COUNTER`] and [`COUNTER_TO_CONTEXT`] below.
#[rustfmt::skip]
pub const DEFAULT_INTER_MODE_PROBS: [[u8; INTER_MODES - 1]; INTER_MODE_CONTEXTS] = [
    [2, 173, 34],  // 0 = both zero mv
    [7, 145, 85],  // 1 = one zero mv + one a predicted mv
    [7, 166, 63],  // 2 = two predicted mvs
    [7, 94, 66],   // 3 = one predicted/zero and one new mv
    [8, 64, 46],   // 4 = two new mvs
    [17, 81, 31],  // 5 = one intra neighbour + x
    [25, 29, 30],  // 6 = two intra neighbours
];

/// Switchable interpolation-filter tree.
///
/// libvpx `vp9/common/vp9_entropymode.c:337-338`:
/// `{ -EIGHTTAP, 2, -EIGHTTAP_SMOOTH, -EIGHTTAP_SHARP }`, i.e.
/// `{ -0, 2, -1, -2 }` with the filter indices of
/// `vp9/common/vp9_filter.h:23-25`.
/// Specification §9.3.1 `interp_filter_tree`.
#[rustfmt::skip]
pub const SWITCHABLE_INTERP_TREE: [i8; 4] = [0, 2, -1, -2];

/// Default switchable interpolation-filter probabilities.
///
/// libvpx `vp9/common/vp9_entropymode.c:315-321`
/// (`default_switchable_interp_prob[SWITCHABLE_FILTER_CONTEXTS][SWITCHABLE_FILTERS - 1]`).
/// Specification §10.5 `default_interp_filter_probs`.
#[rustfmt::skip]
pub const DEFAULT_SWITCHABLE_INTERP_PROBS: [[u8; SWITCHABLE_FILTERS - 1];
    SWITCHABLE_FILTER_CONTEXTS] = [[235, 162], [36, 255], [34, 3], [149, 144]];

// ---------------------------------------------------------------------------
// Motion-vector reference candidate search
// ---------------------------------------------------------------------------
//
// These three tables work together to find a block's motion-vector
// predictors and to pick which row of `DEFAULT_INTER_MODE_PROBS` decodes its
// inter mode: `MV_REF_BLOCKS` lists the neighbour positions to probe for a
// given block size, `MODE_2_COUNTER` turns each probed neighbour's mode into
// a weighted vote, and `COUNTER_TO_CONTEXT` turns the summed vote into a
// context index.

/// Candidate neighbour positions scanned when building the motion-vector
/// reference list, indexed by block size then candidate slot.
///
/// libvpx `vp9/common/vp9_mvref_common.h:89-207` (`mv_ref_blocks`), with the
/// `POSITION { int row; int col; }` struct of `vp9_mvref_common.h:26-29`
/// (`MVREF_NEIGHBOURS 8` at line 24) flattened to `[row, col]`. **Row is
/// index 0, column is index 1** — transposing the pair is invisible until
/// fixture comparison (see the test below, which pins an asymmetric entry).
///
/// Row order is libvpx's `BLOCK_SIZE` enumeration (`vp9_enums.h:46-58`):
/// 4X4, 4X8, 8X4, 8X8, 8X16, 16X8, 16X16, 16X32, 32X16, 32X32, 32X64, 64X32,
/// 64X64 — the same order [`super::tables::NUM_4X4_BLOCKS_WIDE`] and its
/// siblings use.
#[rustfmt::skip]
pub const MV_REF_BLOCKS: [[[i8; 2]; 8]; 13] = [
    // 4X4
    [[-1, 0], [0, -1], [-1, -1], [-2, 0], [0, -2], [-2, -1], [-1, -2], [-2, -2]],
    // 4X8
    [[-1, 0], [0, -1], [-1, -1], [-2, 0], [0, -2], [-2, -1], [-1, -2], [-2, -2]],
    // 8X4
    [[-1, 0], [0, -1], [-1, -1], [-2, 0], [0, -2], [-2, -1], [-1, -2], [-2, -2]],
    // 8X8
    [[-1, 0], [0, -1], [-1, -1], [-2, 0], [0, -2], [-2, -1], [-1, -2], [-2, -2]],
    // 8X16
    [[0, -1], [-1, 0], [1, -1], [-1, -1], [0, -2], [-2, 0], [-2, -1], [-1, -2]],
    // 16X8
    [[-1, 0], [0, -1], [-1, 1], [-1, -1], [-2, 0], [0, -2], [-1, -2], [-2, -1]],
    // 16X16
    [[-1, 0], [0, -1], [-1, 1], [1, -1], [-1, -1], [-3, 0], [0, -3], [-3, -3]],
    // 16X32
    [[0, -1], [-1, 0], [2, -1], [-1, -1], [-1, 1], [0, -3], [-3, 0], [-3, -3]],
    // 32X16
    [[-1, 0], [0, -1], [-1, 2], [-1, -1], [1, -1], [-3, 0], [0, -3], [-3, -3]],
    // 32X32
    [[-1, 1], [1, -1], [-1, 2], [2, -1], [-1, -1], [-3, 0], [0, -3], [-3, -3]],
    // 32X64
    [[0, -1], [-1, 0], [4, -1], [-1, 2], [-1, -1], [0, -3], [-3, 0], [2, -1]],
    // 64X32
    [[-1, 0], [0, -1], [-1, 4], [2, -1], [-1, -1], [-3, 0], [0, -3], [-1, 2]],
    // 64X64
    [[-1, 3], [3, -1], [-1, 4], [4, -1], [-1, -1], [-1, 0], [0, -1], [-1, 6]],
];

/// Maps a neighbour's prediction mode to a weighted vote used to derive the
/// inter-mode context: 9 for any intra mode, 0 for `NEARESTMV`/`NEARMV`
/// (a "predicted" mv), 3 for `ZEROMV`, 1 for `NEWMV`. Two neighbours' votes
/// are summed (0..=18) and looked up in [`COUNTER_TO_CONTEXT`].
///
/// libvpx `vp9/common/vp9_mvref_common.h:47-62` (`mode_2_counter`).
/// **Indexed by the raw `MB_MODE_COUNT` mode number** (`vp9_enums.h:110-124`:
/// `DC_PRED..TM_PRED` = 0..=9, `NEARESTMV` = 10, `NEARMV` = 11, `ZEROMV` = 12,
/// `NEWMV` = 13) — *not* by [`INTER_MODE_NEARESTMV`] and its siblings, which
/// are `INTER_OFFSET`-relative (0..=3). The two numbering schemes coexist in
/// this module; do not subtract `NEARESTMV` twice.
#[rustfmt::skip]
pub const MODE_2_COUNTER: [u8; 14] = [
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, // DC_PRED .. TM_PRED (all intra modes)
    0,                            // NEARESTMV
    0,                            // NEARMV
    3,                            // ZEROMV
    1,                            // NEWMV
];

/// Sentinel value in [`COUNTER_TO_CONTEXT`] for counter sums that cannot
/// occur from two real neighbours.
///
/// libvpx `vp9/common/vp9_mvref_common.h:39` (`INVALID_CASE = 9`, part of
/// the `motion_vector_context` enum at lines 31-40).
pub const COUNTER_INVALID_CASE: u8 = 9;

/// Maps a summed [`MODE_2_COUNTER`] vote (0..=18) to one of the
/// [`INTER_MODE_CONTEXTS`] context indices that select a
/// [`DEFAULT_INTER_MODE_PROBS`] row.
///
/// libvpx `vp9/common/vp9_mvref_common.h:67-87` (`counter_to_context`),
/// resolving the `motion_vector_context` enum of `vp9_mvref_common.h:31-40`:
/// `BOTH_ZERO=0, ZERO_PLUS_PREDICTED=1, BOTH_PREDICTED=2,
/// NEW_PLUS_NON_INTRA=3, BOTH_NEW=4, INTRA_PLUS_NON_INTRA=5, BOTH_INTRA=6,
/// INVALID_CASE=9`.
///
/// **[`COUNTER_INVALID_CASE`] (9) is a real value in this table, not a
/// transcription bug**: sums 5, 7, 8, 11 and 13..=17 cannot occur for two
/// real neighbours (each contributes 0, 1, 3 or 9 per [`MODE_2_COUNTER`])
/// and libvpx fills the unreachable slots with the sentinel so the lookup
/// stays a flat array. `9` is out of [`INTER_MODE_CONTEXTS`]'s `0..7` range
/// **by design** — callers must check for the sentinel before indexing
/// [`DEFAULT_INTER_MODE_PROBS`] with a raw `COUNTER_TO_CONTEXT` entry.
#[rustfmt::skip]
pub const COUNTER_TO_CONTEXT: [u8; 19] = [
    2, // 0  = BOTH_PREDICTED
    3, // 1  = NEW_PLUS_NON_INTRA
    4, // 2  = BOTH_NEW
    1, // 3  = ZERO_PLUS_PREDICTED
    3, // 4  = NEW_PLUS_NON_INTRA
    9, // 5  = INVALID_CASE
    0, // 6  = BOTH_ZERO
    9, // 7  = INVALID_CASE
    9, // 8  = INVALID_CASE
    5, // 9  = INTRA_PLUS_NON_INTRA
    5, // 10 = INTRA_PLUS_NON_INTRA
    9, // 11 = INVALID_CASE
    5, // 12 = INTRA_PLUS_NON_INTRA
    9, // 13 = INVALID_CASE
    9, // 14 = INVALID_CASE
    9, // 15 = INVALID_CASE
    9, // 16 = INVALID_CASE
    9, // 17 = INVALID_CASE
    6, // 18 = BOTH_INTRA
];

// ---------------------------------------------------------------------------
// Reference frame syntax
// ---------------------------------------------------------------------------

/// Number of contexts for the `is_inter` (intra/inter) syntax element.
///
/// libvpx `vp9/common/vp9_enums.h` / `vp9_entropymode.c:266-268`
/// (`default_intra_inter_p[INTRA_INTER_CONTEXTS]`, 4 entries).
pub const INTRA_INTER_CONTEXTS: usize = 4;

/// Number of contexts for the single/compound reference-mode syntax element.
///
/// libvpx `vp9/common/vp9_entropymode.c:270-272`
/// (`default_comp_inter_p[COMP_INTER_CONTEXTS]`, 5 entries).
pub const COMP_INTER_CONTEXTS: usize = 5;

/// Number of contexts for the reference-frame syntax elements.
///
/// libvpx `vp9/common/vp9_entropymode.c:274-278`
/// (`default_comp_ref_p[REF_CONTEXTS]` and
/// `default_single_ref_p[REF_CONTEXTS][2]`, 5 entries each).
pub const REF_CONTEXTS: usize = 5;

/// Default `is_inter` probabilities.
///
/// libvpx `vp9/common/vp9_entropymode.c:266-268` (`default_intra_inter_p`).
/// Specification §10.5 `default_is_inter_probs`.
#[rustfmt::skip]
pub const DEFAULT_INTRA_INTER_PROBS: [u8; INTRA_INTER_CONTEXTS] = [9, 102, 187, 225];

/// Default compound/single reference-mode probabilities.
///
/// libvpx `vp9/common/vp9_entropymode.c:270-272` (`default_comp_inter_p`).
/// Specification §10.5 `default_comp_mode_probs`.
#[rustfmt::skip]
pub const DEFAULT_COMP_INTER_PROBS: [u8; COMP_INTER_CONTEXTS] = [239, 183, 119, 96, 41];

/// Default compound-reference probabilities.
///
/// libvpx `vp9/common/vp9_entropymode.c:274-275` (`default_comp_ref_p`).
/// Specification §10.5 `default_comp_ref_probs`.
#[rustfmt::skip]
pub const DEFAULT_COMP_REF_PROBS: [u8; REF_CONTEXTS] = [50, 126, 123, 221, 226];

/// Default single-reference probabilities.
///
/// libvpx `vp9/common/vp9_entropymode.c:277-279` (`default_single_ref_p`).
/// Specification §10.5 `default_single_ref_probs`.
#[rustfmt::skip]
pub const DEFAULT_SINGLE_REF_PROBS: [[u8; 2]; REF_CONTEXTS] =
    [[33, 16], [77, 74], [142, 142], [172, 170], [238, 247]];

// ---------------------------------------------------------------------------
// Motion vector syntax
// ---------------------------------------------------------------------------

/// Number of motion-vector joint types.
///
/// libvpx `vp9/common/vp9_entropymv.h:38`: `MV_JOINTS 4`.
pub const MV_JOINTS: usize = 4;

/// Number of motion-vector magnitude classes.
///
/// libvpx `vp9/common/vp9_entropymv.h:55`: `MV_CLASSES 11`.
pub const MV_CLASSES: usize = 11;

/// Size of the class-0 (smallest magnitude) integer-pel alphabet.
///
/// libvpx `vp9/common/vp9_entropymv.h:70-71`: `CLASS0_BITS 1`,
/// `CLASS0_SIZE (1 << CLASS0_BITS)`.
pub const CLASS0_SIZE: usize = 2;

/// Number of integer-magnitude bits coded for classes above class 0.
///
/// libvpx `vp9/common/vp9_entropymv.h:72`:
/// `MV_OFFSET_BITS (MV_CLASSES + CLASS0_BITS - 2)` = `11 + 1 - 2` = 10.
pub const MV_OFFSET_BITS: usize = MV_CLASSES + 1 - 2;

/// Size of the fractional-pel alphabet.
///
/// libvpx `vp9/common/vp9_entropymv.h:73`: `MV_FP_SIZE 4`.
pub const MV_FP_SIZE: usize = 4;

/// Motion-vector component range.
///
/// libvpx `vp9/common/vp9_entropymv.h:79-81`: `MV_IN_USE_BITS 14`,
/// `MV_UPP ((1 << MV_IN_USE_BITS) - 1)`, `MV_LOW (-(1 << MV_IN_USE_BITS))`.
pub const MV_IN_USE_BITS: u32 = 14;
/// See [`MV_IN_USE_BITS`].
pub const MV_UPP: i32 = (1 << MV_IN_USE_BITS) - 1;
/// See [`MV_IN_USE_BITS`].
pub const MV_LOW: i32 = -(1 << MV_IN_USE_BITS);

/// Motion-vector joint tree.
///
/// libvpx `vp9/common/vp9_entropymv.c:14-16`:
/// `{ -MV_JOINT_ZERO, 2, -MV_JOINT_HNZVZ, 4, -MV_JOINT_HZVNZ, -MV_JOINT_HNZVNZ }`
/// with the joint values of `vp9/common/vp9_entropymv.h:40-43`
/// (`MV_JOINT_ZERO 0`, `MV_JOINT_HNZVZ 1`, `MV_JOINT_HZVNZ 2`,
/// `MV_JOINT_HNZVNZ 3`).
/// Specification §9.3.1 `mv_joint_tree`.
///
/// `HNZVZ` means "horizontal nonzero, vertical zero"
/// (`vp9/common/vp9_entropymv.h:41`).
#[rustfmt::skip]
pub const MV_JOINT_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];

/// Motion-vector magnitude-class tree.
///
/// libvpx `vp9/common/vp9_entropymv.c:18-23`
/// (`vp9_mv_class_tree[TREE_SIZE(MV_CLASSES)]`).
/// Specification §9.3.1 `mv_class_tree`.
#[rustfmt::skip]
pub const MV_CLASS_TREE: [i8; 20] = [
    0, 2, -1, 4, 6, 8, -2, -3, 10, 12, -4, -5, -6, 14, 16, 18, -7, -8, -9, -10,
];

/// Motion-vector class-0 magnitude tree.
///
/// libvpx `vp9/common/vp9_entropymv.c:25`:
/// `vp9_mv_class0_tree[TREE_SIZE(CLASS0_SIZE)] = { -0, -1 }`.
/// Specification §9.3.1 `mv_class0_tree`.
#[rustfmt::skip]
pub const MV_CLASS0_TREE: [i8; 2] = [0, -1];

/// Motion-vector fractional-pel tree.
///
/// libvpx `vp9/common/vp9_entropymv.c:27-28`:
/// `vp9_mv_fp_tree[TREE_SIZE(MV_FP_SIZE)] = { -0, 2, -1, 4, -2, -3 }`.
/// Specification §9.3.1 `mv_fr_tree`.
#[rustfmt::skip]
pub const MV_FP_TREE: [i8; 6] = [0, 2, -1, 4, -2, -3];

/// Default probabilities for one motion-vector component.
///
/// Field order and shapes follow libvpx's `nmv_component`
/// (`vp9/common/vp9_entropymv.h:88-97`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NmvComponent {
    /// Sign bit probability.
    pub sign: u8,
    /// Magnitude-class tree probabilities (`MV_CLASSES - 1` nodes).
    pub classes: [u8; MV_CLASSES - 1],
    /// Class-0 magnitude tree probabilities (`CLASS0_SIZE - 1` nodes).
    pub class0: [u8; CLASS0_SIZE - 1],
    /// Integer-magnitude bit probabilities for classes above class 0.
    pub bits: [u8; MV_OFFSET_BITS],
    /// Fractional-pel tree probabilities within class 0, per class-0 value.
    pub class0_fp: [[u8; MV_FP_SIZE - 1]; CLASS0_SIZE],
    /// Fractional-pel tree probabilities for classes above class 0.
    pub fp: [u8; MV_FP_SIZE - 1],
    /// High-precision bit probability within class 0.
    pub class0_hp: u8,
    /// High-precision bit probability for classes above class 0.
    pub hp: u8,
}

/// Default probabilities for the motion-vector syntax as a whole.
///
/// Field order follows libvpx's `nmv_context`
/// (`vp9/common/vp9_entropymv.h:99-102`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NmvContext {
    /// Joint tree probabilities (`MV_JOINTS - 1` nodes).
    pub joints: [u8; MV_JOINTS - 1],
    /// Per-component probabilities.
    ///
    /// **`comps[0]` is the row (vertical) component and `comps[1]` is the
    /// column (horizontal) component**, matching libvpx's `comps[2]` layout —
    /// `vp9/common/vp9_entropymv.c:32-53` labels the first initialiser
    /// `// Vertical component` and the second `// Horizontal component`, and
    /// `vp9/decoder/vp9_decodemv.c` reads the vertical component first when
    /// the joint indicates both are present. Transposing these is invisible
    /// until fixture comparison.
    pub comps: [NmvComponent; 2],
}

/// libvpx's `default_nmv_context`.
///
/// libvpx `vp9/common/vp9_entropymv.c:30-54`.
/// Specification §10.5 `default_mv_joint_probs` and the per-component
/// `default_mv_*` tables.
///
/// The horizontal component's `classes` row ends in `208`, **not** `245`
/// (`245` is the *vertical* component's last `classes` value). A sibling
/// table in `vp9/probability.rs` transcribes this incorrectly — both its
/// vertical and horizontal rows end in `245` — do not copy that file; the
/// values below are transcribed directly from libvpx and pinned by
/// [`tests::mv_component_order_is_vertical_then_horizontal`].
#[rustfmt::skip]
pub const DEFAULT_NMV_CONTEXT: NmvContext = NmvContext {
    joints: [32, 64, 96],
    comps: [
        // Vertical component (libvpx vp9_entropymv.c:32-42).
        NmvComponent {
            sign: 128,
            classes: [224, 144, 192, 168, 192, 176, 192, 198, 198, 245],
            class0: [216],
            bits: [136, 140, 148, 160, 176, 192, 224, 234, 234, 240],
            class0_fp: [[128, 128, 64], [96, 112, 64]],
            fp: [64, 96, 64],
            class0_hp: 160,
            hp: 128,
        },
        // Horizontal component (libvpx vp9_entropymv.c:43-53).
        NmvComponent {
            sign: 128,
            classes: [216, 128, 176, 160, 176, 176, 192, 198, 198, 208],
            class0: [208],
            bits: [136, 140, 148, 160, 176, 192, 224, 234, 234, 240],
            class0_fp: [[128, 128, 64], [96, 112, 64]],
            fp: [64, 96, 64],
            class0_hp: 160,
            hp: 128,
        },
    ],
};

// ---------------------------------------------------------------------------
// Non-keyframe intra-mode probabilities
// ---------------------------------------------------------------------------

/// Non-keyframe default Y (luma) intra-mode probabilities, indexed by block
/// size group then tree node.
///
/// libvpx `vp9/common/vp9_entropymode.c:162-167` (`default_if_y_probs`;
/// `if` = "inter frame", i.e. this is the *non-keyframe* table — distinct
/// from [`super::tables::KF_Y_MODE_PROBS`], which is keyed by above/left
/// neighbour mode instead of block-size group).
///
/// Decoded with `INTRA_MODE_TREE` (shared with the keyframe table in
/// [`super::tables`] — only the probabilities differ between the two frame
/// types), after mapping the block size to a group with
/// [`SIZE_GROUP_LOOKUP`].
#[rustfmt::skip]
pub const DEFAULT_Y_MODE_PROBS: [[u8; 9]; 4] = [
    [65, 32, 18, 144, 162, 194, 41, 51, 98],    // block_size < 8x8
    [132, 68, 18, 165, 217, 196, 45, 40, 78],   // block_size < 16x16
    [173, 80, 19, 176, 240, 193, 64, 35, 46],   // block_size < 32x32
    [221, 135, 38, 194, 248, 121, 96, 85, 29],  // block_size >= 32x32
];

/// Maps a block size to its [`DEFAULT_Y_MODE_PROBS`] row (`BLOCK_SIZE_GROUPS
/// == 4` groups: `< 8x8`, `< 16x16`, `< 32x32`, `>= 32x32`).
///
/// libvpx `vp9/common/vp9_common_data.c:32-33` (`size_group_lookup`),
/// derived as `min(3, min(b_width_log2_lookup(bsize),
/// b_height_log2_lookup(bsize)))` per the source comment on line 31. Row
/// order is the `BLOCK_SIZE` enumeration, matching [`MV_REF_BLOCKS`].
#[rustfmt::skip]
pub const SIZE_GROUP_LOOKUP: [u8; 13] = [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3];

/// Non-keyframe default UV (chroma) intra-mode probabilities, indexed by the
/// co-located Y block's *decoded* mode then tree node.
///
/// libvpx `vp9/common/vp9_entropymode.c:169-180` (`default_if_uv_probs`;
/// distinct from [`super::tables::KF_UV_MODE_PROBS`], the keyframe
/// equivalent — the two tables differ numerically, not just in name).
#[rustfmt::skip]
pub const DEFAULT_UV_MODE_PROBS: [[u8; 9]; 10] = [
    [120, 7, 76, 176, 208, 126, 28, 54, 103],    // y = dc
    [48, 12, 154, 155, 139, 90, 34, 117, 119],   // y = v
    [67, 6, 25, 204, 243, 158, 13, 21, 96],      // y = h
    [97, 5, 44, 131, 176, 139, 48, 68, 97],      // y = d45
    [83, 5, 42, 156, 111, 152, 26, 49, 152],     // y = d135
    [80, 5, 58, 178, 74, 83, 33, 62, 145],       // y = d117
    [86, 5, 32, 154, 192, 168, 14, 22, 163],     // y = d153
    [85, 5, 32, 156, 216, 148, 19, 29, 73],      // y = d207
    [77, 7, 64, 116, 132, 122, 37, 126, 120],    // y = d63
    [101, 21, 107, 181, 192, 103, 19, 67, 125],  // y = tm
];

// ---------------------------------------------------------------------------
// Non-keyframe partition probabilities
// ---------------------------------------------------------------------------

/// Non-keyframe default partition probabilities, indexed by partition
/// context (4 block-size tiers x 4 above/left-split combinations) then tree
/// node.
///
/// libvpx `vp9/common/vp9_entropymode.c:209-231` (`default_partition_probs`;
/// distinct from [`super::tables::KF_PARTITION_PROBS`], the keyframe
/// equivalent — keyframes split far more aggressively, so the two tables are
/// numerically unrelated).
///
/// Decoded with `PARTITION_TREE` (shared with the keyframe table in
/// [`super::tables`]). Row grouping follows libvpx's own comments: rows 0-3
/// are the 8x8->4x4 decision, 4-7 are 16x16->8x8, 8-11 are 32x32->16x16,
/// 12-15 are 64x64->32x32; within each group of 4 the sub-order is "a/l both
/// not split", "a split, l not split", "l split, a not split", "a/l both
/// split" (`a` = above, `l` = left).
#[rustfmt::skip]
pub const DEFAULT_PARTITION_PROBS: [[u8; 3]; 16] = [
    // 8x8 -> 4x4
    [199, 122, 141], // a/l both not split
    [147, 63, 159],  // a split, l not split
    [148, 133, 118], // l split, a not split
    [121, 104, 114], // a/l both split
    // 16x16 -> 8x8
    [174, 73, 87], // a/l both not split
    [92, 41, 83],  // a split, l not split
    [82, 99, 50],  // l split, a not split
    [53, 39, 39],  // a/l both split
    // 32x32 -> 16x16
    [177, 58, 59], // a/l both not split
    [68, 26, 63],  // a split, l not split
    [52, 79, 25],  // l split, a not split
    [17, 14, 12],  // a/l both split
    // 64x64 -> 32x32
    [222, 34, 30], // a/l both not split
    [72, 16, 44],  // a split, l not split
    [58, 32, 12],  // l split, a not split
    [10, 7, 6],    // a/l both split
];

// ---------------------------------------------------------------------------
// Loop filter mode lookup
// ---------------------------------------------------------------------------

/// Maps a decoded prediction mode to the loop filter's "does this block have
/// a nonzero-vector inter mode" bit, used to pick between the finer-grained
/// filter-level tables: `0` for any intra mode *and* for `ZEROMV`, `1` for
/// `NEARESTMV`, `NEARMV` and `NEWMV`.
///
/// libvpx `vp9/common/vp9_loopfilter.c:207-210` (`mode_lf_lut`), indexed by
/// the same raw `MB_MODE_COUNT` mode number as [`MODE_2_COUNTER`] (*not* the
/// `INTER_OFFSET`-relative numbering of [`INTER_MODE_NEARESTMV`] and its
/// siblings — see that constant's doc comment).
///
/// `ZEROMV` maps to `0` — libvpx's own comment on this table is
/// `// INTER_MODES (ZEROMV == 0)`, flagging that this is the one place a
/// `ZEROMV` block is grouped with intra blocks rather than with the other
/// three inter modes.
#[rustfmt::skip]
pub const MODE_LF_LUT: [u8; 14] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, // DC_PRED .. TM_PRED (all intra modes)
    1,                            // NEARESTMV
    1,                            // NEARMV
    0,                            // ZEROMV (sic -- grouped with intra here)
    1,                            // NEWMV
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kernel family, paired with its name for failure messages.
    const FAMILIES: [(&str, &InterpKernels); 4] = [
        ("EIGHTTAP", &EIGHTTAP),
        ("EIGHTTAP_SMOOTH", &EIGHTTAP_SMOOTH),
        ("EIGHTTAP_SHARP", &EIGHTTAP_SHARP),
        ("BILINEAR", &BILINEAR),
    ];

    // -- Interpolation kernel structural invariants ---------------------

    /// The invariant that would have caught the shipped 8-row truncation in
    /// `vp9/prediction.rs` (which declared `SUBPEL_SHIFTS = 8` and kept only
    /// libvpx rows 0..=7).
    #[test]
    fn every_family_has_exactly_sixteen_phases() {
        assert_eq!(SUBPEL_SHIFTS, 16, "libvpx vpx_filter.h:25");
        for (name, family) in FAMILIES {
            assert_eq!(family.len(), SUBPEL_SHIFTS, "{name}: phase count");
            for (k, row) in family.iter().enumerate() {
                assert_eq!(row.len(), SUBPEL_TAPS, "{name} row {k}: tap count");
            }
        }
    }

    #[test]
    fn every_row_sums_to_filter_weight() {
        for (name, family) in FAMILIES {
            for (k, row) in family.iter().enumerate() {
                let sum: i32 = row.iter().map(|&t| i32::from(t)).sum();
                assert_eq!(sum, FILTER_WEIGHT, "{name} row {k}: tap sum");
            }
        }
    }

    #[test]
    fn row_zero_is_the_identity_kernel() {
        for (name, family) in FAMILIES {
            assert_eq!(
                family[0],
                [0, 0, 0, 128, 0, 0, 0, 0],
                "{name} row 0: must be a straight sample copy"
            );
        }
    }

    /// Row 8 is the half-pel phase (8/16). The symmetry libvpx's values
    /// actually satisfy is an **exact palindrome over the eight taps**,
    /// `t[i] == t[7 - i]` — no shift or offset is involved.
    #[test]
    fn row_eight_is_the_half_pel_row_and_is_an_exact_palindrome() {
        for (name, family) in FAMILIES {
            let row = family[8];
            for i in 0..SUBPEL_TAPS / 2 {
                assert_eq!(
                    row[i],
                    row[SUBPEL_TAPS - 1 - i],
                    "{name} row 8 (half-pel): tap {i} vs tap {}",
                    SUBPEL_TAPS - 1 - i
                );
            }
        }
        // The half-pel row is also the one the truncated table lacked
        // entirely; pin the actual values so a future edit cannot quietly
        // reintroduce a shifted table.
        assert_eq!(EIGHTTAP[8], [-1, 6, -19, 78, 78, -19, 6, -1]);
        assert_eq!(EIGHTTAP_SMOOTH[8], [-1, -4, 14, 55, 55, 14, -4, -1]);
        assert_eq!(EIGHTTAP_SHARP[8], [-4, 11, -23, 80, 80, -23, 11, -4]);
        assert_eq!(BILINEAR[8], [0, 0, 0, 64, 64, 0, 0, 0]);
    }

    /// Phase symmetry: phase `k` and phase `16 - k` are tap-reversals of each
    /// other. This holds for **all four** families in libvpx's actual values.
    ///
    /// `k` runs 1..=15, not 0..=15: phase 0 has no partner (`16 - 0 == 16` is
    /// out of range), and row 0 reversed is `[0,0,0,0,128,0,0,0]`, which is
    /// not row 0 — so widening this loop bound would make the test fail
    /// against correct data. `k == 8` self-pairs, which is exactly the
    /// palindrome asserted by
    /// [`row_eight_is_the_half_pel_row_and_is_an_exact_palindrome`]; the two
    /// tests are related but not redundant.
    #[test]
    fn phase_k_is_the_tap_reversal_of_phase_sixteen_minus_k() {
        for (name, family) in FAMILIES {
            for k in 1..SUBPEL_SHIFTS {
                for i in 0..SUBPEL_TAPS {
                    assert_eq!(
                        family[k][i],
                        family[SUBPEL_SHIFTS - k][SUBPEL_TAPS - 1 - i],
                        "{name}: row {k} tap {i} vs row {} tap {}",
                        SUBPEL_SHIFTS - k,
                        SUBPEL_TAPS - 1 - i
                    );
                }
            }
        }
    }

    /// Bilinear rows carry their weight on taps 3 and 4 only.
    ///
    /// Stated as "tap 3 is `128 - 8k`, tap 4 is `8k`, everything else zero",
    /// which is exact for all 16 rows. The looser phrasing "exactly two
    /// adjacent nonzero taps" does not hold at `k == 0` (and only there),
    /// where tap 4 is `0` and the row degenerates to the identity kernel.
    #[test]
    fn bilinear_rows_are_two_adjacent_taps_at_positions_three_and_four() {
        for (k, row) in BILINEAR.iter().enumerate() {
            let k = i16::try_from(k).expect("k < 16 fits in i16");
            assert_eq!(row[3], 128 - 8 * k, "BILINEAR row {k}: tap 3");
            assert_eq!(row[4], 8 * k, "BILINEAR row {k}: tap 4");
            for (i, &tap) in row.iter().enumerate() {
                if i != 3 && i != 4 {
                    assert_eq!(tap, 0, "BILINEAR row {k}: tap {i} must be zero");
                }
            }
        }
    }

    /// The four families must be distinct, and `FILTER_KERNELS` must present
    /// them in bitstream-index order — the check that catches a
    /// smooth/sharp transposition.
    #[test]
    fn filter_kernels_are_in_bitstream_index_order() {
        assert_eq!(FILTER_KERNELS[0], EIGHTTAP);
        assert_eq!(FILTER_KERNELS[1], EIGHTTAP_SMOOTH);
        assert_eq!(FILTER_KERNELS[2], EIGHTTAP_SHARP);
        assert_eq!(FILTER_KERNELS[3], BILINEAR);
        // The smooth family is the low-pass one: its half-pel row spreads
        // weight across six taps, while the sharp family concentrates a
        // larger negative lobe. Distinguishes the two without restating them.
        assert_eq!(EIGHTTAP_SMOOTH[8][3], 55, "smooth half-pel centre tap");
        assert_eq!(EIGHTTAP_SHARP[8][3], 80, "sharp half-pel centre tap");
        assert!(
            EIGHTTAP_SMOOTH[8][3] < EIGHTTAP[8][3] && EIGHTTAP[8][3] < EIGHTTAP_SHARP[8][3],
            "smooth < regular < sharp centre-tap concentration at half-pel"
        );
        for i in 0..4 {
            for j in (i + 1)..4 {
                assert_ne!(FILTER_KERNELS[i], FILTER_KERNELS[j], "families {i} and {j}");
            }
        }
    }

    #[test]
    fn literal_to_filter_is_the_permuted_header_encoding() {
        // libvpx vp9_decodeframe.c:1457-1458.
        assert_eq!(LITERAL_TO_FILTER, [1, 0, 2, 3]);
        // It is a permutation of the four filter indices, and it is *not*
        // the identity — the distinction this constant exists to preserve.
        let mut seen = [false; 4];
        for &f in &LITERAL_TO_FILTER {
            seen[f as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "must be a permutation of 0..=3");
        assert_ne!(LITERAL_TO_FILTER, [0, 1, 2, 3], "not the identity mapping");
    }

    // -- Tree structural invariants -------------------------------------

    /// Validates a libvpx `vpx_tree_index` array against the encoding
    /// [`super::super::booldec::BoolReader::read_tree`] implements: the array
    /// has `2 * leaves - 2` entries, every leaf token `0..leaves` appears
    /// exactly once, and every positive entry is an in-range even node index.
    ///
    /// `TREE_SIZE(leaves) == 2 * (leaves - 1)` is always even, so "odd
    /// length" is not a shape a valid `vpx_tree_index` array can have; the
    /// length check below is the correct, and only possible, reading of that
    /// invariant.
    fn assert_valid_tree(name: &str, tree: &[i8], leaves: usize) {
        assert_eq!(tree.len(), 2 * leaves - 2, "{name}: TREE_SIZE({leaves})");
        let mut leaf_seen = vec![0usize; leaves];
        for (i, &e) in tree.iter().enumerate() {
            if e > 0 {
                let node = e as usize;
                assert!(
                    node < tree.len(),
                    "{name}: node offset {node} at {i} out of range"
                );
                assert_eq!(
                    node % 2,
                    0,
                    "{name}: node offset {node} at {i} must be even"
                );
                assert!(
                    node > i,
                    "{name}: node offset {node} at {i} must move forward"
                );
            } else {
                let leaf = (-e) as usize;
                assert!(leaf < leaves, "{name}: leaf {leaf} at {i} out of range");
                leaf_seen[leaf] += 1;
            }
        }
        for (leaf, &count) in leaf_seen.iter().enumerate() {
            assert_eq!(count, 1, "{name}: leaf {leaf} must appear exactly once");
        }
    }

    #[test]
    fn trees_are_structurally_well_formed() {
        assert_valid_tree("INTER_MODE_TREE", &INTER_MODE_TREE, INTER_MODES);
        assert_valid_tree(
            "SWITCHABLE_INTERP_TREE",
            &SWITCHABLE_INTERP_TREE,
            SWITCHABLE_FILTERS,
        );
        assert_valid_tree("MV_JOINT_TREE", &MV_JOINT_TREE, MV_JOINTS);
        assert_valid_tree("MV_CLASS_TREE", &MV_CLASS_TREE, MV_CLASSES);
        assert_valid_tree("MV_CLASS0_TREE", &MV_CLASS0_TREE, CLASS0_SIZE);
        assert_valid_tree("MV_FP_TREE", &MV_FP_TREE, MV_FP_SIZE);
    }

    /// The inter-mode tree's leaf order is deliberately not the mode
    /// enumeration order. Pin the decode of each root-to-leaf bit path so a
    /// "tidying" reorder fails loudly.
    #[test]
    fn inter_mode_tree_decodes_zeromv_first() {
        // Walk the tree by hand: at node `i`, bit b moves to tree[i + b].
        let decode = |bits: &[usize]| -> u8 {
            let mut i: i16 = 0;
            for &b in bits {
                i = i16::from(INTER_MODE_TREE[(i + b as i16) as usize]);
                if i <= 0 {
                    return (-i) as u8;
                }
            }
            panic!("bit path did not reach a leaf");
        };
        assert_eq!(decode(&[0]), INTER_MODE_ZEROMV, "first leaf is ZEROMV");
        assert_eq!(decode(&[1, 0]), INTER_MODE_NEARESTMV);
        assert_eq!(decode(&[1, 1, 0]), INTER_MODE_NEARMV);
        assert_eq!(decode(&[1, 1, 1]), INTER_MODE_NEWMV);
    }

    /// The switchable-filter tree's leaves are filter indices, so leaf 1 must
    /// be the smooth family and leaf 2 the sharp family.
    #[test]
    fn switchable_interp_tree_leaves_are_filter_indices() {
        assert_eq!(SWITCHABLE_INTERP_TREE, [0, 2, -1, -2]);
        // Leaf order: EIGHTTAP(0), then EIGHTTAP_SMOOTH(1), EIGHTTAP_SHARP(2).
        assert_eq!(-SWITCHABLE_INTERP_TREE[0], 0);
        assert_eq!(-SWITCHABLE_INTERP_TREE[2], 1);
        assert_eq!(-SWITCHABLE_INTERP_TREE[3], 2);
    }

    // -- Probability table shape invariants -----------------------------

    /// VP9 probabilities are in 1..=255; 0 is not a representable `vpx_prob`
    /// value for a branch and would make the arithmetic decoder degenerate.
    fn assert_probs_in_range(name: &str, probs: &[u8]) {
        for (i, &p) in probs.iter().enumerate() {
            assert!(p >= 1, "{name}[{i}] = {p} must be >= 1");
        }
    }

    #[test]
    fn inter_mode_probs_have_seven_contexts_of_three() {
        assert_eq!(DEFAULT_INTER_MODE_PROBS.len(), INTER_MODE_CONTEXTS);
        assert_eq!(INTER_MODE_CONTEXTS, 7);
        for (ctx, row) in DEFAULT_INTER_MODE_PROBS.iter().enumerate() {
            assert_eq!(row.len(), INTER_MODES - 1, "context {ctx}");
            assert_probs_in_range("DEFAULT_INTER_MODE_PROBS", row);
        }
    }

    #[test]
    fn switchable_interp_probs_have_four_contexts_of_two() {
        assert_eq!(
            DEFAULT_SWITCHABLE_INTERP_PROBS.len(),
            SWITCHABLE_FILTER_CONTEXTS
        );
        assert_eq!(SWITCHABLE_FILTER_CONTEXTS, 4);
        for row in &DEFAULT_SWITCHABLE_INTERP_PROBS {
            assert_eq!(row.len(), SWITCHABLE_FILTERS - 1);
            assert_probs_in_range("DEFAULT_SWITCHABLE_INTERP_PROBS", row);
        }
    }

    #[test]
    fn reference_probs_have_the_documented_shapes() {
        assert_eq!(DEFAULT_INTRA_INTER_PROBS.len(), INTRA_INTER_CONTEXTS);
        assert_eq!(DEFAULT_COMP_INTER_PROBS.len(), COMP_INTER_CONTEXTS);
        assert_eq!(DEFAULT_COMP_REF_PROBS.len(), REF_CONTEXTS);
        assert_eq!(DEFAULT_SINGLE_REF_PROBS.len(), REF_CONTEXTS);
        assert_probs_in_range("DEFAULT_INTRA_INTER_PROBS", &DEFAULT_INTRA_INTER_PROBS);
        assert_probs_in_range("DEFAULT_COMP_INTER_PROBS", &DEFAULT_COMP_INTER_PROBS);
        assert_probs_in_range("DEFAULT_COMP_REF_PROBS", &DEFAULT_COMP_REF_PROBS);
        for row in &DEFAULT_SINGLE_REF_PROBS {
            assert_eq!(row.len(), 2);
            assert_probs_in_range("DEFAULT_SINGLE_REF_PROBS", row);
        }
    }

    #[test]
    fn nmv_context_has_the_documented_shapes() {
        assert_eq!(CLASS0_SIZE, 2, "libvpx vp9_entropymv.h:70-71");
        assert_eq!(MV_OFFSET_BITS, 10, "libvpx vp9_entropymv.h:72");
        assert_eq!(MV_CLASSES, 11);
        assert_eq!(MV_FP_SIZE, 4);
        assert_eq!(DEFAULT_NMV_CONTEXT.joints.len(), MV_JOINTS - 1);
        assert_probs_in_range("joints", &DEFAULT_NMV_CONTEXT.joints);
        for (which, comp) in DEFAULT_NMV_CONTEXT.comps.iter().enumerate() {
            assert_eq!(comp.classes.len(), MV_CLASSES - 1, "comp {which} classes");
            assert_eq!(comp.class0.len(), CLASS0_SIZE - 1, "comp {which} class0");
            assert_eq!(comp.bits.len(), MV_OFFSET_BITS, "comp {which} bits");
            assert_eq!(comp.class0_fp.len(), CLASS0_SIZE, "comp {which} class0_fp");
            assert_eq!(comp.fp.len(), MV_FP_SIZE - 1, "comp {which} fp");
            assert_probs_in_range("classes", &comp.classes);
            assert_probs_in_range("class0", &comp.class0);
            assert_probs_in_range("bits", &comp.bits);
            assert_probs_in_range("fp", &comp.fp);
            for row in &comp.class0_fp {
                assert_eq!(row.len(), MV_FP_SIZE - 1);
                assert_probs_in_range("class0_fp", row);
            }
            assert!(comp.sign >= 1 && comp.class0_hp >= 1 && comp.hp >= 1);
        }
    }

    /// `comps[0]` is vertical (row) and `comps[1]` is horizontal (column).
    /// The two components differ only in their `classes` and `class0` rows,
    /// so this is the only cheap way to detect a transposition. This is also
    /// the test that pins the vertical/horizontal `classes[9]` values the
    /// module doc comment on [`DEFAULT_NMV_CONTEXT`] warns a sibling table
    /// gets wrong (`245` vertical vs `208` horizontal, not `245`/`245`).
    #[test]
    fn mv_component_order_is_vertical_then_horizontal() {
        let vertical = &DEFAULT_NMV_CONTEXT.comps[0];
        let horizontal = &DEFAULT_NMV_CONTEXT.comps[1];
        // libvpx vp9_entropymv.c:35 (vertical) and :46 (horizontal).
        assert_eq!(
            vertical.classes,
            [224, 144, 192, 168, 192, 176, 192, 198, 198, 245],
            "comps[0] must be the vertical component"
        );
        assert_eq!(
            horizontal.classes,
            [216, 128, 176, 160, 176, 176, 192, 198, 198, 208],
            "comps[1] must be the horizontal component"
        );
        assert_eq!(vertical.class0, [216]);
        assert_eq!(horizontal.class0, [208]);
        // Everything else is shared between the two components.
        assert_eq!(vertical.bits, horizontal.bits);
        assert_eq!(vertical.class0_fp, horizontal.class0_fp);
        assert_eq!(vertical.fp, horizontal.fp);
        assert_eq!(vertical.sign, horizontal.sign);
        assert_eq!(vertical.class0_hp, horizontal.class0_hp);
        assert_eq!(vertical.hp, horizontal.hp);
    }

    #[test]
    fn mv_range_constants_match_libvpx() {
        // libvpx vp9_entropymv.h:79-81.
        assert_eq!(MV_IN_USE_BITS, 14);
        assert_eq!(MV_UPP, 16383);
        assert_eq!(MV_LOW, -16384);
    }

    // -- Non-keyframe mode / partition probability invariants -----------

    #[test]
    fn non_keyframe_y_mode_probs_have_four_groups_of_nine() {
        assert_eq!(DEFAULT_Y_MODE_PROBS.len(), 4, "BLOCK_SIZE_GROUPS");
        for row in &DEFAULT_Y_MODE_PROBS {
            assert_eq!(row.len(), 9, "INTRA_MODES - 1");
            assert_probs_in_range("DEFAULT_Y_MODE_PROBS", row);
        }
        // libvpx vp9_entropymode.c:163 and :166.
        assert_eq!(
            DEFAULT_Y_MODE_PROBS[0],
            [65, 32, 18, 144, 162, 194, 41, 51, 98]
        );
        assert_eq!(
            DEFAULT_Y_MODE_PROBS[3],
            [221, 135, 38, 194, 248, 121, 96, 85, 29]
        );
    }

    #[test]
    fn size_group_lookup_is_monotonic_and_matches_libvpx() {
        assert_eq!(SIZE_GROUP_LOOKUP.len(), 13, "BLOCK_SIZES");
        assert_eq!(SIZE_GROUP_LOOKUP, [0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 3]);
        for pair in SIZE_GROUP_LOOKUP.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "size_group_lookup must be non-decreasing in BLOCK_SIZE order"
            );
        }
        for &group in &SIZE_GROUP_LOOKUP {
            assert!(
                (group as usize) < DEFAULT_Y_MODE_PROBS.len(),
                "every group must index DEFAULT_Y_MODE_PROBS"
            );
        }
    }

    #[test]
    fn non_keyframe_uv_mode_probs_have_ten_groups_of_nine() {
        assert_eq!(DEFAULT_UV_MODE_PROBS.len(), 10, "INTRA_MODES");
        for row in &DEFAULT_UV_MODE_PROBS {
            assert_eq!(row.len(), 9);
            assert_probs_in_range("DEFAULT_UV_MODE_PROBS", row);
        }
        // libvpx vp9_entropymode.c:170 (y=dc) and :179 (y=tm).
        assert_eq!(
            DEFAULT_UV_MODE_PROBS[0],
            [120, 7, 76, 176, 208, 126, 28, 54, 103]
        );
        assert_eq!(
            DEFAULT_UV_MODE_PROBS[9],
            [101, 21, 107, 181, 192, 103, 19, 67, 125]
        );
    }

    #[test]
    fn partition_probs_have_sixteen_contexts_of_three() {
        assert_eq!(DEFAULT_PARTITION_PROBS.len(), 16, "PARTITION_CONTEXTS");
        for row in &DEFAULT_PARTITION_PROBS {
            assert_eq!(row.len(), 3, "PARTITION_TYPES - 1");
            assert_probs_in_range("DEFAULT_PARTITION_PROBS", row);
        }
        // libvpx vp9_entropymode.c:212 and :230.
        assert_eq!(DEFAULT_PARTITION_PROBS[0], [199, 122, 141]);
        assert_eq!(DEFAULT_PARTITION_PROBS[15], [10, 7, 6]);
    }

    /// Non-keyframe tables must differ numerically from their keyframe
    /// counterparts in [`super::tables`] — a copy-paste from the wrong table
    /// would otherwise pass every shape check above without complaint.
    #[test]
    fn non_keyframe_mode_probs_differ_from_keyframe_tables() {
        assert_ne!(
            DEFAULT_UV_MODE_PROBS[0],
            super::super::tables::KF_UV_MODE_PROBS[0]
        );
        assert_ne!(
            DEFAULT_PARTITION_PROBS,
            super::super::tables::KF_PARTITION_PROBS
        );
    }

    // -- Mode-context derivation table invariants ------------------------

    #[test]
    fn mode_2_counter_matches_libvpx() {
        assert_eq!(MODE_2_COUNTER.len(), 14, "MB_MODE_COUNT");
        assert!(
            MODE_2_COUNTER[..10].iter().all(|&v| v == 9),
            "all ten intra modes vote 9"
        );
        assert_eq!(MODE_2_COUNTER[10], 0, "NEARESTMV");
        assert_eq!(MODE_2_COUNTER[11], 0, "NEARMV");
        assert_eq!(MODE_2_COUNTER[12], 3, "ZEROMV");
        assert_eq!(MODE_2_COUNTER[13], 1, "NEWMV");
    }

    #[test]
    fn counter_to_context_matches_libvpx_including_invalid_sentinels() {
        assert_eq!(COUNTER_TO_CONTEXT.len(), 19);
        assert_eq!(COUNTER_INVALID_CASE, 9);
        assert_eq!(
            COUNTER_TO_CONTEXT,
            [2, 3, 4, 1, 3, 9, 0, 9, 9, 5, 5, 9, 5, 9, 9, 9, 9, 9, 6]
        );
        // Every entry is either a valid INTER_MODE_CONTEXTS index or the
        // documented sentinel -- never anything else, and never something
        // that is silently in-range but wrong.
        for (sum, &ctx) in COUNTER_TO_CONTEXT.iter().enumerate() {
            assert!(
                (ctx as usize) < INTER_MODE_CONTEXTS || ctx == COUNTER_INVALID_CASE,
                "counter_to_context[{sum}] = {ctx} is neither a valid context nor the sentinel"
            );
        }
        // Sums that two real neighbours can actually produce (each neighbour
        // contributes 0, 1, 3 or 9 per MODE_2_COUNTER, so reachable sums are
        // 0, 1, 2, 3, 4, 6, 9, 10, 12, 18) must resolve to a real context.
        for &sum in &[0usize, 1, 2, 3, 4, 6, 9, 10, 12, 18] {
            assert_ne!(
                COUNTER_TO_CONTEXT[sum], COUNTER_INVALID_CASE,
                "sum {sum} is reachable from two real neighbours and must not be the sentinel"
            );
        }
    }

    #[test]
    fn mv_ref_blocks_are_row_col_not_col_row() {
        assert_eq!(MV_REF_BLOCKS.len(), 13, "BLOCK_SIZES");
        for size in &MV_REF_BLOCKS {
            assert_eq!(size.len(), 8, "MVREF_NEIGHBOURS");
        }
        // BLOCK_64X64 (last row): an asymmetric entry pins [row, col] order
        // -- a transposition would silently pass every other check here.
        // libvpx vp9_mvref_common.h:199-200.
        assert_eq!(MV_REF_BLOCKS[12][0], [-1, 3], "BLOCK_64X64 candidate 0");
        assert_eq!(MV_REF_BLOCKS[12][1], [3, -1], "BLOCK_64X64 candidate 1");
        // BLOCK_4X4 (first row), pinned in full. libvpx vp9_mvref_common.h:91-98.
        assert_eq!(
            MV_REF_BLOCKS[0],
            [
                [-1, 0],
                [0, -1],
                [-1, -1],
                [-2, 0],
                [0, -2],
                [-2, -1],
                [-1, -2],
                [-2, -2]
            ]
        );
        // BLOCK_8X16 (an asymmetric-size row): first two candidates are
        // transposes of each other, which would stay true even if this
        // whole row were itself transposed -- BLOCK_64X64 above is the test
        // that actually catches a global row/col swap; this one instead
        // catches a per-row scramble. libvpx vp9_mvref_common.h:127-134.
        assert_eq!(
            MV_REF_BLOCKS[4],
            [
                [0, -1],
                [-1, 0],
                [1, -1],
                [-1, -1],
                [0, -2],
                [-2, 0],
                [-2, -1],
                [-1, -2]
            ]
        );
    }

    #[test]
    fn mode_lf_lut_matches_libvpx() {
        assert_eq!(MODE_LF_LUT.len(), 14, "MB_MODE_COUNT");
        assert!(
            MODE_LF_LUT[..10].iter().all(|&v| v == 0),
            "all ten intra modes"
        );
        assert_eq!(MODE_LF_LUT[10], 1, "NEARESTMV");
        assert_eq!(MODE_LF_LUT[11], 1, "NEARMV");
        assert_eq!(MODE_LF_LUT[12], 0, "ZEROMV -- grouped with intra here");
        assert_eq!(MODE_LF_LUT[13], 1, "NEWMV");
    }
}
