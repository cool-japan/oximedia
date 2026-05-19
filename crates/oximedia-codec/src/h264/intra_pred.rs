//! H.264 intra prediction.
//!
//! For an intra-coded macroblock, the decoder builds the prediction
//! from samples already decoded in the *current frame* — the row above
//! and the column to the left of the block being predicted, plus the
//! top-left corner sample and (for some 4×4 modes) the four samples
//! above-and-right of the current 4×4 block.
//!
//! This module covers the 4×4 luma path: the nine prediction modes
//! defined by H.264 plus the neighbour-availability rules that pick
//! fallback values when a required neighbour sample isn't yet
//! reconstructed (e.g. the top-left block of a slice has no top or
//! left neighbour at all).
//!
//! The 16×16 luma and 8×8 chroma intra modes are not yet implemented
//! — they share fewer modes (4 each) and are simpler in shape; they
//! will arrive in a follow-up.

/// One of the nine H.264 intra-4×4 prediction modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intra4x4Mode {
    /// Replicate the top neighbour row downward.
    Vertical,
    /// Replicate the left neighbour column rightward.
    Horizontal,
    /// Mean of available top and left samples.
    Dc,
    /// Project samples diagonally toward the bottom-left.
    DiagonalDownLeft,
    /// Project samples diagonally toward the bottom-right.
    DiagonalDownRight,
    /// Mostly vertical, leaning right.
    VerticalRight,
    /// Mostly horizontal, leaning down.
    HorizontalDown,
    /// Mostly vertical, leaning left.
    VerticalLeft,
    /// Mostly horizontal, leaning up.
    HorizontalUp,
}

/// Neighbour samples for one 4×4 intra prediction.
///
/// Use `None` for a side that is not yet reconstructed (slice
/// boundary, picture edge, or a constrained-intra-pred situation).
/// The DC mode and the directional modes fall back to spec-defined
/// defaults when the inputs they need are unavailable.
///
/// Geometry, with `top` running rightward from column 0 and `left`
/// running downward from row 0:
///
/// ```text
///                 top[0] top[1] top[2] top[3] top_right[0] top_right[1] top_right[2] top_right[3]
///   top_left ───────────────────────────────────────────
///   left[0]       p[0][0] p[0][1] p[0][2] p[0][3]
///   left[1]       p[1][0] p[1][1] p[1][2] p[1][3]
///   left[2]       p[2][0] p[2][1] p[2][2] p[2][3]
///   left[3]       p[3][0] p[3][1] p[3][2] p[3][3]
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct Intra4x4Neighbours {
    /// Top-left corner sample (one pixel diagonally up-left of `p[0][0]`).
    pub top_left: Option<u8>,
    /// Top neighbour row — `top[x]` is the sample directly above
    /// column `x` of the block being predicted.
    pub top: Option<[u8; 4]>,
    /// Top-right neighbour — four samples continuing rightward from
    /// `top[3]`.  Required by `DiagonalDownLeft`, `VerticalLeft`, and
    /// some pixels of other directional modes; when absent the
    /// modes that need it replicate `top[3]`.
    pub top_right: Option<[u8; 4]>,
    /// Left neighbour column — `left[y]` is the sample directly
    /// left of row `y` of the block being predicted.
    pub left: Option<[u8; 4]>,
}

/// Predicts a 4×4 luma block under the given mode.
///
/// Returns a 4×4 matrix in `[row][col]` order with values in `0..=255`.
#[must_use]
pub fn predict_4x4(mode: Intra4x4Mode, n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    match mode {
        Intra4x4Mode::Vertical => predict_vertical(n),
        Intra4x4Mode::Horizontal => predict_horizontal(n),
        Intra4x4Mode::Dc => predict_dc(n),
        Intra4x4Mode::DiagonalDownLeft => predict_diagonal_down_left(n),
        Intra4x4Mode::DiagonalDownRight => predict_diagonal_down_right(n),
        Intra4x4Mode::VerticalRight => predict_vertical_right(n),
        Intra4x4Mode::HorizontalDown => predict_horizontal_down(n),
        Intra4x4Mode::VerticalLeft => predict_vertical_left(n),
        Intra4x4Mode::HorizontalUp => predict_horizontal_up(n),
    }
}

