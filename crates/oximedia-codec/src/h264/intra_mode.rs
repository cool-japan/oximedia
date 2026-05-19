//! Intra-4×4 prediction mode coding helpers.
//!
//! For an `I_NxN` macroblock, each 4×4 luma sub-block carries its own
//! 9-mode intra prediction mode.  The bitstream encodes that mode in
//! a compact way:
//!
//! - A "most probable mode" (MPM) is derived from the modes assigned
//!   to the *top* and *left* 4×4 neighbours.
//! - A single `prev_intra4x4_pred_mode_flag` bit signals whether the
//!   block uses the MPM (flag = 1) or a different mode (flag = 0).
//! - When the flag is 0, a 3-bit `rem_intra4x4_pred_mode` field
//!   selects one of the eight non-MPM modes — values 0..=`MPM - 1`
//!   stay as-is, values >= `MPM` get bumped by 1 to skip the MPM
//!   slot.
//!
//! This module provides the small helpers the orchestrator needs to
//! resolve a `(prev_flag, rem_mode)` pair plus neighbour context into
//! an actual [`Intra4x4Mode`].

use crate::h264::intra_pred::Intra4x4Mode;
use crate::CodecError;

impl Intra4x4Mode {
    /// Maps a spec mode number (0..=8) to the typed enum.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::InvalidData`] for values outside 0..=8.
    pub fn from_u8(raw: u8) -> Result<Self, CodecError> {
        Ok(match raw {
            0 => Self::Vertical,
            1 => Self::Horizontal,
            2 => Self::Dc,
            3 => Self::DiagonalDownLeft,
            4 => Self::DiagonalDownRight,
            5 => Self::VerticalRight,
            6 => Self::HorizontalDown,
            7 => Self::VerticalLeft,
            8 => Self::HorizontalUp,
            _ => {
                return Err(CodecError::InvalidData(format!(
                    "h264 intra_mode: raw mode {raw} out of range 0..=8"
                )))
            }
        })
    }

    /// Maps the typed enum back to the spec mode number.
    #[must_use]
    pub fn to_u8(self) -> u8 {
        match self {
            Self::Vertical => 0,
            Self::Horizontal => 1,
            Self::Dc => 2,
            Self::DiagonalDownLeft => 3,
            Self::DiagonalDownRight => 4,
            Self::VerticalRight => 5,
            Self::HorizontalDown => 6,
            Self::VerticalLeft => 7,
            Self::HorizontalUp => 8,
        }
    }
}

/// Per-macroblock context for intra-4×4 MPM derivation.
///
/// Stores the already-decoded mode for each of the 16 4×4 sub-blocks
/// of the current macroblock plus the modes of the top-edge and
/// left-edge neighbour 4×4 blocks from already-decoded macroblocks.
///
/// Block indexing inside the macroblock is the 4×4-block raster index
/// 0..=15: `(block_x_in_mb, block_y_in_mb) = (idx % 4, idx / 4)`.
#[derive(Debug, Clone, Default)]
pub struct Intra4x4ModeContext {
    /// The 16 sub-block modes inside the current macroblock, in
    /// raster order.  Entries that haven't been decoded yet are
    /// `None`.
    pub current_mb: [Option<Intra4x4Mode>; 16],
    /// Modes of the four 4×4 blocks of the *top* neighbour
    /// macroblock's bottom row, used as top-neighbour context for the
    /// current macroblock's top row.  `None` when that macroblock
    /// isn't an `I_NxN` intra macroblock or doesn't exist.
    pub top_neighbour_bottom_row: [Option<Intra4x4Mode>; 4],
    /// Modes of the four 4×4 blocks of the *left* neighbour
    /// macroblock's right column, used as left-neighbour context for
    /// the current macroblock's left column.  Same `None` semantics.
    pub left_neighbour_right_col: [Option<Intra4x4Mode>; 4],
}

impl Intra4x4ModeContext {
    /// Reads the mode of the top neighbour of block index `idx` in the
    /// current macroblock.
    #[must_use]
    pub fn top_of(&self, idx: usize) -> Option<Intra4x4Mode> {
        let bx = idx % 4;
        let by = idx / 4;
        if by == 0 {
            self.top_neighbour_bottom_row[bx]
        } else {
            self.current_mb[(by - 1) * 4 + bx]
        }
    }

    /// Reads the mode of the left neighbour of block index `idx`.
    #[must_use]
    pub fn left_of(&self, idx: usize) -> Option<Intra4x4Mode> {
        let bx = idx % 4;
        let by = idx / 4;
        if bx == 0 {
            self.left_neighbour_right_col[by]
        } else {
            self.current_mb[by * 4 + (bx - 1)]
        }
    }

    /// Records the just-decoded mode of block index `idx`.
    pub fn set(&mut self, idx: usize, mode: Intra4x4Mode) {
        self.current_mb[idx] = Some(mode);
    }
}

/// Derives the most probable mode for one 4×4 sub-block.
///
/// Rules:
///
/// - If either neighbour is unavailable / non-intra, MPM = `Dc`.
/// - Otherwise MPM is the lower-numbered of the two neighbour modes.
///
/// This matches the H.264 spec's pred prediction-mode derivation
/// for 4×4 intra prediction.
#[must_use]
pub fn most_probable_mode(
    top: Option<Intra4x4Mode>,
    left: Option<Intra4x4Mode>,
) -> Intra4x4Mode {
    match (top, left) {
        (Some(t), Some(l)) => {
            if t.to_u8() < l.to_u8() {
                t
            } else {
                l
            }
        }
        _ => Intra4x4Mode::Dc,
    }
}

