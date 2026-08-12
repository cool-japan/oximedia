//! VP8 in-loop deblocking filter (RFC 6386 §15).
//!
//! VP8 defines two deblocking filters:
//! - the **normal** filter, which has separate sub-block-edge and
//!   macroblock-edge variants and a high-edge-variance ("hev") test, and
//! - the **simple** filter, applied only to luma.
//!
//! Both operate on 4-pixel-wide windows straddling a block boundary. The
//! arithmetic is performed in signed space offset by 128 ("c" values), exactly
//! as in RFC 6386 §15.2 and §15.3. All clamping uses the saturating helper
//! `c()` (clamp to signed 8-bit).

//!
//! Ported from the production-verified `oximedia-image` `webp/vp8` decoder
//! (same workspace; a WebP lossy frame is a VP8 key frame).

/// Per-macroblock filter parameters derived from the frame loop-filter header.
#[derive(Debug, Clone, Copy)]
pub struct FilterParams {
    /// Edge limit for macroblock-edge filtering.
    pub mbedge_limit: i32,
    /// Edge limit for interior sub-block-edge filtering.
    pub sub_bedge_limit: i32,
    /// Interior limit driving the `filter_yes` / segment test.
    pub interior_limit: i32,
    /// High-edge-variance threshold.
    pub hev_threshold: i32,
    /// Effective filter level for this macroblock (0 disables filtering).
    pub filter_level: i32,
}

/// Computes [`FilterParams`] for a macroblock given its effective filter
/// level and the frame type.
///
/// Follows RFC 6386 §15.2: the interior limit is derived from the level and the
/// frame sharpness; the high-edge-variance threshold depends on the level
/// magnitude **and on the frame type**.
///
/// # The frame-type term of `hev_threshold`
///
/// dixie.c `calculate_filter_parameters` (rfc6386.txt lines 9195-9200) reads:
///
/// ```text
/// hev_threshold = (filter_level >= 15);
/// if (filter_level >= 40)                                   hev_threshold++;
/// if (filter_level >= 20 && !ctx->frame_hdr.is_keyframe)     hev_threshold++;
/// ```
///
/// so an **inter** frame gets one extra increment from level 20 upwards
/// (RFC 6386 §15.2 states the same rule in prose). Levels 0..14 give 0 for
/// both frame types; 15..19 give 1; 20..39 give 1 (key) / 2 (inter); 40..63
/// give 2 (key) / 3 (inter).
#[must_use]
pub fn compute_filter_params(filter_level: i32, sharpness: i32, is_keyframe: bool) -> FilterParams {
    let level = filter_level.clamp(0, 63);

    // Interior limit (RFC 6386 §15.2).
    let mut interior_limit = level;
    if sharpness > 0 {
        interior_limit >>= if sharpness > 4 { 2 } else { 1 };
        let cap = 9 - sharpness;
        if interior_limit > cap {
            interior_limit = cap;
        }
    }
    if interior_limit < 1 {
        interior_limit = 1;
    }

    // High-edge-variance threshold (RFC 6386 §15.2; see the doc comment).
    let mut hev_threshold = i32::from(level >= 15);
    if level >= 40 {
        hev_threshold += 1;
    }
    if level >= 20 && !is_keyframe {
        hev_threshold += 1;
    }

    // Edge limits (RFC 6386 §15.2): the macroblock edge gets a wider window.
    let mbedge_limit = ((level + 2) * 2) + interior_limit;
    let sub_bedge_limit = (level * 2) + interior_limit;

    FilterParams {
        mbedge_limit,
        sub_bedge_limit,
        interior_limit,
        hev_threshold,
        filter_level: level,
    }
}

/// Saturating clamp into signed 8-bit range `[-128, 127]` (RFC 6386 `c`).
#[inline]
fn c(v: i32) -> i32 {
    v.clamp(-128, 127)
}

/// Converts an unsigned pixel into the signed ("centred") domain.
#[inline]
fn u2s(v: u8) -> i32 {
    i32::from(v) - 128
}

/// Converts a signed ("centred") value back into an unsigned pixel.
#[inline]
fn s2u(v: i32) -> u8 {
    (v + 128).clamp(0, 255) as u8
}

/// The "common adjustment" applied by every variant of the normal filter.
///
/// Operates on the centred values of the two pixels straddling the edge
/// (`p1`, `p0`, `q0`, `q1`) and returns the updated `(p0, q0)` plus the value
/// `a` (the rounded `f1` term) used by the outer-tap update. `use_outer_taps`
/// selects whether the `p1`/`q1` difference participates.
#[inline]
fn common_adjust(use_outer_taps: bool, p1: i32, p0: i32, q0: i32, q1: i32) -> (i32, i32, i32) {
    // a accumulates the filter response.
    let a = c(if use_outer_taps { c(p1 - q1) } else { 0 } + 3 * (q0 - p0));

    // Two rounded shifts produce the p0 / q0 adjustments.
    let f1 = c(a + 4) >> 3;
    let f2 = c(a + 3) >> 3;

    let new_q0 = c(q0 - f1);
    let new_p0 = c(p0 + f2);

    (new_p0, new_q0, f1)
}

