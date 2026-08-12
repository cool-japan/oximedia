//! VP9 compressed-header parsing — exact port of `read_compressed_header`
//! (libvpx `vp9/decoder/vp9_decodeframe.c:2866-2914`, v1.15.2) and
//! `vp9/decoder/vp9_dsubexp.c`.
//!
//! For keyframes / intra-only frames the compressed header carries only the
//! transform mode (+ tx-size probability updates), coefficient probability
//! updates, and skip probability updates; all inter-related sections are
//! absent (`!frame_is_intra_only`). An inter frame continues with eight more
//! sections in a strict order — inter modes, switchable-filter probabilities
//! (only when the frame-level filter is `SWITCHABLE`), intra/inter flag
//! probabilities, the frame reference mode and its per-mode reference
//! probabilities, Y intra-mode probabilities, partition probabilities, and
//! the motion-vector probabilities.
//!
//! # The two update mechanisms
//!
//! Every section except the last uses the *subexponential* update of
//! `vp9_diff_update_prob` (a flag at [`DIFF_UPDATE_PROB`], then a remapped
//! delta). The motion-vector section does **not**: `update_mv_probs`
//! (`vp9_decodeframe.c:135-139`) reads a flag at [`MV_UPDATE_PROB`] and then
//! a *raw 7-bit literal*, storing `(literal << 1) | 1`. Collapsing the two
//! into one loop, or fusing `read_mv_probs`' three sequential passes over
//! the components into one, decodes plausible-looking garbage rather than
//! failing loudly — see [`read_mv_probs`].

use super::booldec::BoolReader;
use super::counts::{BLOCK_SIZE_GROUPS, INTRA_MODES, PARTITION_CONTEXTS, PARTITION_TYPES};
use super::refs::is_compound_reference_allowed;
use super::tables;
use super::tables_inter::{
    NmvContext, COMP_INTER_CONTEXTS, DEFAULT_COMP_INTER_PROBS, DEFAULT_COMP_REF_PROBS,
    DEFAULT_INTER_MODE_PROBS, DEFAULT_INTRA_INTER_PROBS, DEFAULT_NMV_CONTEXT,
    DEFAULT_PARTITION_PROBS, DEFAULT_SINGLE_REF_PROBS, DEFAULT_SWITCHABLE_INTERP_PROBS,
    DEFAULT_UV_MODE_PROBS, DEFAULT_Y_MODE_PROBS, INTER_MODES, INTER_MODE_CONTEXTS,
    INTRA_INTER_CONTEXTS, REF_CONTEXTS, SWITCHABLE_FILTERS, SWITCHABLE_FILTER_CONTEXTS,
};
use crate::error::{CodecError, CodecResult};
use crate::vp9::uncompressed::UncompressedHeader;

/// VP9 `TX_MODE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxMode {
    /// Only 4x4 transforms.
    Only4x4 = 0,
    /// Up to 8x8.
    Allow8x8 = 1,
    /// Up to 16x16.
    Allow16x16 = 2,
    /// Up to 32x32.
    Allow32x32 = 3,
    /// Per-block selection.
    Select = 4,
}

impl TxMode {
    /// `tx_mode_to_biggest_tx_size` (vp9_common_data.c).
    #[must_use]
    pub fn biggest_tx_size(self) -> usize {
        match self {
            TxMode::Only4x4 => 0,
            TxMode::Allow8x8 => 1,
            TxMode::Allow16x16 => 2,
            TxMode::Allow32x32 | TxMode::Select => 3,
        }
    }
}

/// Coefficient probabilities: `[tx_size][plane_type][ref][band][ctx][node]`.
pub type CoefProbs = [[[[[[u8; 3]; 6]; 6]; 2]; 2]; 4];

/// Entropy state used by the intra-frame decode (defaults + header updates).
///
/// `PartialEq` is derived so the cross-frame context save/load round trip in
/// [`super::state::Vp9DecState`] can be verified byte for byte; `Debug` is
/// deliberately *not*, since a single instance is ~1.7 KB of probabilities.
/// Field names mirror [`super::counts::FrameCounts`] so a backward-adaptation
/// call site pairs `probs.<name>` with `counts.<name>`.
///
/// [`FrameProbs::uv_mode`] is the one field the compressed header never
/// touches: libvpx's `read_compressed_header` has no `uv_mode` section at
/// all, and the probabilities move only through backward adaptation
/// ([`super::adapt::adapt_mode_probs`]). It lives here anyway because it is
/// part of the saved `FRAME_CONTEXT` a later frame loads, and because the
/// non-key-frame intra block path ([`super::modeinfo`]) reads it — feeding
/// that path the static defaults instead would decode the second and later
/// inter frames of any adapting sequence against the wrong probabilities.
#[derive(Clone, PartialEq, Eq)]
pub struct FrameProbs {
    /// Coefficient probabilities.
    pub coef: CoefProbs,
    /// Skip flag probabilities per context.
    pub skip: [u8; 3],
    /// tx_probs.p8x8[ctx][node].
    pub tx8: [[u8; 1]; 2],
    /// tx_probs.p16x16[ctx][node].
    pub tx16: [[u8; 2]; 2],
    /// tx_probs.p32x32[ctx][node].
    pub tx32: [[u8; 3]; 2],
    /// `fc->inter_mode_probs[ctx][node]`.
    pub inter_mode: [[u8; INTER_MODES - 1]; INTER_MODE_CONTEXTS],
    /// `fc->switchable_interp_prob[ctx][node]`.
    pub switchable_interp: [[u8; SWITCHABLE_FILTERS - 1]; SWITCHABLE_FILTER_CONTEXTS],
    /// `fc->intra_inter_prob[ctx]`.
    pub intra_inter: [u8; INTRA_INTER_CONTEXTS],
    /// `fc->comp_inter_prob[ctx]`.
    pub comp_inter: [u8; COMP_INTER_CONTEXTS],
    /// `fc->single_ref_prob[ctx][bit]`.
    pub single_ref: [[u8; 2]; REF_CONTEXTS],
    /// `fc->comp_ref_prob[ctx]`.
    pub comp_ref: [u8; REF_CONTEXTS],
    /// `fc->y_mode_prob[block_size_group][node]` (the non-keyframe table).
    pub y_mode: [[u8; INTRA_MODES - 1]; BLOCK_SIZE_GROUPS],
    /// `fc->uv_mode_prob[y_mode][node]` (the non-keyframe table).
    ///
    /// Never forward-updated — see the struct doc comment.
    pub uv_mode: [[u8; INTRA_MODES - 1]; INTRA_MODES],
    /// `fc->partition_prob[ctx][node]`.
    pub partition: [[u8; PARTITION_TYPES - 1]; PARTITION_CONTEXTS],
    /// `fc->nmvc`, the motion-vector probabilities.
    pub mv: NmvContext,
}

impl FrameProbs {
    /// Default probabilities (`vp9_default_coef_probs` et al). Keyframes
    /// always reset every frame context to these defaults
    /// (`vp9_setup_past_independence`).
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            coef: [
                tables::DEFAULT_COEF_PROBS_4X4,
                tables::DEFAULT_COEF_PROBS_8X8,
                tables::DEFAULT_COEF_PROBS_16X16,
                tables::DEFAULT_COEF_PROBS_32X32,
            ],
            skip: tables::DEFAULT_SKIP_PROBS,
            tx8: tables::DEFAULT_TX_8X8_PROBS,
            tx16: tables::DEFAULT_TX_16X16_PROBS,
            tx32: tables::DEFAULT_TX_32X32_PROBS,
            inter_mode: DEFAULT_INTER_MODE_PROBS,
            switchable_interp: DEFAULT_SWITCHABLE_INTERP_PROBS,
            intra_inter: DEFAULT_INTRA_INTER_PROBS,
            comp_inter: DEFAULT_COMP_INTER_PROBS,
            single_ref: DEFAULT_SINGLE_REF_PROBS,
            comp_ref: DEFAULT_COMP_REF_PROBS,
            y_mode: DEFAULT_Y_MODE_PROBS,
            uv_mode: DEFAULT_UV_MODE_PROBS,
            partition: DEFAULT_PARTITION_PROBS,
            mv: DEFAULT_NMV_CONTEXT,
        }
    }
}

/// libvpx `DIFF_UPDATE_PROB`.
const DIFF_UPDATE_PROB: u8 = 252;

/// libvpx `inv_map_table` (vp9_dsubexp.c) for subexp prob updates.
#[rustfmt::skip]
const INV_MAP_TABLE: [u8; 255] = [7, 20, 33, 46, 59, 72, 85, 98, 111, 124, 137, 150, 163, 176, 189, 202, 215, 228, 241, 254, 1, 2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149, 151, 152, 153, 154, 155, 156, 157, 158, 159, 160, 161, 162, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 190, 191, 192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 203, 204, 205, 206, 207, 208, 209, 210, 211, 212, 213, 214, 216, 217, 218, 219, 220, 221, 222, 223, 224, 225, 226, 227, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, 240, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 253];

