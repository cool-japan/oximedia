//! H.264 motion vector prediction.
//!
//! A motion vector can be up to ~22 bits in H.264, which is a lot to
//! transmit per block.  H.264 predicts the MV from neighbour-block
//! MVs and transmits only the *delta* (`mvd_l0[x]`, `mvd_l0[y]`); the
//! decoder reconstructs the predicted MV using the same rule.
//!
//! The Baseline/Main rule (Median Predictor) takes the per-axis
//! median of the MVs from the left, above, and above-right
//! neighbours.  When above-right is unavailable, above-left
//! substitutes.  When neither above-right nor above-left is
//! available, the predictor degrades to a zero MV for the missing
//! position.
//!
//! H.264 also defines partition-specific overrides for 16×8 and 8×16
//! sub-macroblock partitions where one neighbour's MV is used
//! directly instead of running the median.  Those land here as
//! separate helpers.

/// A motion vector in quarter-pel units.
pub type MotionVector = (i32, i32);

/// Neighbour-block MV availability for one MV prediction.
///
/// `None` for a neighbour means it's unavailable (slice boundary,
/// picture edge, or non-inter macroblock).
#[derive(Debug, Clone, Copy, Default)]
pub struct MvPredictionContext {
    /// Left-neighbour MV.
    pub left: Option<MotionVector>,
    /// Above-neighbour MV.
    pub above: Option<MotionVector>,
    /// Above-right-neighbour MV.
    pub above_right: Option<MotionVector>,
    /// Above-left-neighbour MV (used as fallback when above-right
    /// is missing).
    pub above_left: Option<MotionVector>,
}

/// Returns the median of three signed integers.
#[must_use]
pub fn median3(a: i32, b: i32, c: i32) -> i32 {
    let lo = a.min(b).min(c);
    let hi = a.max(b).max(c);
    a + b + c - lo - hi
}

/// Predicts the MV for one motion-partition using H.264's median
/// predictor rule.
///
/// Algorithm:
///
/// 1. If only one of `left` / `above` is available, use that one.
/// 2. If both are available, pick the third neighbour: prefer
///    `above_right`; fall back to `above_left` if absent; fall back
///    to `(0, 0)` if both are absent.
/// 3. Return the per-axis median of the three neighbour MVs.
#[must_use]
pub fn predict_mv_median(ctx: &MvPredictionContext) -> MotionVector {
    match (ctx.left, ctx.above) {
        (None, None) => {
            // No directly adjacent neighbour — the predictor is just
            // the third neighbour or zero.
            ctx.above_right
                .or(ctx.above_left)
                .unwrap_or((0, 0))
        }
        (Some(left), None) => left,
        (None, Some(above)) => above,
        (Some(left), Some(above)) => {
            let third = ctx
                .above_right
                .or(ctx.above_left)
                .unwrap_or((0, 0));
            (
                median3(left.0, above.0, third.0),
                median3(left.1, above.1, third.1),
            )
        }
    }
}

/// MV predictor for the top half of a 16×8 partition.
///
/// Per spec: when the macroblock above is also a 16×8 partition with
/// the same reference index, use its MV directly; otherwise fall
/// back to the median predictor.
#[must_use]
pub fn predict_mv_16x8_top(
    ctx: &MvPredictionContext,
    above_uses_same_ref: bool,
) -> MotionVector {
    if let (true, Some(above)) = (above_uses_same_ref, ctx.above) {
        above
    } else {
        predict_mv_median(ctx)
    }
}

/// MV predictor for the bottom half of a 16×8 partition.
///
/// Mirrors the top-half rule against the *left* neighbour.
#[must_use]
pub fn predict_mv_16x8_bottom(
    ctx: &MvPredictionContext,
    left_uses_same_ref: bool,
) -> MotionVector {
    if let (true, Some(left)) = (left_uses_same_ref, ctx.left) {
        left
    } else {
        predict_mv_median(ctx)
    }
}

/// MV predictor for the left half of an 8×16 partition.
#[must_use]
pub fn predict_mv_8x16_left(
    ctx: &MvPredictionContext,
    left_uses_same_ref: bool,
) -> MotionVector {
    if let (true, Some(left)) = (left_uses_same_ref, ctx.left) {
        left
    } else {
        predict_mv_median(ctx)
    }
}

