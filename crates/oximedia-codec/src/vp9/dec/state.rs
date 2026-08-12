//! Persistent cross-frame VP9 decoder state (VP9 bitstream spec §6.2, §7.2,
//! §8.4; libvpx `vp9/common/vp9_onyxc_int.h` `VP9_COMMON`,
//! `vp9/common/vp9_entropymode.c` `vp9_setup_past_independence`,
//! `vp9/decoder/vp9_decodeframe.c` `read_uncompressed_header` /
//! `vp9_decode_frame`).
//!
//! A VP9 key frame is self-contained: it resets every adaptive table to the
//! specification defaults and transmits everything else it needs. An inter
//! frame — and, for the entropy tables, an intra-only frame — is not: it may
//! name a probability context a *previous* frame adapted, inherit loop-filter
//! deltas and segmentation feature data it does not retransmit, and predict
//! from reference pixels and motion vectors decoded frames ago.
//! [`Vp9DecState`] is the home for all of that.
//!
//! # What persists, and why
//!
//! - **Four probability contexts** ([`Vp9DecState::frame_contexts`]) plus the
//!   working copy ([`Vp9DecState::fc`]). VP9 has `FRAME_CONTEXTS == 4`
//!   (`frame_context_idx` is `f(2)`, see
//!   [`crate::vp9::uncompressed::UncompressedHeader::frame_context_idx`]) —
//!   not to be confused with the eight *reference* slots. Each frame loads
//!   `frame_contexts[frame_context_idx]` into `fc` before decoding
//!   (`vp9_decodeframe.c:2996`) and, when `refresh_frame_context` is set,
//!   stores `fc` back into the same slot afterwards
//!   (`vp9_decodeframe.c:3068`).
//! - **Initialisation flags** ([`Vp9DecState::contexts_initialized`]) —
//!   libvpx's `fc->initialized`, checked immediately after the load
//!   (`vp9_decodeframe.c:2997-2999`: *"Uninitialized entropy context."*). A
//!   stream whose first frame names a context no reset ever wrote is corrupt,
//!   and must fail rather than decode against zeroed probabilities.
//! - **The decoded-picture buffer** ([`Vp9DecState::dpb`], eight slots) and
//!   [`Vp9DecState::prev_frame`], the last frame in *decode* order, which is
//!   where temporal motion-vector candidates come from.
//! - **Previous-frame geometry and flags** (`last_width`, `last_height`,
//!   `last_show_frame`, `last_intra_only`, `last_frame_type`) — the inputs to
//!   [`Vp9DecState::use_prev_frame_mvs`].
//! - **Loop-filter deltas** (`lf_ref_deltas`, `lf_mode_deltas`) and
//!   **segmentation feature data** (`seg_*`): both are transmitted only when
//!   their update flag is set and otherwise keep the value the last frame
//!   that wrote them left behind (libvpx `setup_loopfilter` /
//!   `setup_segmentation`), and both are reset by
//!   [`Vp9DecState::setup_past_independence`].
//!
//! # Frame order of operations
//!
//! [`Vp9DecState::begin_frame`] performs, in libvpx's order:
//!
//! 1. `vp9_setup_past_independence` when
//!    [`Vp9DecState::needs_past_independence`] — i.e. `frame_is_intra_only ||
//!    error_resilient_mode` (`vp9_decodeframe.c:2822`). Note this entry
//!    condition is *not* the same as the `KEY || error_resilient ||
//!    reset_frame_context == 3` test inside it, which only selects how much of
//!    `frame_contexts[]` is overwritten.
//! 2. the loop-filter delta and segmentation merges — correct in that
//!    position because `setup_loopfilter`/`setup_segmentation` are read
//!    *after* the reset in the bitstream (`vp9_decodeframe.c:2825-2827`), so
//!    a frame's own updates always win over the reset.
//! 3. the frame-context load. This is what makes `reset_frame_context` 0 or 1
//!    observable: the defaults written into `fc` by step 1 are immediately
//!    replaced by `frame_contexts[0]`, which is whatever a previous frame
//!    adapted.
//!
//! [`Vp9DecState::finish_frame`] closes the frame: reference-slot refresh,
//! the conditional context save, and the `last_*` advance.

#![forbid(unsafe_code)]

use super::adapt::{adapt_coef_probs, adapt_mode_probs, adapt_mv_probs};
use super::counts::FrameCounts;
use super::hdr::{FrameProbs, TxMode, SWITCHABLE};
use super::refs::{refresh_dpb, Vp9RefSlot, REF_FRAMES};
use crate::error::{CodecError, CodecResult};
use crate::vp9::uncompressed::{ColorSpace, UncompressedHeader, Vp9FrameType};

/// Number of probability contexts a stream may cycle between (libvpx
/// `FRAME_CONTEXTS`, `FRAME_CONTEXTS_LOG2 == 2`).
pub const FRAME_CONTEXTS: usize = 4;

/// `set_default_lf_deltas` reference deltas, indexed INTRA / LAST / GOLDEN /
/// ALTREF (libvpx `vp9_entropymode.c`).
pub const DEFAULT_LF_REF_DELTAS: [i8; 4] = [1, 0, -1, -1];

/// `set_default_lf_deltas` mode deltas (libvpx `vp9_entropymode.c`).
pub const DEFAULT_LF_MODE_DELTAS: [i8; 2] = [0, 0];

/// All VP9 decoder state that outlives a single frame.
pub struct Vp9DecState {
    /// The four saved probability contexts (libvpx `cm->frame_contexts`).
    pub frame_contexts: [FrameProbs; FRAME_CONTEXTS],
    /// Whether each context has ever been written (libvpx `fc->initialized`).
    pub contexts_initialized: [bool; FRAME_CONTEXTS],
    /// Working probability context for the frame being decoded (libvpx
    /// `cm->fc`).
    pub fc: FrameProbs,
    /// Symbol counts for backward adaptation (libvpx `cm->counts`).
    pub counts: FrameCounts,
    /// Transform mode of the frame currently being decoded (libvpx
    /// `cm->tx_mode`), as its compressed header decided it.
    ///
    /// It lives here rather than staying local to the reconstruction driver
    /// because backward adaptation needs it *after* the tiles are decoded:
    /// `vp9_adapt_mode_probs` moves the transform-size probabilities only
    /// under `TX_MODE_SELECT`. The decode entry points assign it before they
    /// return; [`Vp9DecState::adapt_probabilities`] is the only reader.
    pub tx_mode: TxMode,
    /// The eight reference slots (libvpx `cm->ref_frame_map`).
    pub dpb: [Option<Vp9RefSlot>; REF_FRAMES],
    /// The most recently decoded frame, in decode order (libvpx
    /// `cm->prev_frame`) — the source of temporal MV candidates. Unchanged by
    /// a `show_existing_frame` packet.
    pub prev_frame: Option<Vp9RefSlot>,
    /// Frame type of the previous frame (libvpx `cm->last_frame_type`).
    pub last_frame_type: Vp9FrameType,
    /// Width of the previous frame (libvpx `cm->last_width`).
    pub last_width: u32,
    /// Height of the previous frame (libvpx `cm->last_height`).
    pub last_height: u32,
    /// `show_frame` of the previous frame (libvpx `cm->last_show_frame`).
    pub last_show_frame: bool,
    /// `intra_only` of the previous frame (libvpx `cm->last_intra_only`).
    pub last_intra_only: bool,
    /// Sequence-level bit depth (libvpx `cm->bit_depth`).
    ///
    /// VP9 codes `color_config` on a key frame, and on an intra-only frame of
    /// profile > 0; a profile-0 intra-only frame asserts the normative 8-bit
    /// 4:2:0 default, and an **inter frame codes none of it at all**
    /// (`read_uncompressed_header`, `vp9_decodeframe.c:2700-2760`). libvpx
    /// simply leaves `cm->bit_depth` / `cm->subsampling_*` / `cm->color_space`
    /// holding what the last frame that coded them left; this decoder records
    /// them here and [`Vp9DecState::begin_frame`] writes them back into an
    /// inter frame's header.
    ///
    /// Without that, an inter frame would reconstruct against
    /// `subsampling_x == subsampling_y == false` and allocate full-resolution
    /// chroma planes — not a subtle error, but one that only shows up on
    /// inter frames.
    pub bit_depth: u8,
    /// Sequence-level colour space (libvpx `cm->color_space`) — see
    /// [`Vp9DecState::bit_depth`].
    pub color_space: ColorSpace,
    /// Sequence-level horizontal chroma subsampling (libvpx
    /// `cm->subsampling_x`) — see [`Vp9DecState::bit_depth`].
    pub subsampling_x: bool,
    /// Sequence-level vertical chroma subsampling (libvpx
    /// `cm->subsampling_y`) — see [`Vp9DecState::bit_depth`].
    pub subsampling_y: bool,
    /// Previous frame's per-MI segment ids (libvpx `cm->last_frame_seg_map`),
    /// read by temporally-predicted segment ids. Sized by
    /// [`Vp9DecState::ensure_seg_map_size`]; the decode pass that writes it
    /// arrives with inter mode info.
    pub last_frame_seg_map: Vec<u8>,
    /// Persistent per-reference loop-filter deltas (libvpx `lf->ref_deltas`).
    pub lf_ref_deltas: [i8; 4],
    /// Persistent per-mode loop-filter deltas (libvpx `lf->mode_deltas`).
    pub lf_mode_deltas: [i8; 2],
    /// Persistent segmentation `abs_delta` (libvpx `seg->abs_delta`).
    pub seg_abs_delta: bool,
    /// Persistent per-segment feature enable flags (libvpx
    /// `seg->feature_mask`).
    pub seg_feature_enabled: [[bool; 4]; 8],
    /// Persistent per-segment feature data (libvpx `seg->feature_data`).
    pub seg_feature_data: [[i16; 4]; 8],
}

