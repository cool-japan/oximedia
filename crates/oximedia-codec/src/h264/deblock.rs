//! H.264 in-loop deblocking filter.
//!
//! Block-based coding quantizes adjacent 4×4 blocks independently,
//! which leaves visible discontinuities at the block boundaries.
//! The deblocking filter is an adaptive short-tap filter that smooths
//! those discontinuities — but only when the gradient across the
//! boundary looks like a quantization artefact rather than a real
//! image edge.
//!
//! Each block edge gets:
//!
//! 1. A *boundary strength* `bS` in `0..=4` chosen from the block
//!    types and motion info of the two adjacent blocks.  `bS = 0`
//!    skips the edge.
//! 2. Per-QP thresholds `α` and `β` from the spec's tables.
//! 3. For each 4-sample crossing line, a gradient test: filter only
//!    when `|p0 - q0| < α` and `|p1 - p0| < β` and `|q1 - q0| < β`.
//! 4. A filter formula chosen by `bS`: a strong 5-tap filter when
//!    `bS == 4`, an adaptive 4-tap filter otherwise.
//!
//! This module implements the per-line primitives (gradient test,
//! normal and strong filter formulas) plus the boundary-strength
//! derivation rule.  The MB-level pass that walks every edge of a
//! macroblock and applies these primitives is the next layer up.

/// Block-level context for one side of a deblock edge.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeblockBlockInfo {
    /// True when this block was intra-coded.
    pub is_intra: bool,
    /// True when this block carries any non-zero residual coefficient.
    pub has_residual: bool,
    /// Reference index for the block's L0 motion vector, when inter.
    pub ref_idx_l0: Option<u8>,
    /// L0 motion vector in quarter-pel units, when inter.
    pub mv_l0: Option<(i32, i32)>,
}

/// Derives the boundary strength for the edge between two blocks.
///
/// Rules (per H.264):
///
/// - `bS = 4` when either block is intra **and** the edge is a
///   macroblock boundary (the caller passes `is_mb_edge = true`).
/// - `bS = 3` when either block is intra and the edge is internal.
/// - `bS = 2` when either block carries non-zero residual.
/// - `bS = 1` when reference frames differ or the per-axis motion
///   vector difference exceeds 4 quarter-pel units (one full pixel).
/// - `bS = 0` otherwise — no filtering on this edge.
#[must_use]
pub fn boundary_strength(
    p: DeblockBlockInfo,
    q: DeblockBlockInfo,
    is_mb_edge: bool,
) -> u8 {
    if p.is_intra || q.is_intra {
        return if is_mb_edge { 4 } else { 3 };
    }
    if p.has_residual || q.has_residual {
        return 2;
    }
    if p.ref_idx_l0 != q.ref_idx_l0 {
        return 1;
    }
    if let (Some(mp), Some(mq)) = (p.mv_l0, q.mv_l0) {
        if (mp.0 - mq.0).abs() >= 4 || (mp.1 - mq.1).abs() >= 4 {
            return 1;
        }
    }
    0
}

/// Per-QP α threshold for the boundary gradient test.
///
/// The spec's table A is 52 entries long; this function reproduces it
/// algorithmically using the standard piecewise formula.
#[must_use]
pub fn alpha_threshold(qp: u8) -> u8 {
    // From H.264 Table 8-16, expressed algorithmically.  Values for
    // QP 0..=15 are 0; values from 16 onward grow piecewise.
    const TABLE_ALPHA: [u8; 52] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 5, 6, 7, 8, 9, 10, 12, 13, 15,
        17, 20, 22, 25, 28, 32, 36, 40, 45, 50, 56, 63, 71, 80, 90, 101, 113, 127, 144, 162,
        182, 203, 226, 255, 255,
    ];
    TABLE_ALPHA[qp.min(51) as usize]
}

