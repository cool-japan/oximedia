//! VP9 reference-frame slots and the decoded-picture buffer (VP9 bitstream
//! spec §6.2 `refresh_frame_flags`, §8.10; libvpx `vp9/common/vp9_onyxc_int.h`
//! `RefCntBuffer` / `cm->ref_frame_map`, `vp9/common/vp9_pred_common.c`).
//!
//! A VP9 stream carries eight reference slots. Every decoded frame is written
//! into the slots selected by its `refresh_frame_flags` bitmask, and an inter
//! frame predicts from up to three of them (LAST / GOLDEN / ALTREF, chosen by
//! `ref_frame_idx[0..3]`).
//!
//! # What a slot has to keep
//!
//! * **Pixels** ([`Vp9RefSlot::planes`]) — MI-aligned, *not* cropped to the
//!   display size. VP9 reconstructs whole 8x8 mode-info units, and motion
//!   compensation legitimately reads the alignment padding, so cropping on
//!   the way into the DPB would corrupt prediction near the right/bottom
//!   edges. The crop happens only on the way *out*, to the displayed
//!   `VideoFrame`.
//! * **Motion vectors** ([`Vp9RefSlot::mvs`]) — one [`MvRefRow`] per mode-info
//!   unit, libvpx's `MV_REF` array (`cm->cur_frame->mvs`, written by
//!   `vp9_read_mode_info` for inter frames). The *next* frame reads these as
//!   temporal motion-vector candidates when
//!   [`super::state::Vp9DecState::use_prev_frame_mvs`] holds.
//! * **Geometry and frame flags** — the dimensions a later
//!   `frame_size_with_refs` copies from (`y_crop_width` / `y_crop_height` in
//!   libvpx `setup_frame_size_with_refs`), plus the MI grid size the MV array
//!   is indexed with.
//!
//! # Compound reference derivation
//!
//! [`is_compound_reference_allowed`] and [`setup_compound_reference_mode`] are
//! straight ports of `vp9_compound_reference_allowed` and
//! `vp9_setup_compound_reference_mode` (libvpx `vp9_pred_common.c:16-40`):
//! whether a frame may use two references at once, and which reference is the
//! fixed one, follow purely from the three per-frame sign-bias bits.

#![forbid(unsafe_code)]

use super::recon::{DecodedFrame, PlaneBuf};
use crate::error::{CodecError, CodecResult};
use crate::vp9::mv::MotionVector;

/// Number of decoded-picture-buffer slots (libvpx `REF_FRAMES`).
pub const REF_FRAMES: usize = 8;

/// Number of references a single inter frame may name (libvpx
/// `REFS_PER_FRAME`): LAST, GOLDEN, ALTREF.
pub const REFS_PER_FRAME: usize = 3;

/// libvpx `MV_REFERENCE_FRAME` value `NONE`: no reference (second reference
/// of a single-reference block).
pub const NONE_FRAME: i8 = -1;
/// libvpx `INTRA_FRAME`: the block is intra-coded.
pub const INTRA_FRAME: i8 = 0;
/// libvpx `LAST_FRAME`.
pub const LAST_FRAME: i8 = 1;
/// libvpx `GOLDEN_FRAME`.
pub const GOLDEN_FRAME: i8 = 2;
/// libvpx `ALTREF_FRAME`.
pub const ALTREF_FRAME: i8 = 3;

/// One mode-info unit's stored reference frames and motion vectors — libvpx's
/// `MV_REF` (`typedef struct { MV_REFERENCE_FRAME ref_frame[2]; int_mv
/// mv[2]; } MV_REF;`).
///
/// Despite the name this is a single 8x8 MI *entry*, not a row: a frame's
/// array is indexed `mvs[mi_row * mi_cols + mi_col]`, matching libvpx's
/// `cm->cur_frame->mvs + mi_row * cm->mi_cols + mi_col`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MvRefRow {
    /// Reference frames of the block that covered this MI
    /// ([`NONE_FRAME`], [`INTRA_FRAME`], [`LAST_FRAME`], [`GOLDEN_FRAME`],
    /// [`ALTREF_FRAME`]).
    pub ref_frame: [i8; 2],
    /// The matching motion vectors, 1/8-pel units.
    pub mv: [MotionVector; 2],
}