impl Vp9DecState {
    /// Fresh decoder state: default probabilities in the working context, no
    /// saved context marked initialised (so a stream that opens with a frame
    /// naming an unwritten context fails honestly), empty reference slots.
    #[must_use]
    pub fn new() -> Self {
        let defaults = FrameProbs::defaults();
        Self {
            frame_contexts: [
                defaults.clone(),
                defaults.clone(),
                defaults.clone(),
                defaults.clone(),
            ],
            contexts_initialized: [false; FRAME_CONTEXTS],
            fc: defaults,
            counts: FrameCounts::new(),
            tx_mode: TxMode::Only4x4,
            dpb: Default::default(),
            prev_frame: None,
            last_frame_type: Vp9FrameType::Key,
            last_width: 0,
            last_height: 0,
            last_show_frame: false,
            last_intra_only: false,
            // The normative profile-0 default. Every real stream opens with a
            // key frame, which codes `color_config` and overwrites all four
            // before any frame inherits them; seeding them with the default
            // rather than a sentinel keeps a malformed stream's first error
            // about its missing references, which is the real problem.
            bit_depth: 8,
            color_space: ColorSpace::Unknown,
            subsampling_x: true,
            subsampling_y: true,
            last_frame_seg_map: Vec::new(),
            lf_ref_deltas: DEFAULT_LF_REF_DELTAS,
            lf_mode_deltas: DEFAULT_LF_MODE_DELTAS,
            seg_abs_delta: false,
            seg_feature_enabled: [[false; 4]; 8],
            seg_feature_data: [[0; 4]; 8],
        }
    }

    /// Display dimensions currently held by each reference slot, in the shape
    /// [`UncompressedHeader::parse_with_ref_sizes`] consumes (libvpx reads
    /// `y_crop_width` / `y_crop_height` off `cm->frame_refs[i].buf`).
    #[must_use]
    pub fn ref_sizes(&self) -> [Option<(u32, u32)>; REF_FRAMES] {
        let mut sizes = [None; REF_FRAMES];
        for (out, slot) in sizes.iter_mut().zip(self.dpb.iter()) {
            *out = slot.as_ref().map(Vp9RefSlot::dimensions);
        }
        sizes
    }

    /// Whether this frame must reset its inherited state — libvpx
    /// `vp9_decodeframe.c:2822`:
    ///
    /// ```c
    /// if (frame_is_intra_only(cm) || cm->error_resilient_mode)
    ///   vp9_setup_past_independence(cm);
    /// ```
    ///
    /// `frame_is_intra_only` is "key frame **or** intra-only frame", i.e.
    /// [`UncompressedHeader::is_intra_only`].
    #[must_use]
    pub fn needs_past_independence(hdr: &UncompressedHeader) -> bool {
        hdr.is_intra_only() || hdr.error_resilient
    }

    /// `vp9_setup_past_independence` (libvpx `vp9_entropymode.c`), verbatim in
    /// effect:
    ///
    /// * clear every segmentation feature and return to delta (not absolute)
    ///   feature data;
    /// * zero the previous-frame segment map;
    /// * restore the default loop-filter deltas;
    /// * reset the working probability context to the specification defaults;
    /// * overwrite **all four** saved contexts when `frame_type == KEY ||
    ///   error_resilient || reset_frame_context == 3`, **only**
    ///   `frame_contexts[frame_context_idx]` when `reset_frame_context == 2`,
    ///   and none of them for `0` or `1`;
    /// * zero the reference sign biases and force `frame_context_idx` to 0.
    ///
    /// The last two land in `hdr`, which is why this takes it mutably: libvpx
    /// keeps both in `VP9_COMMON` alongside everything else, and the reset
    /// happens *after* the bits were read, so an error-resilient inter frame
    /// really does lose the sign biases it just transmitted.
    ///
    /// Callers gate on [`Vp9DecState::needs_past_independence`]; the
    /// `reset_frame_context` branch inside is a different, narrower test.
    pub fn setup_past_independence(&mut self, hdr: &mut UncompressedHeader) {
        self.seg_abs_delta = false;
        self.seg_feature_enabled = [[false; 4]; 8];
        self.seg_feature_data = [[0; 4]; 8];
        self.last_frame_seg_map.fill(0);

        self.lf_ref_deltas = DEFAULT_LF_REF_DELTAS;
        self.lf_mode_deltas = DEFAULT_LF_MODE_DELTAS;

        self.fc = FrameProbs::defaults();

        if hdr.frame_type == Vp9FrameType::Key
            || hdr.error_resilient
            || hdr.reset_frame_context == 3
        {
            for (ctx, initialized) in self
                .frame_contexts
                .iter_mut()
                .zip(self.contexts_initialized.iter_mut())
            {
                *ctx = self.fc.clone();
                *initialized = true;
            }
        } else if hdr.reset_frame_context == 2 {
            // Read before `frame_context_idx` is forced to 0 below, matching
            // libvpx's statement order.
            let idx = usize::from(hdr.frame_context_idx).min(FRAME_CONTEXTS - 1);
            self.frame_contexts[idx] = self.fc.clone();
            self.contexts_initialized[idx] = true;
        }

        hdr.ref_frame_sign_bias = [false; 4];
        hdr.frame_context_idx = 0;
    }

    /// Loads `frame_contexts[idx]` into the working context (libvpx
    /// `vp9_decodeframe.c:2996`: `*cm->fc = cm->frame_contexts[...]`).
    ///
    /// # Errors
    ///
    /// [`CodecError::InvalidBitstream`] when `idx` is out of range, or when
    /// the named context was never written — libvpx's *"Uninitialized entropy
    /// context"* error (`vp9_decodeframe.c:2997-2999`). Decoding against an
    /// unwritten context would silently produce garbage, so this fails
    /// instead.
    pub fn load_frame_context(&mut self, idx: u8) -> CodecResult<()> {
        let i = usize::from(idx);
        if i >= FRAME_CONTEXTS {
            return Err(CodecError::InvalidBitstream(format!(
                "VP9: frame_context_idx {i} out of range (FRAME_CONTEXTS = {FRAME_CONTEXTS})"
            )));
        }
        if !self.contexts_initialized[i] {
            return Err(CodecError::InvalidBitstream(format!(
                "VP9: uninitialized entropy context {i} — no preceding frame \
                 reset or refreshed it"
            )));
        }
        self.fc = self.frame_contexts[i].clone();
        Ok(())
    }

