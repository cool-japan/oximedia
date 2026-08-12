//! VP8 **inter-frame** decode driver — RFC 6386 §9.7-§9.8, §16-§18.
//!
//! This module is the integration layer that turns the key-frame pipeline in
//! [`super`] plus the five inter-frame building blocks
//! ([`super::header`]/[`super::state`], [`super::mv`], [`super::mode`],
//! [`super::mc`], [`super::refs`]) into a decoder for a whole VP8 *sequence*:
//! a key frame followed by inter frames that predict from the last / golden /
//! altref reference buffers.
//!
//! # Why a sequence object and not a free function
//!
//! A VP8 inter frame is not self-contained. It inherits, from the frames
//! before it:
//!
//! - the entropy probabilities (RFC 6386 §9.9-§9.10, §17.2),
//! - the loop-filter ref/mode deltas and the segmentation feature data and
//!   per-macroblock segment map (§9.3-§9.4, §10),
//! - the three reference surfaces it may predict from (§9.7-§9.8).
//!
//! [`Vp8SequenceDecoder`] owns exactly that state and nothing else; each call
//! to [`Vp8SequenceDecoder::decode_frame`] advances it by one coded frame.
//! The stateless [`super::decode_keyframe`] entry point is left untouched for
//! the still-image (WebP) caller, for which a "sequence" is always one key
//! frame.
//!
//! # Per-frame order of operations
//!
//! Transcribed from dixie.c `vp8_dixie_decode_frame` (rfc6386.txt lines
//! 8110-8270), which is not gated on frame type except where noted:
//!
//! 1. **Key frame only**: reset all persistent entropy tables to the RFC
//!    defaults (lines 8144-8157 — "These get updated on keyframes regardless
//!    of the refresh_entropy setting"; RFC 6386 §9.9 lines 6814-6819).
//! 2. Where `refresh_entropy_probs` is read — **both** frame types code it
//!    (§19.2, lines 6822 and 6848) — snapshot the entropy context if the bit
//!    is 0, *before* this frame's own probability updates are applied (lines
//!    8159-8163).
//! 3. Decode the frame (modes, tokens, prediction, reconstruction, loop
//!    filter).
//! 4. Restore the snapshot if `refresh_entropy_probs == 0` (lines 8205-8209).
//! 5. Apply the reference-buffer update: altref copy, golden copy, then the
//!    three refreshes, in that order (lines 8211-8266 —
//!    [`Dpb::commit`] implements it).
//!
//! # The motion-vector axis trap
//!
//! [`super::mode`] (and the bitstream, §17 line 6031) orders a vector
//! `(row, col)`; [`super::mc`] (and dixie's `union mv`, line 8634) orders it
//! `(x, y)` — i.e. `(col, row)`. Both are `(i16, i16)`, so a mix-up compiles
//! silently and only shows up as wrong pixels. Exactly one conversion exists,
//! [`to_mc`], and it is the only place either representation is reordered.

#![forbid(unsafe_code)]

use std::sync::Arc;

use super::header::Vp8Header;
use super::mc::{predict_inter_mb, ReconFilter};
use super::mode::{read_all_mb_mode_records, InterMbInfo, MbMode, RefFrame};
use super::mv;
use super::refs::{Dpb, RefSlot, RefSurface};
use super::state::{LoopFilterDeltas, SegmentState, Vp8State};
use super::tables::B_PRED;
use super::{DecodedImage, Decoder, MbInfo};
use crate::error::{CodecError, CodecResult};

/// One decoded frame and the frame-tag facts a caller needs to place it.
pub(crate) struct DecodedFrame {
    /// The reconstructed, cropped YUV 4:2:0 planes.
    pub image: DecodedImage,
    /// `true` when this was a key frame (RFC 6386 §9.1 `frame_type`).
    pub is_keyframe: bool,
    /// The frame tag's `show_frame` bit (RFC 6386 §9.1). A `false` frame is
    /// fully decoded and updates the reference buffers, but is **not** part
    /// of the displayed sequence — the `altref` frames a two-pass encoder
    /// hides. Callers must not emit it.
    pub show_frame: bool,
}

/// A VP8 decoder for a whole sequence: key frames plus the inter frames that
/// reference them.
///
/// See the module documentation for what is carried between frames and in
/// what order each frame updates it.
pub(crate) struct Vp8SequenceDecoder {
    /// Persistent entropy / segmentation / loop-filter-delta state.
    state: Vp8State,
    /// The three reference surfaces (last, golden, altref).
    dpb: Dpb,
    /// Frame dimensions from the most recent key frame. Inter frames carry
    /// no dimensions of their own (RFC 6386 §9.1: the start-code and size
    /// block is `if (key_frame)`), so this is what they decode against.
    dims: Option<(u32, u32)>,
}

