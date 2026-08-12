//! Persistent cross-frame VP8 decoder state (RFC 6386 §9.3, §9.4, §9.7,
//! §9.9-§9.10, §16.1, §17.2; dixie.c reference decoder `struct
//! vp8_decoder_ctx`, rfc6386.txt lines 8710-8740).
//!
//! A VP8 key frame is fully self-describing: every probability table and
//! per-macroblock feature it needs is either a fixed RFC default or is
//! transmitted in full in its own header. An **inter** frame is not —
//! several pieces of decoder state are *adaptive*, meaning a frame may
//! choose to leave them exactly as the previous frame left them rather than
//! retransmitting them. [`Vp8State`] is the home for that carried-over
//! state, mirroring the persistent fields of dixie.c's `vp8_decoder_ctx`
//! (`entropy_hdr`, `segment_hdr`, `loopfilter_hdr`, `reference_hdr.sign_bias`
//! at rfc6386.txt lines 8710-8740, verified against the reset/update logic
//! in `decode_frame` at lines 8082-8209 and the individual `decode_*_header`
//! functions at lines 7722-7983).
//!
//! # What persists, and why
//!
//! - **Entropy probabilities** ([`EntropyContext`]): DCT-coefficient, luma
//!   mode, chroma mode and motion-vector probabilities. RFC 6386 §9.9: "These
//!   tables are maintained across interframes but are of course replaced
//!   with their defaults at the beginning of every key frame" (rfc6386.txt
//!   line 2178). Confirmed unconditional (not gated on `refresh_entropy`) at
//!   dixie.c `decode_frame` lines 8144-8157.
//! - **`refresh_entropy_probs == 0` snapshot** ([`Vp8State::saved_entropy`]):
//!   when a frame's header signals that its entropy updates should not
//!   outlive the frame, the decoder must snapshot the entropy tables
//!   *before* applying that frame's updates and restore the snapshot once
//!   the frame is fully decoded (dixie.c lines 8159-8163 snapshot, 8205-8209
//!   restore).
//! - **Loop-filter deltas** ([`Vp8State::lf_deltas`]): the eight
//!   per-reference-frame / per-mode loop-filter level deltas (RFC 6386 §9.4).
//!   `loop_filter_adj_enable` is read fresh every frame, but the deltas
//!   themselves are only touched when `mode_ref_lf_delta_update` is set, and
//!   even then only the individually-flagged indices change — the rest keep
//!   whatever they were left at (dixie.c lines 7903-7921). The pre-existing
//!   key-frame-only header parser in this crate does not need to model this
//!   because a key frame's "previous" value is always the frame-start zero
//!   reset; an inter frame's is not.
//! - **Segmentation** ([`Vp8State::segment`], [`Vp8State::segment_map`]):
//!   same shape of persistence as the loop-filter deltas — `enabled` is
//!   fresh every frame, `abs_delta`/`quant_deltas`/`lf_deltas` are only
//!   touched by an `update_segment_feature_data` pass, `tree_probs` only by
//!   an `update_mb_segmentation_map` pass, and the per-macroblock segment-id
//!   map itself is only rewritten when the map update pass runs — otherwise
//!   decode reuses the map from the last frame that wrote it (dixie.c lines
//!   7925-7983, and the per-macroblock consumption at lines 10613-10625).
//! - **Sign bias** ([`Vp8State::sign_bias`]): controls motion-vector sign
//!   when referencing the golden or altref frame (RFC 6386 §9.7). Read fresh
//!   every inter frame (`0` on key frames per dixie.c line 7804-7805); kept
//!   here because it is frame-header state a macroblock decode pass needs
//!   available alongside everything else in this module.
//!
//! # Where this state is driven from
//!
//! [`super::inter::Vp8SequenceDecoder`] owns one of these per sequence and
//! is the only thing that advances it. Per frame, in dixie.c's order
//! (rfc6386.txt lines 8110-8270):
//!
//! - a **key frame** calls [`Vp8State::reset_for_keyframe`] on entry (lines
//!   8144-8157: the entropy defaults are restored "regardless of the
//!   refresh_entropy setting"), and mirrors its parsed loop-filter deltas,
//!   segmentation feature data and per-macroblock segment ids back into this
//!   struct afterwards, because the key-frame header parser predates it and
//!   builds a per-frame [`super::header::Vp8Header`] instead of writing
//!   here directly;
//! - an **inter frame** inherits everything, with
//!   `Vp8Header::parse_interframe` writing the fields its header updates;
//! - **both** take the [`Vp8State::saved_entropy`] snapshot where
//!   `refresh_entropy_probs` is read (lines 8159-8163) and call
//!   [`Vp8State::end_of_frame_entropy_restore`] once the frame is decoded
//!   (lines 8205-8209) — the bit is coded for both frame types (RFC 6386
//!   §19.2, rfc6386.txt lines 6822 and 6848), and the `refswap_er` /
//!   `segdelta` conformance fixtures both open with a key frame that sets it
//!   to 0.
//!
//! # Reference-frame buffers live next door
//!
//! The decoded-picture buffer (last / golden / altref reconstructed planes)
//! is a different kind of state — owned pixel buffers rather than
//! header-derived probabilities and flags — and lives in [`super::refs`];
//! [`super::inter::Vp8SequenceDecoder`] holds one of each.