fn fill_constant(value: u8) -> [[u8; 4]; 4] {
    [[value; 4]; 4]
}

fn predict_vertical(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let top = match n.top {
        Some(t) => t,
        None => return fill_constant(128),
    };
    let mut out = [[0u8; 4]; 4];
    for (row, dst) in out.iter_mut().enumerate() {
        let _ = row;
        *dst = top;
    }
    out
}

fn predict_horizontal(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let left = match n.left {
        Some(l) => l,
        None => return fill_constant(128),
    };
    let mut out = [[0u8; 4]; 4];
    for (y, row) in out.iter_mut().enumerate() {
        let v = left[y];
        *row = [v; 4];
    }
    out
}

fn predict_dc(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let dc = match (n.top, n.left) {
        (Some(t), Some(l)) => {
            let sum: u32 = t.iter().chain(l.iter()).map(|&v| u32::from(v)).sum();
            ((sum + 4) >> 3) as u8
        }
        (Some(t), None) => {
            let sum: u32 = t.iter().map(|&v| u32::from(v)).sum();
            ((sum + 2) >> 2) as u8
        }
        (None, Some(l)) => {
            let sum: u32 = l.iter().map(|&v| u32::from(v)).sum();
            ((sum + 2) >> 2) as u8
        }
        (None, None) => 128,
    };
    fill_constant(dc)
}

/// Pads the top-row reference so directional modes can reach
/// columns 4..=7 even when the top-right neighbour is unavailable.
/// In that case the spec replicates `top[3]` rightward.
fn top_extended(n: &Intra4x4Neighbours) -> Option<[u8; 8]> {
    let top = n.top?;
    let mut ext = [0u8; 8];
    ext[..4].copy_from_slice(&top);
    let right = n.top_right.unwrap_or([top[3]; 4]);
    ext[4..].copy_from_slice(&right);
    Some(ext)
}

fn predict_diagonal_down_left(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let t = match top_extended(n) {
        Some(t) => t,
        None => return fill_constant(128),
    };
    // p[x, y] = (t[x + y] + 2·t[x + y + 1] + t[x + y + 2] + 2) >> 2
    // except (x, y) = (3, 3): use (t[6] + 3·t[7] + 2) >> 2.
    let mut out = [[0u8; 4]; 4];
    for y in 0..4 {
        for x in 0..4 {
            let v = if x == 3 && y == 3 {
                (u32::from(t[6]) + 3 * u32::from(t[7]) + 2) >> 2
            } else {
                let k = x + y;
                (u32::from(t[k]) + 2 * u32::from(t[k + 1]) + u32::from(t[k + 2]) + 2) >> 2
            };
            out[y][x] = v as u8;
        }
    }
    out
}

fn predict_diagonal_down_right(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    // Requires top, left, and top-left.
    let (top, left, tl) = match (n.top, n.left, n.top_left) {
        (Some(t), Some(l), Some(p)) => (t, l, p),
        _ => return fill_constant(128),
    };
    // Build a single "stair" reference of 9 samples that runs from
    // bottom of left, up the left, across the top-left, then along
    // the top.  Indexing s[k]: k=0 -> left[3], k=4 -> top_left,
    // k=5..=8 -> top[0..=3].
    let s = [
        left[3], left[2], left[1], left[0], tl, top[0], top[1], top[2], top[3],
    ];
    let mut out = [[0u8; 4]; 4];
    for y in 0..4 {
        for x in 0..4 {
            // Map (x, y) to a position k along the stair.  When
            // (x, y) is on the (top-left → bottom-right) diagonal,
            // k = 4 (top-left).  Off-diagonal positions step
            // up-left or down-right along the stair.
            let centre = 4i32 + (x as i32) - (y as i32);
            // Three-tap filter along the stair: s[k-1], s[k], s[k+1].
            let a = s[(centre - 1) as usize];
            let b = s[centre as usize];
            let c = s[(centre + 1) as usize];
            let v = (u32::from(a) + 2 * u32::from(b) + u32::from(c) + 2) >> 2;
            out[y][x] = v as u8;
        }
    }
    out
}