impl Default for MvRefRow {
    /// `[INTRA_FRAME, NONE_FRAME]` with zero motion — what an intra-coded MI
    /// carries (libvpx `read_intra_frame_mode_info`: `mi->ref_frame[0] =
    /// INTRA_FRAME; mi->ref_frame[1] = NONE;`).
    ///
    /// libvpx's own freshly-allocated MV array is `vpx_calloc`'d, i.e.
    /// `[INTRA_FRAME, INTRA_FRAME]`. The difference is inert: temporal
    /// candidate matching only ever compares against LAST/GOLDEN/ALTREF
    /// (`prev_frame_mvs->ref_frame[0] == ref_frame`, and `ref_frame[1] >
    /// INTRA_FRAME` for the second slot, `vp9_mvref_common.c`), and both `0`
    /// and `-1` fail every one of those tests.
    fn default() -> Self {
        Self {
            ref_frame: [INTRA_FRAME, NONE_FRAME],
            mv: [MotionVector::zero(); 2],
        }
    }
}

/// One decoded-picture-buffer slot: a decoded frame kept for later
/// prediction.
pub struct Vp9RefSlot {
    /// Y, U, V reconstruction planes, **MI-aligned** (not cropped to
    /// `width`/`height`).
    pub planes: [PlaneBuf; 3],
    /// Per-MI reference frames and motion vectors, `mi_rows * mi_cols`
    /// entries, row-major (libvpx `RefCntBuffer::mvs`).
    pub mvs: Vec<MvRefRow>,
    /// MI grid height (`(height + 7) >> 3`), libvpx `RefCntBuffer::mi_rows`.
    pub mi_rows: usize,
    /// MI grid width (`(width + 7) >> 3`), libvpx `RefCntBuffer::mi_cols`.
    pub mi_cols: usize,
    /// Display width in pixels (libvpx `y_crop_width`) — the value a later
    /// frame's `frame_size_with_refs` copies.
    pub width: usize,
    /// Display height in pixels (libvpx `y_crop_height`).
    pub height: usize,
    /// The `intra_only` header bit of the frame stored here (`false` for key
    /// frames, matching libvpx's `cm->intra_only`, which the key-frame branch
    /// of `read_uncompressed_header` never assigns).
    pub intra_only: bool,
    /// The `show_frame` header bit of the frame stored here.
    pub show_frame: bool,
}

impl Vp9RefSlot {
    /// Builds a slot from a decoded frame of either type.
    ///
    /// The motion-vector array comes straight from the decode
    /// ([`DecodedFrame::mvs`]): an inter frame's `vp9_read_mode_info` fills
    /// it per mode-info cell, and an intra frame leaves every entry at
    /// [`MvRefRow::default`] because libvpx's `vp9_read_mode_info` writes
    /// `cm->cur_frame->mvs` on the inter path only. Either way the array is
    /// the frame's real MI size, so a following frame that legitimately reads
    /// temporal candidates finds correctly-sized data rather than a length
    /// mismatch.
    ///
    /// The `mi_rows`/`mi_cols` recorded here are re-derived from the display
    /// dimensions rather than taken from the decode, and
    /// [`MvRefRow`]-indexing (`mv_at`) uses them — a mismatch with the array
    /// the decoder filled would silently mis-address the temporal candidates,
    /// so the length is checked against them.
    ///
    /// # Errors
    ///
    /// [`CodecError::InvalidBitstream`] when the decoded frame's
    /// motion-vector array does not have `mi_rows * mi_cols` entries, which
    /// no output of [`super::recon::decode_frame`] ever does.
    pub fn from_decoded_frame(
        decoded: DecodedFrame,
        intra_only: bool,
        show_frame: bool,
    ) -> CodecResult<Self> {
        let mi_cols = decoded.width.div_ceil(8);
        let mi_rows = decoded.height.div_ceil(8);
        if decoded.mvs.len() != mi_rows * mi_cols {
            return Err(CodecError::InvalidBitstream(format!(
                "VP9: decoded frame carries {} motion-vector records for a \
                 {mi_rows}x{mi_cols} mode-info grid",
                decoded.mvs.len()
            )));
        }
        Ok(Self {
            planes: decoded.planes,
            mvs: decoded.mvs,
            mi_rows,
            mi_cols,
            width: decoded.width,
            height: decoded.height,
            intra_only,
            show_frame,
        })
    }

    /// Stored motion-vector record for one MI position.
    ///
    /// Out-of-range coordinates yield [`MvRefRow::default`] (an intra,
    /// zero-motion record) rather than panicking: a candidate scan that walks
    /// off the grid must contribute no candidate, which is exactly what an
    /// intra record does.
    #[must_use]
    pub fn mv_at(&self, mi_row: usize, mi_col: usize) -> MvRefRow {
        if mi_row >= self.mi_rows || mi_col >= self.mi_cols {
            return MvRefRow::default();
        }
        self.mvs
            .get(mi_row * self.mi_cols + mi_col)
            .copied()
            .unwrap_or_default()
    }