#![forbid(unsafe_code)]

use super::header::{MAX_MODE_LF_DELTAS, MAX_REF_LF_DELTAS, MAX_SEGMENTS};
use super::tables;
use super::tables_inter;

/// Adaptive entropy-coding probabilities carried across inter frames (RFC
/// 6386 §9.9-§9.10, §16.1, §17.2; dixie.c `struct vp8_entropy_hdr`,
/// rfc6386.txt lines 8575-8586 — this type keeps the four fields that are
/// genuinely persistent across frames and leaves out `coeff_skip_enabled` /
/// `coeff_skip_prob` / `prob_inter` / `prob_last` / `prob_gf`, which dixie.c
/// stores in the same struct but which RFC 6386 §9.10 shows are read fresh,
/// unconditionally, every inter frame with no update-flag gating at all —
/// there is nothing to persist for them).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct EntropyContext {
    /// DCT-token probability table (RFC 6386 §13.4). Same type as
    /// [`super::header::Vp8Header::coeff_probs`].
    pub coeff_probs: [[[[u8; 11]; 3]; 8]; 4],
    /// Probabilities for the inter-frame luma mode tree
    /// ([`tables_inter::YMODE_TREE`]).
    pub ymode_prob: [u8; 4],
    /// Probabilities for the inter-frame chroma mode tree
    /// ([`tables_inter::UV_MODE_TREE`]).
    pub uv_mode_prob: [u8; 3],
    /// Motion-vector component probabilities, `[component][MVP* offset]`
    /// (RFC 6386 §17.2; component 0 = row, 1 = column).
    pub mv_probs: [[u8; 19]; 2],
}

impl EntropyContext {
    /// The RFC 6386 default entropy tables, restored at the start of every
    /// key frame (RFC 6386 §9.9, dixie.c lines 8144-8157).
    #[must_use]
    pub fn defaults() -> Self {
        Self {
            coeff_probs: tables::DEFAULT_COEFF_PROBS,
            ymode_prob: tables_inter::DEFAULT_YMODE_PROB,
            uv_mode_prob: tables_inter::DEFAULT_UV_MODE_PROB,
            mv_probs: tables_inter::DEFAULT_MV_CONTEXT,
        }
    }
}