impl std::fmt::Debug for Vp8SequenceDecoder {
    /// Summarises the sequence state without dumping reference pixels: the
    /// three surfaces are whole frames, so a derived `Debug` would print
    /// megabytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vp8SequenceDecoder")
            .field("dims", &self.dims)
            .field("has_last", &self.dpb.last.is_some())
            .field("has_golden", &self.dpb.golden.is_some())
            .field("has_altref", &self.dpb.altref.is_some())
            .finish_non_exhaustive()
    }
}

impl Vp8SequenceDecoder {
    /// A decoder positioned before the first frame of a sequence: RFC
    /// default entropy tables, no reference buffers, no known dimensions.
    pub(crate) fn new() -> Self {
        Self {
            state: Vp8State::new(),
            dpb: Dpb::new(),
            dims: None,
        }
    }

    /// Drops every cross-frame dependency, as after a seek to a point whose
    /// reference frames were never decoded.
    ///
    /// The next frame must be a key frame; an inter frame arriving first is
    /// rejected honestly ([`CodecError::InvalidBitstream`]) rather than
    /// predicted from a stale reference surface.
    pub(crate) fn reset(&mut self) {
        self.state = Vp8State::new();
        self.dpb = Dpb::new();
        self.dims = None;
    }

    /// Decodes one coded VP8 frame payload, advancing all cross-frame state.
    ///
    /// # Errors
    /// Fails on a malformed header or bitstream, on an inter frame that
    /// arrives before any key frame, on a reference to a buffer the sequence
    /// never produced, or on a motion vector pointing outside the reference
    /// surface's border.
    pub(crate) fn decode_frame(&mut self, data: &[u8]) -> CodecResult<DecodedFrame> {
        // The 3-byte uncompressed frame tag (RFC 6386 §9.1, rfc6386.txt
        // lines 1665-1689): bit 0 is the frame type, bit 4 `show_frame`.
        // Read here rather than taken from `Vp8Header`, because the routing
        // decision (which parser to run) precedes the parse.
        let tag0 = *data
            .first()
            .ok_or_else(|| CodecError::InvalidBitstream("VP8: empty frame payload".to_string()))?;
        let is_keyframe = (tag0 & 1) == 0;
        let show_frame = ((tag0 >> 4) & 1) != 0;

        if is_keyframe {
            self.decode_key_frame(data, show_frame)
        } else {
            self.decode_inter_frame(data, show_frame)
        }
    }

    /// Decodes a key frame, threading the persistent state the *next* frames
    /// inherit (see the module doc's order of operations).
    fn decode_key_frame(&mut self, data: &[u8], show_frame: bool) -> CodecResult<DecodedFrame> {
        // 1. Key-frame reset of every persistent table (dixie lines
        //    8144-8157; RFC 6386 §9.9 "replaced with their defaults at the
        //    beginning of every key frame").
        self.state.reset_for_keyframe();

        let (header, bd) = Vp8Header::parse(data)?;
        let (mb_cols, mb_rows) = mb_dims(header.width, header.height);
        self.state.ensure_segment_map_size(mb_cols * mb_rows);

        // 2. Snapshot before this frame's own updates. `Vp8Header::parse`
        //    has already applied them to its own `coeff_probs`, so the
        //    snapshot source is the just-reset context — which is exactly
        //    what dixie snapshots at this point.
        if !header.inter.refresh_entropy_probs {
            self.state.saved_entropy = Some(self.state.entropy.clone());
        }
        self.state.entropy.coeff_probs = header.coeff_probs;

        // 3. Decode. Unchanged key-frame pipeline.
        let mut decoder = Decoder::new(&header, bd, data)?;
        decoder.decode_all();
        let image = decoder.crop_image();
        let super::Decoder {
            planes, mb_info, ..
        } = decoder;

        self.seed_state_from_keyframe(&header, &mb_info);
        self.commit_frame(planes, &header, true, mb_cols, mb_rows)?;

        // 4. Restore (no-op unless a snapshot is pending).
        if !header.inter.refresh_entropy_probs {
            self.state.end_of_frame_entropy_restore();
        }

        self.dims = Some((header.width, header.height));
        Ok(DecodedFrame {
            image,
            is_keyframe: true,
            show_frame,
        })
    }

