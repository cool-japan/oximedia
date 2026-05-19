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

// ---------------------------------------------------------------------------
// Hadamard transforms (DC blocks of I_16x16 macroblocks)
// ---------------------------------------------------------------------------

/// 4-point 1-D Hadamard transform.
///
/// Used by the `I_16x16` luma DC path.  The 4×4 Hadamard is its own
/// inverse up to a factor of 16, so the 1-D pass is identical for
/// forward and inverse — the spec just uses one of them with a
/// post-transform scaling shift that's not present in the other
/// direction.
#[must_use]
pub fn hadamard_1d_4(x: [i32; 4]) -> [i32; 4] {
    let a = x[0] + x[1];
    let b = x[0] - x[1];
    let c = x[2] + x[3];
    let d = x[2] - x[3];
    [a + c, a - c, b - d, b + d]
}

/// 2-D 4×4 Hadamard transform: row pass followed by column pass.
#[must_use]
pub fn hadamard_4x4(coeffs: &[[i32; 4]; 4]) -> [[i32; 4]; 4] {
    let mut intermediate = [[0i32; 4]; 4];
    for i in 0..4 {
        intermediate[i] = hadamard_1d_4(coeffs[i]);
    }
    let mut output = [[0i32; 4]; 4];
    for j in 0..4 {
        let col = [
            intermediate[0][j],
            intermediate[1][j],
            intermediate[2][j],
            intermediate[3][j],
        ];
        let result = hadamard_1d_4(col);
        for i in 0..4 {
            output[i][j] = result[i];
        }
    }
    output
}

/// Inverse-transform and dequantize the 4×4 luma DC block of an
/// `I_16x16` macroblock.
///
/// The 16 DC coefficients of the 16 4×4 luma sub-blocks of an
/// `I_16x16` macroblock are encoded together as a separate 4×4
/// Hadamard-transformed block.  After entropy decoding, this function
/// runs the inverse Hadamard, then applies a QP-dependent scaling
/// that differs from the regular 4×4 path: the shift is biased by an
/// extra factor that accounts for the Hadamard's lack of internal
/// normalisation.
///
/// The returned 4×4 matrix holds the dequantized DC values to place
/// at position `(0, 0)` of each of the 16 sub-blocks' coefficient
/// matrices before their individual inverse transforms.
#[must_use]
pub fn inverse_hadamard_4x4_luma_dc(
    coeffs: &[[i32; 4]; 4],
    qp: u8,
) -> [[i32; 4]; 4] {
    let transformed = hadamard_4x4(coeffs);
    let qp_div = i32::from(qp / 6);
    let scale = level_scale_4x4(qp, 0, 0);
    let mut output = [[0i32; 4]; 4];
    if qp >= 36 {
        let shift = qp_div - 6;
        for i in 0..4 {
            for j in 0..4 {
                output[i][j] = (transformed[i][j] * scale) << shift;
            }
        }
    } else {
        let shift = 6 - qp_div;
        let round = 1i32 << (shift - 1);
        for i in 0..4 {
            for j in 0..4 {
                output[i][j] = (transformed[i][j] * scale + round) >> shift;
            }
        }
    }
    output
}

/// 2-D 2×2 Hadamard transform for the chroma DC block (4:2:0).
///
/// The four DC coefficients of the four 4×4 sub-blocks of one chroma
/// component are encoded together as a 2×2 Hadamard-transformed
/// block.  The 2×2 Hadamard matrix is `[[1, 1], [1, -1]]`, applied
/// from both sides.
#[must_use]
pub fn hadamard_2x2(coeffs: [[i32; 2]; 2]) -> [[i32; 2]; 2] {
    let a = coeffs[0][0] + coeffs[0][1];
    let b = coeffs[0][0] - coeffs[0][1];
    let c = coeffs[1][0] + coeffs[1][1];
    let d = coeffs[1][0] - coeffs[1][1];
    [[a + c, b + d], [a - c, b - d]]
}