impl Default for EntropyContext {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Persistent per-reference-frame / per-mode loop-filter level deltas (RFC
/// 6386 §9.4 `mb_lf_adjustments()`).
///
/// `enabled` is overwritten every frame (both key and inter) with that
/// frame's freshly-read `loop_filter_adj_enable` bit — it is bundled into
/// this struct for locality (it gates whether `ref_deltas`/`mode_deltas` are
/// even consulted at macroblock decode time), not because it has its own
/// inheritance behaviour. `ref_deltas`/`mode_deltas` are the fields with the
/// actual persistence semantics: they are left untouched unless the header's
/// `mode_ref_lf_delta_update` flag is set, and even then only the
/// individually-flagged indices are overwritten (dixie.c lines 7903-7921).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LoopFilterDeltas {
    /// Per-reference-frame (intra / last / golden / altref) level deltas.
    pub ref_deltas: [i8; MAX_REF_LF_DELTAS],
    /// Per-prediction-mode level deltas.
    pub mode_deltas: [i8; MAX_MODE_LF_DELTAS],
    /// Whether macroblock-level loop-filter adjustment is active this frame.
    pub enabled: bool,
}

/// Persistent segmentation state (RFC 6386 §9.3, §10).
///
/// Same persistence shape as [`LoopFilterDeltas`]: `enabled` is fresh every
/// frame; `abs_delta`/`quant_deltas`/`lf_deltas` only change on an
/// `update_segment_feature_data` pass (and are then unconditionally
/// rewritten in full — RFC 6386 §9.3 item 5's "one-bit flag indicates
/// whether the item is 0, or a non-zero value", dixie.c `bool_maybe_get_int`
/// at line 7593-7595 always assigns, defaulting to `0` rather than leaving
/// the previous value); `tree_probs` only changes on an
/// `update_mb_segmentation_map` pass (defaulting each unflagged entry to
/// `255`, matching dixie.c lines 7972-7975). The per-macroblock segment-id
/// assignments live separately in [`Vp8State::segment_map`], since they are
/// per-macroblock rather than per-frame data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SegmentState {
    /// Whether segment-based adjustments are active this frame.
    pub enabled: bool,
    /// `true` = absolute-value feature data, `false` = delta from baseline.
    pub abs_delta: bool,
    /// Per-segment quantiser index deltas (or absolute values, per `abs_delta`).
    pub quant_deltas: [i8; MAX_SEGMENTS],
    /// Per-segment loop-filter level deltas (or absolute values).
    pub lf_deltas: [i8; MAX_SEGMENTS],
    /// Probabilities for the per-macroblock segment-id tree.
    pub tree_probs: [u8; 3],
}

impl Default for SegmentState {
    /// `tree_probs` defaults to `255` (not `0`): 255 is VP8's own "not yet
    /// set" convention for these probabilities (matching the keyframe-only
    /// [`super::header`]'s `parse_segmentation` and dixie.c's
    /// `bool_maybe_get_int`-driven default at rfc6386.txt lines 7972-7975),
    /// not merely this struct's zero-initialised rest. Functionally inert
    /// either way — `tree_probs` is only ever consulted when
    /// `update_map == true`, at which point it was just freshly reassigned
    /// in full this same frame — but 255 keeps a stray read (e.g. from a
    /// future bug) honest about "no real probability was ever set" rather
    /// than silently reading as the maximally-skewed-toward-zero `0`.
    fn default() -> Self {
        Self {
            enabled: false,
            abs_delta: false,
            quant_deltas: [0; MAX_SEGMENTS],
            lf_deltas: [0; MAX_SEGMENTS],
            tree_probs: [255, 255, 255],
        }
    }
}

/// Reference-frame sign-bias flags (RFC 6386 §9.7): controls the sign of
/// motion vectors when the golden or altref frame is the prediction
/// reference. `false` (the RFC-mandated value) on key frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SignBias {
    /// Sign-bias flag when the golden frame is the reference.
    pub golden: bool,
    /// Sign-bias flag when the altref frame is the reference.
    pub altref: bool,
}