    /// Decodes an inter frame against the current reference buffers.
    fn decode_inter_frame(&mut self, data: &[u8], show_frame: bool) -> CodecResult<DecodedFrame> {
        let (width, height) = self.dims.ok_or_else(|| {
            CodecError::InvalidBitstream(
                "VP8: inter frame before any key frame (no reference surfaces)".to_string(),
            )
        })?;

        // Header parse also performs the `refresh_entropy_probs == 0`
        // snapshot internally, at the point the bit is read (see
        // `parse_interframe`), and applies this frame's probability updates
        // to the persistent context.
        let (header, mut bd) = Vp8Header::parse_interframe(data, width, height, &mut self.state)?;
        let filter = ReconFilter::from_version(header.version)?;
        let (mb_cols, mb_rows) = mb_dims(width, height);

        // The rest of the first partition is the per-macroblock prediction
        // record stream (RFC 6386 §16). `read_mv` is injected: `mv::read_mv`
        // returns a struct, `mode` wants the `(row, col)` tuple.
        let records = read_all_mb_mode_records(
            &mut bd,
            &header,
            &mut self.state,
            mb_cols,
            mb_rows,
            &mut |bd, probs| mv::read_mv(bd, probs).map(|m| (m.row, m.col)),
        )?;

        let mut decoder = Decoder::new(&header, bd, data)?;
        decoder.decode_all_inter(&records, &self.dpb, filter)?;
        let image = decoder.crop_image();
        let super::Decoder { planes, .. } = decoder;

        self.commit_frame(planes, &header, false, mb_cols, mb_rows)?;

        if !header.inter.refresh_entropy_probs {
            self.state.end_of_frame_entropy_restore();
        }

        Ok(DecodedFrame {
            image,
            is_keyframe: false,
            show_frame,
        })
    }

    /// Publishes the reconstructed frame into the decoded-picture buffer
    /// (RFC 6386 §9.7-§9.8; [`Dpb::commit`] owns the ordering rules).
    fn commit_frame(
        &mut self,
        planes: super::Planes,
        header: &Vp8Header,
        is_keyframe: bool,
        mb_cols: usize,
        mb_rows: usize,
    ) -> CodecResult<()> {
        let surface = RefSurface::from_planes_owned(
            planes,
            header.width as usize,
            header.height as usize,
            mb_cols,
            mb_rows,
        )?;
        let inter = &header.inter;
        self.dpb.commit(
            Arc::new(surface),
            is_keyframe,
            inter.refresh_last,
            inter.refresh_golden,
            inter.refresh_alt,
            inter.copy_to_golden,
            inter.copy_to_alt,
        )
    }

    /// Copies the parts of a decoded **key** frame that later inter frames
    /// inherit into the persistent [`Vp8State`].
    ///
    /// dixie parses straight into its persistent `segment_hdr` /
    /// `loopfilter_hdr` structs, so for it this step does not exist. This
    /// decoder's key-frame parser predates the persistent state and builds a
    /// per-frame [`Vp8Header`] instead, so the same values are mirrored back
    /// here. Without it a following inter frame that transmits no update —
    /// `mb_lf_adjustments` with `delta_update == 0`, which is *every* inter
    /// frame of the `p5basic` / `splitmv` / `refswap` fixtures — would
    /// inherit zeros instead of the key frame's deltas and mis-filter every
    /// macroblock (RFC 6386 §9.4; dixie `decode_loopfilter_header`, lines
    /// 7900-7922).
    ///
    /// The segment map is mirrored for the same reason (RFC 6386 §9.3/§10:
    /// an inter frame with `update_mb_segmentation_map == 0` keeps the
    /// previous frame's per-macroblock ids).
    fn seed_state_from_keyframe(&mut self, header: &Vp8Header, mb_info: &[MbInfo]) {
        let lf = &header.loop_filter;
        self.state.lf_deltas = LoopFilterDeltas {
            ref_deltas: lf.ref_deltas.map(narrow_delta),
            mode_deltas: lf.mode_deltas.map(narrow_delta),
            enabled: lf.delta_enabled,
        };
        let seg = &header.segment;
        self.state.segment = SegmentState {
            enabled: seg.enabled,
            abs_delta: seg.abs_delta,
            quant_deltas: seg.quantizer.map(narrow_delta),
            lf_deltas: seg.filter_strength.map(narrow_delta),
            tree_probs: seg.tree_probs,
        };
        for (slot, info) in self.state.segment_map.iter_mut().zip(mb_info) {
            *slot = info.segment_id;
        }
    }
}