    /// Saves the working context into `frame_contexts[idx]` (libvpx
    /// `vp9_decodeframe.c:3068`, run when `refresh_frame_context` is set).
    ///
    /// # Errors
    ///
    /// [`CodecError::InvalidBitstream`] when `idx` is out of range.
    pub fn save_frame_context(&mut self, idx: u8) -> CodecResult<()> {
        let i = usize::from(idx);
        if i >= FRAME_CONTEXTS {
            return Err(CodecError::InvalidBitstream(format!(
                "VP9: frame_context_idx {i} out of range (FRAME_CONTEXTS = {FRAME_CONTEXTS})"
            )));
        }
        self.frame_contexts[i] = self.fc.clone();
        self.contexts_initialized[i] = true;
        Ok(())
    }

    /// Whether the frame may use the previous frame's motion vectors as
    /// temporal candidates — libvpx `vp9_decodeframe.c:2989-2992`:
    ///
    /// ```c
    /// cm->use_prev_frame_mvs =
    ///     !cm->error_resilient_mode && cm->width == cm->last_width &&
    ///     cm->height == cm->last_height && !cm->last_intra_only &&
    ///     cm->last_show_frame && (cm->last_frame_type != KEY_FRAME);
    /// ```
    ///
    /// The `last_frame_type != KEY_FRAME` term is load-bearing and is *not*
    /// implied by `!last_intra_only`: libvpx only assigns `cm->intra_only` on
    /// the non-key branch of `read_uncompressed_header`
    /// (`vp9_decodeframe.c:2706`), so after a key frame `last_intra_only`
    /// holds a stale value. This decoder records `last_intra_only = false` for
    /// key frames, which is cleaner but makes the explicit frame-type test the
    /// only thing excluding them.
    ///
    /// The final `prev_frame.is_some()` term has no libvpx counterpart
    /// because libvpx dereferences `cm->prev_frame` unconditionally once the
    /// flag is set; here it keeps the candidate scan total.
    #[must_use]
    pub fn use_prev_frame_mvs(&self, hdr: &UncompressedHeader) -> bool {
        !hdr.error_resilient
            && hdr.width == self.last_width
            && hdr.height == self.last_height
            && !self.last_intra_only
            && self.last_show_frame
            && self.last_frame_type != Vp9FrameType::Key
            && self.prev_frame.is_some()
    }

    /// Whether this frame accumulates symbol counters and runs backward
    /// probability adaptation.
    ///
    /// libvpx expresses the same condition three times and they must not
    /// drift, so this is the single source for all three:
    ///
    /// * the counter zeroing, `if (!cm->frame_parallel_decoding_mode)
    ///   vp9_zero(cm->counts);` on the `!error_resilient_mode` branch of
    ///   `read_uncompressed_header` (`vp9_decodeframe.c:2783-2787`);
    /// * the counter pointer, `xd->counts = cm->frame_parallel_decoding_mode
    ///   ? 0 : &counts` (`vp9_decodeframe.c:1979-1980`), which every
    ///   `if (counts)` guard in `vp9_detokenize.c` / `vp9_decodemv.c` keys
    ///   off;
    /// * the adaptation itself, `if (!cm->error_resilient_mode &&
    ///   !cm->frame_parallel_decoding_mode)` (`vp9_decodeframe.c:3050`).
    ///
    /// The first and third mention `error_resilient_mode` and the second does
    /// not, but they agree: an error-resilient frame codes no
    /// `frame_parallel_decoding_mode` bit and libvpx forces the flag to 1 for
    /// it (`vp9_decodeframe.c:2789-2790`), which
    /// [`UncompressedHeader::parse_with_ref_sizes`] mirrors. Both terms are
    /// kept here so the predicate stays correct even for a header built by
    /// hand in a test.
    #[must_use]
    pub fn counts_enabled(hdr: &UncompressedHeader) -> bool {
        !hdr.error_resilient && !hdr.frame_parallel_decoding
    }

    /// Applies everything that happens between parsing a frame header and
    /// decoding its tiles: the past-independence reset, the loop-filter and
    /// segmentation merges, the probability-context load and the counter
    /// reset.
    ///
    /// `hdr` is updated in place so the rest of the decode reads merged,
    /// post-reset values: inherited loop-filter deltas and segmentation
    /// feature data, `frame_context_idx == 0` and zeroed sign biases after a
    /// reset.
    ///
    /// # Errors
    ///
    /// Propagates [`Vp9DecState::load_frame_context`].
    pub fn begin_frame(&mut self, hdr: &mut UncompressedHeader) -> CodecResult<()> {
        self.apply_color_config(hdr);
        if Self::needs_past_independence(hdr) {
            self.setup_past_independence(hdr);
        }
        self.apply_loop_filter_deltas(hdr);
        self.apply_segmentation_data(hdr);
        self.load_frame_context(hdr.frame_context_idx)?;
        if Self::counts_enabled(hdr) {
            self.counts.reset();
        }
        Ok(())
    }

    /// Records or replays the sequence-level colour configuration
    /// (`read_bitdepth_colorspace_sampling` and the profile-0 intra-only
    /// default, `vp9_decodeframe.c:2699-2760`).
    ///
    /// A key or intra-only frame *codes* (or, for profile-0 intra-only,
    /// asserts) the configuration, so it is the writer and the state records
    /// what its header holds. An inter frame codes none of it, so the state
    /// is the writer and the header is filled in — see
    /// [`Vp9DecState::bit_depth`] for why leaving it unfilled is not an
    /// option.
    pub fn apply_color_config(&mut self, hdr: &mut UncompressedHeader) {
        if hdr.is_intra_only() {
            self.bit_depth = hdr.bit_depth;
            self.color_space = hdr.color_space;
            self.subsampling_x = hdr.subsampling_x;
            self.subsampling_y = hdr.subsampling_y;
        } else {
            hdr.bit_depth = self.bit_depth;
            hdr.color_space = self.color_space;
            hdr.subsampling_x = self.subsampling_x;
            hdr.subsampling_y = self.subsampling_y;
        }
    }

    /// Merges this frame's loop-filter delta updates into the persistent
    /// deltas, then writes the merged values back into `hdr`.
    ///
    /// libvpx `setup_loopfilter` only assigns the individually-flagged
    /// entries, and only when `mode_ref_delta_enabled && mode_ref_delta_update`
    /// — every other entry keeps the value the last frame that wrote it left
    /// behind, which is why the parser records *which* entries the frame
    /// actually transmitted rather than seeding defaults.
    pub fn apply_loop_filter_deltas(&mut self, hdr: &mut UncompressedHeader) {
        let lf = &mut hdr.loop_filter;
        if lf.delta_enabled && lf.delta_update {
            for (i, updated) in lf.ref_delta_updated.iter().enumerate() {
                if *updated {
                    self.lf_ref_deltas[i] = lf.ref_deltas[i];
                }
            }
            for (i, updated) in lf.mode_delta_updated.iter().enumerate() {
                if *updated {
                    self.lf_mode_deltas[i] = lf.mode_deltas[i];
                }
            }
        }
        lf.ref_deltas = self.lf_ref_deltas;
        lf.mode_deltas = self.lf_mode_deltas;
    }

    /// Merges this frame's segmentation feature data into the persistent set,
    /// then writes it back into `hdr`.
    ///
    /// libvpx `setup_segmentation` clears and re-reads the whole feature set
    /// when `update_data` is signalled, and leaves it entirely alone
    /// otherwise (including when segmentation is disabled, where it returns
    /// before reaching the update flag) — so an inter frame that enables
    /// segmentation without retransmitting the data decodes against the
    /// features an earlier frame installed.
    pub fn apply_segmentation_data(&mut self, hdr: &mut UncompressedHeader) {
        let seg = &mut hdr.seg;
        if seg.enabled && seg.update_data {
            self.seg_abs_delta = seg.abs_delta;
            self.seg_feature_enabled = seg.feature_enabled;
            self.seg_feature_data = seg.feature_data;
        }
        seg.abs_delta = self.seg_abs_delta;
        seg.feature_enabled = self.seg_feature_enabled;
        seg.feature_data = self.seg_feature_data;
    }