/// All persistent cross-frame VP8 decoder state that lives outside the
/// per-frame [`super::header::Vp8Header`] — the pieces an inter frame may
/// choose to inherit rather than retransmit. See the module doc for the
/// provenance of each field and the current (P2) scope boundary.
pub(crate) struct Vp8State {
    /// Adaptive entropy-coding probabilities (RFC 6386 §9.9-§9.10, §17.2).
    pub entropy: EntropyContext,
    /// Snapshot of `entropy` taken just before a frame with
    /// `refresh_entropy_probs == 0` applies its own probability updates;
    /// `Some` between that snapshot and the matching
    /// [`Vp8State::end_of_frame_entropy_restore`] call, `None` otherwise.
    pub saved_entropy: Option<EntropyContext>,
    /// Persistent loop-filter level deltas (RFC 6386 §9.4).
    pub lf_deltas: LoopFilterDeltas,
    /// Persistent segmentation feature state (RFC 6386 §9.3).
    pub segment: SegmentState,
    /// Per-macroblock segment-id assignments (RFC 6386 §10), row-major,
    /// `mb_cols * mb_rows` entries. Empty until the first frame that sizes
    /// it via [`Vp8State::ensure_segment_map_size`]. Rewritten by
    /// macroblock-mode decode (out of scope for this package) only on
    /// frames with `segment.enabled && update_mb_segmentation_map`;
    /// otherwise the previous frame's map is the correct one to keep
    /// decoding against.
    pub segment_map: Vec<u8>,
    /// Reference-frame sign-bias flags (RFC 6386 §9.7).
    pub sign_bias: SignBias,
}

impl Vp8State {
    /// Creates decoder state initialised exactly as it would be immediately
    /// after a key frame's reset (RFC defaults, zeroed deltas/segmentation,
    /// no pending entropy snapshot, empty segment map).
    #[must_use]
    pub fn new() -> Self {
        Self {
            entropy: EntropyContext::defaults(),
            saved_entropy: None,
            lf_deltas: LoopFilterDeltas::default(),
            segment: SegmentState::default(),
            segment_map: Vec::new(),
            sign_bias: SignBias::default(),
        }
    }

    /// Resets all persistent state to key-frame defaults.
    ///
    /// RFC 6386 mandates this for the entropy tables explicitly (§9.9) and
    /// dixie.c applies the same `memset(hdr, 0, sizeof(*hdr))` reset to the
    /// segmentation and loop-filter-delta headers on every key frame
    /// (rfc6386.txt lines 7903-7905, 7930-7931) — a key frame is a fresh
    /// starting point with no "previous frame" to inherit from. The
    /// per-macroblock segment map is cleared in place (not reallocated) so
    /// an already-sized buffer keeps its allocation across the reset; a
    /// keyframe with segmentation enabled and `update_mb_segmentation_map`
    /// will overwrite every entry during macroblock decode regardless; one
    /// with segmentation enabled but not updating the map has no valid
    /// previous map to fall back to (there is no previous frame within this
    /// GOU), so the conservative, spec-consistent choice is segment 0
    /// everywhere, matching this crate's existing (keyframe-only)
    /// `read_segment_id`'s `!update_map` default.
    pub fn reset_for_keyframe(&mut self) {
        self.entropy = EntropyContext::defaults();
        self.saved_entropy = None;
        self.lf_deltas = LoopFilterDeltas::default();
        self.segment = SegmentState::default();
        self.segment_map.fill(0);
        self.sign_bias = SignBias::default();
    }

    /// Ensures [`Vp8State::segment_map`] holds exactly `mb_count` entries,
    /// resizing (and zero-filling) it if the macroblock count has changed.
    ///
    /// A no-op when `mb_count` already matches the current length — the
    /// existing per-macroblock segment ids are preserved, which is the
    /// whole point of the map persisting across inter frames. A genuine
    /// size change (first frame, or a dimension change) has no valid
    /// previous per-macroblock data to preserve, so it is discarded and the
    /// map starts from segment 0 everywhere.
    pub fn ensure_segment_map_size(&mut self, mb_count: usize) {
        if self.segment_map.len() != mb_count {
            self.segment_map = vec![0u8; mb_count];
        }
    }