/// Resolves the actual intra-4×4 prediction mode for one sub-block
/// from its `prev_intra4x4_pred_mode_flag` bit, the optional
/// `rem_intra4x4_pred_mode` (only present when the flag is 0), and
/// the MPM derived from neighbour context.
///
/// # Errors
///
/// Returns [`CodecError::InvalidData`] when `prev_flag` is `false`
/// and `rem_mode` is `None` or outside `0..=7`.
pub fn resolve_intra4x4_mode(
    prev_flag: bool,
    rem_mode: Option<u8>,
    mpm: Intra4x4Mode,
) -> Result<Intra4x4Mode, CodecError> {
    if prev_flag {
        return Ok(mpm);
    }
    let rem = rem_mode.ok_or_else(|| {
        CodecError::InvalidData(
            "h264 intra_mode: rem_intra4x4_pred_mode missing when prev_flag=0".into(),
        )
    })?;
    if rem > 7 {
        return Err(CodecError::InvalidData(format!(
            "h264 intra_mode: rem_intra4x4_pred_mode {rem} out of range 0..=7"
        )));
    }
    // rem indexes through the 9 modes skipping the MPM slot.
    let actual_raw = if rem < mpm.to_u8() { rem } else { rem + 1 };
    Intra4x4Mode::from_u8(actual_raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_round_trips_through_u8() {
        for raw in 0..=8u8 {
            let mode = Intra4x4Mode::from_u8(raw).unwrap();
            assert_eq!(mode.to_u8(), raw);
        }
    }

    #[test]
    fn mode_from_u8_rejects_out_of_range() {
        assert!(Intra4x4Mode::from_u8(9).is_err());
    }

    #[test]
    fn mpm_falls_back_to_dc_when_either_neighbour_absent() {
        assert_eq!(most_probable_mode(None, None), Intra4x4Mode::Dc);
        assert_eq!(
            most_probable_mode(Some(Intra4x4Mode::Vertical), None),
            Intra4x4Mode::Dc,
        );
        assert_eq!(
            most_probable_mode(None, Some(Intra4x4Mode::Horizontal)),
            Intra4x4Mode::Dc,
        );
    }

    #[test]
    fn mpm_picks_lower_numbered_neighbour_mode() {
        assert_eq!(
            most_probable_mode(
                Some(Intra4x4Mode::Vertical),     // raw 0
                Some(Intra4x4Mode::Horizontal),   // raw 1
            ),
            Intra4x4Mode::Vertical,
        );
        assert_eq!(
            most_probable_mode(
                Some(Intra4x4Mode::HorizontalUp),     // raw 8
                Some(Intra4x4Mode::DiagonalDownLeft), // raw 3
            ),
            Intra4x4Mode::DiagonalDownLeft,
        );
    }

    #[test]
    fn resolve_uses_mpm_when_prev_flag_set() {
        let actual = resolve_intra4x4_mode(true, None, Intra4x4Mode::Horizontal).unwrap();
        assert_eq!(actual, Intra4x4Mode::Horizontal);
    }

    #[test]
    fn resolve_with_prev_flag_clear_returns_rem_below_mpm() {
        // mpm = 4 (DiagonalDownRight); rem = 0 < 4 -> actual_raw = 0 = Vertical.
        let actual =
            resolve_intra4x4_mode(false, Some(0), Intra4x4Mode::DiagonalDownRight).unwrap();
        assert_eq!(actual, Intra4x4Mode::Vertical);
    }

    #[test]
    fn resolve_with_prev_flag_clear_returns_rem_plus_one_at_or_above_mpm() {
        // mpm = 2 (DC); rem = 2 -> actual_raw = 3 = DiagonalDownLeft.
        let actual = resolve_intra4x4_mode(false, Some(2), Intra4x4Mode::Dc).unwrap();
        assert_eq!(actual, Intra4x4Mode::DiagonalDownLeft);
    }

    #[test]
    fn resolve_rejects_missing_rem_when_prev_flag_clear() {
        assert!(resolve_intra4x4_mode(false, None, Intra4x4Mode::Dc).is_err());
    }

    #[test]
    fn resolve_rejects_rem_above_seven() {
        assert!(resolve_intra4x4_mode(false, Some(8), Intra4x4Mode::Dc).is_err());
    }

    #[test]
    fn context_within_mb_uses_already_decoded_modes() {
        let mut ctx = Intra4x4ModeContext::default();
        ctx.set(0, Intra4x4Mode::Vertical); // top-left of MB
        ctx.set(1, Intra4x4Mode::Horizontal); // top-second-from-left
        // Block 5 (mb position 1, 1): top neighbour is block 1, left is block 4.
        assert_eq!(ctx.top_of(5), Some(Intra4x4Mode::Horizontal));
        // Block 4 isn't set, so left_of(5) returns None.
        assert_eq!(ctx.left_of(5), None);
    }

    #[test]
    fn context_top_row_uses_top_neighbour_bottom_row() {
        let mut ctx = Intra4x4ModeContext::default();
        ctx.top_neighbour_bottom_row[2] = Some(Intra4x4Mode::Dc);
        // Block 2 (top row, third from left) gets its top from the
        // top-neighbour MB's bottom row at column 2.
        assert_eq!(ctx.top_of(2), Some(Intra4x4Mode::Dc));
    }
}