    /// Backward probability adaptation (libvpx `vp9_decodeframe.c:3049-3057`).
    ///
    /// A no-op unless [`Vp9DecState::counts_enabled`]; otherwise it folds the
    /// counters this frame accumulated into the working context, merging from
    /// `frame_contexts[frame_context_idx]` — the context as it was *loaded*,
    /// before the compressed header's `diff_update_prob` passes touched the
    /// working copy and before the conditional save writes it back.
    ///
    /// Two orderings inside [`Vp9DecState::finish_frame`] are load-bearing
    /// and this method depends on both: it must run **before** the save (or
    /// `pre_fc` would be this frame's own output) and **before**
    /// `last_frame_type` advances (or the "adapt quickly after a key frame"
    /// update factor would read this frame's type instead of its
    /// predecessor's).
    ///
    /// Coefficient probabilities adapt for every adapting frame; the mode and
    /// motion-vector groups adapt only for an inter frame, exactly as libvpx
    /// gates them (`vp9_decodeframe.c:3050-3056`):
    ///
    /// ```c
    /// if (!cm->error_resilient_mode && !cm->frame_parallel_decoding_mode) {
    ///   vp9_adapt_coef_probs(cm);
    ///   if (!frame_is_intra_only(cm)) {
    ///     vp9_adapt_mode_probs(cm);
    ///     vp9_adapt_mv_probs(cm, cm->allow_high_precision_mv);
    ///   }
    /// }
    /// ```
    ///
    /// The `frame_is_intra_only` gate is not cosmetic: a key frame *does*
    /// populate the `partition`, `y_mode` and `skip` counters, so dropping
    /// the gate would move probabilities libvpx leaves untouched and
    /// desynchronise every following frame that loads the context.
    ///
    /// [`Vp9DecState::tx_mode`] must describe the frame being closed —
    /// `vp9_adapt_mode_probs`'s transform-size block is gated on it.
    pub fn adapt_probabilities(&mut self, hdr: &UncompressedHeader) {
        if !Self::counts_enabled(hdr) {
            return;
        }
        let idx = usize::from(hdr.frame_context_idx).min(FRAME_CONTEXTS - 1);
        let pre = self.frame_contexts[idx].clone();
        adapt_coef_probs(
            &mut self.fc,
            &pre,
            &self.counts,
            hdr.is_intra_only(),
            self.last_frame_type,
        );
        if !hdr.is_intra_only() {
            adapt_mode_probs(
                &mut self.fc,
                &pre,
                &self.counts,
                hdr.interp_filter == SWITCHABLE,
                self.tx_mode,
            );
            adapt_mv_probs(
                &mut self.fc.mv,
                &pre.mv,
                &self.counts.mv,
                hdr.allow_high_precision_mv,
            );
        }
    }

    /// Closes a decoded frame: runs backward adaptation, saves the
    /// probability context if the header asked for it, refreshes the
    /// reference slots it flags, and advances the `last_*` / `prev_frame`
    /// state — in libvpx's order (`vp9_decode_frame`'s tail, then
    /// `swap_frame_buffers`).
    ///
    /// # `show_existing_frame` may not call this
    ///
    /// A `show_existing_frame` packet decodes no frame, and the
    /// specification gates *every* end-of-frame update on
    /// `show_existing_frame == 0` — the reference refresh, the frame-context
    /// save, and (spec §8.10 step 2) the previous-motion-vector /
    /// `PrevRefFrames` save that [`Vp9DecState::prev_frame`] stands for. That
    /// last gate is **separate from `refresh_frame_flags`**: a normal frame
    /// with `refresh_frame_flags == 0` still becomes the next frame's
    /// temporal-MV source, while a `show_existing_frame` packet never does,
    /// whatever its (uncoded, zero) refresh mask says. libvpx implements it
    /// by returning before `swap_frame_buffers`' `cm->prev_frame` /
    /// `cm->last_show_frame` assignments (`vp9_decoder.c:483-486`).
    ///
    /// Rather than leave that invariant to each caller's early return, this
    /// function refuses the header outright: a caller that forgets gets a
    /// loud error instead of a silently corrupted temporal-MV predictor two
    /// frames later.
    ///
    /// # Errors
    ///
    /// [`CodecError::InvalidParameter`] when `hdr.show_existing_frame` is
    /// set; propagates [`Vp9DecState::save_frame_context`] otherwise.
    pub fn finish_frame(&mut self, hdr: &UncompressedHeader, frame: Vp9RefSlot) -> CodecResult<()> {
        if hdr.show_existing_frame {
            return Err(CodecError::InvalidParameter(
                "VP9: finish_frame called for a show_existing_frame packet — \
                 it decodes no frame, so the reference refresh, the frame \
                 context save and the previous-MV save (spec 8.10 step 2, \
                 gated on show_existing_frame == 0) must all be skipped"
                    .into(),
            ));
        }
        self.adapt_probabilities(hdr);
        if hdr.refresh_frame_context {
            self.save_frame_context(hdr.frame_context_idx)?;
        }
        refresh_dpb(&mut self.dpb, hdr.refresh_frame_flags, &frame);
        self.last_frame_type = hdr.frame_type;
        self.last_intra_only = hdr.intra_only;
        self.last_show_frame = hdr.show_frame;
        self.last_width = hdr.width;
        self.last_height = hdr.height;
        self.prev_frame = Some(frame);
        Ok(())
    }

    /// Ensures the previous-frame segment map holds exactly `mi_count`
    /// entries, discarding it (zero-filled) on a genuine size change.
    ///
    /// A no-op when the size already matches, which is the point: the map
    /// persists across frames so temporally-predicted segment ids have
    /// something to predict from.
    pub fn ensure_seg_map_size(&mut self, mi_count: usize) {
        if self.last_frame_seg_map.len() != mi_count {
            self.last_frame_seg_map = vec![0u8; mi_count];
        }
    }
}

#[cfg(test)]
impl Vp9DecState {
    /// Whether saved context `idx` still holds the specification defaults.
    ///
    /// Test-only: [`FrameProbs`] lives in a private module, so a test outside
    /// `dec` (the decoder-level fixture tests in `vp9/decoder.rs`) cannot name
    /// it to make the comparison itself. Out-of-range indices report `false`
    /// rather than panicking — an out-of-range context is by definition not
    /// the defaults.
    pub(crate) fn context_is_defaults(&self, idx: usize) -> bool {
        self.frame_contexts
            .get(idx)
            .is_some_and(|ctx| *ctx == FrameProbs::defaults())
    }
}

impl Default for Vp9DecState {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Vp9DecState {
    /// Summarises rather than dumping ~9 KB of probability tables and every
    /// reference plane.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let filled = self.dpb.iter().filter(|s| s.is_some()).count();
        f.debug_struct("Vp9DecState")
            .field("contexts_initialized", &self.contexts_initialized)
            .field("dpb_slots_filled", &filled)
            .field("prev_frame", &self.prev_frame)
            .field("last_frame_type", &self.last_frame_type)
            .field("last_width", &self.last_width)
            .field("last_height", &self.last_height)
            .field("last_show_frame", &self.last_show_frame)
            .field("last_intra_only", &self.last_intra_only)
            .field("bit_depth", &self.bit_depth)
            .field("subsampling", &(self.subsampling_x, self.subsampling_y))
            .field("tx_mode", &self.tx_mode)
            .field("lf_ref_deltas", &self.lf_ref_deltas)
            .field("lf_mode_deltas", &self.lf_mode_deltas)
            .field("seg_abs_delta", &self.seg_abs_delta)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::super::counts::{EOB_MODEL_TOKEN, ZERO_TOKEN};
    use super::super::recon::{DecodedFrame, PlaneBuf};
    use super::super::refs::MvRefRow;
    use super::*;
    use crate::vp9::uncompressed::{LoopFilterHeader, SegmentationHeader};

    /// A probability context distinguishable from the defaults in several
    /// fields, so a save/load round trip proves data actually moved.
    fn sentinel_probs(marker: u8) -> FrameProbs {
        let mut p = FrameProbs::defaults();
        p.coef[0][0][0][0][0][0] = marker;
        p.coef[3][1][1][5][5][2] = marker.wrapping_add(1);
        p.skip = [marker, marker, marker];
        p.tx32[1][2] = marker;
        p
    }

    fn inter_header() -> UncompressedHeader {
        UncompressedHeader {
            frame_type: Vp9FrameType::Inter,
            show_frame: true,
            width: 64,
            height: 64,
            ..UncompressedHeader::default()
        }
    }