    /// Applies the `refresh_entropy_probs == 0` snapshot/restore mechanism's
    /// "restore" half (RFC 6386 §9.7/§9.9-§9.10 field `refresh_entropy_probs`;
    /// dixie.c `decode_frame` lines 8205-8209:
    /// `if (!ctx->reference_hdr.refresh_entropy) { ctx->entropy_hdr =
    /// ctx->saved_entropy; ctx->saved_entropy_valid = 0; }`).
    ///
    /// A no-op when no snapshot is pending (`saved_entropy` is `None` —
    /// either `refresh_entropy_probs` was 1 for the frame just decoded, or
    /// this function is called without a matching snapshot having been
    /// taken).
    ///
    /// # Both frame types call this
    ///
    /// dixie.c's snapshot (lines 8159-8163) and this restore are **not**
    /// gated on frame type — only on the `refresh_entropy_probs` bit, which
    /// is coded for key frames too (RFC 6386 §19.2: `if (key_frame)
    /// refresh_entropy_probs L(1)`, rfc6386.txt line 6822). A key frame with
    /// `refresh_entropy_probs == 0` is real bitstream, not a corner case:
    /// the `refswap_er` and `segdelta` conformance fixtures both open with
    /// one. [`super::inter::Vp8SequenceDecoder`] therefore calls this at the
    /// end of *every* frame it decodes, key or inter — which also closed the
    /// key-frame-path gap that existed while the key-frame header parser
    /// simply discarded that bit.
    pub fn end_of_frame_entropy_restore(&mut self) {
        if let Some(saved) = self.saved_entropy.take() {
            self.entropy = saved;
        }
    }
}

impl Default for Vp8State {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A non-default sentinel entropy context, distinguishable from
    /// [`EntropyContext::defaults`] in every field, used across several
    /// tests below to prove persistence/reset/restore actually move data
    /// rather than coincidentally matching defaults.
    fn sentinel_entropy() -> EntropyContext {
        let mut e = EntropyContext::defaults();
        e.coeff_probs[0][0][0][0] = 7;
        e.coeff_probs[3][7][2][10] = 250;
        e.ymode_prob = [1, 2, 3, 4];
        e.uv_mode_prob = [5, 6, 7];
        e.mv_probs[0][0] = 9;
        e.mv_probs[1][18] = 200;
        e
    }

    // -- entropy: persistence table row 1 --------------------------------

    #[test]
    fn test_entropy_defaults_match_source_tables() {
        let e = EntropyContext::defaults();
        assert_eq!(e.coeff_probs, tables::DEFAULT_COEFF_PROBS);
        assert_eq!(e.ymode_prob, tables_inter::DEFAULT_YMODE_PROB);
        assert_eq!(e.uv_mode_prob, tables_inter::DEFAULT_UV_MODE_PROB);
        assert_eq!(e.mv_probs, tables_inter::DEFAULT_MV_CONTEXT);
    }

    #[test]
    fn test_entropy_set_on_frame_n_observed_on_frame_n_plus_1() {
        let mut state = Vp8State::new();
        // "frame N": some update mutates the persistent entropy tables.
        state.entropy = sentinel_entropy();
        // "frame N+1": nothing resets it in between (no keyframe), so a
        // later frame's parse would see exactly what frame N left behind.
        assert_eq!(state.entropy, sentinel_entropy());
    }

    #[test]
    fn test_entropy_reset_on_keyframe() {
        let mut state = Vp8State::new();
        state.entropy = sentinel_entropy();
        state.reset_for_keyframe();
        assert_eq!(state.entropy, EntropyContext::defaults());
    }

    // -- saved_entropy: persistence table row 2 --------------------------

    #[test]
    fn test_saved_entropy_snapshot_survives_further_mutation_then_restores() {
        let mut state = Vp8State::new();
        let pre_update = sentinel_entropy();
        state.entropy = pre_update.clone();

        // The header parser's snapshot step: capture the pre-update state
        // *before* this frame's own probability updates are applied.
        state.saved_entropy = Some(pre_update.clone());

        // Simulate this frame's own updates mutating `entropy` further.
        state.entropy.coeff_probs[1][2][1][5] = 42;
        state.entropy.mv_probs[0][3] = 111;
        assert_ne!(
            state.entropy, pre_update,
            "sanity: mutation must be visible"
        );

        // decode-complete: refresh_entropy_probs was 0, so restore.
        state.end_of_frame_entropy_restore();
        assert_eq!(
            state.entropy, pre_update,
            "restore must produce a byte-identical copy of the pre-update snapshot"
        );
        assert!(
            state.saved_entropy.is_none(),
            "restore must consume the snapshot"
        );
    }

