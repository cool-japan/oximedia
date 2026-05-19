//! H.264 4×4 integer inverse transform and dequantization.
//!
//! Once the entropy decoder has recovered a block's quantized
//! coefficients in zigzag scan order, three steps separate them from
//! the pixel-domain residual that gets added back to the prediction:
//!
//! 1. **Inverse scan** — place the 1-D zigzag sequence into a 4×4
//!    matrix at the positions the encoder wrote them from.
//! 2. **Dequantization** — multiply each coefficient by a
//!    QP-dependent scaling factor that combines the inverse quantizer
//!    step with the inverse-transform normalisation constants.
//! 3. **Inverse 4×4 integer transform** — apply the H.264 integer-
//!    approximation IDCT, separable as two 1-D passes plus a final
//!    round-and-shift by 6 bits.
//!
//! This module implements all three for the ordinary 4×4 AC block
//! path (the one used by `I_NxN` macroblocks and the AC component of
//! `I_16x16`).  The separate Hadamard transforms used for the
//! `I_16x16` luma DC block (4×4) and chroma DC blocks (2×2 or 2×4)
//! are not yet implemented — they are smaller and will follow.

/// Group lookup for the H.264 4×4 dequantization `levelScale4x4` table.
///
/// The table is keyed by `(qp % 6, group)` where `group ∈ {0, 1, 2}`
/// is determined by the coefficient's `(row, col)` position:
///
/// - Group 0: corners `(0,0), (0,2), (2,0), (2,2)`.
/// - Group 1: corners `(1,1), (1,3), (3,1), (3,3)`.
/// - Group 2: all remaining positions.
///
/// Values from H.264 normAdjust4x4 table.
const LEVEL_SCALE_GROUPS: [[i32; 3]; 6] = [
    [10, 16, 13], // qp % 6 == 0
    [11, 18, 14], // qp % 6 == 1
    [13, 20, 16], // qp % 6 == 2
    [14, 23, 18], // qp % 6 == 3
    [16, 25, 20], // qp % 6 == 4
    [18, 29, 23], // qp % 6 == 5
];

/// Inverse zigzag scan order for 4×4 blocks (frame scan).
///
/// Index `k` gives the `(row, col)` position in the 4×4 matrix for
/// the `k`-th coefficient in scan order.
const INVERSE_ZIGZAG_4X4: [(usize, usize); 16] = [
    (0, 0), (0, 1), (1, 0), (2, 0),
    (1, 1), (0, 2), (0, 3), (1, 2),
    (2, 1), (3, 0), (3, 1), (2, 2),
    (1, 3), (2, 3), (3, 2), (3, 3),
];

/// Returns the level scale value for one coefficient position at the
/// given QP.
#[must_use]
pub fn level_scale_4x4(qp: u8, row: usize, col: usize) -> i32 {
    let group = match (row, col) {
        (0, 0) | (0, 2) | (2, 0) | (2, 2) => 0,
        (1, 1) | (1, 3) | (3, 1) | (3, 3) => 1,
        _ => 2,
    };
    LEVEL_SCALE_GROUPS[(qp % 6) as usize][group]
}

/// Places the 16 coefficients of a 4×4 block from zigzag scan order
/// into their natural 2-D positions.
#[must_use]
pub fn inverse_scan_4x4(coeffs_scan: &[i32; 16]) -> [[i32; 4]; 4] {
    let mut block = [[0i32; 4]; 4];
    for (k, &(i, j)) in INVERSE_ZIGZAG_4X4.iter().enumerate() {
        block[i][j] = coeffs_scan[k];
    }
    block
}

/// Dequantizes a 4×4 coefficient block in place.
///
/// Applies the standard H.264 path: multiply each coefficient by the
/// level scale, then either right-shift with rounding (for QP < 24)
/// or left-shift (for QP ≥ 24).
pub fn dequantize_4x4(coeffs: &mut [[i32; 4]; 4], qp: u8) {
    let qp_div = i32::from(qp / 6);
    if qp >= 24 {
        let shift = qp_div - 4;
        for i in 0..4 {
            for j in 0..4 {
                coeffs[i][j] = (coeffs[i][j] * level_scale_4x4(qp, i, j)) << shift;
            }
        }
    } else {
        let shift = 4 - qp_div;
        let round = 1i32 << (shift - 1);
        for i in 0..4 {
            for j in 0..4 {
                coeffs[i][j] =
                    (coeffs[i][j] * level_scale_4x4(qp, i, j) + round) >> shift;
            }
        }
    }
}

/// 4-point 1-D inverse integer transform.
///
/// Butterfly form: two "even" branches (sum / difference of the DC and
/// second AC) plus two "odd" branches with the `b/2`, `d/2` shifts.
/// The two branches are recombined to produce the four outputs.
#[must_use]
pub fn inverse_transform_1d_4(x: [i32; 4]) -> [i32; 4] {
    let e = x[0] + x[2];
    let f = x[0] - x[2];
    let g = (x[1] >> 1) - x[3];
    let h = x[1] + (x[3] >> 1);
    [e + h, f + g, f - g, e - h]
}

/// 4×4 inverse integer transform.
///
/// Applies the 1-D transform along rows, then along columns of the
/// intermediate matrix, then rounds with `(value + 32) >> 6` to
/// account for the 6 extra bits the integer butterfly introduces
/// across the two passes.
#[must_use]
pub fn inverse_transform_4x4(coeffs: &[[i32; 4]; 4]) -> [[i32; 4]; 4] {
    let mut intermediate = [[0i32; 4]; 4];
    for i in 0..4 {
        intermediate[i] = inverse_transform_1d_4(coeffs[i]);
    }
    let mut output = [[0i32; 4]; 4];
    for j in 0..4 {
        let col = [
            intermediate[0][j],
            intermediate[1][j],
            intermediate[2][j],
            intermediate[3][j],
        ];
        let result = inverse_transform_1d_4(col);
        for i in 0..4 {
            output[i][j] = (result[i] + 32) >> 6;
        }
    }
    output
}