fn predict_vertical_right(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let (top, left, tl) = match (n.top, n.left, n.top_left) {
        (Some(t), Some(l), Some(p)) => (t, l, p),
        _ => return fill_constant(128),
    };
    let s = [
        left[3], left[2], left[1], left[0], tl, top[0], top[1], top[2], top[3],
    ];
    let mut out = [[0u8; 4]; 4];
    for y in 0..4 {
        for x in 0..4 {
            // zVR = 2x - y (from H.264 spec).  Pixels on the "even"
            // zVR get the 2-tap average of two adjacent stair
            // samples; pixels on the "odd" zVR get the 3-tap
            // average.  Negative zVR pixels use an alternate stair
            // formula.
            let zvr = 2 * (x as i32) - (y as i32);
            let v: u32 = match zvr {
                0 | 2 | 4 | 6 => {
                    let k = (4 + x as i32 - (y as i32 / 2)) as usize;
                    (u32::from(s[k - 1]) + u32::from(s[k]) + 1) >> 1
                }
                1 | 3 | 5 => {
                    let k = (4 + x as i32 - (y as i32 / 2)) as usize;
                    (u32::from(s[k - 2])
                        + 2 * u32::from(s[k - 1])
                        + u32::from(s[k])
                        + 2)
                        >> 2
                }
                -1 => (u32::from(left[0]) + 2 * u32::from(tl) + u32::from(top[0]) + 2) >> 2,
                -2 => (u32::from(left[1]) + 2 * u32::from(left[0]) + u32::from(tl) + 2) >> 2,
                -3 => (u32::from(left[2]) + 2 * u32::from(left[1]) + u32::from(left[0]) + 2) >> 2,
                _ => 128, // unreachable in a 4×4 block
            };
            out[y][x] = v as u8;
        }
    }
    out
}

fn predict_horizontal_down(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let (top, left, tl) = match (n.top, n.left, n.top_left) {
        (Some(t), Some(l), Some(p)) => (t, l, p),
        _ => return fill_constant(128),
    };
    // Stair samples running from top-left down through the left
    // column: s[0]=tl, s[1]=left[0], s[2]=left[1], s[3]=left[2],
    // s[4]=left[3].
    let s = [tl, left[0], left[1], left[2], left[3]];
    let mut out = [[0u8; 4]; 4];
    for y in 0..4 {
        for x in 0..4 {
            let zhd = 2 * (y as i32) - (x as i32);
            let v: u32 = if zhd >= 0 {
                let k = (y as i32 - (x as i32 / 2)) as usize;
                if zhd % 2 == 0 {
                    // 2-tap average of two adjacent stair samples.
                    (u32::from(s[k]) + u32::from(s[k + 1]) + 1) >> 1
                } else {
                    // 3-tap weighted average centred on s[k].  k is
                    // always ≥ 1 on the odd-zhd path inside a 4×4
                    // block, so s[k-1] is in bounds.
                    (u32::from(s[k - 1])
                        + 2 * u32::from(s[k])
                        + u32::from(s[k + 1])
                        + 2)
                        >> 2
                }
            } else {
                // zhd < 0: along the top row above the block.
                match zhd {
                    -1 => (u32::from(left[0]) + 2 * u32::from(tl) + u32::from(top[0]) + 2) >> 2,
                    -2 => (u32::from(tl) + 2 * u32::from(top[0]) + u32::from(top[1]) + 2) >> 2,
                    -3 => (u32::from(top[0]) + 2 * u32::from(top[1]) + u32::from(top[2]) + 2) >> 2,
                    _ => 128,
                }
            };
            out[y][x] = v as u8;
        }
    }
    out
}

