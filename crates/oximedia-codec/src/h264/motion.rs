//! Sub-pixel motion compensation filters for H.264 inter prediction.
//!
//! Inter-predicted blocks fetch their samples from a reference frame
//! at a position specified by a motion vector that may have
//! fractional-pixel precision: ¼-pixel for luma, ⅛-pixel for chroma.
//! The decoder *interpolates* the integer-grid neighbours of the
//! fractional position to recover the sub-pel sample.
//!
//! Two filters do the work:
//!
//! - Luma: a 6-tap FIR with coefficients `(1, -5, 20, 20, -5, 1)`
//!   applied to either the horizontal or vertical neighbour line for
//!   half-pel positions, then averaged with integer samples for
//!   quarter-pel positions.  The exact bit pattern of intermediate
//!   shifts and clamps is part of H.264's bit-exact specification —
//!   every conformant decoder produces the same output sample by
//!   sample.
//! - Chroma: a 4-tap bilinear filter applied at ⅛-pel granularity.
//!   Simpler than luma; one formula covers all 64 sub-pel offsets.
//!
//! This module implements the filter primitives plus a helper that
//! produces a single sub-pel luma sample.  Block-level motion
//! compensation (fetching a `block_w × block_h` block at a
//! fractional MV from a reference frame) wraps these primitives and
//! lives one layer above.

/// Standard H.264 6-tap luma filter coefficients.
const TAP_COEFFS: [i32; 6] = [1, -5, 20, 20, -5, 1];

/// Applies the 6-tap luma filter to a window of six samples, returning
/// the unclipped intermediate result.  Callers responsible for the
/// post-filter `>>` shift and `clamp` per the bit-exact procedure.
#[must_use]
pub fn luma_6tap_unclipped(samples: [i32; 6]) -> i32 {
    samples[0] * TAP_COEFFS[0]
        + samples[1] * TAP_COEFFS[1]
        + samples[2] * TAP_COEFFS[2]
        + samples[3] * TAP_COEFFS[3]
        + samples[4] * TAP_COEFFS[4]
        + samples[5] * TAP_COEFFS[5]
}

/// Applies the H.264 luma half-pel filter to six samples and produces
/// one clipped 8-bit sub-pel sample.
///
/// `(luma_6tap_unclipped(samples) + 16) >> 5` then `clamp(0..=255)`.
#[must_use]
pub fn luma_half_pel(samples: [i32; 6]) -> u8 {
    let r = luma_6tap_unclipped(samples);
    (((r + 16) >> 5).clamp(0, 255)) as u8
}

/// Quarter-pel positions on a single axis combine a half-pel value
/// with an integer-grid sample by averaging.
///
/// `rounded_average(a, b)` computes `(a + b + 1) >> 1`, used for
/// quarter-pel positions a ¼ or ¾ away from each half-pel.
#[must_use]
pub fn rounded_average(a: u8, b: u8) -> u8 {
    ((u32::from(a) + u32::from(b) + 1) >> 1) as u8
}

/// Chroma bilinear interpolator at ⅛-pel granularity.
///
/// `four_neighbours` holds the four integer-grid chroma samples
/// surrounding the sub-pel position: top-left, top-right, bottom-
/// left, bottom-right.  `dx` and `dy` are the ⅛-pel offsets in
/// `0..=7`.
#[must_use]
pub fn chroma_bilinear(
    four_neighbours: [u8; 4],
    dx: u8,
    dy: u8,
) -> u8 {
    let [tl, tr, bl, br] = four_neighbours;
    let dx = u32::from(dx);
    let dy = u32::from(dy);
    let sum = (8 - dx) * (8 - dy) * u32::from(tl)
        + dx * (8 - dy) * u32::from(tr)
        + (8 - dx) * dy * u32::from(bl)
        + dx * dy * u32::from(br);
    ((sum + 32) >> 6) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_tap_unclipped_sum_of_coeffs_is_thirty_two() {
        // 1 - 5 + 20 + 20 - 5 + 1 = 32.  This is the normalization
        // factor implied by the post-filter (>> 5) shift.
        assert_eq!(TAP_COEFFS.iter().sum::<i32>(), 32);
    }

    #[test]
    fn six_tap_constant_input_yields_input_times_thirty_two() {
        // For uniform input [k; 6], the filter returns 32k.
        assert_eq!(luma_6tap_unclipped([7; 6]), 7 * 32);
    }

    #[test]
    fn half_pel_constant_input_recovers_constant() {
        // After post-shift the constant comes back: (32k + 16) >> 5 = k.
        for k in 0..=255u8 {
            assert_eq!(luma_half_pel([i32::from(k); 6]), k);
        }
    }

    #[test]
    fn half_pel_clips_negative_overshoot() {
        // Set up an input where the unclipped sum is negative — the
        // post-clamp must produce 0.
        let samples = [200, 0, 0, 0, 0, 0];
        let r = luma_6tap_unclipped(samples);
        // r = 200 * 1 + 0 + 0 + 0 + 0 + 0 = 200; (200 + 16) >> 5 = 6.
        // That's not negative; let's pick a configuration that is:
        // 0 * 1 + 255 * -5 + 0 + 0 + 0 + 0 = -1275; (-1275 + 16) >> 5 < 0.
        let _ = r;
        let neg_samples = [0, 255, 0, 0, 0, 0];
        assert_eq!(luma_half_pel(neg_samples), 0, "must clip to 0");
    }

    #[test]
    fn half_pel_clips_positive_overshoot() {
        let samples = [0, 0, 255, 255, 0, 0];
        // 0 + 0 + 5100 + 5100 + 0 + 0 = 10200; (10200 + 16) >> 5 = 319.
        // After clip to 255.
        assert_eq!(luma_half_pel(samples), 255);
    }

    #[test]
    fn rounded_average_round_half_up() {
        assert_eq!(rounded_average(0, 0), 0);
        assert_eq!(rounded_average(0, 1), 1);
        assert_eq!(rounded_average(0, 2), 1);
        assert_eq!(rounded_average(1, 2), 2);
        assert_eq!(rounded_average(255, 255), 255);
    }

    #[test]
    fn chroma_bilinear_at_integer_position_returns_top_left() {
        assert_eq!(chroma_bilinear([100, 200, 50, 150], 0, 0), 100);
    }

    #[test]
    fn chroma_bilinear_at_top_right_returns_top_right() {
        assert_eq!(chroma_bilinear([100, 200, 50, 150], 8, 0), 200);
    }

    #[test]
    fn chroma_bilinear_at_bottom_right_returns_bottom_right() {
        // dx = 8 -> all weight on right column; dy = 8 -> all on
        // bottom row.  Edge values (8) saturate weight to the
        // opposite corner; chroma sub-pel offsets per spec only run
        // 0..=7 in practice, but the formula degenerates correctly.
        let v = chroma_bilinear([100, 200, 50, 150], 8, 8);
        assert_eq!(v, 150);
    }

    #[test]
    fn chroma_bilinear_centre_weights_evenly() {
        // dx = 4, dy = 4: each corner contributes 16 of 64 total.
        // (16 * 100 + 16 * 200 + 16 * 50 + 16 * 150 + 32) >> 6
        // = (8000 + 32) >> 6 = 125
        assert_eq!(chroma_bilinear([100, 200, 50, 150], 4, 4), 125);
    }

    #[test]
    fn chroma_bilinear_constant_input_recovers_constant() {
        for dx in 0..8 {
            for dy in 0..8 {
                assert_eq!(chroma_bilinear([42, 42, 42, 42], dx, dy), 42);
            }
        }
    }
}
