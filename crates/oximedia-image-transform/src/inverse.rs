// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! Analytical inverse of a 2-D affine transform.
//!
//! For a 3×3 homogeneous matrix of the form
//!
//! ```text
//! | a  b  tx |
//! | c  d  ty |
//! | 0  0  1  |
//! ```
//!
//! the inverse exists when `det = a*d - b*c ≠ 0`.  This module computes
//! the inverse analytically without any external dependencies.
//!
//! # Example
//!
//! ```rust
//! use oximedia_image_transform::compose::TransformMatrix;
//! use oximedia_image_transform::inverse::invert_transform;
//!
//! let m = TransformMatrix::translation(4.0, -2.0);
//! let inv = invert_transform(&m).expect("translation is always invertible");
//! let pt = inv.transform_point(4.0, -2.0);  // undo the translation
//! assert!((pt.0).abs() < 1e-9);
//! assert!((pt.1).abs() < 1e-9);
//! ```

use crate::compose::TransformMatrix;

/// Determinant tolerance below which a matrix is considered singular.
const DET_EPSILON: f64 = 1e-12;

/// Compute the analytical inverse of a 2-D affine [`TransformMatrix`].
///
/// The input must be of the standard 3×3 affine form (last row `[0, 0, 1]`).
/// If the linear part `[[a, b], [c, d]]` is singular (determinant ≈ 0),
/// `None` is returned.
///
/// # Arguments
///
/// * `m` — The affine transform matrix to invert.
///
/// # Returns
///
/// `Some(inverse)` when the matrix is invertible, `None` when it is singular.
#[must_use]
pub fn invert_transform(m: &TransformMatrix) -> Option<TransformMatrix> {
    // Unpack the linear 2×2 part and translation vector.
    let a = m.at(0, 0);
    let b = m.at(0, 1);
    let tx = m.at(0, 2);
    let c = m.at(1, 0);
    let d = m.at(1, 1);
    let ty = m.at(1, 2);

    // Determinant of the 2×2 linear part.
    let det = a * d - b * c;
    if det.abs() < DET_EPSILON {
        return None;
    }

    let inv_det = 1.0 / det;

    // Inverse linear part (adjugate / det).
    let ia = d * inv_det;
    let ib = -b * inv_det;
    let ic = -c * inv_det;
    let id = a * inv_det;

    // Inverse translation: -A⁻¹ · t.
    let itx = -(ia * tx + ib * ty);
    let ity = -(ic * tx + id * ty);

    Some(TransformMatrix([ia, ib, itx, ic, id, ity, 0.0, 0.0, 1.0]))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::{compose_transforms, TransformMatrix};
    use std::f64::consts::PI;

    const EPS: f64 = 1e-9;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    fn is_identity(m: &TransformMatrix) -> bool {
        let id = TransformMatrix::identity();
        for i in 0..9 {
            if (m.0[i] - id.0[i]).abs() >= 1e-8 {
                return false;
            }
        }
        true
    }

    // --- Round-trip tests (M * M⁻¹ ≈ I) ---

    #[test]
    fn test_inverse_identity() {
        let m = TransformMatrix::identity();
        let inv = invert_transform(&m).expect("identity is invertible");
        let product = compose_transforms(&m, &inv);
        assert!(
            is_identity(&product),
            "I * I⁻¹ should be I, got {:?}",
            product
        );
    }

    #[test]
    fn test_inverse_translation() {
        let m = TransformMatrix::translation(5.0, -3.0);
        let inv = invert_transform(&m).expect("translation is invertible");
        // Apply original then inverse: point should return to origin.
        let pt = inv.transform_point(5.0, -3.0);
        assert!(approx_eq(pt.0, 0.0));
        assert!(approx_eq(pt.1, 0.0));
    }

    #[test]
    fn test_inverse_uniform_scale() {
        let m = TransformMatrix::scale(4.0, 4.0);
        let inv = invert_transform(&m).expect("non-zero scale is invertible");
        let product = compose_transforms(&m, &inv);
        assert!(is_identity(&product));
    }

    #[test]
    fn test_inverse_non_uniform_scale() {
        let m = TransformMatrix::scale(2.0, 0.5);
        let inv = invert_transform(&m).expect("invertible");
        let product = compose_transforms(&m, &inv);
        assert!(is_identity(&product));
    }

    #[test]
    fn test_inverse_rotation() {
        let m = TransformMatrix::rotation(PI / 6.0);
        let inv = invert_transform(&m).expect("rotation is invertible");
        let product = compose_transforms(&m, &inv);
        assert!(is_identity(&product));
    }

    #[test]
    fn test_inverse_combined_scale_translation() {
        let scale = TransformMatrix::scale(3.0, 2.0);
        let translate = TransformMatrix::translation(7.0, -4.0);
        let combined = compose_transforms(&scale, &translate);
        let inv = invert_transform(&combined).expect("invertible");
        let product = compose_transforms(&combined, &inv);
        assert!(is_identity(&product));
    }

    // --- Singular matrix ---

    #[test]
    fn test_singular_zero_scale_returns_none() {
        let m = TransformMatrix::scale(0.0, 1.0);
        assert!(invert_transform(&m).is_none(), "zero scale is singular");
    }

    #[test]
    fn test_singular_both_axes_zero() {
        let m = TransformMatrix::scale(0.0, 0.0);
        assert!(invert_transform(&m).is_none());
    }

    // --- Point round-trip ---

    #[test]
    fn test_point_roundtrip_via_inverse() {
        let m = TransformMatrix::scale(5.0, 0.2);
        let inv = invert_transform(&m).expect("invertible");
        let original = (3.0_f64, 8.0_f64);
        let transformed = m.transform_point(original.0, original.1);
        let recovered = inv.transform_point(transformed.0, transformed.1);
        assert!(approx_eq(recovered.0, original.0));
        assert!(approx_eq(recovered.1, original.1));
    }

    #[test]
    fn test_inverse_of_inverse_is_original() {
        let m = TransformMatrix::rotation(PI / 4.0);
        let inv = invert_transform(&m).expect("invertible");
        let inv_inv = invert_transform(&inv).expect("double-inverse exists");
        for i in 0..9 {
            assert!((inv_inv.0[i] - m.0[i]).abs() < 1e-8, "idx {i}");
        }
    }
}