/// Per-QP β threshold for the boundary gradient test.
#[must_use]
pub fn beta_threshold(qp: u8) -> u8 {
    // H.264 Table 8-16's β column.
    const TABLE_BETA: [u8; 52] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 6, 6, 7,
        7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18,
    ];
    TABLE_BETA[qp.min(51) as usize]
}

/// Returns true when the four samples crossing one boundary line
/// look like a quantization artefact and should be smoothed.
///
/// Samples are ordered `p1, p0, q0, q1` going across the boundary,
/// where `p0` and `q0` are the two samples immediately on either
/// side.
#[must_use]
pub fn should_filter_line(p1: u8, p0: u8, q0: u8, q1: u8, alpha: u8, beta: u8) -> bool {
    let dp = (i32::from(p0) - i32::from(q0)).unsigned_abs();
    let dpp = (i32::from(p1) - i32::from(p0)).unsigned_abs();
    let dqq = (i32::from(q1) - i32::from(q0)).unsigned_abs();
    dp < u32::from(alpha) && dpp < u32::from(beta) && dqq < u32::from(beta)
}

/// Applies the H.264 strong filter (boundary strength 4) to one
/// crossing line, in the narrow form that only modifies the four
/// innermost samples.
///
/// Input layout is `(p2, p1, p0, q0, q1, q2)`.  Only `p1, p0, q0,
/// q1` are rewritten; `p2` and `q2` pass through unchanged.  The
/// wider strong-filter variant (which also rewrites `p2` and `q2`)
/// requires `p3` and `q3` as additional inputs and is not yet
/// implemented.
#[must_use]
pub fn strong_filter_line(samples: [u8; 6]) -> [u8; 6] {
    let p2 = i32::from(samples[0]);
    let p1 = i32::from(samples[1]);
    let p0 = i32::from(samples[2]);
    let q0 = i32::from(samples[3]);
    let q1 = i32::from(samples[4]);
    let q2 = i32::from(samples[5]);

    let p0_new = (p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3;
    let p1_new = (p2 + p1 + p0 + q0 + 2) >> 2;

    let q0_new = (p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3;
    let q1_new = (p0 + q0 + q1 + q2 + 2) >> 2;

    [
        samples[0],
        p1_new.clamp(0, 255) as u8,
        p0_new.clamp(0, 255) as u8,
        q0_new.clamp(0, 255) as u8,
        q1_new.clamp(0, 255) as u8,
        samples[5],
    ]
}

/// Applies the H.264 normal filter (boundary strength 1..=3) to one
/// crossing line.
///
/// `tc0` is the spec's clipping limit for the `Δ` adjustment, looked
/// up from a QP- and bS-indexed table the caller supplies.
#[must_use]
pub fn normal_filter_line(samples: [u8; 4], tc0: u8) -> [u8; 4] {
    let p1 = i32::from(samples[0]);
    let p0 = i32::from(samples[1]);
    let q0 = i32::from(samples[2]);
    let q1 = i32::from(samples[3]);

    let delta_raw = (4 * (q0 - p0) + (p1 - q1) + 4) >> 3;
    let tc = i32::from(tc0);
    let delta = delta_raw.clamp(-tc, tc);

    let p0_new = (p0 + delta).clamp(0, 255) as u8;
    let q0_new = (q0 - delta).clamp(0, 255) as u8;

    [samples[0], p0_new, q0_new, samples[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intra_block() -> DeblockBlockInfo {
        DeblockBlockInfo {
            is_intra: true,
            has_residual: true,
            ..Default::default()
        }
    }

    fn inter_clean_block() -> DeblockBlockInfo {
        DeblockBlockInfo {
            is_intra: false,
            has_residual: false,
            ref_idx_l0: Some(0),
            mv_l0: Some((0, 0)),
        }
    }

    #[test]
    fn boundary_strength_intra_mb_edge_is_four() {
        let bs = boundary_strength(intra_block(), inter_clean_block(), true);
        assert_eq!(bs, 4);
    }

    #[test]
    fn boundary_strength_intra_internal_edge_is_three() {
        let bs = boundary_strength(intra_block(), inter_clean_block(), false);
        assert_eq!(bs, 3);
    }

    #[test]
    fn boundary_strength_residual_is_two() {
        let mut p = inter_clean_block();
        p.has_residual = true;
        let bs = boundary_strength(p, inter_clean_block(), false);
        assert_eq!(bs, 2);
    }

    #[test]
    fn boundary_strength_different_refs_is_one() {
        let mut q = inter_clean_block();
        q.ref_idx_l0 = Some(1);
        let bs = boundary_strength(inter_clean_block(), q, false);
        assert_eq!(bs, 1);
    }

    #[test]
    fn boundary_strength_large_mv_diff_is_one() {
        let mut q = inter_clean_block();
        q.mv_l0 = Some((5, 0));
        let bs = boundary_strength(inter_clean_block(), q, false);
        assert_eq!(bs, 1);
    }

    #[test]
    fn boundary_strength_smooth_inter_is_zero() {
        let bs = boundary_strength(inter_clean_block(), inter_clean_block(), false);
        assert_eq!(bs, 0);
    }

    #[test]
    fn alpha_beta_tables_match_spot_values() {
        // From the standard's tables — pin a few specific entries.
        assert_eq!(alpha_threshold(15), 0);
        assert_eq!(alpha_threshold(16), 4);
        assert_eq!(alpha_threshold(40), 80);
        assert_eq!(alpha_threshold(51), 255);
        assert_eq!(beta_threshold(15), 0);
        assert_eq!(beta_threshold(16), 2);
        assert_eq!(beta_threshold(40), 13);
        assert_eq!(beta_threshold(51), 18);
    }

    #[test]
    fn alpha_beta_clamp_to_max_at_high_qp() {
        // Out-of-range QP saturates rather than panicking.
        assert_eq!(alpha_threshold(255), 255);
        assert_eq!(beta_threshold(255), 18);
    }

    #[test]
    fn filter_line_skip_when_large_step() {
        // Hard edge of 100 should *not* trigger filtering at low QP.
        let alpha = alpha_threshold(20); // 7
        let beta = beta_threshold(20); // 3
        assert!(!should_filter_line(100, 100, 0, 0, alpha, beta));
    }

    #[test]
    fn filter_line_triggers_on_small_step() {
        let alpha = alpha_threshold(20);
        let beta = beta_threshold(20);
        assert!(should_filter_line(100, 100, 102, 102, alpha, beta));
    }

    #[test]
    fn strong_filter_uniform_input_stays_uniform() {
        let input = [50, 50, 50, 50, 50, 50];
        let out = strong_filter_line(input);
        assert_eq!(out, input);
    }

    #[test]
    fn normal_filter_uniform_input_stays_uniform() {
        let input = [50, 50, 50, 50];
        // Delta = 4*(50-50) + (50-50) = 0; output equals input.
        let out = normal_filter_line(input, 4);
        assert_eq!(out, input);
    }

    #[test]
    fn normal_filter_softens_small_step() {
        // Slight edge: p0=98, q0=102.  delta_raw = (4*(102-98) +
        // (97-103) + 4) >> 3 = (16 - 6 + 4) >> 3 = 14 >> 3 = 1.
        // delta = clamp(1, -4, 4) = 1.  p0 += 1, q0 -= 1.
        let out = normal_filter_line([97, 98, 102, 103], 4);
        assert_eq!(out, [97, 99, 101, 103]);
    }

    #[test]
    fn normal_filter_respects_tc0_clip() {
        // Without clipping, delta would be large; tc0 = 1 caps it.
        let out = normal_filter_line([50, 60, 100, 110], 1);
        // Delta is clamped to ±1, so p0 += 1, q0 -= 1.
        assert_eq!(out, [50, 61, 99, 110]);
    }
}