    /// Display dimensions, as `frame_size_with_refs` consumes them.
    #[must_use]
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width as u32, self.height as u32)
    }
}

impl Clone for Vp9RefSlot {
    fn clone(&self) -> Self {
        Self {
            planes: [
                self.planes[0].clone(),
                self.planes[1].clone(),
                self.planes[2].clone(),
            ],
            mvs: self.mvs.clone(),
            mi_rows: self.mi_rows,
            mi_cols: self.mi_cols,
            width: self.width,
            height: self.height,
            intra_only: self.intra_only,
            show_frame: self.show_frame,
        }
    }
}

impl std::fmt::Debug for Vp9RefSlot {
    /// Summarises geometry instead of dumping the pixel and MV buffers.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vp9RefSlot")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("mi_rows", &self.mi_rows)
            .field("mi_cols", &self.mi_cols)
            .field("intra_only", &self.intra_only)
            .field("show_frame", &self.show_frame)
            .field("mvs_len", &self.mvs.len())
            .field(
                "plane_bytes",
                &[
                    self.planes[0].data.len(),
                    self.planes[1].data.len(),
                    self.planes[2].data.len(),
                ],
            )
            .finish()
    }
}

/// Writes `frame` into every decoded-picture-buffer slot selected by
/// `refresh_frame_flags` (VP9 spec §8.10 `reference frame update process`;
/// libvpx `read_uncompressed_header`'s `next_ref_frame_map` loop followed by
/// `swap_frame_buffers`).
///
/// Bit `i` of the mask refreshes slot `i`; a key frame's mask is `0xFF`, so
/// every slot ends up holding it. Slots whose bit is clear keep whatever they
/// held.
///
/// libvpx reference-counts one buffer into several slots; this stores an
/// independent copy per slot, matching what the pre-existing `VideoFrame`
/// decoded-picture buffer in `vp9/decoder.rs` already did. Switching the
/// slots to a shared handle is a pure optimisation and changes no output.
pub fn refresh_dpb(dpb: &mut [Option<Vp9RefSlot>; REF_FRAMES], flags: u8, frame: &Vp9RefSlot) {
    for (i, slot) in dpb.iter_mut().enumerate() {
        if flags & (1 << i) != 0 {
            *slot = Some(frame.clone());
        }
    }
}

/// Whether the frame may code compound (two-reference) prediction — verbatim
/// `vp9_compound_reference_allowed` (libvpx `vp9_pred_common.c:16-22`):
///
/// ```c
/// int vp9_compound_reference_allowed(const VP9_COMMON *cm) {
///   int i;
///   for (i = 1; i < REFS_PER_FRAME; ++i)
///     if (cm->ref_frame_sign_bias[i + 1] != cm->ref_frame_sign_bias[1]) return 1;
///   return 0;
/// }
/// ```
///
/// i.e. compound prediction is available exactly when the three references do
/// not all share one sign bias — one of them must point the other way in time
/// for a forward/backward pair to exist.
///
/// `sign_bias` is indexed by reference frame, so entry `0` ([`INTRA_FRAME`])
/// is unused and entries `1..=3` are LAST / GOLDEN / ALTREF.
#[must_use]
pub fn is_compound_reference_allowed(sign_bias: &[bool; 4]) -> bool {
    let last = sign_bias[LAST_FRAME as usize];
    (2..=REFS_PER_FRAME).any(|i| sign_bias[i] != last)
}