    fn key_header() -> UncompressedHeader {
        UncompressedHeader {
            frame_type: Vp9FrameType::Key,
            show_frame: true,
            width: 64,
            height: 64,
            refresh_frame_flags: 0xFF,
            ..UncompressedHeader::default()
        }
    }

    // -- frame context save / load round trip ------------------------------

    #[test]
    fn save_then_load_round_trips_every_context() {
        let mut state = Vp9DecState::new();
        for idx in 0..FRAME_CONTEXTS {
            state.fc = sentinel_probs(idx as u8 + 1);
            state
                .save_frame_context(idx as u8)
                .expect("save in range succeeds");
        }
        for idx in 0..FRAME_CONTEXTS {
            state.fc = FrameProbs::defaults();
            state
                .load_frame_context(idx as u8)
                .expect("load in range succeeds");
            assert!(
                state.fc == sentinel_probs(idx as u8 + 1),
                "context {idx} must round trip byte for byte"
            );
        }
    }

    #[test]
    fn load_uninitialized_context_is_an_honest_error() {
        let mut state = Vp9DecState::new();
        let err = state
            .load_frame_context(2)
            .expect_err("never-written context must not decode");
        match err {
            CodecError::InvalidBitstream(msg) => {
                assert!(msg.contains("uninitialized entropy context 2"), "{msg}");
            }
            other => panic!("expected InvalidBitstream, got {other:?}"),
        }
    }

    #[test]
    fn context_index_out_of_range_is_an_error_both_ways() {
        let mut state = Vp9DecState::new();
        assert!(state.load_frame_context(4).is_err());
        assert!(state.save_frame_context(4).is_err());
        assert!(state.save_frame_context(3).is_ok());
    }

    // -- reset_frame_context 0 / 1 / 2 / 3 ---------------------------------

    /// State with all four contexts holding distinguishable, initialised
    /// probabilities, as a stream that has been running for a while would.
    fn warm_state() -> Vp9DecState {
        let mut state = Vp9DecState::new();
        for idx in 0..FRAME_CONTEXTS {
            state.fc = sentinel_probs(0x10 + idx as u8);
            state
                .save_frame_context(idx as u8)
                .expect("save in range succeeds");
        }
        state
    }

    /// An intra-only frame naming context 2, so both the "which contexts get
    /// reset" branch and the forced `frame_context_idx = 0` load are visible.
    fn intra_only_header(reset_frame_context: u8) -> UncompressedHeader {
        UncompressedHeader {
            frame_type: Vp9FrameType::Inter,
            intra_only: true,
            show_frame: false,
            reset_frame_context,
            frame_context_idx: 2,
            width: 64,
            height: 64,
            ..UncompressedHeader::default()
        }
    }

    #[test]
    fn reset_frame_context_0_and_1_preserve_all_saved_contexts() {
        for reset in [0u8, 1] {
            let mut state = warm_state();
            let mut hdr = intra_only_header(reset);
            state
                .begin_frame(&mut hdr)
                .expect("context 0 is initialised");

            // Nothing was overwritten...
            for idx in 0..FRAME_CONTEXTS {
                assert!(
                    state.frame_contexts[idx] == sentinel_probs(0x10 + idx as u8),
                    "reset_frame_context {reset} must not touch context {idx}"
                );
            }
            // ...and the defaults `setup_past_independence` put in `fc` were
            // replaced by the load of the (forced) context 0.
            assert_eq!(hdr.frame_context_idx, 0);
            assert!(
                state.fc == sentinel_probs(0x10),
                "reset_frame_context {reset} decodes against saved context 0"
            );
        }
    }

    #[test]
    fn reset_frame_context_2_resets_only_the_named_context() {
        let mut state = warm_state();
        let mut hdr = intra_only_header(2);
        state.begin_frame(&mut hdr).expect("contexts initialised");

        assert!(
            state.frame_contexts[2] == FrameProbs::defaults(),
            "the named context is reset"
        );
        for idx in [0usize, 1, 3] {
            assert!(
                state.frame_contexts[idx] == sentinel_probs(0x10 + idx as u8),
                "context {idx} survives a partial reset"
            );
        }
        // The load still uses the forced index 0, not the named index 2.
        assert!(state.fc == sentinel_probs(0x10));
    }

    #[test]
    fn reset_frame_context_3_resets_every_context() {
        let mut state = warm_state();
        let mut hdr = intra_only_header(3);
        state.begin_frame(&mut hdr).expect("contexts initialised");

        for idx in 0..FRAME_CONTEXTS {
            assert!(
                state.frame_contexts[idx] == FrameProbs::defaults(),
                "context {idx} reset"
            );
        }
        assert!(state.fc == FrameProbs::defaults());
    }

    #[test]
    fn keyframe_resets_every_context_regardless_of_reset_frame_context() {
        let mut state = warm_state();
        let mut hdr = key_header();
        hdr.reset_frame_context = 0;
        hdr.frame_context_idx = 3;
        state
            .begin_frame(&mut hdr)
            .expect("key frame initialises all");

        for idx in 0..FRAME_CONTEXTS {
            assert!(state.frame_contexts[idx] == FrameProbs::defaults());
            assert!(state.contexts_initialized[idx]);
        }
        assert_eq!(hdr.frame_context_idx, 0);
        assert!(state.fc == FrameProbs::defaults());
    }

    #[test]
    fn error_resilient_inter_frame_resets_every_context_and_sign_bias() {
        let mut state = warm_state();
        let mut hdr = inter_header();
        hdr.error_resilient = true;
        hdr.frame_parallel_decoding = true;
        hdr.ref_frame_sign_bias = [false, true, true, false];
        hdr.frame_context_idx = 1;
        state.begin_frame(&mut hdr).expect("reset initialises all");

        for idx in 0..FRAME_CONTEXTS {
            assert!(state.frame_contexts[idx] == FrameProbs::defaults());
        }
        assert_eq!(
            hdr.ref_frame_sign_bias, [false; 4],
            "an error-resilient frame loses the sign biases it transmitted"
        );
        assert_eq!(hdr.frame_context_idx, 0);
    }

    #[test]
    fn plain_inter_frame_does_not_reset_anything() {
        let mut state = warm_state();
        let mut hdr = inter_header();
        hdr.frame_context_idx = 3;
        hdr.ref_frame_sign_bias = [false, false, true, true];
        state
            .begin_frame(&mut hdr)
            .expect("context 3 is initialised");

        for idx in 0..FRAME_CONTEXTS {
            assert!(state.frame_contexts[idx] == sentinel_probs(0x10 + idx as u8));
        }
        assert_eq!(hdr.frame_context_idx, 3, "no forced reset to context 0");
        assert_eq!(hdr.ref_frame_sign_bias, [false, false, true, true]);
        assert!(state.fc == sentinel_probs(0x13), "loads the named context");
    }

    #[test]
    fn needs_past_independence_truth_table() {
        let mut hdr = inter_header();
        assert!(!Vp9DecState::needs_past_independence(&hdr));
        hdr.error_resilient = true;
        assert!(Vp9DecState::needs_past_independence(&hdr));
        hdr.error_resilient = false;
        hdr.intra_only = true;
        assert!(Vp9DecState::needs_past_independence(&hdr));
        assert!(Vp9DecState::needs_past_independence(&key_header()));
    }

    // -- use_prev_frame_mvs truth table ------------------------------------

    /// State whose `last_*` fields describe a shown 64x64 inter frame, i.e.
    /// every condition satisfied.
    fn state_after_inter_frame() -> Vp9DecState {
        let plane = |w: usize, h: usize| PlaneBuf {
            data: vec![0; w * h],
            stride: w,
            width: w,
            height: h,
        };
        let decoded = DecodedFrame {
            planes: [plane(64, 64), plane(32, 32), plane(32, 32)],
            width: 64,
            height: 64,
            mvs: vec![MvRefRow::default(); 8 * 8],
            tx_mode: TxMode::Only4x4,
        };
        let mut state = warm_state();
        state.last_frame_type = Vp9FrameType::Inter;
        state.last_width = 64;
        state.last_height = 64;
        state.last_show_frame = true;
        state.last_intra_only = false;
        state.prev_frame = match Vp9RefSlot::from_decoded_frame(decoded, false, true) {
            Ok(slot) => Some(slot),
            Err(e) => panic!("fixture frame is well formed: {e}"),
        };
        state
    }