impl Decoder<'_> {
    /// Decodes and reconstructs every macroblock of an inter frame, then runs
    /// the loop filter over the finished frame.
    ///
    /// `records` are the prediction records [`read_all_mb_mode_records`]
    /// already decoded from the first partition, in raster order.
    ///
    /// # Errors
    /// Propagates a missing reference buffer, an out-of-border motion vector
    /// or an inconsistent record set.
    pub(super) fn decode_all_inter(
        &mut self,
        records: &[InterMbInfo],
        dpb: &Dpb,
        filter: ReconFilter,
    ) -> CodecResult<()> {
        if records.len() != self.mb_cols * self.mb_rows {
            return Err(CodecError::Internal(format!(
                "VP8: {} prediction records for {}x{} macroblocks",
                records.len(),
                self.mb_cols,
                self.mb_rows
            )));
        }
        for mb_y in 0..self.mb_rows {
            // "Left" non-zero context resets each row: 4 Y + 2 U + 2 V + 1 Y2.
            let mut left_nz = [false; 9];
            for mb_x in 0..self.mb_cols {
                let mb_idx = mb_y * self.mb_cols + mb_x;
                let info = records.get(mb_idx).ok_or_else(|| {
                    CodecError::Internal("VP8: prediction record index out of range".to_string())
                })?;
                self.decode_inter_macroblock(mb_x, mb_y, info, dpb, filter, &mut left_nz)?;
            }
        }
        self.apply_loop_filter();
        Ok(())
    }

    /// Borrows the reference surface a macroblock predicts from, checking
    /// that it really is a picture of this frame's size.
    ///
    /// VP8 dimensions can only change on a key frame, and a key frame fills
    /// all three slots (RFC 6386 §9.7-§9.8), so a well-formed sequence never
    /// trips this. It is checked anyway because the failure it guards
    /// against is silent: motion compensation validates reads against the
    /// *macroblock-aligned* extent plus border, and two different visible
    /// sizes can share one macroblock grid (96x64 and 96x60 do), so a
    /// mismatched surface would predict plausible-looking pixels from the
    /// wrong picture instead of erroring.
    ///
    /// # Errors
    /// Propagates [`Dpb::get`]'s honest "never decoded" error, or reports
    /// the size mismatch.
    fn checked_reference<'d>(
        &self,
        dpb: &'d Dpb,
        reference: RefFrame,
    ) -> CodecResult<&'d RefSurface> {
        let slot = ref_slot(reference)?;
        let surface = dpb.get(slot)?;
        let (want_w, want_h) = (self.header.width as usize, self.header.height as usize);
        if surface.width != want_w || surface.height != want_h {
            return Err(CodecError::InvalidBitstream(format!(
                "VP8: {slot:?} reference is {}x{}, but this frame is {want_w}x{want_h}",
                surface.width, surface.height
            )));
        }
        Ok(surface)
    }

    /// Decodes the residual of one inter-frame macroblock and reconstructs
    /// it, predicting either from a reference surface (§18) or intra from
    /// this frame's own neighbours (§16.1).
    fn decode_inter_macroblock(
        &mut self,
        mb_x: usize,
        mb_y: usize,
        info: &InterMbInfo,
        dpb: &Dpb,
        filter: ReconFilter,
        left_nz: &mut [bool; 9],
    ) -> CodecResult<()> {
        let mb_idx = mb_y * self.mb_cols + mb_x;

        // --- coefficient decode (RFC 6386 §13) --- identical to the
        // key-frame path; only the source of `has_y2` differs (mode.rs
        // supplies it, since inter `SPLITMV` also codes no Y2 block).
        let mut coeffs = [[0i32; 16]; 25];
        let has_y2 = info.has_y2;
        let mut any_tokens = false;
        if info.skip_coeff {
            // dixie `reset_mb_context` (rfc6386.txt lines 13350-13365): the
            // Y2 context is preserved for the modes that code no Y2 block.
            for c in 0..8 {
                left_nz[c] = false;
                self.above_nz[mb_x * 9 + c] = false;
            }
            if has_y2 {
                left_nz[8] = false;
                self.above_nz[mb_x * 9 + 8] = false;
            }
        } else {
            let dq = self.dequant[usize::from(info.segment_id).min(self.dequant.len() - 1)];
            let part = mb_y % self.token_bd.len();
            any_tokens = self.decode_residuals(mb_x, has_y2, &dq, &mut coeffs, part, left_nz);
        }

        // --- prediction + reconstruction ---
        let mode_delta_slot = match info.mode {
            MbMode::Intra {
                ymode,
                bmodes,
                uv_mode,
            } => {
                let ymode = usize::from(ymode);
                if ymode == B_PRED {
                    // §16.1: an inter frame's B_PRED sub-modes are read with
                    // fixed (context-free) probabilities, but the *pixels*
                    // are predicted exactly as in a key frame.
                    let bmodes = bmodes.ok_or_else(|| {
                        CodecError::Internal(
                            "VP8: B_PRED macroblock without sub-block modes".to_string(),
                        )
                    })?;
                    self.reconstruct_bpred(mb_x, mb_y, &bmodes, &coeffs);
                } else {
                    self.reconstruct_y16(mb_x, mb_y, ymode, has_y2, &mut coeffs);
                }
                self.reconstruct_chroma(mb_x, mb_y, usize::from(uv_mode), &coeffs);
                // Only B_PRED takes an intra mode delta (dixie lines
                // 9194-9198).
                if ymode == B_PRED {
                    Some(0)
                } else {
                    None
                }
            }
            MbMode::Inter { mode, .. } => {
                let reference = self.checked_reference(dpb, info.ref_frame)?;
                let luma_mvs = to_mc_all(&info.sub_mvs);
                // `Some` for the whole-macroblock modes, `None` for SPLITMV.
                let whole_mb = if info.is_split() {
                    None
                } else {
                    Some(to_mc(info.mv()))
                };
                predict_inter_mb(
                    &mut self.planes,
                    reference,
                    mb_x,
                    mb_y,
                    filter,
                    &luma_mvs,
                    whole_mb,
                )?;
                self.add_luma_residual(mb_x, mb_y, has_y2, &mut coeffs);
                self.add_chroma_residual(mb_x, mb_y, &coeffs);
                Some(mode.lf_mode_delta_slot())
            }
        };

        // --- record loop-filter info (RFC 6386 §15.2) ---
        let filter_level = self.compute_mb_filter_level(
            info.segment_id,
            info.ref_frame.lf_ref_delta_slot(),
            mode_delta_slot,
        );
        self.mb_info[mb_idx] = MbInfo {
            // dixie lines 9433-9435: `eob_mask || SPLITMV || B_PRED`. The
            // two mode tests are exactly `!has_y2`: RFC 6386 §13.1 gives a
            // Y2 block to every mode *except* intra B_PRED and inter
            // SPLITMV, which is the same pair.
            filter_inner: any_tokens || !has_y2,
            filter_level,
            segment_id: info.segment_id,
        };
        Ok(())
    }
}