/// Inverse-transform and dequantize the 2×2 chroma DC block.
///
/// `qp_chroma` is the chroma QP (already adjusted by
/// `chroma_qp_index_offset` from the active PPS).
#[must_use]
pub fn inverse_hadamard_2x2_chroma_dc(
    coeffs: [[i32; 2]; 2],
    qp_chroma: u8,
) -> [[i32; 2]; 2] {
    let transformed = hadamard_2x2(coeffs);
    let qp_div = i32::from(qp_chroma / 6);
    let scale = level_scale_4x4(qp_chroma, 0, 0);
    let mut output = [[0i32; 2]; 2];
    for i in 0..2 {
        for j in 0..2 {
            output[i][j] = ((transformed[i][j] * scale) << qp_div) >> 1;
        }
    }
    output
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

    // -- Hadamard transform tests --

    #[test]
    fn hadamard_1d_zero_input_yields_zero_output() {
        assert_eq!(hadamard_1d_4([0, 0, 0, 0]), [0, 0, 0, 0]);
    }

    #[test]
    fn hadamard_1d_dc_only_concentrates_at_index_zero() {
        // [k, 0, 0, 0]: a = k, b = k, c = 0, d = 0
        // -> [a+c, a-c, b-d, b+d] = [k, k, k, k]
        assert_eq!(hadamard_1d_4([4, 0, 0, 0]), [4, 4, 4, 4]);
    }

    #[test]
    fn hadamard_1d_constant_input_concentrates_at_dc() {
        // [k, k, k, k]: a = 2k, b = 0, c = 2k, d = 0
        // -> [4k, 0, 0, 0]
        assert_eq!(hadamard_1d_4([3, 3, 3, 3]), [12, 0, 0, 0]);
    }

    #[test]
    fn hadamard_4x4_is_self_inverse_up_to_scale() {
        let input = [
            [1, 2, 3, 4],
            [5, 6, 7, 8],
            [9, 10, 11, 12],
            [13, 14, 15, 16],
        ];
        let h = hadamard_4x4(&input);
        let back = hadamard_4x4(&h);
        // 4×4 Hadamard squared scales by 16.
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(back[i][j], input[i][j] * 16, "({i}, {j})");
            }
        }
    }

    #[test]
    fn inverse_hadamard_4x4_luma_dc_zero_input_stays_zero() {
        let result = inverse_hadamard_4x4_luma_dc(&[[0i32; 4]; 4], 28);
        for i in 0..4 {
            for j in 0..4 {
                assert_eq!(result[i][j], 0);
            }
        }
    }

    #[test]
    fn inverse_hadamard_4x4_luma_dc_works_at_both_qp_branches() {
        // Same input, two QPs on opposite sides of 36 — both should
        // produce defined output (we don't pin the exact values here;
        // this just exercises both code paths).
        let block = [[1, -1, 1, -1], [1, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]];
        let low_qp = inverse_hadamard_4x4_luma_dc(&block, 20);
        let high_qp = inverse_hadamard_4x4_luma_dc(&block, 40);
        // Sanity: not equal, but both finite.
        assert_ne!(low_qp, high_qp);
    }

    #[test]
    fn hadamard_2x2_constant_input_concentrates_at_dc() {
        let input = [[5, 5], [5, 5]];
        let out = hadamard_2x2(input);
        assert_eq!(out, [[20, 0], [0, 0]]);
    }

    #[test]
    fn hadamard_2x2_zero_input_stays_zero() {
        assert_eq!(hadamard_2x2([[0, 0], [0, 0]]), [[0, 0], [0, 0]]);
    }

    #[test]
    fn inverse_hadamard_2x2_chroma_dc_zero_input_stays_zero() {
        let result = inverse_hadamard_2x2_chroma_dc([[0, 0], [0, 0]], 20);
        assert_eq!(result, [[0, 0], [0, 0]]);
    }
}
