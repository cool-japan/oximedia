// SPDX-License-Identifier: Apache-2.0
// Copyright (c) COOLJAPAN OU (Team KitaSan)

//! 2-D affine transform composition.
//!
//! A 2-D affine transform is represented as a 3×3 homogeneous matrix
//! stored in row-major order:
//!
//! ```text
//! | a  b  tx |
//! | c  d  ty |
//! | 0  0  1  |
//! ```
//!
//! [`TransformMatrix`] wraps this as `[f64; 9]` with helpers for
//! construction and composition.  [`compose_transforms`] returns the
//! product `b × a` so that applying the result is equivalent to first
//! applying `a` and then applying `b` (standard convention for column
//! vectors).
//!
//! # Example
//!
//! ```rust
//! use oximedia_image_transform::compose::{TransformMatrix, compose_transforms};
//!
//! let t1 = TransformMatrix::translation(10.0, 20.0);
//! let t2 = TransformMatrix::scale(2.0, 2.0);
//! let combined = compose_transforms(&t2, &t1);
//!
//! // The combined transform scales first, then translates.
//! let pt = combined.transform_point(1.0, 1.0);
//! assert!((pt.0 - 12.0).abs() < 1e-9);
//! assert!((pt.1 - 22.0).abs() < 1e-9);
//! ```

// ---------------------------------------------------------------------------
// TransformMatrix
// ---------------------------------------------------------------------------

/// A 3×3 row-major homogeneous matrix representing a 2-D affine transform.
///
/// Layout (indices 0-8):
/// ```text
/// [ 0  1  2 ]     [ a  b  tx ]
/// [ 3  4  5 ]  =  [ c  d  ty ]
/// [ 6  7  8 ]     [ 0  0  1  ]
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformMatrix(pub [f64; 9]);

