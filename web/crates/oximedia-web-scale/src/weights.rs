// Copyright COOLJAPAN OU (Team KitaSan). Licensed under Apache-2.0.

//! Precomputed, per-output-sample separable resampling weight tables.
//!
//! This is the flat, fixed-stride cousin of `FilterWeightTable` /
//! `ScaleWeightCache` from `crates/oximedia-scaling/src/ewa_resample.rs`:
//! for a fixed `(src_len, dst_len, filter)` triple, every output sample's
//! filter taps (source indices + normalized weights) are computed once, up
//! front, and reused for every row (horizontal pass) or column (vertical
//! pass) — and, since a [`crate::Resizer`] holds its tables for its whole
//! lifetime, across every frame of a video stream at a fixed resolution.
//!
//! Two deliberate differences from the upstream `FilterWeightTable`:
//!
//! - **Fixed stride, not ragged.** Every output sample gets exactly
//!   [`WeightTable::span`] `(weight, index)` pairs, padded with `weight =
//!   0.0` entries (whose index is simply clamped like any other tap, so
//!   they are always in-bounds and safe to read). This trades a little
//!   memory for a branch-free, uniform-trip-count inner loop in
//!   [`crate::Resizer`]'s hot path, which is friendlier to autovectorization
//!   than a ragged `(offset, count)` layout.
//! - **No weight clamping.** Unlike
//!   `Resampler::resize_horizontal`/`resize_vertical` in
//!   `crates/oximedia-scaling/src/resampler.rs` (which clamp each tap
//!   weight to `>= 0.0` before accumulating), this table keeps negative taps
//!   as-is, matching `FilterWeightTable::build`. Lanczos3 and the two cubic
//!   kernels here have genuine negative side lobes that are load-bearing for
//!   their sharpening behavior; zeroing them would silently degrade
//!   Lanczos3 towards a positive-lobe-only blur.
//!
//! # Interior vs. boundary rows
//!
//! [`WeightTable::row`] always returns a correct, already-clamped `(weight,
//! index)` pair per tap — but computing a source address from a *loaded*
//! index defeats LLVM's autovectorizer, which will not turn an
//! indirect/gather-shaped load into a vector load. For the overwhelming
//! majority of output samples (every one whose tap window doesn't reach off
//! either edge of the source axis) the clamp in [`WeightTable::build`] never
//! actually triggers, so the tap window is just `span` *consecutive* source
//! samples starting at [`WeightTable::base`]. [`WeightTable::is_interior`]
//! reports when that holds, letting [`crate::Resizer`]'s hot loop replace
//! the indexed gather with a plain contiguous slice read for interior
//! samples (a shape LLVM readily auto-vectorizes as a small FIR/convolution
//! kernel), falling back to the always-correct indexed path only for the
//! `O(span)` boundary rows at each end of an axis.

use oximedia_web_core::CoreError;

use crate::filter::Filter;

/// A precomputed 1D resampling weight table for one `(src_len -> dst_len)`
/// axis under a given [`Filter`].
///
/// # Memory layout
///
/// `weights` and `indices` are both `dst_len * span` flat arrays: output
/// sample `i`'s taps occupy `[i * span, (i + 1) * span)` in both. Each row's
/// weights are normalized to sum to `1.0` (subject to floating-point
/// rounding), so applying the table to a constant-valued signal reproduces
/// that constant exactly (DC preservation) regardless of the kernel's
/// negative lobes. `base` and `interior` (both length `dst_len`) are the
/// fast-path sidecar described above.
#[derive(Debug, Clone)]
pub struct WeightTable {
    src_len: usize,
    dst_len: usize,
    span: usize,
    weights: Vec<f32>,
    indices: Vec<u32>,
    /// Unclamped first-tap source index (`lo`) for each output row.
    base: Vec<i32>,
    /// `true` when `[base, base + span)` lies entirely within
    /// `[0, src_len)`, i.e. no tap in this row was clamped.
    interior: Vec<bool>,
}