/// MV predictor for the right half of an 8×16 partition.
#[must_use]
pub fn predict_mv_8x16_right(
    ctx: &MvPredictionContext,
    above_right_uses_same_ref: bool,
) -> MotionVector {
    if let (true, Some(ar)) = (above_right_uses_same_ref, ctx.above_right) {
        ar
    } else {
        predict_mv_median(ctx)
    }
}

/// Reconstructs an actual MV from a predicted MV plus the bitstream
/// `mvd_l0` / `mvd_l1` delta.
#[must_use]
pub fn apply_mv_delta(predicted: MotionVector, delta: MotionVector) -> MotionVector {
    (
        predicted.0.wrapping_add(delta.0),
        predicted.1.wrapping_add(delta.1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median3_handles_typical_cases() {
        assert_eq!(median3(1, 2, 3), 2);
        assert_eq!(median3(3, 1, 2), 2);
        assert_eq!(median3(5, 5, 5), 5);
        assert_eq!(median3(-3, 0, 3), 0);
        assert_eq!(median3(1_000_000, -1_000_000, 0), 0);
    }

    #[test]
    fn predict_with_no_neighbours_yields_zero() {
        let ctx = MvPredictionContext::default();
        assert_eq!(predict_mv_median(&ctx), (0, 0));
    }

    #[test]
    fn predict_with_only_left_returns_left() {
        let ctx = MvPredictionContext {
            left: Some((10, -3)),
            ..Default::default()
        };
        assert_eq!(predict_mv_median(&ctx), (10, -3));
    }

    #[test]
    fn predict_with_only_above_returns_above() {
        let ctx = MvPredictionContext {
            above: Some((7, 2)),
            ..Default::default()
        };
        assert_eq!(predict_mv_median(&ctx), (7, 2));
    }

    #[test]
    fn predict_with_all_three_returns_per_axis_median() {
        let ctx = MvPredictionContext {
            left: Some((1, 10)),
            above: Some((5, 0)),
            above_right: Some((-3, 20)),
            above_left: None,
        };
        // Per-axis median of (1, 5, -3) = 1; of (10, 0, 20) = 10.
        assert_eq!(predict_mv_median(&ctx), (1, 10));
    }

    #[test]
    fn predict_falls_back_to_above_left_when_above_right_missing() {
        let ctx = MvPredictionContext {
            left: Some((1, 1)),
            above: Some((5, 5)),
            above_right: None,
            above_left: Some((9, 9)),
        };
        // Median of (1, 5, 9) = 5; same for y.
        assert_eq!(predict_mv_median(&ctx), (5, 5));
    }

    #[test]
    fn partition_16x8_top_uses_above_when_same_ref() {
        let ctx = MvPredictionContext {
            above: Some((7, 9)),
            left: Some((1, 2)),
            ..Default::default()
        };
        assert_eq!(predict_mv_16x8_top(&ctx, true), (7, 9));
        // Without same-ref flag, fall back to median.
        let med = predict_mv_median(&ctx);
        assert_eq!(predict_mv_16x8_top(&ctx, false), med);
    }

    #[test]
    fn partition_16x8_bottom_uses_left_when_same_ref() {
        let ctx = MvPredictionContext {
            above: Some((7, 9)),
            left: Some((1, 2)),
            ..Default::default()
        };
        assert_eq!(predict_mv_16x8_bottom(&ctx, true), (1, 2));
    }

    #[test]
    fn partition_8x16_left_uses_left() {
        let ctx = MvPredictionContext {
            above: Some((7, 9)),
            left: Some((1, 2)),
            ..Default::default()
        };
        assert_eq!(predict_mv_8x16_left(&ctx, true), (1, 2));
    }

    #[test]
    fn partition_8x16_right_uses_above_right_when_present() {
        let ctx = MvPredictionContext {
            above_right: Some((4, -4)),
            left: Some((1, 2)),
            ..Default::default()
        };
        assert_eq!(predict_mv_8x16_right(&ctx, true), (4, -4));
    }

    #[test]
    fn apply_mv_delta_round_trip() {
        let predicted = (5, -3);
        let delta = (2, 4);
        assert_eq!(apply_mv_delta(predicted, delta), (7, 1));
    }
}