    #[test]
    fn test_end_of_frame_entropy_restore_is_noop_without_snapshot() {
        let mut state = Vp8State::new();
        state.entropy = sentinel_entropy();
        let before = state.entropy.clone();
        assert!(state.saved_entropy.is_none());

        state.end_of_frame_entropy_restore();

        assert_eq!(
            state.entropy, before,
            "no pending snapshot must leave entropy untouched"
        );
        assert!(state.saved_entropy.is_none());
    }

    #[test]
    fn test_reset_for_keyframe_clears_pending_snapshot() {
        let mut state = Vp8State::new();
        state.saved_entropy = Some(sentinel_entropy());
        state.reset_for_keyframe();
        assert!(
            state.saved_entropy.is_none(),
            "a keyframe reset must not leave a stale snapshot for a later restore to apply"
        );
    }

    // -- lf_deltas: persistence table row 3 ------------------------------

    #[test]
    fn test_lf_deltas_set_on_frame_n_observed_on_frame_n_plus_1() {
        let mut state = Vp8State::new();
        state.lf_deltas = LoopFilterDeltas {
            ref_deltas: [2, 0, -2, -2],
            mode_deltas: [4, -2, 2, 4],
            enabled: true,
        };
        // No intervening keyframe reset: a later non-updating inter frame
        // must see exactly this.
        assert_eq!(
            state.lf_deltas,
            LoopFilterDeltas {
                ref_deltas: [2, 0, -2, -2],
                mode_deltas: [4, -2, 2, 4],
                enabled: true,
            }
        );
    }

    #[test]
    fn test_lf_deltas_reset_on_keyframe() {
        let mut state = Vp8State::new();
        state.lf_deltas = LoopFilterDeltas {
            ref_deltas: [2, 0, -2, -2],
            mode_deltas: [4, -2, 2, 4],
            enabled: true,
        };
        state.reset_for_keyframe();
        assert_eq!(state.lf_deltas, LoopFilterDeltas::default());
        assert_eq!(state.lf_deltas.ref_deltas, [0; MAX_REF_LF_DELTAS]);
        assert_eq!(state.lf_deltas.mode_deltas, [0; MAX_MODE_LF_DELTAS]);
        assert!(!state.lf_deltas.enabled);
    }

    // -- segment: persistence table row 4 --------------------------------

    #[test]
    fn test_segment_state_set_on_frame_n_observed_on_frame_n_plus_1() {
        let mut state = Vp8State::new();
        let seg = SegmentState {
            enabled: true,
            abs_delta: true,
            quant_deltas: [0, -6, 0, 0],
            lf_deltas: [0, 0, 0, 0],
            tree_probs: [255, 255, 255],
        };
        state.segment = seg;
        assert_eq!(state.segment, seg);
    }

    #[test]
    fn test_segment_state_reset_on_keyframe() {
        let mut state = Vp8State::new();
        state.segment = SegmentState {
            enabled: true,
            abs_delta: true,
            quant_deltas: [1, 2, 3, 4],
            lf_deltas: [5, 6, 7, 8],
            tree_probs: [10, 20, 30],
        };
        state.reset_for_keyframe();
        assert_eq!(state.segment, SegmentState::default());
        assert!(!state.segment.enabled);
        assert!(!state.segment.abs_delta);
        assert_eq!(state.segment.quant_deltas, [0; MAX_SEGMENTS]);
        assert_eq!(state.segment.lf_deltas, [0; MAX_SEGMENTS]);
        assert_eq!(state.segment.tree_probs, [255, 255, 255]);
    }

    #[test]
    fn test_segment_map_empty_until_sized() {
        let state = Vp8State::new();
        assert!(state.segment_map.is_empty());
    }

    #[test]
    fn test_segment_map_sized_on_first_use() {
        let mut state = Vp8State::new();
        state.ensure_segment_map_size(24);
        assert_eq!(state.segment_map, vec![0u8; 24]);
    }