    #[test]
    fn use_prev_frame_mvs_all_conditions_met() {
        let state = state_after_inter_frame();
        assert!(state.use_prev_frame_mvs(&inter_header()));
    }

    #[test]
    fn use_prev_frame_mvs_blocked_by_each_condition_individually() {
        // error_resilient
        let state = state_after_inter_frame();
        let mut hdr = inter_header();
        hdr.error_resilient = true;
        assert!(!state.use_prev_frame_mvs(&hdr), "error_resilient blocks");

        // width mismatch
        let mut hdr = inter_header();
        hdr.width = 128;
        assert!(!state.use_prev_frame_mvs(&hdr), "width change blocks");

        // height mismatch
        let mut hdr = inter_header();
        hdr.height = 128;
        assert!(!state.use_prev_frame_mvs(&hdr), "height change blocks");

        // previous frame was intra-only
        let mut state = state_after_inter_frame();
        state.last_intra_only = true;
        assert!(
            !state.use_prev_frame_mvs(&inter_header()),
            "last_intra_only blocks"
        );

        // previous frame was not shown
        let mut state = state_after_inter_frame();
        state.last_show_frame = false;
        assert!(
            !state.use_prev_frame_mvs(&inter_header()),
            "!last_show_frame blocks"
        );

        // previous frame was a key frame
        let mut state = state_after_inter_frame();
        state.last_frame_type = Vp9FrameType::Key;
        assert!(
            !state.use_prev_frame_mvs(&inter_header()),
            "last_frame_type == KEY blocks (libvpx vp9_decodeframe.c:2992)"
        );

        // no decoded previous frame at all
        let mut state = state_after_inter_frame();
        state.prev_frame = None;
        assert!(
            !state.use_prev_frame_mvs(&inter_header()),
            "absent prev_frame blocks"
        );
    }

    #[test]
    fn use_prev_frame_mvs_false_on_fresh_state() {
        let state = Vp9DecState::new();
        assert!(!state.use_prev_frame_mvs(&inter_header()));
    }

    // -- loop filter delta persistence -------------------------------------

    /// Header whose loop-filter section signals `delta_enabled` with no
    /// update — the case that must inherit.
    fn lf_header_no_update() -> UncompressedHeader {
        let mut hdr = inter_header();
        hdr.loop_filter = LoopFilterHeader {
            filter_level: 20,
            sharpness: 0,
            delta_enabled: true,
            delta_update: false,
            ref_deltas: DEFAULT_LF_REF_DELTAS,
            mode_deltas: DEFAULT_LF_MODE_DELTAS,
            ref_delta_updated: [false; 4],
            mode_delta_updated: [false; 2],
        };
        hdr.frame_context_idx = 0;
        hdr
    }

    #[test]
    fn lf_deltas_persist_across_frames_without_an_update() {
        let mut state = warm_state();
        state.lf_ref_deltas = [3, -3, 2, -2];
        state.lf_mode_deltas = [5, -5];

        let mut hdr = lf_header_no_update();
        state.begin_frame(&mut hdr).expect("context 0 initialised");

        assert_eq!(
            hdr.loop_filter.ref_deltas,
            [3, -3, 2, -2],
            "a frame with delta_update == 0 inherits the previous deltas"
        );
        assert_eq!(hdr.loop_filter.mode_deltas, [5, -5]);
    }

    #[test]
    fn lf_delta_update_touches_only_flagged_entries() {
        let mut state = warm_state();
        state.lf_ref_deltas = [3, -3, 2, -2];
        state.lf_mode_deltas = [5, -5];

        let mut hdr = lf_header_no_update();
        hdr.loop_filter.delta_update = true;
        hdr.loop_filter.ref_deltas = [9, 0, 0, 7];
        hdr.loop_filter.ref_delta_updated = [true, false, false, true];
        hdr.loop_filter.mode_deltas = [0, 6];
        hdr.loop_filter.mode_delta_updated = [false, true];

        state.begin_frame(&mut hdr).expect("context 0 initialised");

        assert_eq!(hdr.loop_filter.ref_deltas, [9, -3, 2, 7]);
        assert_eq!(hdr.loop_filter.mode_deltas, [5, 6]);
        assert_eq!(state.lf_ref_deltas, [9, -3, 2, 7], "state advanced too");
        assert_eq!(state.lf_mode_deltas, [5, 6]);
    }

    #[test]
    fn lf_deltas_are_not_updated_when_delta_enabled_is_clear() {
        let mut state = warm_state();
        state.lf_ref_deltas = [3, -3, 2, -2];

        let mut hdr = lf_header_no_update();
        hdr.loop_filter.delta_enabled = false;
        hdr.loop_filter.delta_update = true;
        hdr.loop_filter.ref_deltas = [9, 9, 9, 9];
        hdr.loop_filter.ref_delta_updated = [true; 4];

        state.begin_frame(&mut hdr).expect("context 0 initialised");

        assert_eq!(
            state.lf_ref_deltas,
            [3, -3, 2, -2],
            "libvpx reads no delta payload when mode_ref_delta_enabled is 0"
        );
    }

    #[test]
    fn lf_deltas_reset_by_past_independence_before_the_merge() {
        let mut state = warm_state();
        state.lf_ref_deltas = [3, -3, 2, -2];
        state.lf_mode_deltas = [5, -5];

        // A key frame resets, then applies only what it transmits.
        let mut hdr = key_header();
        hdr.loop_filter = LoopFilterHeader {
            filter_level: 20,
            sharpness: 0,
            delta_enabled: true,
            delta_update: true,
            ref_deltas: [0, 0, 0, 4],
            mode_deltas: DEFAULT_LF_MODE_DELTAS,
            ref_delta_updated: [false, false, false, true],
            mode_delta_updated: [false; 2],
        };
        state.begin_frame(&mut hdr).expect("key frame initialises");

        assert_eq!(
            hdr.loop_filter.ref_deltas,
            [1, 0, -1, 4],
            "reset to defaults, then the frame's own flagged update applies"
        );
        assert_eq!(hdr.loop_filter.mode_deltas, DEFAULT_LF_MODE_DELTAS);
    }

    // -- segmentation feature persistence ----------------------------------

    #[test]
    fn segmentation_features_persist_without_update_data() {
        let mut state = warm_state();
        state.seg_abs_delta = true;
        state.seg_feature_enabled[1][0] = true;
        state.seg_feature_data[1][0] = -12;

        let mut hdr = inter_header();
        hdr.seg = SegmentationHeader {
            enabled: true,
            update_data: false,
            ..SegmentationHeader::default()
        };
        hdr.frame_context_idx = 0;
        state.begin_frame(&mut hdr).expect("context 0 initialised");

        assert!(hdr.seg.abs_delta);
        assert!(hdr.seg.feature_enabled[1][0]);
        assert_eq!(hdr.seg.feature_data[1][0], -12);
    }

    #[test]
    fn segmentation_update_data_replaces_the_whole_feature_set() {
        let mut state = warm_state();
        state.seg_feature_enabled[1][0] = true;
        state.seg_feature_data[1][0] = -12;

        let mut hdr = inter_header();
        let mut seg = SegmentationHeader {
            enabled: true,
            update_data: true,
            ..SegmentationHeader::default()
        };
        seg.feature_enabled[2][1] = true;
        seg.feature_data[2][1] = 7;
        hdr.seg = seg;
        hdr.frame_context_idx = 0;
        state.begin_frame(&mut hdr).expect("context 0 initialised");

        assert!(!hdr.seg.feature_enabled[1][0], "old features cleared");
        assert!(hdr.seg.feature_enabled[2][1]);
        assert_eq!(state.seg_feature_data[2][1], 7, "state advanced too");
    }

    #[test]
    fn segmentation_features_cleared_by_past_independence() {
        let mut state = warm_state();
        state.seg_abs_delta = true;
        state.seg_feature_enabled[1][0] = true;
        state.seg_feature_data[1][0] = -12;

        let mut hdr = key_header();
        hdr.seg = SegmentationHeader {
            enabled: true,
            update_data: false,
            ..SegmentationHeader::default()
        };
        state.begin_frame(&mut hdr).expect("key frame initialises");

        assert!(!hdr.seg.abs_delta);
        assert!(!hdr.seg.feature_enabled[1][0]);
        assert_eq!(hdr.seg.feature_data[1][0], 0);
    }