/// Applies the simple loop filter across one edge (RFC 6386 §15.3).
///
/// `data` is a single plane; `p` indexes the pixel immediately *after* the
/// edge (`q0`). `step` is the distance between adjacent samples across the edge
/// (1 for a vertical edge, `stride` for a horizontal edge). `edge_limit` gates
/// whether the edge is filtered.
pub fn simple_filter_edge(data: &mut [u8], p: usize, step: usize, edge_limit: i32) {
    if p < 2 * step || p + step >= data.len() {
        return;
    }
    let p1 = u2s(data[p - 2 * step]);
    let p0 = u2s(data[p - step]);
    let q0 = u2s(data[p]);
    let q1 = u2s(data[p + step]);

    // Simple threshold test (RFC 6386 §15.3).
    if (p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1) > edge_limit {
        return;
    }
    let (np0, nq0, _) = common_adjust(true, p1, p0, q0, q1);
    data[p - step] = s2u(np0);
    data[p] = s2u(nq0);
}

/// Tests whether a 4-pixel window should be filtered at all (RFC 6386 §15.2).
#[inline]
#[allow(clippy::too_many_arguments)]
fn filter_yes(
    edge_limit: i32,
    interior_limit: i32,
    p3: i32,
    p2: i32,
    p1: i32,
    p0: i32,
    q0: i32,
    q1: i32,
    q2: i32,
    q3: i32,
) -> bool {
    (p0 - q0).abs() * 2 + ((p1 - q1).abs() >> 1) <= edge_limit
        && (p3 - p2).abs() <= interior_limit
        && (p2 - p1).abs() <= interior_limit
        && (p1 - p0).abs() <= interior_limit
        && (q3 - q2).abs() <= interior_limit
        && (q2 - q1).abs() <= interior_limit
        && (q1 - q0).abs() <= interior_limit
}

/// High-edge-variance test (RFC 6386 §15.2).
#[inline]
fn hev(threshold: i32, p1: i32, p0: i32, q0: i32, q1: i32) -> bool {
    (p1 - p0).abs() > threshold || (q1 - q0).abs() > threshold
}

/// Applies the normal sub-block-edge filter across one edge (RFC 6386 §15.4).
///
/// Uses 4 samples on each side. `p` indexes `q0`; `step` is the cross-edge
/// stride. Only `p1, p0, q0, q1` may be modified.
pub fn normal_subblock_filter_edge(data: &mut [u8], p: usize, step: usize, params: &FilterParams) {
    if p < 4 * step || p + 3 * step >= data.len() {
        return;
    }
    let p3 = u2s(data[p - 4 * step]);
    let p2 = u2s(data[p - 3 * step]);
    let p1 = u2s(data[p - 2 * step]);
    let p0 = u2s(data[p - step]);
    let q0 = u2s(data[p]);
    let q1 = u2s(data[p + step]);
    let q2 = u2s(data[p + 2 * step]);
    let q3 = u2s(data[p + 3 * step]);

    if !filter_yes(
        params.sub_bedge_limit,
        params.interior_limit,
        p3,
        p2,
        p1,
        p0,
        q0,
        q1,
        q2,
        q3,
    ) {
        return;
    }
    let high_variance = hev(params.hev_threshold, p1, p0, q0, q1);

    // Common adjustment uses the outer taps only when hev is set.
    let (np0, nq0, a) = common_adjust(high_variance, p1, p0, q0, q1);

    // When hev is false, the outer taps p1/q1 receive a half-strength update.
    if !high_variance {
        let adj = (a + 1) >> 1;
        let np1 = c(p1 + adj);
        let nq1 = c(q1 - adj);
        data[p - 2 * step] = s2u(np1);
        data[p + step] = s2u(nq1);
    }
    data[p - step] = s2u(np0);
    data[p] = s2u(nq0);
}