impl WeightTable {
    /// Builds a weight table resampling `src_len` samples to `dst_len`
    /// samples under `filter`.
    ///
    /// The kernel's support radius is scaled by `max(1.0, src_len /
    /// dst_len)` before evaluation on downscale (`src_len > dst_len`), which
    /// widens the effective filter footprint so every source sample still
    /// contributes — this is what prevents moire/aliasing on downscale
    /// (e.g. 4K -> 1080p) instead of just subsampling through the kernel's
    /// native (upscale-sized) support.
    ///
    /// Out-of-range taps (near the first/last sample) are clamped to
    /// `[0, src_len - 1]` ("clamp to edge") rather than treated as zero or
    /// wrapped.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::ZeroDimension`] if either `src_len` or `dst_len`
    /// is zero.
    pub fn build(filter: Filter, src_len: usize, dst_len: usize) -> Result<Self, CoreError> {
        if src_len == 0 || dst_len == 0 {
            return Err(CoreError::ZeroDimension);
        }

        let scale = src_len as f32 / dst_len as f32;
        let filter_scale = scale.max(1.0);
        let support = filter.support() * filter_scale;
        // Upper bound on `hi - lo + 1` where lo = floor(center - support),
        // hi = ceil(center + support): the window width is at most
        // `2 * support + 2`; using `ceil(support)` keeps the bound an
        // integer without under-counting from float rounding.
        let span = (support.ceil() as usize) * 2 + 2;

        let mut weights = vec![0.0f32; dst_len * span];
        let mut indices = vec![0u32; dst_len * span];
        let mut base = vec![0i32; dst_len];
        let mut interior = vec![false; dst_len];

        for i in 0..dst_len {
            let center = (i as f32 + 0.5) * scale - 0.5;
            let lo = (center - support).floor() as i64;
            let row = i * span;

            base[i] = lo as i32;
            interior[i] = lo >= 0 && lo + span as i64 <= src_len as i64;

            let mut sum = 0.0f32;
            for k in 0..span {
                let s = lo + k as i64;
                let clamped = s.clamp(0, src_len as i64 - 1) as u32;
                let x = (s as f32 - center) / filter_scale;
                let w = filter.evaluate(x);
                weights[row + k] = w;
                indices[row + k] = clamped;
                sum += w;
            }

            if sum.abs() > 1e-8 {
                let inv = 1.0 / sum;
                for w in &mut weights[row..row + span] {
                    *w *= inv;
                }
            } else {
                // Degenerate row (should not occur for any of the four
                // kernels here, since the center tap always lands inside
                // the window and evaluates to a positive peak — kept as a
                // defensive fallback rather than dividing by ~0). Collapse
                // to a single nearest-neighbor tap so the row still sums to
                // exactly 1.0.
                let mut best_k = 0usize;
                let mut best_dist = f32::MAX;
                for k in 0..span {
                    let s = lo + k as i64;
                    let dist = (s as f32 - center).abs();
                    if dist < best_dist {
                        best_dist = dist;
                        best_k = k;
                    }
                }
                for w in &mut weights[row..row + span] {
                    *w = 0.0;
                }
                weights[row + best_k] = 1.0;
            }
        }

        Ok(Self {
            src_len,
            dst_len,
            span,
            weights,
            indices,
            base,
            interior,
        })
    }

    /// Source axis length this table was built for.
    #[inline]
    #[must_use]
    pub fn src_len(&self) -> usize {
        self.src_len
    }

    /// Destination axis length this table was built for.
    #[inline]
    #[must_use]
    pub fn dst_len(&self) -> usize {
        self.dst_len
    }

    /// Number of `(weight, index)` taps stored per output sample.
    #[inline]
    #[must_use]
    pub fn span(&self) -> usize {
        self.span
    }

    /// Returns output sample `i`'s parallel weight and source-index slices
    /// (both length [`Self::span`]).
    ///
    /// Indices are always valid (already clamped to `[0, src_len - 1]`), so
    /// callers may index the source axis with them unconditionally.
    #[inline]
    #[must_use]
    pub fn row(&self, i: usize) -> (&[f32], &[u32]) {
        let start = i * self.span;
        let end = start + self.span;
        (&self.weights[start..end], &self.indices[start..end])
    }

    /// Just output sample `i`'s weight slice (length [`Self::span`]), for
    /// callers that already know (via [`Self::is_interior`]) that they will
    /// address the source axis with [`Self::base`] instead of the indexed
    /// path.
    #[inline]
    #[must_use]
    pub fn weights_row(&self, i: usize) -> &[f32] {
        let start = i * self.span;
        &self.weights[start..start + self.span]
    }

    /// `true` when output sample `i`'s tap window `[base(i), base(i) +
    /// span)` lies entirely within `[0, src_len)`, i.e. every tap's index
    /// is exactly `base(i) + k` with no edge clamping.
    #[inline]
    #[must_use]
    pub fn is_interior(&self, i: usize) -> bool {
        self.interior[i]
    }