    // -- frame completion ---------------------------------------------------

    #[test]
    fn finish_frame_advances_last_state_and_saves_context_when_asked() {
        let mut state = warm_state();
        let slot = match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };
        let mut hdr = inter_header();
        hdr.refresh_frame_flags = 0b0000_0110;
        hdr.refresh_frame_context = true;
        hdr.frame_context_idx = 1;
        // Frame-parallel: no adaptation, so the working context is saved
        // exactly as the frame left it (see the sibling test below for what
        // an adapting frame saves instead).
        hdr.frame_parallel_decoding = true;
        state.fc = sentinel_probs(0xAA);

        state.finish_frame(&hdr, slot).expect("in-range context");

        assert!(state.dpb[1].is_some());
        assert!(state.dpb[2].is_some());
        assert!(state.dpb[0].is_none());
        assert!(state.frame_contexts[1] == sentinel_probs(0xAA));
        assert_eq!(state.last_frame_type, Vp9FrameType::Inter);
        assert_eq!(state.last_width, 64);
        assert_eq!(state.last_height, 64);
        assert!(state.last_show_frame);
        assert!(!state.last_intra_only);
        assert!(state.prev_frame.is_some());
    }

    /// An **adapting** frame does not save the working context verbatim: the
    /// coefficient probabilities it saves are
    /// `merge(frame_contexts[idx], counts)`, so this frame's own
    /// compressed-header `diff_update_prob` results are discarded — libvpx
    /// `adapt_coef_probs` assigns `probs[...] = merge_probs(pre_probs[...],
    /// ...)` (`vp9_entropy.c:1069-1082`), and the specification says the same
    /// with its separate `PreCoefProbs` tables (`load_probs2`, §8.4.2).
    ///
    /// On an **inter** frame `vp9_adapt_mode_probs` runs too, so `skip` is
    /// re-derived from the loaded context in the same way. The transform
    /// probabilities are the group that survives here: their block is gated
    /// on `cm->tx_mode == TX_MODE_SELECT`, and this state's
    /// [`Vp9DecState::tx_mode`] is not `Select` — so the saved context is
    /// genuinely a mix of merged and forward-updated values.
    #[test]
    fn an_adapting_inter_frame_saves_merged_probs_but_keeps_the_ungated_groups() {
        let mut state = warm_state();
        let slot = match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };
        let mut hdr = inter_header();
        hdr.refresh_frame_context = true;
        hdr.frame_context_idx = 1;
        // No counts at all: every merge collapses to "keep the prior", which
        // makes the *source* of that prior unambiguous.
        state.fc = sentinel_probs(0xAA);
        assert_ne!(
            state.tx_mode,
            TxMode::Select,
            "this test's point is that the tx block is gated off"
        );

        state.finish_frame(&hdr, slot).expect("in-range context");