/// Applies the normal macroblock-edge filter across one edge (RFC 6386 §15.4).
///
/// Modifies up to 3 samples on each side of the edge (`p2..p0`, `q0..q2`).
pub fn normal_mbedge_filter_edge(data: &mut [u8], p: usize, step: usize, params: &FilterParams) {
    if p < 4 * step || p + 3 * step >= data.len() {
        return;
    }
    let p3 = u2s(data[p - 4 * step]);
    let p2 = u2s(data[p - 3 * step]);
    let p1 = u2s(data[p - 2 * step]);
    let p0 = u2s(data[p - step]);
    let q0 = u2s(data[p]);
    let q1 = u2s(data[p + step]);
    let q2 = u2s(data[p + 2 * step]);
    let q3 = u2s(data[p + 3 * step]);

    if !filter_yes(
        params.mbedge_limit,
        params.interior_limit,
        p3,
        p2,
        p1,
        p0,
        q0,
        q1,
        q2,
        q3,
    ) {
        return;
    }

    if hev(params.hev_threshold, p1, p0, q0, q1) {
        // High variance: behave like the sub-block filter with outer taps.
        let (np0, nq0, _) = common_adjust(true, p1, p0, q0, q1);
        data[p - step] = s2u(np0);
        data[p] = s2u(nq0);
        return;
    }

    // Low variance: the wide 6-tap macroblock filter (RFC 6386 §15.4).
    let w = c(c(p1 - q1) + 3 * (q0 - p0));

    let a = (27 * w + 63) >> 7;
    let new_q0 = c(q0 - a);
    let new_p0 = c(p0 + a);

    let a = (18 * w + 63) >> 7;
    let new_q1 = c(q1 - a);
    let new_p1 = c(p1 + a);

    let a = (9 * w + 63) >> 7;
    let new_q2 = c(q2 - a);
    let new_p2 = c(p2 + a);

    data[p - 3 * step] = s2u(new_p2);
    data[p - 2 * step] = s2u(new_p1);
    data[p - step] = s2u(new_p0);
    data[p] = s2u(new_q0);
    data[p + step] = s2u(new_q1);
    data[p + 2 * step] = s2u(new_q2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_params_level_zero() {
        let p = compute_filter_params(0, 0, true);
        assert_eq!(p.filter_level, 0);
        assert_eq!(p.interior_limit, 1);
        assert_eq!(p.hev_threshold, 0);
    }

    #[test]
    fn test_filter_params_high_level() {
        let p_inter = compute_filter_params(50, 0, false);
        assert_eq!(
            p_inter.hev_threshold, 3,
            "an inter frame at level 50 takes all three hev increments \
             (>=15, >=40, and >=20-when-not-a-key-frame): rfc6386.txt lines 9195-9200"
        );
        let p = compute_filter_params(50, 0, true);
        assert_eq!(p.hev_threshold, 2);
        assert!(p.mbedge_limit > p.sub_bedge_limit);
    }

    #[test]
    fn test_filter_params_sharpness_caps_interior() {
        let p = compute_filter_params(63, 7, true);
        // interior = 63 >> 2 = 15, capped to 9 - 7 = 2.
        assert_eq!(p.interior_limit, 2);
    }

    #[test]
    fn test_hev_threshold_frame_type_table() {
        // dixie `calculate_filter_parameters`, rfc6386.txt lines 9195-9200:
        //   hev  = (level >= 15) + (level >= 40) + (level >= 20 && !key)
        // Only the 20..=39 band and the 40+ band differ between frame types,
        // which is exactly what a key-frame-only implementation gets wrong.
        for (level, key, want) in [
            (0, true, 0),
            (0, false, 0),
            (14, false, 0),
            (15, true, 1),
            (15, false, 1),
            (19, false, 1),
            (20, true, 1),
            (20, false, 2),
            (39, false, 2),
            (40, true, 2),
            (40, false, 3),
            (63, true, 2),
            (63, false, 3),
        ] {
            assert_eq!(
                compute_filter_params(level, 0, key).hev_threshold,
                want,
                "level {level}, is_keyframe {key}"
            );
        }
    }

    #[test]
    fn test_simple_filter_leaves_smooth_region_unchanged() {
        let mut data = vec![128u8; 16];
        let before = data.clone();
        simple_filter_edge(&mut data, 8, 1, 20);
        assert_eq!(data, before, "flat region must be untouched");
    }

    #[test]
    fn test_simple_filter_softens_step() {
        // Step edge: left side 100, right side 156, edge at index 8.
        let mut data = vec![0u8; 16];
        for v in data.iter_mut().take(8) {
            *v = 100;
        }
        for v in data.iter_mut().skip(8) {
            *v = 156;
        }
        let p0_before = data[7];
        let q0_before = data[8];
        simple_filter_edge(&mut data, 8, 1, 200);
        assert!(data[7] >= p0_before, "p0 should rise");
        assert!(data[8] <= q0_before, "q0 should fall");
    }

    #[test]
    fn test_normal_subblock_filter_flat_unchanged() {
        let mut data = vec![120u8; 32];
        let before = data.clone();
        let params = compute_filter_params(30, 0, true);
        normal_subblock_filter_edge(&mut data, 16, 1, &params);
        assert_eq!(data, before);
    }

    #[test]
    fn test_normal_mbedge_filter_softens_step() {
        let mut data = vec![0u8; 32];
        for v in data.iter_mut().take(16) {
            *v = 110;
        }
        for v in data.iter_mut().skip(16) {
            *v = 146;
        }
        let params = compute_filter_params(40, 0, true);
        let q0_before = data[16];
        normal_mbedge_filter_edge(&mut data, 16, 1, &params);
        assert!(data[16] <= q0_before);
    }

    #[test]
    fn test_hev_and_filter_yes_helpers() {
        assert!(hev(0, 10, 0, 0, 0));
        assert!(!hev(20, 10, 0, 0, 0));
        assert!(filter_yes(255, 255, 0, 0, 0, 0, 0, 0, 0, 0));
    }
}