/// Convenience entry point: inverse scan, dequantize, inverse
/// transform.  Suitable for the ordinary 4×4 AC block path used by
/// `I_NxN` macroblocks and the AC of `I_16x16`.  Not for `I_16x16`
/// luma DC nor chroma DC, which use Hadamard variants that this
/// module does not yet implement.
#[must_use]
pub fn dequant_and_inverse_transform_4x4(
    coeffs_scan: &[i32; 16],
    qp: u8,
) -> [[i32; 4]; 4] {
    let mut block = inverse_scan_4x4(coeffs_scan);
    dequantize_4x4(&mut block, qp);
    inverse_transform_4x4(&block)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_scale_corners_match_group_0() {
        assert_eq!(level_scale_4x4(0, 0, 0), 10);
        assert_eq!(level_scale_4x4(0, 0, 2), 10);
        assert_eq!(level_scale_4x4(0, 2, 0), 10);
        assert_eq!(level_scale_4x4(0, 2, 2), 10);
    }

    #[test]
    fn level_scale_group_1_diagonal() {
        assert_eq!(level_scale_4x4(0, 1, 1), 16);
        assert_eq!(level_scale_4x4(0, 1, 3), 16);
        assert_eq!(level_scale_4x4(0, 3, 1), 16);
        assert_eq!(level_scale_4x4(0, 3, 3), 16);
    }

    #[test]
    fn level_scale_group_2_off_diagonal() {
        assert_eq!(level_scale_4x4(0, 0, 1), 13);
        assert_eq!(level_scale_4x4(0, 1, 0), 13);
        assert_eq!(level_scale_4x4(0, 2, 1), 13);
    }

    #[test]
    fn level_scale_varies_with_qp_modulo_six() {
        assert_eq!(level_scale_4x4(0, 0, 0), 10);
        assert_eq!(level_scale_4x4(6, 0, 0), 10); // same group, same %6
        assert_eq!(level_scale_4x4(5, 0, 0), 18);
        assert_eq!(level_scale_4x4(11, 0, 0), 18);
    }

    #[test]
    fn inverse_scan_places_dc_at_origin() {
        let mut scan = [0i32; 16];
        scan[0] = 100;
        let block = inverse_scan_4x4(&scan);
        assert_eq!(block[0][0], 100);
        for i in 0..4 {
            for j in 0..4 {
                if (i, j) != (0, 0) {
                    assert_eq!(block[i][j], 0, "non-zero at ({i}, {j})");
                }
            }
        }
    }

    #[test]
    fn inverse_scan_reaches_high_frequency_corner() {
        let mut scan = [0i32; 16];
        scan[15] = -7;
        let block = inverse_scan_4x4(&scan);
        assert_eq!(block[3][3], -7);
    }

    #[test]
    fn inverse_1d_dc_only_yields_uniform() {
        assert_eq!(inverse_transform_1d_4([4, 0, 0, 0]), [4, 4, 4, 4]);
        assert_eq!(inverse_transform_1d_4([-2, 0, 0, 0]), [-2, -2, -2, -2]);
    }

    #[test]
    fn inverse_1d_zero_input_yields_zero_output() {
        assert_eq!(inverse_transform_1d_4([0, 0, 0, 0]), [0, 0, 0, 0]);
    }

    #[test]
    fn inverse_1d_symmetric_when_odd_branches_zero() {
        // [a, 0, c, 0]: only even branch contributes.
        let out = inverse_transform_1d_4([4, 0, 2, 0]);
        // e = 4+2 = 6, f = 4-2 = 2, g = 0, h = 0
        // -> [e+h, f+g, f-g, e-h] = [6, 2, 2, 6]
        assert_eq!(out, [6, 2, 2, 6]);
    }

    #[test]
    fn all_zero_block_round_trips_to_zero() {
        let scan = [0i32; 16];
        let result = dequant_and_inverse_transform_4x4(&scan, 26);
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(result[i][j], 0);
            }
        }
    }

    #[test]
    fn dequantize_zero_block_stays_zero() {
        let mut block = [[0i32; 4]; 4];
        dequantize_4x4(&mut block, 28);
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(block[i][j], 0);
            }
        }
    }

    #[test]
    fn inverse_2d_zero_yields_zero() {
        let block = [[0i32; 4]; 4];
        let result = inverse_transform_4x4(&block);
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(result[i][j], 0);
            }
        }
    }

    #[test]
    fn inverse_2d_dc_only_yields_uniform_after_shift() {
        // After the 1-D DC-only case yields a uniform vector, the
        // second pass of the 2-D transform applied to a uniform
        // matrix yields uniform output, scaled by the 1-D DC factor
        // and finally shifted by 6 bits with rounding.
        let mut block = [[0i32; 4]; 4];
        // 1-D DC = 64 along both axes after the input matrix has
        // a single non-zero in the (0, 0) position equal to 64:
        // first pass on row 0 turns 64 into [64, 64, 64, 64];
        // other rows stay zero; second pass on each column with
        // input [64, 0, 0, 0] yields [64, 64, 64, 64]; then
        // (64 + 32) >> 6 = 1.
        block[0][0] = 64;
        let result = inverse_transform_4x4(&block);
        let expected = 1;
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(result[i][j], expected, "unexpected value at ({i}, {j})");
            }
        }
    }
}