/// `inv_recenter_nonneg` (vp9_dsubexp.c:17-21).
pub(super) fn inv_recenter_nonneg(v: i32, m: i32) -> i32 {
    if v > 2 * m {
        return v;
    }
    if v & 1 != 0 {
        m - ((v + 1) >> 1)
    } else {
        m + (v >> 1)
    }
}

/// `decode_uniform` (vp9_dsubexp.c:23-28).
pub(super) fn decode_uniform(r: &mut BoolReader<'_>) -> i32 {
    let m = (1 << 8) - 191; // 65
    let v = r.read_literal(7) as i32;
    if v < m {
        v
    } else {
        (v << 1) - m + i32::from(r.read_bit())
    }
}

/// `inv_remap_prob` (vp9_dsubexp.c:30-58). `MAX_PROB` = 255.
pub(super) fn inv_remap_prob(v: i32, m0: u8) -> u8 {
    let v = i32::from(INV_MAP_TABLE[v as usize]);
    let m = i32::from(m0) - 1;
    if (m << 1) <= 255 {
        (1 + inv_recenter_nonneg(v, m)) as u8
    } else {
        (255 - inv_recenter_nonneg(v, 255 - 1 - m)) as u8
    }
}

/// `decode_term_subexp` (vp9_dsubexp.c:60-65).
pub(super) fn decode_term_subexp(r: &mut BoolReader<'_>) -> i32 {
    if !r.read_bit() {
        return r.read_literal(4) as i32;
    }
    if !r.read_bit() {
        return r.read_literal(4) as i32 + 16;
    }
    if !r.read_bit() {
        return r.read_literal(5) as i32 + 32;
    }
    decode_uniform(r) + 64
}

/// `vp9_diff_update_prob` (vp9_dsubexp.c:67-72).
pub(super) fn diff_update_prob(r: &mut BoolReader<'_>, p: &mut u8) {
    if r.read_bool(DIFF_UPDATE_PROB) {
        let delp = decode_term_subexp(r);
        *p = inv_remap_prob(delp, *p);
    }
}

/// `BAND_COEFF_CONTEXTS(band)`: band 0 has 3 contexts, others 6.
fn band_coeff_contexts(band: usize) -> usize {
    if band == 0 {
        3
    } else {
        6
    }
}

/// libvpx `MV_UPDATE_PROB` (`vp9/common/vp9_entropymv.h:35`).
///
/// Deliberately a *different* constant from [`DIFF_UPDATE_PROB`] even though
/// both happen to be 252: they gate two different update encodings, and
/// merging them would hide that.
const MV_UPDATE_PROB: u8 = 252;

/// libvpx `SWITCHABLE` (`vp9/common/vp9_filter.h:32`), the sentinel the
/// frame-level `interp_filter` takes when each block codes its own filter.
///
/// [`UncompressedHeader::interp_filter`] stores this raw value: `4` for
/// switchable, otherwise the *raw 2-bit literal* `0..=3` straight off the
/// bitstream (`uncompressed.rs`'s `parse_interp_filter`), **not** yet mapped
/// through `LITERAL_TO_FILTER`. Compare against this constant before any
/// such mapping.
pub const SWITCHABLE: u8 = 4;

/// libvpx `REFERENCE_MODE` (`vp9/common/vp9_onyxc_int.h:53-58`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceMode {
    /// `SINGLE_REFERENCE`: every block predicts from one reference.
    Single = 0,
    /// `COMPOUND_REFERENCE`: every block predicts from two.
    Compound = 1,
    /// `REFERENCE_MODE_SELECT`: coded per block.
    Select = 2,
}

/// The frame-level inputs `read_compressed_header` reads out of
/// `VP9_COMMON` / `MACROBLOCKD` rather than out of the compressed partition.
///
/// Grouping them keeps the parse signature small and, more importantly,
/// makes the `frame_is_intra_only` trap explicit: it is "key frame **or**
/// intra-only frame" (`frame_is_intra_only`, `vp9_onyxc_int.h`), not the
/// `intra_only` bit alone. Feeding the raw bit here makes a key frame read
/// the inter sections and desynchronise the whole partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderCfg {
    /// `xd->lossless` — forces `ONLY_4X4` instead of reading a tx mode.
    pub lossless: bool,
    /// `frame_is_intra_only(cm)` — [`UncompressedHeader::is_intra_only`].
    pub frame_is_intra_only: bool,
    /// `cm->interp_filter`, raw (see [`SWITCHABLE`]).
    pub interp_filter: u8,
    /// `cm->allow_high_precision_mv`.
    pub allow_high_precision_mv: bool,
    /// `cm->ref_frame_sign_bias`, indexed by reference frame.
    pub ref_frame_sign_bias: [bool; 4],
}

impl HeaderCfg {
    /// Reads every field off a parsed uncompressed header.
    #[must_use]
    pub fn from_header(hdr: &UncompressedHeader) -> Self {
        Self {
            lossless: hdr.quant.lossless(),
            frame_is_intra_only: hdr.is_intra_only(),
            interp_filter: hdr.interp_filter,
            allow_high_precision_mv: hdr.allow_high_precision_mv,
            ref_frame_sign_bias: hdr.ref_frame_sign_bias,
        }
    }

    /// The configuration of a key / intra-only frame, where every field
    /// except `lossless` is unread.
    #[must_use]
    pub fn intra(lossless: bool) -> Self {
        Self {
            lossless,
            frame_is_intra_only: true,
            interp_filter: 0,
            allow_high_precision_mv: false,
            ref_frame_sign_bias: [false; 4],
        }
    }
}

/// What a compressed header decides, beyond the probability updates it
/// writes into [`FrameProbs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompressedHeader {
    /// `cm->tx_mode`.
    pub tx_mode: TxMode,
    /// `cm->reference_mode`. Always [`ReferenceMode::Single`] on an intra
    /// frame, which codes no reference mode at all.
    pub reference_mode: ReferenceMode,
}

/// `read_tx_mode` (`vp9_decodeframe.c:68-73`).
fn read_tx_mode(r: &mut BoolReader<'_>) -> TxMode {
    let mut v = r.read_literal(2);
    if v == 3 {
        v += u32::from(r.read_bit());
    }
    match v {
        0 => TxMode::Only4x4,
        1 => TxMode::Allow8x8,
        2 => TxMode::Allow16x16,
        3 => TxMode::Allow32x32,
        _ => TxMode::Select,
    }
}

/// `read_tx_mode_probs` (`vp9_decodeframe.c:75-89`).
fn read_tx_mode_probs(r: &mut BoolReader<'_>, probs: &mut FrameProbs) {
    for row in &mut probs.tx8 {
        for p in row {
            diff_update_prob(r, p);
        }
    }
    for row in &mut probs.tx16 {
        for p in row {
            diff_update_prob(r, p);
        }
    }
    for row in &mut probs.tx32 {
        for p in row {
            diff_update_prob(r, p);
        }
    }
}