impl TransformMatrix {
    /// Identity transform (no operation).
    ///
    /// ```rust
    /// use oximedia_image_transform::compose::TransformMatrix;
    ///
    /// let m = TransformMatrix::identity();
    /// let pt = m.transform_point(3.0, 7.0);
    /// assert!((pt.0 - 3.0).abs() < 1e-12);
    /// assert!((pt.1 - 7.0).abs() < 1e-12);
    /// ```
    #[must_use]
    pub fn identity() -> Self {
        Self([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
    }

    /// Translation by `(tx, ty)`.
    ///
    /// ```rust
    /// use oximedia_image_transform::compose::TransformMatrix;
    ///
    /// let m = TransformMatrix::translation(5.0, -3.0);
    /// let pt = m.transform_point(0.0, 0.0);
    /// assert!((pt.0 - 5.0).abs() < 1e-12);
    /// assert!((pt.1 + 3.0).abs() < 1e-12);
    /// ```
    #[must_use]
    pub fn translation(tx: f64, ty: f64) -> Self {
        Self([1.0, 0.0, tx, 0.0, 1.0, ty, 0.0, 0.0, 1.0])
    }

    /// Uniform or non-uniform scaling about the origin.
    ///
    /// ```rust
    /// use oximedia_image_transform::compose::TransformMatrix;
    ///
    /// let m = TransformMatrix::scale(3.0, 0.5);
    /// let pt = m.transform_point(2.0, 4.0);
    /// assert!((pt.0 - 6.0).abs() < 1e-12);
    /// assert!((pt.1 - 2.0).abs() < 1e-12);
    /// ```
    #[must_use]
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self([sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 1.0])
    }

    /// Counter-clockwise rotation by `angle_radians` about the origin.
    #[must_use]
    pub fn rotation(angle_radians: f64) -> Self {
        let cos = angle_radians.cos();
        let sin = angle_radians.sin();
        Self([cos, -sin, 0.0, sin, cos, 0.0, 0.0, 0.0, 1.0])
    }

    /// Apply the transform to the 2-D point `(x, y)`.
    ///
    /// Uses homogeneous coordinates: `[x, y, 1]`.
    #[must_use]
    pub fn transform_point(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        let out_x = m[0] * x + m[1] * y + m[2];
        let out_y = m[3] * x + m[4] * y + m[5];
        (out_x, out_y)
    }

    /// Access element at row `r`, column `c` (0-based).
    #[must_use]
    pub fn at(&self, r: usize, c: usize) -> f64 {
        self.0[r * 3 + c]
    }
}

impl Default for TransformMatrix {
    fn default() -> Self {
        Self::identity()
    }
}

// ---------------------------------------------------------------------------
// compose_transforms
// ---------------------------------------------------------------------------

/// Compose two affine transforms into one.
///
/// Returns `b × a` — i.e. the matrix that first applies `a` and then applies
/// `b`.  This is the standard matrix-multiplication convention for
/// column-vector transforms.
///
/// # Example
///
/// ```rust
/// use oximedia_image_transform::compose::{TransformMatrix, compose_transforms};
///
/// let scale = TransformMatrix::scale(2.0, 2.0);
/// let translate = TransformMatrix::translation(10.0, 5.0);
/// // "scale then translate"
/// let combined = compose_transforms(&scale, &translate);
/// let pt = combined.transform_point(1.0, 1.0);
/// assert!((pt.0 - 12.0).abs() < 1e-9);
/// assert!((pt.1 - 7.0).abs() < 1e-9);
/// ```
#[must_use]
pub fn compose_transforms(a: &TransformMatrix, b: &TransformMatrix) -> TransformMatrix {
    let p = &a.0;
    let q = &b.0;
    let mut out = [0.0f64; 9];
    for row in 0..3 {
        for col in 0..3 {
            let mut sum = 0.0f64;
            for k in 0..3 {
                sum += q[row * 3 + k] * p[k * 3 + col];
            }
            out[row * 3 + col] = sum;
        }
    }
    TransformMatrix(out)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    const EPS: f64 = 1e-9;

    fn approx_eq(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn test_identity_is_neutral() {
        let id = TransformMatrix::identity();
        let pt = id.transform_point(7.0, 3.0);
        assert!(approx_eq(pt.0, 7.0));
        assert!(approx_eq(pt.1, 3.0));
    }

    #[test]
    fn test_translation() {
        let m = TransformMatrix::translation(4.0, -2.0);
        let pt = m.transform_point(1.0, 1.0);
        assert!(approx_eq(pt.0, 5.0));
        assert!(approx_eq(pt.1, -1.0));
    }

    #[test]
    fn test_scale() {
        let m = TransformMatrix::scale(3.0, 0.5);
        let pt = m.transform_point(2.0, 4.0);
        assert!(approx_eq(pt.0, 6.0));
        assert!(approx_eq(pt.1, 2.0));
    }

    #[test]
    fn test_rotation_90_degrees() {
        let m = TransformMatrix::rotation(PI / 2.0);
        let pt = m.transform_point(1.0, 0.0);
        assert!(approx_eq(pt.0, 0.0));
        assert!(approx_eq(pt.1, 1.0));
    }

    #[test]
    fn test_compose_scale_then_translate() {
        let scale = TransformMatrix::scale(2.0, 2.0);
        let translate = TransformMatrix::translation(10.0, 5.0);
        let combined = compose_transforms(&scale, &translate);
        let pt = combined.transform_point(1.0, 1.0);
        assert!(approx_eq(pt.0, 12.0));
        assert!(approx_eq(pt.1, 7.0));
    }

    #[test]
    fn test_compose_identity_neutral() {
        let id = TransformMatrix::identity();
        let scale = TransformMatrix::scale(3.0, 3.0);
        let result = compose_transforms(&scale, &id);
        // id × scale = scale
        let pt = result.transform_point(2.0, 2.0);
        assert!(approx_eq(pt.0, 6.0));
        assert!(approx_eq(pt.1, 6.0));
    }

    #[test]
    fn test_compose_two_translations() {
        let t1 = TransformMatrix::translation(3.0, 0.0);
        let t2 = TransformMatrix::translation(0.0, 7.0);
        let combined = compose_transforms(&t1, &t2);
        let pt = combined.transform_point(0.0, 0.0);
        assert!(approx_eq(pt.0, 3.0));
        assert!(approx_eq(pt.1, 7.0));
    }

    #[test]
    fn test_at_accessor() {
        let m = TransformMatrix::translation(5.0, 3.0);
        // tx is at row 0, col 2
        assert!(approx_eq(m.at(0, 2), 5.0));
        // ty is at row 1, col 2
        assert!(approx_eq(m.at(1, 2), 3.0));
        // bottom-right homogeneous is 1
        assert!(approx_eq(m.at(2, 2), 1.0));
    }

    #[test]
    fn test_default_is_identity() {
        let m = TransformMatrix::default();
        let pt = m.transform_point(5.0, -2.0);
        assert!(approx_eq(pt.0, 5.0));
        assert!(approx_eq(pt.1, -2.0));
    }
}