/// Macroblock dimensions of a `width` x `height` frame.
fn mb_dims(width: u32, height: u32) -> (usize, usize) {
    (
        (width as usize).div_ceil(16),
        (height as usize).div_ceil(16),
    )
}

/// Maps the bitstream's reference-frame selector onto a storage slot.
///
/// # Errors
/// [`CodecError::Internal`] for [`RefFrame::Intra`], which has no reference
/// surface — the caller must not reach here for an intra macroblock.
fn ref_slot(reference: RefFrame) -> CodecResult<RefSlot> {
    match reference {
        RefFrame::Last => Ok(RefSlot::Last),
        RefFrame::Golden => Ok(RefSlot::Golden),
        RefFrame::AltRef => Ok(RefSlot::AltRef),
        RefFrame::Intra => Err(CodecError::Internal(
            "VP8: intra macroblock has no reference surface".to_string(),
        )),
    }
}

/// Converts a `mode`-module motion vector — `(row, col)`, the bitstream's
/// own order (RFC 6386 §17 line 6031) — into an `mc`-module one, `(x, y)`
/// == `(col, row)` (dixie `union mv`, line 8634).
///
/// The single axis-swap of this decoder; see the module doc.
const fn to_mc(mv: super::mode::Mv) -> super::mc::Mv {
    (mv.1, mv.0)
}

/// [`to_mc`] over a macroblock's sixteen sub-block vectors.
fn to_mc_all(mvs: &[super::mode::Mv; 16]) -> [super::mc::Mv; 16] {
    let mut out = [(0i16, 0i16); 16];
    for (dst, src) in out.iter_mut().zip(mvs) {
        *dst = to_mc(*src);
    }
    out
}

/// Narrows a header delta (a signed 6- or 7-bit magnitude, RFC 6386
/// §9.3-§9.4) into the persistent state's `i8` storage.
///
/// Saturating rather than wrapping: the range is `-127..=127` by
/// construction, so this never bites for any bitstream the parser accepts,
/// and a future widening would clamp visibly instead of flipping sign.
fn narrow_delta(v: i32) -> i8 {
    v.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8
}

#[cfg(test)]
#[path = "inter_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "inter_fixture_tests.rs"]
mod fixture_tests;