/// `read_coef_probs` (`vp9_decodeframe.c:1314-1333`): one optional update
/// pass per coded transform size.
fn read_coef_probs(r: &mut BoolReader<'_>, probs: &mut FrameProbs, tx_mode: TxMode) {
    for tx_size in 0..=tx_mode.biggest_tx_size() {
        if r.read_bit() {
            for plane in 0..2 {
                for is_inter in 0..2 {
                    for band in 0..6 {
                        for ctx in 0..band_coeff_contexts(band) {
                            for node in 0..3 {
                                diff_update_prob(
                                    r,
                                    &mut probs.coef[tx_size][plane][is_inter][band][ctx][node],
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `read_inter_mode_probs` (`vp9_decodeframe.c:98-103`).
fn read_inter_mode_probs(r: &mut BoolReader<'_>, probs: &mut FrameProbs) {
    for ctx in &mut probs.inter_mode {
        for p in ctx {
            diff_update_prob(r, p);
        }
    }
}

/// `read_switchable_interp_probs` (`vp9_decodeframe.c:91-96`).
fn read_switchable_interp_probs(r: &mut BoolReader<'_>, probs: &mut FrameProbs) {
    for ctx in &mut probs.switchable_interp {
        for p in ctx {
            diff_update_prob(r, p);
        }
    }
}

/// `read_frame_reference_mode` (`vp9_decodeframe.c:105-114`), verbatim:
///
/// ```c
/// if (vp9_compound_reference_allowed(cm)) {
///   return vpx_read_bit(r)
///              ? (vpx_read_bit(r) ? REFERENCE_MODE_SELECT : COMPOUND_REFERENCE)
///              : SINGLE_REFERENCE;
/// } else {
///   return SINGLE_REFERENCE;
/// }
/// ```
///
/// The `else` branch reads **no bits at all**, so a frame whose reference
/// sign biases all agree has two fewer symbols in its header than one whose
/// do not.
fn read_frame_reference_mode(r: &mut BoolReader<'_>, sign_bias: &[bool; 4]) -> ReferenceMode {
    if is_compound_reference_allowed(sign_bias) {
        if r.read_bit() {
            if r.read_bit() {
                ReferenceMode::Select
            } else {
                ReferenceMode::Compound
            }
        } else {
            ReferenceMode::Single
        }
    } else {
        ReferenceMode::Single
    }
}

/// `read_frame_reference_mode_probs` (`vp9_decodeframe.c:116-133`).
///
/// Note the asymmetry: `comp_inter` only for `SELECT`, `single_ref` for
/// anything that is not `COMPOUND` (so `SELECT` reads it too), `comp_ref`
/// for anything that is not `SINGLE`. `SELECT` therefore reads all three.
fn read_frame_reference_mode_probs(
    r: &mut BoolReader<'_>,
    probs: &mut FrameProbs,
    reference_mode: ReferenceMode,
) {
    if reference_mode == ReferenceMode::Select {
        for p in &mut probs.comp_inter {
            diff_update_prob(r, p);
        }
    }
    if reference_mode != ReferenceMode::Compound {
        for ctx in &mut probs.single_ref {
            diff_update_prob(r, &mut ctx[0]);
            diff_update_prob(r, &mut ctx[1]);
        }
    }
    if reference_mode != ReferenceMode::Single {
        for p in &mut probs.comp_ref {
            diff_update_prob(r, p);
        }
    }
}

/// `update_mv_probs` (`vp9_decodeframe.c:135-139`):
///
/// ```c
/// for (i = 0; i < n; ++i)
///   if (vpx_read(r, MV_UPDATE_PROB)) p[i] = (vpx_read_literal(r, 7) << 1) | 1;
/// ```
///
/// **Not** a subexponential update: a raw 7-bit literal, forced odd. Values
/// written by this path are always odd and never zero.
fn update_mv_probs(r: &mut BoolReader<'_>, probs: &mut [u8]) {
    for p in probs {
        if r.read_bool(MV_UPDATE_PROB) {
            *p = ((r.read_literal(7) << 1) | 1) as u8;
        }
    }
}

/// `read_mv_probs` (`vp9_decodeframe.c:141-168`).
///
/// Three **sequential** passes, in this order:
///
/// 1. `joints`, then for each component `sign`, `classes`, `class0`, `bits`;
/// 2. for each component `class0_fp[0..CLASS0_SIZE]`, then `fp`;
/// 3. only when `allow_hp`, for each component `class0_hp` then `hp`.
///
/// Fusing passes 1 and 2 into a single per-component loop — the obvious
/// "simplification" — reorders the bitstream and silently mis-decodes every
/// motion vector in the frame.
fn read_mv_probs(r: &mut BoolReader<'_>, mv: &mut NmvContext, allow_hp: bool) {
    update_mv_probs(r, &mut mv.joints);

    for comp in &mut mv.comps {
        update_mv_probs(r, std::slice::from_mut(&mut comp.sign));
        update_mv_probs(r, &mut comp.classes);
        update_mv_probs(r, &mut comp.class0);
        update_mv_probs(r, &mut comp.bits);
    }

    for comp in &mut mv.comps {
        for fp in &mut comp.class0_fp {
            update_mv_probs(r, fp);
        }
        update_mv_probs(r, &mut comp.fp);
    }

    if allow_hp {
        for comp in &mut mv.comps {
            update_mv_probs(r, std::slice::from_mut(&mut comp.class0_hp));
            update_mv_probs(r, std::slice::from_mut(&mut comp.hp));
        }
    }
}

/// Parses a compressed header out of an already-initialised bool reader,
/// updating `probs` in place — the body of `read_compressed_header`
/// (`vp9_decodeframe.c:2879-2911`) without its reader setup or its
/// `vpx_reader_has_error` return.
///
/// Split out from [`parse_compressed_header`] so a caller (today: this
/// module's tests) can keep reading from the same reader afterwards and
/// check that the parse consumed exactly the symbols it should have.
pub(super) fn parse_compressed_header_into(
    r: &mut BoolReader<'_>,
    cfg: &HeaderCfg,
    probs: &mut FrameProbs,
) -> CompressedHeader {
    let tx_mode = if cfg.lossless {
        TxMode::Only4x4
    } else {
        read_tx_mode(r)
    };
    if tx_mode == TxMode::Select {
        read_tx_mode_probs(r, probs);
    }
    read_coef_probs(r, probs, tx_mode);

    for p in &mut probs.skip {
        diff_update_prob(r, p);
    }

    let mut reference_mode = ReferenceMode::Single;
    if !cfg.frame_is_intra_only {
        read_inter_mode_probs(r, probs);

        if cfg.interp_filter == SWITCHABLE {
            read_switchable_interp_probs(r, probs);
        }

        for p in &mut probs.intra_inter {
            diff_update_prob(r, p);
        }

        reference_mode = read_frame_reference_mode(r, &cfg.ref_frame_sign_bias);
        // `vp9_setup_compound_reference_mode(cm)` runs here for a non-single
        // mode; it reads no bits, and this decoder derives the same
        // comp_fixed_ref / comp_var_ref on demand from the sign biases via
        // `refs::setup_compound_reference_mode`, so the returned
        // `reference_mode` is all the caller needs.
        read_frame_reference_mode_probs(r, probs, reference_mode);

        for group in &mut probs.y_mode {
            for p in group {
                diff_update_prob(r, p);
            }
        }

        for ctx in &mut probs.partition {
            for p in ctx {
                diff_update_prob(r, p);
            }
        }

        read_mv_probs(r, &mut probs.mv, cfg.allow_high_precision_mv);
    }

    CompressedHeader {
        tx_mode,
        reference_mode,
    }
}

/// Parses a compressed header of either frame type, updating `probs` in
/// place — `read_compressed_header` (`vp9_decodeframe.c:2866-2914`).
///
/// # Errors
///
/// Returns [`CodecError::InvalidBitstream`] on a bad marker bit or when the
/// bool decoder consumed more data than the partition holds (libvpx's
/// `vpx_reader_has_error` return value, which the caller turns into
/// `corrupted`).
pub fn parse_compressed_header(
    data: &[u8],
    cfg: &HeaderCfg,
    probs: &mut FrameProbs,
) -> CodecResult<CompressedHeader> {
    let mut r = BoolReader::new(data).ok_or_else(|| {
        CodecError::InvalidBitstream("VP9 compressed header: invalid marker bit".into())
    })?;
    let header = parse_compressed_header_into(&mut r, cfg, probs);
    if r.has_error() {
        return Err(CodecError::InvalidBitstream(
            "VP9 compressed header overran its partition".into(),
        ));
    }
    Ok(header)
}

/// Parses the compressed header of a key / intra-only frame, where every
/// inter section is absent. Returns the parsed `TxMode`.
///
/// A thin wrapper over [`parse_compressed_header`] with
/// [`HeaderCfg::intra`]; kept as its own entry point because that is the
/// only shape the intra reconstruction path needs.
///
/// # Errors
///
/// Returns [`CodecError::InvalidBitstream`] on a bad marker bit or when the
/// bool decoder consumed more data than the partition holds.
pub fn parse_compressed_header_intra(
    data: &[u8],
    lossless: bool,
    probs: &mut FrameProbs,
) -> CodecResult<TxMode> {
    parse_compressed_header(data, &HeaderCfg::intra(lossless), probs).map(|h| h.tx_mode)
}

#[cfg(test)]
mod tests {
    use super::super::boolenc::BoolWriter;
    use super::super::tables_inter::{
        CLASS0_SIZE, MV_CLASSES, MV_FP_SIZE, MV_JOINTS, MV_OFFSET_BITS,
    };
    use super::*;

    // ---------------------------------------------------------------------
    // Encoder side of the subexp / MV update machinery
    // ---------------------------------------------------------------------

    /// Inverse of [`decode_uniform`] (`vp9_dsubexp.c:23-28`).
    ///
    /// For `value >= m` the decoder computes `(v << 1) - m + bit`, so the
    /// encoder must split `value + m` into its top 7 bits and its low bit —
    /// the single easiest place in this file to get an off-by-one.
    fn encode_uniform(w: &mut BoolWriter, value: i32) {
        let m = (1 << 8) - 191; // 65
        if value < m {
            w.write_literal(value as u32, 7);
        } else {
            let t = (value + m) as u32;
            w.write_literal(t >> 1, 7);
            w.write_bit(t & 1 != 0);
        }
    }

    /// Inverse of [`decode_term_subexp`] (`vp9_dsubexp.c:60-65`).
    fn encode_term_subexp(w: &mut BoolWriter, delp: i32) {
        if delp < 16 {
            w.write_bit(false);
            w.write_literal(delp as u32, 4);
        } else if delp < 32 {
            w.write_bit(true);
            w.write_bit(false);
            w.write_literal((delp - 16) as u32, 4);
        } else if delp < 64 {
            w.write_bit(true);
            w.write_bit(true);
            w.write_bit(false);
            w.write_literal((delp - 32) as u32, 5);
        } else {
            w.write_bit(true);
            w.write_bit(true);
            w.write_bit(true);
            encode_uniform(w, delp - 64);
        }
    }

    /// The largest `delp` [`INV_MAP_TABLE`] can index.
    const MAX_DELP: i32 = 254;

    // ---------------------------------------------------------------------
    // An independent transcription of the header's update order
    // ---------------------------------------------------------------------

    /// How a slot's update is coded.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum SlotKind {
        /// `vp9_diff_update_prob`: flag at 252, then a subexp delta.
        Subexp,
        /// `update_mv_probs`: flag at 252, then a raw 7-bit literal.
        Mv,
    }

    /// One updatable probability of the compressed header.
    #[derive(Clone, Debug)]
    struct Slot {
        name: String,
        kind: SlotKind,
        /// The value the slot holds in the `FrameProbs` this list was built
        /// from (the prior when building from pre-parse probabilities, the
        /// result when building from post-parse ones).
        value: u8,
    }

    /// An element of the header's update stream: either a probability slot
    /// or the structural reference-mode bits, which carry no probability but
    /// sit *between* `intra_inter` and the per-mode reference probabilities.
    #[derive(Clone, Debug)]
    enum Item {
        Prob(Slot),
        RefModeBits,
    }

    fn sub(out: &mut Vec<Item>, name: String, value: u8) {
        out.push(Item::Prob(Slot {
            name,
            kind: SlotKind::Subexp,
            value,
        }));
    }

    fn mvp(out: &mut Vec<Item>, name: String, value: u8) {
        out.push(Item::Prob(Slot {
            name,
            kind: SlotKind::Mv,
            value,
        }));
    }

    /// The compressed header's update stream from `skip` onward, in libvpx
    /// read order — a **second, independent** transcription of
    /// `vp9_decodeframe.c:2883-2910`, deliberately written from the C source
    /// rather than from [`parse_compressed_header_into`], so that a
    /// mis-ordered parse and this list disagree.
    ///
    /// The `tx_mode` / `tx_probs` / `coef` prologue is not modelled: it is
    /// already pinned bit-exactly by the four `keyframe_*_bit_exact_vs_libvpx`
    /// fixtures, and the tests here encode it as a fixed no-update preamble.
    fn header_items(p: &FrameProbs, cfg: &HeaderCfg, rm: ReferenceMode) -> Vec<Item> {
        let mut out = Vec::new();
        for (i, v) in p.skip.iter().enumerate() {
            sub(&mut out, format!("skip[{i}]"), *v);
        }
        if cfg.frame_is_intra_only {
            return out;
        }

        for (i, ctx) in p.inter_mode.iter().enumerate() {
            for (j, v) in ctx.iter().enumerate() {
                sub(&mut out, format!("inter_mode[{i}][{j}]"), *v);
            }
        }
        if cfg.interp_filter == SWITCHABLE {
            for (j, ctx) in p.switchable_interp.iter().enumerate() {
                for (i, v) in ctx.iter().enumerate() {
                    sub(&mut out, format!("switchable_interp[{j}][{i}]"), *v);
                }
            }
        }
        for (i, v) in p.intra_inter.iter().enumerate() {
            sub(&mut out, format!("intra_inter[{i}]"), *v);
        }

        out.push(Item::RefModeBits);

        if rm == ReferenceMode::Select {
            for (i, v) in p.comp_inter.iter().enumerate() {
                sub(&mut out, format!("comp_inter[{i}]"), *v);
            }
        }
        if rm != ReferenceMode::Compound {
            for (i, ctx) in p.single_ref.iter().enumerate() {
                sub(&mut out, format!("single_ref[{i}][0]"), ctx[0]);
                sub(&mut out, format!("single_ref[{i}][1]"), ctx[1]);
            }
        }
        if rm != ReferenceMode::Single {
            for (i, v) in p.comp_ref.iter().enumerate() {
                sub(&mut out, format!("comp_ref[{i}]"), *v);
            }
        }

        for (j, group) in p.y_mode.iter().enumerate() {
            for (i, v) in group.iter().enumerate() {
                sub(&mut out, format!("y_mode[{j}][{i}]"), *v);
            }
        }
        for (j, ctx) in p.partition.iter().enumerate() {
            for (i, v) in ctx.iter().enumerate() {
                sub(&mut out, format!("partition[{j}][{i}]"), *v);
            }
        }

        // read_mv_probs, pass 1.
        for (i, v) in p.mv.joints.iter().enumerate() {
            mvp(&mut out, format!("mv.joints[{i}]"), *v);
        }
        for (c, comp) in p.mv.comps.iter().enumerate() {
            mvp(&mut out, format!("mv.comps[{c}].sign"), comp.sign);
            for (i, v) in comp.classes.iter().enumerate() {
                mvp(&mut out, format!("mv.comps[{c}].classes[{i}]"), *v);
            }
            for (i, v) in comp.class0.iter().enumerate() {
                mvp(&mut out, format!("mv.comps[{c}].class0[{i}]"), *v);
            }
            for (i, v) in comp.bits.iter().enumerate() {
                mvp(&mut out, format!("mv.comps[{c}].bits[{i}]"), *v);
            }
        }
        // read_mv_probs, pass 2 — a *separate* walk over the components.
        for (c, comp) in p.mv.comps.iter().enumerate() {
            for (j, row) in comp.class0_fp.iter().enumerate() {
                for (i, v) in row.iter().enumerate() {
                    mvp(&mut out, format!("mv.comps[{c}].class0_fp[{j}][{i}]"), *v);
                }
            }
            for (i, v) in comp.fp.iter().enumerate() {
                mvp(&mut out, format!("mv.comps[{c}].fp[{i}]"), *v);
            }
        }
        // read_mv_probs, pass 3 — only when the frame codes high precision.
        if cfg.allow_high_precision_mv {
            for (c, comp) in p.mv.comps.iter().enumerate() {
                mvp(&mut out, format!("mv.comps[{c}].class0_hp"), comp.class0_hp);
                mvp(&mut out, format!("mv.comps[{c}].hp"), comp.hp);
            }
        }
        out
    }

    /// Just the probability slots of [`header_items`], in order.
    fn header_slots(p: &FrameProbs, cfg: &HeaderCfg, rm: ReferenceMode) -> Vec<Slot> {
        header_items(p, cfg, rm)
            .into_iter()
            .filter_map(|it| match it {
                Item::Prob(s) => Some(s),
                Item::RefModeBits => None,
            })
            .collect()
    }

    /// What to write for one slot.
    #[derive(Clone, Copy, Debug)]
    enum Upd {
        /// No update: just the `false` flag.
        None,
        /// A subexp update with this delta.
        Subexp(i32),
        /// An MV update with this 7-bit literal.
        Mv(u32),
    }

    impl Upd {
        /// The value the slot must hold after the parse.
        fn expect(self, prior: u8) -> u8 {
            match self {
                Upd::None => prior,
                Upd::Subexp(delp) => inv_remap_prob(delp, prior),
                Upd::Mv(lit) => ((lit << 1) | 1) as u8,
            }
        }
    }

    /// Encodes a whole compressed header: the fixed intra prologue, then one
    /// coded update per slot of [`header_items`].
    ///
    /// # Panics
    ///
    /// Panics when an update's kind does not match its slot's — a
    /// test-authoring mistake, not a decoder condition.
    fn encode_header(
        w: &mut BoolWriter,
        cfg: &HeaderCfg,
        tx_mode: TxMode,
        rm: ReferenceMode,
        items: &[Item],
        updates: &[Upd],
    ) {
        // read_tx_mode (skipped entirely when lossless).
        if !cfg.lossless {
            match tx_mode {
                TxMode::Only4x4 => w.write_literal(0, 2),
                TxMode::Allow8x8 => w.write_literal(1, 2),
                TxMode::Allow16x16 => w.write_literal(2, 2),
                TxMode::Allow32x32 => {
                    w.write_literal(3, 2);
                    w.write_bit(false);
                }
                TxMode::Select => {
                    w.write_literal(3, 2);
                    w.write_bit(true);
                }
            }
        }
        // read_tx_mode_probs: 2*1 + 2*2 + 2*3 = 12 no-update flags.
        if tx_mode == TxMode::Select {
            for _ in 0..12 {
                w.write_bool(DIFF_UPDATE_PROB, false);
            }
        }
        // read_coef_probs: one "no updates for this tx size" bit each.
        for _ in 0..=tx_mode.biggest_tx_size() {
            w.write_bit(false);
        }

        let mut next = 0usize;
        for item in items {
            match item {
                Item::RefModeBits => {
                    if is_compound_reference_allowed(&cfg.ref_frame_sign_bias) {
                        match rm {
                            ReferenceMode::Single => w.write_bit(false),
                            ReferenceMode::Compound => {
                                w.write_bit(true);
                                w.write_bit(false);
                            }
                            ReferenceMode::Select => {
                                w.write_bit(true);
                                w.write_bit(true);
                            }
                        }
                    } else {
                        assert_eq!(
                            rm,
                            ReferenceMode::Single,
                            "compound is not allowed by these sign biases"
                        );
                    }
                }
                Item::Prob(slot) => {
                    let upd = updates[next];
                    next += 1;
                    match (slot.kind, upd) {
                        (_, Upd::None) => w.write_bool(DIFF_UPDATE_PROB, false),
                        (SlotKind::Subexp, Upd::Subexp(delp)) => {
                            w.write_bool(DIFF_UPDATE_PROB, true);
                            encode_term_subexp(w, delp);
                        }
                        (SlotKind::Mv, Upd::Mv(lit)) => {
                            w.write_bool(MV_UPDATE_PROB, true);
                            w.write_literal(lit, 7);
                        }
                        (kind, upd) => panic!("slot {} is {kind:?}, got {upd:?}", slot.name),
                    }
                }
            }
        }
        assert_eq!(next, updates.len(), "update count must match slot count");
    }

    /// A 16-bit sentinel written straight after the header, so a parse that
    /// consumed the wrong number of symbols is caught even when every
    /// probability it produced happens to look plausible.
    const SENTINEL: u32 = 0xA5C3;

    /// Baseline configuration: non-lossless `ONLY_4X4`, fixed interpolation
    /// filter, sign biases that forbid compound prediction, no high-precision
    /// motion vectors.
    fn base_cfg() -> HeaderCfg {
        HeaderCfg {
            lossless: false,
            frame_is_intra_only: false,
            interp_filter: 0,
            allow_high_precision_mv: false,
            ref_frame_sign_bias: [false; 4],
        }
    }

    /// Sign biases for which `vp9_compound_reference_allowed` is true.
    fn compound_sign_bias() -> [bool; 4] {
        [false, false, false, true]
    }

    /// Encodes a header, parses it back, and checks the sentinel that
    /// follows it — i.e. that the parse consumed *exactly* the symbols the
    /// encoder wrote.
    fn round_trip(
        cfg: &HeaderCfg,
        tx_mode: TxMode,
        rm: ReferenceMode,
        updates: &[Upd],
    ) -> (FrameProbs, CompressedHeader, usize, usize) {
        let priors = FrameProbs::defaults();
        let items = header_items(&priors, cfg, rm);
        let mut w = BoolWriter::new();
        encode_header(&mut w, cfg, tx_mode, rm, &items, updates);
        let at_252 = w.writes_at(252);
        let at_128 = w.writes_at(128);
        w.write_literal(SENTINEL, 16);
        let buf = w.finish();

        let mut probs = priors;
        let mut r = BoolReader::new(&buf).expect("marker bit");
        let header = parse_compressed_header_into(&mut r, cfg, &mut probs);
        assert!(!r.has_error(), "parse overran its own encode");
        assert_eq!(
            r.read_literal(16),
            SENTINEL,
            "the parse consumed the wrong number of symbols"
        );
        (probs, header, at_252, at_128)
    }

    #[test]
    fn defaults_have_real_coef_probs_not_uniform() {
        let p = FrameProbs::defaults();
        // libvpx default_coef_probs_4x4[0][0][0][0] = { 195, 29, 183 }
        assert_eq!(p.coef[0][0][0][0][0], [195, 29, 183]);
        assert_eq!(p.skip, [192, 128, 64]);
    }

    #[test]
    fn inv_remap_matches_reference_examples() {
        // v=0 -> inv_map_table[0] = 7; m = 128-1 = 127; (127<<1) <= 255
        // -> 1 + inv_recenter_nonneg(7, 127); 7 is odd -> 127 - 4 = 123
        // -> 124 (hand-traced against vp9_dsubexp.c).
        assert_eq!(inv_remap_prob(0, 128), 124);
        // v=20 -> inv_map_table[20] = 1; 1 is odd -> 127 - 1 = 126 -> 127.
        assert_eq!(inv_remap_prob(20, 128), 127);
    }

    #[test]
    fn bad_marker_bit_is_error() {
        let mut probs = FrameProbs::defaults();
        let e = parse_compressed_header_intra(&[0xFF, 0, 0, 0], false, &mut probs);
        assert!(e.is_err());
    }

    // ---------------------------------------------------------------------
    // The update primitives
    // ---------------------------------------------------------------------

    /// Every representable subexp delta must survive encode -> decode.
    /// `decode_uniform`'s `value >= 65` branch is the one that silently
    /// off-by-ones, and it only covers `delp >= 129`.
    #[test]
    fn subexp_encoding_inverts_the_decoder_for_every_delta() {
        let mut w = BoolWriter::new();
        for delp in 0..=MAX_DELP {
            encode_term_subexp(&mut w, delp);
        }
        let buf = w.finish();
        let mut r = BoolReader::new(&buf).expect("marker bit");
        for delp in 0..=MAX_DELP {
            assert_eq!(decode_term_subexp(&mut r), delp, "delta {delp}");
        }
        assert!(!r.has_error());
    }

    /// `INV_MAP_TABLE` has exactly `MAX_PROB` entries and `decode_term_subexp`
    /// can produce exactly that many distinct deltas — an index one past the
    /// end would panic rather than mis-decode, so pin the bound.
    #[test]
    fn subexp_delta_range_matches_the_inverse_map_table() {
        assert_eq!(INV_MAP_TABLE.len(), 255);
        assert_eq!(MAX_DELP, INV_MAP_TABLE.len() as i32 - 1);
        // The largest value decode_term_subexp can return: uniform's largest
        // (v = 127 -> (127 << 1) - 65 + 1 = 190) plus 64.
        assert_eq!(190 + 64, MAX_DELP);
    }

    /// `diff_update_prob` with `delp == 0` must never be a no-op — that is
    /// the property [`each_slot_updates_exactly_its_own_field`] relies on to
    /// say "exactly this field changed".
    ///
    /// `inv_map_table[0]` is 7, so the update recenters 7 around the prior.
    /// Away from the ends that is exactly `-4` below 128 and `+4` above, but
    /// `inv_recenter_nonneg`'s `v > 2 * m` early return takes over once the
    /// prior is within 4 of an end, pinning the result at 8 or 248. Both
    /// branches are asserted; neither can return the prior unchanged.
    #[test]
    fn zero_delta_always_moves_a_prob() {
        for prior in 1..=255u8 {
            let after = inv_remap_prob(0, prior);
            assert_ne!(after, prior, "delta 0 must move prior {prior}");
            let expected = match prior {
                // v = 7 > 2 * (prior - 1): the early return, result 1 + 7.
                1..=4 => 8,
                // Below the midpoint: prior - 4.
                5..=128 => prior - 4,
                // Above it: prior + 4, until the mirrored early return.
                129..=251 => prior + 4,
                // v = 7 > 2 * (255 - prior): result 255 - 7.
                _ => 248,
            };
            assert_eq!(after, expected, "prior {prior}");
        }
    }

    // ---------------------------------------------------------------------
    // Symbol counts: the shape of the header, per configuration
    // ---------------------------------------------------------------------

    /// A zero-update header must consume an exact, hand-derived number of
    /// symbols for every combination of the four conditionals in
    /// `read_compressed_header`'s inter half.
    ///
    /// The counts come from the C source, not from this implementation:
    ///
    /// | section | flags at 252 |
    /// |---|---|
    /// | `skip[3]` | 3 |
    /// | `inter_mode[7][3]` | 21 |
    /// | `switchable_interp[4][2]` (only if `SWITCHABLE`) | 8 |
    /// | `intra_inter[4]` | 4 |
    /// | `comp_inter[5]` (only if `SELECT`) | 5 |
    /// | `single_ref[5][2]` (unless `COMPOUND`) | 10 |
    /// | `comp_ref[5]` (unless `SINGLE`) | 5 |
    /// | `y_mode[4][9]` | 36 |
    /// | `partition[16][3]` | 48 |
    /// | mv pass 1: joints 3 + 2x(1+10+1+10) | 47 |
    /// | mv pass 2: 2x(2x3 + 3) | 18 |
    /// | mv pass 3: 2x(1+1) (only if `allow_hp`) | 4 |
    ///
    /// Bits at 128 are the partition marker, the 2-bit `tx_mode` literal,
    /// one "no coef updates" flag for `ONLY_4X4`, and the 1 or 2
    /// reference-mode bits when compound prediction is allowed.
    #[test]
    fn zero_update_header_consumes_exactly_the_expected_symbols() {
        let cases: [(&str, HeaderCfg, ReferenceMode, usize, usize); 7] = [
            // Intra: the whole inter half is absent.
            (
                "intra",
                HeaderCfg::intra(false),
                ReferenceMode::Single,
                3,
                4,
            ),
            ("baseline", base_cfg(), ReferenceMode::Single, 187, 4),
            (
                "switchable",
                HeaderCfg {
                    interp_filter: SWITCHABLE,
                    ..base_cfg()
                },
                ReferenceMode::Single,
                187 + 8,
                4,
            ),
            (
                "allow_hp",
                HeaderCfg {
                    allow_high_precision_mv: true,
                    ..base_cfg()
                },
                ReferenceMode::Single,
                187 + 4,
                4,
            ),
            (
                "compound-allowed, single",
                HeaderCfg {
                    ref_frame_sign_bias: compound_sign_bias(),
                    ..base_cfg()
                },
                ReferenceMode::Single,
                187,
                5,
            ),
            (
                "compound",
                HeaderCfg {
                    ref_frame_sign_bias: compound_sign_bias(),
                    ..base_cfg()
                },
                ReferenceMode::Compound,
                187 - 10 + 5,
                6,
            ),
            (
                "select",
                HeaderCfg {
                    ref_frame_sign_bias: compound_sign_bias(),
                    ..base_cfg()
                },
                ReferenceMode::Select,
                187 + 5 + 5,
                6,
            ),
        ];

        for (label, cfg, rm, want_252, want_128) in cases {
            let slots = header_slots(&FrameProbs::defaults(), &cfg, rm);
            let updates = vec![Upd::None; slots.len()];
            let (probs, header, at_252, at_128) = round_trip(&cfg, TxMode::Only4x4, rm, &updates);

            assert_eq!(slots.len(), want_252, "{label}: slot count");
            assert_eq!(at_252, want_252, "{label}: flags at DIFF/MV_UPDATE_PROB");
            assert_eq!(at_128, want_128, "{label}: bits at probability 128");
            assert_eq!(header.tx_mode, TxMode::Only4x4, "{label}");
            assert_eq!(header.reference_mode, rm, "{label}");
            assert!(
                probs == FrameProbs::defaults(),
                "{label}: a zero-update header must not move any probability"
            );
        }
    }

    /// The same, for the two `tx_mode` shapes that change the prologue.
    #[test]
    fn tx_mode_prologue_shapes_are_exact() {
        // TX_MODE_SELECT: 3 literal bits, then 12 tx-prob flags, then four
        // coef flags (biggest_tx_size == 3).
        let cfg = base_cfg();
        let slots = header_slots(&FrameProbs::defaults(), &cfg, ReferenceMode::Single);
        let updates = vec![Upd::None; slots.len()];
        let (_, header, at_252, at_128) =
            round_trip(&cfg, TxMode::Select, ReferenceMode::Single, &updates);
        assert_eq!(header.tx_mode, TxMode::Select);
        assert_eq!(at_252, 187 + 12, "12 tx-size probability flags");
        assert_eq!(at_128, 1 + 3 + 4, "marker + 3 literal bits + 4 coef flags");

        // Lossless: no tx_mode literal at all, ONLY_4X4 forced.
        let cfg = HeaderCfg {
            lossless: true,
            ..base_cfg()
        };
        let (_, header, at_252, at_128) =
            round_trip(&cfg, TxMode::Only4x4, ReferenceMode::Single, &updates);
        assert_eq!(header.tx_mode, TxMode::Only4x4);
        assert_eq!(at_252, 187);
        assert_eq!(at_128, 1 + 1, "marker + the single coef flag");
    }

    // ---------------------------------------------------------------------
    // Positional identity: every slot lands in its own field
    // ---------------------------------------------------------------------

    /// Update **one** slot per encode and assert that exactly that one field
    /// moved. This is what catches a transposed index (`y_mode[i][j]` for
    /// `[j][i]`), a swapped pair (`single_ref[i][1]` before `[i][0]`) or a
    /// fused MV pass: those all leave the *count* right and the values
    /// plausible, and only a per-position check sees them.
    ///
    /// Run for all three reference modes, because
    /// `read_frame_reference_mode_probs` is asymmetric — `SELECT` reads
    /// `comp_inter`, `single_ref` and `comp_ref`; `COMPOUND` skips
    /// `single_ref`; `SINGLE` skips `comp_inter` and `comp_ref` — and a
    /// mis-attributed section in the shapes that *drop* a block is exactly
    /// what a `SELECT`-only test would miss.
    #[test]
    fn each_slot_updates_exactly_its_own_field() {
        let cfg = HeaderCfg {
            interp_filter: SWITCHABLE,
            allow_high_precision_mv: true,
            ref_frame_sign_bias: compound_sign_bias(),
            ..base_cfg()
        };
        let defaults = FrameProbs::defaults();

        for (rm, want_slots) in [
            (ReferenceMode::Single, 187 + 8 + 4),
            (ReferenceMode::Compound, 187 + 8 + 4 - 10 + 5),
            (ReferenceMode::Select, 187 + 8 + 4 + 5 + 5),
        ] {
            let slots = header_slots(&defaults, &cfg, rm);
            assert_eq!(
                slots.len(),
                want_slots,
                "{rm:?}: switchable + allow_hp, per-mode reference sections"
            );

            for target in 0..slots.len() {
                let mut updates = vec![Upd::None; slots.len()];
                updates[target] = match slots[target].kind {
                    // delp 0 never leaves a prior unchanged.
                    SlotKind::Subexp => Upd::Subexp(0),
                    // literal 0 stores probability 1, which no default equals.
                    SlotKind::Mv => Upd::Mv(0),
                };
                let (probs, header, _, _) = round_trip(&cfg, TxMode::Only4x4, rm, &updates);
                assert_eq!(header.reference_mode, rm);

                let after = header_slots(&probs, &cfg, rm);
                let moved: Vec<&str> = after
                    .iter()
                    .zip(slots.iter())
                    .filter(|(a, b)| a.value != b.value)
                    .map(|(a, _)| a.name.as_str())
                    .collect();
                assert_eq!(
                    moved,
                    vec![slots[target].name.as_str()],
                    "{rm:?}: updating slot {target} ({}) moved the wrong field(s)",
                    slots[target].name
                );
                assert_eq!(
                    after[target].value,
                    updates[target].expect(slots[target].value),
                    "{rm:?}: slot {} took the wrong value",
                    slots[target].name
                );
                // A slot the mode does *not* code must be untouched, checked
                // straight off the struct rather than through the slot walk.
                if rm == ReferenceMode::Single {
                    assert_eq!(probs.comp_inter, defaults.comp_inter);
                    assert_eq!(probs.comp_ref, defaults.comp_ref);
                } else if rm == ReferenceMode::Compound {
                    assert_eq!(probs.single_ref, defaults.single_ref);
                }
            }
        }
    }

    /// All slots updated at once, each with a different delta, and every
    /// resulting `FrameProbs` field checked against its expected value.
    #[test]
    fn every_header_prob_slot_round_trips_its_own_value() {
        let cfg = HeaderCfg {
            interp_filter: SWITCHABLE,
            allow_high_precision_mv: true,
            ref_frame_sign_bias: compound_sign_bias(),
            ..base_cfg()
        };
        let rm = ReferenceMode::Select;
        let defaults = FrameProbs::defaults();
        let slots = header_slots(&defaults, &cfg, rm);

        let updates: Vec<Upd> = slots
            .iter()
            .enumerate()
            .map(|(i, s)| match s.kind {
                SlotKind::Subexp => Upd::Subexp(((i * 37) % (MAX_DELP as usize + 1)) as i32),
                SlotKind::Mv => Upd::Mv((i % 128) as u32),
            })
            .collect();

        let (probs, header, _, _) = round_trip(&cfg, TxMode::Only4x4, rm, &updates);
        assert_eq!(header.reference_mode, ReferenceMode::Select);

        let after = header_slots(&probs, &cfg, rm);
        for (i, (a, prior)) in after.iter().zip(slots.iter()).enumerate() {
            assert_eq!(a.name, prior.name, "slot list must line up");
            assert_eq!(
                a.value,
                updates[i].expect(prior.value),
                "slot {i} ({}) took the wrong value",
                a.name
            );
        }

        // Spot-check straight off the struct, not through the slot walk, so
        // a walk that reads the wrong field would still be caught.
        assert_eq!(probs.skip[0], inv_remap_prob(0, defaults.skip[0]));
        assert_eq!(
            probs.inter_mode[0][0],
            inv_remap_prob((3 * 37) % 255, defaults.inter_mode[0][0])
        );
        assert!(
            probs.mv.comps[0].classes.iter().all(|&p| p % 2 == 1),
            "MV updates always store an odd probability"
        );
    }

    // ---------------------------------------------------------------------
    // The MV section's three passes
    // ---------------------------------------------------------------------

    /// `read_mv_probs` is three sequential passes, not one fused loop, and
    /// the third only exists when `allow_hp`. Encode an update in each pass
    /// and check both the values and the pass boundaries.
    #[test]
    fn mv_probs_are_three_sequential_passes() {
        for allow_hp in [false, true] {
            let cfg = HeaderCfg {
                allow_high_precision_mv: allow_hp,
                ..base_cfg()
            };
            let rm = ReferenceMode::Single;
            let defaults = FrameProbs::defaults();
            let slots = header_slots(&defaults, &cfg, rm);

            // Index of the first MV slot, and the structure after it.
            let first_mv = slots
                .iter()
                .position(|s| s.kind == SlotKind::Mv)
                .expect("the MV section exists on an inter frame");
            let mv_names: Vec<&str> = slots[first_mv..].iter().map(|s| s.name.as_str()).collect();
            assert_eq!(
                mv_names[0], "mv.joints[0]",
                "the MV section opens with the joints"
            );

            // The section's size, derived from libvpx's own constants rather
            // than restated as a number: joints, then per component
            // sign/classes/class0/bits, then per component class0_fp and fp,
            // then per component class0_hp and hp.
            let pass1 =
                (MV_JOINTS - 1) + 2 * (1 + (MV_CLASSES - 1) + (CLASS0_SIZE - 1) + MV_OFFSET_BITS);
            let pass2 = 2 * (CLASS0_SIZE * (MV_FP_SIZE - 1) + (MV_FP_SIZE - 1));
            let pass3 = if allow_hp { 2 * 2 } else { 0 };
            assert_eq!(
                mv_names.len(),
                pass1 + pass2 + pass3,
                "the MV section's size must follow vp9_entropymv.h's constants"
            );
            assert_eq!((pass1, pass2), (47, 18), "and those work out to these");
            // Pass 1 ends after component 1's last `bits` entry; pass 2 opens
            // by returning to component 0.
            let pass2_start = mv_names
                .iter()
                .position(|n| *n == "mv.comps[0].class0_fp[0][0]")
                .expect("pass 2 exists");
            assert_eq!(
                mv_names[pass2_start - 1],
                "mv.comps[1].bits[9]",
                "pass 2 must start immediately after pass 1's last symbol, \
                 which is component 1's — a fused loop would put a \
                 component-0 symbol here"
            );
            assert_eq!(
                mv_names.len() - pass2_start,
                if allow_hp { 18 + 4 } else { 18 },
                "pass 2 is 18 symbols, pass 3 adds 4 only with allow_hp"
            );
            if allow_hp {
                assert_eq!(mv_names[mv_names.len() - 4], "mv.comps[0].class0_hp");
                assert_eq!(mv_names[mv_names.len() - 1], "mv.comps[1].hp");
            } else {
                assert!(
                    !mv_names.iter().any(|n| n.contains("hp")),
                    "no high-precision symbol may be read"
                );
            }

            // Now actually update one slot in each pass and check the values.
            let mut updates = vec![Upd::None; slots.len()];
            let targets: Vec<usize> = mv_names
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    **n == "mv.comps[1].classes[3]"
                        || **n == "mv.comps[0].fp[1]"
                        || **n == "mv.comps[1].class0_hp"
                })
                .map(|(i, _)| first_mv + i)
                .collect();
            assert_eq!(targets.len(), if allow_hp { 3 } else { 2 });
            for (k, &t) in targets.iter().enumerate() {
                updates[t] = Upd::Mv(17 + k as u32);
            }
            let (probs, _, _, _) = round_trip(&cfg, TxMode::Only4x4, rm, &updates);

            assert_eq!(probs.mv.comps[1].classes[3], (17 << 1) | 1);
            assert_eq!(probs.mv.comps[0].fp[1], (18 << 1) | 1);
            if allow_hp {
                assert_eq!(probs.mv.comps[1].class0_hp, (19 << 1) | 1);
            } else {
                assert_eq!(
                    probs.mv.comps[1].class0_hp, defaults.mv.comps[1].class0_hp,
                    "class0_hp must not move when allow_hp is off"
                );
                assert_eq!(probs.mv.comps[1].hp, defaults.mv.comps[1].hp);
            }
        }
    }

    /// An MV update is a raw literal, *not* a subexp delta. Encoding a
    /// subexp payload where a literal belongs must desynchronise the parse —
    /// this is the negative control that proves the sentinel has teeth.
    #[test]
    fn sentinel_catches_a_wrong_update_encoding() {
        let cfg = base_cfg();
        let rm = ReferenceMode::Single;
        let items = header_items(&FrameProbs::defaults(), &cfg, rm);
        let slots = header_slots(&FrameProbs::defaults(), &cfg, rm);
        let first_mv = slots
            .iter()
            .position(|s| s.kind == SlotKind::Mv)
            .expect("MV section");

        let mut w = BoolWriter::new();
        // Prologue and every slot before the MV section, encoded correctly.
        let mut updates = vec![Upd::None; slots.len()];
        updates[first_mv] = Upd::Mv(0);
        encode_header(&mut w, &cfg, TxMode::Only4x4, rm, &items, &updates);
        w.write_literal(SENTINEL, 16);
        let good = w.finish();

        let mut w = BoolWriter::new();
        w.write_literal(0, 2); // tx_mode ONLY_4X4
        w.write_bit(false); // no coef updates
        for _ in 0..first_mv {
            w.write_bool(DIFF_UPDATE_PROB, false);
        }
        // The trap: a subexp payload where update_mv_probs wants 7 raw bits.
        w.write_bool(MV_UPDATE_PROB, true);
        encode_term_subexp(&mut w, 100);
        for _ in (first_mv + 1)..slots.len() {
            w.write_bool(MV_UPDATE_PROB, false);
        }
        w.write_literal(SENTINEL, 16);
        let bad = w.finish();

        assert_ne!(good, bad, "the two encodings must differ");

        let mut probs = FrameProbs::defaults();
        let mut r = BoolReader::new(&good).expect("marker");
        parse_compressed_header_into(&mut r, &cfg, &mut probs);
        assert_eq!(r.read_literal(16), SENTINEL, "the correct encoding");

        let mut probs = FrameProbs::defaults();
        let mut r = BoolReader::new(&bad).expect("marker");
        parse_compressed_header_into(&mut r, &cfg, &mut probs);
        assert_ne!(
            r.read_literal(16),
            SENTINEL,
            "a subexp payload in an MV slot must desynchronise the parse"
        );
    }

    // ---------------------------------------------------------------------
    // Malformed input
    // ---------------------------------------------------------------------

    /// A header truncated anywhere inside its update stream must report the
    /// overrun rather than return the probabilities it managed to read.
    #[test]
    fn truncated_header_is_an_error() {
        let cfg = base_cfg();
        let rm = ReferenceMode::Single;
        let items = header_items(&FrameProbs::defaults(), &cfg, rm);
        let slots = header_slots(&FrameProbs::defaults(), &cfg, rm);
        // Every slot updated, so the header is long and truncation always
        // lands inside it.
        let updates = vec![Upd::Subexp(200); slots.len()]
            .into_iter()
            .zip(slots.iter())
            .map(|(u, s)| match s.kind {
                SlotKind::Subexp => u,
                SlotKind::Mv => Upd::Mv(99),
            })
            .collect::<Vec<_>>();
        let mut w = BoolWriter::new();
        encode_header(&mut w, &cfg, TxMode::Only4x4, rm, &items, &updates);
        let full = w.finish();
        assert!(
            full.len() > 40,
            "expected a long header, got {}",
            full.len()
        );

        assert!(
            parse_compressed_header(&full, &cfg, &mut FrameProbs::defaults()).is_ok(),
            "the untruncated header must parse"
        );
        for keep in [1usize, 2, 5, 10, full.len() / 2, full.len() - 8] {
            let err = parse_compressed_header(&full[..keep], &cfg, &mut FrameProbs::defaults());
            assert!(
                err.is_err(),
                "truncating to {keep} of {} bytes must be an error",
                full.len()
            );
        }
    }

    /// An empty partition cannot even carry the marker bit.
    #[test]
    fn empty_partition_is_an_error() {
        let mut probs = FrameProbs::defaults();
        assert!(parse_compressed_header(&[], &base_cfg(), &mut probs).is_err());
    }

    // ---------------------------------------------------------------------
    // Real libvpx-produced inter headers
    // ---------------------------------------------------------------------

    /// Real inter frames from the `testdata/` fixtures (see
    /// `super::super::inter_fixture_tests` for their provenance), parsed
    /// with the real production path.
    ///
    /// This is the one check here that does not go through this module's own
    /// encoder, so it is the one that can catch a misconception shared
    /// between [`header_items`] and [`parse_compressed_header_into`]. It is
    /// legitimate to start from `FrameProbs::defaults()` rather than from
    /// the probabilities each frame really inherited: `DIFF_UPDATE_PROB` and
    /// `MV_UPDATE_PROB` are constants and every payload length is
    /// prior-independent, so the *symbol sequence* a header codes — the only
    /// thing asserted here — does not depend on the priors at all. The
    /// resulting probability *values* would differ, and are not checked.
    ///
    /// A parse that reads more symbols than the partition holds trips
    /// `vpx_reader_has_error`; one that reads too few leaves an
    /// implausibly large tail, which [`tail_bits`] measures.
    #[test]
    fn real_inter_fixture_headers_parse_without_overrun() {
        const P9BASIC: [&[u8]; 8] = [
            include_bytes!("testdata/p9basic.frame0.bin"),
            include_bytes!("testdata/p9basic.frame1.bin"),
            include_bytes!("testdata/p9basic.frame2.bin"),
            include_bytes!("testdata/p9basic.frame3.bin"),
            include_bytes!("testdata/p9basic.frame4.bin"),
            include_bytes!("testdata/p9basic.frame5.bin"),
            include_bytes!("testdata/p9basic.frame6.bin"),
            include_bytes!("testdata/p9basic.frame7.bin"),
        ];
        const P9HP: [&[u8]; 8] = [
            include_bytes!("testdata/p9hp.frame0.bin"),
            include_bytes!("testdata/p9hp.frame1.bin"),
            include_bytes!("testdata/p9hp.frame2.bin"),
            include_bytes!("testdata/p9hp.frame3.bin"),
            include_bytes!("testdata/p9hp.frame4.bin"),
            include_bytes!("testdata/p9hp.frame5.bin"),
            include_bytes!("testdata/p9hp.frame6.bin"),
            include_bytes!("testdata/p9hp.frame7.bin"),
        ];
        const SWITCH: [&[u8]; 10] = [
            include_bytes!("testdata/switch.frame0.bin"),
            include_bytes!("testdata/switch.frame1.bin"),
            include_bytes!("testdata/switch.frame2.bin"),
            include_bytes!("testdata/switch.frame3.bin"),
            include_bytes!("testdata/switch.frame4.bin"),
            include_bytes!("testdata/switch.frame5.bin"),
            include_bytes!("testdata/switch.frame6.bin"),
            include_bytes!("testdata/switch.frame7.bin"),
            include_bytes!("testdata/switch.frame8.bin"),
            include_bytes!("testdata/switch.frame9.bin"),
        ];
        const COMPOUND: [&[u8]; 13] = [
            include_bytes!("testdata/compound.frame0.bin"),
            include_bytes!("testdata/compound.frame1.bin"),
            include_bytes!("testdata/compound.frame2.bin"),
            include_bytes!("testdata/compound.frame3.bin"),
            include_bytes!("testdata/compound.frame4.bin"),
            include_bytes!("testdata/compound.frame5.bin"),
            include_bytes!("testdata/compound.frame6.bin"),
            include_bytes!("testdata/compound.frame7.bin"),
            include_bytes!("testdata/compound.frame8.bin"),
            include_bytes!("testdata/compound.frame9.bin"),
            include_bytes!("testdata/compound.frame10.bin"),
            include_bytes!("testdata/compound.frame11.bin"),
            include_bytes!("testdata/compound.frame12.bin"),
        ];

        let mut stats = FixtureStats::default();
        for (name, frames) in [
            ("p9basic", &P9BASIC[..]),
            ("p9hp", &P9HP[..]),
            ("switch", &SWITCH[..]),
            ("compound", &COMPOUND[..]),
        ] {
            walk_fixture(name, frames, &mut stats);
        }

        // Exact, measured coverage of the conditional structure. Anything
        // that drops to zero means a branch stopped being exercised by real
        // data and the synthetic tests above became the only evidence for it.
        assert_eq!(stats.intra_frames, 4, "one key frame per fixture");
        assert_eq!(
            stats.inter_frames, 35,
            "8 + 8 + 10 + 13 frames, minus the four key frames"
        );
        assert_eq!(
            stats.switchable, 1,
            "exactly one fixture frame (switch.frame1) codes SWITCHABLE \
             filters, so exactly one reads switchable_interp_prob"
        );
        assert_eq!(
            stats.allow_hp, 35,
            "every fixture inter frame codes high-precision motion vectors, \
             so read_mv_probs' third pass runs on all of them (the allow_hp \
             == false shape is covered synthetically instead)"
        );
        assert_eq!(
            stats.compound_allowed, 10,
            "ten fixture frames have sign biases that allow compound \
             prediction, i.e. read a reference mode at all"
        );
        assert_eq!(
            stats.selects, 10,
            "all ten of them code REFERENCE_MODE_SELECT, which reads all \
             three of comp_inter, single_ref and comp_ref"
        );
        assert_eq!(
            stats.compounds, 0,
            "no fixture codes COMPOUND_REFERENCE; that shape is covered \
             synthetically only"
        );
        assert_eq!(
            stats.non_single_reference_mode,
            stats.selects + stats.compounds
        );
        // Reported rather than pinned: the per-frame `tail <= 16` assertion
        // in `walk_fixture` is the real check. The worst of these 39 frames
        // measured 9 bits; an equality here would break on any future
        // fixture addition without anything having regressed.
        assert!(
            stats.max_tail <= 16,
            "worst leftover across all fixtures was {} bits",
            stats.max_tail
        );
    }

    #[derive(Default)]
    struct FixtureStats {
        intra_frames: usize,
        inter_frames: usize,
        switchable: usize,
        allow_hp: usize,
        compound_allowed: usize,
        non_single_reference_mode: usize,
        selects: usize,
        compounds: usize,
        /// Largest number of bits left unread after a parse, over all frames.
        max_tail: usize,
    }

    /// Reads bits past the end of a parse until the reader reports the
    /// overrun, i.e. roughly "unread bits left in the partition, plus the
    /// reader's 64-bit lookahead".
    fn tail_bits(r: &mut BoolReader<'_>) -> usize {
        let mut n = 0usize;
        while !r.has_error() && n < 100_000 {
            let _ = r.read_bit();
            n += 1;
        }
        n
    }

    fn walk_fixture(name: &str, frames: &[&[u8]], stats: &mut FixtureStats) {
        let mut ref_sizes: [Option<(u32, u32)>; 8] = [None; 8];
        for (idx, &payload) in frames.iter().enumerate() {
            let hdr = UncompressedHeader::parse_with_ref_sizes(payload, &ref_sizes)
                .unwrap_or_else(|e| panic!("{name}.frame{idx}: header parse: {e}"));
            for slot in 0..8 {
                if hdr.refresh_frame_flags & (1 << slot) != 0 {
                    ref_sizes[slot] = Some((hdr.width, hdr.height));
                }
            }
            if hdr.show_existing_frame {
                continue;
            }

            let start = hdr.uncompressed_header_bytes;
            let end = start + usize::from(hdr.compressed_header_size);
            assert!(
                end <= payload.len(),
                "{name}.frame{idx}: compressed header past the payload"
            );
            let partition = &payload[start..end];
            let cfg = HeaderCfg::from_header(&hdr);

            let mut probs = FrameProbs::defaults();
            let header = parse_compressed_header(partition, &cfg, &mut probs).unwrap_or_else(|e| {
                panic!("{name}.frame{idx}: real compressed header did not parse: {e}")
            });

            // How much of the partition was left over. `vpx_stop_encode`'s
            // 32 flush bits are consumed by the arithmetic decode itself, so
            // a correct parse ends within a few bits of the partition's end;
            // measured, the worst of these 39 frames leaves 9 bits.
            //
            // What this does and does not prove: it is a strong check
            // against a parse that skips whole sections (each carries real
            // subexp payloads), and no check at all against one that miscounts
            // a handful of "no update" flags, which cost ~0.02 bits each at
            // probability 252. `has_error()` above covers the other
            // direction — reading past the end.
            let mut r = BoolReader::new(partition).expect("marker bit");
            let mut scratch = FrameProbs::defaults();
            parse_compressed_header_into(&mut r, &cfg, &mut scratch);
            let tail = tail_bits(&mut r);
            stats.max_tail = stats.max_tail.max(tail);
            assert!(
                tail <= 16,
                "{name}.frame{idx}: {tail} bits left after the parse in a \
                 {}-byte partition — the parse read far too few symbols",
                partition.len()
            );

            if cfg.frame_is_intra_only {
                stats.intra_frames += 1;
                assert_eq!(
                    header.reference_mode,
                    ReferenceMode::Single,
                    "{name}.frame{idx}: an intra frame codes no reference mode"
                );
            } else {
                stats.inter_frames += 1;
                stats.switchable += usize::from(cfg.interp_filter == SWITCHABLE);
                stats.allow_hp += usize::from(cfg.allow_high_precision_mv);
                let allowed = is_compound_reference_allowed(&cfg.ref_frame_sign_bias);
                stats.compound_allowed += usize::from(allowed);
                stats.non_single_reference_mode +=
                    usize::from(header.reference_mode != ReferenceMode::Single);
                stats.selects += usize::from(header.reference_mode == ReferenceMode::Select);
                stats.compounds += usize::from(header.reference_mode == ReferenceMode::Compound);
                assert!(
                    allowed || header.reference_mode == ReferenceMode::Single,
                    "{name}.frame{idx}: compound mode without compound sign biases"
                );
            }
        }
    }
}