/// Derives the compound reference pairing — verbatim
/// `vp9_setup_compound_reference_mode` (libvpx `vp9_pred_common.c:24-40`).
///
/// Returns `(comp_fixed_ref, comp_var_ref)`: the reference that is always one
/// half of a compound pair, and the two that the per-block
/// `comp_ref` bit selects between.
///
/// The odd reference out — the one whose sign bias differs — becomes the
/// fixed reference; when all three agree the `else` branch is taken, which is
/// why callers must gate on [`is_compound_reference_allowed`] first (libvpx
/// only calls this from `read_compressed_header` when the frame actually
/// codes a non-single reference mode).
#[must_use]
pub fn setup_compound_reference_mode(sign_bias: &[bool; 4]) -> (i8, [i8; 2]) {
    if sign_bias[LAST_FRAME as usize] == sign_bias[GOLDEN_FRAME as usize] {
        (ALTREF_FRAME, [LAST_FRAME, GOLDEN_FRAME])
    } else if sign_bias[LAST_FRAME as usize] == sign_bias[ALTREF_FRAME as usize] {
        (GOLDEN_FRAME, [LAST_FRAME, ALTREF_FRAME])
    } else {
        (LAST_FRAME, [GOLDEN_FRAME, ALTREF_FRAME])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a slot with recognisable geometry and a marker pixel/MV, cheap
    /// enough to instantiate in every test.
    fn slot(width: usize, height: usize, marker: u8) -> Vp9RefSlot {
        let mi_cols = width.div_ceil(8);
        let mi_rows = height.div_ceil(8);
        let aligned_w = mi_cols * 8;
        let aligned_h = mi_rows * 8;
        let plane = |w: usize, h: usize| PlaneBuf {
            data: vec![marker; w * h],
            stride: w,
            width: w,
            height: h,
        };
        let mut mvs = vec![MvRefRow::default(); mi_rows * mi_cols];
        mvs[0] = MvRefRow {
            ref_frame: [LAST_FRAME, NONE_FRAME],
            mv: [
                MotionVector::new(i16::from(marker), -4),
                MotionVector::zero(),
            ],
        };
        Vp9RefSlot {
            planes: [
                plane(aligned_w, aligned_h),
                plane(aligned_w.div_ceil(2), aligned_h.div_ceil(2)),
                plane(aligned_w.div_ceil(2), aligned_h.div_ceil(2)),
            ],
            mvs,
            mi_rows,
            mi_cols,
            width,
            height,
            intra_only: false,
            show_frame: true,
        }
    }

    // -- refresh_dpb ------------------------------------------------------

    #[test]
    fn refresh_dpb_keyframe_mask_fills_every_slot() {
        let mut dpb: [Option<Vp9RefSlot>; REF_FRAMES] = Default::default();
        refresh_dpb(&mut dpb, 0xFF, &slot(76, 42, 9));
        for (i, s) in dpb.iter().enumerate() {
            let s = s.as_ref().unwrap_or_else(|| panic!("slot {i} filled"));
            assert_eq!(s.dimensions(), (76, 42));
            assert_eq!(s.planes[0].data[0], 9);
        }
    }

    #[test]
    fn refresh_dpb_zero_mask_changes_nothing() {
        let mut dpb: [Option<Vp9RefSlot>; REF_FRAMES] = Default::default();
        dpb[3] = Some(slot(64, 64, 1));
        refresh_dpb(&mut dpb, 0x00, &slot(32, 32, 2));
        assert!(dpb.iter().enumerate().all(|(i, s)| s.is_some() == (i == 3)));
        assert_eq!(dpb[3].as_ref().map(Vp9RefSlot::dimensions), Some((64, 64)));
    }

    #[test]
    fn refresh_dpb_writes_only_flagged_slots_and_overwrites_them() {
        let mut dpb: [Option<Vp9RefSlot>; REF_FRAMES] = Default::default();
        dpb[0] = Some(slot(64, 64, 1));
        dpb[5] = Some(slot(64, 64, 1));

        // bits 0, 2 and 7 -> slots 0 (overwrite), 2 (fill), 7 (fill).
        refresh_dpb(&mut dpb, 0b1000_0101, &slot(128, 96, 2));

        for i in [0usize, 2, 7] {
            let s = dpb[i]
                .as_ref()
                .unwrap_or_else(|| panic!("slot {i} written"));
            assert_eq!(s.planes[0].data[0], 2, "slot {i} holds the new frame");
            assert_eq!(s.dimensions(), (128, 96));
        }
        assert_eq!(dpb[5].as_ref().map(|s| s.planes[0].data[0]), Some(1));
        for i in [1usize, 3, 4, 6] {
            assert!(dpb[i].is_none(), "slot {i} untouched");
        }
    }

    #[test]
    fn refresh_dpb_slots_are_independent_copies() {
        let mut dpb: [Option<Vp9RefSlot>; REF_FRAMES] = Default::default();
        refresh_dpb(&mut dpb, 0b11, &slot(16, 16, 3));
        if let Some(s) = dpb[0].as_mut() {
            s.planes[0].data[0] = 200;
        }
        assert_eq!(dpb[1].as_ref().map(|s| s.planes[0].data[0]), Some(3));
    }

    #[test]
    fn refresh_dpb_preserves_motion_vectors() {
        let mut dpb: [Option<Vp9RefSlot>; REF_FRAMES] = Default::default();
        refresh_dpb(&mut dpb, 1 << 4, &slot(24, 24, 7));
        let s = dpb[4].as_ref().expect("slot 4 filled");
        assert_eq!(
            s.mv_at(0, 0),
            MvRefRow {
                ref_frame: [LAST_FRAME, NONE_FRAME],
                mv: [MotionVector::new(7, -4), MotionVector::zero()],
            }
        );
        assert_eq!(s.mv_at(1, 1), MvRefRow::default());
    }

    // -- geometry ---------------------------------------------------------

    #[test]
    fn mv_at_out_of_range_is_an_inert_intra_record() {
        let s = slot(24, 24, 1);
        assert_eq!(s.mi_rows, 3);
        assert_eq!(s.mi_cols, 3);
        assert_eq!(s.mv_at(3, 0), MvRefRow::default());
        assert_eq!(s.mv_at(0, 3), MvRefRow::default());
        assert_eq!(s.mv_at(usize::MAX, usize::MAX), MvRefRow::default());
    }

    #[test]
    fn mv_default_is_inert_for_candidate_matching() {
        let d = MvRefRow::default();
        for r in [LAST_FRAME, GOLDEN_FRAME, ALTREF_FRAME] {
            assert_ne!(d.ref_frame[0], r);
        }
        assert!(d.ref_frame[1] <= INTRA_FRAME);
    }

    // -- compound reference (vp9_pred_common.c:16-40) ----------------------

    #[test]
    fn compound_not_allowed_when_all_sign_biases_agree() {
        assert!(!is_compound_reference_allowed(&[false; 4]));
        assert!(!is_compound_reference_allowed(&[false, true, true, true]));
        // Entry 0 (INTRA_FRAME) is not part of the comparison.
        assert!(!is_compound_reference_allowed(&[true, false, false, false]));
    }

    #[test]
    fn compound_allowed_when_any_sign_bias_differs() {
        assert!(is_compound_reference_allowed(&[false, false, false, true]));
        assert!(is_compound_reference_allowed(&[false, false, true, false]));
        assert!(is_compound_reference_allowed(&[false, true, false, false]));
    }

    #[test]
    fn compound_mode_altref_fixed_when_last_matches_golden() {
        // The typical single-altref stream: LAST/GOLDEN forward, ALTREF back.
        let (fixed, var) = setup_compound_reference_mode(&[false, false, false, true]);
        assert_eq!(fixed, ALTREF_FRAME);
        assert_eq!(var, [LAST_FRAME, GOLDEN_FRAME]);
    }

    #[test]
    fn compound_mode_golden_fixed_when_last_matches_altref() {
        let (fixed, var) = setup_compound_reference_mode(&[false, false, true, false]);
        assert_eq!(fixed, GOLDEN_FRAME);
        assert_eq!(var, [LAST_FRAME, ALTREF_FRAME]);
    }

    #[test]
    fn compound_mode_last_fixed_when_golden_matches_altref() {
        let (fixed, var) = setup_compound_reference_mode(&[false, false, true, true]);
        assert_eq!(fixed, LAST_FRAME);
        assert_eq!(var, [GOLDEN_FRAME, ALTREF_FRAME]);
    }

    #[test]
    fn compound_mode_covers_every_sign_bias_combination() {
        // All eight LAST/GOLDEN/ALTREF sign-bias combinations must land on a
        // pairing whose fixed reference is exactly the one left out of the
        // variable pair, and whose variable pair is ordered ascending.
        for bits in 0u8..8 {
            let sb = [false, bits & 1 != 0, bits & 2 != 0, bits & 4 != 0];
            let (fixed, var) = setup_compound_reference_mode(&sb);
            assert!(var[0] < var[1], "variable refs ordered for {sb:?}");
            assert!(!var.contains(&fixed), "fixed ref excluded for {sb:?}");
            let mut all = vec![fixed, var[0], var[1]];
            all.sort_unstable();
            assert_eq!(all, vec![LAST_FRAME, GOLDEN_FRAME, ALTREF_FRAME]);
        }
    }

    #[test]
    fn compound_mode_matches_libvpx_branch_order_when_all_agree() {
        // All-equal sign bias hits the first branch (LAST == GOLDEN), the
        // same fall-through libvpx has; callers gate on
        // `is_compound_reference_allowed` first.
        let (fixed, var) = setup_compound_reference_mode(&[false; 4]);
        assert_eq!(fixed, ALTREF_FRAME);
        assert_eq!(var, [LAST_FRAME, GOLDEN_FRAME]);
    }
}