fn predict_vertical_left(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let t = match top_extended(n) {
        Some(t) => t,
        None => return fill_constant(128),
    };
    let mut out = [[0u8; 4]; 4];
    for y in 0..4 {
        for x in 0..4 {
            let v: u32 = match y {
                0 => (u32::from(t[x]) + u32::from(t[x + 1]) + 1) >> 1,
                1 => (u32::from(t[x]) + 2 * u32::from(t[x + 1]) + u32::from(t[x + 2]) + 2) >> 2,
                2 => (u32::from(t[x + 1]) + u32::from(t[x + 2]) + 1) >> 1,
                3 => (u32::from(t[x + 1]) + 2 * u32::from(t[x + 2]) + u32::from(t[x + 3]) + 2) >> 2,
                _ => 128,
            };
            out[y][x] = v as u8;
        }
    }
    out
}

fn predict_horizontal_up(n: &Intra4x4Neighbours) -> [[u8; 4]; 4] {
    let left = match n.left {
        Some(l) => l,
        None => return fill_constant(128),
    };
    let mut out = [[0u8; 4]; 4];
    for y in 0..4 {
        for x in 0..4 {
            let zhu = (x as i32) + 2 * (y as i32);
            let v: u32 = match zhu {
                0 => (u32::from(left[0]) + u32::from(left[1]) + 1) >> 1,
                1 => (u32::from(left[0]) + 2 * u32::from(left[1]) + u32::from(left[2]) + 2) >> 2,
                2 => (u32::from(left[1]) + u32::from(left[2]) + 1) >> 1,
                3 => (u32::from(left[1]) + 2 * u32::from(left[2]) + u32::from(left[3]) + 2) >> 2,
                4 => (u32::from(left[2]) + u32::from(left[3]) + 1) >> 1,
                5 => (u32::from(left[2]) + 3 * u32::from(left[3]) + 2) >> 2,
                _ => u32::from(left[3]),
            };
            out[y][x] = v as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn neighbours_with_all(top: [u8; 4], left: [u8; 4], tl: u8) -> Intra4x4Neighbours {
        Intra4x4Neighbours {
            top_left: Some(tl),
            top: Some(top),
            top_right: Some([top[3]; 4]),
            left: Some(left),
        }
    }

    #[test]
    fn vertical_replicates_top_row() {
        let n = neighbours_with_all([10, 20, 30, 40], [50, 60, 70, 80], 5);
        let pred = predict_4x4(Intra4x4Mode::Vertical, &n);
        for y in 0..4 {
            assert_eq!(pred[y], [10, 20, 30, 40], "row {y}");
        }
    }

    #[test]
    fn vertical_with_no_top_returns_default() {
        let n = Intra4x4Neighbours {
            top_left: Some(5),
            top: None,
            top_right: None,
            left: Some([50, 60, 70, 80]),
        };
        let pred = predict_4x4(Intra4x4Mode::Vertical, &n);
        for row in &pred {
            assert_eq!(*row, [128, 128, 128, 128]);
        }
    }

    #[test]
    fn horizontal_replicates_left_column() {
        let n = neighbours_with_all([10, 20, 30, 40], [50, 60, 70, 80], 5);
        let pred = predict_4x4(Intra4x4Mode::Horizontal, &n);
        for y in 0..4 {
            assert_eq!(pred[y], [n.left.unwrap()[y]; 4]);
        }
    }

    #[test]
    fn horizontal_with_no_left_returns_default() {
        let n = Intra4x4Neighbours {
            top_left: Some(5),
            top: Some([10, 20, 30, 40]),
            top_right: None,
            left: None,
        };
        let pred = predict_4x4(Intra4x4Mode::Horizontal, &n);
        for row in &pred {
            assert_eq!(*row, [128, 128, 128, 128]);
        }
    }

    #[test]
    fn dc_averages_top_and_left_when_both_present() {
        let n = neighbours_with_all([16, 16, 16, 16], [16, 16, 16, 16], 16);
        let pred = predict_4x4(Intra4x4Mode::Dc, &n);
        // (4*16 + 4*16 + 4) >> 3 = (128 + 4) >> 3 = 16
        assert_eq!(pred, [[16; 4]; 4]);
    }

    #[test]
    fn dc_averages_top_only_when_left_absent() {
        let n = Intra4x4Neighbours {
            top_left: None,
            top: Some([20, 20, 20, 20]),
            top_right: None,
            left: None,
        };
        let pred = predict_4x4(Intra4x4Mode::Dc, &n);
        // (4*20 + 2) >> 2 = 82 >> 2 = 20
        assert_eq!(pred, [[20; 4]; 4]);
    }

    #[test]
    fn dc_averages_left_only_when_top_absent() {
        let n = Intra4x4Neighbours {
            top_left: None,
            top: None,
            top_right: None,
            left: Some([30, 30, 30, 30]),
        };
        let pred = predict_4x4(Intra4x4Mode::Dc, &n);
        assert_eq!(pred, [[30; 4]; 4]);
    }

    #[test]
    fn dc_falls_back_to_128_with_no_neighbours() {
        let n = Intra4x4Neighbours::default();
        let pred = predict_4x4(Intra4x4Mode::Dc, &n);
        assert_eq!(pred, [[128; 4]; 4]);
    }

    #[test]
    fn dc_rounds_half_up() {
        // top sum = 4, left sum = 4, total = 8, (8 + 4) >> 3 = 1.
        let n = Intra4x4Neighbours {
            top_left: Some(0),
            top: Some([1, 1, 1, 1]),
            top_right: None,
            left: Some([1, 1, 1, 1]),
        };
        let pred = predict_4x4(Intra4x4Mode::Dc, &n);
        assert_eq!(pred, [[1; 4]; 4]);
    }

    #[test]
    fn diagonal_down_left_is_uniform_for_constant_top() {
        // All top samples equal -> all 3-tap filtered outputs equal.
        let n = neighbours_with_all([64, 64, 64, 64], [0, 0, 0, 0], 64);
        let pred = predict_4x4(Intra4x4Mode::DiagonalDownLeft, &n);
        assert_eq!(pred, [[64; 4]; 4]);
    }

    #[test]
    fn diagonal_down_left_falls_back_without_top() {
        let n = Intra4x4Neighbours::default();
        let pred = predict_4x4(Intra4x4Mode::DiagonalDownLeft, &n);
        assert_eq!(pred, [[128; 4]; 4]);
    }

    #[test]
    fn diagonal_down_right_is_uniform_when_all_neighbours_match() {
        let n = neighbours_with_all([50, 50, 50, 50], [50, 50, 50, 50], 50);
        let pred = predict_4x4(Intra4x4Mode::DiagonalDownRight, &n);
        assert_eq!(pred, [[50; 4]; 4]);
    }

    #[test]
    fn diagonal_down_right_falls_back_without_corner() {
        let n = Intra4x4Neighbours {
            top_left: None,
            top: Some([10, 10, 10, 10]),
            top_right: None,
            left: Some([10, 10, 10, 10]),
        };
        let pred = predict_4x4(Intra4x4Mode::DiagonalDownRight, &n);
        assert_eq!(pred, [[128; 4]; 4]);
    }

    #[test]
    fn vertical_right_uniform_with_constant_neighbours() {
        let n = neighbours_with_all([42, 42, 42, 42], [42, 42, 42, 42], 42);
        let pred = predict_4x4(Intra4x4Mode::VerticalRight, &n);
        assert_eq!(pred, [[42; 4]; 4]);
    }

    #[test]
    fn horizontal_down_uniform_with_constant_neighbours() {
        let n = neighbours_with_all([42, 42, 42, 42], [42, 42, 42, 42], 42);
        let pred = predict_4x4(Intra4x4Mode::HorizontalDown, &n);
        assert_eq!(pred, [[42; 4]; 4]);
    }

    #[test]
    fn vertical_left_uniform_with_constant_top() {
        let n = neighbours_with_all([7, 7, 7, 7], [0, 0, 0, 0], 7);
        let pred = predict_4x4(Intra4x4Mode::VerticalLeft, &n);
        assert_eq!(pred, [[7; 4]; 4]);
    }

    #[test]
    fn horizontal_up_uniform_with_constant_left() {
        let n = neighbours_with_all([0, 0, 0, 0], [9, 9, 9, 9], 0);
        let pred = predict_4x4(Intra4x4Mode::HorizontalUp, &n);
        assert_eq!(pred, [[9; 4]; 4]);
    }
}