    #[test]
    fn test_segment_map_same_size_call_preserves_contents() {
        let mut state = Vp8State::new();
        state.ensure_segment_map_size(6);
        state.segment_map[2] = 3;
        state.segment_map[5] = 1;

        // A later frame with the same macroblock count (the common case:
        // dimensions unchanged) must not wipe the inherited map.
        state.ensure_segment_map_size(6);

        assert_eq!(state.segment_map, vec![0, 0, 3, 0, 0, 1]);
    }

    #[test]
    fn test_segment_map_size_change_discards_and_resizes() {
        let mut state = Vp8State::new();
        state.ensure_segment_map_size(6);
        state.segment_map.fill(2);

        state.ensure_segment_map_size(9);

        assert_eq!(
            state.segment_map,
            vec![0u8; 9],
            "a genuine size change has no valid previous per-macroblock data to keep"
        );
    }

    #[test]
    fn test_segment_map_cleared_in_place_on_keyframe_reset() {
        let mut state = Vp8State::new();
        state.ensure_segment_map_size(6);
        state.segment_map.copy_from_slice(&[1, 2, 3, 1, 2, 3]);

        state.reset_for_keyframe();

        assert_eq!(
            state.segment_map,
            vec![0u8; 6],
            "keyframe reset clears contents"
        );
        assert_eq!(
            state.segment_map.len(),
            6,
            "keyframe reset must not deallocate an already-sized map"
        );
    }

    // -- sign_bias: persistence table row 5 ------------------------------

    #[test]
    fn test_sign_bias_set_on_frame_n_observed_on_frame_n_plus_1() {
        let mut state = Vp8State::new();
        state.sign_bias = SignBias {
            golden: true,
            altref: true,
        };
        assert_eq!(
            state.sign_bias,
            SignBias {
                golden: true,
                altref: true,
            }
        );
    }

    #[test]
    fn test_sign_bias_reset_on_keyframe() {
        let mut state = Vp8State::new();
        state.sign_bias = SignBias {
            golden: true,
            altref: true,
        };
        state.reset_for_keyframe();
        assert_eq!(state.sign_bias, SignBias::default());
        assert!(!state.sign_bias.golden);
        assert!(!state.sign_bias.altref);
    }

    // -- whole-state sanity -----------------------------------------------

    #[test]
    fn test_new_state_is_already_keyframe_reset_shape() {
        // A freshly-constructed state and a state that has just been
        // through `reset_for_keyframe` must agree on everything except
        // `segment_map`'s allocation history (both are logically "empty",
        // `new()` via zero-length, a post-reset one via zero-fill of
        // whatever length it already had).
        let fresh = Vp8State::new();
        let mut reset = Vp8State::new();
        reset.entropy = sentinel_entropy();
        reset.lf_deltas.enabled = true;
        reset.segment.enabled = true;
        reset.sign_bias.golden = true;
        reset.saved_entropy = Some(sentinel_entropy());
        reset.reset_for_keyframe();

        assert_eq!(fresh.entropy, reset.entropy);
        assert_eq!(fresh.lf_deltas, reset.lf_deltas);
        assert_eq!(fresh.segment, reset.segment);
        assert_eq!(fresh.sign_bias, reset.sign_bias);
        assert_eq!(fresh.saved_entropy, reset.saved_entropy);
    }

    #[test]
    fn test_default_impls_match_new_and_explicit_defaults() {
        assert_eq!(Vp8State::default().entropy, Vp8State::new().entropy);
        assert_eq!(
            LoopFilterDeltas::default(),
            LoopFilterDeltas {
                ref_deltas: [0; MAX_REF_LF_DELTAS],
                mode_deltas: [0; MAX_MODE_LF_DELTAS],
                enabled: false,
            }
        );
        assert_eq!(
            SegmentState::default(),
            SegmentState {
                enabled: false,
                abs_delta: false,
                quant_deltas: [0; MAX_SEGMENTS],
                lf_deltas: [0; MAX_SEGMENTS],
                tree_probs: [255, 255, 255],
            }
        );
        assert_eq!(
            SignBias::default(),
            SignBias {
                golden: false,
                altref: false
            }
        );
    }
}