    /// The unclamped first-tap source index for output sample `i`.
    ///
    /// Only meaningful as a direct (non-negative, in-bounds) source index
    /// when [`Self::is_interior`] returns `true` for the same `i`; callers
    /// must check that first.
    #[inline]
    #[must_use]
    pub fn base(&self, i: usize) -> i32 {
        self.base[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rejects_zero_dimension() {
        assert_eq!(
            WeightTable::build(Filter::Bilinear, 0, 4).unwrap_err(),
            CoreError::ZeroDimension
        );
        assert_eq!(
            WeightTable::build(Filter::Bilinear, 4, 0).unwrap_err(),
            CoreError::ZeroDimension
        );
    }

    #[test]
    fn every_row_sums_to_one_for_all_filters_up_down_identity() {
        for filter in [
            Filter::Bilinear,
            Filter::CatmullRom,
            Filter::Mitchell,
            Filter::Lanczos3,
        ] {
            for (src, dst) in [(64, 64), (64, 32), (32, 64), (641, 123), (7, 1), (1, 7)] {
                let table = WeightTable::build(filter, src, dst).unwrap();
                for i in 0..dst {
                    let (weights, _indices) = table.row(i);
                    let sum: f32 = weights.iter().sum();
                    assert!(
                        (sum - 1.0).abs() < 1e-5,
                        "{filter:?} {src}->{dst} row {i} sum={sum}"
                    );
                }
            }
        }
    }

    #[test]
    fn indices_are_always_in_bounds() {
        for filter in [Filter::Lanczos3, Filter::Mitchell] {
            let table = WeightTable::build(filter, 5, 2).unwrap();
            for i in 0..table.dst_len() {
                let (_weights, indices) = table.row(i);
                for &idx in indices {
                    assert!((idx as usize) < table.src_len());
                }
            }
        }
    }

    #[test]
    fn identity_scale_concentrates_weight_near_source_index() {
        let table = WeightTable::build(Filter::Lanczos3, 16, 16).unwrap();
        for i in 0..16usize {
            let (weights, indices) = table.row(i);
            // The tap closest to `i` should carry the largest weight.
            let (best_idx, _) = weights
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap();
            assert_eq!(indices[best_idx] as usize, i);
        }
    }

    #[test]
    fn downscale_widens_support_beyond_native_radius() {
        // 4x downscale => filter_scale = 4.0, so Lanczos3's effective support
        // in source-sample units is 12, i.e. spans well beyond its native
        // radius-3 window.
        let table = WeightTable::build(Filter::Lanczos3, 64, 16).unwrap();
        assert!(table.span() > 2 * 3 + 2);
    }

    #[test]
    fn interior_base_matches_clamped_indices_exactly() {
        // Wherever `is_interior` is true, the fast-path base+k addressing
        // must agree, tap-for-tap, with the always-correct clamped indices
        // path — this is the invariant `Resizer::run_passes` relies on to
        // skip the indices array entirely for interior samples.
        for filter in [
            Filter::Bilinear,
            Filter::CatmullRom,
            Filter::Mitchell,
            Filter::Lanczos3,
        ] {
            for (src, dst) in [(64, 64), (64, 32), (32, 64), (641, 123), (9, 9)] {
                let table = WeightTable::build(filter, src, dst).unwrap();
                for i in 0..dst {
                    if !table.is_interior(i) {
                        continue;
                    }
                    let base = table.base(i);
                    assert!(base >= 0, "{filter:?} {src}->{dst} row {i} base={base}");
                    let (_weights, indices) = table.row(i);
                    for (k, &idx) in indices.iter().enumerate() {
                        assert_eq!(
                            idx as i64,
                            base as i64 + k as i64,
                            "{filter:?} {src}->{dst} row {i} tap {k}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn every_row_is_interior_or_boundary_consistently() {
        // Every table has at least one interior row for any reasonably
        // sized axis (only the first/last `span/2`-ish rows near each edge
        // should be boundary rows).
        let table = WeightTable::build(Filter::Lanczos3, 64, 64).unwrap();
        let interior_count = (0..table.dst_len()).filter(|&i| table.is_interior(i)).count();
        assert!(interior_count > 0);
        assert!(interior_count < table.dst_len());
    }
}