        let saved = &state.frame_contexts[1];
        assert_eq!(
            saved.coef[0][0][0][0][0],
            sentinel_probs(0x11).coef[0][0][0][0][0],
            "coef probabilities come back from frame_contexts[1], not from 0xAA"
        );
        assert_eq!(
            saved.skip, [0x11; 3],
            "an inter frame adapts skip as well, so it too comes back from \
             frame_contexts[1] rather than keeping the 0xAA forward update"
        );
        assert_eq!(
            saved.tx32[1][2], 0xAA,
            "the transform probabilities are gated on TX_MODE_SELECT, so the \
             frame's own forward update survives"
        );
    }

    /// The `!frame_is_intra_only(cm)` gate on `vp9_adapt_mode_probs` /
    /// `vp9_adapt_mv_probs` (`vp9_decodeframe.c:3052-3056`) is load-bearing:
    /// a key frame populates `skip` (and `partition`, and `y_mode`) counters
    /// just like an inter frame does, so without the gate its forward updates
    /// would be overwritten from the loaded context and every following frame
    /// naming that context would decode against the wrong probabilities.
    ///
    /// Same shape as the inter test above, with the frame type as the only
    /// difference — so the `skip` assertion flipping is attributable to the
    /// gate and nothing else.
    #[test]
    fn an_adapting_key_frame_does_not_adapt_the_mode_probabilities() {
        let mut state = warm_state();
        let slot = match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };
        let mut hdr = key_header();
        hdr.refresh_frame_context = true;
        hdr.frame_context_idx = 1;
        state.fc = sentinel_probs(0xAA);

        state.finish_frame(&hdr, slot).expect("in-range context");

        let saved = &state.frame_contexts[1];
        assert_eq!(
            saved.coef[0][0][0][0][0],
            sentinel_probs(0x11).coef[0][0][0][0][0],
            "coefficient adaptation runs for a key frame too"
        );
        assert_eq!(
            saved.skip, [0xAA; 3],
            "skip is not adapted on an intra frame, so the frame's own \
             forward update survives"
        );
    }

    #[test]
    fn finish_frame_without_refresh_frame_context_leaves_contexts_alone() {
        let mut state = warm_state();
        let slot = match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };
        let mut hdr = inter_header();
        hdr.refresh_frame_context = false;
        hdr.frame_context_idx = 1;
        state.fc = sentinel_probs(0xAA);

        state.finish_frame(&hdr, slot).expect("no save requested");

        assert!(state.frame_contexts[1] == sentinel_probs(0x11));
    }

    #[test]
    fn ref_sizes_reports_slot_dimensions() {
        let mut state = Vp9DecState::new();
        let slot = match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };
        refresh_dpb(&mut state.dpb, 0b1001, &slot);
        let sizes = state.ref_sizes();
        assert_eq!(sizes[0], Some((64, 64)));
        assert_eq!(sizes[3], Some((64, 64)));
        assert_eq!(sizes[1], None);
    }

    // -- segment map sizing --------------------------------------------------

    #[test]
    fn seg_map_sizing_preserves_contents_on_no_op_and_discards_on_change() {
        let mut state = Vp9DecState::new();
        assert!(state.last_frame_seg_map.is_empty());
        state.ensure_seg_map_size(6);
        state.last_frame_seg_map[2] = 3;
        state.ensure_seg_map_size(6);
        assert_eq!(state.last_frame_seg_map, vec![0, 0, 3, 0, 0, 0]);
        state.ensure_seg_map_size(9);
        assert_eq!(state.last_frame_seg_map, vec![0u8; 9]);
    }

    #[test]
    fn seg_map_zeroed_in_place_by_past_independence() {
        let mut state = Vp9DecState::new();
        state.ensure_seg_map_size(4);
        state.last_frame_seg_map.fill(2);
        let mut hdr = key_header();
        state.setup_past_independence(&mut hdr);
        assert_eq!(state.last_frame_seg_map, vec![0u8; 4]);
    }

    // -- counters -----------------------------------------------------------

    #[test]
    fn counts_reset_only_when_the_frame_will_adapt() {
        let mut state = warm_state();
        state.counts.coef[0][0][0][1][0][EOB_MODEL_TOKEN] = 5;
        let mut hdr = inter_header();
        hdr.frame_parallel_decoding = true;
        hdr.frame_context_idx = 0;
        state.begin_frame(&mut hdr).expect("context 0 initialised");
        assert_eq!(
            state.counts.coef[0][0][0][1][0][EOB_MODEL_TOKEN], 5,
            "frame-parallel frames never adapt, so libvpx skips the zeroing"
        );

        let mut hdr = inter_header();
        hdr.frame_parallel_decoding = false;
        hdr.frame_context_idx = 0;
        state.begin_frame(&mut hdr).expect("context 0 initialised");
        assert!(state.counts.is_zero());
    }

    #[test]
    fn counts_enabled_matches_libvpx_gate() {
        let mut hdr = inter_header();
        assert!(
            Vp9DecState::counts_enabled(&hdr),
            "a plain inter frame counts and adapts"
        );
        hdr.frame_parallel_decoding = true;
        assert!(!Vp9DecState::counts_enabled(&hdr));

        let mut hdr = inter_header();
        hdr.error_resilient = true;
        assert!(!Vp9DecState::counts_enabled(&hdr));
        // The real parser forces frame_parallel_decoding for an
        // error-resilient frame, so both terms agree on real headers; this
        // one is hand-built with only the first term set.
        assert!(!hdr.frame_parallel_decoding);
    }

    // -- backward adaptation ------------------------------------------------

    /// State whose context 0 holds the defaults and whose counters describe a
    /// frame that coded some coefficients.
    fn state_with_coef_counts() -> (Vp9DecState, UncompressedHeader) {
        let mut state = Vp9DecState::new();
        state.fc = FrameProbs::defaults();
        state.save_frame_context(0).expect("context 0 in range");
        state.counts.eob_branch[0][0][0][1][0] = 30;
        state.counts.coef[0][0][0][1][0][EOB_MODEL_TOKEN] = 6;
        state.counts.coef[0][0][0][1][0][ZERO_TOKEN] = 10;

        let mut hdr = inter_header();
        hdr.frame_context_idx = 0;
        (state, hdr)
    }

    #[test]
    fn adaptation_moves_the_working_context_and_the_save_persists_it() {
        let (mut state, mut hdr) = state_with_coef_counts();
        hdr.refresh_frame_context = true;
        let slot = match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };

        state.finish_frame(&hdr, slot).expect("in-range context");

        assert!(
            state.fc != FrameProbs::defaults(),
            "counted coefficients must move the working context"
        );
        assert!(
            state.frame_contexts[0] == state.fc,
            "refresh_frame_context saves the *adapted* context, not the loaded one"
        );
    }

    #[test]
    fn adaptation_is_skipped_for_error_resilient_and_frame_parallel_frames() {
        for (er, fpd) in [(true, true), (false, true), (true, false)] {
            let (mut state, mut hdr) = state_with_coef_counts();
            hdr.error_resilient = er;
            hdr.frame_parallel_decoding = fpd;
            let slot = match state_after_inter_frame().prev_frame {
                Some(slot) => slot,
                None => panic!("fixture builds a slot"),
            };
            state.finish_frame(&hdr, slot).expect("in-range context");
            assert!(
                state.fc == FrameProbs::defaults(),
                "er={er} fpd={fpd}: a non-adapting frame may not move a probability"
            );
        }
    }

    /// The prior is `frame_contexts[frame_context_idx]`, not the working
    /// context the compressed header left behind.
    #[test]
    fn adaptation_merges_from_the_saved_context_not_the_working_one() {
        let (mut state, hdr) = state_with_coef_counts();
        // As if this frame's compressed header had moved the node a long way.
        state.fc.coef[0][0][0][1][0][0] = 220;
        let saved_prior = state.frame_contexts[0].coef[0][0][0][1][0][0];
        assert_ne!(saved_prior, 220, "the fixture must make the two differ");

        state.adapt_probabilities(&hdr);

        let mut expected = FrameProbs::defaults();
        let pre = FrameProbs::defaults();
        adapt_coef_probs(
            &mut expected,
            &pre,
            &state.counts,
            hdr.is_intra_only(),
            Vp9FrameType::Key,
        );
        assert_eq!(
            state.fc.coef[0][0][0][1][0][0], expected.coef[0][0][0][1][0][0],
            "the merge must start from frame_contexts[0], not from 220"
        );
    }

    /// The update factor depends on the *previous* frame's type, so
    /// adaptation has to run before `last_frame_type` advances.
    #[test]
    fn adaptation_runs_before_last_frame_type_advances() {
        let slot = || match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };

        let (mut after_key, hdr) = state_with_coef_counts();
        after_key.last_frame_type = Vp9FrameType::Key;
        after_key.finish_frame(&hdr, slot()).expect("in range");

        let (mut steady, hdr) = state_with_coef_counts();
        steady.last_frame_type = Vp9FrameType::Inter;
        steady.finish_frame(&hdr, slot()).expect("in range");

        assert!(
            after_key.fc != steady.fc,
            "libvpx adapts quickly (factor 128) only when the *previous* \
             frame was a key frame; reading the advanced value would make \
             these identical"
        );
        assert_eq!(
            after_key.last_frame_type,
            Vp9FrameType::Inter,
            "and the field really did advance afterwards"
        );
    }

    /// Real libvpx intra-only frame (see `testdata/RECIPE-p9io.md`): the only
    /// fixture in this crate that codes `frame_parallel_decoding_mode == 0`
    /// and therefore actually counts and adapts.
    const INTRA_ONLY_352X288: &[u8] = include_bytes!("testdata/p9io.frame0.bin");

    /// Adaptation must change the working context on a **real** frame's
    /// counts — not merely leave it "not the defaults", which the compressed
    /// header's own `diff_update_prob` passes already achieve.
    ///
    /// This is the discriminating check: `fc` is captured after the decode
    /// (defaults + this frame's forward updates) and again after
    /// [`Vp9DecState::adapt_probabilities`], and the two must differ. A
    /// stubbed-out adaptation leaves them equal while every "the saved
    /// context is not the defaults" assertion still passes.
    #[test]
    fn adaptation_changes_a_real_frames_context_beyond_its_forward_updates() {
        let mut state = Vp9DecState::new();
        let mut hdr = UncompressedHeader::parse(INTRA_ONLY_352X288).expect("header parses");
        assert!(hdr.intra_only && !hdr.frame_parallel_decoding && !hdr.error_resilient);

        state
            .begin_frame(&mut hdr)
            .expect("reset_frame_context 2 initialises");
        assert!(
            state.fc == FrameProbs::defaults(),
            "this frame resets and loads context 0, so it starts from the defaults"
        );

        super::super::decode_intra_frame_with_state(&mut state, &hdr, INTRA_ONLY_352X288)
            .expect("intra-only frame decodes");

        let after_forward_updates = state.fc.clone();
        assert!(
            after_forward_updates != FrameProbs::defaults(),
            "the compressed header moved probabilities — which is exactly why \
             'not the defaults' cannot be the proof that adaptation ran"
        );
        assert!(
            state.counts.total_coef_tokens() > 0 && state.counts.total_eob_branches() > 0,
            "the frame coded coefficients, so there is something to adapt from"
        );

        state.adapt_probabilities(&hdr);

        assert!(
            state.fc != after_forward_updates,
            "backward adaptation must move the context away from what the \
             compressed header alone left behind"
        );
        assert_eq!(
            state.fc.skip, after_forward_updates.skip,
            "skip is adapted by vp9_adapt_mode_probs, which libvpx skips for \
             intra frames, so it must be untouched here"
        );
        assert_eq!(
            (state.fc.tx8, state.fc.tx16, state.fc.tx32),
            (
                after_forward_updates.tx8,
                after_forward_updates.tx16,
                after_forward_updates.tx32
            ),
            "nor may the tx probabilities move on an intra frame"
        );

        // ...so the coefficient probabilities are the only thing that moved,
        // and many of them did. (Individual nodes the frame never coded stay
        // put: `merge_probs(pre, {0, 0}, ..)` returns the prior, so a
        // node-by-node assertion would be a lottery on frame content.)
        let moved = state
            .fc
            .coef
            .iter()
            .flatten()
            .flatten()
            .flatten()
            .flatten()
            .zip(
                after_forward_updates
                    .coef
                    .iter()
                    .flatten()
                    .flatten()
                    .flatten()
                    .flatten(),
            )
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            moved > 0,
            "adaptation must have moved at least one coefficient node"
        );
    }

    // -- show_existing_frame ------------------------------------------------

    #[test]
    fn finish_frame_refuses_a_show_existing_frame_header() {
        let mut state = warm_state();
        let slot = match state_after_inter_frame().prev_frame {
            Some(slot) => slot,
            None => panic!("fixture builds a slot"),
        };
        let hdr = UncompressedHeader {
            show_existing_frame: true,
            frame_to_show: 3,
            // Spec-forced value; the parser sets it (see uncompressed.rs).
            show_frame: true,
            ..UncompressedHeader::default()
        };

        match state.finish_frame(&hdr, slot) {
            Err(CodecError::InvalidParameter(msg)) => {
                assert!(msg.contains("show_existing_frame"), "{msg}");
            }
            other => panic!("expected InvalidParameter, got {other:?}"),
        }
        assert!(
            state.prev_frame.is_none(),
            "nothing may have been written before the refusal"
        );
        assert!(state.dpb.iter().all(Option::is_none));
    }
}
